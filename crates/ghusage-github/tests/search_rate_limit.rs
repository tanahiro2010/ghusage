//! A rate limit on one search holds every search: the requests that were
//! limited, the ones already waiting and the ones started during the wait.
//! After the wait, searches are sent one at a time for a while, so the
//! retries do not arrive together and hit the limit again. Other requests
//! are never held.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use ghusage_github::{GitHubClient, IssueKind};
use serde_json::{Value, json};
use wiremock::matchers::{path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const LOGIN: &str = "alice";

/// The backoff base of these tests. The secondary-limit wait is 60 times
/// this, and paced searches are two of these apart.
const BASE: Duration = Duration::from_millis(30);
const MINUTE: Duration = Duration::from_millis(1800);
const SPACING: Duration = Duration::from_millis(60);

fn jan1() -> ghusage_core::time::Period {
    period("2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z")
}

fn commits(count: usize) -> Vec<Value> {
    let repo = rest_repo("R_widget", "acme", "Organization", "widget", false, false);
    (0..count as u64)
        .map(|i| {
            let t = format!(
                "2024-01-01T{:02}:{:02}:{:02}Z",
                i / 3600,
                (i / 60) % 60,
                i % 60
            );
            search_commit(&repo, &sha(i), Some(LOGIN), &t, &t, 1)
        })
        .collect()
}

fn secondary_limit() -> ResponseTemplate {
    ResponseTemplate::new(403).set_body_json(json!({
        "message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
        "documentation_url": "https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"
    }))
}

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Arrival {
    at: Instant,
    path: String,
    page: usize,
    limited: bool,
}

/// How the server limits searches.
#[derive(Clone)]
enum Limit {
    /// Never.
    None,
    /// A secondary limit when `burst` searches arrive within `window`.
    Burst { burst: usize, window: Duration },
    /// The first search request gets this response.
    First(fn() -> ResponseTemplate),
}

/// A search API: `/search/commits` answers with `pages` (built in advance,
/// so the server answers at once and the arrival times stay accurate);
/// `/search/issues` finds nothing.
struct SearchServer {
    pages: Vec<ResponseTemplate>,
    limit: Limit,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
}

impl Respond for SearchServer {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let now = Instant::now();
        let page: usize = req
            .url
            .query_pairs()
            .find(|(k, _)| k == "page")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(1);
        let mut arrivals = self.arrivals.lock().unwrap();
        let limited = match &self.limit {
            Limit::None => None,
            Limit::Burst { burst, window } => {
                let recent = arrivals
                    .iter()
                    .filter(|a| now.duration_since(a.at) < *window)
                    .count();
                (recent + 1 >= *burst).then(secondary_limit)
            }
            Limit::First(response) => arrivals.is_empty().then(response),
        };
        arrivals.push(Arrival {
            at: now,
            path: req.url.path().to_string(),
            page,
            limited: limited.is_some(),
        });
        drop(arrivals);
        if let Some(response) = limited {
            return response;
        }
        if req.url.path() == "/search/issues" {
            return ResponseTemplate::new(200).set_body_json(search_page(0, false, Vec::new()));
        }
        self.pages[page - 1].clone()
    }
}

/// The pages of a commit search finding `count` commits.
fn commit_pages(uri: &str, count: usize) -> Vec<ResponseTemplate> {
    let all = commits(count);
    let pages = count.div_ceil(100).max(1);
    (1..=pages)
        .map(|page| {
            let items = all[(page - 1) * 100..(page * 100).min(count)].to_vec();
            let mut r =
                ResponseTemplate::new(200).set_body_json(search_page(count as u64, false, items));
            if page < pages {
                r = r.insert_header(
                    "link",
                    format!(
                        "<{uri}/search/commits?marker=m&per_page=100&page={}>; rel=\"next\"",
                        page + 1
                    )
                    .as_str(),
                );
            }
            r
        })
        .collect()
}

struct Setup {
    server: MockServer,
    client: Arc<GitHubClient>,
    arrivals: Arc<Mutex<Vec<Arrival>>>,
}

impl Setup {
    fn arrivals(&self) -> Vec<Arrival> {
        self.arrivals.lock().unwrap().clone()
    }
}

async fn setup(commit_count: usize, limit: Limit) -> Setup {
    let server = MockServer::start().await;
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    Mock::given(path_regex("^/search/(commits|issues)$"))
        .respond_with(SearchServer {
            pages: commit_pages(&server.uri(), commit_count),
            limit,
            arrivals: arrivals.clone(),
        })
        .mount(&server)
        .await;
    Mock::given(path("/users/octocat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(rest_user("octocat", "U_octo", "User")),
        )
        .mount(&server)
        .await;
    let client = Arc::new(client_with(&server, |c| {
        c.concurrency = 8;
        c.max_attempts = 5;
        c.backoff_base = BASE;
    }));
    Setup {
        server,
        client,
        arrivals,
    }
}

/// Time between the first and the last of `arrivals`.
fn spread(arrivals: &[Arrival]) -> Duration {
    let first = arrivals.iter().map(|a| a.at).min().unwrap();
    let last = arrivals.iter().map(|a| a.at).max().unwrap();
    last - first
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_limited_together_wait_once_and_come_back_one_at_a_time() {
    // The eight pages after the first are requested at once; GitHub limits
    // four or more searches within 100 ms.
    let s = setup(
        900,
        Limit::Burst {
            burst: 4,
            window: Duration::from_millis(100),
        },
    )
    .await;
    let r = s.client.search_commits(LOGIN, jan1(), false).await.unwrap();
    assert_eq!(r.items.len(), 900);
    let arrivals = s.arrivals();
    let limited: Vec<_> = arrivals.iter().filter(|a| a.limited).cloned().collect();
    assert!(!limited.is_empty(), "the burst was never limited");
    // Every limit came from the first burst: the retries did not hit it
    // again after the wait.
    assert!(
        spread(&limited) < MINUTE / 2,
        "the limit was hit again after the wait: {limited:?}"
    );
    // No search reached the server during the wait. The retries, and any
    // page that had not left yet, were sent one per slot after it.
    let first_limit = limited.iter().map(|a| a.at).min().unwrap();
    let after: Vec<_> = arrivals
        .iter()
        .filter(|a| a.at - first_limit > MINUTE / 2)
        .collect();
    assert!(after.len() >= limited.len(), "{arrivals:?}");
    for a in &after {
        assert!(a.at - first_limit >= MINUTE, "sent during the wait: {a:?}");
    }
    let last = after.iter().map(|a| a.at).max().unwrap();
    let slots = u32::try_from(after.len() - 1).unwrap();
    assert!(
        last - first_limit >= MINUTE + SPACING * slots,
        "retries were not spaced: {after:?}"
    );
    let stats = s.client.stats();
    assert_eq!(stats.requests, 9 + limited.len() as u64);
    assert_eq!(stats.retries, limited.len() as u64);
    assert_eq!(stats.rate_limit_waits, limited.len() as u64);
    drop(s.server);
}

/// Waits until the client has seen the first (limited) response and
/// started its wait. Returns when the server sent that response.
async fn limited(s: &Setup) -> Instant {
    while s.client.stats().rate_limit_waits == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    s.arrivals()[0].at
}

/// Starts a commit search that the server limits with `response`, then,
/// while it waits, an issue search and a request that is not a search.
/// Returns the time of the limited response and when the issue search and
/// the other request finished.
async fn search_started_during_a_wait(
    response: fn() -> ResponseTemplate,
) -> (Setup, Instant, Instant, Instant) {
    let s = setup(1, Limit::First(response)).await;
    let c = s.client.clone();
    let commits = tokio::spawn(async move { c.search_commits(LOGIN, jan1(), false).await });
    let limited_at = limited(&s).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let c = s.client.clone();
    let issues = tokio::spawn(async move {
        let r = c
            .search_issues(
                &format!("involves:{LOGIN}"),
                IssueKind::PullRequest,
                jan1(),
                false,
            )
            .await;
        (r, Instant::now())
    });
    // A request that is not a search goes out while the searches wait.
    s.client.user("octocat").await.unwrap();
    let other_done = Instant::now();
    let (r, issues_done) = issues.await.unwrap();
    r.unwrap();
    assert_eq!(commits.await.unwrap().unwrap().items.len(), 1);
    (s, limited_at, issues_done, other_done)
}

fn assert_held(s: &Setup, limited_at: Instant, other_done: Instant, wait: Duration) {
    let issues: Vec<_> = s
        .arrivals()
        .into_iter()
        .filter(|a| a.path == "/search/issues")
        .collect();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(
        issues[0].at - limited_at >= wait,
        "a search started during the wait was sent after {:?}",
        issues[0].at - limited_at
    );
    assert!(
        other_done - limited_at < wait / 2,
        "a request that is not a search was held for {:?}",
        other_done - limited_at
    );
    let stats = s.client.stats();
    assert_eq!(stats.requests, 4);
    assert_eq!(stats.rate_limit_waits, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secondary_limit_holds_searches_started_during_the_wait() {
    let (s, limited_at, _, other_done) = search_started_during_a_wait(secondary_limit).await;
    assert_held(&s, limited_at, other_done, MINUTE);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_after_holds_searches_started_during_the_wait() {
    fn limited() -> ResponseTemplate {
        ResponseTemplate::new(403)
            .insert_header("retry-after", "1")
            .set_body_json(json!({ "message": "You have triggered an abuse detection mechanism." }))
    }
    let (s, limited_at, _, other_done) = search_started_during_a_wait(limited).await;
    assert_held(&s, limited_at, other_done, Duration::from_millis(950));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_search_limit_holds_searches_until_the_reset() {
    fn limited() -> ResponseTemplate {
        let reset = jiff::Timestamp::now().as_second() + 1;
        ResponseTemplate::new(403)
            .insert_header("x-ratelimit-limit", "30")
            .insert_header("x-ratelimit-remaining", "0")
            .insert_header("x-ratelimit-used", "30")
            .insert_header("x-ratelimit-reset", reset.to_string().as_str())
            .insert_header("x-ratelimit-resource", "search")
            .set_body_json(json!({ "message": "API rate limit exceeded for user ID 1." }))
    }
    // The wait lasts until one second after the reset, at least a second.
    let (s, limited_at, _, other_done) = search_started_during_a_wait(limited).await;
    assert_held(&s, limited_at, other_done, Duration::from_millis(950));
}

#[tokio::test(flavor = "multi_thread")]
async fn searches_are_not_paced_without_a_limit() {
    let s = setup(700, Limit::None).await;
    let r = s.client.search_commits(LOGIN, jan1(), false).await.unwrap();
    assert_eq!(r.items.len(), 700);
    let later: Vec<_> = s.arrivals().into_iter().filter(|a| a.page > 1).collect();
    assert_eq!(later.len(), 6);
    // Paced, the six pages would take at least five slots.
    assert!(spread(&later) < SPACING * 4, "{later:?}");
    assert_eq!(s.client.stats().rate_limit_waits, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn pacing_ends_a_minute_after_the_wait() {
    let s = setup(700, Limit::First(secondary_limit)).await;
    let r = s.client.search_commits(LOGIN, jan1(), false).await.unwrap();
    assert_eq!(r.items.len(), 700);
    let limited_at = s.arrivals()[0].at;
    // Right after the wait, the retried first page and the six later
    // pages are paced: they take six slots, less how late the first one
    // left its slot.
    let paced: Vec<_> = s.arrivals().into_iter().skip(1).collect();
    assert_eq!(paced.len(), 7);
    assert!(spread(&paced) >= SPACING * 5, "{paced:?}");
    // A minute after the wait ends, pages are requested at once again.
    let resume = limited_at + MINUTE * 2 + Duration::from_millis(250);
    tokio::time::sleep_until(resume.into()).await;
    let before = s.arrivals().len();
    s.client.search_commits(LOGIN, jan1(), false).await.unwrap();
    let later: Vec<_> = s
        .arrivals()
        .into_iter()
        .skip(before)
        .filter(|a| a.page > 1)
        .collect();
    assert_eq!(later.len(), 6);
    assert!(spread(&later) < SPACING * 4, "{later:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn searches_dropped_while_waiting_do_not_hold_later_ones() {
    let s = setup(1, Limit::First(secondary_limit)).await;
    let c = s.client.clone();
    let commits = tokio::spawn(async move { c.search_commits(LOGIN, jan1(), false).await });
    let limited_at = limited(&s).await;
    // Twenty searches start during the wait and are dropped before it ends
    // (as when another page of the same search fails).
    let mut dropped = Vec::new();
    for _ in 0..20 {
        let c = s.client.clone();
        dropped.push(tokio::spawn(async move {
            c.search_issues(
                &format!("involves:{LOGIN}"),
                IssueKind::PullRequest,
                jan1(),
                false,
            )
            .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    for task in &dropped {
        task.abort();
    }
    for task in dropped {
        assert!(task.await.unwrap_err().is_cancelled());
    }
    assert_eq!(commits.await.unwrap().unwrap().items.len(), 1);
    // While searches are still paced, a new search waits at most a slot or
    // so, not behind the dropped ones.
    tokio::time::sleep_until((limited_at + MINUTE + Duration::from_millis(300)).into()).await;
    let started = Instant::now();
    s.client
        .search_issues(
            &format!("involves:{LOGIN}"),
            IssueKind::PullRequest,
            jan1(),
            false,
        )
        .await
        .unwrap();
    let issues: Vec<_> = s
        .arrivals()
        .into_iter()
        .filter(|a| a.path == "/search/issues")
        .collect();
    assert_eq!(issues.len(), 1, "a dropped search was sent: {issues:?}");
    assert!(
        issues[0].at - started < SPACING * 2,
        "held for {:?} by dropped searches",
        issues[0].at - started
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_requests_on_a_search_paces_the_searches() {
    fn limited() -> ResponseTemplate {
        ResponseTemplate::new(429).set_body_json(json!({ "message": "Too Many Requests" }))
    }
    let s = setup(700, Limit::First(limited)).await;
    let r = s.client.search_commits(LOGIN, jan1(), false).await.unwrap();
    assert_eq!(r.items.len(), 700);
    // The retried first page and the six later pages are paced.
    let paced: Vec<_> = s.arrivals().into_iter().skip(1).collect();
    assert_eq!(paced.len(), 7);
    assert!(spread(&paced) >= SPACING * 5, "{paced:?}");
    let stats = s.client.stats();
    assert_eq!(stats.requests, 8);
    assert_eq!(stats.rate_limit_waits, 1);
}
