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
        scope: CoverageScope {
            visibility: if opts.public_only { CoverageVisibility::PublicOnly } else { CoverageVisibility::All },
            generation: match kind {
                DataKind::Commits => CoverageGeneration::CommitsListingSearchV1,
                DataKind::PullRequests => CoverageGeneration::PullRequestsV1,
                DataKind::Issues => CoverageGeneration::IssuesV1,
            },
        },
        description: coverage_description(kind, subject, opts),
        reason,
        recorded_at: opts.now,
    }
}

fn coverage_description(kind: DataKind, subject: &Subject, opts: &SyncOptions) -> String {
    let visibility = if opts.public_only {
        " (public repositories only)"
    } else {
        ""
    };
    match kind {
        DataKind::Commits => {
            let relation = if opts.subject_is_viewer {
                "pushed to or contributed to"
            } else {
                "owns or contributed to"
            };
            format!(
                "commits authored by {} on the default branches of repositories {} {}, plus commits of synced pull requests{}",
                subject.login,
                subject.login,
                relation,
                visibility
            )
            .replace(
                "plus commits of synced pull requests",
                "(repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests",
            )
        }
        DataKind::PullRequests => format!(
            "pull requests involving or reviewed by {} updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks{}",
            subject.login, visibility
        ),
        DataKind::Issues => format!(
            "issues involving {} updated in range, with open/close/reopen history{}",
            subject.login, visibility
        ),
    }
}

/// `GET /user` for a sync that started with the viewer remembered from the
/// cache instead of waiting for it (see `Session::open_deferred`).
pub type ViewerCheck<'a> =
    futures::future::Shared<futures::future::LocalBoxFuture<'a, std::result::Result<Viewer, ApiError>>>;

/// Whether `viewer` is the same account as `subject`, by id and login.
#[must_use]
pub fn is_viewer(subject: &Subject, viewer: &Viewer) -> bool {
    subject.id.as_deref() == Some(viewer.id.as_str()) && subject.login == viewer.login
}

/// Whether `check` (if any) confirms `subject` as the authenticated user.
async fn confirmed(check: Option<&ViewerCheck<'_>>, subject: &Subject) -> bool {
    match check {
        None => true,
        Some(check) => check.clone().await.is_ok_and(|v| is_viewer(subject, &v)),
    }
}

/// Runs a sync for `subject`. The caller holds the sync lock.
///
/// With `check`, `subject` is not confirmed yet: requests start at once,
/// but nothing is written (not even the sync record) before `check`
/// confirms it, and nothing at all when it does not.
pub async fn run(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    opts: &SyncOptions,
    progress: &dyn Fn(SyncProgress),
    check: Option<ViewerCheck<'_>>,
) -> Result<SyncReport> {
    let run_id = Cell::new(None);
    let result = run_inner(client, store, subject, opts, progress, check.clone(), &run_id).await;
    if !confirmed(check.as_ref(), subject).await {
        return result.and(Err(AppError::Other(UNCONFIRMED.into())));
    }
    let run_id = match run_id.get() {
        Some(id) => id,
        None => store.begin_sync_run(&subject.login, Some(opts.period), opts.now)?,
    };
    let finished = Timestamp::now();
    match &result {
        Ok(report) => {
            let stats = serde_json::to_string(&report.api).ok();
            // A run in which a whole phase failed is not counted as a
            // finished sync (see `Store::completed_sync_runs`): it did not
            // fetch what it was asked for, so a sync that does is still the
            // first one.
            let sync_status = if !report.failed_phases.is_empty() {
                "incomplete"
            } else if report.is_complete() {
                "success"
            } else {
                "partial"
            };
            let msg = (!report.failures.is_empty()).then(|| report.failures.join("; "));
            store.finish_sync_run(
                run_id,
                sync_status,
                finished,
                stats.as_deref(),
                msg.as_deref(),
            )?;
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

/// Error of a sync whose subject turned out not to be the authenticated
/// user; the session runs it again for the right account.
const UNCONFIRMED: &str = "the authenticated account is not the cached one";

async fn run_inner(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    opts: &SyncOptions,
    progress: &dyn Fn(SyncProgress),
    check: Option<ViewerCheck<'_>>,
    run_id: &Cell<Option<i64>>,
) -> Result<SyncReport> {
    let existing = applicable_coverage(store.coverage(&subject.login)?, opts.public_only);
    let probe = Period::new(opts.now, opts.now + SignedDuration::from_secs(1))?;
    let cached = store.load_dataset(subject, false, Some(probe))?;
    let known_repos: HashMap<String, Repository> = cached.repositories.into_iter().collect();
    // Pull requests already linked to this account. One cached for another
    // account (another `--user`, or only viewed) still has to be fetched
    // for this one, even if it has not changed.
    let linked_prs = store.pull_request_ids(&subject.login, false)?;
    let cached_issue_repos: HashSet<String> = cached
        .issues
        .values()
        .map(|i| i.repository_id.clone())
        .collect();
    let cached_prs: Vec<CachedPr> = cached
        .pull_requests
        .into_iter()
        .filter(|(id, _)| linked_prs.contains(id))
        .map(|(_, p)| p)
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
        check,
        run_id,
    };
    // Commit search does not depend on the pull request search, so it starts
    // right away; only listing the default branches of pull request
    // repositories, and telling related repositories from unrelated ones,
    // waits for the repositories that pull request and issue search found.
    let (pr_repos_tx, pr_repos_rx) = tokio::sync::oneshot::channel();
    let (issue_repos_tx, issue_repos_rx) = tokio::sync::oneshot::channel();
    // Pull requests are fetched while the remaining searches run.
    //
    // A phase that fails (a search GitHub keeps refusing, or a rate limit
    // too far away to wait for) is recorded as incomplete and the other
    // phases go on; only errors that make every request fail, such as a
    // rejected token, stop the whole sync.
    let prs = async {
        let (queue, queued_prs) = futures::channel::mpsc::unbounded();
        let queued = Cell::new(0);
        // Pull requests queued before the search failed are still fetched.
        let search = async {
            phase_result(
                search_prs(
                    &shared,
                    &pr_ranges,
                    &pr_states,
                    stale_open,
                    pr_repos_tx,
                    queue,
                    &queued,
                )
                .await,
            )
        };
        let (search_status, (failures, partial)) =
            tokio::try_join!(search, fetch_prs(&shared, queued_prs, &queued))?;
        match search_status {
            Ok(status) => record_pr_coverage(&shared, status, failures, partial).await,
            Err(e) => {
                {
                    let mut report = shared.report.borrow_mut();
                    report.failures.extend(failures);
                    report.failures.extend(partial);
                }
                phase_failed(&shared, &existing, DataKind::PullRequests, &e).await
            }
        }
    };
    // Commits: commit search plus the default branches of repositories the
    // subject pushed to or took part in through pull requests.
    let commits = async {
        let synced = sync_commits(
            &shared,
            &commit_ranges,
            &cached_prs,
            &cached_issue_repos,
            pr_repos_rx,
            issue_repos_rx,
            known_repos,
        )
        .await;
        match phase_result(synced)? {
            Ok(()) => Ok(()),
            Err(e) => phase_failed(&shared, &existing, DataKind::Commits, &e).await,
        }
    };
    let issues = async {
        match phase_result(sync_issues(&shared, &issue_ranges, issue_repos_tx).await)? {
            Ok(()) => Ok(()),
            Err(e) => phase_failed(&shared, &existing, DataKind::Issues, &e).await,
        }
    };
    tokio::try_join!(prs, commits, issues)?;

    let mut report = report.into_inner();
    report.api = client.stats();
    report.status = if report.is_complete() {
        "complete".into()
    } else {
        "partial".into()
    };
    Ok(report)
}

/// Separates the failure of one phase from errors that end the sync.
///
/// An API error other than a rejected token only fails the phase it
/// happened in (`Ok(Err(_))`); every other error, such as a rejected token
/// or a cache that cannot be written, is returned as `Err` and ends the
/// sync.
fn phase_result<T>(result: Result<T>) -> Result<std::result::Result<T, ApiError>> {
    match result {
        Ok(v) => Ok(Ok(v)),
        Err(AppError::Api(e)) => Ok(Err(e)),
        Err(e) => Err(e),
    }
}

/// The phase of `kind`, as named in failure messages.
const fn phase_name(kind: DataKind) -> &'static str {
    match kind {
        DataKind::Commits => "commit sync",
        DataKind::PullRequests => "pull request sync",
        DataKind::Issues => "issue sync",
    }
}

/// Records that the phase of `kind` failed with `error`: a failure in the
/// report, and partial coverage with the error as reason for the part of
/// the period not already covered completely by an earlier sync (that data
/// is still in the cache, so it stays complete).
async fn phase_failed(
    sh: &Shared<'_, '_, '_>,
    existing: &[CoverageRecord],
    kind: DataKind,
    error: &ApiError,
) -> Result<()> {
    let (subject, opts) = (sh.subject, sh.opts);
    let message = format!("{} failed: {error}", phase_name(kind));
    let summary = coverage::summarize(existing, kind, opts.period.start, opts.period.end);
    let records: Vec<CoverageRecord> = summary
        .gaps()
        .filter_map(|g| Period::new(g.start, g.end).ok())
        .map(|p| {
            record(
                kind,
                p,
                CoverageStatus::Partial,
                Some(message.clone()),
                subject,
                opts,
            )
        })
        .collect();
    if !records.is_empty() {
        sh.writable().await?;
        sh.store.borrow_mut().apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: records.clone(),
            ..Default::default()
        })?;
    }
    let mut report = sh.report.borrow_mut();
    report.coverage.extend(records);
    report.failures.push(message.clone());
    report.failed_phases.push(message);
    Ok(())
}

/// Sync cursor holding the update time of an issue's last complete fetch.
fn issue_cursor(node_id: &str) -> String {
    format!("issue_updated_at:{node_id}")
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
type PrKey = (RepositoryRef, u64);

fn stale_open_prs(
    store: &Store,
    subject: &Subject,
    opts: &SyncOptions,
) -> Result<Vec<(String, PrKey)>> {
    let probe = Period::new(opts.now, opts.now + SignedDuration::from_secs(1))?;
    let ds = store.load_dataset(subject, false, Some(probe))?;
    let viewed_prs = store.pull_request_ids(&subject.login, true)?;
    Ok(ds
        .pull_requests
        .values()
        .filter(|pr| !viewed_prs.contains(&pr.id))
        .filter(|pr| pr.state == PrState::Open)
        .filter(|pr| {
            pr.synced_at
                .is_none_or(|t| opts.now - t > opts.recheck_open_after)
        })
        .filter_map(|pr| {
            let repo = ds.repositories.get(&pr.repository_id)?;
            Some((
                pr.id.clone(),
                (RepositoryRef::new(repo.owner.clone(), repo.name.clone()), pr.number),
            ))
        })
        .collect())
}

/// Whether the subject took part in a pull request: author, any event,
/// requested reviewer or assignee.
#[must_use]
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

/// Builds the write batch for a fetched pull request.
///
/// Commits in the pull
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
