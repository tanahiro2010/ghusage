impl App {
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

        // The numbered tab the current screen was reached from.
}
