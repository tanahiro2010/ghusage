//! Generated pagination scenarios: whenever any page of a paginated fetch
//! fails, the result is an error or is reported as not complete; only when
//! every page succeeds is it complete, with every item.

mod common;

use std::collections::HashSet;

use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{Collection, EventType};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_partial_json, body_string_contains, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn config() -> Config {
    Config {
        cases: 32,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/pagination_properties.txt",
        ))),
        ..Config::default()
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// Items on every page of the generated fetches.
const PER_PAGE: usize = 3;

/// How the failing page fails.
#[derive(Debug, Clone, Copy)]
enum Failure {
    /// HTTP 500 on every attempt.
    ServerError,
    /// A 200 response whose body is cut off mid-JSON.
    MalformedJson,
    /// REST: the page has no `items`. GraphQL: an `errors` entry and no
    /// data for the connection.
    MissingData,
    /// REST: `incomplete_results: true`. GraphQL: `hasNextPage` without an
    /// `endCursor`.
    Truncated,
}

fn failure() -> impl Strategy<Value = Failure> {
    prop_oneof![
        Just(Failure::ServerError),
        Just(Failure::MalformedJson),
        Just(Failure::MissingData),
        Just(Failure::Truncated),
    ]
}

/// A number of pages `n` in 1..=5 and, optionally, a failing page `k < n`
/// with its kind of failure.
fn scenario() -> impl Strategy<Value = (usize, Option<(usize, Failure)>)> {
    (1usize..=5).prop_flat_map(|n| (Just(n), proptest::option::of((0..n, failure()))))
}

const MALFORMED: &str = r#"{"total_count": 3, "items": [{"sha": "#;

// ---------------------------------------------------------------------------
// Commit search (REST, `Link` header pagination)
// ---------------------------------------------------------------------------

const LOGIN: &str = "alice";
const QUERY: &str =
    "author:alice committer-date:2024-01-01T00:00:00+00:00..2024-01-01T23:59:59+00:00";

fn search_items(page: usize) -> Vec<Value> {
    let repo = rest_repo("R_widget", "acme", "Organization", "widget", false, false);
    (0..PER_PAGE)
        .map(|j| {
            let i = (page * PER_PAGE + j) as u64;
            let t = format!("2024-01-01T{:02}:{:02}:00Z", i / 60, i % 60);
            search_commit(&repo, &sha(i + 1), Some(LOGIN), &t, &t, 1)
        })
        .collect()
}

async fn run_search(
    n: usize,
    fail: Option<(usize, Failure)>,
) -> Result<(CoverageStatus, usize, usize), String> {
    let server = MockServer::start().await;
    let total = (n * PER_PAGE) as u64;
    for page in 0..n {
        let link = (page + 1 < n).then(|| {
            next_link(
                &server,
                &format!("/search/commits?marker=p&per_page=100&page={}", page + 2),
            )
        });
        let mut body = search_page(total, false, search_items(page));
        let mut response = None;
        if let Some((k, kind)) = fail
            && k == page
        {
            match kind {
                Failure::ServerError => response = Some(ResponseTemplate::new(500)),
                Failure::MalformedJson => {
                    response = Some(
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "application/json")
                            .set_body_string(MALFORMED),
                    );
                }
                Failure::MissingData => {
                    body = json!({ "total_count": total, "incomplete_results": false });
                }
                Failure::Truncated => body["incomplete_results"] = json!(true),
            }
        }
        let response = response.unwrap_or_else(|| {
            let mut r = ResponseTemplate::new(200).set_body_json(body);
            if let Some(l) = &link {
                r = r.insert_header("link", l.as_str());
            }
            r
        });
        let mock = Mock::given(method("GET")).and(path("/search/commits"));
        let mock = if page == 0 {
            mock.and(query_param("q", QUERY))
                .and(query_param_is_missing("page"))
        } else {
            mock.and(query_param("marker", "p"))
                .and(query_param("page", (page + 1).to_string()))
        };
        mock.respond_with(response).mount(&server).await;
    }
    let period = period("2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z");
    match client(&server).search_commits(LOGIN, period, false).await {
        Ok(r) => {
            let unique: HashSet<_> = r.items.iter().map(|c| c.event.event_id.clone()).collect();
            Ok((r.status, r.items.len(), unique.len()))
        }
        Err(e) => {
            assert_no_token(&e, &e.to_string());
            Err(e.to_string())
        }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_failed_search_page_is_never_complete((n, fail) in scenario()) {
        let outcome = runtime().block_on(run_search(n, fail));
        match fail {
            // n pages of 3 commits with distinct SHAs: 3n commits, complete.
            None => prop_assert_eq!(
                outcome,
                Ok((CoverageStatus::Complete, n * PER_PAGE, n * PER_PAGE))
            ),
            Some(_) => {
                if let Ok((status, _, _)) = outcome {
                    prop_assert_ne!(status, CoverageStatus::Complete);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pull request commits (GraphQL cursor pagination)
// ---------------------------------------------------------------------------

const PR_ID: &str = "PR_kwDOwidget7";

fn pr_commits(page: usize) -> Vec<Value> {
    (0..PER_PAGE)
        .map(|j| {
            let i = (page * PER_PAGE + j) as u64;
            let t = format!("2024-03-01T{:02}:{:02}:00Z", i / 60, i % 60);
            pr_commit("acme", "widget", 7, &sha(i + 1), Some("alice"), &t, &t, 1)
        })
        .collect()
}

/// The commits connection of page `page` out of `n`: its cursor is `c-<page>`
/// and the next page is asked for with `after: "c-<page>"`.
fn commits_connection(page: usize, n: usize) -> Value {
    let cursor = format!("c-{page}");
    connection(pr_commits(page), page + 1 < n, Some(&cursor))
}

/// A pull request with nothing but `n` pages of commits; its head commit has
/// no checks and no statuses, so nothing else needs to be fetched.
fn pr_body(n: usize) -> Value {
    let head = sha((n * PER_PAGE) as u64);
    let mut r = gql_repo(
        "R_kgDOwidget",
        gql_actor("Organization", "acme", "O_acme"),
        "widget",
        "PUBLIC",
        false,
    );
    r["pullRequest"] = json!({
        "id": PR_ID,
        "number": 7,
        "title": "Many commits",
        "url": "https://github.com/acme/widget/pull/7",
        "state": "OPEN",
        "isDraft": false,
        "createdAt": "2024-03-01T00:00:00Z",
        "updatedAt": "2024-03-01T06:00:00Z",
        "closedAt": null,
        "mergedAt": null,
        "headRefOid": head,
        "author": user("alice"),
        "mergedBy": null,
        "headCommit": { "nodes": [ { "commit": {
            "oid": head,
            "statusCheckRollup": null,
            "status": null,
            "checkSuites": connection(vec![], false, None)
        } } ] },
        "commits": commits_connection(0, n),
        "reviews": connection(vec![], false, None),
        "reviewThreads": connection(vec![], false, None),
        "comments": connection(vec![], false, None),
        "timelineItems": connection(vec![], false, None)
    });
    gql_data(json!({ "repository": r }))
}

/// Applies a GraphQL failure to the response of page `page`.
fn break_page(page: usize, kind: Failure, body: &mut Value) -> Option<ResponseTemplate> {
    match kind {
        Failure::ServerError => Some(ResponseTemplate::new(500)),
        Failure::MalformedJson => Some(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"data": {"node": {"commits": {"#),
        ),
        Failure::MissingData => {
            if page == 0 {
                body["data"]["repository"]["pullRequest"]["commits"] = Value::Null;
                body["errors"] = json!([{
                    "type": "SERVICE_UNAVAILABLE",
                    "path": ["repository", "pullRequest", "commits"],
                    "message": "Something went wrong while executing your query."
                }]);
            } else {
                *body = json!({
                    "data": { "node": null },
                    "errors": [{
                        "type": "NOT_FOUND",
                        "path": ["node"],
                        "message": "Could not resolve to a node with the global id."
                    }]
                });
            }
            None
        }
        Failure::Truncated => {
            let conn = if page == 0 {
                &mut body["data"]["repository"]["pullRequest"]["commits"]
            } else {
                &mut body["data"]["node"]["commits"]
            };
            conn["pageInfo"] = page_info(true, None);
            None
        }
    }
}

async fn run_pr(
    n: usize,
    fail: Option<(usize, Failure)>,
) -> Result<(CoverageStatus, bool, usize, usize), String> {
    let server = MockServer::start().await;
    for page in 0..n {
        let mut body = if page == 0 {
            pr_body(n)
        } else {
            gql_data(json!({ "node": { "commits": commits_connection(page, n) } }))
        };
        let broken = match fail {
            Some((k, kind)) if k == page => break_page(page, kind, &mut body),
            _ => None,
        };
        let response = broken.unwrap_or_else(|| ResponseTemplate::new(200).set_body_json(body));
        let mock = Mock::given(method("POST")).and(path("/graphql"));
        let mock = if page == 0 {
            mock.and(body_string_contains("query PullRequestDetail"))
        } else {
            mock.and(body_string_contains("query PullRequestCommitsPage"))
                .and(body_partial_json(json!({
                    "variables": { "id": PR_ID, "after": format!("c-{}", page - 1) }
                })))
        };
        mock.respond_with(response).mount(&server).await;
    }
    match client(&server)
        .fetch_pull_request("acme", "widget", 7)
        .await
    {
        Ok(b) => {
            let commits: Vec<_> = b
                .events
                .iter()
                .filter(|e| e.event_type == EventType::PrCommit)
                .collect();
            let unique: HashSet<_> = commits.iter().map(|e| e.event_id.clone()).collect();
            Ok((
                b.status,
                b.complete_collections.contains(&Collection::Commits),
                commits.len(),
                unique.len(),
            ))
        }
        Err(e) => {
            assert_no_token(&e, &e.to_string());
            Err(e.to_string())
        }
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_failed_pull_request_page_is_never_complete((n, fail) in scenario()) {
        let outcome = runtime().block_on(run_pr(n, fail));
        match fail {
            // n pages of 3 commits with distinct SHAs: 3n commits, complete.
            None => prop_assert_eq!(
                outcome,
                Ok((CoverageStatus::Complete, true, n * PER_PAGE, n * PER_PAGE))
            ),
            Some(_) => {
                if let Ok((status, commits_complete, _, _)) = outcome {
                    prop_assert_ne!(status, CoverageStatus::Complete);
                    prop_assert!(!commits_complete);
                }
            }
        }
    }
}
