use std::path::PathBuf;

use ghusage_core::coverage::{self, CoverageSummary, DataKind};
use ghusage_core::time::Period;
use jiff::{SignedDuration, Timestamp};

use crate::commands::open_cache::{lock, open};
use crate::error::Result;
use crate::session::SessionConfig;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LastSync {
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CacheStatus {
    pub path: Option<PathBuf>,
    pub file_size_bytes: u64,
    pub schema_version: u32,
    pub account: Option<String>,
    pub repositories: u64,
    pub pull_requests: u64,
    pub issues: u64,
    pub events: u64,
    pub last_sync: Option<LastSync>,
    pub coverage_last_30_days: Vec<CoverageSummary>,
}

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

pub fn clear_cache(cfg: &SessionConfig) -> Result<()> {
    let mut cache = open(cfg)?;
    let _lock = lock(cache.data_dir.as_ref())?;
    cache.store.clear()?;
    Ok(())
}
