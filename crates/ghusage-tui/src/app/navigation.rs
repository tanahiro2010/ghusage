impl App {
    /// Returns the current screen, resolving detail navigation state.
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
    pub const fn detail_tabs(&self) -> &'static [DetailTab] {
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

    /// Applies a key press and returns any effect for the event loop.
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
                        self.search_mode = true;
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
                            self.message = Some("this review's pull request is not cached".into());
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
                    let i = tabs.iter().position(|t| *t == self.detail_tab).unwrap_or(0);
                    self.detail_tab = tabs[stepped_index(tabs.len(), i, delta)];
                }
                _ => {}
            }
        }

        /// Re-reads the cache; starts a background sync when enabled.
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

        // Applies a message from the background worker.
}
