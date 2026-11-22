//! What the client is doing right now: the requests in flight, for live
//! progress displays.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::Url;
use serde_json::Value;

/// One request in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    /// What the request is for, e.g. `search commits p3` or `octo/app#12`.
    pub label: String,
    pub elapsed: Duration,
}

/// A moment's view of the client's requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivitySnapshot {
    /// Oldest first.
    pub running: Vec<InFlight>,
    /// How many requests may run at the same time.
    pub limit: usize,
    /// Requests finished so far (each attempt counts).
    pub done: u64,
}

#[derive(Debug, Default)]
struct Inner {
    next: u64,
    running: BTreeMap<u64, (String, Instant)>,
    done: u64,
}

/// Requests in flight, updated by the transport.
#[derive(Debug)]
pub struct Activity {
    limit: usize,
    inner: Mutex<Inner>,
}

impl Activity {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn snapshot(&self) -> ActivitySnapshot {
        let Ok(inner) = self.inner.lock() else {
            return ActivitySnapshot::default();
        };
        ActivitySnapshot {
            running: inner
                .running
                .values()
                .map(|(label, started)| InFlight {
                    label: label.clone(),
                    elapsed: started.elapsed(),
                })
                .collect(),
            limit: self.limit,
            done: inner.done,
        }
    }

    /// Records a request until the returned guard is dropped.
    pub(crate) fn start(&self, label: String) -> Running<'_> {
        let id = match self.inner.lock() {
            Ok(mut inner) => {
                let id = inner.next;
                inner.next += 1;
                inner.running.insert(id, (label, Instant::now()));
                Some(id)
            }
            Err(_) => None,
        };
        Running { activity: self, id }
    }
}

/// A request in flight; finished when dropped.
pub(crate) struct Running<'a> {
    activity: &'a Activity,
    id: Option<u64>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if let (Some(id), Ok(mut inner)) = (self.id, self.activity.inner.lock()) {
            inner.running.remove(&id);
            inner.done += 1;
        }
    }
}

/// A short description of a request for a progress display.
pub(crate) fn label(url: &Url, body: Option<&[u8]>) -> String {
    let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    let page = url
        .query_pairs()
        .find(|(k, _)| k == "page")
        .map(|(_, v)| format!(" p{v}"))
        .unwrap_or_default();
    let what = match segments.as_slice() {
        ["graphql"] => graphql_label(body),
        ["search", kind] => format!("search {kind}"),
        ["user"] => "account".to_string(),
        ["user", "repos"] | ["users", _, "repos"] => "repository list".to_string(),
        ["repos", owner, name] => format!("{owner}/{name}"),
        ["repos", owner, name, "pulls", n, ..] | ["repos", owner, name, "issues", n, ..] => {
            format!("{owner}/{name}#{n}")
        }
        ["repos", owner, name, "commits"] => format!("{owner}/{name} commits"),
        ["repos", owner, name, "commits", sha, ..] => {
            format!("{owner}/{name}@{}", sha.get(..7).unwrap_or(sha))
        }
        ["repos", owner, name, rest @ ..] => format!("{owner}/{name} {}", rest.join("/")),
        ["repositories", id, "commits"] => format!("commits of repository {id}"),
        other => other.join("/"),
    };
    format!("{what}{page}")
}

/// `owner/name#number` from a GraphQL request's variables, or the
/// operation name.
fn graphql_label(body: Option<&[u8]>) -> String {
    let Some(v) = body.and_then(|b| serde_json::from_slice::<Value>(b).ok()) else {
        return "GraphQL".to_string();
    };
    let vars = &v["variables"];
    if let (Some(owner), Some(name)) = (vars["owner"].as_str(), vars["name"].as_str()) {
        return match vars["number"].as_u64() {
            Some(n) => format!("{owner}/{name}#{n}"),
            None => format!("{owner}/{name}"),
        };
    }
    let op = v["query"]
        .as_str()
        .and_then(|q| {
            q.split_whitespace()
                .skip_while(|w| *w != "query" && *w != "mutation")
                .nth(1)
        })
        .map(|name| name.split('(').next().unwrap_or(name).to_string());
    op.map(|o| format!("GraphQL {o}"))
        .unwrap_or_else(|| "GraphQL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn labels_name_what_a_request_is_for() {
        let l = |s: &str| label(&url(s), None);
        assert_eq!(
            l("https://api.github.com/search/commits?q=x&page=3"),
            "search commits p3"
        );
        assert_eq!(
            l("https://api.github.com/user/repos?per_page=100"),
            "repository list"
        );
        assert_eq!(
            l("https://api.github.com/repos/octo/app/commits?author=a&page=2"),
            "octo/app commits p2"
        );
        assert_eq!(
            l("https://api.github.com/repos/octo/app/pulls/12"),
            "octo/app#12"
        );
        assert_eq!(
            l("https://api.github.com/repos/octo/app/commits/0123456789abcdef"),
            "octo/app@0123456"
        );
        let body = br#"{"query":"query PullRequestDetail($owner: String!) { x }","variables":{"owner":"octo","name":"app","number":7}}"#;
        assert_eq!(
            label(&url("https://api.github.com/graphql"), Some(body)),
            "octo/app#7"
        );
        let body = br#"{"query":"query Viewer { viewer { login } }","variables":{}}"#;
        assert_eq!(
            label(&url("https://api.github.com/graphql"), Some(body)),
            "GraphQL Viewer"
        );
    }

    #[test]
    fn running_requests_are_listed_until_finished() {
        let a = Activity::new(8);
        let one = a.start("one".into());
        let two = a.start("two".into());
        let s = a.snapshot();
        assert_eq!(s.limit, 8);
        assert_eq!(
            s.running
                .iter()
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        drop(one);
        let s = a.snapshot();
        assert_eq!(s.running.len(), 1);
        assert_eq!(s.done, 1);
        drop(two);
        assert_eq!(a.snapshot().done, 2);
    }
}
