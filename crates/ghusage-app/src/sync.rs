//! Synchronization of GitHub data into the local cache.
//!
//! Each pull request and issue is written in its own transaction, so an
//! interrupted sync keeps everything written so far and a rerun continues
//! with what is missing. Coverage records describe exactly which ranges
//! were fetched and how completely.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use futures::StreamExt;
use ghusage_core::coverage::{self, CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::model::{
    ActivityEvent, Collection, EventId, EventType, PrState, Repository, Source, Subject,
};
use ghusage_core::time::Period;
use ghusage_github::{
    ApiError, ApiStats, CommitRecord, CommitRef, GitHubClient, IssueKind, IssueRef, PrBundle,
};
use ghusage_storage::{CommitStats, PrCommitLink, PrSnapshot, Store, WriteBatch};
use jiff::{SignedDuration, Timestamp};

use crate::error::{AppError, Result};

/// Metadata keys remembering the authenticated user for offline use.
pub const META_VIEWER_LOGIN: &str = "viewer_login";
pub const META_VIEWER_ID: &str = "viewer_id";

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub period: Period,
    /// Restrict to public repositories.
    pub public_only: bool,
    /// Refetch the whole period instead of only missing ranges.
    pub full: bool,
    pub now: Timestamp,
    /// How far before a missing range to start refetching, to catch data
    /// GitHub indexed late.
    pub overlap: SignedDuration,
    /// Open pull requests not refreshed for this long are refetched.
    pub recheck_open_after: SignedDuration,
    /// Number of pull requests/issues fetched concurrently.
    pub concurrency: usize,
    /// Whether the subject is the authenticated user. Repository discovery
    /// for commits then also sees collaborator and organization
    /// repositories; for other users only their own repositories are
    /// visible.
    pub subject_is_viewer: bool,
}

impl SyncOptions {
    pub fn new(period: Period, now: Timestamp) -> Self {
        Self {
            period,
            public_only: false,
            full: false,
            now,
            overlap: SignedDuration::from_secs(3 * 24 * 60 * 60),
            recheck_open_after: SignedDuration::from_secs(7 * 24 * 60 * 60),
            concurrency: 8,
            subject_is_viewer: true,
        }
    }
}

/// Progress notifications for interactive display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncProgress {
    Searching(DataKind),
    Fetching {
        kind: DataKind,
        done: usize,
        total: usize,
    },
    /// The latest step, e.g. a repository or pull request just fetched.
    Detail(String),
    Finished,
}

/// What a sync did.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SyncReport {
    pub status: String,
    pub commits_found: u64,
    pub pull_requests_found: u64,
    pub pull_requests_fetched: u64,
    pub pull_requests_unchanged: u64,
    pub issues_found: u64,
    pub issues_fetched: u64,
    pub issues_unchanged: u64,
    pub failures: Vec<String>,
    pub coverage: Vec<CoverageRecord>,
    pub api: ApiStats,
    /// Line counts fetched with `--diff-stats`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_stats: Option<LineCountSync>,
}

/// What fetching commit line counts did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LineCountSync {
    /// The subject's non-merge commits in the period whose line counts go
    /// into the totals.
    pub commits: u64,
    /// Of those, already known before this run (not fetched again).
    pub already_known: u64,
    /// Fetched now.
    pub fetched: u64,
    /// Asked for but not returned by GitHub (left unknown).
    pub unavailable: u64,
}

impl SyncReport {
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && self.coverage.iter().all(|c| c.status.is_complete())
    }
}

/// Resolves who "you" is: `--user` when given, else the authenticated user.
/// Remembers the viewer for offline use.
pub async fn resolve_subject(
    client: &GitHubClient,
    store: &mut Store,
    user: Option<&str>,
) -> Result<(Subject, bool)> {
    match user {
        Some(login) => {
            let u = client.user(login).await?;
            Ok((
                Subject {
                    id: Some(u.id),
                    login: u.login,
                },
                false,
            ))
        }
        None => {
            let v = client.viewer().await?;
            store.set_meta(META_VIEWER_LOGIN, &v.login)?;
            store.set_meta(META_VIEWER_ID, &v.id)?;
            Ok((
                Subject {
                    id: Some(v.id),
                    login: v.login,
                },
                true,
            ))
        }
    }
}

/// Subject from the cache, for offline use.
pub fn cached_subject(store: &Store, user: Option<&str>) -> Result<(Subject, bool)> {
    let viewer_login = store.meta(META_VIEWER_LOGIN)?;
    match (user, viewer_login) {
        (Some(login), Some(v)) if v.eq_ignore_ascii_case(login) => Ok((
            Subject {
                id: store.meta(META_VIEWER_ID)?,
                login: v,
            },
            true,
        )),
        (Some(login), _) => Ok((
            Subject {
                id: None,
                login: login.to_string(),
            },
            false,
        )),
        (None, Some(v)) => Ok((
            Subject {
                id: store.meta(META_VIEWER_ID)?,
                login: v,
            },
            true,
        )),
        (None, None) => Err(AppError::Other(
            "no cached data for the authenticated user; run `ghusage sync` online first or pass --user"
                .into(),
        )),
    }
}

/// Suffix of coverage scopes recorded by `--public-only` syncs.
pub const PUBLIC_ONLY_SCOPE: &str = " (public repositories only)";

/// Marker in the scope of commit coverage recorded by syncs that list
/// repository commits in addition to commit search, and leave out copies of
/// commits found in related repositories.
pub const COMMIT_LISTING_SCOPE: &str = "(repository listing + commit search; other repositories by commit search alone, copies excluded)";

/// Coverage records that count for the current mode. Records from a
/// public-only sync do not cover private data, so they are ignored unless
/// the current mode is public-only too. Commit records without
/// [`COMMIT_LISTING_SCOPE`] are ignored and the range is synced again:
/// records written before repository listing was added relied on commit
/// search alone, which misses commits, and records written before copies
/// were told apart may count copies of the subject's commits in other
/// repositories.
pub fn applicable_coverage(records: Vec<CoverageRecord>, public_only: bool) -> Vec<CoverageRecord> {
    records
        .into_iter()
        .filter(|r| public_only || !r.scope.ends_with(PUBLIC_ONLY_SCOPE))
        .filter(|r| r.kind != DataKind::Commits || r.scope.contains(COMMIT_LISTING_SCOPE))
        .collect()
}

/// Ranges of `kind` that need fetching for `period`.
pub fn ranges_to_fetch(
    records: &[CoverageRecord],
    kind: DataKind,
    opts: &SyncOptions,
) -> Vec<Period> {
    if opts.full {
        return vec![opts.period];
    }
    let summary = coverage::summarize(records, kind, opts.period.start, opts.period.end);
    let mut ranges: Vec<(Timestamp, Timestamp)> = summary
        .gaps()
        .map(|g| ((g.start - opts.overlap).max(opts.period.start), g.end))
        .collect();
    ranges.sort();
    let mut merged: Vec<(Timestamp, Timestamp)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
        .into_iter()
        .filter_map(|(s, e)| Period::new(s, e).ok())
        .collect()
}

fn scope(kind: DataKind, login: &str, opts: &SyncOptions) -> String {
    let vis = if opts.public_only {
        PUBLIC_ONLY_SCOPE
    } else {
        ""
    };
    match kind {
        DataKind::Commits => {
            let repos = if opts.subject_is_viewer {
                "pushed to or contributed to"
            } else {
                "owns or contributed to"
            };
            format!(
                "commits authored by {login} on the default branches of repositories {login} {repos} {COMMIT_LISTING_SCOPE}, plus commits of synced pull requests{vis}"
            )
        }
        DataKind::PullRequests => format!(
            "pull requests involving or reviewed by {login} updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks{vis}"
        ),
        DataKind::Issues => format!(
            "issues involving {login} updated in range, with open/close/reopen history{vis}"
        ),
    }
}

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

/// Fetches the line counts of the subject's non-merge commits in `period`
/// that have none yet, in batches of up to 100 commits per GraphQL query.
/// Counts already known are reused, so running it again only asks for what
/// is still missing. Commits outside the period, merge commits and commits
/// of repositories hidden by `public_only` are left alone.
pub async fn fetch_line_counts(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    subject_is_viewer: bool,
    period: Period,
    public_only: bool,
    progress: &dyn Fn(SyncProgress),
) -> Result<LineCountSync> {
    let ds = store.load_dataset(subject, subject_is_viewer, Some(period))?;
    let mut report = LineCountSync::default();
    let mut seen: HashSet<&str> = HashSet::new();
    // CommitRef -> repository id, for storing the answers.
    let mut wanted: Vec<(CommitRef, String)> = Vec::new();
    for e in &ds.events {
        if e.event_type != EventType::CommitAuthored
            || !e.actor.is(subject)
            || e.meta.is_merge_commit == Some(true)
            || !(period.contains(e.occurred_at)
                || e.meta.author_date.is_some_and(|d| period.contains(d)))
            || !seen.insert(e.event_id.as_str())
        {
            continue;
        }
        let Some(repo) = ds.repositories.get(&e.repository_id) else {
            continue;
        };
        if public_only && !repo.visibility.is_public() {
            continue;
        }
        // Commits whose changes are counted through other commits are not
        // part of the totals, so their counts are not needed.
        if !ds.counts_lines(e.event_id.as_str()) {
            continue;
        }
        report.commits += 1;
        if ds.commit_stats.contains_key(e.event_id.as_str()) {
            report.already_known += 1;
            continue;
        }
        let Some(sha) = e.meta.sha.clone() else {
            report.unavailable += 1;
            continue;
        };
        wanted.push((
            CommitRef {
                owner: repo.owner.clone(),
                name: repo.name.clone(),
                sha: sha.to_ascii_lowercase(),
            },
            repo.id.clone(),
        ));
    }
    let total = wanted.len();
    let now = Timestamp::now();
    let mut done = 0;
    for chunk in wanted.chunks(500) {
        progress(SyncProgress::Fetching {
            kind: DataKind::Commits,
            done,
            total,
        });
        progress(SyncProgress::Detail(format!(
            "line counts of {done}/{total} commits"
        )));
        let refs: Vec<CommitRef> = chunk.iter().map(|(c, _)| c.clone()).collect();
        let repo_of: HashMap<&CommitRef, &str> =
            chunk.iter().map(|(c, id)| (c, id.as_str())).collect();
        let answers = client.commit_line_counts(&refs).await?;
        let mut batch = WriteBatch {
            account: subject.login.clone(),
            ..Default::default()
        };
        for (c, counts) in answers {
            match (counts, repo_of.get(&c)) {
                (Some(counts), Some(repo_id)) => {
                    report.fetched += 1;
                    batch.commit_stats.push(CommitStats {
                        repository_id: (*repo_id).to_string(),
                        sha: c.sha,
                        counts,
                        observed_at: now,
                    });
                }
                _ => report.unavailable += 1,
            }
        }
        store.apply(&batch)?;
        done += chunk.len();
    }
    progress(SyncProgress::Fetching {
        kind: DataKind::Commits,
        done,
        total,
    });
    Ok(report)
}

/// What every sync phase works with.
struct Shared<'a, 's> {
    client: &'a GitHubClient,
    store: &'a RefCell<&'s mut Store>,
    report: &'a RefCell<SyncReport>,
    subject: &'a Subject,
    opts: &'a SyncOptions,
    progress: &'a dyn Fn(SyncProgress),
}

type SearchStatus = Vec<(Period, CoverageStatus, Option<String>)>;

/// Finds pull requests updated in `ranges`.
async fn search_prs(
    sh: &Shared<'_, '_>,
    ranges: &[Period],
) -> Result<(BTreeMap<String, IssueRef>, SearchStatus)> {
    let (client, subject, opts) = (sh.client, sh.subject, sh.opts);
    let mut refs: BTreeMap<String, IssueRef> = BTreeMap::new();
    let mut search_status = Vec::new();
    for range in ranges {
        (sh.progress)(SyncProgress::Searching(DataKind::PullRequests));
        let mut statuses = Vec::new();
        let mut reasons = Vec::new();
        for q in [
            format!("involves:{}", subject.login),
            format!("reviewed-by:{}", subject.login),
        ] {
            (sh.progress)(SyncProgress::Detail(format!(
                "searching pull requests: {q} updated since {}",
                range.start.strftime("%Y-%m-%d")
            )));
            let found = client
                .search_issues(
                    &q,
                    IssueKind::PullRequest,
                    search_window(*range, opts),
                    opts.public_only,
                )
                .await?;
            statuses.push(found.status);
            reasons.extend(found.reason);
            for r in found.items {
                refs.insert(r.node_id.clone(), r);
            }
        }
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        search_status.push((*range, CoverageStatus::combine(statuses), reason));
    }
    sh.report.borrow_mut().pull_requests_found = refs.len() as u64;
    Ok((refs, search_status))
}

/// Fetches changed and stale pull requests and records pull request
/// coverage for the searched ranges.
async fn fetch_prs(
    sh: &Shared<'_, '_>,
    refs: &BTreeMap<String, IssueRef>,
    search_status: SearchStatus,
    states: &HashMap<String, (Option<Timestamp>, CoverageStatus)>,
    stale_open: Vec<(String, PrKey)>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let mut to_fetch: Vec<PrKey> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in refs.values() {
        let unchanged = matches!(
            states.get(&r.node_id),
            Some((Some(u), CoverageStatus::Complete)) if *u == r.updated_at
        );
        if unchanged && !opts.full {
            sh.report.borrow_mut().pull_requests_unchanged += 1;
            continue;
        }
        if seen.insert(r.node_id.clone()) {
            to_fetch.push((r.owner.clone(), r.name.clone(), r.number));
        }
    }
    for (id, key) in stale_open {
        if seen.insert(id) {
            to_fetch.push(key);
        }
    }

    let mut pr_failures: Vec<String> = Vec::new();
    let mut pr_partial: Vec<String> = Vec::new();
    let total = to_fetch.len();
    let mut done = 0;
    progress(SyncProgress::Fetching {
        kind: DataKind::PullRequests,
        done,
        total,
    });
    let mut stream = futures::stream::iter(to_fetch.into_iter().map(|(o, n, num)| async move {
        let res = client.fetch_pull_request(&o, &n, num).await;
        (o, n, num, res)
    }))
    .buffer_unordered(opts.concurrency.clamp(1, 16));
    while let Some((o, n, num, res)) = stream.next().await {
        done += 1;
        progress(SyncProgress::Fetching {
            kind: DataKind::PullRequests,
            done,
            total,
        });
        progress(SyncProgress::Detail(format!("pull request {o}/{n}#{num}")));
        match res {
            Ok(bundle) => {
                if !bundle.status.is_complete() {
                    pr_partial.push(format!("{o}/{n}#{num}: {}", bundle.notes.join(", ")));
                }
                let batch = pr_batch(subject, bundle);
                sh.store.borrow_mut().apply(&batch)?;
                sh.report.borrow_mut().pull_requests_fetched += 1;
            }
            Err(e @ ApiError::Unauthorized) => return Err(e.into()),
            Err(e) => {
                if matches!(e, ApiError::NotFound(_) | ApiError::Forbidden(_)) {
                    // Keep earlier data but show that access was lost.
                    sh.store
                        .borrow_mut()
                        .set_repository_accessible(&o, &n, false)?;
                }
                pr_failures.push(format!("{o}/{n}#{num}: {e}"));
            }
        }
    }
    drop(stream);
    for (range, status, reason) in search_status {
        let mut reasons: Vec<String> = reason.into_iter().collect();
        if !pr_failures.is_empty() {
            reasons.push(format!("{} pull request(s) failed", pr_failures.len()));
        }
        if !pr_partial.is_empty() {
            reasons.push(format!(
                "{} pull request(s) partially fetched",
                pr_partial.len()
            ));
        }
        let status = if pr_failures.is_empty() && pr_partial.is_empty() {
            status
        } else {
            CoverageStatus::combine([status, CoverageStatus::Partial])
        };
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        let rec = record(DataKind::PullRequests, range, status, reason, subject, opts);
        sh.store.borrow_mut().apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        sh.report.borrow_mut().coverage.push(rec);
    }
    let mut report = sh.report.borrow_mut();
    report.failures.extend(pr_failures);
    report.failures.extend(pr_partial);
    Ok(())
}

/// Finds issues updated in `ranges` and fetches the changed ones. The
/// repositories of the issues found are sent to `issue_repos` as soon as
/// every range was searched.
async fn sync_issues(
    sh: &Shared<'_, '_>,
    ranges: &[Period],
    issue_repos: tokio::sync::oneshot::Sender<Vec<(String, String)>>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let mut searched = Vec::new();
    for &range in ranges {
        progress(SyncProgress::Searching(DataKind::Issues));
        progress(SyncProgress::Detail(format!(
            "searching issues updated since {}",
            range.start.strftime("%Y-%m-%d")
        )));
        let found = client
            .search_issues(
                &format!("involves:{}", subject.login),
                IssueKind::Issue,
                search_window(range, opts),
                opts.public_only,
            )
            .await?;
        searched.push((range, found));
    }
    let mut repos: Vec<(String, String)> = Vec::new();
    let mut seen_names = HashSet::new();
    for r in searched.iter().flat_map(|(_, found)| &found.items) {
        if seen_names.insert(format!("{}/{}", r.owner, r.name).to_ascii_lowercase()) {
            repos.push((r.owner.clone(), r.name.clone()));
        }
    }
    let _ = issue_repos.send(repos);
    for (range, found) in searched {
        sh.report.borrow_mut().issues_found += found.items.len() as u64;
        // An issue whose update time is unchanged since it was last fetched
        // completely has no new history.
        let mut changed = Vec::new();
        for r in found.items {
            let key = issue_cursor(&r.node_id);
            let seen = sh.store.borrow().cursor(&subject.login, &key)?;
            if !opts.full && seen.as_deref() == Some(r.updated_at.to_string().as_str()) {
                sh.report.borrow_mut().issues_unchanged += 1;
            } else {
                changed.push(r);
            }
        }
        let mut failures = Vec::new();
        let total = changed.len();
        progress(SyncProgress::Fetching {
            kind: DataKind::Issues,
            done: 0,
            total,
        });
        let mut stream = futures::stream::iter(changed.into_iter().map(|r| async move {
            let res = client.fetch_issue(&r.owner, &r.name, r.number).await;
            (r, res)
        }))
        .buffer_unordered(opts.concurrency.clamp(1, 16));
        let mut done = 0;
        while let Some((r, res)) = stream.next().await {
            done += 1;
            progress(SyncProgress::Fetching {
                kind: DataKind::Issues,
                done,
                total,
            });
            progress(SyncProgress::Detail(format!(
                "issue {}/{}#{}",
                r.owner, r.name, r.number
            )));
            match res {
                Ok(bundle) => {
                    if !bundle.status.is_complete() {
                        failures.push(format!(
                            "{}/{}#{}: issue history partially fetched",
                            r.owner, r.name, r.number
                        ));
                    }
                    let cursors = if bundle.status.is_complete() {
                        vec![(issue_cursor(&r.node_id), r.updated_at.to_string())]
                    } else {
                        Vec::new()
                    };
                    sh.store.borrow_mut().apply(&WriteBatch {
                        account: subject.login.clone(),
                        repositories: vec![bundle.repository],
                        issues: vec![bundle.issue],
                        events: bundle.events,
                        cursors,
                        ..Default::default()
                    })?;
                    sh.report.borrow_mut().issues_fetched += 1;
                }
                Err(e @ ApiError::Unauthorized) => return Err(e.into()),
                Err(e) => {
                    if matches!(e, ApiError::NotFound(_) | ApiError::Forbidden(_)) {
                        sh.store
                            .borrow_mut()
                            .set_repository_accessible(&r.owner, &r.name, false)?;
                    }
                    failures.push(format!("{}/{}#{}: {e}", r.owner, r.name, r.number));
                }
            }
        }
        drop(stream);
        let mut reasons: Vec<String> = found.reason.into_iter().collect();
        if !failures.is_empty() {
            reasons.push(format!("{} issue(s) failed", failures.len()));
        }
        let status = if failures.is_empty() {
            found.status
        } else {
            CoverageStatus::combine([found.status, CoverageStatus::Partial])
        };
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        let rec = record(DataKind::Issues, range, status, reason, subject, opts);
        sh.store.borrow_mut().apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        let mut report = sh.report.borrow_mut();
        report.coverage.push(rec);
        report.failures.extend(failures);
    }
    Ok(())
}

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
        s.push_str(&format!(" and {} more", names.len() - MAX_NAMED_REPOS));
    }
    s
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
#[allow(clippy::too_many_arguments)]
async fn sync_commits(
    sh: &Shared<'_, '_>,
    ranges: &[Period],
    cached_prs: &[CachedPr],
    cached_issue_repos: &HashSet<String>,
    pr_repos: tokio::sync::oneshot::Receiver<Vec<(String, String)>>,
    issue_repos: tokio::sync::oneshot::Receiver<Vec<(String, String)>>,
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
    let mut pr_repo_ids: HashSet<String> = HashSet::new();
    let mut unresolved: Vec<String> = Vec::new();
    for (owner, name) in &pr_repos {
        let local = find_repo(known_repos, owner, name).cloned().or_else(|| {
            pushed
                .iter()
                .map(|p| &p.repository)
                .find(|r| r.owner.eq_ignore_ascii_case(owner) && r.name.eq_ignore_ascii_case(name))
                .cloned()
        });
        if let Some(r) = local {
            pr_repo_ids.insert(r.id.clone());
            known_repos.entry(r.id.clone()).or_insert(r);
            continue;
        }
        match client.repository(owner, name).await {
            Ok(full) => {
                pr_repo_ids.insert(full.id.clone());
                known_repos.insert(full.id.clone(), full);
            }
            Err(e @ ApiError::Unauthorized) => return Err(e.into()),
            Err(_) => unresolved.push(format!("{owner}/{name}")),
        }
    }
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
            .chain(issue_repos.iter().map(|(o, n)| format!("{o}/{n}")))
            .map(|n| n.to_ascii_lowercase())
            .collect(),
    };

    for range in ranges {
        let range = *range;
        let fetched = searches.next().expect("one commit search per range");
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
        if let Some(f) = &listing_failure {
            statuses.push(CoverageStatus::Partial);
            reasons.push(f.clone());
        }
        if !unresolved.is_empty() {
            statuses.push(CoverageStatus::Partial);
            reasons.push(format!(
                "could not look up pull request repositories: {}",
                name_repos(&unresolved)
            ));
        }

        // Candidate repositories for this range.
        let mut candidates: BTreeMap<String, Repository> = BTreeMap::new();
        for p in &pushed {
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
        let mut inaccessible: Vec<(String, String)> = Vec::new();
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
                    inaccessible.push((repo.owner.clone(), repo.name));
                }
                Err(e) => failed.push(format!("{}: {e}", repo.full_name())),
            }
        }
        drop(stream);

        // So far `found` holds only commits of related repositories. A
        // commit found elsewhere with one of their SHAs is a copy.
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
        // Copies stored by earlier versions.
        let stale_copies: Vec<(String, String)> = cached
            .iter()
            .filter(|c| matches!(c.source, Source::Search | Source::Rest))
            .filter(|c| !related.contains_id(&c.repository_id, known))
            .filter(|c| related_shas.contains(&c.sha))
            .map(|c| (c.repository_id.clone(), c.sha.clone()))
            .collect();
        if !inaccessible.is_empty() {
            for (owner, name) in &inaccessible {
                sh.store
                    .borrow_mut()
                    .set_repository_accessible(owner, name, false)?;
            }
            let names: Vec<String> = inaccessible
                .iter()
                .map(|(o, n)| format!("{o}/{n}"))
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
        // Commit search does not report the viewer's permission, which
        // ownership depends on; complete repository metadata once.
        for repo in &mut batch.repositories {
            if let Some(known) = known_repos.get(&repo.id)
                && known.viewer_permission.is_some()
            {
                // Names, visibility and URL come from this fetch, so a
                // renamed repository takes its new name.
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
            sh.store
                .borrow_mut()
                .remove_listed_commits(&subject.login, &stale_copies)?;
        }
        sh.store.borrow_mut().apply(&batch)?;
        sh.report.borrow_mut().coverage.push(rec);
    }
    Ok(())
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
