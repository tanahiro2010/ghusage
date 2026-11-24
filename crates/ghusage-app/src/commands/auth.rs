use jiff::Timestamp;

use crate::error::Result;
use crate::session::{SessionConfig, credentials};
use ghusage_github::GitHubClient;

/// Rate limit data included in authentication status output.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RateLimit {
    /// Remaining core REST requests.
    pub core_remaining: u64,
    /// Core REST request limit.
    pub core_limit: u64,
    /// Core REST reset time.
    pub core_reset: Timestamp,
    /// Remaining search requests.
    pub search_remaining: u64,
    /// Search request limit.
    pub search_limit: u64,
    /// Remaining GraphQL points.
    pub graphql_remaining: u64,
    /// GraphQL point limit.
    pub graphql_limit: u64,
}

/// Authenticated user information and optional rate limit details.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthStatus {
    /// Safe description of the credential source.
    pub source: String,
    /// Authenticated GitHub login.
    pub login: String,
    /// Authenticated GitHub node ID.
    pub id: String,
    /// OAuth scopes reported by GitHub, when available.
    pub scopes: Option<Vec<String>>,
    /// Current rate limit state, when fetched successfully.
    pub rate_limit: Option<RateLimit>,
}

/// Resolves credentials and fetches account authentication status.
pub async fn auth_status(cfg: &SessionConfig) -> Result<AuthStatus> {
    let creds = credentials(cfg.token_file.as_deref())?;
    let source = creds.source.describe();
    let client = GitHubClient::new(Some(creds), cfg.client.clone())?;
    let viewer = client.viewer().await?;
    let rate_limit = client.rate_limit().await.ok().map(|r| RateLimit {
        core_remaining: r.core_remaining,
        core_limit: r.core_limit,
        core_reset: r.core_reset,
        search_remaining: r.search_remaining,
        search_limit: r.search_limit,
        graphql_remaining: r.graphql_remaining,
        graphql_limit: r.graphql_limit,
    });
    Ok(AuthStatus {
        source,
        login: viewer.login,
        id: viewer.id,
        scopes: viewer.scopes,
        rate_limit,
    })
}
