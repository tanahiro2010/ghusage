//! Acceptance tests against the built `ghusage` binary.

mod support;

use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SENTINEL_TOKEN: &str = "ghp_sentinelTOKENvalue1234567890abcdef";

/// A command isolated from the developer's environment: a private data
/// directory, no inherited GitHub tokens and no `gh` on PATH.
fn ghusage(data: &tempfile::TempDir) -> Command {
    let mut c = Command::cargo_bin("ghusage").unwrap();
    c.env_clear();
    keep_system_env(&mut c);
    c.env("PATH", data.path().join("no-bin"))
        .env("GHUSAGE_DATA_DIR", data.path())
        .env("TZ", "UTC")
        .env("GHUSAGE_BACKOFF_MS", "1");
    c
}

#[test]
fn help_and_version_succeed() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("Exit codes"));
    ghusage(&d)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("ghusage "));
    for sub in [
        "daily", "weekly", "monthly", "yearly", "repos", "prs", "reviews", "issues", "comments",
        "tui", "sync", "doctor", "pr", "commit",
    ] {
        ghusage(&d).args([sub, "--help"]).assert().success();
    }
    ghusage(&d)
        .args(["auth", "status", "--help"])
        .assert()
        .success();
    ghusage(&d)
        .args(["cache", "status", "--help"])
        .assert()
        .success();
}

#[test]
fn invalid_arguments_exit_with_code_2() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d).args(["--format", "xml"]).assert().code(2);
    ghusage(&d).arg("no-such-command").assert().code(2);
    ghusage(&d)
        .args(["--since", "someday", "--offline"])
        .assert()
        .code(2);
    ghusage(&d)
        .args(["--timezone", "Mars/Olympus", "--offline"])
        .assert()
        .code(2);
    // Concurrent requests are limited to 1..=16, however they are given.
    ghusage(&d).args(["--jobs", "0"]).assert().code(2);
    ghusage(&d).args(["-j", "17"]).assert().code(2);
    ghusage(&d).args(["--concurrency", "17"]).assert().code(2);
    ghusage(&d)
        .env("GHUSAGE_JOBS", "0")
        .args(["cache", "status"])
        .assert()
        .code(2);
    ghusage(&d)
        .args(["pr", "https://example.com/o/r/pull/1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not a pull request URL"));
    ghusage(&d)
        .args(["pr", "https://github.com/o/r/issues/1"])
        .assert()
        .code(2);
    ghusage(&d)
        .args([
            "--since",
            "2026-11-10",
            "--until",
            "2026-11-01",
            "--offline",
        ])
        .assert()
        .code(2);
}

#[test]
fn missing_credentials_is_an_authentication_error() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d)
        .args(["auth", "status"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no GitHub credentials"));
}

#[test]
fn offline_without_cached_user_explains_what_to_do() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d)
        .args(["daily", "--offline"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("ghusage sync"));
}

#[test]
fn cache_status_and_clear_work_on_an_empty_cache() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d)
        .args(["cache", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("last sync: never"));
    let out = ghusage(&d)
        .args(["cache", "status", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["events"], 0);
    ghusage(&d)
        .args(["cache", "clear"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Credentials were not changed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_token_exits_3_and_never_prints_the_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;
    let d = tempfile::tempdir().unwrap();
    let uri = server.uri();
    let d_path = d.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        let mut c = Command::cargo_bin("ghusage").unwrap();
        c.env_clear();
        keep_system_env(&mut c);
        c.env("PATH", d_path.join("no-bin"))
            .env("GHUSAGE_DATA_DIR", &d_path)
            .env("GH_TOKEN", SENTINEL_TOKEN)
            .env("GHUSAGE_API_URL", uri)
            .env("GHUSAGE_LOG", "trace")
            .args(["auth", "status"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!all.contains(SENTINEL_TOKEN), "token leaked: {all}");
    assert!(!all.contains("sentinelTOKEN"));
}

#[test]
fn csv_is_rejected_for_commands_without_tables() {
    let d = tempfile::tempdir().unwrap();
    for args in [["cache", "status"], ["doctor", "--offline"]] {
        ghusage(&d)
            .args(args)
            .args(["--format", "csv"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("--format csv is not available"));
    }
}

#[test]
fn options_a_command_does_not_use_are_reported() {
    let d = tempfile::tempdir().unwrap();
    ghusage(&d)
        .args(["cache", "status", "--repo", "a/b", "--since", "7d"])
        .args(["--commit-date", "author"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "warning: --repo has no effect on `cache status`",
        ))
        .stderr(predicate::str::contains(
            "warning: --since has no effect on `cache status`",
        ))
        .stderr(predicate::str::contains(
            "warning: --commit-date has no effect on `cache status`",
        ));
    ghusage(&d)
        .args(["doctor", "--offline", "--visibility", "public"])
        .assert()
        .stderr(predicate::str::contains(
            "warning: --visibility has no effect on `doctor`",
        ));
    // Options the command uses, and defaults, produce no warning.
    ghusage(&d)
        .args(["cache", "status", "--timezone", "Asia/Tokyo"])
        .assert()
        .success()
        .stderr(predicate::str::contains("has no effect").not());
}

/// Environment variables the OS needs to start a process with networking
/// (Windows cannot initialize sockets without them).
fn keep_system_env(c: &mut Command) {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn api_error_messages_are_sanitized_on_stderr() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "message": "\u{1b}]0;pwned\u{7}\u{1b}[2Jdenied",
        })))
        .mount(&server)
        .await;
    let d = tempfile::tempdir().unwrap();
    let uri = server.uri();
    let d_path = d.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        let mut c = Command::cargo_bin("ghusage").unwrap();
        c.env_clear();
        keep_system_env(&mut c);
        c.env("PATH", d_path.join("no-bin"))
            .env("GHUSAGE_DATA_DIR", &d_path)
            .env("GH_TOKEN", SENTINEL_TOKEN)
            .env("GHUSAGE_API_URL", uri)
            .env("GHUSAGE_BACKOFF_MS", "1");
        c.args(["auth", "status"]).output().unwrap()
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0));
    assert!(stderr.contains("denied"), "{stderr}");
    assert!(
        !stderr.chars().any(|c| c == '\u{1b}' || c == '\u{7}'),
        "raw control characters on stderr: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------
// Online account checks, colors, cache clearing and time zones, run against
// the mocked GitHub of `support::World::base`.
// ---------------------------------------------------------------------------

/// `/user` and `/rate_limit` both reject the token, as GitHub does for a
/// revoked token.
async fn rejecting_server() -> MockServer {
    let server = MockServer::start().await;
    for p in ["/user", "/rate_limit"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "message": "Bad credentials",
                "documentation_url": "https://docs.github.com/rest"
            })))
            .mount(&server)
            .await;
    }
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_status_reports_the_user_and_rate_limit() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);

    // The world's `/user` is `me`; `/rate_limit` reports core 4999/5000,
    // search 30/30 and graphql 5000/5000 with every reset at 4102444800
    // (= 47482 days after 1970-01-01 = 2100-01-01T00:00:00Z).
    let t = env.run(&["auth", "status"]).await;
    t.expect_code(0).assert_clean();
    assert!(
        t.stdout
            .contains("Logged in as me (credentials from GH_TOKEN)"),
        "{}",
        t.stdout
    );
    assert!(
        t.stdout.contains(
            "rate limit: core 4999/5000 (resets 2100-01-01T00:00:00+00:00), search 30/30, graphql 5000/5000"
        ),
        "{}",
        t.stdout
    );

    let j = env.run(&["auth", "status", "--format", "json"]).await;
    j.expect_code(0).assert_clean();
    let v = j.json();
    assert_eq!(v["authenticated"], true);
    assert_eq!(v["source"], "GH_TOKEN");
    assert_eq!(v["login"], support::ME);
    assert_eq!(v["id"], support::ME_ID);
    // The mock sends no X-OAuth-Scopes header (as for fine-grained tokens).
    assert_eq!(v["scopes"], serde_json::Value::Null);
    let rl = &v["rate_limit"];
    assert_eq!(rl["core_remaining"], 4999);
    assert_eq!(rl["core_limit"], 5000);
    assert_eq!(rl["search_remaining"], 30);
    assert_eq!(rl["search_limit"], 30);
    assert_eq!(rl["graphql_remaining"], 5000);
    assert_eq!(rl["graphql_limit"], 5000);
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_online_checks_credentials_and_rate_limit() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);
    let r = env.run(&["doctor", "--format", "json"]).await;
    r.expect_code(0).assert_clean();
    let v = r.json();
    assert_eq!(v["ok"], true, "{v}");
    let messages: Vec<String> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            assert_eq!(c["ok"], true, "{c}");
            c["message"].as_str().unwrap().to_string()
        })
        .collect();
    for expected in [
        "cache integrity: ok",
        "credentials found: GH_TOKEN",
        "authenticated as me",
        "rate limit: core 4999/5000, search 30/30, graphql 5000/5000",
    ] {
        assert!(
            messages.iter().any(|m| m == expected),
            "missing {expected:?} in {messages:?}"
        );
    }

    // A rejected token: the authentication check fails and so does doctor.
    let bad = rejecting_server().await;
    let env = support::Env::new(&bad);
    let t = env.run(&["doctor"]).await;
    t.expect_code(1).assert_clean();
    assert!(
        t.stdout
            .lines()
            .any(|l| l.starts_with("FAIL authentication:")),
        "{}",
        t.stdout
    );
    assert!(t.stdout.contains("ok   credentials found: GH_TOKEN"));
    let j = env.run(&["doctor", "--format", "json"]).await;
    j.expect_code(1).assert_clean();
    assert_eq!(j.json()["ok"], false);
}

/// Every table command over the synced world, with options that keep them
/// offline.
fn table_commands() -> Vec<Vec<&'static str>> {
    let period = ["--since", support::SINCE, "--until", support::UNTIL];
    let mut cmds: Vec<Vec<&str>> = [
        "overview", "daily", "weekly", "monthly", "yearly", "repos", "prs", "reviews", "comments",
        "issues",
    ]
    .iter()
    .map(|c| {
        let mut v = vec![*c];
        v.extend_from_slice(&period);
        v.push("--offline");
        v
    })
    .collect();
    cmds.push(vec![
        "pr",
        "https://github.com/acme/widget/pull/10",
        "--timeline",
        "--offline",
    ]);
    cmds.push(vec!["prs", "--details", "--offline"]);
    cmds.push(vec!["cache", "status"]);
    cmds.push(vec!["doctor", "--offline"]);
    cmds
}

#[tokio::test(flavor = "multi_thread")]
async fn piped_output_has_no_escape_sequences_without_no_color() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);
    // A table sync is piped too.
    let mut c = env.command(&["sync", "--since", support::SINCE, "--until", support::UNTIL]);
    c.env_remove("NO_COLOR");
    let r = env.run_command(c).await;
    r.expect_code(0).assert_clean();
    assert!(!r.all_output().contains('\u{1b}'), "{:?}", r.all_output());

    for args in table_commands() {
        let mut c = env.command(&args);
        c.env_remove("NO_COLOR");
        let r = env.run_command(c).await;
        r.assert_clean();
        assert_eq!(r.code, Some(0), "{args:?}: {}", r.stderr);
        assert!(!r.stdout.is_empty(), "{args:?} printed nothing");
        assert!(
            !r.all_output().contains('\u{1b}'),
            "{args:?} wrote escape sequences into a pipe: {:?}",
            r.all_output()
        );
    }
}

/// Runs a prepared command with stdout on a fresh pseudo-terminal and
/// returns what it wrote there.
#[cfg(unix)]
fn run_on_tty(mut c: std::process::Command) -> (Option<i32>, String) {
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty writes two new descriptors into the out-pointers;
    // name, termios and window size are optional and left null.
    let rc = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty failed");
    // SAFETY: both descriptors were just created and are owned only here.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut reader = std::fs::File::from(master);
    let collector = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        // EOF, or EIO once the last slave descriptor is closed.
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    });
    c.stdout(std::process::Stdio::from(slave))
        .stderr(std::process::Stdio::piped());
    let mut child = c.spawn().unwrap();
    // Close the parent's copy of the slave so the reader sees the end.
    drop(c);
    let status = child.wait().unwrap();
    let out = collector.join().unwrap();
    (status.code(), String::from_utf8_lossy(&out).into_owned())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn colors_follow_tty_no_color_env_and_flag() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let args = [
        "overview",
        "--since",
        support::SINCE,
        "--until",
        support::UNTIL,
        "--offline",
    ];
    // (description, NO_COLOR value or None to unset, --no-color, colored?)
    // NO_COLOR only counts when it is non-empty (no-color.org).
    let cases: [(&str, Option<&str>, bool, bool); 4] = [
        ("terminal", None, false, true),
        ("NO_COLOR=1", Some("1"), false, false),
        ("empty NO_COLOR", Some(""), false, true),
        ("--no-color", None, true, false),
    ];
    for (what, no_color, flag, colored) in cases {
        let mut c = env.command(&args);
        c.env_remove("NO_COLOR");
        if let Some(v) = no_color {
            c.env("NO_COLOR", v);
        }
        if flag {
            c.arg("--no-color");
        }
        let (code, out) = tokio::task::spawn_blocking(move || run_on_tty(c))
            .await
            .unwrap();
        assert_eq!(code, Some(0), "{what}: {out}");
        assert!(out.contains("Commits"), "{what}: no report: {out:?}");
        assert!(!out.contains(SENTINEL_TOKEN));
        assert_eq!(
            out.contains("\u{1b}["),
            colored,
            "{what}: expected colored={colored}: {out:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_clear_keeps_the_token_file_and_auth_works() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);
    // The token lives in a file inside the data directory itself.
    let token_file = env.data_dir().join("token");
    std::fs::write(&token_file, format!("{}\n", support::SENTINEL_TOKEN)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tf = token_file.to_str().unwrap().to_string();
    let with_file = |args: &[&str]| {
        let mut c = env.command(args);
        c.env_remove("GH_TOKEN").arg("--token-file").arg(&tf);
        c
    };
    let r = env
        .run_command(with_file(&[
            "sync",
            "--since",
            support::SINCE,
            "--until",
            support::UNTIL,
        ]))
        .await;
    r.expect_code(0).assert_clean();
    let before = env.run(&["cache", "status", "--format", "json"]).await;
    assert!(before.json()["events"].as_u64().unwrap() > 0);

    let mut c = env.command(&["cache", "clear"]);
    c.env_remove("GH_TOKEN");
    let r = env.run_command(c).await;
    r.expect_code(0).assert_clean();
    assert!(r.stdout.contains("Credentials were not changed"));
    let after = env.run(&["cache", "status", "--format", "json"]).await;
    assert_eq!(after.json()["events"], 0);

    // The token file is untouched...
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap(),
        format!("{}\n", support::SENTINEL_TOKEN)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // ...and still authenticates.
    let a = env.run_command(with_file(&["auth", "status"])).await;
    a.expect_code(0).assert_clean();
    assert!(
        a.stdout.contains(&format!(
            "Logged in as me (credentials from token file {tf})"
        )),
        "{}",
        a.stdout
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn timezone_option_overrides_tz_environment() {
    let server = support::serve(&support::World::base()).await;
    let env = support::Env::new(&server);
    // Sync a day more on each side so both zones' periods are covered.
    env.run(&[
        "sync",
        "--since",
        "2024-03-03",
        "--until",
        "2024-03-11",
        "--timezone",
        "UTC",
    ])
    .await
    .expect_code(0);

    let daily = |explicit: Option<&str>| {
        let mut c = env.command(&[
            "daily",
            "--since",
            support::SINCE,
            "--until",
            support::UNTIL,
            "--offline",
            "--format",
            "json",
        ]);
        c.env("TZ", "Asia/Tokyo");
        if let Some(z) = explicit {
            c.args(["--timezone", z]);
        }
        c
    };
    let commits_by_day = |v: &serde_json::Value| -> Vec<(String, u64)> {
        v["activity_breakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["bucket"]["key"].as_str().unwrap().to_string(),
                    b["summary"]["commits"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    let days = |counts: [u64; 7]| -> Vec<(String, u64)> {
        (4..=10)
            .zip(counts)
            .map(|(d, n)| (format!("2024-03-{d:02}"), n))
            .collect()
    };

    // My commits: sha 1 03-04T10:00Z, sha 11 03-04T08:30Z, sha 2
    // 03-05T12:00Z, sha 12 03-05T09:30Z, sha 21 03-06T07:50Z and sha 3
    // 03-07T23:30Z. In Tokyo (UTC+9) only sha 3 changes day: 03-08 08:30.
    let tokyo = env.run_command(daily(None)).await;
    tokyo.expect_code(0).assert_clean();
    let tokyo = tokyo.json();
    assert_eq!(tokyo["timezone"], "Asia/Tokyo", "TZ is the default zone");
    assert_eq!(tokyo["period"]["start"], "2024-03-04T00:00:00+09:00");
    assert_eq!(commits_by_day(&tokyo), days([2, 2, 1, 0, 1, 0, 0]));

    // --timezone wins over TZ: sha 3 stays on 03-07.
    let utc = env.run_command(daily(Some("UTC"))).await;
    utc.expect_code(0).assert_clean();
    let utc = utc.json();
    assert_eq!(utc["timezone"], "UTC");
    assert_eq!(utc["period"]["start"], "2024-03-04T00:00:00+00:00");
    assert_eq!(commits_by_day(&utc), days([2, 2, 1, 1, 0, 0, 0]));
}

#[test]
fn concurrent_requests_come_from_flag_alias_or_environment() {
    let d = tempfile::tempdir().unwrap();
    for args in [
        &["-j", "4"][..],
        &["--jobs", "4"],
        &["--jobs=16"],
        // The former name keeps working.
        &["--concurrency", "4"],
    ] {
        ghusage(&d)
            .args(args)
            .args(["cache", "status"])
            .assert()
            .success();
    }
    ghusage(&d)
        .env("GHUSAGE_JOBS", "4")
        .args(["cache", "status"])
        .assert()
        .success();
    ghusage(&d)
        .env("GHUSAGE_JOBS", "2")
        .args(["-j", "4", "cache", "status"])
        .assert()
        .success();
    // An invalid environment value is reported even next to the flag.
    ghusage(&d)
        .env("GHUSAGE_JOBS", "99")
        .args(["-j", "4", "cache", "status"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--jobs"));
}
