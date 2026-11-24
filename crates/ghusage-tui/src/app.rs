//! UI state and key handling. Numbers come from the shared report builders
//! (`ghusage_app::report`); this module only decides which report to show,
//! caches it, and tracks navigation, selection and search.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use ghusage_app::report::{IssueReport, PrReport, QueryContext, Report, ReportKind};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    CommitDateBasis, Filter, Ownership, Participation, PrSort, ReviewDirection, ReviewRow,
    VisibilityFilter,
};
use ghusage_core::model::{EventCategory, EventType};
use ghusage_core::time::{Period, Zone, resolve_period};
use jiff::Timestamp;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::background::SyncMsg;
use crate::commit_view::{CommitFailure, CommitKey, CommitMsg, CommitSeed, CommitView};
use crate::compute::{self, Build, Job, Outcome, Request, Response, Worker};
use crate::source::{DataSource, SystemInfo};

fn stepped_index(len: usize, current: usize, delta: isize) -> usize {
    let len = isize::try_from(len).expect("cycle length fits in isize");
    let current = isize::try_from(current).expect("cycle index fits in isize");
    let offset = delta.rem_euclid(len);
    usize::try_from((current + offset).rem_euclid(len)).expect("cycle index is nonnegative")
}

/// A screen of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// Summary and recent activity landing page.
    Dashboard,
    /// Overview metrics.
    Overview,
    /// Daily activity buckets.
    Daily,
    /// Events for one selected day.
    DayDetail,
    /// Per-repository summaries.
    Repositories,
    /// Details for one repository.
    RepoDetail,
    /// Pull request activity list.
    PullRequests,
    /// Details for one pull request.
    PrDetail,
    /// Timeline for one pull request.
    PrTimeline,
    /// Reviews submitted or received.
    Reviews,
    /// Issues with activity.
    Issues,
    /// Details for one issue.
    IssueDetail,
    /// Commit details.
    CommitDetail,
    /// Cache, sync, and system information.
    System,
}

#[cfg(test)]
mod cycle_index_tests {
    use super::stepped_index;

    #[test]
    fn cycle_index_handles_large_signed_steps_without_overflow() {
        assert_eq!(stepped_index(3, 2, 1), 0);
        assert_eq!(stepped_index(3, 0, -1), 2);
        let positive = usize::try_from(isize::MAX.rem_euclid(3)).unwrap();
        assert_eq!(stepped_index(3, 1, isize::MAX), (1 + positive) % 3);
        let negative = usize::try_from(isize::MIN.rem_euclid(3)).unwrap();
        assert_eq!(stepped_index(3, 1, isize::MIN), (1 + negative) % 3);
    }
}

impl Screen {
    /// Screens reachable with the number keys, in key order.
    pub const TOP: [Self; 8] = [
        Self::Dashboard,
        Self::Overview,
        Self::Daily,
        Self::Repositories,
        Self::PullRequests,
        Self::Reviews,
        Self::Issues,
        Self::System,
    ];

    /// Returns the full screen title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Overview => "Overview",
            Self::Daily => "Daily",
            Self::DayDetail => "Day Detail",
            Self::Repositories => "Repositories",
            Self::RepoDetail => "Repository Detail",
            Self::PullRequests => "Pull Requests",
            Self::PrDetail => "PR Detail",
            Self::PrTimeline => "PR Timeline",
            Self::Reviews => "Reviews",
            Self::Issues => "Issues",
            Self::IssueDetail => "Issue Detail",
            Self::CommitDetail => "Commit",
            Self::System => "System",
        }
    }

    /// A shorter title for narrow terminals.
    #[must_use]
    pub const fn short_title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dash",
            Self::Overview => "Over",
            Self::Daily => "Daily",
            Self::DayDetail => "Day",
            Self::Repositories => "Repos",
            Self::RepoDetail => "Repo",
            Self::PullRequests => "PRs",
            Self::PrDetail => "PR",
            Self::PrTimeline => "Timeline",
            Self::Reviews => "Revs",
            Self::Issues => "Issues",
            Self::IssueDetail => "Issue",
            Self::CommitDetail => "Commit",
            Self::System => "Sys",
        }
    }

    /// The top-level screen a screen belongs to when it was not opened
    /// from another screen.
    #[must_use]
    pub const fn top(self) -> Self {
        match self {
            Self::PrDetail | Self::PrTimeline | Self::CommitDetail => Self::PullRequests,
            Self::DayDetail => Self::Daily,
            Self::RepoDetail => Self::Repositories,
            Self::IssueDetail => Self::Issues,
            s => s,
        }
    }

    /// Whether the screen is opened from another one (Esc goes back).
    #[must_use]
    pub fn is_detail(self) -> bool {
        self.top() != self
    }
}

/// Time range presets cycled with `t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeRange {
    /// The range given on the command line.
    Initial,
    /// Current local calendar day.
    Today,
    /// Previous seven days.
    Last7Days,
    /// Previous thirty days.
    Last30Days,
    /// Previous ninety days.
    Last90Days,
    /// Current local calendar month.
    ThisMonth,
}

impl TimeRange {
    /// All presets in cycle order.
    pub const CYCLE: [Self; 6] = [
        Self::Initial,
        Self::Today,
        Self::Last7Days,
        Self::Last30Days,
        Self::Last90Days,
        Self::ThisMonth,
    ];

    /// Returns the display label for this range.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Initial => "as started",
            Self::Today => "today",
            Self::Last7Days => "last 7 days",
            Self::Last30Days => "last 30 days",
            Self::Last90Days => "last 90 days",
            Self::ThisMonth => "this month",
        }
    }

    const fn since(self) -> Option<&'static str> {
        match self {
            Self::Initial => None,
            Self::Today => Some("today"),
            Self::Last7Days => Some("7d"),
            Self::Last30Days => Some("30d"),
            Self::Last90Days => Some("90d"),
            Self::ThisMonth => Some("1m"),
        }
    }

    /// Advances to the next range preset.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[(i + 1) % Self::CYCLE.len()]
    }
}

/// Repository filter presets cycled with `f`, applied on top of the filter
/// given on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterPreset {
    /// Do not add another repository restriction.
    All,
    /// Public repositories only.
    Public,
    /// Private and internal repositories only.
    Private,
    /// Repositories owned by the subject.
    Owned,
    /// Repositories not owned by the subject.
    External,
}

impl FilterPreset {
    /// All filter presets in cycle order.
    pub const CYCLE: [Self; 5] = [
        Self::All,
        Self::Public,
        Self::Private,
        Self::Owned,
        Self::External,
    ];

    /// Returns the display label for this preset.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Public => "public",
            Self::Private => "private",
            Self::Owned => "owned",
            Self::External => "external",
        }
    }

    /// Advances to the next filter preset.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[(i + 1) % Self::CYCLE.len()]
    }

    /// The filter used for reports.
    #[must_use]
    pub fn apply(self, base: &Filter) -> Filter {
        let mut f = base.clone();
        match self {
            Self::All => {}
            Self::Public => f.visibility = Some(VisibilityFilter::Public),
            Self::Private => f.visibility = Some(VisibilityFilter::Private),
            Self::Owned => f.ownership = Some(Ownership::Owned),
            Self::External => f.ownership = Some(Ownership::External),
        }
        f
    }
}

/// Event-type filter of the PR timeline, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimelineFilter {
    /// Include every event type.
    All,
    /// Pull request commits and force pushes.
    Commits,
    /// Review submissions and dismissals.
    Reviews,
    /// Conversation, inline, and summary comments.
    Comments,
    /// Pull request lifecycle changes.
    Lifecycle,
    /// Reviewer requests and assignment changes.
    Coordination,
    /// Labels, milestones, and title changes.
    Metadata,
    /// CI checks and commit statuses.
    Checks,
}

impl TimelineFilter {
    /// All filters in cycle order.
    pub const CYCLE: [Self; 8] = [
        Self::All,
        Self::Commits,
        Self::Reviews,
        Self::Comments,
        Self::Lifecycle,
        Self::Coordination,
        Self::Metadata,
        Self::Checks,
    ];

    /// Returns the display label for this filter.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Commits => "Commits",
            Self::Reviews => "Reviews",
            Self::Comments => "Comments",
            Self::Lifecycle => "Lifecycle",
            Self::Coordination => "Coordination",
            Self::Metadata => "Metadata",
            Self::Checks => "CI",
        }
    }

    /// Whether an event type belongs to this filter.
    #[must_use]
    pub fn matches(self, t: EventType) -> bool {
        let c = t.category();
        match self {
            Self::All => true,
            Self::Commits => c == EventCategory::PrDevelopment,
            Self::Reviews => c == EventCategory::PrReview,
            Self::Comments => c == EventCategory::PrDiscussion,
            Self::Lifecycle => c == EventCategory::PrLifecycle,
            Self::Coordination => c == EventCategory::PrCoordination,
            Self::Metadata => c == EventCategory::PrMetadata,
            Self::Checks => c == EventCategory::Automation,
        }
    }

    fn step(self, delta: isize) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[stepped_index(Self::CYCLE.len(), i, delta)]
    }
}

/// Focus on the Overview screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewPane {
    /// Aggregate activity metrics.
    Summary,
    /// Repository activity rows.
    Repositories,
}

/// Focus on the Dashboard screen, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardPane {
    /// Dashboard summary metrics.
    Summary,
    /// Repository activity list.
    Repositories,
    /// Pull request activity list.
    PullRequests,
}

impl DashboardPane {
    const CYCLE: [Self; 3] = [Self::Summary, Self::Repositories, Self::PullRequests];

    fn step(self, delta: isize) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[stepped_index(Self::CYCLE.len(), i, delta)]
    }
}

/// Focus on the PR Detail screen, switched with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrPane {
    /// The pull request's facts; Enter opens the timeline.
    Summary,
    /// The list of its commits; Enter opens the selected commit.
    Commits,
}

/// Loading state of a commit shown on the commit screen.
#[derive(Debug, Clone)]
pub enum CommitState {
    /// Commit details are being loaded.
    Loading,
    /// Commit details loaded successfully.
    Ready(Arc<CommitView>),
    /// Commit loading failed.
    Failed(CommitFailure),
}

/// Section of a day or repository detail, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailTab {
    /// Activity events.
    Activity,
    /// Commits.
    Commits,
    /// Pull requests.
    PullRequests,
    /// Reviews.
    Reviews,
    /// Comments.
    Comments,
    /// Issues.
    Issues,
}

impl DetailTab {
    /// Section order of the day detail.
    pub const DAY: [Self; 6] = [
        Self::Activity,
        Self::Commits,
        Self::PullRequests,
        Self::Reviews,
        Self::Comments,
        Self::Issues,
    ];
    /// Section order of the repository detail.
    pub const REPO: [Self; 6] = [
        Self::PullRequests,
        Self::Commits,
        Self::Issues,
        Self::Reviews,
        Self::Comments,
        Self::Activity,
    ];

    /// Returns the full tab label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Activity => "Activity",
            Self::Commits => "Commits",
            Self::PullRequests => "Pull requests",
            Self::Reviews => "Reviews",
            Self::Comments => "Comments",
            Self::Issues => "Issues",
        }
    }

    /// A shorter label for narrow terminals.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Activity => "Act",
            Self::Commits => "Commits",
            Self::PullRequests => "PRs",
            Self::Reviews => "Revs",
            Self::Comments => "Cmts",
            Self::Issues => "Issues",
        }
    }

    /// Returns the list associated with this tab.
    #[must_use]
    pub const fn list(self) -> ListId {
        match self {
            Self::Activity => ListId::DetailActivity,
            Self::Commits => ListId::DetailCommits,
            Self::PullRequests => ListId::DetailPullRequests,
            Self::Reviews => ListId::DetailReviews,
            Self::Comments => ListId::DetailComments,
            Self::Issues => ListId::DetailIssues,
        }
    }
}

/// The day shown on the day detail screen.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DayScope {
    /// Inclusive start of the selected day.
    pub start: Timestamp,
    /// Exclusive end of the selected day.
    pub end: Timestamp,
    /// Display label for the selected day.
    pub label: String,
}

/// The repository shown on the repository detail screen.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoScope {
    /// GitHub node ID of the repository.
    pub id: String,
    /// Repository owner and name.
    pub full_name: String,
}

/// A scrollable list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
    /// Repository list on the dashboard.
    DashboardRepos,
    /// Pull request list on the dashboard.
    DashboardPrs,
    /// Repository list on the overview.
    OverviewRepos,
    /// Daily activity list.
    Daily,
    /// Repository list.
    Repositories,
    /// Pull request list.
    PullRequests,
    /// Reviews submitted by the subject.
    ReviewsSubmitted,
    /// Reviews received on the subject's pull requests.
    ReviewsReceived,
    /// Issues list.
    Issues,
    /// Events for the selected issue.
    IssueEvents,
    /// Pull request timeline.
    Timeline,
    /// Activity in a day or repository detail.
    DetailActivity,
    /// Commits in a day or repository detail.
    DetailCommits,
    /// Pull requests in a day or repository detail.
    DetailPullRequests,
    /// Reviews in a day or repository detail.
    DetailReviews,
    /// Comments in a day or repository detail.
    DetailComments,
    /// Issues in a day or repository detail.
    DetailIssues,
    /// Commits belonging to a pull request.
    PrCommits,
}

impl ListId {
    const DETAIL: [Self; 6] = [
        Self::DetailActivity,
        Self::DetailCommits,
        Self::DetailPullRequests,
        Self::DetailReviews,
        Self::DetailComments,
        Self::DetailIssues,
    ];
}

/// Which report a screen is built from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    /// Summary, daily breakdown, repositories and recent pull requests.
    Base,
    PullRequests,
    Reviews,
    Issues,
    /// Every listing for one day of the period.
    Day {
        start: Timestamp,
        end: Timestamp,
    },
    /// Every listing for one repository.
    Repo(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub(crate) slot: Slot,
    pub(crate) start: Timestamp,
    pub(crate) end: Timestamp,
    pub(crate) filter: FilterPreset,
}

/// Identifies an issue detail: the issue and the period it covers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IssueKey {
    pub(crate) id: String,
    pub(crate) start: Timestamp,
    pub(crate) end: Timestamp,
    pub(crate) filter: FilterPreset,
}

/// Deepest chain of detail screens kept for Esc.
const MAX_NAV_DEPTH: usize = 16;

/// Something the event loop must do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Exit the terminal UI.
    Quit,
    /// Open a URL in the user's browser.
    OpenUrl(String),
    /// Start synchronization for the period.
    StartSync(Period),
    /// Load a commit (cache first, then GitHub when online) and report
    /// back through [`App::on_commit`].
    LoadCommit(CommitKey),
}

/// Background sync status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    /// Whether a background synchronization is active.
    pub running: bool,
    /// Current synchronization progress message.
    pub progress: Option<String>,
    /// Safe credential source description.
    pub auth: Option<String>,
    /// Current rate limit information.
    pub rate_limit: Option<String>,
    /// Summary of the most recent synchronization.
    pub last_summary: Option<String>,
    /// Whether the most recent synchronization completed fully.
    pub last_complete: Option<bool>,
    /// Non-fatal sync warnings.
    pub warnings: Vec<String>,
    /// Most recent sync error.
    pub last_error: Option<String>,
}

/// Static configuration of the UI.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Initial report period.
    pub period: Period,
    /// Time zone used for report dates.
    pub zone: Zone,
    /// Base repository filter.
    pub filter: Filter,
    /// Whether `r` also starts a background sync.
    pub background_sync: bool,
}

type Clock = Box<dyn Fn() -> Timestamp>;

/// Where reports are computed.
enum Compute {
    /// On the calling thread, synchronously.
    Inline(Box<Worker>),
    /// On a worker thread; results arrive through [`App::poll_results`].
    Thread {
        tx: Sender<Request>,
        rx: Receiver<Response>,
    },
}

/// The whole UI state.
#[expect(
    clippy::struct_excessive_bools,
    reason = "These booleans track independent UI states that can coexist."
)]
#[expect(
    missing_debug_implementations,
    reason = "App owns worker channels and UI caches; a debug dump would expose unstable internal state."
)]
pub struct App {
    compute: Compute,
    clock: Clock,
    initial_period: Period,
    base_filter: Filter,
    /// Time zone used for report dates and labels.
    pub zone: Zone,
    /// Whether UI actions may start background synchronization.
    pub background_sync: bool,

    /// Currently visible screen.
    pub screen: Screen,
    /// Currently selected report period.
    pub period: Period,
    /// Active time range preset.
    pub time_range: TimeRange,
    /// Active repository filter preset.
    pub filter_preset: FilterPreset,
    /// Whether submitted or received reviews are shown.
    pub review_direction: ReviewDirection,
    /// Focused section on the Overview screen.
    pub overview_pane: OverviewPane,
    /// Focused section on the Dashboard screen.
    pub dashboard_pane: DashboardPane,
    /// Active pull request timeline filter.
    pub timeline_filter: TimelineFilter,
    /// Selected tab on a detail screen.
    pub detail_tab: DetailTab,
    /// Which commit date counts (same meaning as the CLI's `--commit-date`).
    pub commit_basis: CommitDateBasis,
    /// Pull request currently shown on the detail screen.
    pub detail_pr: Option<String>,
    /// Issue currently shown on the detail screen.
    pub detail_issue: Option<String>,
    /// Day currently shown on the detail screen.
    pub detail_day: Option<DayScope>,
    /// Repository currently shown on the detail screen.
    pub detail_repo: Option<RepoScope>,
    /// Focused section on the pull request detail screen.
    pub pr_pane: PrPane,
    /// The commit shown on the commit screen and what was known about it
    /// before it was loaded.
    pub detail_commit: Option<CommitKey>,
    /// Commit data shown before or while commit details load.
    pub commit_seed: CommitSeed,
    /// First row of the commit screen at the top of the view.
    pub commit_scroll: usize,
    /// Rows of the commit screen that fit on screen, as last drawn.
    pub(crate) commit_view_height: Cell<usize>,
    commits: HashMap<CommitKey, CommitState>,
    /// An effect produced while handling a key, returned by `handle_key`.
    outbox: Option<Effect>,
    /// The day or repository detail last opened (whose report
    /// [`App::detail_report`] returns).
    scope: Option<Screen>,
    /// Whether search input is active.
    pub search_mode: bool,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Whether the event loop should exit.
    pub should_quit: bool,
    /// Transient message shown in the UI.
    pub message: Option<String>,
    /// Background synchronization state.
    pub sync: SyncState,
    /// Cache and system information.
    pub system: SystemInfo,

    /// Screens to return to with Esc, oldest first.
    nav: Vec<Screen>,
    selection: HashMap<ListId, usize>,
    search: HashMap<ListId, String>,
    /// Scroll offsets remembered between frames.
    pub(crate) offsets: RefCell<HashMap<ListId, usize>>,

    generation: u64,
    pending: HashSet<Job>,
    reports: HashMap<CacheKey, Report>,
    errors: HashMap<CacheKey, String>,
    pr_reports: HashMap<String, Result<Option<PrReport>, String>>,
    issue_reports: HashMap<IssueKey, Result<Option<IssueReport>, String>>,
    report_builds: usize,
    dataset_loads: usize,
}

include!("app/construction.rs");
include!("app/report_cache.rs");
include!("app/navigation.rs");
include!("app/selection.rs");
include!("app/commit.rs");
include!("app/sync.rs");
