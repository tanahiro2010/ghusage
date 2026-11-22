//! Fetching one commit with its files and diff over REST.

mod common;

use common::*;
use ghusage_core::commit::FileStatus;
use ghusage_core::model::Visibility;
use ghusage_github::ApiError;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PATH: &str = "/repos/acme/widget/commits/abcdef0";

fn rest_file(name: &str, status: &str, add: u64, del: u64, patch: Option<&str>) -> Value {
    let mut v = json!({
        "sha": "f00",
        "filename": name,
        "status": status,
        "additions": add,
        "deletions": del,
        "changes": add + del,
        "blob_url": format!("https://github.com/acme/widget/blob/abc/{name}"),
        "raw_url": format!("https://github.com/acme/widget/raw/abc/{name}"),
        "contents_url": format!("https://api.github.com/repos/acme/widget/contents/{name}?ref=abc"),
    });
    if let Some(p) = patch {
        v["patch"] = json!(p);
    }
    v
}

/// A response of `GET /repos/{owner}/{repo}/commits/{sha}` as GitHub sends it.
fn commit_body(files: Vec<Value>, parents: usize) -> Value {
    let full = sha(0xabcdef0);
    json!({
        "sha": full,
        "node_id": "C_abc",
        "url": format!("https://api.github.com/repos/acme/widget/commits/{full}"),
        "html_url": format!("https://github.com/acme/widget/commit/{full}"),
        "comments_url": "https://api.github.com/repos/acme/widget/commits/x/comments",
        "commit": {
            "url": "https://api.github.com/repos/acme/widget/git/commits/x",
            "author": { "name": "Alice Example", "email": "alice@example.com", "date": "2026-11-10T03:00:00Z" },
            "committer": { "name": "GitHub", "email": "noreply@github.com", "date": "2026-11-10T04:30:00+09:00" },
            "message": "fix(parser): handle empty input\n\nThe parser crashed on empty files.\n\nCo-authored-by: Bob <bob@example.com>",
            "tree": { "sha": "t", "url": "https://api.github.com/repos/acme/widget/git/trees/t" },
            "comment_count": 0,
            "verification": { "verified": false, "reason": "unsigned", "signature": null, "payload": null }
        },
        "author": rest_user("alice", "U_alice", "User"),
        "committer": rest_user("web-flow", "U_webflow", "User"),
        "parents": (0..parents).map(|i| json!({
            "sha": sha(9000 + i as u64),
            "url": "https://api.github.com/repos/acme/widget/commits/p",
            "html_url": "https://github.com/acme/widget/commit/p"
        })).collect::<Vec<_>>(),
        "stats": { "total": 7, "additions": 5, "deletions": 2 },
        "files": files
    })
}

#[tokio::test]
async fn commit_with_message_people_parents_stats_and_files() {
    let server = MockServer::start().await;
    let files = vec![
        rest_file(
            "src/parser.rs",
            "modified",
            3,
            2,
            Some(
                "@@ -1,4 +1,5 @@\n fn parse() {\n-    old();\n-    older();\n+    new();\n+    newer();\n+    newest();\n }",
            ),
        ),
        {
            let mut f = rest_file(
                "src/lexer.rs",
                "renamed",
                2,
                0,
                Some("@@ -9,0 +10,2 @@\n+a\n+b"),
            );
            f["previous_filename"] = json!("src/tokens.rs");
            f
        },
    ];
    Mock::given(method("GET"))
        .and(path(PATH))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(commit_body(files, 2)))
        .expect(1)
        .mount(&server)
        .await;
    let d = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap();
    assert_eq!(d.repository, "acme/widget");
    assert_eq!(d.sha, sha(0xabcdef0));
    assert_eq!(
        d.url.as_deref(),
        Some(format!("https://github.com/acme/widget/commit/{}", sha(0xabcdef0)).as_str())
    );
    assert_eq!(d.headline(), "fix(parser): handle empty input");
    assert!(d.message.contains("The parser crashed on empty files."));
    assert!(d.message.ends_with("Co-authored-by: Bob <bob@example.com>"));
    assert_eq!(d.author.name.as_deref(), Some("Alice Example"));
    assert_eq!(d.author.login.as_deref(), Some("alice"));
    assert_eq!(d.author.date, Some(ts("2026-11-10T03:00:00Z")));
    assert_eq!(d.committer.name.as_deref(), Some("GitHub"));
    assert_eq!(d.committer.login.as_deref(), Some("web-flow"));
    // Offsets are converted to UTC.
    assert_eq!(d.committer.date, Some(ts("2026-11-09T19:30:00Z")));
    assert_eq!(d.parents, vec![sha(9000), sha(9001)]);
    assert!(d.is_merge());
    assert_eq!(
        (d.stats.additions, d.stats.deletions, d.stats.total),
        (5, 2, 7)
    );
    assert_eq!(d.files.len(), 2);
    assert_eq!(d.files[0].filename, "src/parser.rs");
    assert_eq!(d.files[0].status, FileStatus::Modified);
    assert_eq!((d.files[0].additions, d.files[0].deletions), (3, 2));
    assert!(
        d.files[0]
            .patch
            .as_deref()
            .unwrap()
            .contains("+    newest();")
    );
    assert_eq!(d.files[1].status, FileStatus::Renamed);
    assert_eq!(
        d.files[1].previous_filename.as_deref(),
        Some("src/tokens.rs")
    );
    assert!(!d.files_truncated);
    // The commit endpoint does not say whether the repository is private.
    assert_eq!(d.visibility, Visibility::Unknown);
}

#[tokio::test]
async fn files_are_read_from_every_page() {
    let server = MockServer::start().await;
    let page1: Vec<Value> = (0..100)
        .map(|i| {
            rest_file(
                &format!("f{i:03}.txt"),
                "added",
                1,
                0,
                Some("@@ -0,0 +1 @@\n+x"),
            )
        })
        .collect();
    let page2: Vec<Value> = (100..130)
        .map(|i| {
            rest_file(
                &format!("f{i:03}.txt"),
                "added",
                1,
                0,
                Some("@@ -0,0 +1 @@\n+x"),
            )
        })
        .collect();
    Mock::given(method("GET"))
        .and(path(PATH))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(commit_body(page2, 1)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(commit_body(page1, 1))
                .insert_header(
                    "link",
                    next_link(&server, &format!("{PATH}?per_page=100&page=2")).as_str(),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let d = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap();
    assert_eq!(d.files.len(), 130);
    assert_eq!(d.files[0].filename, "f000.txt");
    assert_eq!(d.files[129].filename, "f129.txt");
    // Every file is listed once.
    let names: std::collections::HashSet<_> = d.files.iter().map(|f| &f.filename).collect();
    assert_eq!(names.len(), 130);
    assert!(!d.files_truncated);
    assert!(!d.is_merge());
}

#[tokio::test]
async fn files_without_a_patch_are_kept_and_explained() {
    let server = MockServer::start().await;
    let files = vec![
        rest_file("logo.png", "added", 0, 0, None),
        rest_file("data/huge.json", "modified", 40000, 39000, None),
        rest_file("README.md", "modified", 1, 1, Some("@@ -1 +1 @@\n-a\n+b")),
    ];
    Mock::given(method("GET"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(commit_body(files, 1)))
        .mount(&server)
        .await;
    let d = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap();
    assert_eq!(d.files.len(), 3);
    assert_eq!(d.files[0].patch, None);
    assert!(
        d.files[0]
            .missing_patch_reason()
            .unwrap()
            .contains("binary file or diff too large")
    );
    assert_eq!(d.files[1].patch, None);
    assert_eq!(d.files[1].changes, 79000);
    assert!(d.files[1].missing_patch_reason().is_some());
    assert_eq!(d.files[2].missing_patch_reason(), None);
}

#[tokio::test]
async fn missing_commit_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;
    let e = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap_err();
    assert!(matches!(e, ApiError::NotFound(_)), "{e:?}");
    assert!(e.to_string().contains("acme/widget@abcdef0"), "{e}");
    assert_no_token(&e, &e.to_string());
}

#[tokio::test]
async fn unknown_sha_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "No commit found for SHA: abcdef0"
        })))
        .mount(&server)
        .await;
    let e = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap_err();
    assert!(matches!(e, ApiError::NotFound(_)), "{e:?}");
}

#[tokio::test]
async fn forbidden_commit_is_an_access_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "4999")
                .set_body_json(json!({
                    "message": "Resource not accessible by personal access token"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;
    let e = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap_err();
    match &e {
        ApiError::Forbidden(m) => assert!(m.contains("not accessible"), "{m}"),
        other => panic!("expected Forbidden, got {other:?}"),
    }
    assert_no_token(&e, &e.to_string());
}

#[tokio::test]
async fn a_failing_files_page_fails_the_whole_fetch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(commit_body(
                    vec![rest_file("a", "added", 1, 0, Some("+a"))],
                    1,
                ))
                .insert_header(
                    "link",
                    next_link(&server, &format!("{PATH}?per_page=100&page=2")).as_str(),
                ),
        )
        .mount(&server)
        .await;
    let e = client(&server)
        .fetch_commit("acme", "widget", "abcdef0")
        .await
        .unwrap_err();
    assert!(matches!(e, ApiError::NotFound(_)), "{e:?}");
}
