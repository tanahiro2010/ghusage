//! Normalized activity model.
//!
//! Every piece of GitHub activity is represented as an [`ActivityEvent`] with a
//! stable [`EventId`]. Current entity state (a pull request's present state,
//! a review thread's resolution, the latest CI result) lives in separate
//! structs and is never used as a substitute for historical events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::coverage::CoverageStatus;

/// Stable unique identifier of an event. Identical events fetched from
/// different APIs map to the same id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub String);

impl EventId {
    pub fn commit(repository_id: &str, sha: &str) -> Self {
        Self(format!(
            "commit:{repository_id}:{}",
            sha.to_ascii_lowercase()
        ))
    }
    pub fn pr_created(pr_id: &str) -> Self {
        Self(format!("pr:{pr_id}:created"))
    }
    pub fn pr_merged(pr_id: &str) -> Self {
        Self(format!("pr:{pr_id}:merged"))
    }
    pub fn pr_commit(pr_id: &str, sha: &str) -> Self {
        Self(format!("pr-commit:{pr_id}:{}", sha.to_ascii_lowercase()))
    }
    pub fn review(review_id: &str) -> Self {
        Self(format!("review:{review_id}"))
    }
    pub fn review_summary(review_id: &str) -> Self {
        Self(format!("review-summary:{review_id}"))
    }
    pub fn comment(comment_id: &str) -> Self {
        Self(format!("comment:{comment_id}"))
    }
    pub fn timeline(node_id: &str) -> Self {
        Self(format!("tl:{node_id}"))
    }
    pub fn issue_created(issue_id: &str) -> Self {
        Self(format!("issue:{issue_id}:created"))
    }
    pub fn check_run(node_id: &str) -> Self {
        Self(format!("check:{node_id}"))
    }
    pub fn commit_status(node_id: &str) -> Self {
        Self(format!("status:{node_id}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of account performed an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Bot,
    Organization,
    Mannequin,
    /// The actor could not be determined (deleted account, missing field).
    Unknown,
}

/// The account that performed an event. `id`/`login` are `None` when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Actor {
    pub id: Option<String>,
    pub login: Option<String>,
    pub kind: ActorKind,
}

impl Actor {
    pub fn unknown() -> Self {
        Self {
            id: None,
            login: None,
            kind: ActorKind::Unknown,
        }
    }

    /// Builds an actor, classifying bots by type name or the `[bot]` suffix.
    pub fn new(id: Option<String>, login: Option<String>, type_name: Option<&str>) -> Self {
        if id.is_none() && login.is_none() {
            return Self::unknown();
        }
        let is_bot_login = login.as_deref().is_some_and(|l| l.ends_with("[bot]"));
        let kind = match type_name {
            Some("Bot") => ActorKind::Bot,
            _ if is_bot_login => ActorKind::Bot,
            Some("Organization") => ActorKind::Organization,
            Some("Mannequin") => ActorKind::Mannequin,
            Some("User") | None => ActorKind::User,
            Some(_) => ActorKind::User,
        };
        Self { id, login, kind }
    }

    pub fn is_bot(&self) -> bool {
        self.kind == ActorKind::Bot
    }

    pub fn is_unknown(&self) -> bool {
        self.kind == ActorKind::Unknown
    }

    pub fn display(&self) -> &str {
        self.login.as_deref().unwrap_or("unknown")
    }

    /// Whether this actor is the given subject. Unknown actors never match.
    /// When both ids are known they decide; the login is only compared when
    /// an id is missing, so a renamed or reused login cannot match.
    pub fn is(&self, subject: &Subject) -> bool {
        match (&self.id, subject.id.as_deref(), &self.login) {
            (Some(id), Some(sid), _) => id == sid,
            (_, _, Some(login)) => login.eq_ignore_ascii_case(&subject.login),
            _ => false,
        }
    }
}

/// The user whose activity is being analyzed ("you").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub id: Option<String>,
    pub login: String,
}

/// Event categories from the activity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Commit,
    PrLifecycle,
    PrDevelopment,
    PrReview,
    PrDiscussion,
    PrCoordination,
    PrMetadata,
    Issue,
    Automation,
}

/// Every normalized event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // Commit
    CommitAuthored,
    // PR lifecycle
    PrOpened,
    PrConvertedToDraft,
    PrReadyForReview,
    PrClosed,
    PrReopened,
    PrMerged,
    // PR development
    PrCommit,
    PrHeadForcePushed,
    // PR review
    ReviewSubmitted,
    ReviewDismissed,
    // PR discussion
    ConversationComment,
    InlineComment,
    ReviewSummary,
    // PR coordination
    ReviewRequested,
    ReviewRequestRemoved,
    Assigned,
    Unassigned,
    // PR metadata
    Labeled,
    Unlabeled,
    Milestoned,
    Demilestoned,
    Renamed,
    // Issue
    IssueOpened,
    IssueClosed,
    IssueReopened,
    // Automation
    CheckRun,
    CommitStatus,
}

impl EventType {
    pub const ALL: [EventType; 28] = [
        EventType::CommitAuthored,
        EventType::PrOpened,
        EventType::PrConvertedToDraft,
        EventType::PrReadyForReview,
        EventType::PrClosed,
        EventType::PrReopened,
        EventType::PrMerged,
        EventType::PrCommit,
        EventType::PrHeadForcePushed,
        EventType::ReviewSubmitted,
        EventType::ReviewDismissed,
        EventType::ConversationComment,
        EventType::InlineComment,
        EventType::ReviewSummary,
        EventType::ReviewRequested,
        EventType::ReviewRequestRemoved,
        EventType::Assigned,
        EventType::Unassigned,
        EventType::Labeled,
        EventType::Unlabeled,
        EventType::Milestoned,
        EventType::Demilestoned,
        EventType::Renamed,
        EventType::IssueOpened,
        EventType::IssueClosed,
        EventType::IssueReopened,
        EventType::CheckRun,
        EventType::CommitStatus,
    ];

    pub fn category(self) -> EventCategory {
        use EventType::*;
        match self {
            CommitAuthored => EventCategory::Commit,
            PrOpened | PrConvertedToDraft | PrReadyForReview | PrClosed | PrReopened | PrMerged => {
                EventCategory::PrLifecycle
            }
            PrCommit | PrHeadForcePushed => EventCategory::PrDevelopment,
            ReviewSubmitted | ReviewDismissed => EventCategory::PrReview,
            ConversationComment | InlineComment | ReviewSummary => EventCategory::PrDiscussion,
            ReviewRequested | ReviewRequestRemoved | Assigned | Unassigned => {
                EventCategory::PrCoordination
            }
            Labeled | Unlabeled | Milestoned | Demilestoned | Renamed => EventCategory::PrMetadata,
            IssueOpened | IssueClosed | IssueReopened => EventCategory::Issue,
            CheckRun | CommitStatus => EventCategory::Automation,
        }
    }

    /// Automation events are never counted as human activity.
    pub fn is_automation(self) -> bool {
        self.category() == EventCategory::Automation
    }

    /// Events that belong to a pull request.
    pub fn is_pr_event(self) -> bool {
        !matches!(
            self.category(),
            EventCategory::Commit | EventCategory::Issue
        )
    }

    /// Ordering used to break ties between events at the same timestamp,
    /// following the natural order of a pull request's life.
    pub fn sort_rank(self) -> u8 {
        use EventType::*;
        match self {
            PrOpened | IssueOpened => 0,
            CommitAuthored | PrCommit => 1,
            PrHeadForcePushed => 2,
            PrConvertedToDraft | PrReadyForReview => 3,
            ReviewRequested | ReviewRequestRemoved | Assigned | Unassigned => 4,
            Labeled | Unlabeled | Milestoned | Demilestoned | Renamed => 5,
            CheckRun | CommitStatus => 6,
            InlineComment => 7,
            ReviewSummary => 8,
            ReviewSubmitted => 9,
            ReviewDismissed => 10,
            ConversationComment => 11,
            PrMerged => 12,
            PrClosed | IssueClosed => 13,
            PrReopened | IssueReopened => 14,
        }
    }

    pub fn as_str(self) -> &'static str {
        use EventType::*;
        match self {
            CommitAuthored => "commit_authored",
            PrOpened => "pr_opened",
            PrConvertedToDraft => "pr_converted_to_draft",
            PrReadyForReview => "pr_ready_for_review",
            PrClosed => "pr_closed",
            PrReopened => "pr_reopened",
            PrMerged => "pr_merged",
            PrCommit => "pr_commit",
            PrHeadForcePushed => "pr_head_force_pushed",
            ReviewSubmitted => "review_submitted",
            ReviewDismissed => "review_dismissed",
            ConversationComment => "conversation_comment",
            InlineComment => "inline_comment",
            ReviewSummary => "review_summary",
            ReviewRequested => "review_requested",
            ReviewRequestRemoved => "review_request_removed",
            Assigned => "assigned",
            Unassigned => "unassigned",
            Labeled => "labeled",
            Unlabeled => "unlabeled",
            Milestoned => "milestoned",
            Demilestoned => "demilestoned",
            Renamed => "renamed",
            IssueOpened => "issue_opened",
            IssueClosed => "issue_closed",
            IssueReopened => "issue_reopened",
            CheckRun => "check_run",
            CommitStatus => "commit_status",
        }
    }

    pub fn label(self) -> &'static str {
        use EventType::*;
        match self {
            CommitAuthored => "Commit",
            PrOpened => "PR Created",
            PrConvertedToDraft => "Converted to Draft",
            PrReadyForReview => "Ready for Review",
            PrClosed => "PR Closed",
            PrReopened => "PR Reopened",
            PrMerged => "PR Merged",
            PrCommit => "Commit (committed)",
            PrHeadForcePushed => "Force Pushed",
            ReviewSubmitted => "Review",
            ReviewDismissed => "Review Dismissed",
            ConversationComment => "Comment",
            InlineComment => "Inline Comment",
            ReviewSummary => "Review Summary",
            ReviewRequested => "Review Requested",
            ReviewRequestRemoved => "Review Request Removed",
            Assigned => "Assigned",
            Unassigned => "Unassigned",
            Labeled => "Labeled",
            Unlabeled => "Unlabeled",
            Milestoned => "Milestone Set",
            Demilestoned => "Milestone Removed",
            Renamed => "Title Changed",
            IssueOpened => "Issue Opened",
            IssueClosed => "Issue Closed",
            IssueReopened => "Issue Reopened",
            CheckRun => "Check Run",
            CommitStatus => "Commit Status",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        // "commit" is accepted for caches written by earlier versions.
        if s == "commit" {
            return Some(EventType::CommitAuthored);
        }
        Self::ALL.iter().copied().find(|t| t.as_str() == s)
    }
}

/// Where an event was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Rest,
    Graphql,
    Search,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Rest => "rest",
            Source::Graphql => "graphql",
            Source::Search => "search",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rest" => Some(Source::Rest),
            "graphql" => Some(Source::Graphql),
            "search" => Some(Source::Search),
            _ => None,
        }
    }
}

/// Submitted review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    /// Review was dismissed and its original state could not be observed.
    Dismissed,
    /// Draft review that has not been submitted. Never counted.
    Pending,
}

impl ReviewState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "APPROVED" => Some(Self::Approved),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "COMMENTED" => Some(Self::Commented),
            "DISMISSED" => Some(Self::Dismissed),
            "PENDING" => Some(Self::Pending),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::ChangesRequested => "CHANGES_REQUESTED",
            Self::Commented => "COMMENTED",
            Self::Dismissed => "DISMISSED",
            Self::Pending => "PENDING",
        }
    }
}

/// Outcome of a CI check or commit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Success,
    Failure,
    Pending,
    Neutral,
    Skipped,
    Cancelled,
    Unknown,
}

impl CheckOutcome {
    /// Maps GitHub check-run status/conclusion or commit-status state.
    pub fn from_github(status: Option<&str>, conclusion: Option<&str>) -> Self {
        match conclusion.map(|c| c.to_ascii_lowercase()) {
            Some(c) => match c.as_str() {
                "success" => Self::Success,
                "failure" | "timed_out" | "startup_failure" | "action_required" | "error" => {
                    Self::Failure
                }
                "neutral" => Self::Neutral,
                "skipped" | "stale" => Self::Skipped,
                "cancelled" => Self::Cancelled,
                "pending" | "expected" => Self::Pending,
                _ => Self::Unknown,
            },
            None => match status.map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("queued" | "in_progress" | "waiting" | "requested" | "pending") => {
                    Self::Pending
                }
                Some("completed") | None | Some(_) => Self::Unknown,
            },
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Pending => "pending",
            Self::Neutral => "neutral",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "pending" => Self::Pending,
            "neutral" => Self::Neutral,
            "skipped" => Self::Skipped,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

/// Type-specific event information. All fields are optional; absent means
/// "not applicable or not observed", never zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Commit author date (the event's `occurred_at` is the committer date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_date: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_merge_commit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committer: Option<Actor>,
    /// For PR commits: `occurred_at` is the commit time, not when it was added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_state: Option<ReviewState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// For inline comments: the comment this replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_outcome: Option<CheckOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_suite_id: Option<String>,
    /// For a PR closed event: whether the close happened as part of a merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_merge: Option<bool>,
    /// Target of coordination events (requested reviewer, assignee, label...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<DateTime<Utc>>,
    /// Set on a `ReviewSubmitted` event when the review only carries replies
    /// to existing review threads (GitHub records every thread reply as a
    /// review with no body). Such reviews are not counted as submissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_reply_only: Option<bool>,
    /// Set when a resync observed that the source object no longer exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_observed_at: Option<DateTime<Utc>>,
}

/// A normalized activity event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub event_id: EventId,
    pub event_type: EventType,
    pub actor: Actor,
    pub repository_id: String,
    /// Id of the source entity (commit sha, review id, comment id, ...).
    pub entity_id: String,
    pub parent_pr_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub source: Source,
    pub url: Option<String>,
    pub meta: EventMeta,
    pub coverage: CoverageStatus,
}

impl ActivityEvent {
    /// Whether the event is excluded from counts because the source object
    /// was observed to be removed.
    pub fn is_removed(&self) -> bool {
        self.meta.removed_observed_at.is_some()
    }
}

/// Repository visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
    Internal,
    Unknown,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "public" => Self::Public,
            "private" => Self::Private,
            "internal" => Self::Internal,
            _ => Self::Unknown,
        }
    }
    /// Private and internal repositories are both non-public.
    pub fn is_public(self) -> bool {
        self == Self::Public
    }
}

/// Current repository metadata. Identified by `id`; the name may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub owner_kind: ActorKind,
    pub visibility: Visibility,
    pub is_fork: bool,
    /// Viewer permission (`ADMIN`, `MAINTAIN`, `WRITE`, ...), when known.
    pub viewer_permission: Option<String>,
    /// False when the repository could not be accessed on the last sync.
    pub accessible: bool,
    pub url: Option<String>,
}

impl Repository {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Owned when the owner is the subject, or the subject administers it.
    /// Organization membership alone does not make a repository owned.
    pub fn is_owned_by(&self, subject: &Subject, subject_is_viewer: bool) -> bool {
        self.owner.eq_ignore_ascii_case(&subject.login)
            || (subject_is_viewer && self.viewer_permission.as_deref() == Some("ADMIN"))
    }
}

/// Current state of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl PrState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "merged" => Some(Self::Merged),
            _ => None,
        }
    }
}

/// Current pull request snapshot (state as of the last sync).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: String,
    pub repository_id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Actor,
    pub state: PrState,
    pub is_draft: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    /// `None` when GitHub did not report who merged.
    pub merged_by: Option<Actor>,
    pub head_sha: Option<String>,
    /// Latest combined CI state of the head commit as of the last sync.
    pub latest_ci: Option<CheckOutcome>,
    /// Coverage of this PR's internal activity.
    pub coverage: CoverageStatus,
    pub synced_at: Option<DateTime<Utc>>,
}

/// Current review thread state. Resolution history is not reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: String,
    pub pr_id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub resolved_by: Option<Actor>,
    pub path: Option<String>,
    pub comment_count: u32,
    pub first_comment_author: Option<Actor>,
}

/// Current issue snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub repository_id: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: Actor,
    pub is_open: bool,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// A collection of pull request data fetched as a unit. When a collection
/// is fetched completely, stored events of that collection that are no
/// longer present are marked as removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collection {
    /// `PrCommit` events.
    Commits,
    /// `ReviewSubmitted` and `ReviewSummary` events.
    Reviews,
    /// `ConversationComment` events.
    Comments,
    /// `InlineComment` events and review threads.
    ReviewComments,
    /// Lifecycle, coordination, metadata, force-push and dismissal events.
    Timeline,
    /// `CheckRun` and `CommitStatus` events.
    Checks,
}

impl Collection {
    pub const ALL: [Collection; 6] = [
        Collection::Commits,
        Collection::Reviews,
        Collection::Comments,
        Collection::ReviewComments,
        Collection::Timeline,
        Collection::Checks,
    ];

    /// The collection an event type belongs to, if it is a PR event.
    pub fn of(t: EventType) -> Option<Collection> {
        use EventType::*;
        Some(match t {
            PrCommit => Collection::Commits,
            ReviewSubmitted | ReviewSummary => Collection::Reviews,
            ConversationComment => Collection::Comments,
            InlineComment => Collection::ReviewComments,
            CheckRun | CommitStatus => Collection::Checks,
            PrOpened | PrConvertedToDraft | PrReadyForReview | PrClosed | PrReopened | PrMerged
            | PrHeadForcePushed | ReviewDismissed | ReviewRequested | ReviewRequestRemoved
            | Assigned | Unassigned | Labeled | Unlabeled | Milestoned | Demilestoned | Renamed => {
                Collection::Timeline
            }
            CommitAuthored | IssueOpened | IssueClosed | IssueReopened => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::Reviews => "reviews",
            Self::Comments => "comments",
            Self::ReviewComments => "review_comments",
            Self::Timeline => "timeline",
            Self::Checks => "checks",
        }
    }
}
