//! Aggregation engine shared by the CLI, the TUI and every export format.
//!
//! All functions are pure: the same dataset and options always produce the
//! same result, independent of event order or duplicated input.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::coverage::CoverageRecord;
use crate::model::{
    ActivityEvent, Actor, CheckOutcome, EventCategory, EventType, Issue, PrState, PullRequest,
    Repository, ReviewState, ReviewThread, Subject, Visibility,
};
use crate::time::{Bucket, Granularity, Period, Zone, bucket_key, buckets};

/// Everything the engine aggregates over.
///
/// The subject's commit count is the number of distinct `CommitAuthored`
/// events. Callers that also discover the subject's commits through pull
/// requests add them as `CommitAuthored` events with the same
/// [`EventId::commit`](crate::model::EventId::commit) id, so they are
/// counted once alongside commit search results.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub subject: Subject,
    /// Whether the subject is the authenticated viewer (affects ownership).
    pub subject_is_viewer: bool,
    pub repositories: BTreeMap<String, Repository>,
    pub pull_requests: BTreeMap<String, PullRequest>,
    pub issues: BTreeMap<String, Issue>,
    pub threads: BTreeMap<String, ReviewThread>,
    /// Events, deduplicated by event id and sorted by (time, rank, id).
    pub events: Vec<ActivityEvent>,
    pub coverage: Vec<CoverageRecord>,
}

impl Dataset {
    /// Builds a dataset. Duplicate events (same id) are collapsed; the most
    /// recently observed copy is kept so a resync's state wins.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        subject: Subject,
        subject_is_viewer: bool,
        repositories: impl IntoIterator<Item = Repository>,
        pull_requests: impl IntoIterator<Item = PullRequest>,
        issues: impl IntoIterator<Item = Issue>,
        threads: impl IntoIterator<Item = ReviewThread>,
        events: impl IntoIterator<Item = ActivityEvent>,
        coverage: Vec<CoverageRecord>,
    ) -> Self {
        let mut by_id: HashMap<String, ActivityEvent> = HashMap::new();
        for e in events {
            match by_id.get(e.event_id.as_str()) {
                Some(existing)
                    if (existing.observed_at, existing.source.as_str())
                        >= (e.observed_at, e.source.as_str()) => {}
                _ => {
                    by_id.insert(e.event_id.0.clone(), e);
                }
            }
        }
        let mut events: Vec<ActivityEvent> = by_id.into_values().collect();
        mark_thread_reply_reviews(&mut events);
        sort_events(&mut events);
        Self {
            subject,
            subject_is_viewer,
            repositories: repositories
                .into_iter()
                .map(|r| (r.id.clone(), r))
                .collect(),
            pull_requests: pull_requests
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect(),
            issues: issues.into_iter().map(|i| (i.id.clone(), i)).collect(),
            threads: threads.into_iter().map(|t| (t.id.clone(), t)).collect(),
            events,
            coverage,
        }
    }

    pub fn repo_name(&self, repository_id: &str) -> String {
        self.repositories
            .get(repository_id)
            .map(|r| r.full_name())
            .unwrap_or_else(|| format!("unknown:{repository_id}"))
    }

    fn pr_author_is_subject(&self, pr_id: Option<&str>) -> bool {
        pr_id
            .and_then(|id| self.pull_requests.get(id))
            .is_some_and(|pr| pr.author.is(&self.subject))
    }
}

fn review_key(e: &ActivityEvent) -> &str {
    e.meta.review_id.as_deref().unwrap_or(&e.entity_id)
}

/// Marks reviews that only contain replies to existing threads: commented,
/// without a summary, and every inline comment of the review is a reply.
fn mark_thread_reply_reviews(events: &mut [ActivityEvent]) {
    let mut with_summary: HashSet<String> = HashSet::new();
    // review id -> (has inline comments, all of them are replies)
    let mut inline: HashMap<String, (bool, bool)> = HashMap::new();
    for e in events.iter() {
        match e.event_type {
            EventType::ReviewSummary => {
                with_summary.insert(review_key(e).to_string());
            }
            EventType::InlineComment => {
                if let Some(rid) = e.meta.review_id.as_deref() {
                    let entry = inline.entry(rid.to_string()).or_insert((false, true));
                    entry.0 = true;
                    entry.1 &= e.meta.reply_to.is_some();
                }
            }
            _ => {}
        }
    }
    for e in events.iter_mut() {
        if e.event_type == EventType::ReviewSubmitted
            && e.meta.review_state == Some(ReviewState::Commented)
        {
            let key = review_key(e).to_string();
            let reply_only = !with_summary.contains(&key)
                && inline.get(&key).is_some_and(|(has, all)| *has && *all);
            e.meta.thread_reply_only = reply_only.then_some(true);
        }
    }
}

/// Deterministic event order: time, then lifecycle rank, then id.
pub fn sort_events(events: &mut [ActivityEvent]) {
    events.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
}

/// Which date counts as a commit's date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitDateBasis {
    #[default]
    Committer,
    Author,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    External,
}

/// Repository filters. Empty lists mean "no restriction".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// `owner/name` (case-insensitive) or repository id.
    pub repositories: Vec<String>,
    /// Owner or organization login.
    pub owners: Vec<String>,
    pub visibility: Option<VisibilityFilter>,
    pub ownership: Option<Ownership>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityFilter {
    Public,
    Private,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.repositories.is_empty()
            && self.owners.is_empty()
            && self.visibility.is_none()
            && self.ownership.is_none()
    }

    fn matches_repo(&self, ds: &Dataset, repository_id: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let Some(repo) = ds.repositories.get(repository_id) else {
            return false;
        };
        if !self.repositories.is_empty() {
            let full = repo.full_name();
            if !self
                .repositories
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&full) || r == &repo.id)
            {
                return false;
            }
        }
        if !self.owners.is_empty()
            && !self
                .owners
                .iter()
                .any(|o| o.eq_ignore_ascii_case(&repo.owner))
        {
            return false;
        }
        match self.visibility {
            Some(VisibilityFilter::Public) if repo.visibility != Visibility::Public => {
                return false;
            }
            Some(VisibilityFilter::Private) if repo.visibility == Visibility::Public => {
                return false;
            }
            _ => {}
        }
        match self.ownership {
            Some(Ownership::Owned) if !repo.is_owned_by(&ds.subject, ds.subject_is_viewer) => {
                return false;
            }
            Some(Ownership::External) if repo.is_owned_by(&ds.subject, ds.subject_is_viewer) => {
                return false;
            }
            _ => {}
        }
        true
    }
}

/// Aggregation options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub period: Period,
    pub zone: Zone,
    pub filter: Filter,
    pub commit_basis: CommitDateBasis,
}

/// The instant an event is attributed to.
pub fn event_time(e: &ActivityEvent, basis: CommitDateBasis) -> DateTime<Utc> {
    match (e.event_type, basis) {
        (EventType::CommitAuthored | EventType::PrCommit, CommitDateBasis::Author) => {
            e.meta.author_date.unwrap_or(e.occurred_at)
        }
        _ => e.occurred_at,
    }
}

/// Events counted by default: not removed, not pending reviews.
fn is_countable(e: &ActivityEvent) -> bool {
    let not_a_submission = e.event_type == EventType::ReviewSubmitted
        && (e.meta.review_state == Some(ReviewState::Pending)
            || e.meta.thread_reply_only == Some(true));
    !e.is_removed() && !not_a_submission
}

fn selected<'a>(ds: &'a Dataset, opts: &'a Options) -> impl Iterator<Item = &'a ActivityEvent> {
    ds.events.iter().filter(move |e| {
        is_countable(e)
            && opts.period.contains(event_time(e, opts.commit_basis))
            && opts.filter.matches_repo(ds, &e.repository_id)
    })
}

/// Comment counts by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentCounts {
    pub total: u64,
    pub conversation: u64,
    pub inline: u64,
    pub review_summary: u64,
}

impl CommentCounts {
    pub fn total(&self) -> u64 {
        self.total
    }
    fn add(&mut self, t: EventType) {
        if matches!(
            t,
            EventType::ConversationComment | EventType::InlineComment | EventType::ReviewSummary
        ) {
            self.total += 1;
        }
        match t {
            EventType::ConversationComment => self.conversation += 1,
            EventType::InlineComment => self.inline += 1,
            EventType::ReviewSummary => self.review_summary += 1,
            _ => {}
        }
    }
}

/// Review submission counts by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCounts {
    pub total: u64,
    pub approved: u64,
    pub changes_requested: u64,
    pub commented: u64,
    /// Dismissed reviews whose original state could not be observed.
    pub dismissed_unknown_state: u64,
}

impl ReviewCounts {
    fn add(&mut self, state: Option<ReviewState>) {
        self.total += 1;
        match state {
            Some(ReviewState::Approved) => self.approved += 1,
            Some(ReviewState::ChangesRequested) => self.changes_requested += 1,
            Some(ReviewState::Commented) => self.commented += 1,
            Some(ReviewState::Dismissed) | None => self.dismissed_unknown_state += 1,
            Some(ReviewState::Pending) => {}
        }
    }
}

/// CI results observed in the period. Never part of human activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCounts {
    pub passed: u64,
    pub failed: u64,
    pub pending: u64,
    pub other: u64,
}

/// Additive activity counts for a period. Unique counts are labeled as such.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Distinct commits authored by the subject.
    pub commits: u64,
    pub merge_commits: u64,
    pub prs_created: u64,
    /// The subject's pull requests merged in the period (by anyone).
    pub prs_merged: u64,
    /// Merges the subject performed (any author).
    pub merges_performed: u64,
    /// Merges of the subject's pull requests whose merger is unknown.
    pub merges_unknown_actor: u64,
    pub prs_closed_without_merge: u64,
    pub prs_reopened: u64,
    pub reviews_submitted: ReviewCounts,
    /// Unique pull requests the subject reviewed.
    pub reviewed_prs: u64,
    pub reviews_received: ReviewCounts,
    pub reviews_received_from_bots: u64,
    pub review_dismissals_performed: u64,
    pub comments_written: CommentCounts,
    pub comments_received: CommentCounts,
    /// Subject's replies in review threads started by others on the
    /// subject's pull requests.
    pub review_responses: u64,
    /// Distinct pull request commits authored by the subject.
    pub pr_commits: u64,
    pub force_pushes: u64,
    pub coordination_actions: u64,
    pub metadata_actions: u64,
    pub issues_opened: u64,
    pub issues_closed: u64,
    pub issues_reopened: u64,
    /// Unique pull requests with at least one event by the subject.
    pub active_prs: u64,
    /// Unique pull requests with at least one event by anyone.
    pub active_prs_all_participants: u64,
    /// The subject's events inside pull requests (automation excluded).
    pub pr_internal_events: u64,
    pub ci: CiCounts,
    /// Distinct local dates with activity by the subject.
    pub active_days: u64,
    pub last_activity: Option<DateTime<Utc>>,
}

/// Computes a summary over `events` (already filtered to one scope).
fn summarize_events<'a>(
    ds: &Dataset,
    events: impl Iterator<Item = &'a ActivityEvent>,
    zone: Zone,
    basis: CommitDateBasis,
) -> Summary {
    let mut s = Summary::default();
    let subject = &ds.subject;
    let mut commits: HashSet<&str> = HashSet::new();
    let mut pr_commits: HashSet<(&str, &str)> = HashSet::new();
    let mut reviewed: HashSet<&str> = HashSet::new();
    let mut active_self: HashSet<&str> = HashSet::new();
    let mut active_all: HashSet<&str> = HashSet::new();
    let mut days: HashSet<NaiveDate> = HashSet::new();

    for e in events {
        let t = e.event_type;
        let by_subject = e.actor.is(subject);
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !t.is_automation()
        {
            active_all.insert(pr);
            if by_subject {
                active_self.insert(pr);
            }
        }
        if by_subject && !t.is_automation() {
            days.insert(zone.local_date(event_time(e, basis)));
            s.last_activity = s.last_activity.max(Some(event_time(e, basis)));
            if t.is_pr_event() {
                s.pr_internal_events += 1;
            }
        }
        let on_subject_pr = ds.pr_author_is_subject(e.parent_pr_id.as_deref());
        match t {
            EventType::CommitAuthored if by_subject => {
                if commits.insert(e.event_id.as_str()) && e.meta.is_merge_commit == Some(true) {
                    s.merge_commits += 1;
                }
            }
            EventType::PrCommit if by_subject => {
                if let (Some(pr), Some(sha)) = (e.parent_pr_id.as_deref(), e.meta.sha.as_deref()) {
                    pr_commits.insert((pr, sha));
                }
            }
            EventType::PrOpened if by_subject => s.prs_created += 1,
            EventType::PrMerged => {
                if on_subject_pr {
                    s.prs_merged += 1;
                    if e.actor.is_unknown() {
                        s.merges_unknown_actor += 1;
                    }
                }
                if by_subject {
                    s.merges_performed += 1;
                }
            }
            // Only closures known not to be part of a merge.
            EventType::PrClosed if on_subject_pr && e.meta.via_merge == Some(false) => {
                s.prs_closed_without_merge += 1;
            }
            EventType::PrReopened if on_subject_pr => s.prs_reopened += 1,
            EventType::PrHeadForcePushed if by_subject => s.force_pushes += 1,
            EventType::ReviewSubmitted => {
                if by_subject {
                    s.reviews_submitted.add(e.meta.review_state);
                    if let Some(pr) = e.parent_pr_id.as_deref() {
                        reviewed.insert(pr);
                    }
                } else if on_subject_pr {
                    s.reviews_received.add(e.meta.review_state);
                    if e.actor.is_bot() {
                        s.reviews_received_from_bots += 1;
                    }
                }
            }
            EventType::ReviewDismissed if by_subject => s.review_dismissals_performed += 1,
            EventType::ConversationComment
            | EventType::InlineComment
            | EventType::ReviewSummary => {
                if by_subject {
                    s.comments_written.add(t);
                    if t == EventType::InlineComment
                        && on_subject_pr
                        && e.meta.reply_to.is_some()
                        && thread_started_by_other(ds, e)
                    {
                        s.review_responses += 1;
                    }
                } else if on_subject_pr {
                    s.comments_received.add(t);
                }
            }
            t if by_subject && t.category() == EventCategory::PrCoordination => {
                s.coordination_actions += 1;
            }
            t if by_subject && t.category() == EventCategory::PrMetadata => {
                s.metadata_actions += 1;
            }
            EventType::IssueOpened if by_subject => s.issues_opened += 1,
            EventType::IssueClosed if by_subject => s.issues_closed += 1,
            EventType::IssueReopened if by_subject => s.issues_reopened += 1,
            EventType::CheckRun | EventType::CommitStatus => {
                match e.meta.check_outcome.unwrap_or(CheckOutcome::Unknown) {
                    CheckOutcome::Success => s.ci.passed += 1,
                    CheckOutcome::Failure => s.ci.failed += 1,
                    CheckOutcome::Pending => s.ci.pending += 1,
                    _ => s.ci.other += 1,
                }
            }
            _ => {}
        }
    }
    s.commits = commits.len() as u64;
    s.pr_commits = pr_commits.len() as u64;
    s.reviewed_prs = reviewed.len() as u64;
    s.active_prs = active_self.len() as u64;
    s.active_prs_all_participants = active_all.len() as u64;
    s.active_days = days.len() as u64;
    s
}

fn thread_started_by_other(ds: &Dataset, e: &ActivityEvent) -> bool {
    e.meta
        .thread_id
        .as_deref()
        .and_then(|id| ds.threads.get(id))
        .and_then(|t| t.first_comment_author.as_ref())
        .is_some_and(|a| !a.is(&ds.subject) && !a.is_unknown())
}

/// Summary for the whole period.
pub fn summary(ds: &Dataset, opts: &Options) -> Summary {
    summarize_events(ds, selected(ds, opts), opts.zone, opts.commit_basis)
}

/// One calendar bucket and its summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketSummary {
    pub bucket: Bucket,
    pub summary: Summary,
}

/// Per-day/week/month summaries. Buckets partition the period, so additive
/// counts sum to the whole-period summary.
pub fn breakdown(ds: &Dataset, opts: &Options, gran: Granularity) -> Vec<BucketSummary> {
    let bs = buckets(opts.period, opts.zone, gran);
    let mut per: BTreeMap<NaiveDate, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in selected(ds, opts) {
        per.entry(bucket_key(
            event_time(e, opts.commit_basis),
            opts.zone,
            gran,
        ))
        .or_default()
        .push(e);
    }
    bs.into_iter()
        .map(|b| {
            let events = per.remove(&b.key).unwrap_or_default();
            let summary = summarize_events(ds, events.into_iter(), opts.zone, opts.commit_basis);
            BucketSummary { bucket: b, summary }
        })
        .collect()
}

/// Per-repository activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
    pub repository_id: String,
    pub full_name: String,
    pub visibility: Visibility,
    pub owned: bool,
    pub accessible: bool,
    pub summary: Summary,
}

/// Activity per repository, ordered by subject activity then name.
pub fn by_repository(ds: &Dataset, opts: &Options) -> Vec<RepoSummary> {
    let mut per: BTreeMap<&str, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in selected(ds, opts) {
        per.entry(e.repository_id.as_str()).or_default().push(e);
    }
    let mut out: Vec<RepoSummary> = per
        .into_iter()
        .map(|(id, events)| {
            let summary = summarize_events(ds, events.into_iter(), opts.zone, opts.commit_basis);
            let repo = ds.repositories.get(id);
            RepoSummary {
                repository_id: id.to_string(),
                full_name: ds.repo_name(id),
                visibility: repo.map_or(Visibility::Unknown, |r| r.visibility),
                owned: repo.is_some_and(|r| r.is_owned_by(&ds.subject, ds.subject_is_viewer)),
                accessible: repo.is_none_or(|r| r.accessible),
                summary,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        activity_weight(&b.summary)
            .cmp(&activity_weight(&a.summary))
            .then_with(|| a.full_name.cmp(&b.full_name))
    });
    out
}

/// Activity measure used to rank repositories and draw activity charts:
/// commits, the subject's pull request events and issue events.
pub fn activity_weight(s: &Summary) -> u64 {
    s.commits + s.pr_internal_events + s.issues_opened + s.issues_closed + s.issues_reopened
}

/// Snapshot values derived from current state (not historical events).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The subject's pull requests currently open.
    pub open_prs: u64,
    pub open_issues_authored: u64,
    pub threads: ThreadCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCounts {
    pub total: u64,
    pub open: u64,
    pub resolved: u64,
    pub outdated: u64,
}

impl ThreadCounts {
    fn add(&mut self, t: &ReviewThread) {
        self.total += 1;
        if t.is_resolved {
            self.resolved += 1;
        } else {
            self.open += 1;
        }
        if t.is_outdated {
            self.outdated += 1;
        }
    }
}

/// Current-state values. Thread counts cover pull requests active in the
/// period (all participants).
pub fn snapshot(ds: &Dataset, opts: &Options) -> Snapshot {
    let mut s = Snapshot::default();
    for pr in ds.pull_requests.values() {
        if pr.state == PrState::Open
            && pr.author.is(&ds.subject)
            && opts.filter.matches_repo(ds, &pr.repository_id)
        {
            s.open_prs += 1;
        }
    }
    for issue in ds.issues.values() {
        if issue.is_open
            && issue.author.is(&ds.subject)
            && opts.filter.matches_repo(ds, &issue.repository_id)
        {
            s.open_issues_authored += 1;
        }
    }
    let active: BTreeSet<&str> = selected(ds, opts)
        .filter(|e| !e.event_type.is_automation())
        .filter_map(|e| e.parent_pr_id.as_deref())
        .collect();
    for t in ds.threads.values() {
        if active.contains(t.pr_id.as_str()) {
            s.threads.add(t);
        }
    }
    s
}

/// Sort keys for the pull request list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrSort {
    #[default]
    Activity,
    Comments,
    Reviews,
    Created,
    Updated,
}

/// Which pull requests a list covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Participation {
    /// Pull requests where the subject had activity in the period.
    #[default]
    Subject,
    /// Pull requests with any participant's activity in the period.
    All,
}

/// Activity counts for one pull request within the period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRow {
    pub pr_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub state: PrState,
    pub is_draft: bool,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    /// Human events by anyone (automation excluded).
    pub events: u64,
    pub events_by_subject: u64,
    pub commits: u64,
    pub reviews: u64,
    pub comments: u64,
    pub participants: u64,
    /// People who wrote a conversation, inline or review summary comment.
    pub discussion_participants: u64,
    /// Review threads with an inline comment in the period.
    pub threads: u64,
    pub last_activity: Option<DateTime<Utc>>,
    pub latest_ci: Option<CheckOutcome>,
}

/// Pull requests active in the period with per-PR counts.
pub fn pr_rows(
    ds: &Dataset,
    opts: &Options,
    participation: Participation,
    state: Option<PrState>,
    sort: PrSort,
) -> Vec<PrRow> {
    let mut per: BTreeMap<&str, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in selected(ds, opts) {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !e.event_type.is_automation()
        {
            per.entry(pr).or_default().push(e);
        }
    }
    let mut rows: Vec<PrRow> = per
        .into_iter()
        .filter_map(|(pr_id, events)| {
            let pr = ds.pull_requests.get(pr_id)?;
            if state.is_some_and(|s| s != pr.state) {
                return None;
            }
            let by_subject = events.iter().filter(|e| e.actor.is(&ds.subject)).count() as u64;
            if participation == Participation::Subject && by_subject == 0 {
                return None;
            }
            Some(pr_row(ds, pr, &events, by_subject, opts.commit_basis))
        })
        .collect();
    rows.sort_by(|a, b| {
        let key = |r: &PrRow| match sort {
            PrSort::Activity => r.events as i64,
            PrSort::Comments => r.comments as i64,
            PrSort::Reviews => r.reviews as i64,
            PrSort::Created => r.created_at.timestamp(),
            PrSort::Updated => r.last_activity.map_or(i64::MIN, |t| t.timestamp()),
        };
        key(b)
            .cmp(&key(a))
            .then_with(|| a.repository.cmp(&b.repository))
            .then_with(|| a.number.cmp(&b.number))
    });
    rows
}

fn pr_row(
    ds: &Dataset,
    pr: &PullRequest,
    events: &[&ActivityEvent],
    by_subject: u64,
    basis: CommitDateBasis,
) -> PrRow {
    let mut commits: HashSet<&str> = HashSet::new();
    let mut reviews = 0;
    let mut comments = 0;
    let mut participants: HashSet<String> = HashSet::new();
    let mut discussion: HashSet<String> = HashSet::new();
    let mut threads: HashSet<&str> = HashSet::new();
    let mut last = None;
    for e in events {
        if e.event_type.is_discussion()
            && let Some(key) = participant_key(&e.actor)
        {
            discussion.insert(key);
        }
        if e.event_type == EventType::InlineComment
            && let Some(t) = e.meta.thread_id.as_deref()
        {
            threads.insert(t);
        }
        match e.event_type {
            EventType::PrCommit => {
                if let Some(sha) = e.meta.sha.as_deref() {
                    commits.insert(sha);
                }
            }
            EventType::ReviewSubmitted => reviews += 1,
            EventType::ConversationComment
            | EventType::InlineComment
            | EventType::ReviewSummary => comments += 1,
            _ => {}
        }
        if let Some(key) = participant_key(&e.actor) {
            participants.insert(key);
        }
        last = last.max(Some(event_time(e, basis)));
    }
    PrRow {
        pr_id: pr.id.clone(),
        repository: ds.repo_name(&pr.repository_id),
        number: pr.number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        author: pr.author.display().to_string(),
        state: pr.state,
        is_draft: pr.is_draft,
        created_at: pr.created_at,
        merged_at: pr.merged_at,
        events: events.len() as u64,
        events_by_subject: by_subject,
        commits: commits.len() as u64,
        reviews,
        comments,
        participants: participants.len() as u64,
        discussion_participants: discussion.len() as u64,
        threads: threads.len() as u64,
        last_activity: last,
        latest_ci: pr.latest_ci,
    }
}

fn participant_key(a: &Actor) -> Option<String> {
    if a.is_unknown() {
        return None;
    }
    a.id.clone()
        .or_else(|| a.login.as_ref().map(|l| l.to_ascii_lowercase()))
}

/// Descriptive statistics over per-PR values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub count: u64,
    pub min: f64,
    pub median: f64,
    pub mean: f64,
    pub p90: f64,
    pub max: f64,
}

impl Stats {
    pub fn from_values(mut v: Vec<f64>) -> Option<Self> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let n = v.len();
        let pct = |p: f64| {
            // Nearest-rank percentile.
            let rank = ((p * n as f64).ceil() as usize).clamp(1, n);
            v[rank - 1]
        };
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            (v[n / 2 - 1] + v[n / 2]) / 2.0
        };
        Some(Self {
            count: n as u64,
            min: v[0],
            median,
            mean: v.iter().sum::<f64>() / n as f64,
            p90: pct(0.9),
            max: v[n - 1],
        })
    }
}

/// Activity density across pull requests active in the period. These are
/// descriptive statistics, not quality or skill measures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Intensity {
    pub prs: u64,
    pub events_per_pr: Option<Stats>,
    pub reviews_per_pr: Option<Stats>,
    pub comments_per_pr: Option<Stats>,
    pub commits_per_pr: Option<Stats>,
    pub participants_per_pr: Option<Stats>,
    pub discussion_participants_per_pr: Option<Stats>,
    pub threads_per_pr: Option<Stats>,
    pub active_days_per_pr: Option<Stats>,
}

pub fn intensity(ds: &Dataset, opts: &Options, participation: Participation) -> Intensity {
    let rows = pr_rows(ds, opts, participation, None, PrSort::Activity);
    let mut days: HashMap<&str, HashSet<NaiveDate>> = HashMap::new();
    for e in selected(ds, opts) {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !e.event_type.is_automation()
        {
            days.entry(pr)
                .or_default()
                .insert(opts.zone.local_date(event_time(e, opts.commit_basis)));
        }
    }
    let col = |f: &dyn Fn(&PrRow) -> f64| Stats::from_values(rows.iter().map(f).collect());
    Intensity {
        prs: rows.len() as u64,
        events_per_pr: col(&|r| r.events as f64),
        reviews_per_pr: col(&|r| r.reviews as f64),
        comments_per_pr: col(&|r| r.comments as f64),
        commits_per_pr: col(&|r| r.commits as f64),
        participants_per_pr: col(&|r| r.participants as f64),
        discussion_participants_per_pr: col(&|r| r.discussion_participants as f64),
        threads_per_pr: col(&|r| r.threads as f64),
        active_days_per_pr: Stats::from_values(
            rows.iter()
                .map(|r| days.get(r.pr_id.as_str()).map_or(0, |d| d.len()) as f64)
                .collect(),
        ),
    }
}

/// Lifecycle timings of one pull request, from observed events only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrTiming {
    /// Creation to the first submitted review by someone other than the author.
    pub time_to_first_review_secs: Option<i64>,
    pub creation_to_merge_secs: Option<i64>,
    /// First review by another participant to the author's next observed
    /// event of any kind. This is not necessarily a response to the review.
    pub review_to_next_author_activity_secs: Option<i64>,
    pub first_to_last_activity_secs: Option<i64>,
}

pub fn pr_timing(ds: &Dataset, pr: &PullRequest) -> PrTiming {
    let events: Vec<&ActivityEvent> = ds
        .events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr.id.as_str()))
        .collect();
    timing_of(pr, &events)
}

/// Timings from one pull request's events (in dataset order).
fn timing_of(pr: &PullRequest, all: &[&ActivityEvent]) -> PrTiming {
    let events: Vec<&&ActivityEvent> = all
        .iter()
        .filter(|e| is_countable(e) && !e.event_type.is_automation())
        .collect();
    let first_review = events.iter().find(|e| {
        e.event_type == EventType::ReviewSubmitted
            && !e.actor.is_unknown()
            && !same_actor(&e.actor, &pr.author)
    });
    let after_review = first_review.and_then(|r| {
        events
            .iter()
            .find(|e| e.occurred_at > r.occurred_at && same_actor(&e.actor, &pr.author))
            .map(|e| (e.occurred_at - r.occurred_at).num_seconds())
    });
    let secs = |d: Duration| d.num_seconds();
    PrTiming {
        time_to_first_review_secs: first_review.map(|r| secs(r.occurred_at - pr.created_at)),
        creation_to_merge_secs: pr.merged_at.map(|m| secs(m - pr.created_at)),
        review_to_next_author_activity_secs: after_review,
        first_to_last_activity_secs: match (events.first(), events.last()) {
            (Some(a), Some(b)) => Some(secs(b.occurred_at - a.occurred_at)),
            _ => None,
        },
    }
}

fn same_actor(a: &Actor, b: &Actor) -> bool {
    if a.is_unknown() || b.is_unknown() {
        return false;
    }
    match (&a.id, &b.id) {
        (Some(x), Some(y)) => x == y,
        _ => match (&a.login, &b.login) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => false,
        },
    }
}

/// Timing statistics (seconds) over the subject's pull requests active in
/// the period.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimingStats {
    pub time_to_first_review: Option<Stats>,
    pub creation_to_merge: Option<Stats>,
    pub review_to_next_author_activity: Option<Stats>,
    pub first_to_last_activity: Option<Stats>,
}

pub fn timing_stats(ds: &Dataset, opts: &Options) -> TimingStats {
    let rows = pr_rows(ds, opts, Participation::All, None, PrSort::Activity);
    let wanted: HashSet<&str> = rows
        .iter()
        .filter_map(|r| ds.pull_requests.get(&r.pr_id))
        .filter(|pr| pr.author.is(&ds.subject))
        .map(|pr| pr.id.as_str())
        .collect();
    // Group each wanted pull request's events in one pass.
    let mut per: HashMap<&str, Vec<&ActivityEvent>> = HashMap::new();
    for e in &ds.events {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && wanted.contains(pr)
        {
            per.entry(pr).or_default().push(e);
        }
    }
    let mut ids: Vec<&str> = wanted.into_iter().collect();
    ids.sort_unstable();
    let timings: Vec<PrTiming> = ids
        .into_iter()
        .filter_map(|id| {
            let pr = ds.pull_requests.get(id)?;
            Some(timing_of(pr, per.get(id).map_or(&[][..], |v| v.as_slice())))
        })
        .collect();
    let col = |f: &dyn Fn(&PrTiming) -> Option<i64>| {
        Stats::from_values(timings.iter().filter_map(f).map(|v| v as f64).collect())
    };
    TimingStats {
        time_to_first_review: col(&|t| t.time_to_first_review_secs),
        creation_to_merge: col(&|t| t.creation_to_merge_secs),
        review_to_next_author_activity: col(&|t| t.review_to_next_author_activity_secs),
        first_to_last_activity: col(&|t| t.first_to_last_activity_secs),
    }
}

/// Lifetime detail of one pull request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrDetail {
    pub pr: PullRequest,
    pub repository: String,
    /// Commits currently in the pull request.
    pub commits: u64,
    /// Commits observed earlier but no longer in the pull request.
    pub removed_commits: u64,
    pub reviews: ReviewCounts,
    pub comments: CommentCounts,
    pub threads: ThreadCounts,
    pub participants: Vec<String>,
    /// People who wrote a conversation, inline or review summary comment.
    pub discussion_participants: Vec<String>,
    /// Current commits by commit author, most commits first.
    pub commit_authors: Vec<AuthorCount>,
    pub timing: PrTiming,
    pub timeline: Vec<ActivityEvent>,
}

/// Number of commits by one author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorCount {
    pub author: String,
    pub commits: u64,
}

/// Full detail and timeline of a pull request. `types` filters the
/// timeline only; counts always cover every event.
pub fn pr_detail(ds: &Dataset, pr_id: &str, types: &[EventType]) -> Option<PrDetail> {
    let pr = ds.pull_requests.get(pr_id)?;
    let mut commits = HashSet::new();
    let mut removed_commits = HashSet::new();
    let mut reviews = ReviewCounts::default();
    let mut comments = CommentCounts::default();
    let mut participants = BTreeSet::new();
    let mut discussion = BTreeSet::new();
    let mut authors: HashMap<String, String> = HashMap::new();
    let mut timeline = Vec::new();
    for e in ds
        .events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr_id))
    {
        if e.event_type == EventType::PrCommit {
            let sha = e.meta.sha.clone().unwrap_or_default();
            if e.is_removed() {
                removed_commits.insert(sha);
            } else {
                authors.insert(sha.to_ascii_lowercase(), e.actor.display().to_string());
                commits.insert(sha);
            }
        }
        if is_countable(e) {
            match e.event_type {
                EventType::ReviewSubmitted => reviews.add(e.meta.review_state),
                t @ (EventType::ConversationComment
                | EventType::InlineComment
                | EventType::ReviewSummary) => comments.add(t),
                _ => {}
            }
            if !e.event_type.is_automation() && !e.actor.is_unknown() {
                participants.insert(e.actor.display().to_string());
                if e.event_type.is_discussion() {
                    discussion.insert(e.actor.display().to_string());
                }
            }
        }
        if types.is_empty() || types.contains(&e.event_type) {
            timeline.push(e.clone());
        }
    }
    let mut by_author: BTreeMap<String, u64> = BTreeMap::new();
    for author in authors.into_values() {
        *by_author.entry(author).or_default() += 1;
    }
    let mut commit_authors: Vec<AuthorCount> = by_author
        .into_iter()
        .map(|(author, commits)| AuthorCount { author, commits })
        .collect();
    commit_authors.sort_by(|a, b| b.commits.cmp(&a.commits).then(a.author.cmp(&b.author)));
    let mut threads = ThreadCounts::default();
    for t in ds.threads.values().filter(|t| t.pr_id == pr_id) {
        threads.add(t);
    }
    Some(PrDetail {
        pr: pr.clone(),
        repository: ds.repo_name(&pr.repository_id),
        commits: commits.len() as u64,
        removed_commits: removed_commits.len() as u64,
        reviews,
        comments,
        threads,
        participants: participants.into_iter().collect(),
        discussion_participants: discussion.into_iter().collect(),
        commit_authors,
        timing: pr_timing(ds, pr),
        timeline,
    })
}

/// A review row for the reviews view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRow {
    pub event_id: String,
    pub submitted_at: DateTime<Utc>,
    pub reviewer: String,
    pub state: Option<ReviewState>,
    pub repository: String,
    /// The reviewed pull request, when known.
    pub pr_id: Option<String>,
    pub pr_number: Option<u64>,
    pub pr_title: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDirection {
    Submitted,
    Received,
}

/// Reviews submitted by the subject, or received on the subject's PRs.
pub fn review_rows(ds: &Dataset, opts: &Options, dir: ReviewDirection) -> Vec<ReviewRow> {
    selected(ds, opts)
        .filter(|e| e.event_type == EventType::ReviewSubmitted)
        .filter(|e| match dir {
            ReviewDirection::Submitted => e.actor.is(&ds.subject),
            ReviewDirection::Received => {
                !e.actor.is(&ds.subject) && ds.pr_author_is_subject(e.parent_pr_id.as_deref())
            }
        })
        .map(|e| {
            let pr = e
                .parent_pr_id
                .as_deref()
                .and_then(|id| ds.pull_requests.get(id));
            ReviewRow {
                event_id: e.event_id.0.clone(),
                submitted_at: e.occurred_at,
                reviewer: e.actor.display().to_string(),
                state: e.meta.review_state,
                repository: ds.repo_name(&e.repository_id),
                pr_id: e.parent_pr_id.clone(),
                pr_number: pr.map(|p| p.number),
                pr_title: pr.map(|p| p.title.clone()),
                url: e.url.clone(),
            }
        })
        .collect()
}

/// Issue activity row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRow {
    pub issue_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub is_open: bool,
    pub opened_in_period: bool,
    pub closed_in_period: u64,
    pub reopened_in_period: u64,
}

/// Issues with subject activity in the period. Pull requests never appear.
pub fn issue_rows(ds: &Dataset, opts: &Options) -> Vec<IssueRow> {
    let mut per: BTreeMap<&str, IssueRow> = BTreeMap::new();
    for e in selected(ds, opts) {
        if e.event_type.category() != EventCategory::Issue || !e.actor.is(&ds.subject) {
            continue;
        }
        let Some(issue) = ds.issues.get(&e.entity_id) else {
            continue;
        };
        let row = per.entry(issue.id.as_str()).or_insert_with(|| IssueRow {
            issue_id: issue.id.clone(),
            repository: ds.repo_name(&issue.repository_id),
            number: issue.number,
            title: issue.title.clone(),
            url: issue.url.clone(),
            is_open: issue.is_open,
            opened_in_period: false,
            closed_in_period: 0,
            reopened_in_period: 0,
        });
        match e.event_type {
            EventType::IssueOpened => row.opened_in_period = true,
            EventType::IssueClosed => row.closed_in_period += 1,
            EventType::IssueReopened => row.reopened_in_period += 1,
            _ => {}
        }
    }
    let mut rows: Vec<IssueRow> = per.into_values().collect();
    rows.sort_by(|a, b| {
        a.repository
            .cmp(&b.repository)
            .then(a.number.cmp(&b.number))
    });
    rows
}

/// A comment row for the comments view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRow {
    pub event_id: String,
    pub created_at: DateTime<Utc>,
    pub author: String,
    pub kind: EventType,
    pub written_by_subject: bool,
    pub repository: String,
    /// The pull request the comment belongs to, when known.
    pub pr_id: Option<String>,
    pub pr_number: Option<u64>,
    pub edited: bool,
    pub url: Option<String>,
}

/// Comments written by the subject or received on the subject's PRs.
pub fn comment_rows(ds: &Dataset, opts: &Options) -> Vec<CommentRow> {
    selected(ds, opts)
        .filter(|e| {
            matches!(
                e.event_type,
                EventType::ConversationComment
                    | EventType::InlineComment
                    | EventType::ReviewSummary
            )
        })
        .filter(|e| e.actor.is(&ds.subject) || ds.pr_author_is_subject(e.parent_pr_id.as_deref()))
        .map(|e| CommentRow {
            event_id: e.event_id.0.clone(),
            created_at: e.occurred_at,
            author: e.actor.display().to_string(),
            kind: e.event_type,
            written_by_subject: e.actor.is(&ds.subject),
            repository: ds.repo_name(&e.repository_id),
            pr_id: e.parent_pr_id.clone(),
            pr_number: e
                .parent_pr_id
                .as_deref()
                .and_then(|id| ds.pull_requests.get(id))
                .map(|p| p.number),
            edited: e.meta.edited_at.is_some(),
            url: e.url.clone(),
        })
        .collect()
}

/// A commit authored by the subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRow {
    pub event_id: String,
    pub sha: Option<String>,
    /// The date the commit is attributed to (see [`CommitDateBasis`]).
    pub committed_at: DateTime<Utc>,
    pub repository: String,
    pub is_merge_commit: bool,
    pub url: Option<String>,
}

/// The subject's distinct commits in the period, oldest first. There is
/// exactly one row per commit counted in [`Summary::commits`].
pub fn commit_rows(ds: &Dataset, opts: &Options) -> Vec<CommitRow> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut rows: Vec<CommitRow> = selected(ds, opts)
        .filter(|e| e.event_type == EventType::CommitAuthored && e.actor.is(&ds.subject))
        .filter(|e| seen.insert(e.event_id.as_str()))
        .map(|e| CommitRow {
            event_id: e.event_id.0.clone(),
            sha: e.meta.sha.clone(),
            committed_at: event_time(e, opts.commit_basis),
            repository: ds.repo_name(&e.repository_id),
            is_merge_commit: e.meta.is_merge_commit == Some(true),
            url: e.url.clone(),
        })
        .collect();
    rows.sort_by(|a, b| (a.committed_at, &a.event_id).cmp(&(b.committed_at, &b.event_id)));
    rows
}

/// One event in an activity listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRow {
    pub event_id: String,
    /// The instant the event is attributed to (see [`event_time`]).
    pub occurred_at: DateTime<Utc>,
    pub event_type: EventType,
    pub actor: String,
    pub by_subject: bool,
    pub repository: String,
    /// The pull request the event belongs to, when known.
    pub pr_id: Option<String>,
    /// The issue the event belongs to, when known.
    pub issue_id: Option<String>,
    /// Number of the pull request or issue.
    pub number: Option<u64>,
    /// Title of the pull request or issue.
    pub title: Option<String>,
    pub url: Option<String>,
}

fn activity_row(ds: &Dataset, e: &ActivityEvent, basis: CommitDateBasis) -> ActivityRow {
    let pr = e
        .parent_pr_id
        .as_deref()
        .and_then(|id| ds.pull_requests.get(id));
    let issue = (e.event_type.category() == EventCategory::Issue)
        .then(|| ds.issues.get(&e.entity_id))
        .flatten();
    ActivityRow {
        event_id: e.event_id.0.clone(),
        occurred_at: event_time(e, basis),
        event_type: e.event_type,
        actor: e.actor.display().to_string(),
        by_subject: e.actor.is(&ds.subject),
        repository: ds.repo_name(&e.repository_id),
        pr_id: e.parent_pr_id.clone(),
        issue_id: issue.map(|i| i.id.clone()),
        number: pr.map(|p| p.number).or(issue.map(|i| i.number)),
        title: pr
            .map(|p| p.title.clone())
            .or(issue.map(|i| i.title.clone())),
        url: e.url.clone(),
    }
}

fn by_time(rows: &mut [ActivityRow]) {
    rows.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
}

/// Activity in the period, oldest first: every human event by the subject,
/// plus reviews and comments others left on the subject's pull requests.
/// Automation (CI) is never listed.
pub fn activity_rows(ds: &Dataset, opts: &Options) -> Vec<ActivityRow> {
    let mut rows: Vec<ActivityRow> = selected(ds, opts)
        .filter(|e| !e.event_type.is_automation())
        .filter(|e| {
            e.actor.is(&ds.subject)
                || (matches!(
                    e.event_type,
                    EventType::ReviewSubmitted
                        | EventType::ConversationComment
                        | EventType::InlineComment
                        | EventType::ReviewSummary
                ) && ds.pr_author_is_subject(e.parent_pr_id.as_deref()))
        })
        .map(|e| activity_row(ds, e, opts.commit_basis))
        .collect();
    by_time(&mut rows);
    rows
}

/// One issue and its events in the period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub issue: Issue,
    pub repository: String,
    /// Events of the issue in the period, by anyone, oldest first.
    pub events: Vec<ActivityRow>,
}

/// Detail of one issue: its current state and its events in the period.
pub fn issue_detail(ds: &Dataset, opts: &Options, issue_id: &str) -> Option<IssueDetail> {
    let issue = ds.issues.get(issue_id)?;
    let mut events: Vec<ActivityRow> = selected(ds, opts)
        .filter(|e| e.event_type.category() == EventCategory::Issue && e.entity_id == issue_id)
        .map(|e| activity_row(ds, e, opts.commit_basis))
        .collect();
    by_time(&mut events);
    Some(IssueDetail {
        issue: issue.clone(),
        repository: ds.repo_name(&issue.repository_id),
        events,
    })
}

/// Events of one pull request in deterministic order, optionally filtered.
pub fn timeline<'a>(ds: &'a Dataset, pr_id: &str, types: &[EventType]) -> Vec<&'a ActivityEvent> {
    ds.events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr_id))
        .filter(|e| types.is_empty() || types.contains(&e.event_type))
        .collect()
}
