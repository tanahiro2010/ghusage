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
    /// Time the synchronization began.
    pub started_at: Timestamp,
    /// Time the synchronization finished, when complete.
    pub finished_at: Option<Timestamp>,
    /// Synchronization result.
    pub status: String,
    /// Additional run message, when present.
    pub message: Option<String>,
}

/// Facts about the cache shown on the System screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemInfo {
    /// GitHub login being analyzed.
    pub login: String,
    /// Whether the analyzed account is the authenticated viewer.
    pub subject_is_viewer: bool,
    /// Cache directory, when file-backed.
    pub data_dir: Option<PathBuf>,
    /// Cache database size in bytes.
    pub cache_size_bytes: Option<u64>,
    /// Number of cached events.
    pub cached_events: Option<u64>,
    /// Number of cached pull requests.
    pub cached_pull_requests: Option<u64>,
    /// Number of cached repositories.
    pub cached_repositories: Option<u64>,
    /// Most recent synchronization run.
    pub last_sync: Option<LastSync>,
    /// Completion time of the latest sync, when known.
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
    /// Dataset returned by every query.
    pub dataset: Dataset,
    /// System information returned by the source.
    pub system: SystemInfo,
}

impl MemorySource {
    /// Creates a memory data source for a dataset.
    #[must_use]
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
#[derive(Debug)]
pub struct SessionSource {
    session: Session,
}

impl SessionSource {
    /// Creates a cache-backed data source from a session.
    pub const fn new(session: Session) -> Self {
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
        let identity = self.session.identity();
        let cache = self.session.cache_info();
        let mut info = SystemInfo {
            login: identity.login,
            subject_is_viewer: identity.is_viewer,
            data_dir: cache.data_dir,
            ..SystemInfo::default()
        };
        info.cache_size_bytes = cache.file_size_bytes;
        info.cached_events = cache.cached_events;
        info.cached_pull_requests = cache.cached_pull_requests;
        info.cached_repositories = cache.cached_repositories;
        info.last_sync = cache.last_sync.map(|r| LastSync {
            started_at: r.started_at,
            finished_at: r.finished_at,
            status: r.status,
            message: r.message,
        });
        info.synced_at = info.last_sync.as_ref().and_then(|r| r.finished_at);
        if !cache.errors.is_empty() {
            info.error = Some(cache.errors.join("; "));
        }
        info
    }
}
