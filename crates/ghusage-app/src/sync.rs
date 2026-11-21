//! Synchronization of GitHub data into the local cache.
//!
//! Each pull request and issue is written in its own transaction, so an
//! interrupted sync keeps everything written so far and a rerun continues
//! with what is missing. Coverage records describe exactly which ranges
//! were fetched and how completely.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use ghusage_core::coverage::{self, CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::model::{
    ActivityEvent, Collection, EventId, EventType, PrState, Source, Subject,
};
use ghusage_core::time::Period;
use ghusage_github::{
    ApiError, ApiStats, CommitRecord, GitHubClient, IssueKind, IssueRef, PrBundle,
};
use ghusage_storage::{PrCommitLink, PrSnapshot, Store, WriteBatch};

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
    pub now: DateTime<Utc>,
    /// How far before a missing range to start refetching, to catch data
    /// GitHub indexed late.
    pub overlap: Duration,
    /// Open pull requests not refreshed for this long are refetched.
    pub recheck_open_after: Duration,
    /// Number of pull requests/issues fetched concurrently.
    pub concurrency: usize,
}

impl SyncOptions {
    pub fn new(period: Period, now: DateTime<Utc>) -> Self {
        Self {
            period,
            public_only: false,
            full: false,
            now,
            overlap: Duration::days(3),
            recheck_open_after: Duration::days(7),
            concurrency: 4,
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
    pub failures: Vec<String>,
    pub coverage: Vec<CoverageRecord>,
    pub api: ApiStats,
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
    let mut ranges: Vec<(DateTime<Utc>, DateTime<Utc>)> = summary
        .gaps()
        .map(|g| ((g.start - opts.overlap).max(opts.period.start), g.end))
        .collect();
    ranges.sort();
    let mut merged: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::new();
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

fn scope(kind: DataKind, login: &str, public_only: bool) -> String {
    let vis = if public_only {
        " (public repositories only)"
    } else {
        ""
    };
    match kind {
        DataKind::Commits => format!(
            "commits authored by {login} found by GitHub commit search (default branches of indexed repositories), plus commits of synced pull requests{vis}"
        ),
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
        scope: scope(kind, &subject.login, opts.public_only),
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
    let finished = Utc::now();
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
    let mut report = SyncReport::default();
    let existing = store.coverage(&subject.login)?;

    // Commits.
    for range in ranges_to_fetch(&existing, DataKind::Commits, opts) {
        progress(SyncProgress::Searching(DataKind::Commits));
        let fetched = client
            .search_commits(&subject.login, range, opts.public_only)
            .await?;
        report.commits_found += fetched.items.len() as u64;
        let mut batch = WriteBatch {
            account: subject.login.clone(),
            ..Default::default()
        };
        add_commits(&mut batch, fetched.items);
        let rec = record(
            DataKind::Commits,
            range,
            fetched.status,
            fetched.reason,
            subject,
            opts,
        );
        batch.coverage.push(rec.clone());
        store.apply(&batch)?;
        report.coverage.push(rec);
    }

    // Pull requests.
    let pr_ranges = ranges_to_fetch(&existing, DataKind::PullRequests, opts);
    let mut refs: BTreeMap<String, IssueRef> = BTreeMap::new();
    let mut search_status: Vec<(Period, CoverageStatus, Option<String>)> = Vec::new();
    for range in &pr_ranges {
        progress(SyncProgress::Searching(DataKind::PullRequests));
        let mut statuses = Vec::new();
        let mut reasons = Vec::new();
        for q in [
            format!("involves:{}", subject.login),
            format!("reviewed-by:{}", subject.login),
        ] {
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
    // Periodically recheck open pull requests even if search did not
    // report them as updated.
    let stale_open = stale_open_prs(store, subject, opts)?;
    report.pull_requests_found = refs.len() as u64;

    let states: HashMap<String, (Option<DateTime<Utc>>, CoverageStatus)> = store
        .pr_sync_states()?
        .into_iter()
        .map(|s| (s.pr_id, (s.updated_at, s.coverage)))
        .collect();
    let mut to_fetch: Vec<PrKey> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in refs.values() {
        let unchanged = matches!(
            states.get(&r.node_id),
            Some((Some(u), CoverageStatus::Complete)) if *u == r.updated_at
        );
        if unchanged && !opts.full {
            report.pull_requests_unchanged += 1;
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
        match res {
            Ok(bundle) => {
                if !bundle.status.is_complete() {
                    pr_partial.push(format!("{o}/{n}#{num}: {}", bundle.notes.join(", ")));
                }
                let batch = pr_batch(subject, bundle);
                store.apply(&batch)?;
                report.pull_requests_fetched += 1;
            }
            Err(e @ ApiError::Unauthorized) => return Err(e.into()),
            Err(e) => pr_failures.push(format!("{o}/{n}#{num}: {e}")),
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
        store.apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        report.coverage.push(rec);
    }
    report.failures.extend(pr_failures);
    report.failures.extend(pr_partial);

    // Issues.
    for range in ranges_to_fetch(&existing, DataKind::Issues, opts) {
        progress(SyncProgress::Searching(DataKind::Issues));
        let found = client
            .search_issues(
                &format!("involves:{}", subject.login),
                IssueKind::Issue,
                search_window(range, opts),
                opts.public_only,
            )
            .await?;
        report.issues_found += found.items.len() as u64;
        let mut failures = Vec::new();
        let total = found.items.len();
        let mut stream = futures::stream::iter(found.items.into_iter().map(|r| async move {
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
            match res {
                Ok(bundle) => {
                    store.apply(&WriteBatch {
                        account: subject.login.clone(),
                        repositories: vec![bundle.repository],
                        issues: vec![bundle.issue],
                        events: bundle.events,
                        ..Default::default()
                    })?;
                    report.issues_fetched += 1;
                }
                Err(e @ ApiError::Unauthorized) => return Err(e.into()),
                Err(e) => failures.push(format!("{}/{}#{}: {e}", r.owner, r.name, r.number)),
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
        store.apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        report.coverage.push(rec);
        report.failures.extend(failures);
    }

    report.api = client.stats();
    report.status = if report.is_complete() {
        "complete".into()
    } else {
        "partial".into()
    };
    Ok(report)
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
    let probe = Period::new(opts.now, opts.now + Duration::seconds(1))?;
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
        .unwrap_or_else(Utc::now);
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
        ..Default::default()
    }
}
