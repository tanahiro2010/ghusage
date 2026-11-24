//! Commit details shared by the command and terminal UI.
//!
//! Commits are looked up in the cache first and fetched
//! from GitHub only when missing; a fetched commit is stored for offline
//! use. Commit content never affects any count.

use ghusage_core::commit::{CommitDetail, CommitPerson, FileStatus};
use jiff::Timestamp;
use serde::Serialize;

use crate::error::AppError;
use crate::report::{SCHEMA_VERSION, short_hash};
use crate::session::Session;

/// A commit to show: repository and full or abbreviated SHA.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitTarget {
    /// Repository owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Full or abbreviated commit SHA.
    pub sha: String,
}

impl CommitTarget {
    /// Parses `owner/repo@sha` or a GitHub commit URL.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let (owner, name, sha) = ghusage_github::parse_commit_ref(input)?;
        Some(Self { owner, name, sha })
    }

    /// From `owner/name` and a SHA, as listed in reports.
    #[must_use]
    pub fn from_parts(repository: &str, sha: &str) -> Option<Self> {
        Self::parse(&format!("{repository}@{sha}"))
    }

    /// `owner/name`.
    #[must_use]
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `owner/name@sha`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}@{}", self.owner, self.name, self.sha)
    }
}

/// Where a commit was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitSource {
    /// Commit detail was loaded from the local cache.
    Cache,
    /// Commit detail was fetched from GitHub.
    Github,
}

/// Commit detail and its origin.
#[derive(Debug, Clone)]
pub struct LoadedCommit {
    /// Fetched commit metadata and diff.
    pub detail: CommitDetail,
    /// Source used to load the detail.
    pub source: CommitSource,
}

/// Why a commit could not be shown.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// Not cached, and GitHub is not used (offline) or not reachable.
    #[error("commit {target} is not in the cache, and GitHub cannot be reached ({reason})")]
    Offline {
        /// Commit reference that could not be loaded.
        target: String,
        /// Reason a remote fetch was unavailable.
        reason: String,
    },
    /// `--public-only` is set and the repository is not public.
    #[error("{0} is not a public repository (--public-only)")]
    NotPublic(String),
    /// Another application error occurred while loading the commit.
    #[error(transparent)]
    App(#[from] AppError),
}

impl CommitError {
    /// Whether the commit could not be loaded because it was unavailable offline.
    #[must_use]
    pub const fn is_offline(&self) -> bool {
        matches!(self, Self::Offline { .. })
    }
}

impl From<CommitError> for AppError {
    fn from(e: CommitError) -> Self {
        match e {
            CommitError::App(e) => e,
            e @ CommitError::Offline { .. } => Self::Other(e.to_string()),
            e @ CommitError::NotPublic(_) => Self::InvalidArgs(e.to_string()),
        }
    }
}

/// Loads a commit from the cache or GitHub.
///
/// An uncached commit is fetched only while online. A fetched commit is stored
/// in the cache; failing to
/// store it (for example in a read-only cache) is not an error.
pub async fn load(
    session: &mut Session,
    target: &CommitTarget,
) -> Result<LoadedCommit, CommitError> {
    session.load_commit(target).await
}

/// A person of a commit in reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersonReport {
    /// Git identity name, omitted when private details are redacted.
    pub name: Option<String>,
    /// GitHub account login, when known.
    pub login: Option<String>,
    /// Identity timestamp, when known.
    pub date: Option<Timestamp>,
}

impl PersonReport {
    fn of(p: &CommitPerson, redact: bool) -> Self {
        Self {
            name: if redact { None } else { p.name.clone() },
            login: p.login.clone(),
            date: p.date,
        }
    }
}

/// Line totals reported for a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitStatsReport {
    /// Number of changed files.
    pub files: u64,
    /// Lines added.
    pub additions: u64,
    /// Lines deleted.
    pub deletions: u64,
    /// Total changed lines.
    pub total: u64,
}

/// Changed file information included in a commit report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitFileReport {
    /// File path, redacted for non-public repositories when requested.
    pub filename: String,
    /// Previous path of a renamed or copied file.
    pub previous_filename: Option<String>,
    /// GitHub change classification.
    pub status: FileStatus,
    /// Added line count.
    pub additions: u64,
    /// Deleted line count.
    pub deletions: u64,
    /// Total changed lines.
    pub changes: u64,
    /// Whether GitHub returned a diff for the file.
    pub patch_available: bool,
    /// The diff, only when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

/// A commit as shown by `ghusage commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitReport {
    /// Version of the serialized report schema.
    pub schema_version: u32,
    /// Repository display name.
    pub repository: String,
    /// Full commit SHA.
    pub sha: String,
    /// GitHub commit URL, when available.
    pub url: Option<String>,
    /// Full commit message.
    pub message: String,
    /// Commit author identity.
    pub author: PersonReport,
    /// Committer identity.
    pub committer: PersonReport,
    /// Parent commit SHAs.
    pub parents: Vec<String>,
    /// Whether the commit has multiple parents.
    pub is_merge: bool,
    /// Aggregate line statistics.
    pub stats: CommitStatsReport,
    /// Changed file details.
    pub files: Vec<CommitFileReport>,
    /// Whether GitHub omitted files beyond its response cap.
    pub files_truncated: bool,
    /// Source from which the commit was loaded.
    pub source: CommitSource,
    /// Time the commit detail was fetched.
    pub fetched_at: Timestamp,
    /// Whether private repository details were redacted.
    pub private_repositories_redacted: bool,
}

/// Builds a report for a commit.
///
/// Diffs are included only with
/// `include_patch`. With `redact_private`, a commit of a repository that is
/// not known to be public loses its repository name, URL, message, file
/// paths, diffs and git names; SHAs, logins, dates and counts stay.
#[must_use]
pub fn report(loaded: &LoadedCommit, include_patch: bool, redact_private: bool) -> CommitReport {
    let d = &loaded.detail;
    let redact = redact_private && !d.visibility.is_public();
    let files = d
        .files
        .iter()
        .map(|f| CommitFileReport {
            filename: if redact {
                "[private]".into()
            } else {
                f.filename.clone()
            },
            previous_filename: if redact {
                None
            } else {
                f.previous_filename.clone()
            },
            status: f.status,
            additions: f.additions,
            deletions: f.deletions,
            changes: f.changes,
            patch_available: f.missing_patch_reason().is_none(),
            patch: if include_patch && !redact {
                f.patch.clone()
            } else {
                None
            },
        })
        .collect();
    CommitReport {
        schema_version: SCHEMA_VERSION,
        repository: if redact {
            format!(
                "private/repo-{}",
                short_hash(&d.repository.to_ascii_lowercase())
            )
        } else {
            d.repository.clone()
        },
        sha: d.sha.clone(),
        url: if redact { None } else { d.url.clone() },
        message: if redact {
            "[private]".into()
        } else {
            d.message.clone()
        },
        author: PersonReport::of(&d.author, redact),
        committer: PersonReport::of(&d.committer, redact),
        parents: d.parents.clone(),
        is_merge: d.is_merge(),
        stats: CommitStatsReport {
            files: d.files.len() as u64,
            additions: d.stats.additions,
            deletions: d.stats.deletions,
            total: d.stats.total,
        },
        files,
        files_truncated: d.files_truncated,
        source: loaded.source,
        fetched_at: d.fetched_at,
        private_repositories_redacted: redact_private,
    }
}
