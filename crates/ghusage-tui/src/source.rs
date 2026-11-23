//! Where the UI reads its data from. The UI only ever reads the local cache
//! through this trait; network access happens in the background worker.

use std::path::PathBuf;

use ghusage_app::session::Session;
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Period;
use jiff::Timestamp;

/// The most recent synchronization recorded in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSync {
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub status: String,
    pub message: Option<String>,
}

/// Facts about the cache shown on the System screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub login: String,
    pub subject_is_viewer: bool,
    pub data_dir: Option<PathBuf>,
    pub cache_size_bytes: Option<u64>,
    pub cached_events: Option<u64>,
    pub cached_pull_requests: Option<u64>,
    pub cached_repositories: Option<u64>,
    pub last_sync: Option<LastSync>,
    pub synced_at: Option<Timestamp>,
    /// Set when the cache could not be inspected.
    pub error: Option<String>,
}

/// Read access to cached activity.
pub trait DataSource {
    /// Activity for a period.
    fn dataset(&self, period: Period) -> Result<Dataset, String>;
    /// Everything known about one pull request.
    fn pr_dataset(&self, pr_id: &str) -> Result<Dataset, String>;
    /// Cache and sync status.
    fn system_info(&self) -> SystemInfo;
}

/// An in-memory data source, used for embedding and tests.
#[derive(Debug, Clone)]
pub struct MemorySource {
    pub dataset: Dataset,
    pub system: SystemInfo,
}

impl MemorySource {
    pub fn new(dataset: Dataset) -> Self {
        let system = SystemInfo {
            login: dataset.subject.login.clone(),
            subject_is_viewer: dataset.subject_is_viewer,
            ..SystemInfo::default()
        };
        Self { dataset, system }
    }
}

impl DataSource for MemorySource {
    fn dataset(&self, _period: Period) -> Result<Dataset, String> {
        // Reports select the period themselves.
        Ok(self.dataset.clone())
    }

    fn pr_dataset(&self, _pr_id: &str) -> Result<Dataset, String> {
        Ok(self.dataset.clone())
    }

    fn system_info(&self) -> SystemInfo {
        self.system.clone()
    }
}

/// Reads from the cache of an offline [`Session`].
pub struct SessionSource {
    session: Session,
}

impl SessionSource {
    pub fn new(session: Session) -> Self {
        Self { session }
    }
}

impl DataSource for SessionSource {
    fn dataset(&self, period: Period) -> Result<Dataset, String> {
        self.session
            .dataset(Some(period))
            .map_err(|e| e.to_string())
    }

    fn pr_dataset(&self, pr_id: &str) -> Result<Dataset, String> {
        self.session.pr_dataset(pr_id).map_err(|e| e.to_string())
    }

    fn system_info(&self) -> SystemInfo {
        let s = &self.session;
        let mut info = SystemInfo {
            login: s.subject.login.clone(),
            subject_is_viewer: s.subject_is_viewer,
            data_dir: s.data_dir.clone(),
            ..SystemInfo::default()
        };
        let mut errors = Vec::new();
        match s.store.stats() {
            Ok(st) => {
                info.cache_size_bytes = Some(st.file_size_bytes);
                info.cached_events = Some(st.events);
                info.cached_pull_requests = Some(st.pull_requests);
                info.cached_repositories = Some(st.repositories);
            }
            Err(e) => errors.push(e.to_string()),
        }
        match s.store.last_sync_run(&s.subject.login) {
            Ok(run) => {
                info.last_sync = run.map(|r| LastSync {
                    started_at: r.started_at,
                    finished_at: r.finished_at,
                    status: r.status,
                    message: r.message,
                });
                info.synced_at = info.last_sync.as_ref().and_then(|r| r.finished_at);
            }
            Err(e) => errors.push(e.to_string()),
        }
        if !errors.is_empty() {
            info.error = Some(errors.join("; "));
        }
        info
    }
}
