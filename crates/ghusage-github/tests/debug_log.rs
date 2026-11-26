//! The debug log of GitHub requests: what it shows to diagnose a slow or
//! failing sync, and what it must never show.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Collects everything the subscriber writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Routes this thread's `ghusage_github` debug events into the returned
/// buffer until the guard is dropped. Tests run on a current-thread runtime,
/// so the client's events are emitted on this thread.
fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
    let out = Captured::default();
    let writer = out.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    (out, tracing::subscriber::set_default(subscriber))
}

fn lines_with<'a>(log: &'a str, needle: &str) -> Vec<&'a str> {
    log.lines().filter(|l| l.contains(needle)).collect()
}

/// Responds with each template in turn, repeating the last one.
struct Sequence {
    calls: std::sync::atomic::AtomicUsize,
    responses: Vec<ResponseTemplate>,
}

impl Respond for Sequence {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.responses[i.min(self.responses.len() - 1)].clone()
    }
}

const fn sequence(responses: Vec<ResponseTemplate>) -> Sequence {
    Sequence {
        calls: std::sync::atomic::AtomicUsize::new(0),
        responses,
    }
}

#[tokio::test]
async fn graphql_requests_name_the_operation_but_not_the_variables() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": null }
        })))
        .mount(&server)
        .await;
    let (log, guard) = capture();
    // A missing repository is an error; only the log matters here.
    client(&server)
        .repository("someowner", "hidden-repo-name")
        .await
        .unwrap_err();
    drop(guard);
    let log = log.text();
    let requests = lines_with(&log, "GitHub request");
    assert_eq!(requests.len(), 1, "{log}");
    assert!(
        requests[0].contains("operation=\"RepositoryDetail\"")
            || requests[0].contains("operation=RepositoryDetail"),
        "{log}"
    );
    assert!(!log.contains("hidden-repo-name"), "{log}");
    assert!(!log.contains("someowner"), "{log}");
    assert!(!log.contains(TOKEN), "{log}");
}

#[tokio::test]
async fn secondary_rate_limit_shows_the_limit_and_message_but_not_the_token() {
    let server = MockServer::start().await;
    let limited = ResponseTemplate::new(403)
        .insert_header("x-ratelimit-remaining", "4321")
        .insert_header("x-ratelimit-resource", "core")
        .insert_header("x-ratelimit-used", "679")
        .set_body_json(json!({
            "message": "You have exceeded a secondary rate limit.\u{1b}]0;pwned\u{7} Please wait.",
            "documentation_url": "https://docs.github.com/rest"
        }));
    Mock::given(path("/user"))
        .respond_with(sequence(vec![
            limited,
            ResponseTemplate::new(200).set_body_json(rest_user("octocat", "U_octocat", "User")),
        ]))
        .mount(&server)
        .await;
    let (log, guard) = capture();
    client(&server).viewer().await.unwrap();
    drop(guard);
    let log = log.text();

    let limited: Vec<&str> = lines_with(&log, "GitHub request")
        .into_iter()
        .filter(|l| l.contains("status=403"))
        .collect();
    assert_eq!(limited.len(), 1, "{log}");
    let line = limited[0];
    assert!(
        line.contains("ratelimit_resource=\"core\"") || line.contains("ratelimit_resource=core"),
        "{log}"
    );
    assert!(
        line.contains("ratelimit_remaining=\"4321\"") || line.contains("ratelimit_remaining=4321"),
        "{log}"
    );
    assert!(
        line.contains("ratelimit_used=\"679\"") || line.contains("ratelimit_used=679"),
        "{log}"
    );
    assert!(line.contains("exceeded a secondary rate limit"), "{log}");

    let retries = lines_with(&log, "retrying GitHub request");
    assert_eq!(retries.len(), 1, "{log}");
    assert!(
        retries[0].contains("wait=\"secondary\"") || retries[0].contains("wait=secondary"),
        "{log}"
    );
    assert!(retries[0].contains("ratelimit_resource"), "{log}");

    assert!(!log.contains(TOKEN), "{log}");
    assert!(!log.to_ascii_lowercase().contains("bearer"), "{log}");
    assert!(
        !log.contains("pwned\u{7}") && !log.contains('\u{1b}'),
        "{log}"
    );
    assert!(
        !log.contains("\\u{1b}"),
        "control characters are removed: {log}"
    );
}

#[tokio::test]
async fn retry_after_and_primary_limits_name_their_wait() {
    let server = MockServer::start().await;
    let retry_after = ResponseTemplate::new(429)
        .insert_header("retry-after", "0")
        .set_body_json(json!({ "message": "slow down" }));
    let primary = ResponseTemplate::new(403)
        .insert_header("x-ratelimit-remaining", "0")
        .insert_header("x-ratelimit-resource", "core")
        .set_body_json(json!({ "message": "API rate limit exceeded" }));
    Mock::given(path("/user"))
        .respond_with(sequence(vec![
            retry_after,
            primary,
            ResponseTemplate::new(200).set_body_json(rest_user("octocat", "U_octocat", "User")),
        ]))
        .mount(&server)
        .await;
    let (log, guard) = capture();
    client(&server).viewer().await.unwrap();
    drop(guard);
    let log = log.text();
    let retries = lines_with(&log, "retrying GitHub request");
    assert_eq!(retries.len(), 2, "{log}");
    assert!(retries[0].contains("retry-after"), "{log}");
    assert!(retries[0].contains("slow down"), "{log}");
    assert!(retries[1].contains("primary"), "{log}");
    assert!(
        lines_with(&log, "status=429")[0].contains("retry_after"),
        "{log}"
    );
}

#[tokio::test]
async fn graphql_rate_limit_logs_only_the_rate_limit_message() {
    let server = MockServer::start().await;
    let limited = ResponseTemplate::new(200).set_body_json(json!({
        "data": { "repository": null },
        "errors": [
            {
                "type": "NOT_FOUND",
                "path": ["repository"],
                "message": "Could not resolve to a Repository with the name 'someowner/hidden-repo-name'."
            },
            { "type": "RATE_LIMITED", "message": "API rate limit exceeded for user ID 1." }
        ]
    }));
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(sequence(vec![
            limited,
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "repository": null } })),
        ]))
        .mount(&server)
        .await;
    let (log, guard) = capture();
    client(&server)
        .repository("someowner", "hidden-repo-name")
        .await
        .unwrap_err();
    drop(guard);
    let log = log.text();
    let retries = lines_with(&log, "retrying GitHub request");
    assert_eq!(retries.len(), 1, "{log}");
    assert!(retries[0].contains("wait=\"primary\""), "{log}");
    assert!(retries[0].contains("API rate limit exceeded"), "{log}");
    assert!(!log.contains("hidden-repo-name"), "{log}");
    assert!(!log.contains("someowner"), "{log}");
}

#[tokio::test]
async fn graphql_requests_show_the_points_they_used() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "rateLimit": { "cost": 3 }, "repository": null }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "octocat", "id": 1, "node_id": "U_1", "type": "User"
        })))
        .mount(&server)
        .await;
    let (log, guard) = capture();
    let c = client(&server);
    c.repository("someowner", "repo").await.unwrap_err();
    c.viewer().await.unwrap();
    drop(guard);
    let log = log.text();
    let requests = lines_with(&log, "GitHub request");
    assert_eq!(requests.len(), 2, "{log}");
    let graphql = lines_with(&log, "/graphql");
    assert!(graphql[0].contains("graphql_cost=3"), "{log}");
    // A REST request has no GraphQL cost.
    let rest = lines_with(&log, "/user");
    assert!(!rest[0].contains("graphql_cost"), "{log}");
    assert_eq!(c.stats().graphql_points, 3);
}
