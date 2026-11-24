//! Normalized activity model.
//!
//! Every piece of GitHub activity is represented as an [`ActivityEvent`] with a
//! stable [`EventId`]. Current entity state (a pull request's present state,
//! a review thread's resolution, the latest CI result) lives in separate
//! structs and is never used as a substitute for historical events.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::coverage::CoverageStatus;

/// Stable unique identifier of an event. Identical events fetched from
/// different APIs map to the same id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub String);

impl EventId {
    /// Builds the stable identifier for a commit in a repository.
    #[must_use]
    pub fn commit(repository_id: &str, sha: &str) -> Self {
        Self(format!(
            "commit:{repository_id}:{}",
            sha.to_ascii_lowercase()
        ))
    }
    /// Builds the stable identifier for a pull request creation event.
    #[must_use]
    pub fn pr_created(pr_id: &str) -> Self {
        Self(format!("pr:{pr_id}:created"))
    }
    /// Builds the stable identifier for a pull request merge event.
    #[must_use]
    pub fn pr_merged(pr_id: &str) -> Self {
        Self(format!("pr:{pr_id}:merged"))
    }
    /// Builds the stable identifier for a commit associated with a pull request.
    #[must_use]
    pub fn pr_commit(pr_id: &str, sha: &str) -> Self {
        Self(format!("pr-commit:{pr_id}:{}", sha.to_ascii_lowercase()))
    }
    /// Builds the stable identifier for a submitted review.
    #[must_use]
    pub fn review(review_id: &str) -> Self {
        Self(format!("review:{review_id}"))
    }
    /// Builds the stable identifier for a review summary comment.
    #[must_use]
    pub fn review_summary(review_id: &str) -> Self {
        Self(format!("review-summary:{review_id}"))
    }
    /// Builds the stable identifier for a comment.
    #[must_use]
    pub fn comment(comment_id: &str) -> Self {
        Self(format!("comment:{comment_id}"))
    }
    /// Builds the stable identifier for a timeline event.
    #[must_use]
    pub fn timeline(node_id: &str) -> Self {
        Self(format!("tl:{node_id}"))
    }
    /// Builds the stable identifier for an issue creation event.
    #[must_use]
    pub fn issue_created(issue_id: &str) -> Self {
        Self(format!("issue:{issue_id}:created"))
    }
    /// Builds the stable identifier for a check run.
    #[must_use]
    pub fn check_run(node_id: &str) -> Self {
        Self(format!("check:{node_id}"))
    }
    /// Builds the stable identifier for a commit status.
    #[must_use]
    pub fn commit_status(node_id: &str) -> Self {
        Self(format!("status:{node_id}"))
    }
    /// Returns the identifier's underlying string.
    #[must_use]
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
    /// A human GitHub account.
    User,
    /// An automated GitHub account.
    Bot,
    /// A GitHub organization account.
    Organization,
    /// A placeholder account representing a deleted or unavailable user.
    Mannequin,
    /// The actor could not be determined (deleted account, missing field).
    Unknown,
}

/// The account that performed an event. `id`/`login` are `None` when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Actor {
    /// GitHub node ID, when available.
    pub id: Option<String>,
    /// GitHub login, when available.
    pub login: Option<String>,
    /// Classification of the account.
    pub kind: ActorKind,
}

impl Actor {
    /// Creates an actor with no known GitHub identity.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            id: None,
            login: None,
            kind: ActorKind::Unknown,
        }
    }

    /// Builds an actor, classifying bots by type name or the `[bot]` suffix.
    /// Whether this actor is classified as a bot.
    #[must_use]
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
            _ => ActorKind::User,
        };
        Self { id, login, kind }
    }

    /// Whether this actor is classified as a bot.
    #[must_use]
    pub fn is_bot(&self) -> bool {
        self.kind == ActorKind::Bot
    }

    /// Whether this actor has no known identity.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        self.kind == ActorKind::Unknown
    }

    /// Returns the login, or `unknown` when it is unavailable.
    #[must_use]
    pub fn display(&self) -> &str {
        self.login.as_deref().unwrap_or("unknown")
    }

    /// Whether this actor is the given subject. Unknown actors never match.
    /// When both ids are known they decide; the login is only compared when
    /// an id is missing, so a renamed or reused login cannot match.
    #[must_use]
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
    /// GitHub node ID, when available.
    pub id: Option<String>,
    /// GitHub login used to identify the subject when no ID is available.
    pub login: String,
}

/// Event categories from the activity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    /// Commits authored outside pull request event history.
    Commit,
    /// Pull request creation, closure, reopening, and merge events.
    PrLifecycle,
    /// Commits and force pushes within pull requests.
    PrDevelopment,
    /// Review submission and dismissal events.
    PrReview,
    /// Comments and review summaries.
    PrDiscussion,
    /// Reviewer requests and assignment changes.
    PrCoordination,
    /// Labels, milestones, and title changes.
    PrMetadata,
    /// Issue lifecycle events.
    Issue,
    /// CI checks and commit statuses.
    Automation,
}

/// Every normalized event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // Commit
    /// A commit authored by an actor.
    CommitAuthored,
    // PR lifecycle
    /// A pull request was opened.
    PrOpened,
    /// A pull request was converted to draft.
    PrConvertedToDraft,
    /// A draft pull request was marked ready for review.
    PrReadyForReview,
    /// A pull request was closed without merging.
    PrClosed,
    /// A closed pull request was reopened.
    PrReopened,
    /// A pull request was merged.
    PrMerged,
    // PR development
    /// A commit was added to a pull request.
    PrCommit,
    /// The pull request head was force pushed.
    PrHeadForcePushed,
    // PR review
    /// A review was submitted.
    ReviewSubmitted,
    /// A review was dismissed.
    ReviewDismissed,
    // PR discussion
    /// A pull request conversation comment was added.
    ConversationComment,
    /// A comment was added to a diff line.
    InlineComment,
    /// A review body or summary comment was added.
    ReviewSummary,
    // PR coordination
    /// A reviewer was requested.
    ReviewRequested,
    /// A reviewer request was removed.
    ReviewRequestRemoved,
    /// An assignee was added.
    Assigned,
    /// An assignee was removed.
    Unassigned,
    // PR metadata
    /// A label was added.
    Labeled,
    /// A label was removed.
    Unlabeled,
    /// A milestone was assigned.
    Milestoned,
    /// A milestone was removed.
    Demilestoned,
    /// A pull request title was changed.
    Renamed,
    // Issue
    /// An issue was opened.
    IssueOpened,
    /// An issue was closed.
    IssueClosed,
    /// A closed issue was reopened.
    IssueReopened,
    // Automation
    /// A check run completed or changed state.
    CheckRun,
    /// A commit status was reported.
    CommitStatus,
}

impl EventType {
    /// All event types in stable declaration order.
    pub const ALL: [Self; 28] = [
        Self::CommitAuthored,
        Self::PrOpened,
        Self::PrConvertedToDraft,
        Self::PrReadyForReview,
        Self::PrClosed,
        Self::PrReopened,
        Self::PrMerged,
        Self::PrCommit,
        Self::PrHeadForcePushed,
        Self::ReviewSubmitted,
        Self::ReviewDismissed,
        Self::ConversationComment,
        Self::InlineComment,
        Self::ReviewSummary,
        Self::ReviewRequested,
        Self::ReviewRequestRemoved,
        Self::Assigned,
        Self::Unassigned,
        Self::Labeled,
        Self::Unlabeled,
        Self::Milestoned,
        Self::Demilestoned,
        Self::Renamed,
        Self::IssueOpened,
        Self::IssueClosed,
        Self::IssueReopened,
        Self::CheckRun,
        Self::CommitStatus,
    ];

    /// Returns the broad category for this event.
    #[must_use]
    pub const fn category(self) -> EventCategory {
        use EventType::{
            Assigned, CheckRun, CommitAuthored, CommitStatus, ConversationComment, Demilestoned,
            InlineComment, IssueClosed, IssueOpened, IssueReopened, Labeled, Milestoned, PrClosed,
            PrCommit, PrConvertedToDraft, PrHeadForcePushed, PrMerged, PrOpened, PrReadyForReview,
            PrReopened, Renamed, ReviewDismissed, ReviewRequestRemoved, ReviewRequested,
            ReviewSubmitted, ReviewSummary, Unassigned, Unlabeled,
        };
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
    /// Comments that make up a pull request's discussion.
    /// Whether the event represents automation rather than a person.
    #[must_use]
    pub const fn is_discussion(self) -> bool {
        matches!(
            self,
            Self::ConversationComment | Self::InlineComment | Self::ReviewSummary
        )
    }

    /// Whether this event represents automation rather than a person.
    #[must_use]
    pub fn is_automation(self) -> bool {
        self.category() == EventCategory::Automation
    }

    /// Events that belong to a pull request.
    #[must_use]
    pub const fn is_pr_event(self) -> bool {
        !matches!(
            self.category(),
            EventCategory::Commit | EventCategory::Issue
        )
    }

    /// Ordering used to break ties between events at the same timestamp,
    /// following the natural order of a pull request's life.
    /// Parses a serialized event type name.
    #[must_use]
    pub const fn sort_rank(self) -> u8 {
        use EventType::{
            Assigned, CheckRun, CommitAuthored, CommitStatus, ConversationComment, Demilestoned,
            InlineComment, IssueClosed, IssueOpened, IssueReopened, Labeled, Milestoned, PrClosed,
            PrCommit, PrConvertedToDraft, PrHeadForcePushed, PrMerged, PrOpened, PrReadyForReview,
            PrReopened, Renamed, ReviewDismissed, ReviewRequestRemoved, ReviewRequested,
            ReviewSubmitted, ReviewSummary, Unassigned, Unlabeled,
        };
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

    /// Returns the stable serialized event type name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        use EventType::{
            Assigned, CheckRun, CommitAuthored, CommitStatus, ConversationComment, Demilestoned,
            InlineComment, IssueClosed, IssueOpened, IssueReopened, Labeled, Milestoned, PrClosed,
            PrCommit, PrConvertedToDraft, PrHeadForcePushed, PrMerged, PrOpened, PrReadyForReview,
            PrReopened, Renamed, ReviewDismissed, ReviewRequestRemoved, ReviewRequested,
            ReviewSubmitted, ReviewSummary, Unassigned, Unlabeled,
        };
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

    /// Returns the display label used in report output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        use EventType::{
            Assigned, CheckRun, CommitAuthored, CommitStatus, ConversationComment, Demilestoned,
            InlineComment, IssueClosed, IssueOpened, IssueReopened, Labeled, Milestoned, PrClosed,
            PrCommit, PrConvertedToDraft, PrHeadForcePushed, PrMerged, PrOpened, PrReadyForReview,
            PrReopened, Renamed, ReviewDismissed, ReviewRequestRemoved, ReviewRequested,
            ReviewSubmitted, ReviewSummary, Unassigned, Unlabeled,
        };
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

    /// Parses a serialized event type name, including the legacy `commit` alias.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        // "commit" is accepted for caches written by earlier versions.
        if s == "commit" {
            return Some(Self::CommitAuthored);
        }
        Self::ALL.iter().copied().find(|t| t.as_str() == s)
    }
}

/// Where an event was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Event fetched from a GitHub REST endpoint.
    Rest,
    /// Event fetched from the GitHub GraphQL API.
    Graphql,
    /// Event found through GitHub search.
    Search,
}

impl Source {
    /// Returns the stable serialized source name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::Graphql => "graphql",
            Self::Search => "search",
        }
    }
    /// Parses a serialized source name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rest" => Some(Self::Rest),
            "graphql" => Some(Self::Graphql),
            "search" => Some(Self::Search),
            _ => None,
        }
    }
}

/// Submitted review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    /// The review approved the pull request.
    Approved,
    /// The review requested changes.
    ChangesRequested,
    /// The review left comments without approval or requested changes.
    Commented,
    /// Review was dismissed and its original state could not be observed.
    Dismissed,
    /// Draft review that has not been submitted. Never counted.
    Pending,
}

impl ReviewState {
    /// Parses a GitHub review state.
    #[must_use]
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
    /// Returns the GitHub serialized review state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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
    /// The check completed successfully.
    Success,
    /// The check failed.
    Failure,
    /// The check is queued or running.
    Pending,
    /// The check completed without a success or failure result.
    Neutral,
    /// The check was skipped or marked stale.
    Skipped,
    /// The check was cancelled.
    Cancelled,
    /// The check result is not recognized.
    Unknown,
}

impl CheckOutcome {
    /// Maps GitHub check-run status/conclusion or commit-status state.
    #[must_use]
    pub fn from_github(status: Option<&str>, conclusion: Option<&str>) -> Self {
        conclusion.map_or_else(
            || match status.map(str::to_ascii_lowercase).as_deref() {
                Some("queued" | "in_progress" | "waiting" | "requested" | "pending") => {
                    Self::Pending
                }
                Some("completed" | _) | None => Self::Unknown,
            },
            |c| match c.to_ascii_lowercase().as_str() {
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
        )
    }
    /// Returns the stable serialized outcome name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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
    /// Parses a serialized outcome name, using `Unknown` for unrecognized values.
    #[must_use]
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
    /// Commit SHA associated with the event, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Commit author date (the event's `occurred_at` is the committer date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_date: Option<Timestamp>,
    /// Whether the commit has multiple parents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_merge_commit: Option<bool>,
    /// Committer identity, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committer: Option<Actor>,
    /// For PR commits: `occurred_at` is the commit time, not when it was added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_basis: Option<String>,
    /// Review state for review submission events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_state: Option<ReviewState>,
    /// GitHub node ID of the review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    /// GitHub node ID of the review thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// For inline comments: the comment this replies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Name of a CI check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_name: Option<String>,
    /// Result of a CI check or commit status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_outcome: Option<CheckOutcome>,
    /// GitHub node ID of the check suite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_suite_id: Option<String>,
    /// For a PR closed event: whether the close happened as part of a merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via_merge: Option<bool>,
    /// Target of coordination events (requested reviewer, assignee, label...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Pull request title before a rename event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_title: Option<String>,
    /// Time the source object was last edited, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<Timestamp>,
    /// Set on a `ReviewSubmitted` event when the review only carries replies
    /// to existing review threads (GitHub records every thread reply as a
    /// review with no body). Such reviews are not counted as submissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_reply_only: Option<bool>,
    /// Set when a resync observed that the source object no longer exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_observed_at: Option<Timestamp>,
    /// First line of a commit's message (commits only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_headline: Option<String>,
}

/// A normalized activity event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEvent {
    /// Stable identifier used to deduplicate this event.
    pub event_id: EventId,
    /// Normalized kind of activity.
    pub event_type: EventType,
    /// Account that performed the activity.
    pub actor: Actor,
    /// GitHub node ID of the repository.
    pub repository_id: String,
    /// Id of the source entity (commit sha, review id, comment id, ...).
    pub entity_id: String,
    /// Pull request containing the event, when applicable.
    pub parent_pr_id: Option<String>,
    /// Time attributed to the event.
    pub occurred_at: Timestamp,
    /// Time this copy of the event was fetched.
    pub observed_at: Timestamp,
    /// API used to observe the event.
    pub source: Source,
    /// GitHub URL for the source object, when available.
    pub url: Option<String>,
    /// Event-specific optional metadata.
    pub meta: EventMeta,
    /// Completeness of the data collection containing this event.
    pub coverage: CoverageStatus,
}

impl ActivityEvent {
    /// Whether the event is excluded from counts because the source object
    /// was observed to be removed.
    #[must_use]
    pub const fn is_removed(&self) -> bool {
        self.meta.removed_observed_at.is_some()
    }
}

/// Repository visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Publicly accessible repository.
    Public,
    /// Private repository.
    Private,
    /// Organization-internal repository.
    Internal,
    /// Visibility was not reported.
    Unknown,
}

impl Visibility {
    /// Returns the stable serialized visibility name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
            Self::Unknown => "unknown",
        }
    }
    /// Parses a visibility name, returning `Unknown` for other values.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "public" => Self::Public,
            "private" => Self::Private,
            "internal" => Self::Internal,
            _ => Self::Unknown,
        }
    }
    /// Private and internal repositories are both non-public.
    #[must_use]
    pub fn is_public(self) -> bool {
        self == Self::Public
    }
}

/// GitHub API reference to a repository. Identity ignores ASCII case while
/// retaining the original spelling for display and requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryRef {
    /// Repository owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
}

impl RepositoryRef {
    /// Creates a repository reference while preserving the supplied casing.
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

impl PartialEq for RepositoryRef {
    fn eq(&self, other: &Self) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}
impl Eq for RepositoryRef {}
impl std::hash::Hash for RepositoryRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.owner.to_ascii_lowercase().hash(state);
        self.name.to_ascii_lowercase().hash(state);
    }
}

#[cfg(test)]
mod repository_ref_tests {
    use super::RepositoryRef;
    use std::collections::HashSet;

    #[test]
    fn repository_ref_identity_is_case_insensitive_and_keeps_display_case() {
        let original = RepositoryRef::new("Tanahiro2010", "GhusAGE");
        let equivalent = RepositoryRef::new("tanahiro2010", "ghusage");
        assert_eq!(original, equivalent);
        assert_eq!(original.owner, "Tanahiro2010");
        assert_eq!(original.name, "GhusAGE");
        assert_eq!(HashSet::from([original, equivalent]).len(), 1);
    }
}

/// Current repository metadata. Identified by `id`; the name may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// GitHub node ID of the repository.
    pub id: String,
    /// Repository owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Classification of the owner account.
    pub owner_kind: ActorKind,
    /// Current repository visibility.
    pub visibility: Visibility,
    /// Whether the repository is a fork.
    pub is_fork: bool,
    /// Viewer permission (`ADMIN`, `MAINTAIN`, `WRITE`, ...), when known.
    pub viewer_permission: Option<String>,
    /// False when the repository could not be accessed on the last sync.
    pub accessible: bool,
    /// Repository URL, when available.
    pub url: Option<String>,
}

impl Repository {
    /// Returns the owner and repository name joined by `/`.
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// Owned when the owner is the subject, or the subject administers it.
    /// Organization membership alone does not make a repository owned.
    #[must_use]
    pub fn is_owned_by(&self, subject: &Subject, subject_is_viewer: bool) -> bool {
        self.owner.eq_ignore_ascii_case(&subject.login)
            || (subject_is_viewer && self.viewer_permission.as_deref() == Some("ADMIN"))
    }
}

/// Current state of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// The pull request is open.
    Open,
    /// The pull request is closed without merging.
    Closed,
    /// The pull request was merged.
    Merged,
}

impl PrState {
    /// Returns the stable serialized state name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
    /// Parses a serialized pull request state.
    #[must_use]
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
    /// GitHub node ID of the pull request.
    pub id: String,
    /// GitHub node ID of the repository.
    pub repository_id: String,
    /// Pull request number within the repository.
    pub number: u64,
    /// Current pull request title.
    pub title: String,
    /// GitHub URL for the pull request.
    pub url: String,
    /// Account that opened the pull request.
    pub author: Actor,
    /// Current lifecycle state.
    pub state: PrState,
    /// Whether the pull request is currently a draft.
    pub is_draft: bool,
    /// Time the pull request was created.
    pub created_at: Timestamp,
    /// Time the pull request was last updated, when known.
    pub updated_at: Option<Timestamp>,
    /// Time the pull request was closed, when applicable.
    pub closed_at: Option<Timestamp>,
    /// Time the pull request was merged, when applicable.
    pub merged_at: Option<Timestamp>,
    /// `None` when GitHub did not report who merged.
    pub merged_by: Option<Actor>,
    /// Latest commit SHA at the pull request head.
    pub head_sha: Option<String>,
    /// Latest combined CI state of the head commit as of the last sync.
    pub latest_ci: Option<CheckOutcome>,
    /// Coverage of this PR's internal activity.
    /// Coverage of this pull request's data.
    pub coverage: CoverageStatus,
    /// Time this pull request snapshot was last synchronized.
    pub synced_at: Option<Timestamp>,
}

/// Current review thread state. Resolution history is not reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// GitHub node ID of the thread.
    pub id: String,
    /// GitHub node ID of the containing pull request.
    pub pr_id: String,
    /// Whether the thread is currently resolved.
    pub is_resolved: bool,
    /// Whether the thread is outdated relative to the current diff.
    pub is_outdated: bool,
    /// Account that resolved the thread, when known.
    pub resolved_by: Option<Actor>,
    /// Repository path containing the thread, when known.
    pub path: Option<String>,
    /// Number of comments currently in the thread.
    pub comment_count: u32,
    /// Author of the first comment, when known.
    pub first_comment_author: Option<Actor>,
}

/// Current issue snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// GitHub node ID of the issue.
    pub id: String,
    /// GitHub node ID of the repository.
    pub repository_id: String,
    /// Issue number within the repository.
    pub number: u64,
    /// Current issue title.
    pub title: String,
    /// GitHub URL for the issue.
    pub url: String,
    /// Account that opened the issue.
    pub author: Actor,
    /// Whether the issue is currently open.
    pub is_open: bool,
    /// Time the issue was created.
    pub created_at: Timestamp,
    /// Time the issue was closed, when applicable.
    pub closed_at: Option<Timestamp>,
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
    /// All tracked pull request collections in stable declaration order.
    pub const ALL: [Self; 6] = [
        Self::Commits,
        Self::Reviews,
        Self::Comments,
        Self::ReviewComments,
        Self::Timeline,
        Self::Checks,
    ];

    /// The collection an event type belongs to, if it is a PR event.
    #[must_use]
    pub const fn of(t: EventType) -> Option<Self> {
        use EventType::{
            Assigned, CheckRun, CommitAuthored, CommitStatus, ConversationComment, Demilestoned,
            InlineComment, IssueClosed, IssueOpened, IssueReopened, Labeled, Milestoned, PrClosed,
            PrCommit, PrConvertedToDraft, PrHeadForcePushed, PrMerged, PrOpened, PrReadyForReview,
            PrReopened, Renamed, ReviewDismissed, ReviewRequestRemoved, ReviewRequested,
            ReviewSubmitted, ReviewSummary, Unassigned, Unlabeled,
        };
        Some(match t {
            PrCommit => Self::Commits,
            ReviewSubmitted | ReviewSummary => Self::Reviews,
            ConversationComment => Self::Comments,
            InlineComment => Self::ReviewComments,
            CheckRun | CommitStatus => Self::Checks,
            PrOpened | PrConvertedToDraft | PrReadyForReview | PrClosed | PrReopened | PrMerged
            | PrHeadForcePushed | ReviewDismissed | ReviewRequested | ReviewRequestRemoved
            | Assigned | Unassigned | Labeled | Unlabeled | Milestoned | Demilestoned | Renamed => {
                Self::Timeline
            }
            CommitAuthored | IssueOpened | IssueClosed | IssueReopened => return None,
        })
    }

    /// Returns the stable serialized collection name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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
