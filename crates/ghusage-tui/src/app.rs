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

/// A screen of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Dashboard,
    Overview,
    Daily,
    DayDetail,
    Repositories,
    RepoDetail,
    PullRequests,
    PrDetail,
    PrTimeline,
    Reviews,
    Issues,
    IssueDetail,
    CommitDetail,
    System,
}

impl Screen {
    /// Screens reachable with the number keys, in key order.
    pub const TOP: [Screen; 8] = [
        Screen::Dashboard,
        Screen::Overview,
        Screen::Daily,
        Screen::Repositories,
        Screen::PullRequests,
        Screen::Reviews,
        Screen::Issues,
        Screen::System,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Overview => "Overview",
            Screen::Daily => "Daily",
            Screen::DayDetail => "Day Detail",
            Screen::Repositories => "Repositories",
            Screen::RepoDetail => "Repository Detail",
            Screen::PullRequests => "Pull Requests",
            Screen::PrDetail => "PR Detail",
            Screen::PrTimeline => "PR Timeline",
            Screen::Reviews => "Reviews",
            Screen::Issues => "Issues",
            Screen::IssueDetail => "Issue Detail",
            Screen::CommitDetail => "Commit",
            Screen::System => "System",
        }
    }

    /// A shorter title for narrow terminals.
    pub fn short_title(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dash",
            Screen::Overview => "Over",
            Screen::Daily => "Daily",
            Screen::DayDetail => "Day",
            Screen::Repositories => "Repos",
            Screen::RepoDetail => "Repo",
            Screen::PullRequests => "PRs",
            Screen::PrDetail => "PR",
            Screen::PrTimeline => "Timeline",
            Screen::Reviews => "Revs",
            Screen::Issues => "Issues",
            Screen::IssueDetail => "Issue",
            Screen::CommitDetail => "Commit",
            Screen::System => "Sys",
        }
    }

    /// The top-level screen a screen belongs to when it was not opened
    /// from another screen.
    pub fn top(self) -> Screen {
        match self {
            Screen::PrDetail | Screen::PrTimeline | Screen::CommitDetail => Screen::PullRequests,
            Screen::DayDetail => Screen::Daily,
            Screen::RepoDetail => Screen::Repositories,
            Screen::IssueDetail => Screen::Issues,
            s => s,
        }
    }

    /// Whether the screen is opened from another one (Esc goes back).
    pub fn is_detail(self) -> bool {
        self.top() != self
    }
}

/// Time range presets cycled with `t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeRange {
    /// The range given on the command line.
    Initial,
    Today,
    Last7Days,
    Last30Days,
    Last90Days,
    ThisMonth,
}

impl TimeRange {
    pub const CYCLE: [TimeRange; 6] = [
        TimeRange::Initial,
        TimeRange::Today,
        TimeRange::Last7Days,
        TimeRange::Last30Days,
        TimeRange::Last90Days,
        TimeRange::ThisMonth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::Initial => "as started",
            TimeRange::Today => "today",
            TimeRange::Last7Days => "last 7 days",
            TimeRange::Last30Days => "last 30 days",
            TimeRange::Last90Days => "last 90 days",
            TimeRange::ThisMonth => "this month",
        }
    }

    fn since(self) -> Option<&'static str> {
        match self {
            TimeRange::Initial => None,
            TimeRange::Today => Some("today"),
            TimeRange::Last7Days => Some("7d"),
            TimeRange::Last30Days => Some("30d"),
            TimeRange::Last90Days => Some("90d"),
            TimeRange::ThisMonth => Some("1m"),
        }
    }

    pub fn next(self) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[(i + 1) % Self::CYCLE.len()]
    }
}

/// Repository filter presets cycled with `f`, applied on top of the filter
/// given on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterPreset {
    All,
    Public,
    Private,
    Owned,
    External,
}

impl FilterPreset {
    pub const CYCLE: [FilterPreset; 5] = [
        FilterPreset::All,
        FilterPreset::Public,
        FilterPreset::Private,
        FilterPreset::Owned,
        FilterPreset::External,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterPreset::All => "all",
            FilterPreset::Public => "public",
            FilterPreset::Private => "private",
            FilterPreset::Owned => "owned",
            FilterPreset::External => "external",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0);
        Self::CYCLE[(i + 1) % Self::CYCLE.len()]
    }

    /// The filter used for reports.
    pub fn apply(self, base: &Filter) -> Filter {
        let mut f = base.clone();
        match self {
            FilterPreset::All => {}
            FilterPreset::Public => f.visibility = Some(VisibilityFilter::Public),
            FilterPreset::Private => f.visibility = Some(VisibilityFilter::Private),
            FilterPreset::Owned => f.ownership = Some(Ownership::Owned),
            FilterPreset::External => f.ownership = Some(Ownership::External),
        }
        f
    }
}

/// Event-type filter of the PR timeline, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimelineFilter {
    All,
    Commits,
    Reviews,
    Comments,
    Lifecycle,
    Coordination,
    Metadata,
    Checks,
}

impl TimelineFilter {
    pub const CYCLE: [TimelineFilter; 8] = [
        TimelineFilter::All,
        TimelineFilter::Commits,
        TimelineFilter::Reviews,
        TimelineFilter::Comments,
        TimelineFilter::Lifecycle,
        TimelineFilter::Coordination,
        TimelineFilter::Metadata,
        TimelineFilter::Checks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimelineFilter::All => "All",
            TimelineFilter::Commits => "Commits",
            TimelineFilter::Reviews => "Reviews",
            TimelineFilter::Comments => "Comments",
            TimelineFilter::Lifecycle => "Lifecycle",
            TimelineFilter::Coordination => "Coordination",
            TimelineFilter::Metadata => "Metadata",
            TimelineFilter::Checks => "CI",
        }
    }

    pub fn matches(self, t: EventType) -> bool {
        let c = t.category();
        match self {
            TimelineFilter::All => true,
            TimelineFilter::Commits => c == EventCategory::PrDevelopment,
            TimelineFilter::Reviews => c == EventCategory::PrReview,
            TimelineFilter::Comments => c == EventCategory::PrDiscussion,
            TimelineFilter::Lifecycle => c == EventCategory::PrLifecycle,
            TimelineFilter::Coordination => c == EventCategory::PrCoordination,
            TimelineFilter::Metadata => c == EventCategory::PrMetadata,
            TimelineFilter::Checks => c == EventCategory::Automation,
        }
    }

    fn step(self, delta: isize) -> Self {
        let n = Self::CYCLE.len() as isize;
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0) as isize;
        Self::CYCLE[((i + delta).rem_euclid(n)) as usize]
    }
}

/// Focus on the Overview screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewPane {
    Summary,
    Repositories,
}

/// Focus on the Dashboard screen, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardPane {
    Summary,
    Repositories,
    PullRequests,
}

impl DashboardPane {
    const CYCLE: [DashboardPane; 3] = [
        DashboardPane::Summary,
        DashboardPane::Repositories,
        DashboardPane::PullRequests,
    ];

    fn step(self, delta: isize) -> Self {
        let n = Self::CYCLE.len() as isize;
        let i = Self::CYCLE.iter().position(|t| *t == self).unwrap_or(0) as isize;
        Self::CYCLE[((i + delta).rem_euclid(n)) as usize]
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
    Loading,
    Ready(Arc<CommitView>),
    Failed(CommitFailure),
}

/// Section of a day or repository detail, cycled with Tab / Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailTab {
    Activity,
    Commits,
    PullRequests,
    Reviews,
    Comments,
    Issues,
}

impl DetailTab {
    /// Section order of the day detail.
    pub const DAY: [DetailTab; 6] = [
        DetailTab::Activity,
        DetailTab::Commits,
        DetailTab::PullRequests,
        DetailTab::Reviews,
        DetailTab::Comments,
        DetailTab::Issues,
    ];
    /// Section order of the repository detail.
    pub const REPO: [DetailTab; 6] = [
        DetailTab::PullRequests,
        DetailTab::Commits,
        DetailTab::Issues,
        DetailTab::Reviews,
        DetailTab::Comments,
        DetailTab::Activity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DetailTab::Activity => "Activity",
            DetailTab::Commits => "Commits",
            DetailTab::PullRequests => "Pull requests",
            DetailTab::Reviews => "Reviews",
            DetailTab::Comments => "Comments",
            DetailTab::Issues => "Issues",
        }
    }

    /// A shorter label for narrow terminals.
    pub fn short_label(self) -> &'static str {
        match self {
            DetailTab::Activity => "Act",
            DetailTab::Commits => "Commits",
            DetailTab::PullRequests => "PRs",
            DetailTab::Reviews => "Revs",
            DetailTab::Comments => "Cmts",
            DetailTab::Issues => "Issues",
        }
    }

    pub fn list(self) -> ListId {
        match self {
            DetailTab::Activity => ListId::DetailActivity,
            DetailTab::Commits => ListId::DetailCommits,
            DetailTab::PullRequests => ListId::DetailPullRequests,
            DetailTab::Reviews => ListId::DetailReviews,
            DetailTab::Comments => ListId::DetailComments,
            DetailTab::Issues => ListId::DetailIssues,
        }
    }
}

/// The day shown on the day detail screen.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DayScope {
    pub start: Timestamp,
    pub end: Timestamp,
    pub label: String,
}

/// The repository shown on the repository detail screen.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoScope {
    pub id: String,
    pub full_name: String,
}

/// A scrollable list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
    DashboardRepos,
    DashboardPrs,
    OverviewRepos,
    Daily,
    Repositories,
    PullRequests,
    ReviewsSubmitted,
    ReviewsReceived,
    Issues,
    IssueEvents,
    Timeline,
    DetailActivity,
    DetailCommits,
    DetailPullRequests,
    DetailReviews,
    DetailComments,
    DetailIssues,
    PrCommits,
}

impl ListId {
    const DETAIL: [ListId; 6] = [
        ListId::DetailActivity,
        ListId::DetailCommits,
        ListId::DetailPullRequests,
        ListId::DetailReviews,
        ListId::DetailComments,
        ListId::DetailIssues,
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
    Quit,
    OpenUrl(String),
    StartSync(Period),
    /// Load a commit (cache first, then GitHub when online) and report
    /// back through [`App::on_commit`].
    LoadCommit(CommitKey),
}

/// Background sync status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    pub running: bool,
    pub progress: Option<String>,
    pub auth: Option<String>,
    pub rate_limit: Option<String>,
    pub last_summary: Option<String>,
    pub last_complete: Option<bool>,
    pub warnings: Vec<String>,
    pub last_error: Option<String>,
}

/// Static configuration of the UI.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub period: Period,
    pub zone: Zone,
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
pub struct App {
    compute: Compute,
    clock: Clock,
    initial_period: Period,
    base_filter: Filter,
    pub zone: Zone,
    pub background_sync: bool,

    pub screen: Screen,
    pub period: Period,
    pub time_range: TimeRange,
    pub filter_preset: FilterPreset,
    pub review_direction: ReviewDirection,
    pub overview_pane: OverviewPane,
    pub dashboard_pane: DashboardPane,
    pub timeline_filter: TimelineFilter,
    pub detail_tab: DetailTab,
    /// Which commit date counts (same meaning as the CLI's `--commit-date`).
    pub commit_basis: CommitDateBasis,
    pub detail_pr: Option<String>,
    pub detail_issue: Option<String>,
    pub detail_day: Option<DayScope>,
    pub detail_repo: Option<RepoScope>,
    pub pr_pane: PrPane,
    /// The commit shown on the commit screen and what was known about it
    /// before it was loaded.
    pub detail_commit: Option<CommitKey>,
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
    pub search_mode: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub message: Option<String>,
    pub sync: SyncState,
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
