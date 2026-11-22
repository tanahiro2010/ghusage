//! `ghusage commit`: fetching a commit once, then reading it from the cache.

use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn run(data: &Path, api: Option<&str>, args: &[&str]) -> Output {
    let mut c = Command::cargo_bin("ghusage").unwrap();
    c.env_clear();
    for k in [
        "SYSTEMROOT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "COMSPEC",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.env("PATH", data.join("no-bin"))
        .env("GHUSAGE_DATA_DIR", data)
        .env("TZ", "UTC")
        .env("GHUSAGE_BACKOFF_MS", "1")
        .env("GH_TOKEN", "ghp_testtoken0123456789");
    if let Some(api) = api {
        c.env("GHUSAGE_API_URL", api);
    }
    c.args(args).output().unwrap()
}

fn commit_body() -> Value {
    json!({
        "sha": SHA,
        "html_url": format!("https://github.com/acme/widget/commit/{SHA}"),
        "commit": {
            "author": { "name": "Alice", "email": "a@example.com", "date": "2026-11-10T03:00:00Z" },
            "committer": { "name": "Bob", "email": "b@example.com", "date": "2026-11-10T04:00:00Z" },
            "message": "fix: handle \u{1b}[31mempty\u{1b}[0m input\n\nDetails here."
        },
        "author": { "login": "alice", "node_id": "U_alice", "type": "User" },
        "committer": { "login": "bob", "node_id": "U_bob", "type": "User" },
        "parents": [ { "sha": "1111111111111111111111111111111111111111" } ],
        "stats": { "total": 3, "additions": 2, "deletions": 1 },
        "files": [
            { "filename": "src/lib.rs", "status": "modified", "additions": 2, "deletions": 1, "changes": 3,
              "patch": "@@ -1,2 +1,3 @@\n keep\n-old\n+new\n+more" },
            { "filename": "logo.png", "status": "added", "additions": 0, "deletions": 0, "changes": 0 }
        ]
    })
}

async fn server(private: bool) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "alice", "node_id": "U_alice", "type": "User"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("RepositoryDetail"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "repository": {
            "id": "R_widget", "name": "widget", "url": "https://github.com/acme/widget",
            "visibility": if private { "PRIVATE" } else { "PUBLIC" },
            "isFork": false, "viewerPermission": "WRITE",
            "owner": { "__typename": "Organization", "login": "acme", "id": "O_acme" }
        } } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits/0123456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(commit_body()))
        .expect(1)
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_is_fetched_once_then_shown_from_the_cache() {
    let server = server(false).await;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    let uri = server.uri();
    let (table, json_out, offline) = tokio::task::spawn_blocking(move || {
        let table = run(
            &dir,
            Some(&uri),
            &["commit", "acme/widget@0123456", "--patch"],
        );
        let json_out = run(
            &dir,
            None,
            &[
                "commit",
                &format!("https://github.com/acme/widget/commit/{SHA}"),
                "--offline",
                "--format",
                "json",
            ],
        );
        let offline = run(&dir, None, &["commit", "acme/widget@abcdef0", "--offline"]);
        (table, json_out, offline)
    })
    .await
    .unwrap();

    let out = String::from_utf8_lossy(&table.stdout);
    assert!(
        table.status.success(),
        "{}",
        String::from_utf8_lossy(&table.stderr)
    );
    for needle in [
        "acme/widget@0123456 — fix: handle empty input",
        SHA,
        "Alice (alice)",
        "Bob (bob)",
        "1111111",
        "Details here.",
        "modified",
        "src/lib.rs",
        "Total: 2 file(s), +2 −1",
        "@@ -1,2 +1,3 @@",
        "-old",
        "+new",
        "no diff available from GitHub",
    ] {
        assert!(out.contains(needle), "missing {needle:?}:\n{out}");
    }
    assert!(!out.contains('\u{1b}'), "escape sequence in output:\n{out}");

    // The same commit is read from the cache, offline, by full SHA.
    assert!(
        json_out.status.success(),
        "{}",
        String::from_utf8_lossy(&json_out.stderr)
    );
    let v: Value = serde_json::from_slice(&json_out.stdout).unwrap();
    assert_eq!(v["repository"], "acme/widget");
    assert_eq!(v["sha"], SHA);
    assert_eq!(v["source"], "cache");
    assert_eq!(v["is_merge"], false);
    assert_eq!(v["stats"]["files"], 2);
    assert_eq!(v["files"][0]["patch_available"], true);
    assert_eq!(v["files"][1]["patch_available"], false);
    assert!(
        v["files"][0].get("patch").is_none(),
        "diffs only with --patch"
    );

    // A commit that is not cached cannot be shown offline.
    assert_eq!(offline.status.code(), Some(1));
    let err = String::from_utf8_lossy(&offline.stderr);
    assert!(err.contains("not in the cache"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn json_hides_private_commit_content_unless_asked() {
    let server = server(true).await;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().to_path_buf();
    let uri = server.uri();
    let (hidden, shown) = tokio::task::spawn_blocking(move || {
        let hidden = run(
            &dir,
            Some(&uri),
            &[
                "commit",
                "acme/widget@0123456",
                "--patch",
                "--format",
                "json",
            ],
        );
        let shown = run(
            &dir,
            None,
            &[
                "commit",
                "acme/widget@0123456",
                "--offline",
                "--patch",
                "--format",
                "json",
                "--include-private",
            ],
        );
        (hidden, shown)
    })
    .await
    .unwrap();
    assert!(
        hidden.status.success(),
        "{}",
        String::from_utf8_lossy(&hidden.stderr)
    );
    let text = String::from_utf8_lossy(&hidden.stdout);
    assert!(!text.contains("widget"), "{text}");
    assert!(!text.contains("src/lib.rs"), "{text}");
    assert!(!text.contains("handle"), "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["message"], "[private]");
    assert_eq!(v["private_repositories_redacted"], true);
    assert_eq!(v["stats"]["additions"], 2);

    let v: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(v["repository"], "acme/widget");
    assert!(v["files"][0]["patch"].as_str().unwrap().contains("+new"));
}

#[test]
fn invalid_commit_references_exit_with_code_2() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        "acme/widget",
        "acme/widget@xyz",
        "https://github.com/acme/widget/pull/1",
    ] {
        let out = run(d.path(), None, &["commit", bad, "--offline"]);
        assert_eq!(out.status.code(), Some(2), "{bad}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("not a commit"));
    }
}
