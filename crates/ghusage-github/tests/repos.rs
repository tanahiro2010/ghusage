//! Repository discovery and default-branch commit listing.

mod common;

use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{ActorKind, EventId, EventType, Repository, Source, Visibility};
use ghusage_github::ApiError;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN: &str = "alice";

/// An item of `GET /user/repos` with the viewer's permissions.
fn listed(node_id: &str, owner: &str, name: &str, private: bool, pushed_at: Option<&str>) -> Value {
    let owner_type = if owner == LOGIN {
        "User"
    } else {
        "Organization"
    };
    let mut v = rest_repo(node_id, owner, owner_type, name, private, false);
    v["pushed_at"] = json!(pushed_at);
    v["visibility"] = json!(if private { "private" } else { "public" });
    v["permissions"] = json!({
        "admin": owner == LOGIN, "maintain": owner == LOGIN, "push": true,
        "triage": true, "pull": true
    });
    v
}

/// One item of `GET /repos/{owner}/{name}/commits`.
fn list_commit(
    repo: &Value,
    sha: &str,
    author: Option<&str>,
    author_date: &str,
    committer_date: &str,
    parents: usize,
) -> Value {
    let mut v = search_commit(repo, sha, author, author_date, committer_date, parents);
    let obj = v.as_object_mut().unwrap();
    obj.remove("repository");
    obj.remove("score");
    v
}

fn widget() -> Value {
    rest_repo("R_widget", "acme", "Organization", "widget", false, false)
}

fn widget_repo() -> Repository {
    Repository {
        id: "R_widget".into(),
        owner: "acme".into(),
        name: "widget".into(),
        owner_kind: ActorKind::Organization,
        visibility: Visibility::Public,
        is_fork: false,
        viewer_permission: Some("WRITE".into()),
        accessible: true,
        url: Some("https://github.com/acme/widget".into()),
    }
}

fn jan() -> ghusage_core::time::Period {
    period("2024-01-10T00:00:00Z", "2024-01-20T00:00:00Z")
}

/// `n` repositories `R_0`.. pushed one hour apart, the newest at `newest`.
fn pushed_hourly(n: usize, newest: &str) -> Vec<Value> {
    let newest = ts(newest);
    (0..n)
        .map(|i| {
            let at = newest - jiff::SignedDuration::from_hours(i as i64);
            listed(
                &format!("R_{i}"),
                "acme",
                &format!("r{i}"),
                false,
                Some(&at.to_string()),
            )
        })
        .collect()
}

/// Mounts `GET /user/repos` serving `all` (sorted newest push first) the
/// way GitHub pages it: `per_page` and `page` select a slice, and a `next`
/// link is present while items remain.
async fn mount_repo_list(server: &MockServer, route: &str, all: &[Value]) {
    use wiremock::{Request, Respond};
    struct Pages {
        uri: String,
        route: String,
        all: Vec<Value>,
    }
    impl Respond for Pages {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let q: std::collections::HashMap<String, String> =
                req.url.query_pairs().into_owned().collect();
            let per_page: usize = q.get("per_page").map_or(30, |v| v.parse().unwrap());
            let page: usize = q.get("page").map_or(1, |v| v.parse().unwrap());
            let from = (page - 1) * per_page;
            let slice: Vec<Value> = self.all.iter().skip(from).take(per_page).cloned().collect();
            let mut resp = ResponseTemplate::new(200).set_body_json(Value::Array(slice));
            if from + per_page < self.all.len() {
                resp = resp.insert_header(
                    "link",
                    format!(
                        "<{}{}?per_page={per_page}&page={}>; rel=\"next\"",
                        self.uri,
                        self.route,
                        page + 1
                    )
                    .as_str(),
                );
            }
            resp
        }
    }
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(Pages {
            uri: server.uri(),
            route: route.into(),
            all: all.to_vec(),
        })
        .mount(server)
        .await;
}

/// The `(per_page, page)` of every repository list request, in order.
async fn list_requests(server: &MockServer, route: &str) -> Vec<(String, Option<String>)> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == route)
        .map(|r| {
            let q: std::collections::HashMap<String, String> =
                r.url.query_pairs().into_owned().collect();
            (
                q.get("per_page").cloned().unwrap_or_default(),
                q.get("page").cloned(),
            )
        })
        .collect()
}

fn ids(repos: &[ghusage_github::PushedRepository]) -> Vec<String> {
    repos.iter().map(|r| r.repository.id.clone()).collect()
}

#[tokio::test]
async fn viewer_repositories_are_paged_until_older_than_the_start() {
    let server = MockServer::start().await;
    let page1 = vec![
        listed("R_a", LOGIN, "a", false, Some("2024-01-25T00:00:00Z")),
        listed("R_b", "acme", "b", true, Some("2024-01-15T00:00:00Z")),
    ];
    let page2 = vec![
        listed("R_c", "acme", "c", false, Some("2024-01-10T00:00:00Z")),
        // Older than the start: paging stops after this page.
        listed("R_d", "acme", "d", false, Some("2024-01-09T23:59:59Z")),
        listed("R_e", "acme", "e", false, Some("2023-06-01T00:00:00Z")),
    ];
    // A small first page without anything older than the start: the list
    // continues with large pages from the beginning.
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param(
            "affiliation",
            "owner,collaborator,organization_member",
        ))
        .and(query_param("sort", "pushed"))
        .and(query_param("direction", "desc"))
        .and(query_param("per_page", "30"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([page1[0].clone()]))
                .insert_header(
                    "link",
                    next_link(&server, "/user/repos?per_page=30&page=2").as_str(),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param(
            "affiliation",
            "owner,collaborator,organization_member",
        ))
        .and(query_param("sort", "pushed"))
        .and(query_param("direction", "desc"))
        .and(query_param("per_page", "100"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(Value::Array(page1))
                .insert_header(
                    "link",
                    next_link(&server, "/user/repos?per_page=100&page=2").as_str(),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("page", "2"))
        .and(query_param("per_page", "100"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(Value::Array(page2))
                .insert_header(
                    "link",
                    next_link(&server, "/user/repos?per_page=100&page=3").as_str(),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("page", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(0)
        .mount(&server)
        .await;

    let repos = client(&server)
        .pushed_repositories(None, ts("2024-01-10T00:00:00Z"))
        .await
        .unwrap();
    let names: Vec<&str> = repos.iter().map(|r| r.repository.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
    let a = &repos[0];
    assert_eq!(a.repository.id, "R_a");
    assert_eq!(a.repository.owner, LOGIN);
    assert_eq!(a.repository.owner_kind, ActorKind::User);
    assert_eq!(a.repository.viewer_permission.as_deref(), Some("ADMIN"));
    assert_eq!(a.pushed_at, Some(ts("2024-01-25T00:00:00Z")));
    let b = &repos[1];
    assert_eq!(b.repository.visibility, Visibility::Private);
    assert_eq!(b.repository.viewer_permission.as_deref(), Some("WRITE"));
    assert_eq!(b.repository.owner_kind, ActorKind::Organization);
}

#[tokio::test]
async fn a_short_list_takes_one_small_request() {
    // A recent start is usually reached within the first few repositories:
    // one small page answers it, and no large page is requested.
    let server = MockServer::start().await;
    let all = pushed_hourly(250, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/user/repos", &all).await;
    // R_0..R_12 are pushed at or after the start.
    let repos = client(&server)
        .pushed_repositories(None, ts("2024-01-19T12:00:00Z"))
        .await
        .unwrap();
    assert_eq!(ids(&repos), ids_of(0..13));
    assert_eq!(
        list_requests(&server, "/user/repos").await,
        vec![("30".into(), None)]
    );
}

fn ids_of(r: std::ops::Range<usize>) -> Vec<String> {
    r.map(|i| format!("R_{i}")).collect()
}

#[tokio::test]
async fn a_long_list_continues_with_large_pages_without_duplicates() {
    // An early start reaches past the small first page: the list goes on
    // with pages of 100 from the beginning, and every repository pushed at
    // or after the start is returned once, in push order.
    let server = MockServer::start().await;
    let all = pushed_hourly(250, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/user/repos", &all).await;
    // R_0..R_149 are pushed at or after the start (R_149 exactly at it).
    let start = ts("2024-01-20T00:00:00Z") - jiff::SignedDuration::from_hours(149);
    let repos = client(&server)
        .pushed_repositories(None, start)
        .await
        .unwrap();
    assert_eq!(ids(&repos), ids_of(0..150));
    assert_eq!(
        list_requests(&server, "/user/repos").await,
        vec![
            ("30".into(), None),
            ("100".into(), None),
            ("100".into(), Some("2".into())),
        ]
    );
}

#[tokio::test]
async fn a_repository_pushed_exactly_at_the_start_does_not_end_the_list() {
    // The 30th repository of the small page is pushed exactly at the start,
    // so it is kept and the list continues to the first older one.
    let server = MockServer::start().await;
    let all = pushed_hourly(40, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/user/repos", &all).await;
    let start = ts("2024-01-20T00:00:00Z") - jiff::SignedDuration::from_hours(29);
    let repos = client(&server)
        .pushed_repositories(None, start)
        .await
        .unwrap();
    assert_eq!(ids(&repos), ids_of(0..30));
    assert_eq!(
        list_requests(&server, "/user/repos").await,
        vec![("30".into(), None), ("100".into(), None)]
    );
}

#[tokio::test]
async fn every_repository_is_returned_when_none_is_older_than_the_start() {
    let server = MockServer::start().await;
    let all = pushed_hourly(130, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/user/repos", &all).await;
    let repos = client(&server)
        .pushed_repositories(None, ts("2023-01-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(ids(&repos), ids_of(0..130));
    assert_eq!(
        list_requests(&server, "/user/repos").await,
        vec![
            ("30".into(), None),
            ("100".into(), None),
            ("100".into(), Some("2".into())),
        ]
    );
}

#[tokio::test]
async fn a_list_shorter_than_the_small_page_takes_one_request() {
    let server = MockServer::start().await;
    let all = pushed_hourly(7, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/user/repos", &all).await;
    let repos = client(&server)
        .pushed_repositories(None, ts("2023-01-01T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(ids(&repos), ids_of(0..7));
    assert_eq!(
        list_requests(&server, "/user/repos").await,
        vec![("30".into(), None)]
    );
}

#[tokio::test]
async fn another_users_list_starts_small_as_well() {
    let server = MockServer::start().await;
    let all = pushed_hourly(250, "2024-01-20T00:00:00Z");
    mount_repo_list(&server, "/users/bob/repos", &all).await;
    let recent = client(&server)
        .pushed_repositories(Some("bob"), ts("2024-01-19T12:00:00Z"))
        .await
        .unwrap();
    assert_eq!(ids(&recent), ids_of(0..13));
    assert_eq!(
        list_requests(&server, "/users/bob/repos").await,
        vec![("30".into(), None)]
    );
    server.reset().await;
    mount_repo_list(&server, "/users/bob/repos", &all).await;
    let start = ts("2024-01-20T00:00:00Z") - jiff::SignedDuration::from_hours(149);
    let long = client(&server)
        .pushed_repositories(Some("bob"), start)
        .await
        .unwrap();
    assert_eq!(ids(&long), ids_of(0..150));
    assert_eq!(
        list_requests(&server, "/users/bob/repos").await,
        vec![
            ("30".into(), None),
            ("100".into(), None),
            ("100".into(), Some("2".into())),
        ]
    );
}

#[tokio::test]
async fn repositories_never_pushed_are_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            listed("R_new", LOGIN, "new", false, None),
            listed("R_a", LOGIN, "a", false, Some("2024-01-12T00:00:00Z")),
        ])))
        .mount(&server)
        .await;
    let repos = client(&server)
        .pushed_repositories(None, ts("2024-01-10T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].repository.id, "R_a");
}

#[tokio::test]
async fn other_users_repositories_come_from_their_public_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/bob/repos"))
        .and(query_param("sort", "pushed"))
        .and(query_param("direction", "desc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            listed("R_bob", "bob", "tools", false, Some("2024-01-12T00:00:00Z")),
            listed("R_old", "bob", "old", false, Some("2023-01-12T00:00:00Z")),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let repos = client(&server)
        .pushed_repositories(Some("bob"), ts("2024-01-10T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].repository.full_name(), "bob/tools");
}

#[tokio::test]
async fn default_branch_commits_are_paged_filtered_and_normalized() {
    let server = MockServer::start().await;
    let w = widget();
    let page1 = vec![
        // Committer date after the period (within the queried margin).
        list_commit(
            &w,
            &sha(9),
            Some(LOGIN),
            "2024-01-19T10:00:00Z",
            "2024-01-20T00:00:00Z",
            1,
        ),
        // Authored before the period, committed inside it: counted.
        list_commit(
            &w,
            &sha(2),
            Some(LOGIN),
            "2024-01-01T08:00:00Z",
            "2024-01-15T12:00:00+09:00",
            2,
        ),
    ];
    let page2 = vec![
        list_commit(
            &w,
            &sha(1),
            Some(LOGIN),
            "2024-01-10T00:00:00Z",
            "2024-01-10T00:00:00Z",
            1,
        ),
        // Committed before the period.
        list_commit(
            &w,
            &sha(0),
            Some(LOGIN),
            "2024-01-09T23:00:00Z",
            "2024-01-09T23:59:59Z",
            1,
        ),
    ];
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .and(query_param("author", LOGIN))
        .and(query_param("since", "2024-01-09T00:00:00Z"))
        .and(query_param("until", "2024-01-21T00:00:00Z"))
        .and(query_param("per_page", "100"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(Value::Array(page1))
                .insert_header(
                    "link",
                    next_link(&server, "/repos/acme/widget/commits?author=alice&page=2").as_str(),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(page2)))
        .expect(1)
        .mount(&server)
        .await;

    let r = client(&server)
        .default_branch_commits(&widget_repo(), LOGIN, jan())
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    assert_eq!(r.reason, None);
    let shas: Vec<String> = r
        .items
        .iter()
        .map(|c| c.event.meta.sha.clone().unwrap())
        .collect();
    assert_eq!(shas, vec![sha(1), sha(2)]);
    let rec = &r.items[1];
    assert_eq!(rec.repository, widget_repo());
    let e = &rec.event;
    // Same id as a commit search hit for the same repository and SHA.
    assert_eq!(e.event_id, EventId::commit("R_widget", &sha(2)));
    assert_eq!(e.event_type, EventType::CommitAuthored);
    assert_eq!(e.actor.login.as_deref(), Some(LOGIN));
    assert_eq!(e.actor.id.as_deref(), Some("U_alice"));
    assert_eq!(e.occurred_at, ts("2024-01-15T03:00:00Z"));
    assert_eq!(e.meta.author_date, Some(ts("2024-01-01T08:00:00Z")));
    assert_eq!(e.meta.is_merge_commit, Some(true));
    assert_eq!(e.source, Source::Rest);
    assert_eq!(e.repository_id, "R_widget");
    assert_eq!(e.entity_id, sha(2));
    assert_eq!(r.items[0].event.meta.is_merge_commit, Some(false));
}

#[tokio::test]
async fn unreadable_commits_make_the_listing_partial() {
    let server = MockServer::start().await;
    let w = widget();
    let mut no_sha = list_commit(
        &w,
        &sha(3),
        Some(LOGIN),
        "2024-01-12T00:00:00Z",
        "2024-01-12T00:00:00Z",
        1,
    );
    no_sha.as_object_mut().unwrap().remove("sha");
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            no_sha,
            list_commit(
                &w,
                &sha(4),
                Some(LOGIN),
                "2024-01-12T00:00:00Z",
                "2024-01-12T00:00:00Z",
                1
            ),
        ])))
        .mount(&server)
        .await;
    let r = client(&server)
        .default_branch_commits(&widget_repo(), LOGIN, jan())
        .await
        .unwrap();
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.status, CoverageStatus::Partial);
    assert!(r.reason.unwrap().contains("acme/widget"));
}

#[tokio::test]
async fn empty_repository_has_no_commits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "message": "Git Repository is empty.",
            "documentation_url": "https://docs.github.com/rest/commits/commits#list-commits",
            "status": "409"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let r = client(&server)
        .default_branch_commits(&widget_repo(), LOGIN, jan())
        .await
        .unwrap();
    assert_eq!(r.items, Vec::new());
    assert_eq!(r.status, CoverageStatus::Complete);
}

#[tokio::test]
async fn inaccessible_repositories_are_errors_without_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "Resource protected by organization SAML enforcement. You must grant your Personal Access token access to this organization.",
            "documentation_url": "https://docs.github.com/articles/authenticating-to-a-github-organization-with-saml-single-sign-on/",
            "status": "403"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/gone/commits"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest",
            "status": "404"
        })))
        .mount(&server)
        .await;
    let c = client(&server);
    let err = c
        .default_branch_commits(&widget_repo(), LOGIN, jan())
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden(_)), "{err:?}");
    assert_no_token(&err, &err.to_string());

    let gone = Repository {
        id: "R_gone".into(),
        name: "gone".into(),
        ..widget_repo()
    };
    let err = c
        .default_branch_commits(&gone, LOGIN, jan())
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    assert_no_token(&err, &err.to_string());

    // A failing repository list is an error as well, also without the token.
    let err = c
        .pushed_repositories(None, ts("2024-01-10T00:00:00Z"))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    assert_no_token(&err, &err.to_string());
}

#[tokio::test]
async fn commit_pages_up_to_the_last_link_are_all_fetched_in_order() {
    // GitHub's first page links to the next and the last page; pages 2..4
    // are fetched together and their commits kept in page order.
    let server = MockServer::start().await;
    let w = widget();
    let day = |d: u32| format!("2024-01-{d:02}T10:00:00Z");
    let page = |n: u32| {
        vec![list_commit(
            &w,
            &sha(u64::from(n)),
            Some(LOGIN),
            &day(10 + n),
            &day(10 + n),
            1,
        )]
    };
    let base = "/repos/acme/widget/commits?author=alice&per_page=100";
    let links = format!(
        "{}, <{}{base}&page=4>; rel=\"last\"",
        next_link(&server, &format!("{base}&page=2")),
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(Value::Array(page(1)))
                .insert_header("link", links.as_str()),
        )
        .expect(1)
        .mount(&server)
        .await;
    for n in 2..=4u32 {
        Mock::given(method("GET"))
            .and(path("/repos/acme/widget/commits"))
            .and(query_param("page", n.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(page(n))))
            .expect(1)
            .mount(&server)
            .await;
    }
    let r = client(&server)
        .default_branch_commits(&widget_repo(), LOGIN, jan())
        .await
        .unwrap();
    assert_eq!(r.status, CoverageStatus::Complete);
    let shas: Vec<String> = r
        .items
        .iter()
        .map(|c| c.event.meta.sha.clone().unwrap())
        .collect();
    assert_eq!(shas, (1..=4).map(sha).collect::<Vec<_>>());
}
