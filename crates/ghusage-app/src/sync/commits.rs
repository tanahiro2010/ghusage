use std::fmt::Write as _;

/// Repository id, creation and last update of a cached pull request.
type CachedPr = (String, Timestamp, Option<Timestamp>);

/// Most repository names listed in a coverage reason.
const MAX_NAMED_REPOS: usize = 5;

fn name_repos(names: &[String]) -> String {
    let mut s = names
        .iter()
        .take(MAX_NAMED_REPOS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_NAMED_REPOS {
        let _ = write!(s, " and {} more", names.len() - MAX_NAMED_REPOS);
    }
    s
}
async fn persist_commit_range(
    sh: &Shared<'_, '_, '_>,
    range: Period,
    mut found: BTreeMap<EventId, CommitRecord>,
    unconfirmed: Vec<CommitRecord>,
    cached: &[ghusage_storage::StoredCommit],
    related: &Related<'_>,
    known_repos: &mut HashMap<String, Repository>,
    mut statuses: Vec<CoverageStatus>,
    mut reasons: Vec<String>,
    inaccessible: Vec<RepositoryRef>,
    failed: Vec<String>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let known = &*known_repos;
    let mut related_shas: HashSet<String> = found
        .values()
        .filter_map(commit_sha)
        .map(str::to_owned)
        .collect();
    related_shas.extend(
        cached
            .iter()
            .filter(|c| related.contains_id(&c.repository_id, known))
            .map(|c| c.sha.clone()),
    );
    let mut copies = 0;
    for c in unconfirmed {
        if commit_sha(&c).is_some_and(|sha| related_shas.contains(sha)) {
            copies += 1;
        } else {
            found.entry(c.event.event_id.clone()).or_insert(c);
        }
    }
    if copies > 0 {
        progress(SyncProgress::Detail(format!(
            "skipping {copies} commit(s) found by search that copy commits of related repositories"
        )));
    }
    let stale_copies: Vec<(String, String)> = cached
        .iter()
        .filter(|c| matches!(c.source, Source::Search | Source::Rest))
        .filter(|c| !related.contains_id(&c.repository_id, known))
        .filter(|c| related_shas.contains(&c.sha))
        .map(|c| (c.repository_id.clone(), c.sha.clone()))
        .collect();
    if !inaccessible.is_empty() {
        for repo in &inaccessible {
            sh.writable().await?;
            sh.store
                .borrow_mut()
                .set_repository_accessible(&repo.owner, &repo.name, false)?;
        }
        let names: Vec<String> = inaccessible
            .iter()
            .map(|repo| format!("{}/{}", repo.owner, repo.name))
            .collect();
        statuses.push(CoverageStatus::Partial);
        reasons.push(format!(
            "{} repositories not accessible: {}",
            names.len(),
            name_repos(&names)
        ));
    }
    if !failed.is_empty() {
        statuses.push(CoverageStatus::Partial);
        reasons.push(format!(
            "listing commits failed for {} repositories: {}",
            failed.len(),
            name_repos(&failed)
        ));
    }
    sh.report.borrow_mut().commits_found += found.len() as u64;
    let mut batch = WriteBatch {
        account: subject.login.clone(),
        ..Default::default()
    };
    add_commits(&mut batch, found.into_values().collect());
    for repo in &mut batch.repositories {
        if let Some(known) = known_repos.get(&repo.id)
            && known.viewer_permission.is_some()
        {
            repo.viewer_permission.clone_from(&known.viewer_permission);
            repo.accessible = true;
            continue;
        }
        if repo.viewer_permission.is_none() {
            match client.repository(&repo.owner, &repo.name).await {
                Ok(full) => *repo = full,
                Err(e @ ApiError::Unauthorized) => return Err(e.into()),
                Err(_) => {}
            }
        }
        known_repos.insert(repo.id.clone(), repo.clone());
    }
    let status = CoverageStatus::combine(statuses);
    let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
    let rec = record(DataKind::Commits, range, status, reason, subject, opts);
    batch.coverage.push(rec.clone());
    if !stale_copies.is_empty() {
        sh.writable().await?;
        sh.store
            .borrow_mut()
            .remove_listed_commits(&subject.login, &stale_copies)?;
    }
    sh.writable().await?;
    sh.store.borrow_mut().apply(&batch)?;
    sh.report.borrow_mut().coverage.push(rec);
    Ok(())
}
/// Finds a repository by owner and name among known ones.
fn find_repo<'a>(
    repos: &'a HashMap<String, Repository>,
    owner: &str,
    name: &str,
) -> Option<&'a Repository> {
    repos
        .values()
        .find(|r| r.owner.eq_ignore_ascii_case(owner) && r.name.eq_ignore_ascii_case(name))
}

/// Repositories the subject is known to be related to other than through
/// commits: owned by the subject, in the subject's repository list, or with
/// pull requests or issues of the subject.
///
/// Commit search also finds the subject's commits in repositories whose
/// relation cannot be confirmed from that metadata. Some are legitimate,
/// such as a repository another user owns that the subject pushes to
/// directly as a collaborator (not in the list of `--user`). Others only
/// hold a copy of the subject's commits, such as a project uploaded again
/// as a new repository instead of forked (GitHub does not mark those as
/// forks). The two cannot always be told apart, so search results in those
/// repositories are kept, except copies: commits whose SHA is also found in
/// a related repository.
struct Related<'a> {
    login: &'a str,
    ids: HashSet<String>,
    /// Lowercase `owner/name` of repositories known only by name.
    names: HashSet<String>,
}

impl Related<'_> {
    fn contains(&self, r: &Repository) -> bool {
        r.owner.eq_ignore_ascii_case(self.login)
            || self.ids.contains(&r.id)
            || self.names.contains(&r.full_name().to_ascii_lowercase())
    }

    /// Whether the repository with `id` is related; one that is not known
    /// at all is not.
    fn contains_id(&self, id: &str, known: &HashMap<String, Repository>) -> bool {
        self.ids.contains(id) || known.get(id).is_some_and(|r| self.contains(r))
    }
}

/// Resolves repositories referenced by pull requests into repository ids.
async fn resolve_pr_repositories(
    client: &GitHubClient,
    known_repos: &mut HashMap<String, Repository>,
    pushed: &[PushedRepository],
    pr_repos: &[RepositoryRef],
) -> Result<(HashSet<String>, Vec<String>)> {
    let mut ids = HashSet::new();
    let mut unresolved = Vec::new();
    for reference in pr_repos {
        let (owner, name) = (&reference.owner, &reference.name);
        let local = find_repo(known_repos, owner, name).cloned().or_else(|| {
            pushed
                .iter()
                .map(|p| &p.repository)
                .find(|r| r.owner.eq_ignore_ascii_case(owner) && r.name.eq_ignore_ascii_case(name))
                .cloned()
        });
        if let Some(repo) = local {
            ids.insert(repo.id.clone());
            known_repos.entry(repo.id.clone()).or_insert(repo);
            continue;
        }
        match client.repository(owner, name).await {
            Ok(repo) => {
                ids.insert(repo.id.clone());
                known_repos.insert(repo.id.clone(), repo);
            }
            Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized.into()),
            Err(_) => unresolved.push(format!("{owner}/{name}")),
        }
    }
    Ok((ids, unresolved))
}

/// The lowercase SHA of a commit, from its event id (`commit:{repo}:{sha}`).
fn commit_sha(c: &CommitRecord) -> Option<&str> {
    c.event
        .event_id
        .as_str()
        .rsplit_once(':')
        .map(|(_, sha)| sha)
}

/// Syncs commits for `ranges`: commit search, plus the commits list of the
/// default branch of every repository the subject pushed to (repository
/// listing), had commits found by search in, or has pull requests in.
/// Both sources produce the same event ids, so a commit found by both is
/// stored once. In repositories that are not [`Related`], search results
/// are used as they are, without listing, and copies are left out.
async fn sync_commits(
    sh: &Shared<'_, '_, '_>,
    ranges: &[Period],
    cached_prs: &[CachedPr],
    cached_issue_repos: &HashSet<String>,
    pr_repos: tokio::sync::oneshot::Receiver<Vec<RepositoryRef>>,
    issue_repos: tokio::sync::oneshot::Receiver<Vec<RepositoryRef>>,
    mut known_repos: HashMap<String, Repository>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let known_repos = &mut known_repos;
    let Some(earliest) = ranges.iter().map(|r| r.start).min() else {
        return Ok(());
    };
    progress(SyncProgress::Searching(DataKind::Commits));
    // Repositories pushed to since the earliest range, newest first.
    let user = (!opts.subject_is_viewer).then_some(subject.login.as_str());
    progress(SyncProgress::Detail(
        "listing repositories with recent pushes".into(),
    ));
    progress(SyncProgress::Detail("searching commits".into()));
    // The repository list and every range's commit search run at once.
    let (listing, searches) = tokio::join!(
        client.pushed_repositories(user, earliest),
        futures::future::try_join_all(ranges.iter().map(|range| client.search_commits(
            &subject.login,
            *range,
            opts.public_only
        )),),
    );
    let mut searches = searches?.into_iter();
    let (pushed, listing_failure) = match listing {
        Ok(list) => (list, None),
        Err(e @ ApiError::Unauthorized) => return Err(e.into()),
        Err(e) => (Vec::new(), Some(format!("repository list failed: {e}"))),
    };
    // A failed pull request search fails the whole sync elsewhere; here it
    // just means no pull request repositories.
    let pr_repos = pr_repos.await.unwrap_or_default();
    // Repositories of pull requests found by this sync.
    let (pr_repo_ids, unresolved) =
        resolve_pr_repositories(client, known_repos, &pushed, &pr_repos).await?;
    // A failed issue search fails the whole sync elsewhere too.
    let issue_repos = issue_repos.await.unwrap_or_default();
    let related = Related {
        login: &subject.login,
        ids: pushed
            .iter()
            .map(|p| p.repository.id.clone())
            .chain(pr_repo_ids.iter().cloned())
            .chain(cached_prs.iter().map(|(id, _, _)| id.clone()))
            .chain(cached_issue_repos.iter().cloned())
            .collect(),
        names: unresolved
            .iter()
            .cloned()
            .chain(issue_repos.iter().map(|repo| format!("{}/{}", repo.owner, repo.name)))
            .map(|n| n.to_ascii_lowercase())
            .collect(),
    };

    for range in ranges {
        let range = *range;
        let fetched = searches.next().expect("one commit search per range");
        sync_commit_range(
            sh,
            range,
            fetched,
            &pushed,
            &pr_repo_ids,
            cached_prs,
            listing_failure.as_ref(),
            &unresolved,
            &related,
            known_repos,
        )
        .await?;
    }
    Ok(())
}

async fn sync_commit_range(
    sh: &Shared<'_, '_, '_>,
    range: Period,
    fetched: ghusage_github::Fetched<Vec<CommitRecord>>,
    pushed: &[PushedRepository],
    pr_repo_ids: &HashSet<String>,
    cached_prs: &[CachedPr],
    listing_failure: Option<&String>,
    unresolved: &[String],
    related: &Related<'_>,
    known_repos: &mut HashMap<String, Repository>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
            // Search results in repositories not known to be related are used as
        // they are: listing their default branches could fail or take long
        // for a huge copy, and could not tell copies apart either.
        let (items, unconfirmed): (Vec<CommitRecord>, Vec<CommitRecord>) = fetched
            .items
            .into_iter()
            .partition(|c| related.contains(&c.repository));
        let cached = sh.store.borrow().commits_in(&subject.login, range)?;
        let mut statuses = vec![fetched.status];
        let mut reasons: Vec<String> = fetched.reason.into_iter().collect();
        if let Some(f) = listing_failure {
            statuses.push(CoverageStatus::Partial);
            reasons.push(f.clone());
        }
        if !unresolved.is_empty() {
            statuses.push(CoverageStatus::Partial);
            reasons.push(format!(
                "could not look up pull request repositories: {}",
                name_repos(unresolved)
            ));
        }

        // Candidate repositories for this range.
        let mut candidates: BTreeMap<String, Repository> = BTreeMap::new();
        for p in pushed {
            if p.pushed_at.is_some_and(|t| t >= range.start) {
                candidates.insert(p.repository.id.clone(), p.repository.clone());
            }
        }
        for c in &items {
            candidates
                .entry(c.repository.id.clone())
                .or_insert_with(|| c.repository.clone());
        }
        let cached_pr_ids = cached_prs
            .iter()
            .filter(|(_, created, updated)| {
                *created < range.end && updated.is_none_or(|u| u >= range.start)
            })
            .map(|(id, _, _)| id);
        for id in pr_repo_ids.iter().chain(cached_pr_ids) {
            if let Some(r) = known_repos.get(id) {
                candidates.entry(id.clone()).or_insert_with(|| r.clone());
            }
        }
        // Forks repeat the commits of their upstream repository, and commit
        // search leaves them out too.
        let to_list: Vec<Repository> = candidates
            .into_values()
            .filter(|r| !r.is_fork)
            .filter(|r| !opts.public_only || r.visibility.is_public())
            .collect();

        let total = to_list.len();
        let mut done = 0;
        progress(SyncProgress::Fetching {
            kind: DataKind::Commits,
            done,
            total,
        });
        let mut found: BTreeMap<EventId, CommitRecord> = BTreeMap::new();
        for c in items {
            found.insert(c.event.event_id.clone(), c);
        }
        let mut inaccessible: Vec<RepositoryRef> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut stream = futures::stream::iter(to_list.into_iter().map(|repo| async move {
            let res = client
                .default_branch_commits(&repo, &subject.login, range)
                .await;
            (repo, res)
        }))
        .buffer_unordered(opts.concurrency.clamp(1, 16));
        while let Some((repo, res)) = stream.next().await {
            done += 1;
            progress(SyncProgress::Fetching {
                kind: DataKind::Commits,
                done,
                total,
            });
            progress(SyncProgress::Detail(res.as_ref().map_or_else(
                |_| format!("commits in {}: failed", repo.full_name()),
                |listed| {
                    format!(
                        "commits in {}: {} found",
                        repo.full_name(),
                        listed.items.len()
                    )
                },
            )));
            match res {
                Ok(listed) => {
                    statuses.push(listed.status);
                    reasons.extend(listed.reason);
                    for c in listed.items {
                        found.entry(c.event.event_id.clone()).or_insert(c);
                    }
                }
                Err(e @ ApiError::Unauthorized) => return Err(e.into()),
                Err(ApiError::NotFound(_) | ApiError::Forbidden(_)) => {
                    inaccessible.push(RepositoryRef::new(repo.owner.clone(), repo.name));
                }
                Err(e) => failed.push(format!("{}: {e}", repo.full_name())),
            }
        }
        drop(stream);

        persist_commit_range(
            sh,
            range,
            found,
            unconfirmed,
            &cached,
            related,
            known_repos,
            statuses,
            reasons,
            inaccessible,
            failed,
        )
    .await?;
    Ok(())
}
