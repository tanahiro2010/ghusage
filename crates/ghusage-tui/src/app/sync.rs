impl App {
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
