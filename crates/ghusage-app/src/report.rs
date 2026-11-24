//! Reports built from the aggregation engine. The CLI (table, JSON, CSV)
//! and the TUI all render these same structures.

use jiff::Timestamp;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeSet;

use ghusage_core::coverage::{self, CoverageStatus, CoverageSummary, DataKind};
use ghusage_core::metrics::{
    self, ActivityRow, BucketSummary, CommentRow, CommitDateBasis, CommitRow, Dataset, Filter,
    Intensity, IssueRow, Participation, PrDetail, PrRow, PrSort, PrTiming, RepoSummary,
    ReviewCounts, ReviewDirection, ReviewRow, Snapshot, Summary, ThreadCounts, TimingStats,
};
use ghusage_core::model::{ActorKind, EventType, PrState, Visibility};
use ghusage_core::time::{Granularity, Period, Zone};

/// Version of the JSON output schema.
pub const SCHEMA_VERSION: u32 = 1;

/// What to report on.
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Time range covered by the report.
    pub period: Period,
    /// Time zone used for dates in the report.
    pub zone: Zone,
    /// Repository selection criteria.
    pub filter: Filter,
    /// Date source used for commit activity.
    pub commit_basis: CommitDateBasis,
    /// Hide names, titles and URLs of non-public repositories.
    pub redact_private: bool,
    /// Time of the last completed synchronization, when available.
    pub synced_at: Option<Timestamp>,
}

/// Account identity included in a report.
#[derive(Debug, Clone, Serialize)]
pub struct AccountInfo {
    /// GitHub login.
    pub login: String,
    /// GitHub node ID, when known.
    pub id: Option<String>,
    /// Whether this is the authenticated viewer.
    pub is_authenticated_user: bool,
}

/// Serialized start and end of a report period.
#[derive(Debug, Clone, Serialize)]
pub struct PeriodInfo {
    /// Inclusive start (RFC 3339 in the report time zone).
    pub start: String,
    /// Exclusive end (RFC 3339 in the report time zone).
    pub end: String,
}

/// Which optional sections to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Summary metrics and repository activity.
    Overview,
    /// Summary grouped by the selected calendar unit.
    Breakdown(Granularity),
    /// Repository activity rows.
    Repositories,
    /// Pull request activity rows with the requested filters.
    PullRequests {
        /// Pull request participation scope.
        participation: Participation,
        /// Optional current state filter.
        state: Option<PrState>,
        /// Sort order for pull request rows.
        sort: PrSort,
    },
    /// Reviews submitted and received by the subject.
    Reviews,
    /// Comments written and received by the subject.
    Comments,
    /// Issues with subject activity.
    Issues,
}

/// A period report. Root fields are always present; sections specific to a
/// command are omitted when not requested.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Version of the serialized report schema.
    pub schema_version: u32,
    /// Account described by the report.
    pub account: AccountInfo,
    /// Report period boundaries.
    pub period: PeriodInfo,
    /// Report time zone name.
    pub timezone: String,
    /// Aggregate activity metrics for the period.
    pub metrics: Summary,
    /// Current-state counts for the account.
    pub snapshot: Snapshot,
    /// Per-repository activity summaries.
    pub repositories: Vec<RepoSummary>,
    /// Activity totals partitioned by calendar bucket.
    pub activity_breakdown: Vec<BucketSummary>,
    /// Calendar unit used for `activity_breakdown`, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granularity: Option<Granularity>,
    /// Coverage state for each data kind in the report period.
    pub coverage: Vec<CoverageSummary>,
    /// Time of the last synchronization, when known.
    pub synced_at: Option<Timestamp>,
    /// Whether private repository details were redacted.
    pub private_repositories_redacted: bool,
    /// Pull request activity rows, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_requests: Option<Vec<PrRow>>,
    /// Per-pull-request activity density, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity: Option<Intensity>,
    /// Pull request lifecycle timing statistics, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingStats>,
    /// Reviews submitted by the subject, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviews_submitted: Option<Vec<ReviewRow>>,
    /// Reviews received on the subject's pull requests, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviews_received: Option<Vec<ReviewRow>>,
    /// Comments written or received, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentRow>>,
    /// Issues with subject activity, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<Vec<IssueRow>>,
    /// Commits authored by the subject, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<Vec<CommitRow>>,
    /// Human activity events, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<Vec<ActivityRow>>,
}

impl Report {
    /// Combined coverage of every data kind for the period.
    #[must_use]
    pub fn overall_coverage(&self) -> CoverageStatus {
        CoverageStatus::combine(self.coverage.iter().map(|c| c.status))
    }
}

fn period_info(ctx: &QueryContext) -> PeriodInfo {
    PeriodInfo {
        start: ctx.zone.rfc3339(ctx.period.start),
        end: ctx.zone.rfc3339(ctx.period.end),
    }
}

fn account(ds: &Dataset) -> AccountInfo {
    AccountInfo {
        login: ds.subject.login.clone(),
        id: ds.subject.id.clone(),
        is_authenticated_user: ds.subject_is_viewer,
    }
}

/// Coverage of each data kind for the period.
#[must_use]
pub fn coverage_for(ds: &Dataset, period: Period) -> Vec<CoverageSummary> {
    DataKind::ALL
        .iter()
        .map(|k| coverage::summarize(&ds.coverage, *k, period.start, period.end))
        .collect()
}

fn options(ctx: &QueryContext, filter: Filter) -> metrics::Options {
    metrics::Options {
        period: ctx.period,
        zone: ctx.zone.clone(),
        filter,
        commit_basis: ctx.commit_basis,
    }
}

/// Builds a report. When redaction is on, private names are replaced
/// before aggregation, so numbers are identical either way.
#[must_use]
pub fn build(ds: &Dataset, ctx: &QueryContext, kind: ReportKind) -> Report {
    let (ds, filter) = prepare(ds, ctx);
    build_on(&ds, &options(ctx, filter), ctx, kind)
}

/// [`build`] on a dataset already passed through [`prepare`].
fn build_on(ds: &Dataset, opts: &metrics::Options, ctx: &QueryContext, kind: ReportKind) -> Report {
    let gran = match kind {
        ReportKind::Breakdown(g) => Some(g),
        _ => None,
    };
    let breakdown = metrics::breakdown(ds, opts, gran.unwrap_or(Granularity::Day));
    let mut r = Report {
        schema_version: SCHEMA_VERSION,
        account: account(ds),
        period: period_info(ctx),
        timezone: ctx.zone.name().to_string(),
        metrics: metrics::summary(ds, opts),
        snapshot: metrics::snapshot(ds, opts),
        repositories: metrics::by_repository(ds, opts),
        activity_breakdown: breakdown,
        granularity: gran,
        coverage: coverage_for(ds, ctx.period),
        synced_at: ctx.synced_at,
        private_repositories_redacted: ctx.redact_private,
        pull_requests: None,
        intensity: None,
        timing: None,
        reviews_submitted: None,
        reviews_received: None,
        comments: None,
        issues: None,
        commits: None,
        activity: None,
    };
    match kind {
        ReportKind::PullRequests {
            participation,
            state,
            sort,
        } => {
            r.pull_requests = Some(metrics::pr_rows(ds, opts, participation, state, sort));
            r.intensity = Some(metrics::intensity(ds, opts, participation));
            r.timing = Some(metrics::timing_stats(ds, opts));
        }
        ReportKind::Reviews => {
            r.reviews_submitted = Some(metrics::review_rows(ds, opts, ReviewDirection::Submitted));
            r.reviews_received = Some(metrics::review_rows(ds, opts, ReviewDirection::Received));
        }
        ReportKind::Comments => r.comments = Some(metrics::comment_rows(ds, opts)),
        ReportKind::Issues => r.issues = Some(metrics::issue_rows(ds, opts)),
        _ => {}
    }
    r
}

/// An overview report that also lists the pull requests the subject was
/// active in, most recently active first.
#[must_use]
pub fn build_dashboard(ds: &Dataset, ctx: &QueryContext) -> Report {
    let (ds, filter) = prepare(ds, ctx);
    let opts = options(ctx, filter);
    let mut r = build_on(&ds, &opts, ctx, ReportKind::Overview);
    r.pull_requests = Some(metrics::pr_rows(
        &ds,
        &opts,
        Participation::Subject,
        None,
        PrSort::Updated,
    ));
    r
}

/// Builds a detailed activity report.
///
/// The result includes pull requests, reviews in both
/// directions, comments, issues, commits and the activity itself. Used for
/// the detail of one day (a one-day period) or one repository (a
/// one-repository filter); its numbers are those of [`build`] for the same
/// context.
#[must_use]
pub fn build_detail(ds: &Dataset, ctx: &QueryContext) -> Report {
    let (ds, filter) = prepare(ds, ctx);
    let ds = &ds;
    let opts = options(ctx, filter);
    let mut r = build_on(
        ds,
        &opts,
        ctx,
        ReportKind::PullRequests {
            participation: Participation::Subject,
            state: None,
            sort: PrSort::Activity,
        },
    );
    r.reviews_submitted = Some(metrics::review_rows(ds, &opts, ReviewDirection::Submitted));
    r.reviews_received = Some(metrics::review_rows(ds, &opts, ReviewDirection::Received));
    r.comments = Some(metrics::comment_rows(ds, &opts));
    r.issues = Some(metrics::issue_rows(ds, &opts));
    r.commits = Some(metrics::commit_rows(ds, &opts));
    r.activity = Some(metrics::activity_rows(ds, &opts));
    r
}

/// Detail report of one issue: its current state and its events in the
/// period.
#[derive(Debug, Clone, Serialize)]
pub struct IssueReport {
    /// GitHub node ID of the issue.
    pub issue_id: String,
    /// Repository display name.
    pub repository: String,
    /// Issue number in the repository.
    pub number: u64,
    /// Current issue title.
    pub title: String,
    /// GitHub URL for the issue.
    pub url: String,
    /// Issue author's login or display name.
    pub author: String,
    /// Whether the issue is currently open.
    pub is_open: bool,
    /// Issue creation time.
    pub created_at: Timestamp,
    /// Issue close time, when applicable.
    pub closed_at: Option<Timestamp>,
    /// Issue events in the report period.
    pub events: Vec<ActivityRow>,
    /// Whether private repository details were redacted.
    pub private_repositories_redacted: bool,
}

/// Builds the detail report of one issue for the period of `ctx`.
#[must_use]
pub fn issue_report(ds: &Dataset, ctx: &QueryContext, issue_id: &str) -> Option<IssueReport> {
    let (ds, filter) = prepare(ds, ctx);
    let d = metrics::issue_detail(&ds, &options(ctx, filter), issue_id)?;
    Some(IssueReport {
        issue_id: d.issue.id.clone(),
        repository: d.repository,
        number: d.issue.number,
        title: d.issue.title.clone(),
        url: d.issue.url.clone(),
        author: d.issue.author.display().to_string(),
        is_open: d.issue.is_open,
        created_at: d.issue.created_at,
        closed_at: d.issue.closed_at,
        events: d.events,
        private_repositories_redacted: ctx.redact_private,
    })
}

/// Resolves the filter to repository ids on the real names, then redacts.
fn prepare<'a>(ds: &'a Dataset, ctx: &QueryContext) -> (Cow<'a, Dataset>, Filter) {
    let filter = if ctx.filter.is_empty() {
        Filter::default()
    } else {
        let probe = metrics::Options {
            period: ctx.period,
            zone: ctx.zone.clone(),
            filter: ctx.filter.clone(),
            commit_basis: ctx.commit_basis,
        };
        let ids: BTreeSet<String> = ds
            .repositories
            .keys()
            .filter(|id| repo_matches(ds, &probe, id))
            .cloned()
            .collect();
        Filter {
            // An impossible id keeps an empty match set empty.
            repositories: if ids.is_empty() {
                vec!["\u{0}none".into()]
            } else {
                ids.into_iter().collect()
            },
            ..Default::default()
        }
    };
    let ds = if ctx.redact_private {
        Cow::Owned(redact(ds))
    } else {
        Cow::Borrowed(ds)
    };
    (ds, filter)
}

fn repo_matches(ds: &Dataset, opts: &metrics::Options, repo_id: &str) -> bool {
    let Some(repo) = ds.repositories.get(repo_id) else {
        return false;
    };
    let f = &opts.filter;
    let full = repo.full_name();
    if !f.repositories.is_empty()
        && !f
            .repositories
            .iter()
            .any(|r| r.eq_ignore_ascii_case(&full) || r == &repo.id)
    {
        return false;
    }
    if !f.owners.is_empty() && !f.owners.iter().any(|o| o.eq_ignore_ascii_case(&repo.owner)) {
        return false;
    }
    match f.visibility {
        Some(metrics::VisibilityFilter::Public) if repo.visibility != Visibility::Public => {
            return false;
        }
        Some(metrics::VisibilityFilter::Private) if repo.visibility == Visibility::Public => {
            return false;
        }
        _ => {}
    }
    let owned = repo.is_owned_by(&ds.subject, ds.subject_is_viewer);
    match f.ownership {
        Some(metrics::Ownership::Owned) if !owned => false,
        Some(metrics::Ownership::External) if owned => false,
        _ => true,
    }
}

pub(crate) fn short_hash(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    hex::encode(&d[..4])
}

/// Lower-case `owner/name` of every public repository in `ds`.
#[must_use]
pub fn public_names(ds: &Dataset) -> BTreeSet<String> {
    ds.repositories
        .values()
        .filter(|r| r.visibility.is_public())
        .map(|r| r.full_name().to_ascii_lowercase())
        .collect()
}

/// Replaces every `owner/name` (with an optional `#number`) in `text` that
/// is not a known public repository. Coverage reasons and sync failures
/// name repositories; unknown ones are treated as private.
#[must_use]
pub fn scrub_names(text: &str, public: &BTreeSet<String>) -> String {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if !is_name_char(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_name_char(chars[i]) {
            i += 1;
        }
        if i + 1 < chars.len() && chars[i] == '/' && is_name_char(chars[i + 1]) {
            let mut j = i + 1;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            let full: String = chars[start..j].iter().collect();
            // Trailing dots are punctuation, not part of the name.
            let trimmed = full.trim_end_matches('.');
            let end = start + trimmed.chars().count();
            if public.contains(&trimmed.to_ascii_lowercase()) {
                out.push_str(trimmed);
                i = end;
                continue;
            }
            out.push_str("[private repository]");
            i = end;
            // Drop a following `#number`.
            if i < chars.len() && chars[i] == '#' {
                let mut k = i + 1;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    k += 1;
                }
                if k > i + 1 {
                    i = k;
                }
            }
            continue;
        }
        out.extend(&chars[start..i]);
    }
    out
}

/// Replaces names, titles and URLs of non-public repositories. Owned
/// private repositories keep the subject as owner so ownership is stable.
#[must_use]
pub fn redact(ds: &Dataset) -> Dataset {
    let mut out = ds.clone();
    let private: BTreeSet<String> = ds
        .repositories
        .values()
        .filter(|r| !r.visibility.is_public())
        .map(|r| r.id.clone())
        .collect();
    for repo in out.repositories.values_mut() {
        if private.contains(&repo.id) {
            let owned = repo.is_owned_by(&ds.subject, ds.subject_is_viewer);
            if owned {
                repo.owner.clone_from(&ds.subject.login);
            } else {
                repo.owner = "private".into();
            }
            // Keep ownership decisions identical after renaming.
            if owned {
                repo.viewer_permission = Some("ADMIN".into());
            }
            repo.name = format!("repo-{}", short_hash(&repo.id));
            repo.url = None;
        }
    }
    for pr in out.pull_requests.values_mut() {
        if private.contains(&pr.repository_id) {
            pr.title = "[private]".into();
            pr.url = String::new();
        }
    }
    for issue in out.issues.values_mut() {
        if private.contains(&issue.repository_id) {
            issue.title = "[private]".into();
            issue.url = String::new();
        }
    }
    for t in out.threads.values_mut() {
        if ds
            .pull_requests
            .get(&t.pr_id)
            .is_some_and(|pr| private.contains(&pr.repository_id))
        {
            t.path = None;
        }
    }
    let public = public_names(ds);
    for c in &mut out.coverage {
        if let Some(r) = &c.reason {
            c.reason = Some(scrub_names(r, &public));
        }
    }
    for e in &mut out.events {
        if private.contains(&e.repository_id) {
            e.url = None;
            e.meta.previous_title = None;
            if matches!(
                e.event_type,
                EventType::Renamed
                    | EventType::Labeled
                    | EventType::Unlabeled
                    | EventType::Milestoned
                    | EventType::Demilestoned
            ) {
                e.meta.target = None;
            }
            e.meta.check_name = None;
            e.meta.message_headline = None;
        }
    }
    out
}

/// One timeline row of a pull request.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEntry {
    /// Stable event identifier.
    pub event_id: String,
    /// Time attributed to the event.
    pub occurred_at: Timestamp,
    /// Time this event was observed from GitHub.
    pub observed_at: Timestamp,
    /// Actor login or display name.
    pub actor: String,
    /// Classification of the event actor.
    pub actor_kind: ActorKind,
    /// Normalized event kind.
    pub event_type: EventType,
    /// Human-readable event label.
    pub label: String,
    /// Event-specific detail for display.
    pub detail: Option<String>,
    /// GitHub URL for the event, when available.
    pub url: Option<String>,
    /// Present when the object was later observed to be gone.
    pub removed_observed_at: Option<Timestamp>,
}

/// Detail report of one pull request.
#[derive(Debug, Clone, Serialize)]
pub struct PrReport {
    /// Version of the serialized report schema.
    pub schema_version: u32,
    /// Account described by the report.
    pub account: AccountInfo,
    /// Time zone used for report timestamps.
    pub timezone: String,
    /// GitHub node ID of the pull request.
    pub pr_id: String,
    /// Repository display name.
    pub repository: String,
    /// Pull request number in the repository.
    pub number: u64,
    /// Current pull request title.
    pub title: String,
    /// GitHub URL for the pull request.
    pub url: String,
    /// Pull request author's login or display name.
    pub author: String,
    /// Current pull request state.
    pub state: PrState,
    /// Whether the pull request is currently a draft.
    pub is_draft: bool,
    /// Pull request creation time.
    pub created_at: Timestamp,
    /// Merge time, when merged.
    pub merged_at: Option<Timestamp>,
    /// Merger login, when known.
    pub merged_by: Option<String>,
    /// Close time, when closed.
    pub closed_at: Option<Timestamp>,
    /// Latest combined CI state, when known.
    pub latest_ci: Option<String>,
    /// Current commit count.
    pub commits: u64,
    /// Commits previously present but removed from the pull request.
    pub removed_commits: u64,
    /// Review submissions grouped by state.
    pub reviews: ReviewCounts,
    /// Comments grouped by type.
    pub comments: metrics::CommentCounts,
    /// Current review thread totals.
    pub review_threads: ThreadCounts,
    /// Distinct human participants.
    pub participants: Vec<String>,
    /// Distinct authors of discussion comments.
    pub discussion_participants: Vec<String>,
    /// Commit counts grouped by author.
    pub commit_authors: Vec<metrics::AuthorCount>,
    /// The commits of the pull request, including removed ones. Shown by
    /// the terminal UI; not part of the JSON output.
    #[serde(skip)]
    pub commit_list: Vec<metrics::PrCommitRow>,
    /// Lifecycle timings derived from observed events.
    pub timing: PrTiming,
    /// Base-to-head diff when last fetched (a snapshot of the pull request).
    pub diff: Option<metrics::PrDiff>,
    /// Pull request event timeline.
    pub timeline: Vec<TimelineEntry>,
    /// Completeness of pull request data.
    pub coverage: CoverageStatus,
    /// Time the pull request was last synchronized.
    pub synced_at: Option<Timestamp>,
    /// Whether private repository details were redacted.
    pub private_repositories_redacted: bool,
}

/// Builds the detail report of one pull request.
#[must_use]
pub fn pr_report(
    ds: &Dataset,
    pr_id: &str,
    types: &[EventType],
    zone: Zone,
    redact_private: bool,
) -> Option<PrReport> {
    let ds = if redact_private {
        redact(ds)
    } else {
        ds.clone()
    };
    let d: PrDetail = metrics::pr_detail(&ds, pr_id, types)?;
    let timeline = d
        .timeline
        .iter()
        .map(|e| TimelineEntry {
            event_id: e.event_id.0.clone(),
            occurred_at: e.occurred_at,
            observed_at: e.observed_at,
            actor: e.actor.display().to_string(),
            actor_kind: e.actor.kind,
            event_type: e.event_type,
            label: e.event_type.label().to_string(),
            detail: event_detail(e),
            url: e.url.clone(),
            removed_observed_at: e.meta.removed_observed_at,
        })
        .collect();
    let pr = d.pr;
    Some(PrReport {
        schema_version: SCHEMA_VERSION,
        account: account(&ds),
        timezone: zone.name().to_string(),
        pr_id: pr.id.clone(),
        repository: d.repository,
        number: pr.number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        author: pr.author.display().to_string(),
        state: pr.state,
        is_draft: pr.is_draft,
        created_at: pr.created_at,
        merged_at: pr.merged_at,
        merged_by: pr.merged_by.as_ref().map(|a| a.display().to_string()),
        closed_at: pr.closed_at,
        latest_ci: pr.latest_ci.map(|c| c.as_str().to_string()),
        commits: d.commits,
        removed_commits: d.removed_commits,
        reviews: d.reviews,
        comments: d.comments,
        review_threads: d.threads,
        participants: d.participants,
        discussion_participants: d.discussion_participants,
        commit_authors: d.commit_authors,
        commit_list: metrics::pr_commit_rows(&ds, pr_id),
        timing: d.timing,
        diff: d.diff,
        timeline,
        coverage: pr.coverage,
        synced_at: pr.synced_at,
        private_repositories_redacted: redact_private,
    })
}

fn event_detail(e: &ghusage_core::model::ActivityEvent) -> Option<String> {
    let m = &e.meta;
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = m.review_state {
        parts.push(s.as_str().to_ascii_lowercase());
    }
    if let Some(sha) = &m.sha {
        parts.push(sha.chars().take(7).collect());
    }
    if let Some(t) = &m.target {
        parts.push(t.clone());
    }
    if let Some(p) = &m.previous_title {
        parts.push(format!("was: {p}"));
    }
    if let (Some(n), Some(o)) = (&m.check_name, m.check_outcome) {
        parts.push(format!("{n}: {}", o.as_str()));
    } else if let Some(o) = m.check_outcome {
        parts.push(o.as_str().to_string());
    }
    if m.thread_reply_only == Some(true) {
        parts.push("thread reply only, not counted as a review".into());
    }
    if m.edited_at.is_some() {
        parts.push("edited".into());
    }
    if m.via_merge == Some(true) {
        parts.push("via merge".into());
    }
    if m.removed_observed_at.is_some() {
        parts.push("no longer present".into());
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

#[cfg(test)]
mod scrub_tests {
    use super::scrub_names;
    use std::collections::BTreeSet;

    #[test]
    fn only_public_repository_names_survive() {
        let public = BTreeSet::from(["octo/app".to_string()]);
        assert_eq!(
            scrub_names(
                "2 repositories not accessible: acme/secret, Octo/App; acme/secret#4: 404.",
                &public
            ),
            "2 repositories not accessible: [private repository], Octo/App; [private repository]: 404."
        );
        assert_eq!(
            scrub_names("searching 2026-11-01..2026-11-02 for me", &public),
            "searching 2026-11-01..2026-11-02 for me"
        );
        assert_eq!(
            scrub_names("ends with acme/x.", &public),
            "ends with [private repository]."
        );
    }
}
