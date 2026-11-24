//! One commit with its changed files and diff, as fetched on demand from
//! GitHub. Unlike activity events this is a snapshot of content, not
//! activity: it never contributes to any count.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::Visibility;

/// A git identity of a commit: the name recorded in git, the GitHub account
/// it is linked to (if any) and the date recorded for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPerson {
    /// Name recorded in the Git commit.
    pub name: Option<String>,
    /// GitHub account linked to the commit identity.
    pub login: Option<String>,
    /// Date recorded for this identity.
    pub date: Option<Timestamp>,
}

impl CommitPerson {
    /// `login`, else the git name, else `unknown`.
    #[must_use]
    pub fn display(&self) -> &str {
        self.login
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or("unknown")
    }
}

/// How a file changed in a commit (GitHub's `files[].status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// File was added by the commit.
    Added,
    /// Existing file contents were modified.
    Modified,
    /// File was removed.
    Removed,
    /// File was renamed.
    Renamed,
    /// File was copied.
    Copied,
    /// File changed in another way.
    Changed,
    /// File contents were unchanged.
    Unchanged,
    /// GitHub returned an unrecognized status.
    Unknown,
}

impl FileStatus {
    /// Parses a GitHub file status, returning `Unknown` for unrecognized values.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "added" => Self::Added,
            "modified" => Self::Modified,
            "removed" => Self::Removed,
            "renamed" => Self::Renamed,
            "copied" => Self::Copied,
            "changed" => Self::Changed,
            "unchanged" => Self::Unchanged,
            _ => Self::Unknown,
        }
    }

    /// Returns the stable serialized file status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Removed => "removed",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::Changed => "changed",
            Self::Unchanged => "unchanged",
            Self::Unknown => "unknown",
        }
    }
}

/// One changed file of a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitFile {
    /// Path of the file in the commit.
    pub filename: String,
    /// The old path of a renamed or copied file.
    pub previous_filename: Option<String>,
    /// How GitHub classified the file change.
    pub status: FileStatus,
    /// Number of added lines.
    pub additions: u64,
    /// Number of removed lines.
    pub deletions: u64,
    /// Total changed lines reported by GitHub.
    pub changes: u64,
    /// The unified diff of the file. GitHub leaves it out for binary files
    /// and for diffs that are too large.
    pub patch: Option<String>,
}

impl CommitFile {
    /// Why there is no diff to show, or `None` when there is one.
    #[must_use]
    pub fn missing_patch_reason(&self) -> Option<&'static str> {
        if self.patch.as_deref().is_some_and(|p| !p.is_empty()) {
            return None;
        }
        Some(
            if self.changes == 0
                && matches!(
                    self.status,
                    FileStatus::Renamed | FileStatus::Copied | FileStatus::Unchanged
                )
            {
                "no content changes"
            } else {
                "no diff available from GitHub (binary file or diff too large)"
            },
        )
    }
}

/// Lines added and removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStats {
    /// Number of added lines.
    pub additions: u64,
    /// Number of removed lines.
    pub deletions: u64,
    /// Total changed lines.
    pub total: u64,
}

/// A commit with its message, people, parents, changed files and diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDetail {
    /// `owner/name` of the repository the commit was fetched from.
    pub repository: String,
    /// Full SHA.
    pub sha: String,
    /// The commit's page on GitHub.
    pub url: Option<String>,
    /// The full commit message.
    pub message: String,
    /// Author identity recorded for the commit.
    pub author: CommitPerson,
    /// Committer identity recorded for the commit.
    pub committer: CommitPerson,
    /// SHAs of the parent commits. More than one means a merge commit.
    pub parents: Vec<String>,
    /// Totals as reported by GitHub for the whole commit.
    pub stats: DiffStats,
    /// Changed file details returned by GitHub.
    pub files: Vec<CommitFile>,
    /// GitHub lists at most 3000 files of a commit; set when files are
    /// missing from `files`.
    pub files_truncated: bool,
    /// Visibility of the repository when the commit was fetched.
    pub visibility: Visibility,
    /// Time this commit detail was fetched.
    pub fetched_at: Timestamp,
}

impl CommitDetail {
    /// The first line of the message.
    #[must_use]
    pub fn headline(&self) -> &str {
        headline(&self.message)
    }

    /// Whether the commit has multiple parent commits.
    #[must_use]
    pub const fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// Additions and deletions summed over the listed files.
    #[must_use]
    pub fn file_totals(&self) -> DiffStats {
        let mut s = DiffStats::default();
        for f in &self.files {
            s.additions += f.additions;
            s.deletions += f.deletions;
            s.total += f.changes;
        }
        s
    }
}

/// The first line of a commit message, without trailing whitespace.
#[must_use]
pub fn headline(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim_end()
}

/// The first seven characters of a SHA, the usual abbreviation.
#[must_use]
pub fn short_sha(sha: &str) -> &str {
    let end = sha.char_indices().nth(7).map_or(sha.len(), |(i, _)| i);
    &sha[..end]
}

/// What a line of a unified diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    /// `@@ -a,b +c,d @@` hunk header.
    Hunk,
    /// Added source line.
    Added,
    /// Removed source line.
    Removed,
    /// Unchanged context line.
    Context,
    /// `\ No newline at end of file` and other notes.
    Note,
}

/// Classifies the lines of a unified diff as GitHub returns it in
/// `files[].patch` (hunks only, no file headers).
pub fn diff_lines(patch: &str) -> impl Iterator<Item = (DiffLineKind, &str)> {
    patch.lines().map(|line| {
        let kind = if line.starts_with("@@") {
            DiffLineKind::Hunk
        } else if line.starts_with('+') {
            DiffLineKind::Added
        } else if line.starts_with('-') {
            DiffLineKind::Removed
        } else if line.starts_with('\\') {
            DiffLineKind::Note
        } else {
            DiffLineKind::Context
        };
        (kind, line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_diff_lines() {
        let patch = "@@ -1,3 +1,3 @@ fn main\n context\n-old\n+new\n\\ No newline at end of file";
        let kinds: Vec<DiffLineKind> = diff_lines(patch).map(|(k, _)| k).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Hunk,
                DiffLineKind::Context,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Note,
            ]
        );
    }

    #[test]
    fn headline_and_short_sha() {
        assert_eq!(headline("fix: parser\n\nbody"), "fix: parser");
        assert_eq!(headline(""), "");
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn missing_patch_reasons() {
        let mut f = CommitFile {
            filename: "a.png".into(),
            previous_filename: None,
            status: FileStatus::Added,
            additions: 0,
            deletions: 0,
            changes: 0,
            patch: None,
        };
        assert!(f.missing_patch_reason().unwrap().contains("binary"));
        f.status = FileStatus::Renamed;
        assert_eq!(f.missing_patch_reason(), Some("no content changes"));
        f.patch = Some("@@ -1 +1 @@\n-a\n+b".into());
        assert_eq!(f.missing_patch_reason(), None);
    }
}
