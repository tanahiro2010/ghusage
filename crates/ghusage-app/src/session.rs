//! A working session: cache, subject and (optionally) the GitHub client.
//! The CLI and the TUI both go through this type.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ghusage_core::coverage::{self, DataKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::model::{EventType, Subject, Visibility};
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
    store: Store,
    data_dir: Option<PathBuf>,
    subject: Subject,
    subject_is_viewer: bool,
    client: Option<GitHubClient>,
    token_source: Option<TokenSource>,
    config: SessionConfig,
    /// Set when GitHub could not be reached and cached data is used instead.
    offline_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    pub login: String,
    pub is_viewer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineState {
    pub offline: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInfo {
    pub data_dir: Option<PathBuf>,
    pub file_size_bytes: Option<u64>,
    pub cached_events: Option<u64>,
    pub cached_pull_requests: Option<u64>,
    pub cached_repositories: Option<u64>,
    pub last_sync: Option<ghusage_storage::SyncRun>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiInFlight {
    pub label: String,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiActivitySnapshot {
    pub running: Vec<ApiInFlight>,
    pub limit: usize,
    pub done: u64,
}

#[derive(Clone)]
pub struct ApiActivity(Arc<ghusage_github::Activity>);

impl ApiActivity {
    pub fn snapshot(&self) -> ApiActivitySnapshot {
        let snapshot = self.0.snapshot();
        ApiActivitySnapshot {
            running: snapshot
                .running
                .into_iter()
                .map(|request| ApiInFlight {
                    label: request.label,
                    elapsed: request.elapsed,
                })
                .collect(),
            limit: snapshot.limit,
            done: snapshot.done,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiStats {
    pub requests: u64,
    pub retries: u64,
    pub total_api_time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLoadResult {
    pub pr_id: Option<String>,
    pub refreshed: bool,
    pub refresh_error: Option<String>,
    pub offline: bool,
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
pub(crate) fn open_store(cfg: &SessionConfig) -> Result<(Store, Option<PathBuf>)> {
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

/// Builds client settings from the process environment.
pub fn client_config(concurrency: usize) -> ClientConfig {
    let mut config = ClientConfig {
        concurrency,
        ..ClientConfig::default()
    };
    if let Some(url) = std::env::var("GHUSAGE_API_URL")
        .ok()
        .filter(|value| !value.is_empty())
    {
        config.api_base = url.trim_end_matches('/').to_string();
        config.graphql_url = match config.api_base.strip_suffix("/api/v3") {
            Some(host) => format!("{host}/api/graphql"),
            None => format!("{}/graphql", config.api_base),
        };
    }
    if let Some(url) = std::env::var("GHUSAGE_GRAPHQL_URL")
        .ok()
        .filter(|value| !value.is_empty())
    {
        config.graphql_url = url;
    }
    if let Some(ms) = std::env::var("GHUSAGE_BACKOFF_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        config.backoff_base = std::time::Duration::from_millis(ms);
    }
    if let Some(ms) = std::env::var("GHUSAGE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
    {
        config.request_timeout = std::time::Duration::from_millis(ms);
    }
    config
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

    pub fn identity(&self) -> SessionIdentity {
        SessionIdentity {
            login: self.subject.login.clone(),
            is_viewer: self.subject_is_viewer,
        }
    }

    pub fn offline_state(&self) -> OfflineState {
        OfflineState {
            offline: self.is_offline(),
            reason: self.offline_reason.clone(),
        }
    }

    pub fn offline_error(&self) -> Option<AppError> {
        self.offline_state()
            .reason
            .map(|reason| AppError::Other(format!("GitHub is not reachable: {reason}")))
    }

    pub fn token_source_description(&self) -> Option<String> {
        self.token_source.as_ref().map(|source| source.describe())
    }

    pub fn api_activity(&self) -> Option<ApiActivity> {
        self.client
            .as_ref()
            .map(|client| ApiActivity(client.activity()))
    }

    pub fn api_stats(&self) -> ApiStats {
        self.client.as_ref().map_or(
            ApiStats {
                requests: 0,
                retries: 0,
                total_api_time_ms: 0,
            },
            |client| {
                let stats = client.stats();
                ApiStats {
                    requests: stats.requests,
                    retries: stats.retries,
                    total_api_time_ms: stats.total_api_time_ms,
                }
            },
        )
    }

    pub async fn rate_limit(&self) -> Result<Option<ghusage_github::RateLimitInfo>> {
        let Some(client) = &self.client else {
            return Ok(None);
        };
        Ok(client.rate_limit().await.ok())
    }

    pub fn cache_info(&self) -> CacheInfo {
        let mut info = CacheInfo {
            data_dir: self.data_dir.clone(),
            file_size_bytes: None,
            cached_events: None,
            cached_pull_requests: None,
            cached_repositories: None,
            last_sync: None,
            errors: Vec::new(),
        };
        match self.store.stats() {
            Ok(stats) => {
                info.file_size_bytes = Some(stats.file_size_bytes);
                info.cached_events = Some(stats.events);
                info.cached_pull_requests = Some(stats.pull_requests);
                info.cached_repositories = Some(stats.repositories);
            }
            Err(error) => info.errors.push(error.to_string()),
        }
        match self.store.last_sync_run(&self.subject.login) {
            Ok(last_sync) => info.last_sync = last_sync,
            Err(error) => info.errors.push(error.to_string()),
        }
        info
    }

    pub async fn load_pr(&mut self, owner: &str, name: &str, number: u64) -> Result<PrLoadResult> {
        if let Some(client) = &self.client {
            let fetched = client.fetch_pull_request(owner, name, number).await;
            match fetched {
                Ok(bundle) => {
                    let pr_id = bundle.pull_request.id.clone();
                    let involved = sync::involves(&self.subject, &bundle);
                    let mut batch = sync::pr_batch(&self.subject, bundle);
                    if !involved {
                        batch.account = format!("{}{}", self.subject.login, sync::VIEWED_SUFFIX);
                        batch
                            .events
                            .retain(|event| event.event_type != EventType::CommitAuthored);
                    }
                    self.store.apply(&batch)?;
                    return Ok(PrLoadResult {
                        pr_id: Some(pr_id),
                        refreshed: true,
                        refresh_error: None,
                        offline: false,
                    });
                }
                Err(error) if error.is_auth() => return Err(error.into()),
                Err(error) => {
                    let cached = self.store.find_pr(owner, name, number)?;
                    if cached.is_none() {
                        return Err(error.into());
                    }
                    return Ok(PrLoadResult {
                        pr_id: cached,
                        refreshed: false,
                        refresh_error: Some(error.to_string()),
                        offline: false,
                    });
                }
            }
        }
        Ok(PrLoadResult {
            pr_id: self.store.find_pr(owner, name, number)?,
            refreshed: false,
            refresh_error: None,
            offline: true,
        })
    }

    pub(crate) async fn load_commit(
        &mut self,
        target: &crate::commit::CommitTarget,
    ) -> std::result::Result<crate::commit::LoadedCommit, crate::commit::CommitError> {
        let public_only = self.config.public_only;
        let cached = self
            .store
            .commit_detail(&target.repository(), &target.sha)
            .map_err(AppError::from)?;
        if let Some(detail) = cached {
            if public_only && !detail.visibility.is_public() {
                return Err(crate::commit::CommitError::NotPublic(target.repository()));
            }
            return Ok(crate::commit::LoadedCommit {
                detail,
                source: crate::commit::CommitSource::Cache,
            });
        }
        let Some(client) = &self.client else {
            return Err(crate::commit::CommitError::Offline {
                target: target.label(),
                reason: self
                    .offline_reason
                    .clone()
                    .unwrap_or_else(|| "offline mode".into()),
            });
        };
        let known = self
            .store
            .repository_by_name(&target.owner, &target.name)
            .map_err(AppError::from)?;
        let (repository, visibility) = match known {
            Some(r) => (r.full_name(), r.visibility),
            None => match client.repository(&target.owner, &target.name).await {
                Ok(r) => (r.full_name(), r.visibility),
                Err(e) if public_only => return Err(AppError::from(e).into()),
                Err(_) => (target.repository(), Visibility::Unknown),
            },
        };
        if public_only && !visibility.is_public() {
            return Err(crate::commit::CommitError::NotPublic(target.repository()));
        }
        let mut detail = client
            .fetch_commit(&target.owner, &target.name, &target.sha)
            .await
            .map_err(AppError::from)?;
        detail.repository = repository;
        detail.visibility = visibility;
        if let Err(error) = self.store.put_commit_detail(&detail) {
            tracing::warn!(error = %error, "could not store the commit in the cache");
        }
        Ok(crate::commit::LoadedCommit {
            detail,
            source: crate::commit::CommitSource::Github,
        })
    }

    pub fn mark_offline(&mut self, reason: String) {
        self.client = None;
        self.offline_reason = Some(reason);
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
