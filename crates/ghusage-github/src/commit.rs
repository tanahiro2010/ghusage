//! One commit with its files and diff (`GET /repos/{owner}/{repo}/commits/{ref}`).

use chrono::Utc;
use ghusage_core::commit::{CommitDetail, CommitFile, CommitPerson, DiffStats, FileStatus};
use ghusage_core::model::Visibility;
use serde::Deserialize;
use serde_json::Value;

use crate::ApiError;
use crate::http::{Transport, parse_time};
use crate::pull_request::RestUser;

/// Files requested per page.
const FILES_PER_PAGE: &str = "100";
/// GitHub lists at most 3000 files of a commit, which is 30 pages of 100.
const MAX_FILE_PAGES: usize = 30;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestCommit {
    sha: Option<String>,
    html_url: Option<String>,
    commit: Option<GitData>,
    author: Option<RestUser>,
    committer: Option<RestUser>,
    parents: Vec<RestParent>,
    stats: Option<RestStats>,
    files: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GitData {
    message: Option<String>,
    author: Option<GitSignature>,
    committer: Option<GitSignature>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GitSignature {
    name: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestParent {
    sha: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestStats {
    additions: Option<u64>,
    deletions: Option<u64>,
    total: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestFile {
    filename: Option<String>,
    previous_filename: Option<String>,
    status: Option<String>,
    additions: Option<u64>,
    deletions: Option<u64>,
    changes: Option<u64>,
    patch: Option<String>,
}

fn person(sig: Option<&GitSignature>, user: Option<&RestUser>) -> CommitPerson {
    CommitPerson {
        name: sig.and_then(|s| s.name.clone()).filter(|n| !n.is_empty()),
        login: user.and_then(|u| u.login.clone()),
        date: sig.and_then(|s| s.date.as_deref()).and_then(parse_time),
    }
}

fn file(v: Value) -> Option<CommitFile> {
    let f: RestFile = serde_json::from_value(v).ok()?;
    let additions = f.additions.unwrap_or(0);
    let deletions = f.deletions.unwrap_or(0);
    Some(CommitFile {
        filename: f.filename?,
        previous_filename: f.previous_filename,
        status: f
            .status
            .as_deref()
            .map(FileStatus::parse)
            .unwrap_or(FileStatus::Unknown),
        additions,
        deletions,
        changes: f.changes.unwrap_or(additions + deletions),
        patch: f.patch,
    })
}

/// Fetches a commit and every page of its files. `reference` is a full or
/// abbreviated SHA. A missing commit (HTTP 404, or 422 for an unknown
/// SHA) and an empty repository (409) are `NotFound`.
pub(crate) async fn fetch(
    t: &Transport,
    owner: &str,
    name: &str,
    reference: &str,
) -> Result<CommitDetail, ApiError> {
    let what = format!("commit {owner}/{name}@{reference}");
    let first = t.rest_url(
        &["repos", owner, name, "commits", reference],
        &[("per_page", FILES_PER_PAGE)],
    );
    let not_found = |e: ApiError| match e {
        ApiError::NotFound(_)
        | ApiError::Http {
            status: 409 | 422, ..
        } => ApiError::NotFound(what.clone()),
        e => e,
    };
    let (value, headers) = t.get_json(&first).await.map_err(not_found)?;
    let raw: RestCommit = serde_json::from_value(value)
        .map_err(|e| ApiError::InvalidResponse(format!("{what}: {e}")))?;
    let sha = raw
        .sha
        .clone()
        .ok_or_else(|| ApiError::InvalidResponse(format!("{what} is missing `sha`")))?;
    let mut files: Vec<CommitFile> = Vec::new();
    let mut bad = 0usize;
    for v in raw.files.unwrap_or_default() {
        match file(v) {
            Some(f) => files.push(f),
            None => bad += 1,
        }
    }
    let mut next = t.next_link(&headers)?;
    let mut pages = 1;
    let mut seen = std::collections::HashSet::from([first.to_string()]);
    while let Some(url) = next.take() {
        if pages >= MAX_FILE_PAGES {
            next = Some(url);
            break;
        }
        if !seen.insert(url.to_string()) {
            return Err(ApiError::InvalidResponse(format!(
                "pagination of {what} did not terminate"
            )));
        }
        let (value, headers) = t.get_json(&url).await.map_err(not_found)?;
        let page: RestCommit = serde_json::from_value(value)
            .map_err(|e| ApiError::InvalidResponse(format!("{what}: {e}")))?;
        for v in page.files.unwrap_or_default() {
            match file(v) {
                Some(f) => files.push(f),
                None => bad += 1,
            }
        }
        pages += 1;
        next = t.next_link(&headers)?;
    }
    let data = raw.commit.unwrap_or_default();
    let stats = raw.stats.unwrap_or_default();
    let additions = stats.additions.unwrap_or(0);
    let deletions = stats.deletions.unwrap_or(0);
    Ok(CommitDetail {
        repository: format!("{owner}/{name}"),
        sha,
        url: raw.html_url,
        message: data.message.clone().unwrap_or_default(),
        author: person(data.author.as_ref(), raw.author.as_ref()),
        committer: person(data.committer.as_ref(), raw.committer.as_ref()),
        parents: raw.parents.into_iter().filter_map(|p| p.sha).collect(),
        stats: DiffStats {
            additions,
            deletions,
            total: stats.total.unwrap_or(additions + deletions),
        },
        files,
        files_truncated: next.is_some() || bad > 0,
        visibility: Visibility::Unknown,
        fetched_at: Utc::now(),
    })
}
