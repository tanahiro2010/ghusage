impl App {
    /// Returns the active search query for a list.
    pub fn search_query(&self, list: ListId) -> &str {
            self.search.get(&list).map_or("", String::as_str)
        }

    /// Returns the selected row index for a list.
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
        match list {
            ListId::DashboardRepos | ListId::OverviewRepos => self.visible_base_repos(),
            ListId::DashboardPrs => self.visible_dashboard_prs(),
            ListId::Repositories => self.visible_repositories(),
            ListId::Daily => self.visible_daily(),
            ListId::PullRequests => self.visible_pull_requests(),
            ListId::ReviewsSubmitted | ListId::ReviewsReceived => self.visible_reviews(list),
            ListId::Issues => self.visible_issues(),
            ListId::IssueEvents => self.visible_issue_events(),
            ListId::Timeline => self.visible_timeline(),
            ListId::DetailActivity => self.visible_detail_activity(),
            ListId::DetailCommits => self.visible_detail_commits(),
            ListId::DetailPullRequests => self.visible_detail_pull_requests(),
            ListId::DetailReviews => self.visible_detail_reviews(),
            ListId::DetailComments => self.visible_detail_comments(),
            ListId::DetailIssues => self.visible_detail_issues(),
            ListId::PrCommits => self.visible_pr_commits(),
        }
    }

    fn search_matches(&self, list: ListId, parts: &[&str]) -> bool {
        let q = self.search_query(list).to_lowercase();
        q.is_empty() || parts.iter().any(|p| p.to_lowercase().contains(&q))
    }

    fn visible_base_repos(&self) -> Vec<usize> {
        self.base_report().map_or_else(Vec::new, |r| (0..r.repositories.len()).collect())
    }

    fn visible_dashboard_prs(&self) -> Vec<usize> {
        self.base_report()
            .and_then(|r| r.pull_requests.as_ref())
            .map_or_else(Vec::new, |rows| (0..rows.len()).collect())
    }

    fn visible_repositories(&self) -> Vec<usize> {
        self.base_report().map_or_else(Vec::new, |r| {
            (0..r.repositories.len())
                .filter(|&i| self.search_matches(ListId::Repositories, &[&r.repositories[i].full_name]))
                .collect()
        })
    }

    fn visible_daily(&self) -> Vec<usize> {
        self.base_report().map_or_else(Vec::new, |r| {
            (0..r.activity_breakdown.len())
                .rev()
                .filter(|&i| self.search_matches(ListId::Daily, &[&r.activity_breakdown[i].bucket.label]))
                .collect()
        })
    }

    fn visible_pull_requests(&self) -> Vec<usize> {
        self.pr_list_report()
            .and_then(|r| r.pull_requests.as_ref())
            .map_or_else(Vec::new, |rows| {
                (0..rows.len()).filter(|&i| {
                    let p = &rows[i];
                    let number = format!("#{}", p.number);
                    self.search_matches(ListId::PullRequests, &[&p.title, &p.repository, &number, &p.author])
                }).collect()
            })
    }

    fn visible_reviews(&self, list: ListId) -> Vec<usize> {
        self.reviews_report()
            .and_then(|r| if list == ListId::ReviewsSubmitted { r.reviews_submitted.as_ref() } else { r.reviews_received.as_ref() })
            .map_or_else(Vec::new, |rows| {
                (0..rows.len()).filter(|&i| {
                    let x = &rows[i];
                    self.search_matches(list, &[&x.reviewer, &x.repository, x.pr_title.as_deref().unwrap_or("")])
                }).collect()
            })
    }

    fn visible_issues(&self) -> Vec<usize> {
        self.issues_report().and_then(|r| r.issues.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).filter(|&i| {
                let x = &rows[i];
                let number = format!("#{}", x.number);
                self.search_matches(ListId::Issues, &[&x.title, &x.repository, &number])
            }).collect()
        })
    }

    fn visible_issue_events(&self) -> Vec<usize> {
        self.issue_report().map_or_else(Vec::new, |r| {
            (0..r.events.len()).filter(|&i| {
                let e = &r.events[i];
                self.search_matches(ListId::IssueEvents, &[&e.actor, e.event_type.label()])
            }).collect()
        })
    }

    fn visible_timeline(&self) -> Vec<usize> {
        self.pr_report().map_or_else(Vec::new, |r| {
            r.timeline.iter().enumerate()
                .filter(|(_, e)| self.timeline_filter.matches(e.event_type))
                .filter(|(_, e)| self.search_matches(ListId::Timeline, &[&e.actor, &e.label, e.detail.as_deref().unwrap_or("")]))
                .map(|(i, _)| i).collect()
        })
    }

    fn visible_detail_activity(&self) -> Vec<usize> {
        self.detail_report().and_then(|r| r.activity.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).rev().filter(|&i| {
                let a = &rows[i];
                self.search_matches(ListId::DetailActivity, &[&a.actor, a.event_type.label(), &a.repository, a.title.as_deref().unwrap_or("")])
            }).collect()
        })
    }

    fn visible_detail_commits(&self) -> Vec<usize> {
        self.detail_report().and_then(|r| r.commits.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).rev().filter(|&i| {
                let c = &rows[i];
                self.search_matches(ListId::DetailCommits, &[c.sha.as_deref().unwrap_or(""), &c.repository])
            }).collect()
        })
    }

    fn visible_detail_pull_requests(&self) -> Vec<usize> {
        self.detail_report().and_then(|r| r.pull_requests.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).filter(|&i| {
                let p = &rows[i];
                let number = format!("#{}", p.number);
                self.search_matches(ListId::DetailPullRequests, &[&p.title, &p.repository, &number, &p.author])
            }).collect()
        })
    }

    fn visible_detail_reviews(&self) -> Vec<usize> {
        let rows = self.detail_reviews();
        (0..rows.len()).filter(|&i| {
            let x = rows[i].1;
            self.search_matches(ListId::DetailReviews, &[&x.reviewer, &x.repository, x.pr_title.as_deref().unwrap_or("")])
        }).collect()
    }

    fn visible_detail_comments(&self) -> Vec<usize> {
        self.detail_report().and_then(|r| r.comments.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).filter(|&i| {
                let c = &rows[i];
                self.search_matches(ListId::DetailComments, &[&c.author, &c.repository, c.kind.label()])
            }).collect()
        })
    }

    fn visible_detail_issues(&self) -> Vec<usize> {
        self.detail_report().and_then(|r| r.issues.as_ref()).map_or_else(Vec::new, |rows| {
            (0..rows.len()).filter(|&i| {
                let x = &rows[i];
                let number = format!("#{}", x.number);
                self.search_matches(ListId::DetailIssues, &[&x.title, &x.repository, &number])
            }).collect()
        })
    }

    fn visible_pr_commits(&self) -> Vec<usize> {
        self.pr_report().map_or_else(Vec::new, |r| {
            (0..r.commit_list.len()).filter(|&i| {
                let c = &r.commit_list[i];
                self.search_matches(ListId::PrCommits, &[&c.sha, c.headline.as_deref().unwrap_or(""), &c.author])
            }).collect()
        })
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
            let next = self
                .selected(list)
                .saturating_add_signed(delta)
                .min(len.saturating_sub(1));
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
}
