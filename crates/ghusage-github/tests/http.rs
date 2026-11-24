//! Transport behavior: headers, retries, rate limits, timeouts, size
//! limits, concurrency and error hygiene.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::*;
use ghusage_github::{ApiError, ClientConfig, GitHubClient};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{bearer_token, header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn user_body() -> serde_json::Value {
    rest_user("octocat", "U_octocat", "User")
}

/// Responds with each template in turn, repeating the last one.
struct Sequence {
    calls: AtomicUsize,
    responses: Vec<ResponseTemplate>,
}

impl Sequence {
    const fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            responses,
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses[i.min(self.responses.len() - 1)].clone()
    }
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

#[tokio::test]
async fn requests_carry_auth_and_api_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(bearer_token(TOKEN))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", "2022-11-28"))
        .and(header("user-agent", "ghusage-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(user_body())
                .insert_header("x-oauth-scopes", "repo, read:org"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let viewer = client(&server).viewer().await.unwrap();
    assert_eq!(viewer.login, "octocat");
    assert_eq!(viewer.id, "U_octocat");
    assert_eq!(
        viewer.scopes,
        Some(vec!["repo".to_string(), "read:org".to_string()])
    );
    let stats = client(&server).stats();
    assert_eq!(stats.requests, 0);
}

#[tokio::test]
async fn unauthenticated_client_sends_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/octocat"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/octocat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(user_body()))
        .mount(&server)
        .await;
    let c = GitHubClient::new(None, config(&server)).unwrap();
    let u = c.user("octocat").await.unwrap();
    assert!(!u.is_bot);
}

#[tokio::test]
async fn bot_user_is_classified() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/dependabot[bot]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rest_user(
            "dependabot[bot]",
            "BOT_x",
            "Bot",
        )))
        .mount(&server)
        .await;
    let u = client(&server).user("dependabot[bot]").await.unwrap();
    assert!(u.is_bot);
}

#[tokio::test]
async fn invalid_credentials_are_unauthorized_without_retry() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/rest",
            "status": "401"
        })))
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::Unauthorized), "{err:?}");
    assert!(err.is_auth());
    assert_no_token(&err, &err.to_string());
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn missing_permission_is_forbidden_not_an_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(path("/users/someone"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "Resource not accessible by personal access token",
            "documentation_url": "https://docs.github.com/rest",
            "status": "403"
        })))
        .mount(&server)
        .await;
    let err = client(&server).user("someone").await.unwrap_err();
    assert!(matches!(err, ApiError::Forbidden(_)), "{err:?}");
    assert!(!err.is_auth());
    assert!(err.to_string().contains("Resource not accessible"));
    assert_no_token(&err, &err.to_string());
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn inaccessible_resource_is_not_found_not_an_auth_failure() {
    let server = MockServer::start().await;
    Mock::given(path("/users/ghost-user"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "Not Found",
            "documentation_url": "https://docs.github.com/rest",
            "status": "404"
        })))
        .mount(&server)
        .await;
    let err = client(&server).user("ghost-user").await.unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    assert!(!err.is_auth());
}

#[tokio::test]
async fn secondary_rate_limit_with_retry_after_is_retried() {
    let server = MockServer::start().await;
    let limited = ResponseTemplate::new(403)
        .insert_header("retry-after", "1")
        .set_body_json(json!({
            "message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
            "documentation_url": "https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"
        }));
    Mock::given(path("/user"))
        .respond_with(Sequence::new(vec![
            limited,
            ResponseTemplate::new(200).set_body_json(user_body()),
        ]))
        .mount(&server)
        .await;
    let c = client(&server);
    let started = std::time::Instant::now();
    c.viewer().await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "Retry-After not honored"
    );
    let s = c.stats();
    assert_eq!(s.requests, 2);
    assert_eq!(s.retries, 1);
    assert_eq!(s.rate_limit_waits, 1);
}

#[tokio::test]
async fn secondary_rate_limit_message_without_retry_after_is_retried() {
    let server = MockServer::start().await;
    let limited = ResponseTemplate::new(403).set_body_json(json!({
        "message": "You have exceeded a secondary rate limit and have been temporarily blocked from content creation.",
        "documentation_url": "https://docs.github.com/rest"
    }));
    Mock::given(path("/user"))
        .respond_with(Sequence::new(vec![
            limited,
            ResponseTemplate::new(200).set_body_json(user_body()),
        ]))
        .mount(&server)
        .await;
    let c = client(&server);
    c.viewer().await.unwrap();
    assert_eq!(c.stats().rate_limit_waits, 1);
}

#[tokio::test]
async fn too_many_requests_is_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(429).set_body_json(json!({"message": "Too Many Requests"})),
            ResponseTemplate::new(200).set_body_json(user_body()),
        ]))
        .mount(&server)
        .await;
    let c = client(&server);
    c.viewer().await.unwrap();
    assert_eq!(c.stats().requests, 2);
}

#[tokio::test]
async fn too_many_requests_gives_up_after_max_attempts() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(
            ResponseTemplate::new(429).set_body_json(json!({"message": "Too Many Requests"})),
        )
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::RateLimited { .. }), "{err:?}");
    assert_eq!(request_count(&server).await, 3);
}

#[tokio::test]
async fn primary_rate_limit_beyond_max_wait_fails_immediately() {
    let server = MockServer::start().await;
    let reset = jiff::Timestamp::now().as_second() + 3600;
    Mock::given(path("/user"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("x-ratelimit-limit", "5000")
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-used", "5000")
                .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                .insert_header("x-ratelimit-resource", "core")
                .set_body_json(json!({
                    "message": "API rate limit exceeded for user ID 1.",
                    "documentation_url": "https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"
                })),
        )
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    match &err {
        ApiError::RateLimited { reset_at } => {
            assert_eq!(reset_at.map(jiff::Timestamp::as_second), Some(reset));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert!(!err.is_auth());
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn primary_rate_limit_waits_for_reset_within_max_wait() {
    let server = MockServer::start().await;
    let reset = jiff::Timestamp::now().as_second();
    Mock::given(path("/user"))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(429)
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset", reset.to_string().as_str())
                .set_body_json(json!({"message": "API rate limit exceeded"})),
            ResponseTemplate::new(200).set_body_json(user_body()),
        ]))
        .mount(&server)
        .await;
    let c = client(&server);
    c.viewer().await.unwrap();
    assert_eq!(c.stats().rate_limit_waits, 1);
}

#[tokio::test]
async fn server_errors_are_retried_until_success() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(500),
            ResponseTemplate::new(502),
            ResponseTemplate::new(200).set_body_json(user_body()),
        ]))
        .mount(&server)
        .await;
    let c = client(&server);
    c.viewer().await.unwrap();
    let s = c.stats();
    assert_eq!(s.requests, 3);
    assert_eq!(s.retries, 2);
}

#[tokio::test]
async fn persistent_server_errors_stop_after_max_attempts() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    assert!(
        matches!(
            err,
            ApiError::Http {
                status: 502,
                attempts: 3
            }
        ),
        "{err:?}"
    );
    assert_eq!(request_count(&server).await, 3);
}

#[tokio::test]
async fn timeouts_are_retried_a_bounded_number_of_times() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(user_body())
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let c = client_with(&server, |c| {
        c.request_timeout = Duration::from_millis(50);
        c.max_attempts = 3;
    });
    let err = c.viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::Timeout { attempts: 3 }), "{err:?}");
    assert_eq!(c.stats().requests, 3);
    assert_no_token(&err, &err.to_string());
}

#[tokio::test]
async fn invalid_json_is_an_invalid_response() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string("{\"login\": \"octocat\", "),
        )
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::InvalidResponse(_)), "{err:?}");
}

#[tokio::test]
async fn missing_required_field_is_an_invalid_response() {
    let server = MockServer::start().await;
    Mock::given(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octocat" })))
        .mount(&server)
        .await;
    let err = client(&server).viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::InvalidResponse(_)), "{err:?}");
    assert!(err.to_string().contains("node_id"));
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let server = MockServer::start().await;
    let big = format!("{{\"login\": \"{}\"}}", "x".repeat(10_000));
    Mock::given(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big))
        .mount(&server)
        .await;
    let c = client_with(&server, |c| c.max_response_bytes = 1024);
    let err = c.viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::TooLarge { limit: 1024 }), "{err:?}");
    // Not retried: the same response would be too large again.
    assert_eq!(request_count(&server).await, 1);
}

/// A minimal HTTP/1.1 server. Each connection serves one request: it counts
/// requests in flight, waits `delay`, then writes `response`.
async fn raw_server(
    delay: Duration,
    response: String,
) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let (inf, max) = (in_flight.clone(), max_seen.clone());
    let response = Arc::new(response);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let (inf, max, response) = (inf.clone(), max.clone(), response.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut read = 0;
                loop {
                    let Ok(n) = sock.read(&mut buf[read..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    read += n;
                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let now = inf.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                inf.fetch_sub(1, Ordering::SeqCst);
                drop(sock.write_all(response.as_bytes()).await);
                drop(sock.shutdown().await);
            });
        }
    });
    (format!("http://{addr}"), in_flight, max_seen)
}

fn ok_user() -> String {
    let body = r#"{"login":"octocat","node_id":"U_octo1"}"#;
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn raw_config(base: &str, concurrency: usize) -> ClientConfig {
    ClientConfig {
        api_base: base.to_string(),
        graphql_url: format!("{base}/graphql"),
        concurrency,
        max_attempts: 1,
        request_timeout: Duration::from_secs(5),
        max_rate_limit_wait: Duration::from_secs(1),
        backoff_base: Duration::from_millis(1),
        max_response_bytes: 1024,
        user_agent: "ghusage-test".into(),
    }
}

#[tokio::test]
async fn concurrency_limit_is_respected() {
    let (base, _, max_seen) = raw_server(Duration::from_millis(100), ok_user()).await;
    let client = Arc::new(GitHubClient::new(Some(credentials()), raw_config(&base, 2)).unwrap());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let c = client.clone();
        tasks.spawn(async move { c.user("octocat").await });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap().unwrap();
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 2);
    assert_eq!(client.stats().requests, 8);
}

#[tokio::test]
async fn concurrency_is_clamped_to_at_least_one() {
    let (base, _, max_seen) = raw_server(Duration::from_millis(30), ok_user()).await;
    let client = Arc::new(GitHubClient::new(None, raw_config(&base, 0)).unwrap());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let c = client.clone();
        tasks.spawn(async move { c.user("octocat").await });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap().unwrap();
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn oversized_streamed_body_without_length_is_rejected() {
    // Chunked transfer: no Content-Length, so the limit applies while reading.
    let chunk = format!("{:x}\r\n{}\r\n", 1024, "x".repeat(1024));
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{chunk}{chunk}0\r\n\r\n"
    );
    let (base, _, _) = raw_server(Duration::from_millis(0), chunked).await;
    let client = GitHubClient::new(None, raw_config(&base, 1)).unwrap();
    let err = client.viewer().await.unwrap_err();
    assert!(matches!(err, ApiError::TooLarge { limit: 1024 }), "{err:?}");
}

#[tokio::test]
async fn token_never_appears_in_any_error() {
    let server = MockServer::start().await;
    let cases: Vec<(&str, ResponseTemplate)> = vec![
        (
            "/users/u401",
            ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
        ),
        (
            "/users/u403",
            ResponseTemplate::new(403).set_body_json(json!({"message": "Forbidden"})),
        ),
        (
            "/users/u404",
            ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})),
        ),
        (
            "/users/u422",
            ResponseTemplate::new(422).set_body_json(json!({"message": "Validation Failed"})),
        ),
        ("/users/u500", ResponseTemplate::new(500)),
        ("/users/u429", ResponseTemplate::new(429)),
        (
            "/users/ubad",
            ResponseTemplate::new(200).set_body_string("not json"),
        ),
        (
            "/users/ubig",
            ResponseTemplate::new(200).set_body_string("x".repeat(5000)),
        ),
    ];
    for (p, r) in cases {
        Mock::given(path(p)).respond_with(r).mount(&server).await;
    }
    let c = client_with(&server, |c| c.max_response_bytes = 2048);
    for login in [
        "u401", "u403", "u404", "u422", "u500", "u429", "ubad", "ubig",
    ] {
        let err = c.user(login).await.unwrap_err();
        assert_no_token(&err, &err.to_string());
    }
    // A connection failure.
    let dead = GitHubClient::new(
        Some(credentials()),
        ClientConfig {
            api_base: "http://127.0.0.1:9".into(),
            max_attempts: 1,
            ..config(&server)
        },
    )
    .unwrap();
    let err = dead.viewer().await.unwrap_err();
    assert!(
        matches!(err, ApiError::Network(_) | ApiError::Timeout { .. }),
        "{err:?}"
    );
    assert_no_token(&err, &err.to_string());
}

#[tokio::test]
async fn rate_limit_resources_are_reported() {
    let server = MockServer::start().await;
    Mock::given(path("/rate_limit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resources": {
                "core": { "limit": 5000, "used": 1, "remaining": 4999, "reset": 1_700_000_000 },
                "search": { "limit": 30, "used": 0, "remaining": 30, "reset": 1_700_000_060 },
                "graphql": { "limit": 5000, "used": 7, "remaining": 4993, "reset": 1_700_000_000 }
            },
            "rate": { "limit": 5000, "used": 1, "remaining": 4999, "reset": 1_700_000_000 }
        })))
        .mount(&server)
        .await;
    let r = client(&server).rate_limit().await.unwrap();
    assert_eq!(r.core_remaining, 4999);
    assert_eq!(r.core_limit, 5000);
    assert_eq!(r.core_reset.as_second(), 1_700_000_000);
    assert_eq!(r.search_limit, 30);
    assert_eq!(r.graphql_remaining, 4993);
}
