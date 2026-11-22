//! GitHub API access for ghusage: authentication, HTTP with pagination,
//! rate limits and retries, discovery via search, and normalization of
//! pull requests, issues, commits and checks into the core event model.

mod activity;
mod auth;
mod commit;
mod gql;
mod http;
mod issue;
mod line_counts;
mod pull_request;
mod queries;
mod repos;
mod search;
mod url;

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{LineCounts, PrDiff};
use ghusage_core::model::{
    ActivityEvent, Collection, Issue, PullRequest, Repository, ReviewThread,
};
use ghusage_core::time::Period;
use serde::Deserialize;
use serde_json::json;

pub use activity::{Activity, ActivitySnapshot, InFlight};
pub use auth::{AuthError, Credentials, Secret, TokenSource, gh_cli_token, resolve_credentials};
pub use line_counts::CommitRef;
pub use url::{parse_commit_ref, parse_pr_url};

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// REST base, default `https://api.github.com`.
    pub api_base: String,
    /// GraphQL endpoint, default `https://api.github.com/graphql`.
    pub graphql_url: String,
    /// Maximum concurrent requests (1..=16).
    pub concurrency: usize,
    /// Maximum attempts per request, including the first.
    pub max_attempts: u32,
    pub request_timeout: Duration,
    /// Longest single wait for a rate-limit reset before giving up.
    pub max_rate_limit_wait: Duration,
    /// Base delay for exponential backoff (tests use a tiny value).
    pub backoff_base: Duration,
    pub max_response_bytes: usize,
    pub user_agent: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com".into(),
            graphql_url: "https://api.github.com/graphql".into(),
            concurrency: 8,
            max_attempts: 5,
            request_timeout: Duration::from_secs(30),
            max_rate_limit_wait: Duration::from_secs(15 * 60),
            backoff_base: Duration::from_secs(1),
            max_response_bytes: 50 * 1024 * 1024,
            user_agent: concat!("ghusage/", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

/// Errors from the GitHub API. Messages never contain the token.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    #[error("GitHub rejected the credentials (HTTP 401)")]
    Unauthorized,
    #[error("access denied: {0}")]
    Forbidden(String),
    #[error("not found or not accessible: {0}")]
    NotFound(String),
    #[error("GitHub rate limit exceeded; resets at {reset_at:?}")]
    RateLimited { reset_at: Option<DateTime<Utc>> },
    #[error("request timed out after {attempts} attempt(s)")]
    Timeout { attempts: u32 },
    #[error("GitHub API returned HTTP {status} after {attempts} attempt(s)")]
    Http { status: u16, attempts: u32 },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected API response: {0}")]
    InvalidResponse(String),
    #[error("API response exceeded the size limit of {limit} bytes")]
    TooLarge { limit: usize },
    #[error("GraphQL error: {0}")]
    Graphql(String),
}

impl ApiError {
    /// Whether the error means credentials are invalid (exit code 3).
    pub fn is_auth(&self) -> bool {
        matches!(self, ApiError::Unauthorized)
    }
}

/// Counters for API performance reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ApiStats {
    pub requests: u64,
    pub retries: u64,
    pub rate_limit_waits: u64,
    pub total_api_time_ms: u64,
    pub bytes_received: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Viewer {
    pub id: String,
    pub login: String,
    /// OAuth scopes from `X-OAuth-Scopes`, when reported.
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UserInfo {
    pub id: String,
    pub login: String,
    pub is_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RateLimitInfo {
    pub core_remaining: u64,
    pub core_limit: u64,
    pub core_reset: DateTime<Utc>,
    pub search_remaining: u64,
    pub search_limit: u64,
    pub graphql_remaining: u64,
    pub graphql_limit: u64,
}

/// Items plus how completely they were fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched<T> {
    pub items: T,
    pub status: CoverageStatus,
    /// Explanation when `status` is not complete.
    pub reason: Option<String>,
}

/// One commit authored by the searched user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub repository: Repository,
    /// A `CommitAuthored` event (`occurred_at` = committer date).
    pub event: ActivityEvent,
}

/// A repository with the time of its last push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushedRepository {
    pub repository: Repository,
    pub pushed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    PullRequest,
    Issue,
}

/// A pull request or issue found by search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub node_id: String,
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub kind: IssueKind,
    pub updated_at: DateTime<Utc>,
}

/// All data of one pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrBundle {
    pub repository: Repository,
    pub pull_request: PullRequest,
    pub events: Vec<ActivityEvent>,
    pub threads: Vec<ReviewThread>,
    /// SHAs currently in the pull request.
    pub current_commit_shas: Vec<String>,
    /// Lines added and deleted by the pull request's commits, by SHA.
    pub commit_stats: Vec<(String, LineCounts)>,
    /// The pull request's base-to-head diff when fetched (a snapshot).
    pub diff: Option<PrDiff>,
    /// Parents of the commit a merged pull request was merged with: 2 for a
    /// merge commit (its commits reach the base branch unchanged), 1 for a
    /// squash or rebase merge (its changes reach it as new commits).
    pub merge_commit_parents: Option<u64>,
    /// Collections fetched completely (all pages).
    pub complete_collections: Vec<Collection>,
    /// Overall coverage of this pull request's internal activity.
    pub status: CoverageStatus,
    pub notes: Vec<String>,
}

/// All data of one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueBundle {
    pub repository: Repository,
    pub issue: Issue,
    /// `IssueOpened`, `IssueClosed`, `IssueReopened` events.
    pub events: Vec<ActivityEvent>,
    pub status: CoverageStatus,
}

pub struct GitHubClient {
    transport: http::Transport,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestUserInfo {
    login: Option<String>,
    node_id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RateResource {
    limit: Option<u64>,
    remaining: Option<u64>,
    reset: Option<i64>,
}

impl GitHubClient {
    /// `credentials` is `None` only for unauthenticated public access.
    /// `config.concurrency` is clamped to 1..=16 and `config.max_attempts`
    /// to at least 1.
    pub fn new(credentials: Option<Credentials>, config: ClientConfig) -> Result<Self, ApiError> {
        Ok(Self {
            transport: http::Transport::new(credentials, config)?,
        })
    }

    pub fn stats(&self) -> ApiStats {
        self.transport.stats()
    }

    /// The requests in flight, shared so a progress display can watch them.
    pub fn activity(&self) -> std::sync::Arc<Activity> {
        std::sync::Arc::clone(&self.transport.activity)
    }

    /// The authenticated user (`GET /user`).
    pub async fn viewer(&self) -> Result<Viewer, ApiError> {
        let url = self.transport.rest_url(&["user"], &[]);
        let (value, headers) = self.transport.get_json(&url).await?;
        let user: RestUserInfo = serde_json::from_value(value)
            .map_err(|e| ApiError::InvalidResponse(format!("/user: {e}")))?;
        let scopes = headers
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            });
        Ok(Viewer {
            id: user
                .node_id
                .ok_or_else(|| ApiError::InvalidResponse("/user is missing `node_id`".into()))?,
            login: user
                .login
                .ok_or_else(|| ApiError::InvalidResponse("/user is missing `login`".into()))?,
            scopes,
        })
    }

    /// A user by login (`GET /users/{login}`).
    pub async fn user(&self, login: &str) -> Result<UserInfo, ApiError> {
        let url = self.transport.rest_url(&["users", login], &[]);
        let (value, _) = self.transport.get_json(&url).await?;
        let user: RestUserInfo = serde_json::from_value(value)
            .map_err(|e| ApiError::InvalidResponse(format!("/users: {e}")))?;
        let login = user
            .login
            .ok_or_else(|| ApiError::InvalidResponse("user is missing `login`".into()))?;
        Ok(UserInfo {
            id: user
                .node_id
                .ok_or_else(|| ApiError::InvalidResponse("user is missing `node_id`".into()))?,
            is_bot: user.kind.as_deref() == Some("Bot") || login.ends_with("[bot]"),
            login,
        })
    }

    pub async fn rate_limit(&self) -> Result<RateLimitInfo, ApiError> {
        let url = self.transport.rest_url(&["rate_limit"], &[]);
        let (value, _) = self.transport.get_json(&url).await?;
        let resources = value.get("resources").ok_or_else(|| {
            ApiError::InvalidResponse("/rate_limit is missing `resources`".into())
        })?;
        let get = |name: &str| -> RateResource { gql::from_value(resources.get(name)) };
        let core = get("core");
        let search = get("search");
        let graphql = get("graphql");
        Ok(RateLimitInfo {
            core_remaining: core.remaining.unwrap_or(0),
            core_limit: core.limit.unwrap_or(0),
            core_reset: core
                .reset
                .and_then(|r| Utc.timestamp_opt(r, 0).single())
                .unwrap_or_else(Utc::now),
            search_remaining: search.remaining.unwrap_or(0),
            search_limit: search.limit.unwrap_or(0),
            graphql_remaining: graphql.remaining.unwrap_or(0),
            graphql_limit: graphql.limit.unwrap_or(0),
        })
    }

    /// Commits authored by `author_login` with committer date in `period`
    /// via `GET /search/commits`. Splits the period when a query exceeds
    /// 1000 results; reports `Partial` when splitting cannot get under the
    /// limit or GitHub reports `incomplete_results`.
    pub async fn search_commits(
        &self,
        author_login: &str,
        period: Period,
        public_only: bool,
    ) -> Result<Fetched<Vec<CommitRecord>>, ApiError> {
        search::commits(&self.transport, author_login, period, public_only).await
    }

    /// Repositories pushed to at or after `pushed_since`, newest push first.
    /// With `user` = `None` these are the authenticated user's repositories
    /// (`GET /user/repos`: owned, collaborator and organization member);
    /// otherwise the repositories `user` owns (`GET /users/{login}/repos`).
    /// Paging stops at the first repository last pushed before
    /// `pushed_since`.
    pub async fn pushed_repositories(
        &self,
        user: Option<&str>,
        pushed_since: DateTime<Utc>,
    ) -> Result<Vec<PushedRepository>, ApiError> {
        repos::pushed_repositories(&self.transport, user, pushed_since).await
    }

    /// Commits on the default branch of `repository` authored by
    /// `author_login` with committer date in `period`
    /// (`GET /repos/{owner}/{name}/commits`), normalized like commit search
    /// results so both deduplicate by repository and SHA. An empty
    /// repository yields no commits; other failures (e.g. 403/404) are
    /// errors for the caller to report.
    pub async fn default_branch_commits(
        &self,
        repository: &Repository,
        author_login: &str,
        period: Period,
    ) -> Result<Fetched<Vec<CommitRecord>>, ApiError> {
        repos::default_branch_commits(&self.transport, repository, author_login, period).await
    }

    /// Pull requests or issues matching `qualifiers` (e.g.
    /// `involves:octocat`) updated in `period` via `GET /search/issues`,
    /// with the same splitting and completeness rules.
    pub async fn search_issues(
        &self,
        qualifiers: &str,
        kind: IssueKind,
        period: Period,
        public_only: bool,
    ) -> Result<Fetched<Vec<IssueRef>>, ApiError> {
        search::issues(&self.transport, qualifiers, kind, period, public_only).await
    }

    /// Fetches a pull request with every page of commits, reviews, review
    /// threads and their comments, conversation comments, timeline items
    /// and check runs of the head commit (GraphQL), plus the head commit's
    /// commit statuses when it has any (REST). Check runs that do not fit in
    /// the pull request query are fetched over REST.
    pub async fn fetch_pull_request(
        &self,
        owner: &str,
        name: &str,
        number: u64,
    ) -> Result<PrBundle, ApiError> {
        pull_request::fetch(&self.transport, owner, name, number).await
    }

    /// Fetches an issue with its open/close/reopen history (GraphQL).
    /// Fails with `InvalidResponse` when the number belongs to a pull request.
    pub async fn fetch_issue(
        &self,
        owner: &str,
        name: &str,
        number: u64,
    ) -> Result<IssueBundle, ApiError> {
        issue::fetch(&self.transport, owner, name, number).await
    }

    /// Fetches one commit with its message, people, parents, totals and
    /// every page of its changed files with their diffs
    /// (`GET /repos/{owner}/{name}/commits/{sha}`). `sha` may be abbreviated.
    /// The repository's visibility is not part of the response and is left
    /// `Unknown`. A missing commit is `NotFound`.
    pub async fn fetch_commit(
        &self,
        owner: &str,
        name: &str,
        sha: &str,
    ) -> Result<ghusage_core::commit::CommitDetail, ApiError> {
        commit::fetch(&self.transport, owner, name, sha).await
    }

    /// Lines added and deleted by each commit, fetched in batches of up to
    /// 100 commits per GraphQL query. `None` for a commit GitHub did not
    /// return.
    pub async fn commit_line_counts(
        &self,
        commits: &[CommitRef],
    ) -> Result<Vec<(CommitRef, Option<LineCounts>)>, ApiError> {
        line_counts::fetch(&self.transport, commits).await
    }

    /// Repository metadata by owner and name.
    pub async fn repository(&self, owner: &str, name: &str) -> Result<Repository, ApiError> {
        let what = format!("repository {owner}/{name}");
        let resp = self
            .transport
            .graphql(
                &queries::repository(),
                json!({ "owner": owner, "name": name }),
            )
            .await?;
        let Some(repo) = resp
            .data
            .as_ref()
            .and_then(|d| d.get("repository"))
            .filter(|v| !v.is_null())
        else {
            return Err(gql::failure(&resp.errors, &what));
        };
        gql::from_value::<gql::GqlRepository>(Some(repo)).to_repository()
    }
}
