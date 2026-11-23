//! HTTP transport: authentication headers, concurrency limit, timeouts,
//! retries with backoff, rate-limit handling, response size limit and
//! REST `Link` pagination.
//!
//! Error values produced here never contain request headers or the token.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::activity::{self, Activity};
use crate::{ApiError, ApiStats, ClientConfig, Credentials, Secret};

/// Longest backoff between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Upper bound on followed REST pages, as a guard against link loops.
const MAX_PAGES: usize = 10_000;

/// A successful (2xx) response.
#[derive(Debug)]
pub(crate) struct RawResponse {
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// A GraphQL response: `data` may be present together with `errors`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct GraphqlResponse {
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub errors: Vec<GraphqlError>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct GraphqlError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub path: Vec<Value>,
}

impl GraphqlError {
    pub fn message(&self) -> String {
        truncate(self.message.as_deref().unwrap_or("unknown error"), 300)
    }

    /// Path elements that are field names (array indices dropped).
    pub fn field_path(&self) -> Vec<&str> {
        self.path.iter().filter_map(|p| p.as_str()).collect()
    }
}

/// Outcome of one attempt that produced an HTTP response.
struct Attempted {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

enum AttemptError {
    Timeout,
    Network(String),
    TooLarge,
}

enum Decision {
    Done(RawResponse),
    Retry {
        delay: Duration,
        error: ApiError,
        rate_limit: bool,
    },
    Fail(ApiError),
}

pub(crate) struct Transport {
    http: reqwest::Client,
    token: Option<Secret>,
    pub config: ClientConfig,
    semaphore: Semaphore,
    pub activity: Arc<Activity>,
    stats: Mutex<ApiStats>,
    api_base: Url,
    graphql_url: Url,
    jitter_state: AtomicU64,
}

impl Transport {
    pub fn new(
        credentials: Option<Credentials>,
        mut config: ClientConfig,
    ) -> Result<Self, ApiError> {
        config.concurrency = config.concurrency.clamp(1, 16);
        config.max_attempts = config.max_attempts.max(1);
        let api_base = Url::parse(config.api_base.trim_end_matches('/'))
            .map_err(|_| ApiError::InvalidResponse("invalid API base URL".into()))?;
        let graphql_url = Url::parse(&config.graphql_url)
            .map_err(|_| ApiError::InvalidResponse("invalid GraphQL URL".into()))?;
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(config.request_timeout)
            .build()
            .map_err(|_| ApiError::Network("failed to initialize the HTTP client".into()))?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        Ok(Self {
            http,
            token: credentials.map(|c| c.token),
            semaphore: Semaphore::new(config.concurrency),
            activity: Arc::new(Activity::new(config.concurrency)),
            config,
            stats: Mutex::new(ApiStats::default()),
            api_base,
            graphql_url,
            jitter_state: AtomicU64::new(seed),
        })
    }

    pub fn stats(&self) -> ApiStats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn with_stats(&self, f: impl FnOnce(&mut ApiStats)) {
        if let Ok(mut s) = self.stats.lock() {
            f(&mut s);
        }
    }

    /// Builds a REST URL from path segments (each percent-encoded) and query.
    pub fn rest_url(&self, segments: &[&str], query: &[(&str, &str)]) -> Url {
        let mut url = self.api_base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty();
            path.extend(segments);
        }
        if !query.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
        }
        url
    }

    /// `GET` returning parsed JSON and the response headers.
    pub async fn get_json(&self, url: &Url) -> Result<(Value, HeaderMap), ApiError> {
        let resp = self.send(Method::GET, url, None, false).await?;
        let value = parse_json(&resp.body, url)?;
        Ok((value, resp.headers))
    }

    /// Follows `Link: rel="next"` until absent, collecting the items each page
    /// yields through `extract`, in page order. Any failing page fails the
    /// whole call. When the first page also links to the last page, the
    /// pages in between are fetched at once (still limited by the
    /// transport's concurrency) from the `next` link with only its page
    /// number changed.
    pub async fn get_paginated(
        &self,
        first: Url,
        extract: impl Fn(&Value) -> Option<Vec<Value>>,
    ) -> Result<Vec<Value>, ApiError> {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let take = |items: &mut Vec<Value>, url: &Url, value: &Value| {
            let page = extract(value).ok_or_else(|| {
                ApiError::InvalidResponse(format!("unexpected response shape from {}", url.path()))
            })?;
            items.extend(page);
            Ok::<_, ApiError>(())
        };
        seen.insert(first.to_string());
        let (value, mut headers) = self.get_json(&first).await?;
        take(&mut items, &first, &value)?;
        let mut fetched_up_to = 1;
        if let (Some(next), Some(last)) = (self.next_link(&headers)?, self.link(&headers, "last")?)
            && page_of(&next) == Some(2)
            && let Some(last_page) = page_of(&last)
            && last_page >= 2
        {
            if last_page as usize > MAX_PAGES {
                return Err(ApiError::InvalidResponse(format!(
                    "pagination of {} did not terminate",
                    first.path()
                )));
            }
            let urls: Vec<Url> = (2..=last_page).map(|n| with_page(&next, n)).collect();
            let pages =
                futures::future::try_join_all(urls.iter().map(|u| self.get_json(u))).await?;
            for (url, (value, h)) in urls.iter().zip(pages) {
                seen.insert(url.to_string());
                take(&mut items, url, &value)?;
                headers = h;
            }
            fetched_up_to = last_page;
        }
        while let Some(url) = self.next_link(&headers)? {
            if page_of(&url).is_some_and(|n| n <= fetched_up_to) {
                break;
            }
            if !seen.insert(url.to_string()) || seen.len() > MAX_PAGES {
                return Err(ApiError::InvalidResponse(format!(
                    "pagination of {} did not terminate",
                    url.path()
                )));
            }
            let (value, h) = self.get_json(&url).await?;
            take(&mut items, &url, &value)?;
            headers = h;
        }
        Ok(items)
    }

    /// The `rel="next"` URL of a REST response. Links pointing to another
    /// origin than the API base are rejected so the token is never sent
    /// elsewhere.
    pub fn next_link(&self, headers: &HeaderMap) -> Result<Option<Url>, ApiError> {
        self.link(headers, "next")
    }

    /// The URL of the `Link` header entry with relation `rel`, checked like
    /// [`Self::next_link`].
    pub fn link(&self, headers: &HeaderMap, rel: &str) -> Result<Option<Url>, ApiError> {
        let Some(link) = headers.get("link").and_then(|v| v.to_str().ok()) else {
            return Ok(None);
        };
        let Some(raw) = parse_link(link, rel) else {
            return Ok(None);
        };
        let url = Url::parse(&raw)
            .map_err(|_| ApiError::InvalidResponse("malformed pagination link".into()))?;
        if url.origin() != self.api_base.origin() {
            return Err(ApiError::InvalidResponse(
                "pagination link points to an unexpected host".into(),
            ));
        }
        Ok(Some(url))
    }

    /// Sends a GraphQL query. Transport-level failures are errors; GraphQL
    /// `errors` are returned to the caller together with any partial data.
    pub async fn graphql(
        &self,
        query: &str,
        variables: Value,
    ) -> Result<GraphqlResponse, ApiError> {
        let body = serde_json::to_vec(&serde_json::json!({
            "query": query,
            "variables": variables,
        }))
        .map_err(|_| ApiError::InvalidResponse("failed to encode GraphQL request".into()))?;
        let url = self.graphql_url.clone();
        let resp = self.send(Method::POST, &url, Some(&body), true).await?;
        let value = parse_json(&resp.body, &url)?;
        if !value.is_object() {
            return Err(ApiError::InvalidResponse(
                "GraphQL response is not an object".into(),
            ));
        }
        serde_json::from_value::<GraphqlResponse>(value)
            .map_err(|e| ApiError::InvalidResponse(format!("GraphQL response: {e}")))
    }

    /// Sends a request with retries. Returns only 2xx responses.
    async fn send(
        &self,
        method: Method,
        url: &Url,
        body: Option<&[u8]>,
        graphql: bool,
    ) -> Result<RawResponse, ApiError> {
        let max_attempts = self.config.max_attempts.max(1);
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let decision = match self.attempt(&method, url, body).await {
                Ok(resp) => self.decide(resp, url, &method, attempt, graphql),
                Err(AttemptError::Timeout) => Decision::Retry {
                    delay: self.backoff(attempt),
                    error: ApiError::Timeout { attempts: attempt },
                    rate_limit: false,
                },
                Err(AttemptError::Network(msg)) => Decision::Retry {
                    delay: self.backoff(attempt),
                    error: ApiError::Network(msg),
                    rate_limit: false,
                },
                Err(AttemptError::TooLarge) => Decision::Fail(ApiError::TooLarge {
                    limit: self.config.max_response_bytes,
                }),
            };
            match decision {
                Decision::Done(r) => return Ok(r),
                Decision::Fail(e) => return Err(e),
                Decision::Retry {
                    delay,
                    error,
                    rate_limit,
                } => {
                    if attempt >= max_attempts {
                        return Err(error);
                    }
                    tracing::debug!(
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        path = url.path(),
                        "retrying GitHub request"
                    );
                    self.with_stats(|s| {
                        s.retries += 1;
                        if rate_limit {
                            s.rate_limit_waits += 1;
                        }
                    });
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn attempt(
        &self,
        method: &Method,
        url: &Url,
        body: Option<&[u8]>,
    ) -> Result<Attempted, AttemptError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| AttemptError::Network("client is shutting down".into()))?;
        let _running = self.activity.start(activity::label(url, body));
        let started = Instant::now();
        let limit = self.config.max_response_bytes;
        let fut = async {
            let mut req = self
                .http
                .request(method.clone(), url.clone())
                .header(ACCEPT, "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header(USER_AGENT, self.config.user_agent.as_str());
            if let Some(token) = &self.token {
                let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                    .map_err(|_| {
                        AttemptError::Network("token contains invalid characters".into())
                    })?;
                value.set_sensitive(true);
                req = req.header(AUTHORIZATION, value);
            }
            if let Some(b) = body {
                req = req
                    .header(CONTENT_TYPE, "application/json")
                    .body(b.to_vec());
            }
            let mut resp = req.send().await.map_err(map_reqwest_error)?;
            let status = resp.status().as_u16();
            let headers = resp.headers().clone();
            if resp.content_length().is_some_and(|len| len > limit as u64) {
                return Err(AttemptError::TooLarge);
            }
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(map_reqwest_error)? {
                if buf.len() + chunk.len() > limit {
                    return Err(AttemptError::TooLarge);
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(Attempted {
                status,
                headers,
                body: buf,
            })
        };
        let result = match tokio::time::timeout(self.config.request_timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(AttemptError::Timeout),
        };
        let elapsed = started.elapsed().as_millis() as u64;
        let bytes = result.as_ref().map(|r| r.body.len() as u64).unwrap_or(0);
        tracing::debug!(
            %method,
            path = url.path(),
            status = result.as_ref().map(|r| r.status).unwrap_or(0),
            elapsed_ms = elapsed,
            bytes,
            "GitHub request"
        );
        self.with_stats(|s| {
            s.requests += 1;
            s.total_api_time_ms += elapsed;
            s.bytes_received += bytes;
        });
        result
    }

    fn decide(
        &self,
        resp: Attempted,
        url: &Url,
        method: &Method,
        attempt: u32,
        graphql: bool,
    ) -> Decision {
        let Attempted {
            status,
            headers,
            body,
        } = resp;
        let path = url.path().to_string();
        if (200..300).contains(&status) {
            if graphql && graphql_rate_limited(&body) {
                return self.primary_limit(&headers, attempt);
            }
            return Decision::Done(RawResponse { headers, body });
        }
        match status {
            401 => Decision::Fail(ApiError::Unauthorized),
            403 | 429 => {
                if header_str(&headers, "x-ratelimit-remaining") == Some("0") {
                    return self.primary_limit(&headers, attempt);
                }
                let message = body_message(&body);
                if let Some(secs) =
                    header_str(&headers, "retry-after").and_then(|v| v.trim().parse::<u64>().ok())
                {
                    let wait = Duration::from_secs(secs);
                    let reset_at = Timestamp::now() + SignedDuration::from_secs(secs as i64);
                    if wait > self.config.max_rate_limit_wait {
                        return Decision::Fail(ApiError::RateLimited {
                            reset_at: Some(reset_at),
                        });
                    }
                    return Decision::Retry {
                        delay: wait,
                        error: ApiError::RateLimited {
                            reset_at: Some(reset_at),
                        },
                        rate_limit: true,
                    };
                }
                let secondary = message.as_deref().is_some_and(|m| {
                    let m = m.to_ascii_lowercase();
                    m.contains("secondary rate limit") || m.contains("abuse detection")
                });
                if secondary || status == 429 {
                    // Without Retry-After GitHub asks clients to wait at least a
                    // minute; scale that by the backoff base.
                    let delay = if secondary {
                        let minute = self.config.backoff_base.saturating_mul(60);
                        minute.max(self.backoff(attempt)).min(MAX_BACKOFF)
                    } else {
                        self.backoff(attempt)
                    };
                    if delay > self.config.max_rate_limit_wait {
                        return Decision::Fail(ApiError::RateLimited { reset_at: None });
                    }
                    return Decision::Retry {
                        delay,
                        error: ApiError::RateLimited { reset_at: None },
                        rate_limit: true,
                    };
                }
                let detail = message.unwrap_or_else(|| "forbidden".into());
                Decision::Fail(ApiError::Forbidden(format!("{method} {path}: {detail}")))
            }
            404 => Decision::Fail(ApiError::NotFound(format!("{method} {path}"))),
            500 | 502 | 503 | 504 => Decision::Retry {
                delay: self.backoff(attempt),
                error: ApiError::Http {
                    status,
                    attempts: attempt,
                },
                rate_limit: false,
            },
            _ => Decision::Fail(ApiError::Http {
                status,
                attempts: attempt,
            }),
        }
    }

    /// Primary rate limit exhausted: wait for `x-ratelimit-reset` when it is
    /// within the allowed wait, otherwise give up.
    fn primary_limit(&self, headers: &HeaderMap, attempt: u32) -> Decision {
        let reset_at = header_str(headers, "x-ratelimit-reset")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .and_then(|secs| Timestamp::from_second(secs).ok());
        let delay = match reset_at {
            Some(reset) => {
                let secs = reset
                    .duration_since(Timestamp::now())
                    .as_secs_f64()
                    .max(0.0) as u64;
                Duration::from_secs(secs + 1)
            }
            None => self.backoff(attempt),
        };
        if delay > self.config.max_rate_limit_wait {
            return Decision::Fail(ApiError::RateLimited { reset_at });
        }
        Decision::Retry {
            delay,
            error: ApiError::RateLimited { reset_at },
            rate_limit: true,
        }
    }

    /// Exponential backoff with jitter, capped at one minute.
    pub(crate) fn backoff(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(20);
        let base = self
            .config
            .backoff_base
            .saturating_mul(1u32 << exp)
            .min(MAX_BACKOFF);
        // xorshift64 for jitter; quality is irrelevant here.
        let mut x = self.jitter_state.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.jitter_state.store(x, Ordering::Relaxed);
        let factor = 0.5 + (x % 1000) as f64 / 2000.0;
        base.mul_f64(factor)
    }
}

fn map_reqwest_error(e: reqwest::Error) -> AttemptError {
    if e.is_timeout() {
        AttemptError::Timeout
    } else if e.is_connect() {
        AttemptError::Network("could not connect to GitHub".into())
    } else if e.is_body() || e.is_decode() {
        AttemptError::Network("connection interrupted while reading the response".into())
    } else {
        AttemptError::Network("request to GitHub failed".into())
    }
}

fn parse_json(body: &[u8], url: &Url) -> Result<Value, ApiError> {
    serde_json::from_slice(body)
        .map_err(|_| ApiError::InvalidResponse(format!("invalid JSON from {}", url.path())))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// GitHub's `message` field from an error body, if any.
fn body_message(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v.get("message")
        .and_then(|m| m.as_str())
        .map(|m| truncate(m, 300))
}

fn graphql_rate_limited(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    v.get("errors")
        .and_then(|e| e.as_array())
        .is_some_and(|errs| {
            errs.iter()
                .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("RATE_LIMITED"))
        })
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Extracts the target with relation `rel` from a `Link` header.
pub(crate) fn parse_link(header: &str, rel: &str) -> Option<String> {
    let quoted = format!("rel=\"{rel}\"");
    let bare = format!("rel={rel}");
    for part in header.split(',') {
        let mut pieces = part.split(';');
        let target = pieces.next()?.trim();
        let matches = pieces.any(|p| {
            let p = p.trim();
            p == quoted || p == bare
        });
        if matches && target.starts_with('<') && target.ends_with('>') {
            return Some(target[1..target.len() - 1].to_string());
        }
    }
    None
}

/// The `page` query parameter of a pagination link.
pub(crate) fn page_of(url: &Url) -> Option<u64> {
    url.query_pairs()
        .find(|(k, _)| k == "page")
        .and_then(|(_, v)| v.parse().ok())
}

/// `url` with its `page` query parameter set to `n`.
pub(crate) fn with_page(url: &Url, n: u64) -> Url {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if k == "page" {
                n.to_string()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    let mut out = url.clone();
    out.query_pairs_mut().clear().extend_pairs(pairs);
    out
}

/// Parses a GitHub timestamp.
pub(crate) fn parse_time(s: &str) -> Option<Timestamp> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_link_is_found_among_others() {
        let h = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#;
        assert_eq!(
            parse_link(h, "next").as_deref(),
            Some("https://api.github.com/x?page=2")
        );
        let h = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=1>; rel="first""#;
        assert_eq!(parse_link(h, "next"), None);
    }

    #[test]
    fn last_link_and_page_numbers() {
        let h = r#"<https://api.github.com/x?q=a%20b&page=2>; rel="next", <https://api.github.com/x?q=a%20b&page=5>; rel="last""#;
        let last = Url::parse(&parse_link(h, "last").unwrap()).unwrap();
        assert_eq!(page_of(&last), Some(5));
        let next = Url::parse(&parse_link(h, "next").unwrap()).unwrap();
        let third = with_page(&next, 3);
        assert_eq!(page_of(&third), Some(3));
        // Everything but the page number is kept.
        let q: Vec<(String, String)> = third
            .query_pairs()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        assert_eq!(q[0], ("q".to_string(), "a b".to_string()));
        assert_eq!(third.host_str(), Some("api.github.com"));
    }

    #[test]
    fn backoff_doubles_from_the_base_and_is_capped_at_one_minute() {
        // The default base is one second. Without jitter, retry n waits
        // 2^(n-1) seconds: 1, 2, 4, 8, 16, 32, then 64 is capped to 60.
        let t = Transport::new(None, ClientConfig::default()).unwrap();
        assert_eq!(t.config.backoff_base, Duration::from_secs(1));
        let nominal = [1u64, 2, 4, 8, 16, 32, 60, 60, 60, 60];
        for (i, secs) in nominal.into_iter().enumerate() {
            let attempt = i as u32 + 1;
            let full = Duration::from_secs(secs);
            for _ in 0..200 {
                let d = t.backoff(attempt);
                // Jitter only shortens a wait, by at most half, so the
                // doubling stays visible and the cap is never exceeded.
                assert!(d <= full, "attempt {attempt}: {d:?} > {full:?}");
                assert!(d >= full / 2, "attempt {attempt}: {d:?} < {:?}", full / 2);
                assert!(d <= MAX_BACKOFF);
            }
        }
        // Very late attempts neither overflow nor exceed the cap.
        for attempt in [20, 32, 64, u32::MAX] {
            let d = t.backoff(attempt);
            assert!(d <= Duration::from_secs(60), "attempt {attempt}: {d:?}");
            assert!(d >= Duration::from_secs(30), "attempt {attempt}: {d:?}");
        }
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }
}
