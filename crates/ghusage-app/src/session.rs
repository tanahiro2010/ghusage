//! A working session: cache, subject and (optionally) the GitHub client.
//! The CLI and the TUI both go through this type.

use std::path::{Path, PathBuf};

use ghusage_core::coverage::{self, DataKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::model::Subject;
use ghusage_core::time::Period;
use ghusage_github::{ClientConfig, Credentials, GitHubClient, TokenSource};
use ghusage_storage::{Store, SyncLock};
use jiff::Timestamp;

use crate::error::{AppError, Result};
use crate::paths;
use crate::sync::{self, SyncOptions, SyncProgress, SyncReport};

#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    pub data_dir: Option<PathBuf>,
    /// Use a temporary in-memory cache.
    pub no_cache: bool,
    /// Never contact GitHub.
    pub offline: bool,
    /// Analyze this user instead of the authenticated user.
    pub user: Option<String>,
    pub token_file: Option<PathBuf>,
    pub public_only: bool,
    pub client: ClientConfig,
}

pub struct Session {
    pub store: Store,
    pub data_dir: Option<PathBuf>,
    pub subject: Subject,
    pub subject_is_viewer: bool,
    pub client: Option<GitHubClient>,
    pub token_source: Option<TokenSource>,
    pub config: SessionConfig,
    /// Set when GitHub could not be reached and cached data is used instead.
    pub offline_reason: Option<String>,
}

/// Resolves credentials from the environment and `gh`.
pub fn credentials(token_file: Option<&Path>) -> Result<Credentials> {
    Ok(ghusage_github::resolve_credentials(
        token_file,
        &|k| std::env::var(k).ok(),
        &ghusage_github::gh_cli_token,
    )?)
}

/// Opens the cache database for a configuration.
pub fn open_store(cfg: &SessionConfig) -> Result<(Store, Option<PathBuf>)> {
    if cfg.no_cache {
        return Ok((Store::open_in_memory()?, None));
    }
    let dir = paths::data_dir(cfg.data_dir.clone()).ok_or_else(|| {
        AppError::Other("cannot determine a data directory; use --data-dir".into())
    })?;
    ghusage_storage::create_private_dir(&dir).map_err(|e| match e {
        ghusage_storage::StorageError::Io(e) => {
            AppError::Storage(ghusage_storage::StorageError::Io(std::io::Error::new(
                e.kind(),
                format!("cannot create data directory {}: {e}", dir.display()),
            )))
        }
        other => AppError::Storage(other),
    })?;
    let path = paths::db_path(&dir);
    let store = match Store::open(&path) {
        Ok(s) => s,
        // Without write access the cache can still be read; any write
        // (such as a sync) then fails with a cache error.
        Err(e) if e.is_read_only() && path.is_file() => {
            let s = Store::open_read_only(&path).map_err(|_| e)?;
            tracing::warn!(path = %path.display(), "cache is not writable; opened read-only");
            s
        }
        Err(e) => return Err(e.into()),
    };
    Ok((store, Some(dir)))
}

impl Session {
    pub async fn open(cfg: SessionConfig) -> Result<Self> {
        let (mut store, data_dir) = open_store(&cfg)?;
        if cfg.offline {
            let (subject, viewer) = sync::cached_subject(&store, cfg.user.as_deref())?;
            return Ok(Self {
                store,
                data_dir,
                subject,
                subject_is_viewer: viewer,
                client: None,
                token_source: None,
                config: cfg,
                offline_reason: None,
            });
        }
        let creds = credentials(cfg.token_file.as_deref())?;
        let source = creds.source.clone();
        let client = GitHubClient::new(Some(creds), cfg.client.clone())?;
        let (subject, viewer) =
            match sync::resolve_subject(&client, &mut store, cfg.user.as_deref()).await {
                Ok(s) => s,
                // Without a connection, fall back to cached data. Rejected
                // credentials are still an error.
                Err(AppError::Api(e)) if is_connectivity(&e) => {
                    let (subject, viewer) = sync::cached_subject(&store, cfg.user.as_deref())
                        .map_err(|_| AppError::Api(e.clone()))?;
                    return Ok(Self {
                        store,
                        data_dir,
                        subject,
                        subject_is_viewer: viewer,
                        client: None,
                        token_source: Some(source),
                        config: cfg,
                        offline_reason: Some(e.to_string()),
                    });
                }
                Err(e) => return Err(e),
            };
        Ok(Self {
            store,
            data_dir,
            subject,
            subject_is_viewer: viewer,
            client: Some(client),
            token_source: Some(source),
            config: cfg,
            offline_reason: None,
        })
    }

    pub fn is_offline(&self) -> bool {
        self.client.is_none()
    }

    /// Coverage records that apply to this session's mode.
    pub fn coverage(&self) -> Result<Vec<coverage::CoverageRecord>> {
        Ok(sync::applicable_coverage(
            self.store.coverage(&self.subject.login)?,
            self.config.public_only,
        ))
    }

    /// Whether any data kind has gaps in `period`.
    pub fn needs_sync(&self, period: Period) -> Result<bool> {
        let records = self.coverage()?;
        Ok(DataKind::ALL.iter().any(|k| {
            !coverage::summarize(&records, *k, period.start, period.end)
                .status
                .is_complete()
        }))
    }

    /// End of the longest leading part of `period` that every data kind has
    /// completely synced, or `None` when even its start is missing.
    pub fn synced_until(&self, period: Period) -> Result<Option<Timestamp>> {
        let records = self.coverage()?;
        let mut until = period.end;
        for k in DataKind::ALL {
            let summary = coverage::summarize(&records, k, period.start, period.end);
            if let Some(gap) = summary.gaps().map(|g| g.start).min() {
                until = until.min(gap);
            }
        }
        Ok((until > period.start).then_some(until))
    }

    /// Syncs `period` (only missing ranges unless `full`).
    pub async fn sync(
        &mut self,
        period: Period,
        full: bool,
        concurrency: usize,
        progress: &dyn Fn(SyncProgress),
    ) -> Result<SyncReport> {
        let Some(client) = &self.client else {
            return Err(AppError::Other("cannot sync in offline mode".into()));
        };
        let _lock = match &self.data_dir {
            Some(dir) => Some(SyncLock::acquire(dir)?),
            None => None,
        };
        let mut opts = SyncOptions::new(period, Timestamp::now());
        opts.full = full;
        opts.public_only = self.config.public_only;
        opts.concurrency = concurrency;
        opts.subject_is_viewer = self.subject_is_viewer;
        sync::run(client, &mut self.store, &self.subject, &opts, progress).await
    }

    /// Fetches the line counts still missing for the subject's commits in
    /// `period` (see [`sync::fetch_line_counts`]).
    pub async fn fetch_line_counts(
        &mut self,
        period: Period,
        progress: &dyn Fn(SyncProgress),
    ) -> Result<sync::LineCountSync> {
        let Some(client) = &self.client else {
            return Err(AppError::Other("cannot sync in offline mode".into()));
        };
        let _lock = match &self.data_dir {
            Some(dir) => Some(SyncLock::acquire(dir)?),
            None => None,
        };
        sync::fetch_line_counts(
            client,
            &mut self.store,
            &self.subject,
            self.subject_is_viewer,
            period,
            self.config.public_only,
            progress,
        )
        .await
    }

    /// Loads the dataset for a period (or everything).
    pub fn dataset(&self, period: Option<Period>) -> Result<Dataset> {
        let mut ds = self
            .store
            .load_dataset(&self.subject, self.subject_is_viewer, period)?;
        ds.coverage =
            sync::applicable_coverage(std::mem::take(&mut ds.coverage), self.config.public_only);
        if self.config.public_only {
            restrict_public(&mut ds);
        }
        Ok(ds)
    }

    pub fn pr_dataset(&self, pr_id: &str) -> Result<Dataset> {
        let mut ds = self
            .store
            .load_pr_dataset(&self.subject, self.subject_is_viewer, pr_id)?;
        if self.config.public_only {
            restrict_public(&mut ds);
        }
        Ok(ds)
    }

    pub fn last_synced_at(&self) -> Result<Option<Timestamp>> {
        Ok(self
            .store
            .last_sync_run(&self.subject.login)?
            .and_then(|r| r.finished_at))
    }
}

/// Errors meaning GitHub could not be reached at all.
fn is_connectivity(e: &ghusage_github::ApiError) -> bool {
    use ghusage_github::ApiError;
    matches!(
        e,
        ApiError::Network(_) | ApiError::Timeout { .. } | ApiError::Http { status: 500.., .. }
    )
}

/// Drops everything from non-public repositories.
pub fn restrict_public(ds: &mut Dataset) {
    let public: std::collections::HashSet<String> = ds
        .repositories
        .values()
        .filter(|r| r.visibility.is_public())
        .map(|r| r.id.clone())
        .collect();
    ds.repositories.retain(|id, _| public.contains(id));
    ds.pull_requests
        .retain(|_, p| public.contains(&p.repository_id));
    ds.issues.retain(|_, i| public.contains(&i.repository_id));
    let prs: std::collections::HashSet<String> = ds.pull_requests.keys().cloned().collect();
    ds.threads.retain(|_, t| prs.contains(&t.pr_id));
    ds.events.retain(|e| public.contains(&e.repository_id));
}
