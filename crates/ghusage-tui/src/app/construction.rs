impl App {
    /// Creates UI state and begins loading the initial reports.
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
    #[must_use]
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

        // Number of reports computed so far (cached reports are not rebuilt).
}
