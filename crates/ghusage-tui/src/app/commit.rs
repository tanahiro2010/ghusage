impl App {
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
}
