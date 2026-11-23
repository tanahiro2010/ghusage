//! One commit with its files and diff, shared by the `commit` command and
//! the terminal UI. Commits are looked up in the cache first and fetched
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
    pub owner: String,
    pub name: String,
    pub sha: String,
}

impl CommitTarget {
    /// Parses `owner/repo@sha` or a GitHub commit URL.
    pub fn parse(input: &str) -> Option<Self> {
        let (owner, name, sha) = ghusage_github::parse_commit_ref(input)?;
        Some(Self { owner, name, sha })
    }

    /// From `owner/name` and a SHA, as listed in reports.
    pub fn from_parts(repository: &str, sha: &str) -> Option<Self> {
        Self::parse(&format!("{repository}@{sha}"))
    }

    /// `owner/name`.
    pub fn repository(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// `owner/name@sha`.
    pub fn label(&self) -> String {
        format!("{}/{}@{}", self.owner, self.name, self.sha)
    }
}

/// Where a commit was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitSource {
    Cache,
    Github,
}

#[derive(Debug, Clone)]
pub struct LoadedCommit {
    pub detail: CommitDetail,
    pub source: CommitSource,
}

/// Why a commit could not be shown.
#[derive(Debug, thiserror::Error)]
pub enum CommitError {
    /// Not cached, and GitHub is not used (offline) or not reachable.
    #[error("commit {target} is not in the cache, and GitHub cannot be reached ({reason})")]
    Offline { target: String, reason: String },
    /// `--public-only` is set and the repository is not public.
    #[error("{0} is not a public repository (--public-only)")]
    NotPublic(String),
    #[error(transparent)]
    App(#[from] AppError),
}

impl CommitError {
    pub fn is_offline(&self) -> bool {
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

/// The commit from the cache, or from GitHub when it is not cached and the
/// session is online. A fetched commit is stored in the cache; failing to
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
    pub name: Option<String>,
    pub login: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitStatsReport {
    pub files: u64,
    pub additions: u64,
    pub deletions: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitFileReport {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: FileStatus,
    pub additions: u64,
    pub deletions: u64,
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
    pub schema_version: u32,
    pub repository: String,
    pub sha: String,
    pub url: Option<String>,
    pub message: String,
    pub author: PersonReport,
    pub committer: PersonReport,
    pub parents: Vec<String>,
    pub is_merge: bool,
    pub stats: CommitStatsReport,
    pub files: Vec<CommitFileReport>,
    pub files_truncated: bool,
    pub source: CommitSource,
    pub fetched_at: Timestamp,
    pub private_repositories_redacted: bool,
}

/// Builds the report of a commit. Diffs are included only with
/// `include_patch`. With `redact_private`, a commit of a repository that is
/// not known to be public loses its repository name, URL, message, file
/// paths, diffs and git names; SHAs, logins, dates and counts stay.
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
