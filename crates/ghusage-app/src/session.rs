//! A working session: cache, subject and (optionally) the GitHub client.
//! The CLI and the TUI both go through this type.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::Either;
use ghusage_core::coverage::{self, DataKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::model::{Subject, Visibility};
use ghusage_core::time::Period;
use ghusage_github::{ApiStats, ClientConfig, Credentials, GitHubClient, TokenSource};
use ghusage_storage::{Store, SyncLock};
use jiff::Timestamp;

use crate::error::{AppError, Result};
use crate::paths;
use crate::sync::{self, SyncOptions, SyncProgress, SyncReport};

/// Settings used to create an application session.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// Directory containing the cache database.
    pub data_dir: Option<PathBuf>,
    /// Use a temporary in-memory cache.
    pub no_cache: bool,
    /// Never contact GitHub.
    pub offline: bool,
    /// Analyze this user instead of the authenticated user.
    pub user: Option<String>,
    /// Path to an explicit GitHub token file.
    pub token_file: Option<PathBuf>,
    /// Restrict report data to public repositories.
    pub public_only: bool,
    /// GitHub API transport settings.
    pub client: ClientConfig,
}

/// Active application session containing cache, identity, and API access.
#[derive(Debug)]
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
    /// The subject is the viewer remembered from the last online run and
    /// has not been checked against `GET /user` yet (see
    /// [`Session::open_deferred`]).
    unconfirmed: bool,
}

/// Account identity associated with a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    /// GitHub login being analyzed.
    pub login: String,
    /// Whether the account is the authenticated viewer.
    pub is_viewer: bool,
}

/// Current offline state and the reason for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineState {
    /// Whether remote API access is disabled or unavailable.
    pub offline: bool,
    /// Reason the session is offline, when known.
    pub reason: Option<String>,
}

/// Summary of data and errors associated with the local cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInfo {
    /// Cache directory, when file-backed.
    pub data_dir: Option<PathBuf>,
    /// Cache database size in bytes, when available.
    pub file_size_bytes: Option<u64>,
    /// Number of cached activity events, when available.
    pub cached_events: Option<u64>,
    /// Number of cached pull requests, when available.
    pub cached_pull_requests: Option<u64>,
    /// Number of cached repositories, when available.
    pub cached_repositories: Option<u64>,
    /// Most recent synchronization record, when available.
    pub last_sync: Option<ghusage_storage::SyncRun>,
    /// Cache inspection errors.
    pub errors: Vec<String>,
}

/// One GitHub API request currently in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiInFlight {
    /// Safe request label for progress display.
    pub label: String,
    /// Elapsed time since the request started.
    pub elapsed: Duration,
}

/// Snapshot of current and completed GitHub API requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiActivitySnapshot {
    /// Requests currently running.
    pub running: Vec<ApiInFlight>,
    /// Maximum concurrent requests.
    pub limit: usize,
    /// Number of completed attempts.
    pub done: u64,
}

/// Live API activity handle for progress reporting.
#[derive(Clone)]
pub struct ApiActivity(Arc<ghusage_github::Activity>);

impl std::fmt::Debug for ApiActivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ApiActivity")
            .field(&self.snapshot())
            .finish()
    }
}

impl ApiActivity {
    /// Returns the latest request activity snapshot.
    #[must_use]
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

/// Result of loading or refreshing a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrLoadResult {
    /// Pull request ID when found.
    pub pr_id: Option<String>,
    /// Whether the pull request was refreshed from GitHub.
    pub refreshed: bool,
    /// Refresh error, when cached details were used instead.
    pub refresh_error: Option<String>,
    /// Whether the result came from offline cache access.
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
#[must_use]
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
        config.backoff_base = Duration::from_millis(ms);
    }
    if let Some(ms) = std::env::var("GHUSAGE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
    {
        config.request_timeout = Duration::from_millis(ms);
    }
    config
}

impl Session {
    /// Opens a session and resolves its subject and cache.
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
                unconfirmed: false,
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
                        unconfirmed: false,
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
            unconfirmed: false,
        })
    }

    /// Opens a session like [`Session::open`], but when the viewer is
    /// remembered from an earlier online run (and neither `--user` nor
    /// offline mode is given) it does not wait for `GET /user`: the session
    /// starts with the remembered viewer and checks it later, at the same
    /// time as the first requests of [`Session::sync`] or in
    /// [`Session::confirm_subject`]. Nothing is written or shown for the
    /// remembered viewer before the check confirms it.
    pub async fn open_deferred(cfg: SessionConfig) -> Result<Self> {
        if cfg.offline || cfg.user.is_some() {
            return Self::open(cfg).await;
        }
        let (store, data_dir) = open_store(&cfg)?;
        let cached = match sync::cached_subject(&store, None) {
            Ok((subject, true)) if subject.id.is_some() => subject,
            _ => {
                drop(store);
                return Self::open(cfg).await;
            }
        };
        let creds = credentials(cfg.token_file.as_deref())?;
        let source = creds.source.clone();
        let client = GitHubClient::new(Some(creds), cfg.client.clone())?;
        Ok(Self {
            store,
            data_dir,
            subject: cached,
            subject_is_viewer: true,
            client: Some(client),
            token_source: Some(source),
            config: cfg,
            offline_reason: None,
            unconfirmed: true,
        })
    }

    /// Checks a remembered subject against `GET /user` (see
    /// [`Session::open_deferred`]); does nothing once it was checked.
    /// Switches to the authenticated account when the token now belongs to
    /// another one, and to cached data when GitHub cannot be reached.
    pub async fn confirm_subject(&mut self) -> Result<()> {
        if !self.unconfirmed {
            return Ok(());
        }
        let Some(client) = &self.client else {
            self.unconfirmed = false;
            return Ok(());
        };
        let viewer = client.viewer().await;
        self.apply_viewer(viewer)
    }

    /// Takes the result of `GET /user` for an unconfirmed subject.
    fn apply_viewer(
        &mut self,
        viewer: std::result::Result<ghusage_github::Viewer, ghusage_github::ApiError>,
    ) -> Result<()> {
        match viewer {
            Ok(v) => {
                if !sync::is_viewer(&self.subject, &v) {
                    self.store.set_meta(sync::META_VIEWER_LOGIN, &v.login)?;
                    self.store.set_meta(sync::META_VIEWER_ID, &v.id)?;
                    self.subject = Subject {
                        id: Some(v.id),
                        login: v.login,
                    };
                }
                self.unconfirmed = false;
                Ok(())
            }
            Err(e) if is_connectivity(&e) => {
                self.unconfirmed = false;
                self.mark_offline(e.to_string());
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Whether the subject still has to be checked against `GET /user`
    /// (see [`Session::open_deferred`]).
    pub const fn is_unconfirmed(&self) -> bool {
        self.unconfirmed
    }

    /// Whether the session can contact GitHub.
    pub const fn is_offline(&self) -> bool {
        self.client.is_none()
    }

    /// Returns the account identity selected for this session.
    pub fn identity(&self) -> SessionIdentity {
        SessionIdentity {
            login: self.subject.login.clone(),
            is_viewer: self.subject_is_viewer,
        }
    }

    /// Returns the current offline state and any connection failure reason.
    pub fn offline_state(&self) -> OfflineState {
        OfflineState {
            offline: self.is_offline(),
            reason: self.offline_reason.clone(),
        }
    }

    /// Converts an offline reason into an application error, when offline.
    pub fn offline_error(&self) -> Option<AppError> {
        self.offline_state()
            .reason
            .map(|reason| AppError::Other(format!("GitHub is not reachable: {reason}")))
    }

    /// Returns a safe description of the credential source.
    pub fn token_source_description(&self) -> Option<String> {
        self.token_source.as_ref().map(TokenSource::describe)
    }

    /// Returns a live handle for observing API request activity.
    pub fn api_activity(&self) -> Option<ApiActivity> {
        self.client
            .as_ref()
            .map(|client| ApiActivity(client.activity()))
    }

    /// Returns cumulative API request statistics.
    pub fn api_stats(&self) -> ApiStats {
        self.client
            .as_ref()
            .map_or_else(ApiStats::default, GitHubClient::stats)
    }

    /// Fetches current GitHub API rate-limit information when online.
    pub async fn rate_limit(&self) -> Result<Option<ghusage_github::RateLimitInfo>> {
        let Some(client) = &self.client else {
            return Ok(None);
        };
        Ok(client.rate_limit().await.ok())
    }

    /// Returns local cache size, record counts, and the latest sync state.
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

    /// Loads a pull request from cache and refreshes it when online.
    pub async fn load_pr(&mut self, owner: &str, name: &str, number: u64) -> Result<PrLoadResult> {
        if let Some(client) = &self.client {
            let fetched = client.fetch_pull_request(owner, name, number).await;
            match fetched {
                Ok(bundle) => {
                    let pr_id = bundle.pull_request.id.clone();
                    let involved = sync::involves(&self.subject, &bundle);
                    let mut batch = sync::pr_batch(&self.subject, bundle);
                    if !involved {
                        batch.viewed_pull_requests.push(pr_id.clone());
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

    /// Disables API access after a connection failure and records its reason.
    pub fn mark_offline(&mut self, reason: String) {
        self.client = None;
        self.offline_reason = Some(reason);
    }

    /// Number of finished syncs (success or partial) of the subject, the
    /// one just run included.
    pub fn completed_syncs(&self) -> Result<u64> {
        Ok(self.store.completed_sync_runs(&self.subject.login)?)
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
        if self.unconfirmed {
            // Check the remembered viewer while the sync starts. The sync
            // writes nothing before the check confirms it; when the check
            // says otherwise, the sync is dropped at once, which cancels its
            // requests in flight, and runs again for the right account.
            let check: sync::ViewerCheck<'_> = client.viewer().boxed_local().shared();
            let run = Box::pin(sync::run(
                client,
                &mut self.store,
                &self.subject,
                &opts,
                progress,
                Some(check.clone()),
            ));
            let viewer = match futures::future::select(check.clone(), run).await {
                Either::Left((viewer, run)) => {
                    if viewer
                        .as_ref()
                        .is_ok_and(|v| sync::is_viewer(&self.subject, v))
                    {
                        let report = run.await;
                        self.unconfirmed = false;
                        return report;
                    }
                    drop(run);
                    viewer
                }
                Either::Right((report, _)) => {
                    let viewer = check.clone().await;
                    if viewer
                        .as_ref()
                        .is_ok_and(|v| sync::is_viewer(&self.subject, v))
                    {
                        self.unconfirmed = false;
                        return report;
                    }
                    viewer
                }
            };
            drop(check);
            self.apply_viewer(viewer)?;
            if let Some(error) = self.offline_error() {
                return Err(error);
            }
            opts.subject_is_viewer = self.subject_is_viewer;
        }
        let Some(client) = &self.client else {
            return Err(AppError::Other("cannot sync in offline mode".into()));
        };
        sync::run(
            client,
            &mut self.store,
            &self.subject,
            &opts,
            progress,
            None,
        )
        .await
    }

    /// Fetches the line counts still missing for the subject's commits in
    /// `period` (see [`sync::fetch_line_counts`]).
    pub async fn fetch_line_counts(
        &mut self,
        period: Period,
        progress: &dyn Fn(SyncProgress),
    ) -> Result<sync::LineCountSync> {
        self.confirm_subject().await?;
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

    /// Loads all cached details for one pull request.
    pub fn pr_dataset(&self, pr_id: &str) -> Result<Dataset> {
        let mut ds = self
            .store
            .load_pr_dataset(&self.subject, self.subject_is_viewer, pr_id)?;
        if self.config.public_only {
            restrict_public(&mut ds);
        }
        Ok(ds)
    }

    /// Returns when the latest sync run finished, if any.
    pub fn last_synced_at(&self) -> Result<Option<Timestamp>> {
        Ok(self
            .store
            .last_sync_run(&self.subject.login)?
            .and_then(|r| r.finished_at))
    }
}

/// Errors meaning GitHub could not be reached at all.
const fn is_connectivity(e: &ghusage_github::ApiError) -> bool {
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
