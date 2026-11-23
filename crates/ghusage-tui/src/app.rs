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
    CommitDateBasis, Filter, IssueRow, Ownership, Participation, PrRow, PrSort, ReviewDirection,
    ReviewRow, VisibilityFilter,
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

impl App {
    /// Creates the UI state. Reports are computed on a worker thread that
    /// owns `source`; the calling thread never waits for them.
    pub fn new(source: Box<dyn DataSource + Send>, cfg: AppConfig) -> std::io::Result<Self> {
        Self::with_clock(source, cfg, Box::new(Timestamp::now))
    }

    /// Like [`App::new`] with a custom clock for relative time ranges.
    pub fn with_clock(
        source: Box<dyn DataSource + Send>,
        cfg: AppConfig,
        clock: Clock,
    ) -> std::io::Result<Self> {
        let system = source.system_info();
        let (tx, rx) = compute::spawn(Worker::new(source))?;
        Ok(Self::build(Compute::Thread { tx, rx }, system, cfg, clock))
    }

    /// Creates the UI state with reports computed synchronously on the
    /// calling thread. Useful for scripting and tests.
    pub fn inline(source: Box<dyn DataSource + Send>, cfg: AppConfig, clock: Clock) -> Self {
        let system = source.system_info();
        Self::build(
            Compute::Inline(Box::new(Worker::new(source))),
            system,
            cfg,
            clock,
        )
    }

    fn build(compute: Compute, system: SystemInfo, cfg: AppConfig, clock: Clock) -> Self {
        Self {
            compute,
            clock,
            initial_period: cfg.period,
            base_filter: cfg.filter,
            zone: cfg.zone,
            background_sync: cfg.background_sync,
            screen: Screen::Dashboard,
            period: cfg.period,
            time_range: TimeRange::Initial,
            filter_preset: FilterPreset::All,
            review_direction: ReviewDirection::Submitted,
            overview_pane: OverviewPane::Summary,
            dashboard_pane: DashboardPane::Summary,
            timeline_filter: TimelineFilter::All,
            detail_tab: DetailTab::Activity,
            commit_basis: CommitDateBasis::default(),
            detail_pr: None,
            detail_issue: None,
            detail_day: None,
            detail_repo: None,
            pr_pane: PrPane::Summary,
            detail_commit: None,
            commit_seed: CommitSeed::default(),
            commit_scroll: 0,
            commit_view_height: Cell::new(20),
            commits: HashMap::new(),
            outbox: None,
            scope: None,
            search_mode: false,
            show_help: false,
            should_quit: false,
            message: None,
            sync: SyncState::default(),
            system,
            nav: Vec::new(),
            selection: HashMap::new(),
            search: HashMap::new(),
            offsets: RefCell::new(HashMap::new()),
            generation: 0,
            pending: HashSet::new(),
            reports: HashMap::new(),
            errors: HashMap::new(),
            pr_reports: HashMap::new(),
            issue_reports: HashMap::new(),
            report_builds: 0,
            dataset_loads: 0,
        }
    }

    // ----- data -------------------------------------------------------

    /// Number of reports computed so far (cached reports are not rebuilt).
    pub fn report_builds(&self) -> usize {
        self.report_builds
    }

    /// Number of datasets read from the source so far.
    pub fn dataset_loads(&self) -> usize {
        self.dataset_loads
    }

    pub fn filter(&self) -> Filter {
        self.filter_preset.apply(&self.base_filter)
    }

    pub fn query_context(&self) -> QueryContext {
        QueryContext {
            period: self.period,
            zone: self.zone.clone(),
            filter: self.filter(),
            commit_basis: self.commit_basis,
            redact_private: false,
            synced_at: self.system.synced_at,
        }
    }

    /// The context a report of `slot` is computed with.
    fn context_for(&self, slot: &Slot) -> QueryContext {
        let mut ctx = self.query_context();
        match slot {
            Slot::Day { start, end } => {
                ctx.period = Period {
                    start: *start,
                    end: *end,
                }
            }
            Slot::Repo(id) => ctx.filter.repositories = vec![id.clone()],
            _ => {}
        }
        ctx
    }

    fn slot_for(&self, screen: Screen) -> Option<Slot> {
        match screen {
            Screen::Dashboard
            | Screen::Overview
            | Screen::Daily
            | Screen::Repositories
            | Screen::System => Some(Slot::Base),
            Screen::PullRequests => Some(Slot::PullRequests),
            Screen::Reviews => Some(Slot::Reviews),
            Screen::Issues => Some(Slot::Issues),
            Screen::DayDetail => self.detail_day.as_ref().map(|d| Slot::Day {
                start: d.start,
                end: d.end,
            }),
            Screen::RepoDetail => self.detail_repo.as_ref().map(|r| Slot::Repo(r.id.clone())),
            Screen::PrDetail | Screen::PrTimeline | Screen::IssueDetail | Screen::CommitDetail => {
                None
            }
        }
    }

    fn key(&self, slot: Slot) -> CacheKey {
        CacheKey {
            slot,
            start: self.period.start,
            end: self.period.end,
            filter: self.filter_preset,
        }
    }

    fn issue_key(&self) -> Option<IssueKey> {
        self.detail_issue.as_ref().map(|id| IssueKey {
            id: id.clone(),
            start: self.period.start,
            end: self.period.end,
            filter: self.filter_preset,
        })
    }

    fn build_for(slot: &Slot) -> Build {
        match slot {
            Slot::Base => Build::Dashboard,
            Slot::PullRequests => Build::Kind(ReportKind::PullRequests {
                participation: Participation::Subject,
                state: None,
                sort: PrSort::Activity,
            }),
            Slot::Reviews => Build::Kind(ReportKind::Reviews),
            Slot::Issues => Build::Kind(ReportKind::Issues),
            Slot::Day { .. } | Slot::Repo(_) => Build::Detail,
        }
    }

    fn send(&mut self, job: Job, foreground: bool) {
        if self.pending.contains(&job) {
            return;
        }
        let (build, ctx) = match &job {
            Job::Report(k) => (Self::build_for(&k.slot), self.context_for(&k.slot)),
            _ => (Build::Kind(ReportKind::Overview), self.query_context()),
        };
        let req = Request {
            job: job.clone(),
            generation: self.generation,
            foreground,
            view: self.period,
            ctx,
            build,
        };
        self.pending.insert(job.clone());
        match &mut self.compute {
            Compute::Inline(w) => {
                let resp = w.handle(req);
                self.apply(resp);
            }
            Compute::Thread { tx, .. } => {
                if tx.send(req).is_err() {
                    self.pending.remove(&job);
                    let e = "the report worker stopped".to_string();
                    match job {
                        Job::Report(k) => {
                            self.errors.insert(k, e);
                        }
                        Job::Pr(id) => {
                            self.pr_reports.insert(id, Err(e));
                        }
                        Job::Issue(k) => {
                            self.issue_reports.insert(k, Err(e));
                        }
                        Job::System => self.system.error = Some(e),
                    }
                }
            }
        }
    }

    fn apply(&mut self, resp: Response) {
        if resp.generation != self.generation {
            // Computed from data that has since been reloaded.
            return;
        }
        self.pending.remove(&resp.job);
        if resp.loaded_dataset {
            self.dataset_loads += 1;
        }
        match (resp.job, resp.outcome) {
            (Job::Report(k), Outcome::Report(Ok(r))) => {
                self.report_builds += 1;
                self.reports.insert(k, *r);
            }
            (Job::Report(k), Outcome::Report(Err(e))) => {
                self.errors.insert(k, e);
            }
            (Job::Pr(id), Outcome::Pr(r)) => {
                if r.is_ok() {
                    self.report_builds += 1;
                }
                self.pr_reports.insert(id, r.map(|o| o.map(|b| *b)));
            }
            (Job::Issue(k), Outcome::Issue(r)) => {
                if r.is_ok() {
                    self.report_builds += 1;
                }
                self.issue_reports.insert(k, r.map(|o| o.map(|b| *b)));
            }
            (Job::System, Outcome::System(info)) => self.system = *info,
            _ => {}
        }
    }

    /// Applies finished computations. Never blocks. Returns whether
    /// anything changed.
    pub fn poll_results(&mut self) -> bool {
        let mut got = Vec::new();
        if let Compute::Thread { rx, .. } = &self.compute {
            got.extend(rx.try_iter());
        }
        let changed = !got.is_empty();
        for r in got {
            self.apply(r);
        }
        if changed {
            // A dropped request may still be needed by the current view.
            self.prepare();
        }
        changed
    }

    /// Whether any computation is outstanding.
    pub fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether the view on screen is still being computed.
    pub fn is_computing(&self) -> bool {
        if let Some(slot) = self.slot_for(self.screen) {
            return self.pending.contains(&Job::Report(self.key(slot)));
        }
        match self.screen {
            Screen::IssueDetail => self
                .issue_key()
                .is_some_and(|k| self.pending.contains(&Job::Issue(k))),
            Screen::CommitDetail => false,
            _ => self
                .detail_pr
                .as_ref()
                .is_some_and(|id| self.pending.contains(&Job::Pr(id.clone()))),
        }
    }

    /// Blocks until outstanding computations finish or `timeout` passes.
    /// Returns whether everything finished.
    pub fn settle(&mut self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.prepare();
            if self.pending.is_empty() {
                return true;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            let resp = match &self.compute {
                Compute::Thread { rx, .. } => rx.recv_timeout(deadline - now),
                Compute::Inline(_) => return true,
            };
            match resp {
                Ok(r) => {
                    self.apply(r);
                    self.poll_results();
                }
                Err(_) => return self.pending.is_empty(),
            }
        }
    }

    /// Requests whatever the current screen needs that is not computed yet.
    /// Cheap when already cached; never touches the network and never
    /// waits for the computation.
    pub fn prepare(&mut self) {
        if let Some(slot) = self.slot_for(self.screen) {
            let key = self.key(slot);
            if !self.reports.contains_key(&key) && !self.errors.contains_key(&key) {
                self.send(Job::Report(key), true);
            }
            return;
        }
        match self.screen {
            Screen::IssueDetail => {
                if let Some(k) = self.issue_key()
                    && !self.issue_reports.contains_key(&k)
                {
                    self.send(Job::Issue(k), true);
                }
            }
            Screen::PrDetail | Screen::PrTimeline => {
                if let Some(id) = self.detail_pr.clone()
                    && !self.pr_reports.contains_key(&id)
                {
                    self.send(Job::Pr(id), true);
                }
            }
            _ => {}
        }
    }

    /// Requests the reports the user is likely to open next, so switching
    /// screens hits the cache. Returns `false` when nothing is left to do.
    /// Meant to be called while the UI is idle.
    pub fn warm_up(&mut self) -> bool {
        let mut sent = false;
        for slot in [Slot::Base, Slot::PullRequests, Slot::Reviews, Slot::Issues] {
            let key = self.key(slot);
            let job = Job::Report(key.clone());
            if !self.reports.contains_key(&key)
                && !self.errors.contains_key(&key)
                && !self.pending.contains(&job)
            {
                self.send(job, false);
                sent = true;
            }
        }
        sent
    }

    fn report(&self, slot: Slot) -> Option<&Report> {
        self.reports.get(&self.key(slot))
    }

    /// The report behind the current screen, if computed.
    pub fn current_report(&self) -> Option<&Report> {
        self.slot_for(self.screen).and_then(|s| self.report(s))
    }

    /// Summary, daily breakdown, repositories and the most recently active
    /// pull requests for the current period.
    pub fn base_report(&self) -> Option<&Report> {
        self.report(Slot::Base)
    }

    pub fn pr_list_report(&self) -> Option<&Report> {
        self.report(Slot::PullRequests)
    }

    pub fn reviews_report(&self) -> Option<&Report> {
        self.report(Slot::Reviews)
    }

    pub fn issues_report(&self) -> Option<&Report> {
        self.report(Slot::Issues)
    }

    /// The report of the day or repository detail last opened.
    pub fn detail_report(&self) -> Option<&Report> {
        self.scope
            .and_then(|s| self.slot_for(s))
            .and_then(|s| self.report(s))
    }

    /// Error reading the cache for the current screen.
    pub fn load_error(&self) -> Option<&str> {
        if let Some(slot) = self.slot_for(self.screen) {
            return self.errors.get(&self.key(slot)).map(String::as_str);
        }
        match self.screen {
            Screen::IssueDetail => self
                .issue_key()
                .and_then(|k| self.issue_reports.get(&k))
                .and_then(|r| r.as_ref().err())
                .map(String::as_str),
            // Commit loading errors are shown on the commit screen itself.
            Screen::CommitDetail => None,
            _ => self
                .detail_pr
                .as_ref()
                .and_then(|id| self.pr_reports.get(id))
                .and_then(|r| r.as_ref().err())
                .map(String::as_str),
        }
    }

    /// The pull request shown on the detail screens.
    pub fn pr_report(&self) -> Option<&PrReport> {
        self.detail_pr
            .as_ref()
            .and_then(|id| self.pr_reports.get(id))
            .and_then(|r| r.as_ref().ok())
            .and_then(|r| r.as_ref())
    }

    /// The issue shown on the issue detail screen.
    pub fn issue_report(&self) -> Option<&IssueReport> {
        self.issue_key()
            .and_then(|k| self.issue_reports.get(&k))
            .and_then(|r| r.as_ref().ok())
            .and_then(|r| r.as_ref())
    }

    /// Whether the issue detail lookup finished (found or not).
    pub fn issue_lookup_done(&self) -> bool {
        self.issue_key()
            .is_some_and(|k| self.issue_reports.contains_key(&k))
    }

    /// Coverage of the data on screen.
    pub fn coverage(&self) -> Option<CoverageStatus> {
        match self.screen {
            Screen::PrDetail | Screen::PrTimeline => self.pr_report().map(|r| r.coverage),
            Screen::IssueDetail => self.base_report().map(|r| r.overall_coverage()),
            Screen::CommitDetail => None,
            _ => self.current_report().map(|r| r.overall_coverage()),
        }
    }

    /// Drops every computed report and re-reads cache status.
    pub fn reload(&mut self) {
        self.generation += 1;
        self.pending.clear();
        self.reports.clear();
        self.errors.clear();
        self.pr_reports.clear();
        self.issue_reports.clear();
        self.send(Job::System, true);
        self.prepare();
    }

    /// Whether the PR detail lookup finished (found or not).
    pub fn pr_lookup_done(&self) -> bool {
        self.detail_pr
            .as_ref()
            .is_some_and(|id| self.pr_reports.contains_key(id))
    }

    // ----- navigation -------------------------------------------------

    /// The numbered tab the current screen was reached from.
    pub fn tab(&self) -> Screen {
        self.nav.first().copied().unwrap_or(self.screen).top()
    }

    /// Detail screens opened from the tab, in order, ending with the
    /// current screen. Empty on a tab itself.
    pub fn breadcrumb(&self) -> Vec<Screen> {
        if self.nav.is_empty() {
            return if self.screen.is_detail() {
                vec![self.screen]
            } else {
                Vec::new()
            };
        }
        let mut out: Vec<Screen> = self.nav[1..].to_vec();
        out.push(self.screen);
        out
    }

    fn open(&mut self, screen: Screen) {
        if self.nav.len() >= MAX_NAV_DEPTH {
            self.nav.remove(0);
        }
        self.nav.push(self.screen);
        self.screen = screen;
    }

    fn go_to_tab(&mut self, screen: Screen) {
        self.nav.clear();
        self.screen = screen;
    }

    fn open_pr(&mut self, id: String) {
        if self.detail_pr.as_ref() != Some(&id) {
            self.timeline_filter = TimelineFilter::All;
            for l in [ListId::Timeline, ListId::PrCommits] {
                self.search.remove(&l);
                self.reset_list(l);
            }
        }
        self.pr_pane = PrPane::Summary;
        self.detail_pr = Some(id);
        self.open(Screen::PrDetail);
    }

    /// Opens the commit screen and asks for the commit unless it is
    /// already loaded or loading.
    fn open_commit(&mut self, key: CommitKey, seed: CommitSeed) {
        if self.detail_commit.as_ref() != Some(&key) {
            self.commit_scroll = 0;
        }
        self.detail_commit = Some(key.clone());
        self.commit_seed = seed;
        self.open(Screen::CommitDetail);
        if matches!(self.commits.get(&key), None | Some(CommitState::Failed(_))) {
            self.commits.insert(key.clone(), CommitState::Loading);
            self.outbox = Some(Effect::LoadCommit(key));
        }
    }

    /// Loading state of the commit on the commit screen.
    pub fn commit_state(&self) -> Option<&CommitState> {
        self.detail_commit
            .as_ref()
            .and_then(|k| self.commits.get(k))
    }

    /// The loaded commit on the commit screen.
    pub fn commit_view(&self) -> Option<&CommitView> {
        match self.commit_state() {
            Some(CommitState::Ready(v)) => Some(v),
            _ => None,
        }
    }

    /// Whether any commit is being loaded.
    pub fn commit_loading(&self) -> bool {
        self.commits
            .values()
            .any(|s| matches!(s, CommitState::Loading))
    }

    /// Applies a loaded commit (or why it could not be loaded).
    pub fn on_commit(&mut self, msg: CommitMsg) {
        let state = match msg.result {
            Ok(v) => CommitState::Ready(v),
            Err(f) => CommitState::Failed(f),
        };
        self.commits.insert(msg.key, state);
    }

    /// Scrolls the commit screen to `row` (clamped to the content).
    fn scroll_commit_to(&mut self, row: usize) {
        let len = self.commit_view().map_or(0, |v| v.rows.len());
        let height = self.commit_view_height.get().max(1);
        self.commit_scroll = row.min(len.saturating_sub(height));
    }

    fn scroll_commit(&mut self, delta: isize) {
        let row = self.commit_scroll.saturating_add_signed(delta);
        self.scroll_commit_to(row);
    }

    /// Keys of the commit screen. Returns `false` for keys it does not use.
    fn commit_key(&mut self, code: KeyCode) -> bool {
        let page = self.commit_view_height.get().max(1) as isize;
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_commit(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_commit(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_commit(page),
            KeyCode::PageUp => self.scroll_commit(-page),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_commit_to(0),
            KeyCode::End | KeyCode::Char('G') => self.scroll_commit_to(usize::MAX),
            KeyCode::Char(']') => match self.commit_view().map(|v| v.next_file(self.commit_scroll))
            {
                Some(Some(row)) => self.scroll_commit_to(row),
                Some(None) => self.message = Some("no next file".into()),
                None => {}
            },
            KeyCode::Char('[') => match self.commit_view().map(|v| v.prev_file(self.commit_scroll))
            {
                Some(Some(row)) => self.scroll_commit_to(row),
                // Before the first file: back to the top.
                Some(None) if self.commit_scroll > 0 => self.scroll_commit_to(0),
                Some(None) => self.message = Some("no previous file".into()),
                None => {}
            },
            KeyCode::Char('r') => {
                if let (Some(key), Some(CommitState::Failed(_))) =
                    (self.detail_commit.clone(), self.commit_state())
                {
                    self.commits.insert(key.clone(), CommitState::Loading);
                    self.outbox = Some(Effect::LoadCommit(key));
                    self.message = Some("loading the commit again".into());
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    /// The commit behind the selected row of the PR Detail commit list.
    fn selected_pr_commit(&self) -> Option<(CommitKey, CommitSeed)> {
        let r = self.pr_report()?;
        let c = r.commit_list.get(self.selected_row(ListId::PrCommits)?)?;
        Some((
            CommitKey {
                repository: r.repository.clone(),
                sha: c.sha.clone(),
            },
            CommitSeed {
                headline: c.headline.clone(),
                author: Some(c.author.clone()),
                date: Some(c.committed_at),
                url: c.url.clone(),
                removed: c.removed,
                is_merge: c.is_merge_commit,
            },
        ))
    }

    /// The commit behind the selected row of a day or repository detail.
    fn selected_detail_commit(&self) -> Option<(CommitKey, CommitSeed)> {
        let r = self.detail_report()?;
        let c = r
            .commits
            .as_ref()?
            .get(self.selected_row(ListId::DetailCommits)?)?;
        Some((
            CommitKey {
                repository: c.repository.clone(),
                sha: c.sha.clone()?,
            },
            CommitSeed {
                headline: c.message_headline.clone(),
                author: Some(self.system.login.clone()).filter(|l| !l.is_empty()),
                date: Some(c.committed_at),
                url: c.url.clone(),
                removed: false,
                is_merge: c.is_merge_commit,
            },
        ))
    }

    fn open_issue(&mut self, id: String) {
        if self.detail_issue.as_ref() != Some(&id) {
            self.search.remove(&ListId::IssueEvents);
            self.reset_list(ListId::IssueEvents);
        }
        self.detail_issue = Some(id);
        self.open(Screen::IssueDetail);
    }

    fn reset_detail_lists(&mut self, first: DetailTab) {
        for l in ListId::DETAIL {
            self.search.remove(&l);
            self.reset_list(l);
        }
        self.detail_tab = first;
    }

    fn open_day(&mut self, day: DayScope) {
        if self.scope != Some(Screen::DayDetail) || self.detail_day.as_ref() != Some(&day) {
            self.reset_detail_lists(DetailTab::DAY[0]);
        }
        self.detail_day = Some(day);
        self.scope = Some(Screen::DayDetail);
        self.open(Screen::DayDetail);
    }

    fn open_repo(&mut self, repo: RepoScope) {
        if self.scope != Some(Screen::RepoDetail) || self.detail_repo.as_ref() != Some(&repo) {
            self.reset_detail_lists(DetailTab::REPO[0]);
        }
        self.detail_repo = Some(repo);
        self.scope = Some(Screen::RepoDetail);
        self.open(Screen::RepoDetail);
    }

    fn back(&mut self) {
        if let Some(prev) = self.nav.pop() {
            self.screen = prev;
            return;
        }
        if self.screen.is_detail() {
            self.screen = self.screen.top();
            return;
        }
        if let Some(list) = self.current_list()
            && self.search.remove(&list).is_some()
        {
            self.reset_list(list);
        }
    }

    /// Leaves day details: the day may not be part of a new period.
    fn leave_day_detail(&mut self) {
        if self.screen == Screen::DayDetail || self.nav.contains(&Screen::DayDetail) {
            let tab = self.tab();
            self.go_to_tab(tab);
        }
    }

    /// Section order of the current day or repository detail.
    pub fn detail_tabs(&self) -> &'static [DetailTab] {
        match self.scope {
            Some(Screen::RepoDetail) => &DetailTab::REPO,
            _ => &DetailTab::DAY,
        }
    }

    // ----- lists ------------------------------------------------------

    /// The list that keys act on, if any.
    pub fn current_list(&self) -> Option<ListId> {
        match self.screen {
            Screen::Dashboard => match self.dashboard_pane {
                DashboardPane::Summary => None,
                DashboardPane::Repositories => Some(ListId::DashboardRepos),
                DashboardPane::PullRequests => Some(ListId::DashboardPrs),
            },
            Screen::Overview => {
                (self.overview_pane == OverviewPane::Repositories).then_some(ListId::OverviewRepos)
            }
            Screen::Daily => Some(ListId::Daily),
            Screen::Repositories => Some(ListId::Repositories),
            Screen::PullRequests => Some(ListId::PullRequests),
            Screen::Reviews => Some(match self.review_direction {
                ReviewDirection::Submitted => ListId::ReviewsSubmitted,
                ReviewDirection::Received => ListId::ReviewsReceived,
            }),
            Screen::Issues => Some(ListId::Issues),
            Screen::IssueDetail => Some(ListId::IssueEvents),
            Screen::DayDetail | Screen::RepoDetail => Some(self.detail_tab.list()),
            Screen::PrTimeline => Some(ListId::Timeline),
            Screen::PrDetail => (self.pr_pane == PrPane::Commits).then_some(ListId::PrCommits),
            Screen::System | Screen::CommitDetail => None,
        }
    }

    pub fn search_query(&self, list: ListId) -> &str {
        self.search.get(&list).map(String::as_str).unwrap_or("")
    }

    pub fn selected(&self, list: ListId) -> usize {
        self.selection.get(&list).copied().unwrap_or(0)
    }

    /// Reviews of the day or repository detail: submitted, then received.
    pub fn detail_reviews(&self) -> Vec<(ReviewDirection, &ReviewRow)> {
        let Some(r) = self.detail_report() else {
            return Vec::new();
        };
        let sub = r.reviews_submitted.iter().flatten();
        let rec = r.reviews_received.iter().flatten();
        sub.map(|x| (ReviewDirection::Submitted, x))
            .chain(rec.map(|x| (ReviewDirection::Received, x)))
            .collect()
    }

    /// Indices into the underlying rows that are visible after search and
    /// filters, in display order.
    pub fn visible(&self, list: ListId) -> Vec<usize> {
        let q = self.search_query(list).to_lowercase();
        let hit =
            |parts: &[&str]| q.is_empty() || parts.iter().any(|p| p.to_lowercase().contains(&q));
        let pr_hit = |p: &PrRow| {
            let num = format!("#{}", p.number);
            hit(&[&p.title, &p.repository, &num, &p.author])
        };
        let issue_hit = |x: &IssueRow| {
            let num = format!("#{}", x.number);
            hit(&[&x.title, &x.repository, &num])
        };
        let indices =
            |n: usize, keep: &dyn Fn(usize) -> bool| (0..n).filter(|i| keep(*i)).collect();
        match list {
            ListId::DashboardRepos | ListId::OverviewRepos => self
                .base_report()
                .map(|r| (0..r.repositories.len()).collect())
                .unwrap_or_default(),
            ListId::DashboardPrs => self
                .base_report()
                .and_then(|r| r.pull_requests.as_ref())
                .map(|rows| (0..rows.len()).collect())
                .unwrap_or_default(),
            ListId::Repositories => self
                .base_report()
                .map(|r| {
                    indices(r.repositories.len(), &|i| {
                        hit(&[&r.repositories[i].full_name])
                    })
                })
                .unwrap_or_default(),
            ListId::Daily => self
                .base_report()
                .map(|r| {
                    // Newest day first.
                    (0..r.activity_breakdown.len())
                        .rev()
                        .filter(|i| hit(&[&r.activity_breakdown[*i].bucket.label]))
                        .collect()
                })
                .unwrap_or_default(),
            ListId::PullRequests => self
                .pr_list_report()
                .and_then(|r| r.pull_requests.as_ref())
                .map(|rows| indices(rows.len(), &|i| pr_hit(&rows[i])))
                .unwrap_or_default(),
            ListId::ReviewsSubmitted | ListId::ReviewsReceived => self
                .reviews_report()
                .and_then(|r| {
                    if list == ListId::ReviewsSubmitted {
                        r.reviews_submitted.as_ref()
                    } else {
                        r.reviews_received.as_ref()
                    }
                })
                .map(|rows| {
                    indices(rows.len(), &|i| {
                        let x = &rows[i];
                        hit(&[
                            &x.reviewer,
                            &x.repository,
                            x.pr_title.as_deref().unwrap_or(""),
                        ])
                    })
                })
                .unwrap_or_default(),
            ListId::Issues => self
                .issues_report()
                .and_then(|r| r.issues.as_ref())
                .map(|rows| indices(rows.len(), &|i| issue_hit(&rows[i])))
                .unwrap_or_default(),
            ListId::IssueEvents => self
                .issue_report()
                .map(|r| {
                    indices(r.events.len(), &|i| {
                        let e = &r.events[i];
                        hit(&[&e.actor, e.event_type.label()])
                    })
                })
                .unwrap_or_default(),
            ListId::Timeline => self
                .pr_report()
                .map(|r| {
                    r.timeline
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| self.timeline_filter.matches(e.event_type))
                        .filter(|(_, e)| {
                            hit(&[&e.actor, &e.label, e.detail.as_deref().unwrap_or("")])
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .unwrap_or_default(),
            ListId::DetailActivity => self
                .detail_report()
                .and_then(|r| r.activity.as_ref())
                .map(|rows| {
                    // Newest first.
                    (0..rows.len())
                        .rev()
                        .filter(|i| {
                            let a = &rows[*i];
                            hit(&[
                                &a.actor,
                                a.event_type.label(),
                                &a.repository,
                                a.title.as_deref().unwrap_or(""),
                            ])
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ListId::DetailCommits => self
                .detail_report()
                .and_then(|r| r.commits.as_ref())
                .map(|rows| {
                    (0..rows.len())
                        .rev()
                        .filter(|i| {
                            let c = &rows[*i];
                            hit(&[c.sha.as_deref().unwrap_or(""), &c.repository])
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ListId::DetailPullRequests => self
                .detail_report()
                .and_then(|r| r.pull_requests.as_ref())
                .map(|rows| indices(rows.len(), &|i| pr_hit(&rows[i])))
                .unwrap_or_default(),
            ListId::DetailReviews => {
                let rows = self.detail_reviews();
                indices(rows.len(), &|i| {
                    let x = rows[i].1;
                    hit(&[
                        &x.reviewer,
                        &x.repository,
                        x.pr_title.as_deref().unwrap_or(""),
                    ])
                })
            }
            ListId::DetailComments => self
                .detail_report()
                .and_then(|r| r.comments.as_ref())
                .map(|rows| {
                    indices(rows.len(), &|i| {
                        let c = &rows[i];
                        hit(&[&c.author, &c.repository, c.kind.label()])
                    })
                })
                .unwrap_or_default(),
            ListId::DetailIssues => self
                .detail_report()
                .and_then(|r| r.issues.as_ref())
                .map(|rows| indices(rows.len(), &|i| issue_hit(&rows[i])))
                .unwrap_or_default(),
            ListId::PrCommits => self
                .pr_report()
                .map(|r| {
                    let rows = &r.commit_list;
                    indices(rows.len(), &|i| {
                        let c = &rows[i];
                        hit(&[&c.sha, c.headline.as_deref().unwrap_or(""), &c.author])
                    })
                })
                .unwrap_or_default(),
        }
    }

    /// Underlying row index of the selected item.
    pub fn selected_row(&self, list: ListId) -> Option<usize> {
        let v = self.visible(list);
        v.get(self.selected(list).min(v.len().saturating_sub(1)))
            .copied()
    }

    fn move_selection(&mut self, delta: isize) {
        let Some(list) = self.current_list() else {
            return;
        };
        let len = self.visible(list).len();
        let cur = self.selected(list) as isize;
        let next = if len == 0 {
            0
        } else {
            (cur + delta).clamp(0, len as isize - 1) as usize
        };
        self.selection.insert(list, next);
    }

    fn reset_selection(&mut self) {
        self.selection.clear();
        self.offsets.borrow_mut().clear();
    }

    fn reset_list(&mut self, list: ListId) {
        self.selection.insert(list, 0);
        self.offsets.borrow_mut().insert(list, 0);
    }

    // ----- keys -------------------------------------------------------

    /// Applies a key press. Returns what the event loop has to do.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        self.outbox = None;
        let effect = self.dispatch(key).or_else(|| self.outbox.take());
        self.prepare();
        effect
    }

    fn dispatch(&mut self, key: KeyEvent) -> Option<Effect> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Some(Effect::Quit);
        }
        if self.search_mode {
            self.search_key(key);
            return None;
        }
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => self.show_help = false,
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return Some(Effect::Quit);
                }
                _ => {}
            }
            return None;
        }
        self.message = None;
        if self.screen == Screen::CommitDetail && self.commit_key(key.code) {
            return None;
        }
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return Some(Effect::Quit);
            }
            KeyCode::Char(c @ '1'..='8') => {
                let i = (c as u8 - b'1') as usize;
                self.go_to_tab(Screen::TOP[i]);
            }
            KeyCode::Enter => self.enter(),
            KeyCode::Esc => self.back(),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => self.move_selection(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_selection(isize::MAX / 2),
            KeyCode::Tab => self.switch_pane(1),
            KeyCode::BackTab => self.switch_pane(-1),
            KeyCode::Char('/') => match self.current_list() {
                Some(_) if !matches!(self.screen, Screen::Overview | Screen::Dashboard) => {
                    self.search_mode = true
                }
                _ => self.message = Some("search works on lists (screens 3–7, details)".into()),
            },
            KeyCode::Char('r') => return self.refresh(),
            KeyCode::Char('t') => self.cycle_time_range(),
            KeyCode::Char('f') => self.cycle_filter(),
            KeyCode::Char('o') => return self.open_selected(),
            KeyCode::Char('?') => self.show_help = true,
            _ => {}
        }
        None
    }

    fn search_key(&mut self, key: KeyEvent) {
        let Some(list) = self.current_list() else {
            self.search_mode = false;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.search.remove(&list);
                self.search_mode = false;
                self.reset_list(list);
            }
            KeyCode::Enter => self.search_mode = false,
            KeyCode::Backspace => {
                self.search.entry(list).or_default().pop();
                self.reset_list(list);
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.entry(list).or_default().push(c);
                self.reset_list(list);
            }
            _ => {}
        }
    }

    fn selected_repo(&self, list: ListId) -> Option<RepoScope> {
        let i = self.selected_row(list)?;
        let r = self.base_report()?.repositories.get(i)?;
        Some(RepoScope {
            id: r.repository_id.clone(),
            full_name: r.full_name.clone(),
        })
    }

    fn enter(&mut self) {
        match self.screen {
            Screen::Dashboard => match self.dashboard_pane {
                DashboardPane::Summary => {}
                DashboardPane::Repositories => {
                    if let Some(r) = self.selected_repo(ListId::DashboardRepos) {
                        self.open_repo(r);
                    }
                }
                DashboardPane::PullRequests => {
                    let id = self.selected_row(ListId::DashboardPrs).and_then(|i| {
                        self.base_report()
                            .and_then(|r| r.pull_requests.as_ref())
                            .and_then(|rows| rows.get(i))
                            .map(|p| p.pr_id.clone())
                    });
                    if let Some(id) = id {
                        self.open_pr(id);
                    }
                }
            },
            Screen::Overview => {
                if self.overview_pane == OverviewPane::Repositories
                    && let Some(r) = self.selected_repo(ListId::OverviewRepos)
                {
                    self.open_repo(r);
                }
            }
            Screen::Repositories => {
                if let Some(r) = self.selected_repo(ListId::Repositories) {
                    self.open_repo(r);
                }
            }
            Screen::Daily => {
                let day = self.selected_row(ListId::Daily).and_then(|i| {
                    self.base_report()
                        .and_then(|r| r.activity_breakdown.get(i))
                        .map(|b| DayScope {
                            start: b.bucket.start,
                            end: b.bucket.end,
                            label: b.bucket.label.clone(),
                        })
                });
                if let Some(day) = day {
                    self.open_day(day);
                }
            }
            Screen::PullRequests => {
                let id = self.selected_row(ListId::PullRequests).and_then(|i| {
                    self.pr_list_report()
                        .and_then(|r| r.pull_requests.as_ref())
                        .and_then(|rows| rows.get(i))
                        .map(|p| p.pr_id.clone())
                });
                if let Some(id) = id {
                    self.open_pr(id);
                }
            }
            Screen::PrDetail => match self.pr_pane {
                PrPane::Summary => {
                    if self.pr_report().is_some() {
                        self.open(Screen::PrTimeline);
                    }
                }
                PrPane::Commits => {
                    if let Some((key, seed)) = self.selected_pr_commit() {
                        self.open_commit(key, seed);
                    }
                }
            },
            Screen::Reviews => {
                let Some(list) = self.current_list() else {
                    return;
                };
                let id = self.selected_row(list).and_then(|i| {
                    self.reviews_report()
                        .and_then(|r| {
                            if list == ListId::ReviewsSubmitted {
                                r.reviews_submitted.as_ref()
                            } else {
                                r.reviews_received.as_ref()
                            }
                        })
                        .and_then(|rows| rows.get(i))
                        .and_then(|x| x.pr_id.clone())
                });
                match id {
                    Some(id) => self.open_pr(id),
                    None if self.selected_row(list).is_some() => {
                        self.message = Some("this review's pull request is not cached".into())
                    }
                    None => {}
                }
            }
            Screen::Issues => {
                let id = self.selected_row(ListId::Issues).and_then(|i| {
                    self.issues_report()
                        .and_then(|r| r.issues.as_ref())
                        .and_then(|rows| rows.get(i))
                        .map(|x| x.issue_id.clone())
                });
                if let Some(id) = id {
                    self.open_issue(id);
                }
            }
            Screen::DayDetail | Screen::RepoDetail => self.enter_detail_row(),
            Screen::PrTimeline | Screen::IssueDetail | Screen::System | Screen::CommitDetail => {}
        }
    }

    /// Opens the pull request or issue behind the selected detail row.
    fn enter_detail_row(&mut self) {
        enum Target {
            Pr(String),
            Issue(String),
        }
        let list = self.detail_tab.list();
        let Some(i) = self.selected_row(list) else {
            return;
        };
        if self.detail_tab == DetailTab::Commits {
            match self.selected_detail_commit() {
                Some((key, seed)) => self.open_commit(key, seed),
                None => self.message = Some("this commit has no SHA to open".into()),
            }
            return;
        }
        let Some(r) = self.detail_report() else {
            return;
        };
        let target = match self.detail_tab {
            DetailTab::Activity => r
                .activity
                .as_ref()
                .and_then(|rows| rows.get(i))
                .and_then(|a| {
                    a.pr_id
                        .clone()
                        .map(Target::Pr)
                        .or_else(|| a.issue_id.clone().map(Target::Issue))
                }),
            DetailTab::Commits => None,
            DetailTab::PullRequests => r
                .pull_requests
                .as_ref()
                .and_then(|rows| rows.get(i))
                .map(|p| Target::Pr(p.pr_id.clone())),
            DetailTab::Reviews => self
                .detail_reviews()
                .get(i)
                .and_then(|(_, x)| x.pr_id.clone())
                .map(Target::Pr),
            DetailTab::Comments => r
                .comments
                .as_ref()
                .and_then(|rows| rows.get(i))
                .and_then(|c| c.pr_id.clone())
                .map(Target::Pr),
            DetailTab::Issues => r
                .issues
                .as_ref()
                .and_then(|rows| rows.get(i))
                .map(|x| Target::Issue(x.issue_id.clone())),
        };
        match target {
            Some(Target::Pr(id)) => self.open_pr(id),
            Some(Target::Issue(id)) => self.open_issue(id),
            None => self.message = Some("nothing to open for this row".into()),
        }
    }

    fn switch_pane(&mut self, delta: isize) {
        match self.screen {
            Screen::Dashboard => self.dashboard_pane = self.dashboard_pane.step(delta),
            Screen::Overview => {
                self.overview_pane = match self.overview_pane {
                    OverviewPane::Summary => OverviewPane::Repositories,
                    OverviewPane::Repositories => OverviewPane::Summary,
                }
            }
            Screen::Reviews => {
                self.review_direction = match self.review_direction {
                    ReviewDirection::Submitted => ReviewDirection::Received,
                    ReviewDirection::Received => ReviewDirection::Submitted,
                }
            }
            Screen::PrTimeline => {
                self.timeline_filter = self.timeline_filter.step(delta);
                self.reset_list(ListId::Timeline);
            }
            Screen::PrDetail => {
                self.pr_pane = match self.pr_pane {
                    PrPane::Summary => PrPane::Commits,
                    PrPane::Commits => PrPane::Summary,
                }
            }
            Screen::DayDetail | Screen::RepoDetail => {
                let tabs = self.detail_tabs();
                let n = tabs.len() as isize;
                let i = tabs.iter().position(|t| *t == self.detail_tab).unwrap_or(0) as isize;
                self.detail_tab = tabs[((i + delta).rem_euclid(n)) as usize];
            }
            _ => {}
        }
    }

    /// Re-reads the cache; starts a background sync when enabled.
    pub fn refresh(&mut self) -> Option<Effect> {
        self.reload();
        if !self.background_sync {
            self.message = Some("reloaded from cache (offline)".into());
            return None;
        }
        if self.sync.running {
            self.message = Some("reloaded from cache; sync already running".into());
            return None;
        }
        self.message = Some("reloaded from cache; syncing in the background".into());
        self.start_sync()
    }

    /// Marks a sync as started and asks the event loop to run it.
    pub fn start_sync(&mut self) -> Option<Effect> {
        if !self.background_sync || self.sync.running {
            return None;
        }
        self.sync.running = true;
        self.sync.progress = Some("connecting".into());
        self.sync.last_error = None;
        Some(Effect::StartSync(self.period))
    }

    fn cycle_time_range(&mut self) {
        let mut next = self.time_range.next();
        let now = (self.clock)();
        // Skip presets that cannot be resolved (never expected).
        for _ in 0..TimeRange::CYCLE.len() {
            let p = match next.since() {
                None => Ok(self.initial_period),
                Some(since) => resolve_period(Some(since), None, since, &self.zone, now),
            };
            if let Ok(p) = p {
                self.time_range = next;
                self.period = p;
                break;
            }
            next = next.next();
        }
        self.leave_day_detail();
        self.reset_selection();
        self.message = Some(format!("time range: {}", self.time_range.label()));
    }

    fn cycle_filter(&mut self) {
        self.filter_preset = self.filter_preset.next();
        self.leave_day_detail();
        self.reset_selection();
        self.message = Some(format!("repositories: {}", self.filter_preset.label()));
    }

    /// URL of the selected item.
    pub fn selected_url(&self) -> Option<String> {
        let repo_url = |list| {
            self.selected_row(list).and_then(|i| {
                self.base_report()
                    .and_then(|r| r.repositories.get(i))
                    .map(|r| format!("https://github.com/{}", r.full_name))
            })
        };
        let row = |list| self.selected_row(list);
        match self.screen {
            Screen::Dashboard => match self.dashboard_pane {
                DashboardPane::Summary => None,
                DashboardPane::Repositories => repo_url(ListId::DashboardRepos),
                DashboardPane::PullRequests => row(ListId::DashboardPrs).and_then(|i| {
                    self.base_report()
                        .and_then(|r| r.pull_requests.as_ref())
                        .and_then(|rows| rows.get(i))
                        .map(|p| p.url.clone())
                }),
            },
            Screen::Overview if self.overview_pane == OverviewPane::Repositories => {
                repo_url(ListId::OverviewRepos)
            }
            Screen::Repositories => repo_url(ListId::Repositories),
            Screen::PullRequests => row(ListId::PullRequests).and_then(|i| {
                self.pr_list_report()
                    .and_then(|r| r.pull_requests.as_ref())
                    .and_then(|rows| rows.get(i))
                    .map(|p| p.url.clone())
            }),
            Screen::PrDetail => match self.pr_pane {
                PrPane::Summary => self.pr_report().map(|r| r.url.clone()),
                PrPane::Commits => self
                    .selected_pr_commit()
                    .map(|(key, seed)| seed.url.unwrap_or_else(|| key.github_url())),
            },
            Screen::CommitDetail => {
                let key = self.detail_commit.as_ref()?;
                self.commit_view()
                    .and_then(|v| v.url.clone())
                    .or_else(|| self.commit_seed.url.clone())
                    .or_else(|| Some(key.github_url()))
            }
            Screen::PrTimeline => {
                let r = self.pr_report()?;
                row(ListId::Timeline)
                    .and_then(|i| r.timeline.get(i))
                    .and_then(|e| e.url.clone())
                    .or_else(|| Some(r.url.clone()))
            }
            Screen::Reviews => {
                let list = self.current_list()?;
                let rows = self.reviews_report().and_then(|r| {
                    if list == ListId::ReviewsSubmitted {
                        r.reviews_submitted.as_ref()
                    } else {
                        r.reviews_received.as_ref()
                    }
                })?;
                row(list)
                    .and_then(|i| rows.get(i))
                    .and_then(|x| x.url.clone())
            }
            Screen::Issues => row(ListId::Issues).and_then(|i| {
                self.issues_report()
                    .and_then(|r| r.issues.as_ref())
                    .and_then(|rows| rows.get(i))
                    .map(|x| x.url.clone())
            }),
            Screen::IssueDetail => {
                let r = self.issue_report()?;
                row(ListId::IssueEvents)
                    .and_then(|i| r.events.get(i))
                    .and_then(|e| e.url.clone())
                    .or_else(|| Some(r.url.clone()))
            }
            Screen::DayDetail | Screen::RepoDetail => {
                let r = self.detail_report()?;
                let i = row(self.detail_tab.list());
                let url = i.and_then(|i| match self.detail_tab {
                    DetailTab::Activity => r.activity.as_ref()?.get(i)?.url.clone(),
                    DetailTab::Commits => r.commits.as_ref()?.get(i)?.url.clone(),
                    DetailTab::PullRequests => Some(r.pull_requests.as_ref()?.get(i)?.url.clone()),
                    DetailTab::Reviews => self.detail_reviews().get(i)?.1.url.clone(),
                    DetailTab::Comments => r.comments.as_ref()?.get(i)?.url.clone(),
                    DetailTab::Issues => Some(r.issues.as_ref()?.get(i)?.url.clone()),
                });
                url.or_else(|| match (self.screen, &self.detail_repo) {
                    (Screen::RepoDetail, Some(repo)) => {
                        Some(format!("https://github.com/{}", repo.full_name))
                    }
                    _ => None,
                })
            }
            _ => None,
        }
        .filter(|u| !u.is_empty())
    }

    fn open_selected(&mut self) -> Option<Effect> {
        let Some(url) = self.selected_url() else {
            self.message = Some("nothing to open here".into());
            return None;
        };
        match crate::term::checked_url(&url) {
            Ok(u) => {
                self.message = Some("opening in browser".into());
                Some(Effect::OpenUrl(u))
            }
            Err(e) => {
                self.message = Some(format!("not opening link: {e}"));
                None
            }
        }
    }

    // ----- background sync -------------------------------------------

    /// Applies a message from the background worker.
    pub fn on_sync(&mut self, msg: SyncMsg) {
        match msg {
            SyncMsg::Started { auth } => {
                self.sync.running = true;
                self.sync.auth = auth;
                self.sync.progress = Some("starting".into());
            }
            SyncMsg::Progress(p) => {
                self.sync.running = true;
                self.sync.progress = Some(p);
            }
            SyncMsg::RateLimit(r) => self.sync.rate_limit = Some(r),
            SyncMsg::Finished {
                complete,
                failures,
                summary,
            } => {
                self.sync.running = false;
                self.sync.progress = None;
                self.sync.last_complete = Some(complete);
                self.sync.last_summary = Some(summary);
                self.sync.warnings = failures;
                self.sync.last_error = None;
                self.reload();
            }
            SyncMsg::Failed(e) => {
                self.sync.running = false;
                self.sync.progress = None;
                self.sync.last_error = Some(e);
            }
        }
    }

    /// The clock used for relative ranges.
    pub fn now(&self) -> Timestamp {
        (self.clock)()
    }
}
