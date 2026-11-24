//! Read-only checks against the real GitHub API. Ignored by default; run
//! with `GHUSAGE_LIVE_TESTS=1 cargo test -p ghusage-github --test live -- --ignored`.
//! Credentials come from `GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`.

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{EventType, PrState};
use ghusage_core::time::Period;
use ghusage_github::{
    ClientConfig, GitHubClient, IssueKind, gh_cli_token, parse_pr_url, resolve_credentials,
};

fn live_client() -> Option<GitHubClient> {
    if std::env::var("GHUSAGE_LIVE_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    let creds = resolve_credentials(None, &|k| std::env::var(k).ok(), &gh_cli_token).ok()?;
    Some(GitHubClient::new(Some(creds), ClientConfig::default()).unwrap())
}

#[tokio::test]
#[ignore = "requires GH_TOKEN and makes live GitHub API requests"]
async fn live_pull_request_of_a_public_repository() {
    let Some(c) = live_client() else { return };
    let (o, r, n) = parse_pr_url("https://github.com/cli/cli/pull/15109").unwrap();
    let b = c.fetch_pull_request(&o, &r, n).await.unwrap();
    eprintln!(
        "status={:?} notes={:?} events={}",
        b.status,
        b.notes,
        b.events.len()
    );
    assert_eq!(b.pull_request.state, PrState::Merged);
    assert!(b.events.iter().any(|e| e.event_type == EventType::PrOpened));
    assert!(b.events.iter().any(|e| e.event_type == EventType::PrMerged));
    assert!(
        b.events
            .iter()
            .any(|e| e.event_type == EventType::ReviewSubmitted)
    );
    assert_ne!(b.current_commit_shas, Vec::<String>::new());
    eprintln!("{:?}", c.stats());
}

#[tokio::test]
#[ignore = "requires GH_TOKEN and makes live GitHub API requests"]
async fn live_search_and_metadata() {
    let Some(c) = live_client() else { return };
    let viewer = c.viewer().await.unwrap();
    let end = jiff::Timestamp::now();
    let period = Period::new(end - jiff::SignedDuration::from_secs(7 * 24 * 60 * 60), end).unwrap();
    let commits = c.search_commits(&viewer.login, period, true).await.unwrap();
    eprintln!(
        "commits={} status={:?}",
        commits.items.len(),
        commits.status
    );
    assert!(matches!(
        commits.status,
        CoverageStatus::Complete | CoverageStatus::Partial
    ));
    let prs = c
        .search_issues("repo:cli/cli", IssueKind::PullRequest, period, true)
        .await
        .unwrap();
    eprintln!("prs={} status={:?}", prs.items.len(), prs.status);
    assert!(prs.items.iter().all(|p| p.kind == IssueKind::PullRequest));
    let repo = c.repository("cli", "cli").await.unwrap();
    assert!(repo.visibility.is_public());
    let missing = c
        .repository("cli", "does-not-exist-xyz-123")
        .await
        .unwrap_err();
    assert!(matches!(missing, ghusage_github::ApiError::NotFound(_)));
    let rl = c.rate_limit().await.unwrap();
    eprintln!("core remaining {}", rl.core_remaining);
}
