//! Acceptance tests against the built `ghusage` binary.

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
        "daily", "weekly", "monthly", "repos", "prs", "reviews", "issues", "comments", "tui",
        "sync", "doctor", "pr",
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
    ghusage(&d).args(["--concurrency", "0"]).assert().code(2);
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
