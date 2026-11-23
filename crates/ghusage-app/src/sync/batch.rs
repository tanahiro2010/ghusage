fn record(
    kind: DataKind,
    p: Period,
    status: CoverageStatus,
    reason: Option<String>,
    subject: &Subject,
    opts: &SyncOptions,
) -> CoverageRecord {
    CoverageRecord {
        kind,
        start: p.start,
        end: p.end,
        status,
        scope: scope(kind, &subject.login, opts),
        reason,
        recorded_at: opts.now,
    }
}

/// Runs a sync for `subject`. The caller holds the sync lock.
pub async fn run(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    opts: &SyncOptions,
    progress: &dyn Fn(SyncProgress),
) -> Result<SyncReport> {
    let run_id = store.begin_sync_run(&subject.login, Some(opts.period), opts.now)?;
    let result = run_inner(client, store, subject, opts, progress).await;
    let finished = Timestamp::now();
    match &result {
        Ok(report) => {
            let stats = serde_json::to_string(&report.api).ok();
            let status = if report.is_complete() {
                "success"
            } else {
                "partial"
            };
            let msg = (!report.failures.is_empty()).then(|| report.failures.join("; "));
            store.finish_sync_run(run_id, status, finished, stats.as_deref(), msg.as_deref())?;
        }
        Err(e) => {
            let stats = serde_json::to_string(&client.stats()).ok();
            store.finish_sync_run(
                run_id,
                "failed",
                finished,
                stats.as_deref(),
                Some(&e.to_string()),
            )?;
        }
    }
    progress(SyncProgress::Finished);
    result
}

async fn run_inner(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    opts: &SyncOptions,
    progress: &dyn Fn(SyncProgress),
) -> Result<SyncReport> {
    let existing = applicable_coverage(store.coverage(&subject.login)?, opts.public_only);
    let probe = Period::new(opts.now, opts.now + SignedDuration::from_secs(1))?;
    let cached = store.load_dataset(subject, false, Some(probe))?;
    let known_repos: HashMap<String, Repository> = cached.repositories.into_iter().collect();
    // Pull requests already linked to this account. One cached for another
    // account (another `--user`, or only viewed) still has to be fetched
    // for this one, even if it has not changed.
    let linked_prs: HashSet<String> = cached.pull_requests.keys().cloned().collect();
    let cached_issue_repos: HashSet<String> = cached
        .issues
        .values()
        .map(|i| i.repository_id.clone())
        .collect();
    let cached_prs: Vec<CachedPr> = cached
        .pull_requests
        .into_values()
        .map(|p| (p.repository_id, p.created_at, p.updated_at))
        .collect();
    let pr_ranges = ranges_to_fetch(&existing, DataKind::PullRequests, opts);
    let commit_ranges = ranges_to_fetch(&existing, DataKind::Commits, opts);
    let issue_ranges = ranges_to_fetch(&existing, DataKind::Issues, opts);
    // Periodically recheck open pull requests even if search did not
    // report them as updated.
    let stale_open = stale_open_prs(store, subject, opts)?;
    let pr_states: HashMap<String, (Option<Timestamp>, CoverageStatus)> = store
        .pr_sync_states()?
        .into_iter()
        .filter(|s| linked_prs.contains(&s.pr_id))
        .map(|s| (s.pr_id, (s.updated_at, s.coverage)))
        .collect();

    // Pull requests, commits and issues are fetched at the same time; the
    // client limits the number of requests in flight. All phases run on
    // this task, so the store is only borrowed between awaits.
    let store = RefCell::new(store);
    let report = RefCell::new(SyncReport::default());
    let shared = Shared {
        client,
        store: &store,
        report: &report,
        subject,
        opts,
        progress,
    };
    // Commit search does not depend on the pull request search, so it starts
    // right away; only listing the default branches of pull request
    // repositories, and telling related repositories from unrelated ones,
    // waits for the repositories that pull request and issue search found.
    let (pr_repos_tx, pr_repos_rx) = tokio::sync::oneshot::channel();
    let (issue_repos_tx, issue_repos_rx) = tokio::sync::oneshot::channel();
    let prs = async {
        let (refs, search_status) = search_prs(&shared, &pr_ranges).await?;
        let mut pr_repos: Vec<(String, String)> = Vec::new();
        let mut seen_names = HashSet::new();
        for r in refs.values() {
            if seen_names.insert(format!("{}/{}", r.owner, r.name).to_ascii_lowercase()) {
                pr_repos.push((r.owner.clone(), r.name.clone()));
            }
        }
        let _ = pr_repos_tx.send(pr_repos);
        fetch_prs(&shared, &refs, search_status, &pr_states, stale_open).await
    };
    // Commits: commit search plus the default branches of repositories the
    // subject pushed to or took part in through pull requests.
    let commits = sync_commits(
        &shared,
        &commit_ranges,
        &cached_prs,
        &cached_issue_repos,
        pr_repos_rx,
        issue_repos_rx,
        known_repos,
    );
    tokio::try_join!(
        prs,
        commits,
        sync_issues(&shared, &issue_ranges, issue_repos_tx)
    )?;

    let mut report = report.into_inner();
    report.api = client.stats();
    report.status = if report.is_complete() {
        "complete".into()
    } else {
        "partial".into()
    };
    Ok(report)
}

/// Sync cursor holding the update time of an issue's last complete fetch.
fn issue_cursor(node_id: &str) -> String {
    format!("issue_updated_at:{node_id}")
}

/// Search window for pull requests and issues. GitHub matches `updated:`
/// against the current update time, so anything active in `range` but
/// updated again later is only found when the window extends to now.
fn search_window(range: Period, opts: &SyncOptions) -> Period {
    Period::new(range.start, opts.now.max(range.end)).unwrap_or(range)
}

fn add_commits(batch: &mut WriteBatch, commits: Vec<CommitRecord>) {
    let mut repos = HashSet::new();
    for c in commits {
        if repos.insert(c.repository.id.clone()) {
            batch.repositories.push(c.repository);
        }
        batch.events.push(c.event);
    }
}

/// Owner, name and number of a pull request.
type PrKey = (String, String, u64);

fn stale_open_prs(
    store: &Store,
    subject: &Subject,
    opts: &SyncOptions,
) -> Result<Vec<(String, PrKey)>> {
    let probe = Period::new(opts.now, opts.now + SignedDuration::from_secs(1))?;
    let ds = store.load_dataset(subject, false, Some(probe))?;
    Ok(ds
        .pull_requests
        .values()
        .filter(|pr| pr.state == PrState::Open)
        .filter(|pr| {
            pr.synced_at
                .is_none_or(|t| opts.now - t > opts.recheck_open_after)
        })
        .filter_map(|pr| {
            let repo = ds.repositories.get(&pr.repository_id)?;
            Some((
                pr.id.clone(),
                (repo.owner.clone(), repo.name.clone(), pr.number),
            ))
        })
        .collect())
}

/// Account suffix for pull requests fetched only for viewing.
pub const VIEWED_SUFFIX: &str = "+viewed";

/// Whether the subject took part in a pull request: author, any event,
/// requested reviewer or assignee.
pub fn involves(subject: &Subject, bundle: &PrBundle) -> bool {
    bundle.pull_request.author.is(subject)
        || bundle.events.iter().any(|e| {
            e.actor.is(subject)
                || (matches!(
                    e.event_type,
                    EventType::ReviewRequested | EventType::Assigned
                ) && e
                    .meta
                    .target
                    .as_deref()
                    .is_some_and(|t| t.eq_ignore_ascii_case(&subject.login)))
        })
}

/// Builds the write batch for one fetched pull request. Commits in the pull
/// request authored by the subject are also recorded as the subject's
/// commits (deduplicated with commit search results by repository and SHA).
pub fn pr_batch(subject: &Subject, bundle: PrBundle) -> WriteBatch {
    let pr_id = bundle.pull_request.id.clone();
    let repo_id = bundle.repository.id.clone();
    let observed_at = bundle
        .events
        .iter()
        .map(|e| e.observed_at)
        .max()
        .or(bundle.pull_request.synced_at)
        .unwrap_or_else(Timestamp::now);
    let present: HashSet<EventId> = bundle.events.iter().map(|e| e.event_id.clone()).collect();
    let present_threads = bundle.threads.iter().map(|t| t.id.clone()).collect();
    let pr_commits: Vec<PrCommitLink> = bundle
        .current_commit_shas
        .iter()
        .map(|sha| PrCommitLink {
            pr_id: pr_id.clone(),
            repository_id: repo_id.clone(),
            sha: sha.clone(),
            is_current: true,
        })
        .collect();
    let mut events = bundle.events;
    let own_commits: Vec<ActivityEvent> = events
        .iter()
        .filter(|e| e.event_type == EventType::PrCommit && e.actor.is(subject))
        .filter_map(|e| {
            let sha = e.meta.sha.as_deref()?;
            let mut c = e.clone();
            c.event_id = EventId::commit(&e.repository_id, sha);
            c.event_type = EventType::CommitAuthored;
            c.parent_pr_id = None;
            c.meta.time_basis = None;
            c.source = Source::Graphql;
            Some(c)
        })
        .collect();
    events.extend(own_commits);
    let complete: Vec<Collection> = bundle.complete_collections.clone();
    let commit_stats = bundle
        .commit_stats
        .into_iter()
        .map(|(sha, counts)| CommitStats {
            repository_id: repo_id.clone(),
            sha,
            counts,
            observed_at,
        })
        .collect();
    let pr_diffs = bundle
        .diff
        .map(|d| vec![(pr_id.clone(), d)])
        .unwrap_or_default();
    let pr_merge_commit_parents = bundle
        .merge_commit_parents
        .map(|n| vec![(pr_id.clone(), n)])
        .unwrap_or_default();
    WriteBatch {
        account: subject.login.clone(),
        repositories: vec![bundle.repository],
        pull_requests: vec![bundle.pull_request],
        threads: bundle.threads,
        events,
        pr_commits,
        pr_snapshots: vec![PrSnapshot {
            pr_id,
            observed_at,
            complete_collections: complete,
            present,
            present_threads,
        }],
        commit_stats,
        pr_diffs,
        pr_merge_commit_parents,
        ..Default::default()
    }
}
