//! Commit and issue discovery through the search API.

mod common;

use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{ActorKind, EventId, EventType, Source, Visibility};
use ghusage_github::{ApiError, IssueKind};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN: &str = "alice";

fn widget() -> Value {
    rest_repo("R_widget", "acme", "Organization", "widget", false, false)
}

fn gadget() -> Value {
    rest_repo("R_gadget", "alice", "User", "gadget", true, false)
}

/// The query sent for a period given as inclusive whole-second bounds.
fn commit_q(lo: &str, hi: &str) -> String {
    format!("author:{LOGIN} committer-date:{lo}..{hi}")
}

const JAN1: &str = "2024-01-01T00:00:00+00:00";
const JAN1_END: &str = "2024-01-01T23:59:59+00:00";

fn jan1() -> ghusage_core::time::Period {
    period("2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z")
}

/// Mounts the first page of a search (no `page` parameter).
async fn first_page(
    server: &MockServer,
    endpoint: &str,
    q: &str,
    body: Value,
    link: Option<String>,
) {
    let mut r = ResponseTemplate::new(200).set_body_json(body);
    if let Some(l) = link {
        r = r.insert_header("link", l.as_str());
    }
    Mock::given(method("GET"))
        .and(path(format!("/search/{endpoint}")))
        .and(query_param("q", q))
        .and(query_param("per_page", "100"))
        .and(query_param_is_missing("page"))
        .respond_with(r)
        .mount(server)
        .await;
}

/// Mounts page `n` (reached through a `Link` header naming `marker`).
async fn later_page(
    server: &MockServer,
    endpoint: &str,
    marker: &str,
    n: u32,
    body: Value,
    link: Option<String>,
) {
    let mut r = ResponseTemplate::new(200).set_body_json(body);
    if let Some(l) = link {
        r = r.insert_header("link", l.as_str());
    }
    Mock::given(method("GET"))
        .and(path(format!("/search/{endpoint}")))
        .and(query_param("marker", marker))
        .and(query_param("page", n.to_string()))
        .respond_with(r)
        .mount(server)
        .await;
}

fn link_to(server: &MockServer, endpoint: &str, marker: &str, n: u32) -> String {
    next_link(
        server,
        &format!("/search/{endpoint}?marker={marker}&per_page=100&page={n}"),
    )
}

fn commits_on_jan1(repo: &Value, from: u64, count: u64) -> Vec<Value> {
    (from..from + count)
        .map(|i| {
            let t = format!(
                "2024-01-01T{:02}:{:02}:{:02}Z",
                (i / 3600) % 24,
                (i / 60) % 60,
                i % 60
            );
            search_commit(repo, &sha(i), Some(LOGIN), &t, &t, 1)
        })
        .collect()
}

#[tokio::test]
async fn empty_results_are_complete() {
    let server = MockServer::start().await;
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(0, false, vec![]),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert!(r.items.is_empty());
    assert_eq!(r.status, CoverageStatus::Complete);
    assert_eq!(r.reason, None);
}

#[tokio::test]
async fn single_commit_is_normalized() {
    let server = MockServer::start().await;
    let item = search_commit(
        &widget(),
        &sha(1),
        Some(LOGIN),
        "2024-01-01T08:00:00.000+09:00",
        "2024-01-01T12:30:00.000+09:00",
        1,
    );
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(1, false, vec![item]),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    assert_eq!(r.items.len(), 1);
    let rec = &r.items[0];
    assert_eq!(rec.repository.id, "R_widget");
    assert_eq!(rec.repository.owner, "acme");
    assert_eq!(rec.repository.name, "widget");
    assert_eq!(rec.repository.owner_kind, ActorKind::Organization);
    assert_eq!(rec.repository.visibility, Visibility::Public);
    assert!(!rec.repository.is_fork);
    assert_eq!(rec.repository.viewer_permission, None);
    let e = &rec.event;
    assert_eq!(e.event_id, EventId::commit("R_widget", &sha(1)));
    assert_eq!(e.event_type, EventType::CommitAuthored);
    assert_eq!(e.actor.login.as_deref(), Some(LOGIN));
    assert_eq!(e.actor.id.as_deref(), Some("U_alice"));
    assert_eq!(e.actor.kind, ActorKind::User);
    // Committer date (converted to UTC) is the event time; author date is kept.
    assert_eq!(e.occurred_at, ts("2024-01-01T03:30:00Z"));
    assert_eq!(e.meta.author_date, Some(ts("2023-12-31T23:00:00Z")));
    assert_eq!(e.meta.sha.as_deref(), Some(sha(1).as_str()));
    assert_eq!(e.meta.is_merge_commit, Some(false));
    assert_eq!(
        e.meta.committer.as_ref().and_then(|c| c.login.as_deref()),
        Some("web-flow")
    );
    assert_eq!(e.source, Source::Search);
    assert_eq!(e.entity_id, sha(1));
    assert_eq!(e.parent_pr_id, None);
    assert_eq!(e.repository_id, "R_widget");
    assert_eq!(e.coverage, CoverageStatus::Complete);
    assert_eq!(
        e.url.as_deref(),
        Some(format!("https://github.com/acme/widget/commit/{}", sha(1)).as_str())
    );
}

#[tokio::test]
async fn merge_commit_is_identified() {
    let server = MockServer::start().await;
    let items = vec![
        search_commit(
            &widget(),
            &sha(1),
            Some(LOGIN),
            "2024-01-01T01:00:00Z",
            "2024-01-01T01:00:00Z",
            2,
        ),
        search_commit(
            &widget(),
            &sha(2),
            Some(LOGIN),
            "2024-01-01T02:00:00Z",
            "2024-01-01T02:00:00Z",
            1,
        ),
    ];
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(2, false, items),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    let merge: Vec<_> = r
        .items
        .iter()
        .map(|c| c.event.meta.is_merge_commit)
        .collect();
    assert_eq!(merge, vec![Some(true), Some(false)]);
}

#[tokio::test]
async fn two_hundred_fifty_commits_over_three_pages() {
    let server = MockServer::start().await;
    let all = commits_on_jan1(&widget(), 0, 250);
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(250, false, all[0..100].to_vec()),
        Some(link_to(&server, "commits", "m", 2)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "m",
        2,
        search_page(250, false, all[100..200].to_vec()),
        Some(link_to(&server, "commits", "m", 3)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "m",
        3,
        search_page(250, false, all[200..250].to_vec()),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    assert_eq!(r.items.len(), 250);
    let ids: std::collections::HashSet<_> =
        r.items.iter().map(|c| c.event.event_id.clone()).collect();
    assert_eq!(ids.len(), 250);
}

#[tokio::test]
async fn a_short_page_with_a_next_link_is_followed() {
    let server = MockServer::start().await;
    let all = commits_on_jan1(&widget(), 0, 30);
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(30, false, all[0..10].to_vec()),
        Some(link_to(&server, "commits", "s", 2)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "s",
        2,
        search_page(30, false, all[10..30].to_vec()),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 30);
}

#[tokio::test]
async fn a_full_page_without_a_next_link_ends_pagination() {
    let server = MockServer::start().await;
    let all = commits_on_jan1(&widget(), 0, 100);
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(100, false, all),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 100);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn duplicate_sha_across_pages_is_counted_once() {
    let server = MockServer::start().await;
    let c1 = search_commit(
        &widget(),
        &sha(7),
        Some(LOGIN),
        "2024-01-01T01:00:00Z",
        "2024-01-01T01:00:00Z",
        1,
    );
    let c2 = search_commit(
        &widget(),
        &sha(8),
        Some(LOGIN),
        "2024-01-01T02:00:00Z",
        "2024-01-01T02:00:00Z",
        1,
    );
    // The same commit appears again on the second page (results shifted).
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(3, false, vec![c1.clone(), c2]),
        Some(link_to(&server, "commits", "d", 2)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "d",
        2,
        search_page(3, false, vec![c1]),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 2);
}

#[tokio::test]
async fn same_sha_in_two_repositories_is_kept_separately() {
    let server = MockServer::start().await;
    let fork = rest_repo("R_fork", "alice", "User", "widget", false, true);
    let items = vec![
        search_commit(
            &widget(),
            &sha(5),
            Some(LOGIN),
            "2024-01-01T01:00:00Z",
            "2024-01-01T01:00:00Z",
            1,
        ),
        search_commit(
            &fork,
            &sha(5),
            Some(LOGIN),
            "2024-01-01T01:00:00Z",
            "2024-01-01T01:00:00Z",
            1,
        ),
    ];
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(2, false, items),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 2);
    let mut repos: Vec<_> = r.items.iter().map(|c| c.repository.id.as_str()).collect();
    repos.sort();
    assert_eq!(repos, vec!["R_fork", "R_widget"]);
    let fork_rec = r
        .items
        .iter()
        .find(|c| c.repository.id == "R_fork")
        .unwrap();
    assert!(fork_rec.repository.is_fork);
    assert_eq!(fork_rec.repository.owner_kind, ActorKind::User);
}

#[tokio::test]
async fn more_than_1000_results_split_the_period() {
    let server = MockServer::start().await;
    let p = period("2024-01-01T00:00:00Z", "2024-01-03T00:00:00Z");
    // Whole range: too many results.
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, "2024-01-02T23:59:59+00:00"),
        search_page(1500, false, commits_on_jan1(&widget(), 0, 100)),
        Some(link_to(&server, "commits", "whole", 2)),
    )
    .await;
    // First half.
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(2, false, commits_on_jan1(&widget(), 0, 2)),
        None,
    )
    .await;
    // Second half.
    let day2 = vec![search_commit(
        &gadget(),
        &sha(77),
        Some(LOGIN),
        "2024-01-02T05:00:00Z",
        "2024-01-02T05:00:00Z",
        1,
    )];
    first_page(
        &server,
        "commits",
        &commit_q("2024-01-02T00:00:00+00:00", "2024-01-02T23:59:59+00:00"),
        search_page(1, false, day2),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, p, false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete, "{:?}", r.reason);
    assert_eq!(r.items.len(), 3);
    assert_eq!(r.items[2].repository.visibility, Visibility::Private);
    // The oversized query's later pages were never requested.
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| !r.url.query().unwrap_or("").contains("marker=whole"))
    );
}

#[tokio::test]
async fn split_floor_reached_is_partial() {
    let server = MockServer::start().await;
    let p = period("2024-01-01T00:00:00Z", "2024-01-01T00:00:01Z");
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1),
        search_page(1200, false, commits_on_jan1(&widget(), 0, 1)),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, p, false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Partial);
    assert!(r.reason.unwrap().contains("1200"));
    assert_eq!(r.items.len(), 1);
}

#[tokio::test]
async fn incomplete_results_are_partial() {
    let server = MockServer::start().await;
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(5, true, commits_on_jan1(&widget(), 0, 5)),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Partial);
    assert!(r.reason.unwrap().contains("incomplete"));
    assert_eq!(r.items.len(), 5);
}

#[tokio::test]
async fn unreadable_items_make_the_result_partial() {
    let server = MockServer::start().await;
    let mut broken = search_commit(
        &widget(),
        &sha(2),
        Some(LOGIN),
        "2024-01-01T01:00:00Z",
        "2024-01-01T01:00:00Z",
        1,
    );
    broken["repository"] = Value::Null;
    let mut no_date = search_commit(
        &widget(),
        &sha(3),
        Some(LOGIN),
        "2024-01-01T01:00:00Z",
        "2024-01-01T01:00:00Z",
        1,
    );
    no_date["commit"]["committer"] = Value::Null;
    let ok = search_commit(
        &widget(),
        &sha(1),
        Some(LOGIN),
        "2024-01-01T01:00:00Z",
        "2024-01-01T01:00:00Z",
        1,
    );
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(3, false, vec![ok, broken, no_date]),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.status, CoverageStatus::Partial);
}

#[tokio::test]
async fn commit_without_linked_user_has_unknown_actor() {
    let server = MockServer::start().await;
    let item = search_commit(
        &widget(),
        &sha(1),
        None,
        "2024-01-01T01:00:00Z",
        "2024-01-01T01:00:00Z",
        1,
    );
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(1, false, vec![item]),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert!(r.items[0].event.actor.is_unknown());
}

#[tokio::test]
async fn public_only_restricts_the_query_and_drops_private_repositories() {
    let server = MockServer::start().await;
    let items = vec![
        search_commit(
            &widget(),
            &sha(1),
            Some(LOGIN),
            "2024-01-01T01:00:00Z",
            "2024-01-01T01:00:00Z",
            1,
        ),
        search_commit(
            &gadget(),
            &sha(2),
            Some(LOGIN),
            "2024-01-01T02:00:00Z",
            "2024-01-01T02:00:00Z",
            1,
        ),
    ];
    first_page(
        &server,
        "commits",
        &format!("{} is:public", commit_q(JAN1, JAN1_END)),
        search_page(2, false, items),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), true)
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].repository.id, "R_widget");
}

#[tokio::test]
async fn failure_mid_pagination_is_an_error() {
    let server = MockServer::start().await;
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(150, false, commits_on_jan1(&widget(), 0, 100)),
        Some(link_to(&server, "commits", "f", 2)),
    )
    .await;
    Mock::given(path("/search/commits"))
        .and(query_param("marker", "f"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let err = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Http { status: 502, .. }), "{err:?}");
}

#[tokio::test]
async fn pagination_link_to_another_host_is_refused() {
    let server = MockServer::start().await;
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(150, false, commits_on_jan1(&widget(), 0, 100)),
        Some("<https://evil.example.com/search/commits?page=2>; rel=\"next\"".to_string()),
    )
    .await;
    let err = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidResponse(_)), "{err:?}");
}

fn issue_q(kind: &str) -> String {
    format!("involves:{LOGIN} {kind} updated:{JAN1}..{JAN1_END}")
}

#[tokio::test]
async fn issue_search_never_returns_pull_requests() {
    let server = MockServer::start().await;
    let items = vec![
        search_issue("acme", "widget", 1, "I_1", false, "2024-01-01T05:00:00Z"),
        // GitHub would not return this for is:issue, but if it does it is dropped.
        search_issue("acme", "widget", 2, "PR_2", true, "2024-01-01T06:00:00Z"),
    ];
    first_page(
        &server,
        "issues",
        &issue_q("is:issue"),
        search_page(2, false, items),
        None,
    )
    .await;
    let r = client(&server)
        .search_issues(
            &format!("involves:{LOGIN}"),
            IssueKind::Issue,
            jan1(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    let i = &r.items[0];
    assert_eq!(i.node_id, "I_1");
    assert_eq!(i.kind, IssueKind::Issue);
    assert_eq!(
        (i.owner.as_str(), i.name.as_str(), i.number),
        ("acme", "widget", 1)
    );
    assert_eq!(i.updated_at, ts("2024-01-01T05:00:00Z"));
}

#[tokio::test]
async fn pull_request_search_returns_pull_requests() {
    let server = MockServer::start().await;
    let items = vec![
        search_issue("acme", "widget", 2, "PR_2", true, "2024-01-01T06:00:00Z"),
        search_issue("other", "repo", 9, "PR_9", true, "2024-01-01T07:00:00Z"),
    ];
    first_page(
        &server,
        "issues",
        &format!("{} is:public", issue_q("is:pr")),
        search_page(2, false, items),
        None,
    )
    .await;
    let r = client(&server)
        .search_issues(
            &format!("involves:{LOGIN}"),
            IssueKind::PullRequest,
            jan1(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    let ids: Vec<_> = r.items.iter().map(|i| i.node_id.as_str()).collect();
    assert_eq!(ids, vec!["PR_2", "PR_9"]);
    assert!(r.items.iter().all(|i| i.kind == IssueKind::PullRequest));
}

#[tokio::test]
async fn issue_search_over_1000_splits_and_deduplicates() {
    let server = MockServer::start().await;
    let p = period("2024-01-01T00:00:00Z", "2024-01-03T00:00:00Z");
    first_page(
        &server,
        "issues",
        &format!("involves:{LOGIN} is:pr updated:{JAN1}..2024-01-02T23:59:59+00:00"),
        search_page(1001, false, vec![]),
        None,
    )
    .await;
    let a = search_issue("acme", "widget", 1, "PR_1", true, "2024-01-01T05:00:00Z");
    first_page(
        &server,
        "issues",
        &issue_q("is:pr"),
        search_page(1, false, vec![a.clone()]),
        None,
    )
    .await;
    // The same PR shows up in the later half too (it was updated again).
    let mut a2 = a.clone();
    a2["updated_at"] = json!("2024-01-02T05:00:00Z");
    first_page(
        &server,
        "issues",
        &format!(
            "involves:{LOGIN} is:pr updated:2024-01-02T00:00:00+00:00..2024-01-02T23:59:59+00:00"
        ),
        search_page(1, false, vec![a2]),
        None,
    )
    .await;
    let r = client(&server)
        .search_issues(
            &format!("involves:{LOGIN}"),
            IssueKind::PullRequest,
            p,
            false,
        )
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.status, CoverageStatus::Complete);
}

#[tokio::test]
async fn results_added_while_paging_are_still_fetched() {
    // The first page says 150 results (two pages), but by the time page 2 is
    // read a third page exists and page 2 links to it.
    let server = MockServer::start().await;
    let all = commits_on_jan1(&widget(), 0, 250);
    first_page(
        &server,
        "commits",
        &commit_q(JAN1, JAN1_END),
        search_page(150, false, all[0..100].to_vec()),
        Some(link_to(&server, "commits", "g", 2)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "g",
        2,
        search_page(250, false, all[100..200].to_vec()),
        Some(link_to(&server, "commits", "g", 3)),
    )
    .await;
    later_page(
        &server,
        "commits",
        "g",
        3,
        search_page(250, false, all[200..250].to_vec()),
        None,
    )
    .await;
    let r = client(&server)
        .search_commits(LOGIN, jan1(), false)
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    assert_eq!(r.items.len(), 250);
}
