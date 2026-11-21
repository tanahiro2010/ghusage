//! UI state and key handling. Numbers come from the shared report builders
//! (`ghusage_app::report`); this module only decides which report to show,
//! caches it, and tracks navigation, selection and search.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};

use chrono::{DateTime, Utc};
use ghusage_app::report::{PrReport, QueryContext, Report, ReportKind};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    CommitDateBasis, Filter, Ownership, Participation, PrSort, ReviewDirection, VisibilityFilter,
};
use ghusage_core::model::{EventCategory, EventType};
use ghusage_core::time::{Period, Zone, resolve_period};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::background::SyncMsg;
use crate::compute::{self, Job, Outcome, Request, Response, Worker};
use crate::source::{DataSource, SystemInfo};

/// A screen of the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    Overview,
    Daily,
    Repositories,
    PullRequests,
    PrDetail,
    PrTimeline,
    Reviews,
    System,
}

impl Screen {
    /// Screens reachable with the number keys, in key order.
    pub const TOP: [Screen; 6] = [
        Screen::Overview,
        Screen::Daily,
        Screen::Repositories,
        Screen::PullRequests,
        Screen::Reviews,
        Screen::System,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Daily => "Daily",
            Screen::Repositories => "Repositories",
            Screen::PullRequests => "Pull Requests",
            Screen::PrDetail => "PR Detail",
            Screen::PrTimeline => "PR Timeline",
            Screen::Reviews => "Reviews",
            Screen::System => "System",
        }
    }

    /// The top-level screen a screen belongs to.
    pub fn top(self) -> Screen {
        match self {
            Screen::PrDetail | Screen::PrTimeline => Screen::PullRequests,
            s => s,
        }
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

/// A scrollable list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListId {
    OverviewRepos,
    Daily,
    Repositories,
    PullRequests,
    ReviewsSubmitted,
    ReviewsReceived,
    Timeline,
}

/// Which report a screen is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    /// Summary, daily breakdown and repositories.
    Base,
    PullRequests,
    Reviews,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub(crate) slot: Slot,
    pub(crate) start: DateTime<Utc>,
    pub(crate) end: DateTime<Utc>,
    pub(crate) filter: FilterPreset,
}

/// Something the event loop must do after a key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Quit,
    OpenUrl(String),
    StartSync(Period),
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

type Clock = Box<dyn Fn() -> DateTime<Utc>>;

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
    pub timeline_filter: TimelineFilter,
    /// Which commit date counts (same meaning as the CLI's `--commit-date`).
    pub commit_basis: CommitDateBasis,
    pub detail_pr: Option<String>,
    pub search_mode: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub message: Option<String>,
    pub sync: SyncState,
    pub system: SystemInfo,

    selection: HashMap<ListId, usize>,
    search: HashMap<ListId, String>,
    /// Scroll offsets remembered between frames.
    pub(crate) offsets: RefCell<HashMap<ListId, usize>>,

    generation: u64,
    pending: HashSet<Job>,
    reports: HashMap<CacheKey, Report>,
    errors: HashMap<CacheKey, String>,
    pr_reports: HashMap<String, Result<Option<PrReport>, String>>,
    report_builds: usize,
    dataset_loads: usize,
}

impl App {
    /// Creates the UI state. Reports are computed on a worker thread that
    /// owns `source`; the calling thread never waits for them.
    pub fn new(source: Box<dyn DataSource + Send>, cfg: AppConfig) -> std::io::Result<Self> {
        Self::with_clock(source, cfg, Box::new(Utc::now))
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
            screen: Screen::Overview,
            period: cfg.period,
            time_range: TimeRange::Initial,
            filter_preset: FilterPreset::All,
            review_direction: ReviewDirection::Submitted,
            overview_pane: OverviewPane::Summary,
            timeline_filter: TimelineFilter::All,
            commit_basis: CommitDateBasis::default(),
            detail_pr: None,
            search_mode: false,
            show_help: false,
            should_quit: false,
            message: None,
            sync: SyncState::default(),
            system,
            selection: HashMap::new(),
            search: HashMap::new(),
            offsets: RefCell::new(HashMap::new()),
            generation: 0,
            pending: HashSet::new(),
            reports: HashMap::new(),
            errors: HashMap::new(),
            pr_reports: HashMap::new(),
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
            zone: self.zone,
            filter: self.filter(),
            commit_basis: self.commit_basis,
            redact_private: false,
            synced_at: self.system.synced_at,
        }
    }

    fn slot_for(screen: Screen) -> Option<Slot> {
        match screen {
            Screen::Overview | Screen::Daily | Screen::Repositories | Screen::System => {
                Some(Slot::Base)
            }
            Screen::PullRequests => Some(Slot::PullRequests),
            Screen::Reviews => Some(Slot::Reviews),
            Screen::PrDetail | Screen::PrTimeline => None,
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

    fn kind(slot: Slot) -> ReportKind {
        match slot {
            Slot::Base => ReportKind::Overview,
            Slot::PullRequests => ReportKind::PullRequests {
                participation: Participation::Subject,
                state: None,
                sort: PrSort::Activity,
            },
            Slot::Reviews => ReportKind::Reviews,
        }
    }

    fn send(&mut self, job: Job, foreground: bool) {
        if self.pending.contains(&job) {
            return;
        }
        let kind = match &job {
            Job::Report(k) => Self::kind(k.slot),
            _ => ReportKind::Overview,
        };
        let req = Request {
            job: job.clone(),
            generation: self.generation,
            foreground,
            ctx: self.query_context(),
            kind,
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
            (Job::System, Outcome::System(info)) => self.system = info,
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
        match Self::slot_for(self.screen) {
            Some(slot) => self.pending.contains(&Job::Report(self.key(slot))),
            None => self
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
        if let Some(slot) = Self::slot_for(self.screen) {
            let key = self.key(slot);
            if !self.reports.contains_key(&key) && !self.errors.contains_key(&key) {
                self.send(Job::Report(key), true);
            }
        } else if let Some(id) = self.detail_pr.clone()
            && !self.pr_reports.contains_key(&id)
        {
            self.send(Job::Pr(id), true);
        }
    }

    /// Requests the reports the user is likely to open next, so switching
    /// screens hits the cache. Returns `false` when nothing is left to do.
    /// Meant to be called while the UI is idle.
    pub fn warm_up(&mut self) -> bool {
        let mut sent = false;
        for slot in [Slot::Base, Slot::PullRequests, Slot::Reviews] {
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
        Self::slot_for(self.screen).and_then(|s| self.report(s))
    }

    /// Summary, daily breakdown and repositories for the current period.
    pub fn base_report(&self) -> Option<&Report> {
        self.report(Slot::Base)
    }

    pub fn pr_list_report(&self) -> Option<&Report> {
        self.report(Slot::PullRequests)
    }

    pub fn reviews_report(&self) -> Option<&Report> {
        self.report(Slot::Reviews)
    }

    /// Error reading the cache for the current screen.
    pub fn load_error(&self) -> Option<&str> {
        match Self::slot_for(self.screen) {
            Some(slot) => self.errors.get(&self.key(slot)).map(String::as_str),
            None => self
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

    /// Coverage of the data on screen.
    pub fn coverage(&self) -> Option<CoverageStatus> {
        match self.screen {
            Screen::PrDetail | Screen::PrTimeline => self.pr_report().map(|r| r.coverage),
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
        self.send(Job::System, true);
        self.prepare();
    }

    /// Whether the PR detail lookup finished (found or not).
    pub fn pr_lookup_done(&self) -> bool {
        self.detail_pr
            .as_ref()
            .is_some_and(|id| self.pr_reports.contains_key(id))
    }

    // ----- lists ------------------------------------------------------

    /// The list that keys act on, if any.
    pub fn current_list(&self) -> Option<ListId> {
        match self.screen {
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
            Screen::PrTimeline => Some(ListId::Timeline),
            Screen::PrDetail | Screen::System => None,
        }
    }

    pub fn search_query(&self, list: ListId) -> &str {
        self.search.get(&list).map(String::as_str).unwrap_or("")
    }

    pub fn selected(&self, list: ListId) -> usize {
        self.selection.get(&list).copied().unwrap_or(0)
    }

    /// Indices into the underlying rows that are visible after search and
    /// filters, in display order.
    pub fn visible(&self, list: ListId) -> Vec<usize> {
        let q = self.search_query(list).to_lowercase();
        let hit =
            |parts: &[&str]| q.is_empty() || parts.iter().any(|p| p.to_lowercase().contains(&q));
        match list {
            ListId::OverviewRepos | ListId::Repositories => self
                .base_report()
                .map(|r| {
                    let rows = r.repositories.iter().enumerate();
                    if list == ListId::OverviewRepos {
                        rows.map(|(i, _)| i).collect()
                    } else {
                        rows.filter(|(_, x)| hit(&[&x.full_name]))
                            .map(|(i, _)| i)
                            .collect()
                    }
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
                .map(|rows| {
                    rows.iter()
                        .enumerate()
                        .filter(|(_, p)| {
                            let num = format!("#{}", p.number);
                            hit(&[&p.title, &p.repository, &num, &p.author])
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
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
                    rows.iter()
                        .enumerate()
                        .filter(|(_, x)| {
                            hit(&[
                                &x.reviewer,
                                &x.repository,
                                x.pr_title.as_deref().unwrap_or(""),
                            ])
                        })
                        .map(|(i, _)| i)
                        .collect()
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
        let effect = self.dispatch(key);
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
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return Some(Effect::Quit);
            }
            KeyCode::Char(c @ '1'..='6') => {
                let i = (c as u8 - b'1') as usize;
                self.screen = Screen::TOP[i];
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
                Some(_) if self.screen != Screen::Overview => self.search_mode = true,
                _ => self.message = Some("search works on lists (screens 2–5, timeline)".into()),
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

    fn enter(&mut self) {
        match self.screen {
            Screen::PullRequests => {
                let id = self.selected_row(ListId::PullRequests).and_then(|i| {
                    self.pr_list_report()
                        .and_then(|r| r.pull_requests.as_ref())
                        .and_then(|rows| rows.get(i))
                        .map(|p| p.pr_id.clone())
                });
                if let Some(id) = id {
                    if self.detail_pr.as_ref() != Some(&id) {
                        self.timeline_filter = TimelineFilter::All;
                        self.search.remove(&ListId::Timeline);
                        self.reset_list(ListId::Timeline);
                    }
                    self.detail_pr = Some(id);
                    self.screen = Screen::PrDetail;
                }
            }
            Screen::PrDetail => {
                if self.pr_report().is_some() {
                    self.screen = Screen::PrTimeline;
                }
            }
            Screen::Repositories | Screen::Overview => {
                let list = if self.screen == Screen::Overview {
                    if self.overview_pane != OverviewPane::Repositories {
                        return;
                    }
                    ListId::OverviewRepos
                } else {
                    ListId::Repositories
                };
                let name = self.selected_row(list).and_then(|i| {
                    self.base_report()
                        .and_then(|r| r.repositories.get(i))
                        .map(|r| r.full_name.clone())
                });
                if let Some(name) = name {
                    // Show this repository's pull requests.
                    self.search.insert(ListId::PullRequests, name);
                    self.reset_list(ListId::PullRequests);
                    self.screen = Screen::PullRequests;
                }
            }
            _ => {}
        }
    }

    fn back(&mut self) {
        match self.screen {
            Screen::PrTimeline => self.screen = Screen::PrDetail,
            Screen::PrDetail => self.screen = Screen::PullRequests,
            _ => {
                if let Some(list) = self.current_list()
                    && self.search.remove(&list).is_some()
                {
                    self.reset_list(list);
                }
            }
        }
    }

    fn switch_pane(&mut self, delta: isize) {
        match self.screen {
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
                Some(since) => resolve_period(Some(since), None, since, self.zone, now),
            };
            if let Ok(p) = p {
                self.time_range = next;
                self.period = p;
                break;
            }
            next = next.next();
        }
        self.reset_selection();
        self.message = Some(format!("time range: {}", self.time_range.label()));
    }

    fn cycle_filter(&mut self) {
        self.filter_preset = self.filter_preset.next();
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
        match self.screen {
            Screen::Overview if self.overview_pane == OverviewPane::Repositories => {
                repo_url(ListId::OverviewRepos)
            }
            Screen::Repositories => repo_url(ListId::Repositories),
            Screen::PullRequests => self.selected_row(ListId::PullRequests).and_then(|i| {
                self.pr_list_report()
                    .and_then(|r| r.pull_requests.as_ref())
                    .and_then(|rows| rows.get(i))
                    .map(|p| p.url.clone())
            }),
            Screen::PrDetail => self.pr_report().map(|r| r.url.clone()),
            Screen::PrTimeline => {
                let r = self.pr_report()?;
                self.selected_row(ListId::Timeline)
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
                self.selected_row(list)
                    .and_then(|i| rows.get(i))
                    .and_then(|x| x.url.clone())
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
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
}
