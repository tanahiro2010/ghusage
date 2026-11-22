//! Failure injection against the built binary: API errors, rate limits,
//! malformed responses, concurrent and interrupted syncs, and damaged or
//! unwritable caches. In every case the binary must not report a false
//! success, must keep valid cached data, must be safe to rerun, must explain
//! the problem and must never print the token.

mod support;

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{method, path, query_param, query_param_contains};
use wiremock::{Mock, ResponseTemplate};

fn server_error() -> ResponseTemplate {
    ResponseTemplate::new(500).set_body_json(json!({ "message": "Server Error" }))
}

/// Pull request numbers in an offline `prs` report over the standard period.
async fn cached_prs(env: &Env) -> Vec<u64> {
    let r = env.offline_json(&["prs"]).await;
    r.expect_code(0);
    pr_numbers(&r.json())
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_pull_request_makes_sync_partial_and_a_rerun_completes_it() {
    let world = World::base();
    let server = serve(&world).await;
    pr_detail_mock("acme", "widget", 20, server_error(), 1)
        .mount(&server)
        .await;
    let env = Env::new(&server);

    let r = env.sync(&[]).await;
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "partial");
    assert_eq!(s["pull_requests_fetched"], 2);
    let failures = s["failures"].to_string();
    assert!(failures.contains("acme/widget#20"), "{failures}");
    // Retried before giving up (5 attempts).
    assert_eq!(pr_detail_requests(&requests(&server).await, 20), 5);

    // The other pull requests were stored.
    assert_eq!(cached_prs(&env).await, vec![3, 10]);
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0);
    let o = o.json();
    assert_eq!(coverage_status(&o, "pull_requests"), "partial");
    assert_eq!(coverage_status(&o, "commits"), "complete");
    let reasons = coverage_reasons(&o, "pull_requests").join(" ");
    assert!(reasons.contains("failed"), "reason: {reasons}");
    // Strict mode refuses incomplete data.
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);
    // A table sync explains what is incomplete.
    let t = env.run(&["sync", "--since", SINCE, "--until", UNTIL]).await;
    t.expect_code(4);
    assert!(t.stdout.contains("incomplete"), "{}", t.stdout);
    assert!(t.stdout.contains("acme/widget#20"), "{}", t.stdout);
    // --require-complete turns the sync failure into exit code 6.
    env.sync(&["--require-complete"]).await.expect_code(6);

    // GitHub recovers: only the missing pull request is fetched again.
    reserve(&server, &world).await;
    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "complete");
    assert_eq!(s["pull_requests_fetched"], 1, "{s}");
    let reqs = requests(&server).await;
    assert_eq!(pr_detail_requests(&reqs, 20), 1);
    assert_eq!(pr_detail_requests(&reqs, 10), 0);
    assert_eq!(cached_prs(&env).await, vec![3, 10, 20]);
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(coverage_status(&o, "pull_requests"), "complete");
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(0);
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_502_is_retried_and_the_sync_completes() {
    let world = World::base();
    let server = serve(&world).await;
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>"),
        1,
    )
    .up_to_n_times(2)
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(0);
    let s = r.json();
    assert_eq!(s["status"], "complete");
    assert!(s["api"]["retries"].as_u64().unwrap() >= 2, "{s}");
    assert_eq!(pr_detail_requests(&requests(&server).await, 20), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn search_failing_mid_pagination_keeps_the_existing_cache() {
    let world = World::base();
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = env.offline_json(&["overview"]).await.json();

    // Page 1 of the pull request search links to page 2, which keeps failing.
    let link = format!(
        "<{}/search/issues?page=2&q=continued>; rel=\"next\"",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param("page", "2"))
        .respond_with(server_error())
        .with_priority(1)
        .mount(&server)
        .await;
    let page1: Vec<Value> = world
        .prs
        .iter()
        .map(|p| search_issue(&p.owner, &p.name, p.number, &p.node_id, true, &p.updated_at))
        .collect();
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param_contains("q", format!("involves:{ME} is:pr")))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", link.as_str())
                .set_body_json(json!({
                    "total_count": 150, "incomplete_results": false, "items": page1
                })),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    // Extend the period by two days so the sync has something to fetch.
    let r = env
        .run(&[
            "sync",
            "--since",
            SINCE,
            "--until",
            "2024-03-12",
            "--format",
            "json",
        ])
        .await;
    r.expect_code(4).assert_clean();
    assert!(
        requests(&server)
            .await
            .iter()
            .any(|q| q.url.query().unwrap_or("").contains("page=2")),
        "page 2 was never requested"
    );

    // The previously synced period is untouched.
    let after = env.offline_json(&["overview"]).await.json();
    assert_eq!(before["metrics"], after["metrics"]);
    assert_eq!(coverage_status(&after, "pull_requests"), "complete");
    // The extended period is not reported as complete.
    let ext = env
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--until",
            "2024-03-12",
            "--offline",
            "--format",
            "json",
        ])
        .await;
    ext.expect_code(0);
    assert_ne!(coverage_status(&ext.json(), "pull_requests"), "complete");
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        "2024-03-12",
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);

    // A report command that needs the missing range warns and shows the cache.
    let online = env
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--until",
            "2024-03-12",
            "--format",
            "json",
        ])
        .await;
    online.expect_code(0).assert_clean();
    assert!(
        online.stderr.contains("showing cached data"),
        "{}",
        online.stderr
    );
    assert_eq!(online.json()["metrics"], before["metrics"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_token_during_sync_exits_3_without_leaking_it() {
    let world = World::base();
    let server = serve(&world).await;
    Mock::given(method("GET"))
        .and(path("/search/commits"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials", "documentation_url": "https://docs.github.com/rest"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let mut cmd = env.command(&["sync", "--since", SINCE, "--until", UNTIL]);
    cmd.env("GHUSAGE_LOG", "trace");
    let r = env.run_command(cmd).await;
    r.expect_code(3).assert_clean();
    assert!(
        r.stderr.to_lowercase().contains("authentication")
            || r.stderr.to_lowercase().contains("credentials"),
        "{}",
        r.stderr
    );
    // Report commands do not hide an authentication failure behind the cache.
    env.run(&["overview", "--since", SINCE, "--until", UNTIL])
        .await
        .expect_code(3)
        .assert_clean();
}

/// A pull request in a private repository the token cannot read.
fn private_pr() -> PrFixture {
    let mut p = pr_b();
    p.owner = "acme".into();
    p.name = "secret".into();
    p.number = 4;
    p.node_id = "PR_D".into();
    p.gql["id"] = json!("PR_D");
    p.gql["number"] = json!(4);
    p
}

#[tokio::test(flavor = "multi_thread")]
async fn forbidden_private_repository_is_partial_not_an_auth_error() {
    let http_403 = ResponseTemplate::new(403).set_body_json(json!({
        "message": "Resource not accessible by integration",
        "documentation_url": "https://docs.github.com/graphql"
    }));
    let graphql_forbidden = ResponseTemplate::new(200).set_body_json(json!({
        "data": { "repository": null },
        "errors": [{
            "type": "FORBIDDEN",
            "path": ["repository"],
            "message": "Resource protected by organization SAML enforcement."
        }]
    }));
    for resp in [http_403, graphql_forbidden] {
        let mut world = World::base();
        world.prs.push(private_pr());
        let server = serve(&world).await;
        pr_detail_mock("acme", "secret", 4, resp, 1)
            .mount(&server)
            .await;
        let env = Env::new(&server);
        let r = env.sync(&[]).await;
        r.expect_code(4).assert_clean();
        let s = r.json();
        assert_eq!(s["status"], "partial");
        // The private repository is not named unless asked for.
        assert!(!s.to_string().contains("acme/secret"), "{s}");
        assert!(
            s["failures"].to_string().contains("[private repository]"),
            "{s}"
        );
        assert_eq!(cached_prs(&env).await, vec![3, 10, 20]);
        let named = env.sync(&["--full", "--include-private"]).await.json();
        assert!(
            named["failures"].to_string().contains("acme/secret#4"),
            "{named}"
        );
        let o = env.offline_json(&["overview"]).await.json();
        assert_eq!(coverage_status(&o, "pull_requests"), "partial");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_after_429_is_honoured_and_the_sync_completes() {
    let world = World::base();
    let server = serve(&world).await;
    pr_detail_mock(
        "acme",
        "widget",
        10,
        ResponseTemplate::new(429)
            .insert_header("retry-after", "1")
            .set_body_json(json!({ "message": "Too many requests" })),
        1,
    )
    .up_to_n_times(1)
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let start = Instant::now();
    let r = env.sync(&[]).await;
    r.expect_code(0);
    let s = r.json();
    assert_eq!(s["status"], "complete");
    assert!(s["api"]["rate_limit_waits"].as_u64().unwrap() >= 1, "{s}");
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "Retry-After ignored"
    );
    assert_eq!(pr_detail_requests(&requests(&server).await, 10), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn exhausted_rate_limit_far_in_the_future_fails_fast() {
    let reset = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp();
    // REST search: primary limit exhausted.
    let world = World::base();
    let server = serve(&world).await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-limit", "30")
                .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                .set_body_json(json!({ "message": "API rate limit exceeded" })),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let start = Instant::now();
    let r = env.run(&["sync", "--since", SINCE, "--until", UNTIL]).await;
    assert!(start.elapsed() < Duration::from_secs(20), "sync waited");
    r.expect_code(4).assert_clean();
    assert!(r.stderr.contains("rate limit"), "{}", r.stderr);
    // Nothing is reported as complete.
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);

    // GraphQL: RATE_LIMITED error for one pull request.
    let server = serve(&world).await;
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200)
            .insert_header("x-ratelimit-remaining", "0")
            .insert_header("x-ratelimit-reset", reset.to_string().as_str())
            .set_body_json(json!({
                "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded for user." }]
            })),
        1,
    )
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let start = Instant::now();
    let r = env.sync(&[]).await;
    assert!(start.elapsed() < Duration::from_secs(20), "sync waited");
    r.expect_code(4).assert_clean();
    assert_eq!(r.json()["status"], "partial");
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_failures_leave_the_sync_partial() {
    let world = World::base();
    let server = serve(&world).await;
    // GraphQL goes to a port nothing listens on.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let env = Env::new(&server);
    let mut cmd = env.command(&[
        "sync", "--since", SINCE, "--until", UNTIL, "--format", "json",
    ]);
    cmd.env(
        "GHUSAGE_GRAPHQL_URL",
        format!("http://127.0.0.1:{closed}/graphql"),
    );
    let r = env.run_command(cmd).await;
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "partial");
    assert_eq!(s["pull_requests_fetched"], 0);
    assert!(s["failures"].to_string().contains("connect"), "{s}");
    let o = env.offline_json(&["overview"]).await.json();
    assert_ne!(coverage_status(&o, "pull_requests"), "complete");
    assert_ne!(coverage_status(&o, "issues"), "complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_responses_do_not_crash_and_are_not_complete() {
    let mut world = World::base();
    // PR #3: one commit node is null and one review lacks its id; the rest
    // of the pull request is usable.
    {
        let b = world.pr_mut(3);
        b.gql["commits"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(Value::Null);
        b.gql["reviews"]["nodes"] =
            json!([{ "state": "APPROVED", "submittedAt": "2024-03-06T08:30:00Z" }]);
    }
    // PR #20: required field createdAt missing.
    world
        .pr_mut(20)
        .gql
        .as_object_mut()
        .unwrap()
        .remove("createdAt");
    // A commit search result without a SHA.
    let mut broken = world.commits[0].clone();
    broken.as_object_mut().unwrap().remove("sha");
    world.commits.push(broken);
    let server = serve(&world).await;
    // PR #10: truncated JSON.
    pr_detail_mock(
        "acme",
        "widget",
        10,
        ResponseTemplate::new(200).set_body_raw(
            br#"{"data": {"repository": {"pullRequest": {"id": "PR_A", "#.to_vec(),
            "application/json",
        ),
        1,
    )
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "partial");
    let failures = s["failures"].to_string();
    assert!(failures.contains("acme/widget#10"), "{failures}");
    assert!(failures.contains("acme/widget#20"), "{failures}");
    assert!(failures.contains("me/dotfiles#3"), "{failures}");

    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(coverage_status(&o, "commits"), "partial");
    assert_eq!(coverage_status(&o, "pull_requests"), "partial");
    // PR #3 was stored, but is marked as partially fetched.
    assert_eq!(cached_prs(&env).await, vec![3]);
    let pr = env
        .run(&[
            "pr",
            "https://github.com/me/dotfiles/pull/3",
            "--offline",
            "--format",
            "json",
            "--require-complete",
        ])
        .await;
    pr.expect_code(6);
    assert_eq!(pr.json()["coverage"], "partial");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_unicode_in_responses_is_a_failure_not_a_crash() {
    let world = World::base();
    let server = serve(&world).await;
    // Issue #5: a lone surrogate escape; issue #7: raw invalid UTF-8.
    for (owner, name, number, mangle) in
        [("me", "dotfiles", 5u64, 0u8), ("acme", "widget", 7u64, 1u8)]
    {
        let issue = world.issues.iter().find(|i| i.number == number).unwrap();
        let mut gql = issue.gql.clone();
        gql["title"] = json!("PLACEHOLDER");
        let mut repo = if owner == "me" {
            dotfiles().gql
        } else {
            widget().gql
        };
        repo["issueOrPullRequest"] = gql;
        let text = json!({ "data": { "repository": repo } }).to_string();
        let body: Vec<u8> = if mangle == 0 {
            text.replace("PLACEHOLDER", "bad \\ud800 title")
                .into_bytes()
        } else {
            let mut b = Vec::new();
            for part in text
                .split("PLACEHOLDER")
                .collect::<Vec<_>>()
                .join("\u{1}")
                .bytes()
            {
                if part == 1 {
                    b.extend_from_slice(&[0xff, 0xfe, 0x80]);
                } else {
                    b.push(part);
                }
            }
            b
        };
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(wiremock::matchers::body_string_contains(
                "query IssueDetail",
            ))
            .and(wiremock::matchers::body_partial_json(json!({
                "variables": { "owner": owner, "name": name, "number": number }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["issues_fetched"], 0);
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(coverage_status(&o, "issues"), "partial");
    assert_eq!(coverage_status(&o, "pull_requests"), "complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_sync_is_refused_while_one_is_running() {
    let world = World::base();
    let server = serve(&world).await;
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200)
            .set_body_json(world.pr_body(&world.prs[2]))
            .set_delay(Duration::from_secs(3)),
        1,
    )
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let first = env
        .command(&["sync", "--since", SINCE, "--until", UNTIL])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The lock is held once the first sync starts searching.
    let started = wait_until(Duration::from_secs(10), || async {
        requests(&server)
            .await
            .iter()
            .any(|r| r.url.path() == "/search/commits")
    })
    .await;
    assert!(started, "first sync never started");

    let second = env.run(&["sync", "--since", SINCE, "--until", UNTIL]).await;
    second.expect_code(5).assert_clean();
    assert!(
        second.stderr.contains("already running"),
        "{}",
        second.stderr
    );
    // Clearing the cache under a running sync is refused too.
    let clear = env.run(&["cache", "clear"]).await;
    clear.expect_code(5).assert_clean();
    assert!(clear.stderr.contains("already running"), "{}", clear.stderr);

    let out = tokio::task::spawn_blocking(move || first.wait_with_output().unwrap())
        .await
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // After the first one finishes, syncing works again.
    env.sync(&[]).await.expect_code(0);
}

#[tokio::test(flavor = "multi_thread")]
async fn killed_sync_leaves_a_consistent_cache_and_a_rerun_completes() {
    let world = World::base();
    let server = serve(&world).await;
    // The third pull request (fetched last with concurrency 1) hangs.
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200)
            .set_body_json(world.pr_body(&world.prs[2]))
            .set_delay(Duration::from_secs(30)),
        1,
    )
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let mut child = env
        .command(&[
            "sync",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--concurrency",
            "1",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let reached = wait_until(Duration::from_secs(15), || async {
        pr_detail_requests(&requests(&server).await, 20) > 0
    })
    .await;
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(reached, "sync never reached the last pull request");

    // The pull requests fetched before the kill are stored and the
    // database is intact.
    let st = env.run(&["cache", "status", "--format", "json"]).await;
    st.expect_code(0).assert_clean();
    let st = st.json();
    assert_eq!(st["pull_requests"], 2, "{st}");
    let doc = env.run(&["doctor", "--offline", "--format", "json"]).await;
    doc.assert_clean();
    let checks = doc.json()["checks"].clone();
    assert!(
        checks
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["message"] == "cache integrity: ok" && c["ok"] == true),
        "{checks}"
    );
    // Not reported as complete.
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);

    // Rerun after GitHub responds normally: completes and matches a clean run.
    reserve(&server, &world).await;
    let r = env.sync(&[]).await;
    r.expect_code(0);
    assert_eq!(r.json()["pull_requests_fetched"], 1);
    let resumed = env.offline_json(&["overview"]).await.json();

    let clean = Env::new(&server);
    clean.sync(&[]).await.expect_code(0);
    let fresh = clean.offline_json(&["overview"]).await.json();
    assert_eq!(resumed["metrics"], fresh["metrics"]);
    assert_eq!(coverage_status(&resumed, "pull_requests"), "complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_cache_is_reported_as_a_cache_error() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    let garbage: Vec<u8> = (0..64 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(env.db_path(), &garbage).unwrap();

    for args in [
        vec!["cache", "status"],
        vec!["overview", "--offline"],
        vec!["sync", "--since", SINCE, "--until", UNTIL],
    ] {
        let r = env.run(&args).await;
        r.expect_code(5).assert_clean();
        assert!(r.stderr.contains("corrupt"), "{args:?}: {}", r.stderr);
    }
    // The damaged file is left alone for the user to inspect or delete.
    assert_eq!(std::fs::read(env.db_path()).unwrap(), garbage);
}

/// Whether permission bits are enforced for this process (not root).
#[cfg(unix)]
fn permissions_enforced(file: &std::path::Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(file).is_err()
}

/// Says loudly that a permission test checked nothing. As root (or with a
/// capability that bypasses file modes) a read-only cache cannot be
/// simulated, so the test is skipped rather than reported as verified.
/// The message goes to stderr, which `cargo test -- --nocapture` shows; set
/// `GHUSAGE_REQUIRE_PERMISSION_TESTS=1` to turn the skip into a failure.
#[cfg(unix)]
fn report_skipped(test: &str) {
    let msg = format!(
        "SKIPPED {test}: file permissions are not enforced for this user \
         (running as root?); nothing was verified"
    );
    if std::env::var_os("GHUSAGE_REQUIRE_PERMISSION_TESTS").is_some_and(|v| !v.is_empty()) {
        panic!("{msg}");
    }
    eprintln!("{msg}");
}

#[cfg(unix)]
fn set_mode(p: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn read_only_cache_is_a_cache_error_and_data_survives() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = env.offline_json(&["overview"]).await.json();
    let size_before = std::fs::metadata(env.db_path()).unwrap().len();

    set_mode(&env.db_path(), 0o444);
    set_mode(env.data_dir(), 0o555);
    if !permissions_enforced(&env.db_path()) {
        set_mode(env.data_dir(), 0o755);
        set_mode(&env.db_path(), 0o644);
        report_skipped("read_only_cache_is_a_cache_error_and_data_survives");
        return;
    }
    let mut results = Vec::new();
    for args in [
        vec!["sync", "--since", SINCE, "--until", UNTIL, "--full"],
        vec!["cache", "clear"],
    ] {
        let r = env.run(&args).await;
        results.push((args, r));
    }
    // Restore before asserting so the temp dir can be cleaned up.
    set_mode(env.data_dir(), 0o755);
    set_mode(&env.db_path(), 0o644);
    for (args, r) in &results {
        r.assert_clean();
        assert_eq!(r.code, Some(5), "{args:?}: {}{}", r.stdout, r.stderr);
        let msg = r.stderr.to_lowercase();
        assert!(
            msg.contains("read-only")
                || msg.contains("readonly")
                || msg.contains("permission")
                || msg.contains("unable to open"),
            "{args:?}: not understandable: {}",
            r.stderr
        );
    }
    assert_eq!(std::fs::metadata(env.db_path()).unwrap().len(), size_before);
    let after = env.offline_json(&["overview"]).await.json();
    assert_eq!(before["metrics"], after["metrics"]);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn reading_a_read_only_cache_works_offline() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = env.offline_json(&["overview"]).await.json();
    set_mode(&env.db_path(), 0o444);
    set_mode(env.data_dir(), 0o555);
    let enforced = permissions_enforced(&env.db_path());
    let r = env.offline_json(&["overview"]).await;
    let st = env.run(&["cache", "status"]).await;
    set_mode(env.data_dir(), 0o755);
    set_mode(&env.db_path(), 0o644);
    if !enforced {
        report_skipped("reading_a_read_only_cache_works_offline");
        return;
    }
    r.assert_clean();
    st.assert_clean();
    // Reading needs no write access: the cached data is shown unchanged.
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(r.json()["metrics"], before["metrics"]);
    assert_eq!(st.code, Some(0), "{}", st.stderr);
}

/// Runs `ghusage` with a tiny file size limit, so every database write
/// fails as it would on a full disk (SIGXFSZ ignored => EFBIG).
#[cfg(unix)]
async fn run_with_file_size_limit(env: &Env, args: &[&str]) -> Run {
    let mut c = std::process::Command::new("/bin/sh");
    env.configure(&mut c);
    c.arg("-c")
        .arg("trap '' XFSZ; ulimit -f 8 || exit 99; exec \"$0\" \"$@\"")
        .arg(bin())
        .args(args);
    env.run_command(c).await
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn failing_database_writes_are_cache_errors_and_keep_data() {
    let world = World::base();
    let server = serve(&world).await;

    // A brand-new cache cannot even be created.
    let fresh = Env::new(&server);
    let r = run_with_file_size_limit(&fresh, &["sync", "--since", SINCE, "--until", UNTIL]).await;
    assert_ne!(r.code, Some(99), "ulimit unsupported");
    r.assert_clean();
    assert_eq!(r.code, Some(5), "{}{}", r.stdout, r.stderr);

    // An existing cache: a refetch that cannot write fails cleanly...
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = env.offline_json(&["overview"]).await.json();
    let r = run_with_file_size_limit(
        &env,
        &["sync", "--since", SINCE, "--until", UNTIL, "--full"],
    )
    .await;
    r.assert_clean();
    assert_eq!(r.code, Some(5), "{}{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("cache"), "{}", r.stderr);

    // ...and the cache is intact afterwards.
    let doc = env.run(&["doctor", "--offline", "--format", "json"]).await;
    assert!(
        doc.json()["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["message"] == "cache integrity: ok"),
        "{}",
        doc.stdout
    );
    let after = env.offline_json(&["overview"]).await.json();
    assert_eq!(before["metrics"], after["metrics"]);
}

#[tokio::test(flavor = "multi_thread")]

async fn failing_issue_history_page_is_not_reported_complete() {
    let mut world = World::base();
    // acme/widget#7 has a second page of timeline items that keeps failing.
    let i2 = world.issues.iter_mut().find(|i| i.number == 7).unwrap();
    i2.gql["timelineItems"]["pageInfo"] = page_info(true, Some("cursor-1"));
    let server = serve(&world).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(wiremock::matchers::body_string_contains(
            "query IssueTimelinePage",
        ))
        .respond_with(server_error())
        .with_priority(1)
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    let pages = requests(&server)
        .await
        .iter()
        .filter(|q| body_str(q).contains("query IssueTimelinePage"))
        .count();
    assert!(pages > 0, "the second history page was never requested");
    r.assert_clean();
    let o = env.offline_json(&["overview"]).await.json();
    assert_ne!(
        coverage_status(&o, "issues"),
        "complete",
        "issue history is missing a page"
    );
    assert_eq!(r.code, Some(4), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn request_timeout_makes_sync_partial_and_a_rerun_completes_it() {
    let world = World::base();
    let server = serve(&world).await;
    // PR #20's detail answers only after 5 s; the client gives up on a
    // request after 1 s. The limit is far above how long the mock server
    // takes for every other request, even on a slow runner, so only #20
    // times out.
    let slow = world.pr_body(world.prs.iter().find(|p| p.number == 20).unwrap());
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200)
            .set_body_json(slow)
            .set_delay(Duration::from_secs(5)),
        1,
    )
    .mount(&server)
    .await;
    let env = Env::new(&server);
    let timed_sync = || {
        let mut c = env.command(&[
            "sync", "--since", SINCE, "--until", UNTIL, "--format", "json",
        ]);
        c.env("GHUSAGE_TIMEOUT_MS", "1000");
        c
    };

    let start = Instant::now();
    let r = env.run_command(timed_sync()).await;
    // 5 attempts x 1 s plus small backoffs: far below a single default
    // 30 s timeout, so the setting took effect.
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "partial");
    // #3 and #10 were fetched; #20 timed out.
    assert_eq!(s["pull_requests_fetched"], 2, "{s}");
    let failures = s["failures"].to_string();
    assert!(failures.contains("acme/widget#20"), "{failures}");
    assert!(failures.contains("timed out"), "{failures}");
    // The client tries one request at most 5 times (its default attempt
    // limit): 4 retries, all for #20. The server may not see the last
    // attempt, because a request can time out before it arrives on a slow
    // machine, but it never sees more. The other pull requests need a single
    // request each.
    assert_eq!(s["api"]["retries"], 4, "{s}");
    let reqs = requests(&server).await;
    assert!((1..=5).contains(&pr_detail_requests(&reqs, 20)));
    assert_eq!(pr_detail_requests(&reqs, 10), 1);
    assert_eq!(pr_detail_requests(&reqs, 3), 1);

    // What did arrive is stored and the gap is visible.
    assert_eq!(cached_prs(&env).await, vec![3, 10]);
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0).assert_clean();
    let o = o.json();
    assert_eq!(coverage_status(&o, "pull_requests"), "partial");
    assert_eq!(coverage_status(&o, "commits"), "complete");
    // Commits are unaffected: search sha 1, 2, 3 plus my pull request
    // commits sha 11, 12 (#10) and 21 (#3) = 6, as after a clean sync.
    assert_eq!(o["metrics"]["commits"], 6);
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);

    // GitHub answers promptly again: the rerun fills the gap.
    reserve(&server, &world).await;
    let r = env.run_command(timed_sync()).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "complete");
    assert_eq!(s["pull_requests_fetched"], 1, "{s}");
    assert_eq!(cached_prs(&env).await, vec![3, 10, 20]);
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(0);
}
