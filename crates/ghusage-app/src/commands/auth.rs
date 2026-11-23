use jiff::Timestamp;

use crate::error::Result;
use crate::session::{SessionConfig, credentials};
use ghusage_github::GitHubClient;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RateLimit {
    pub core_remaining: u64,
    pub core_limit: u64,
    pub core_reset: Timestamp,
    pub search_remaining: u64,
    pub search_limit: u64,
    pub graphql_remaining: u64,
    pub graphql_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthStatus {
    pub source: String,
    pub login: String,
    pub id: String,
    pub scopes: Option<Vec<String>>,
    pub rate_limit: Option<RateLimit>,
}

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
