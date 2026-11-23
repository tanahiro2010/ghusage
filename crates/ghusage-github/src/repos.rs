//! Repository discovery and listing of default-branch commits through the
//! REST API. This complements commit search, whose index can miss commits.

use std::collections::HashSet;

use ghusage_core::model::{Repository, Source, Visibility};
use ghusage_core::time::Period;
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;

use crate::http::{Transport, parse_time};
use crate::pull_request::RestUser;
use crate::search::{SearchCommit, commit_record, finish};
use crate::{ApiError, CommitRecord, Fetched, PushedRepository};

/// Upper bound on followed pages, as a guard against link loops.
const MAX_PAGES: usize = 10_000;

/// How far beyond the requested period the commits list is queried. The
/// endpoint filters on the committer date, but it walks history the way
/// `git log` does, so a margin guards against slightly out-of-order dates;
/// results are filtered exactly afterwards.
const LIST_MARGIN_DAYS: i64 = 1;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestPermissions {
    admin: Option<bool>,
    maintain: Option<bool>,
    push: Option<bool>,
    triage: Option<bool>,
    pull: Option<bool>,
}

impl RestPermissions {
    /// The permission name GraphQL would report as `viewerPermission`.
    fn as_viewer_permission(&self) -> Option<String> {
        let level = if self.admin == Some(true) {
            "ADMIN"
        } else if self.maintain == Some(true) {
            "MAINTAIN"
        } else if self.push == Some(true) {
            "WRITE"
        } else if self.triage == Some(true) {
            "TRIAGE"
        } else if self.pull == Some(true) {
            "READ"
        } else {
            return None;
        };
        Some(level.into())
    }
}

/// A repository object of the REST API.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RestRepo {
    node_id: Option<String>,
    name: Option<String>,
    private: Option<bool>,
    visibility: Option<String>,
    fork: Option<bool>,
    html_url: Option<String>,
    owner: Option<RestUser>,
    permissions: Option<RestPermissions>,
    pushed_at: Option<String>,
}

impl RestRepo {
    pub(crate) fn to_repository(&self) -> Option<Repository> {
        let owner = self.owner.as_ref()?;
        let visibility = match (self.visibility.as_deref(), self.private) {
            (Some(v), _) if Visibility::parse(v) != Visibility::Unknown => Visibility::parse(v),
            (_, Some(true)) => Visibility::Private,
            (_, Some(false)) => Visibility::Public,
            _ => Visibility::Unknown,
        };
        Some(Repository {
            id: self.node_id.clone()?,
            owner: owner.login.clone()?,
            name: self.name.clone()?,
            owner_kind: owner.actor().kind,
            visibility,
            is_fork: self.fork.unwrap_or(false),
            viewer_permission: self
                .permissions
                .as_ref()
                .and_then(RestPermissions::as_viewer_permission),
            accessible: true,
            url: self.html_url.clone(),
        })
    }
}

/// Repositories sorted by last push, newest first, until one was last
/// pushed before `pushed_since`. `user` is `None` for the authenticated
/// user (`GET /user/repos`: owned, collaborator and organization member
/// repositories) and a login otherwise (`GET /users/{login}/repos`: only
/// repositories the user owns).
pub(crate) async fn pushed_repositories(
    t: &Transport,
    user: Option<&str>,
    pushed_since: Timestamp,
) -> Result<Vec<PushedRepository>, ApiError> {
    let first = user.map_or_else(
        || {
            t.rest_url(
                &["user", "repos"],
                &[
                    ("affiliation", "owner,collaborator,organization_member"),
                    ("sort", "pushed"),
                    ("direction", "desc"),
                    ("per_page", "100"),
                ],
            )
        },
        |login| {
            t.rest_url(
                &["users", login, "repos"],
                &[
                    ("type", "owner"),
                    ("sort", "pushed"),
                    ("direction", "desc"),
                    ("per_page", "100"),
                ],
            )
        },
    );
    let mut out = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_urls = HashSet::new();
    let mut next = Some(first);
    while let Some(url) = next.take() {
        if !seen_urls.insert(url.to_string()) || seen_urls.len() > MAX_PAGES {
            return Err(ApiError::InvalidResponse(
                "repository list pagination did not terminate".into(),
            ));
        }
        let (value, headers) = t.get_json(&url).await?;
        let items = value
            .as_array()
            .ok_or_else(|| ApiError::InvalidResponse("repository list is not an array".into()))?;
        let mut reached_older = false;
        for v in items {
            let Ok(raw) = serde_json::from_value::<RestRepo>(v.clone()) else {
                return Err(ApiError::InvalidResponse(
                    "unreadable repository in repository list".into(),
                ));
            };
            // Never-pushed repositories have no commits to list.
            let Some(pushed_at) = raw.pushed_at.as_deref().and_then(parse_time) else {
                continue;
            };
            if pushed_at < pushed_since {
                reached_older = true;
                continue;
            }
            let repository = raw.to_repository().ok_or_else(|| {
                ApiError::InvalidResponse("repository without id, owner or name".into())
            })?;
            if seen_ids.insert(repository.id.clone()) {
                out.push(PushedRepository {
                    repository,
                    pushed_at: Some(pushed_at),
                });
            }
        }
        if reached_older {
            break;
        }
        next = t.next_link(&headers)?;
    }
    Ok(out)
}

/// Commits on the default branch of `repository` authored by `author_login`
/// with committer date in `period` (`GET /repos/{owner}/{name}/commits`).
/// An empty repository (HTTP 409) has no commits.
pub(crate) async fn default_branch_commits(
    t: &Transport,
    repository: &Repository,
    author_login: &str,
    period: Period,
) -> Result<Fetched<Vec<CommitRecord>>, ApiError> {
    let margin = SignedDuration::from_secs(LIST_MARGIN_DAYS * 24 * 60 * 60);
    let since = (period.start - margin).to_string();
    let until = (period.end + margin).to_string();
    let first = t.rest_url(
        &["repos", &repository.owner, &repository.name, "commits"],
        &[
            ("author", author_login),
            ("since", since.as_str()),
            ("until", until.as_str()),
            ("per_page", "100"),
        ],
    );
    let items = match t
        .get_paginated(first, |v| v.as_array().map(|a| a.to_vec()))
        .await
    {
        Ok(items) => items,
        Err(ApiError::Http { status: 409, .. }) => Vec::new(),
        Err(e) => return Err(e),
    };
    let now = Timestamp::now();
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    let mut bad = 0usize;
    for v in items {
        let Ok(item) = serde_json::from_value::<SearchCommit>(v) else {
            bad += 1;
            continue;
        };
        let Some(record) = commit_record(item, repository.clone(), now, Source::Rest) else {
            bad += 1;
            continue;
        };
        if !period.contains(record.event.occurred_at) {
            continue;
        }
        if seen.insert(record.event.event_id.clone()) {
            records.push(record);
        }
    }
    let mut reasons = Vec::new();
    if bad > 0 {
        reasons.push(format!(
            "{bad} commit(s) of {} could not be read",
            repository.full_name()
        ));
    }
    records.sort_by(|a, b| {
        (a.event.occurred_at, &a.event.event_id).cmp(&(b.event.occurred_at, &b.event.event_id))
    });
    Ok(finish(records, reasons))
}
