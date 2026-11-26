//! Discovery through the REST search API, splitting periods that exceed
//! the 1000-result cap.

use std::collections::HashSet;

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{
    ActivityEvent, Actor, EventId, EventMeta, EventType, Repository, Source, Visibility,
};
use ghusage_core::time::Period;
use jiff::Timestamp;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::Value;

use crate::http::{Transport, page_of, parse_time, with_page};
use crate::pull_request::RestUser;
use crate::repos::RestRepo;
use crate::{ApiError, CommitRecord, Fetched, IssueKind, IssueRef};

/// GitHub returns at most this many results per search query.
const SEARCH_CAP: u64 = 1000;

/// Results per search page (the most GitHub allows).
const PER_PAGE: u64 = 100;

/// Whole seconds `[lo, hi)` searched as `lo..hi-1` (bounds are inclusive),
/// or, when `open`, every second from `lo` on searched as `>=lo`; `hi` then
/// only says where to split the range.
#[derive(Debug, Clone, Copy)]
struct Range {
    lo: i64,
    hi: i64,
    open: bool,
}

impl Range {
    fn of(period: Period) -> Self {
        let lo = period.start.as_second();
        let hi = if period.end.subsec_nanosecond() > 0 {
            period.end.as_second() + 1
        } else {
            period.end.as_second()
        };
        Self {
            lo,
            hi: hi.max(lo + 1),
            open: false,
        }
    }

    /// Every second from `since` on, split at the current time first.
    ///
    /// The split point comes from the system clock, not from the time a
    /// caller treats as "now" (such as the start of a sync): it only decides
    /// where a range with too many results is cut, and the range stays
    /// open either way, so nothing is missed. When `since` is later than the
    /// system clock, the range is one second wide and cannot be split.
    fn since(since: Timestamp) -> Self {
        let lo = since.as_second();
        Self {
            lo,
            hi: (Timestamp::now().as_second() + 1).max(lo + 1),
            open: true,
        }
    }

    /// The earlier and the later half. Only the later half of an open
    /// range stays open.
    const fn halves(self) -> (Self, Self) {
        let mid = self.lo + (self.hi - self.lo) / 2;
        (
            Self {
                lo: self.lo,
                hi: mid,
                open: false,
            },
            Self { lo: mid, ..self },
        )
    }

    fn qualifier(self) -> String {
        if self.open {
            format!(">={}", iso(self.lo))
        } else {
            format!("{}..{}", iso(self.lo), iso(self.hi - 1))
        }
    }
}

fn iso(secs: i64) -> String {
    Timestamp::from_second(secs)
        .map(|t| t.strftime("%Y-%m-%dT%H:%M:%S%.f%:z").to_string())
        .unwrap_or_default()
}

/// Raw search results of a whole period.
struct SearchOutcome {
    items: Vec<Value>,
    reasons: Vec<String>,
}

/// Extra attempts for a search page that GitHub reports as incomplete.
const INCOMPLETE_RETRIES: u32 = 2;

/// Fetches one search page. GitHub marks a page `incomplete_results` when
/// the search timed out on its side, which happens more often while several
/// pages are requested at once; such a page is fetched again after a short
/// wait, and only a page that stays incomplete is reported as such.
async fn get_page(t: &Transport, url: &reqwest::Url) -> Result<(Value, HeaderMap), ApiError> {
    let mut page = t.get_json(url).await?;
    for attempt in 1..=INCOMPLETE_RETRIES {
        if page.0.get("incomplete_results").and_then(Value::as_bool) != Some(true) {
            break;
        }
        tokio::time::sleep(t.backoff(attempt)).await;
        page = t.get_json(url).await?;
    }
    Ok(page)
}

/// Runs a search over `range`, splitting ranges whose total exceeds the
/// cap. The earlier half is searched first, so an item updated again while
/// an open range is searched moves to a part that is still to come.
async fn search(
    t: &Transport,
    endpoint: &str,
    sort: &str,
    range: Range,
    build_query: impl Fn(&str) -> String,
) -> Result<SearchOutcome, ApiError> {
    let mut out = SearchOutcome {
        items: Vec::new(),
        reasons: Vec::new(),
    };
    let mut stack = vec![range];
    while let Some(range) = stack.pop() {
        let q = build_query(&range.qualifier());
        let first = t.rest_url(
            &["search", endpoint],
            &[
                ("q", q.as_str()),
                ("sort", sort),
                ("order", "asc"),
                ("per_page", "100"),
            ],
        );
        let (value, headers) = get_page(t, &first).await?;
        let total = value
            .get("total_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ApiError::InvalidResponse("search response without total_count".into())
            })?;
        if total > SEARCH_CAP && range.hi - range.lo > 1 {
            let (earlier, later) = range.halves();
            // Later half pushed first so the earlier half is processed first.
            stack.push(later);
            stack.push(earlier);
            continue;
        }
        if total > SEARCH_CAP {
            out.reasons.push(format!(
                "{total} results in the one-second range {}; only the first {SEARCH_CAP} are available",
                range.qualifier()
            ));
        }
        let mut incomplete = false;
        let mut seen_urls = HashSet::from([first.to_string()]);
        let mut take = |value: &Value| -> Result<(), ApiError> {
            if value.get("incomplete_results").and_then(Value::as_bool) == Some(true) {
                incomplete = true;
            }
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| ApiError::InvalidResponse("search response without items".into()))?;
            out.items.extend(items.iter().cloned());
            Ok(())
        };
        take(&value)?;
        // The total says how many pages there are. The rest are fetched at
        // once (the transport limits how many run at the same time) from
        // the first page's `next` link with only its page number changed.
        let pages = total.min(SEARCH_CAP).div_ceil(PER_PAGE);
        let mut last_headers = headers;
        let template = t.next_link(&last_headers)?;
        let mut fetched_up_to = if let Some(next) = template.filter(|u| page_of(u) == Some(2))
            && pages > 1
        {
            let urls: Vec<_> = (2..=pages).map(|n| with_page(&next, n)).collect();
            let fetched =
                futures::future::try_join_all(urls.iter().map(|u| get_page(t, u))).await?;
            for (url, (value, headers)) in urls.iter().zip(fetched) {
                seen_urls.insert(url.to_string());
                take(&value)?;
                last_headers = headers;
            }
            pages
        } else {
            1
        };
        // Results added while paging can leave more behind the last page.
        // A `next` link to a page already fetched ends the paging.
        while let Some(next) = t.next_link(&last_headers)? {
            match page_of(&next) {
                Some(n) if n <= fetched_up_to => break,
                Some(n) => fetched_up_to = n,
                None => {}
            }
            if !seen_urls.insert(next.to_string()) {
                return Err(ApiError::InvalidResponse(
                    "search pagination did not terminate".into(),
                ));
            }
            let (value, headers) = get_page(t, &next).await?;
            take(&value)?;
            last_headers = headers;
        }
        if incomplete {
            out.reasons.push(format!(
                "GitHub reported incomplete search results for {}",
                range.qualifier()
            ));
        }
    }
    Ok(out)
}

/// A commit as returned by commit search and by the commits list endpoint
/// (the latter without `repository`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SearchCommit {
    sha: Option<String>,
    html_url: Option<String>,
    commit: Option<CommitDetail>,
    author: Option<RestUser>,
    committer: Option<RestUser>,
    parents: Option<Vec<Value>>,
    repository: Option<RestRepo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitDetail {
    message: Option<String>,
    author: Option<GitSignature>,
    committer: Option<GitSignature>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GitSignature {
    date: Option<String>,
}

pub(crate) fn finish<T>(items: T, reasons: Vec<String>) -> Fetched<T> {
    if reasons.is_empty() {
        Fetched {
            items,
            status: CoverageStatus::Complete,
            reason: None,
        }
    } else {
        Fetched {
            items,
            status: CoverageStatus::Partial,
            reason: Some(reasons.join("; ")),
        }
    }
}

pub(crate) async fn commits(
    t: &Transport,
    author_login: &str,
    period: Period,
    public_only: bool,
) -> Result<Fetched<Vec<CommitRecord>>, ApiError> {
    let visibility = if public_only { " is:public" } else { "" };
    let outcome = search(t, "commits", "committer-date", Range::of(period), |range| {
        format!("author:{author_login} committer-date:{range}{visibility}")
    })
    .await?;
    let mut reasons = outcome.reasons;
    let now = Timestamp::now();
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    let mut bad = 0usize;
    for v in outcome.items {
        let Ok(item) = serde_json::from_value::<SearchCommit>(v) else {
            bad += 1;
            continue;
        };
        let Some(repository) = item.repository.as_ref().and_then(RestRepo::to_repository) else {
            bad += 1;
            continue;
        };
        let Some(record) = commit_record(item, repository, now, Source::Search) else {
            bad += 1;
            continue;
        };
        if public_only && record.repository.visibility != Visibility::Public {
            continue;
        }
        if !period.contains(record.event.occurred_at) {
            continue;
        }
        if seen.insert(record.event.event_id.clone()) {
            records.push(record);
        }
    }
    if bad > 0 {
        reasons.push(format!("{bad} commit search result(s) could not be read"));
    }
    records.sort_by(|a, b| {
        (a.event.occurred_at, &a.event.event_id).cmp(&(b.event.occurred_at, &b.event.event_id))
    });
    Ok(finish(records, reasons))
}

/// Normalizes a commit of `repository` into a `CommitAuthored` record.
pub(crate) fn commit_record(
    item: SearchCommit,
    repository: Repository,
    now: Timestamp,
    source: Source,
) -> Option<CommitRecord> {
    let sha = item.sha.clone()?;
    let detail = item.commit.as_ref()?;
    let committed_at = detail
        .committer
        .as_ref()
        .and_then(|c| c.date.as_deref())
        .and_then(parse_time)?;
    let author_date = detail
        .author
        .as_ref()
        .and_then(|a| a.date.as_deref())
        .and_then(parse_time);
    let actor = item
        .author
        .as_ref()
        .map_or_else(Actor::unknown, RestUser::actor);
    let committer = item
        .committer
        .as_ref()
        .map(RestUser::actor)
        .filter(|a| !a.is_unknown());
    let event = ActivityEvent {
        event_id: EventId::commit(&repository.id, &sha),
        event_type: EventType::CommitAuthored,
        actor,
        repository_id: repository.id.clone(),
        entity_id: sha.clone(),
        parent_pr_id: None,
        occurred_at: committed_at,
        observed_at: now,
        source,
        url: item.html_url.clone(),
        meta: EventMeta {
            sha: Some(sha),
            author_date,
            is_merge_commit: item.parents.as_ref().map(|p| p.len() > 1),
            committer,
            message_headline: detail
                .message
                .as_deref()
                .map(ghusage_core::commit::headline)
                .filter(|h| !h.is_empty())
                .map(|h| crate::http::truncate(h, 300)),
            ..EventMeta::default()
        },
        coverage: CoverageStatus::Complete,
    };
    Some(CommitRecord { repository, event })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchIssue {
    node_id: Option<String>,
    number: Option<u64>,
    repository_url: Option<String>,
    updated_at: Option<String>,
    pull_request: Option<Value>,
}

pub(crate) async fn issues(
    t: &Transport,
    qualifiers: &str,
    kind: IssueKind,
    since: Timestamp,
    public_only: bool,
) -> Result<Fetched<Vec<IssueRef>>, ApiError> {
    let kind_q = match kind {
        IssueKind::PullRequest => "is:pr",
        IssueKind::Issue => "is:issue",
    };
    let visibility = if public_only { " is:public" } else { "" };
    let qualifiers = qualifiers.trim();
    let outcome = search(t, "issues", "created", Range::since(since), |range| {
        format!("{qualifiers} {kind_q} updated:{range}{visibility}")
            .trim()
            .to_string()
    })
    .await?;
    let mut reasons = outcome.reasons;
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    let mut bad = 0usize;
    for v in outcome.items {
        let Ok(item) = serde_json::from_value::<SearchIssue>(v) else {
            bad += 1;
            continue;
        };
        let is_pr = item.pull_request.as_ref().is_some_and(|p| !p.is_null());
        let item_kind = if is_pr {
            IssueKind::PullRequest
        } else {
            IssueKind::Issue
        };
        if item_kind != kind {
            continue;
        }
        let parsed = (|| {
            let (owner, name) = owner_name(item.repository_url.as_deref()?)?;
            Some(IssueRef {
                node_id: item.node_id.clone()?,
                owner,
                name,
                number: item.number?,
                kind: item_kind,
                updated_at: item.updated_at.as_deref().and_then(parse_time)?,
            })
        })();
        match parsed {
            Some(r) => {
                if seen.insert(r.node_id.clone()) {
                    refs.push(r);
                }
            }
            None => bad += 1,
        }
    }
    if bad > 0 {
        reasons.push(format!("{bad} issue search result(s) could not be read"));
    }
    refs.sort_by(|a, b| (a.updated_at, &a.node_id).cmp(&(b.updated_at, &b.node_id)));
    Ok(finish(refs, reasons))
}

/// `owner` and `name` from `https://api.github.com/repos/{owner}/{name}`.
fn owner_name(repository_url: &str) -> Option<(String, String)> {
    let mut parts = repository_url.trim_end_matches('/').rsplit('/');
    let name = parts.next()?.to_string();
    let owner = parts.next()?.to_string();
    if parts.next()? != "repos" || owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_whole_seconds_inclusively() {
        let p = Period::new(
            "2024-01-01T00:00:00Z".parse().unwrap(),
            "2024-01-02T00:00:00Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(
            Range::of(p).qualifier(),
            "2024-01-01T00:00:00+00:00..2024-01-01T23:59:59+00:00"
        );
    }

    #[test]
    fn open_ranges_have_no_upper_bound_and_split_into_a_closed_and_an_open_half() {
        let r = Range {
            lo: ts("2024-01-01T00:00:00Z"),
            hi: ts("2024-01-03T00:00:00Z"),
            open: true,
        };
        assert_eq!(r.qualifier(), ">=2024-01-01T00:00:00+00:00");
        let (earlier, later) = r.halves();
        assert_eq!(
            earlier.qualifier(),
            "2024-01-01T00:00:00+00:00..2024-01-01T23:59:59+00:00"
        );
        assert_eq!(later.qualifier(), ">=2024-01-02T00:00:00+00:00");
        let (_, closed) = Range { open: false, ..r }.halves();
        assert_eq!(
            closed.qualifier(),
            "2024-01-02T00:00:00+00:00..2024-01-02T23:59:59+00:00"
        );
    }

    fn ts(s: &str) -> i64 {
        s.parse::<Timestamp>().unwrap().as_second()
    }

    #[test]
    fn owner_and_name_from_repository_url() {
        assert_eq!(
            owner_name("https://api.github.com/repos/octo/hello"),
            Some(("octo".into(), "hello".into()))
        );
        assert_eq!(owner_name("https://api.github.com/octo/hello"), None);
    }
}
