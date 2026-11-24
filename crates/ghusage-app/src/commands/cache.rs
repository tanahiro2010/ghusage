use std::path::PathBuf;

use ghusage_core::coverage::{self, CoverageSummary, DataKind};
use ghusage_core::time::Period;
use jiff::{SignedDuration, Timestamp};

use crate::commands::open_cache::{lock, open};
use crate::error::Result;
use crate::session::SessionConfig;

/// Summary of the most recent synchronization run.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LastSync {
    /// Time the run started.
    pub started_at: Timestamp,
    /// Time the run finished, when complete.
    pub finished_at: Option<Timestamp>,
    /// Run outcome.
    pub status: String,
    /// Run message, when present.
    pub message: Option<String>,
}

/// Cache counts, sync metadata, and recent data coverage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CacheStatus {
    /// Cache file path, when file-backed.
    pub path: Option<PathBuf>,
    /// Database and WAL size in bytes.
    pub file_size_bytes: u64,
    /// Current SQLite schema version.
    pub schema_version: u32,
    /// Account associated with the status, when known.
    pub account: Option<String>,
    /// Number of cached repositories.
    pub repositories: u64,
    /// Number of cached pull requests.
    pub pull_requests: u64,
    /// Number of cached issues.
    pub issues: u64,
    /// Number of cached activity events.
    pub events: u64,
    /// Most recent sync run for the account.
    pub last_sync: Option<LastSync>,
    /// Coverage summaries for the previous 30 days.
    pub coverage_last_30_days: Vec<CoverageSummary>,
}

/// Reads cache status and recent coverage for the selected account.
pub fn cache_status(
    cfg: &SessionConfig,
    user: Option<&str>,
    now: Timestamp,
) -> Result<CacheStatus> {
    let cache = open(cfg)?;
    let stats = cache.store.stats()?;
    let login = cache.store.meta(crate::sync::META_VIEWER_LOGIN)?;
    let account = user.map(str::to_owned).or(login);
    let last_sync = account
        .as_deref()
        .map(|account| cache.store.last_sync_run(account))
        .transpose()?
        .flatten()
        .map(|run| LastSync {
            started_at: run.started_at,
            finished_at: run.finished_at,
            status: run.status,
            message: run.message,
        });
    let period = Period::new(now - SignedDuration::from_secs(30 * 24 * 60 * 60), now)?;
    let coverage_last_30_days = account
        .as_deref()
        .map(|account| -> Result<Vec<CoverageSummary>> {
            let records = cache.store.coverage(account)?;
            Ok(DataKind::ALL
                .iter()
                .map(|kind| coverage::summarize(&records, *kind, period.start, period.end))
                .collect())
        })
        .transpose()?
        .unwrap_or_default();
    Ok(CacheStatus {
        path: cache.data_dir.map(|dir| crate::paths::db_path(&dir)),
        file_size_bytes: stats.file_size_bytes,
        schema_version: stats.schema_version,
        account,
        repositories: stats.repositories,
        pull_requests: stats.pull_requests,
        issues: stats.issues,
        events: stats.events,
        last_sync,
        coverage_last_30_days,
    })
}

/// Deletes cached GitHub data while preserving credentials.
pub fn clear_cache(cfg: &SessionConfig) -> Result<()> {
    let mut cache = open(cfg)?;
    let _lock = lock(cache.data_dir.as_ref())?;
    cache.store.clear()?;
    Ok(())
}
