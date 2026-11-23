//! SQLite storage for ghusage: schema, migrations, idempotent upserts,
//! coverage records, sync bookkeeping and the sync lock.
//!
//! # Design notes
//!
//! - Every statement binds values as parameters; SQL text never contains
//!   user or GitHub data.
//! - Timestamps are stored as UTC epoch seconds plus a sub-second
//!   nanoseconds column, so they round-trip exactly.
//! - `activity_events` is the source of truth for aggregation. The review,
//!   comment, check-run and commit tables are thin projections maintained
//!   from events when a batch is applied.
//! - Repositories and pull requests are shared rows; which account each
//!   repository, pull request, issue and event was fetched for is recorded
//!   in `account_*` association tables, so different subjects never mix.
//! - A batch that references a repository or pull request that is neither
//!   in the batch nor already stored is rejected with
//!   [`StorageError::Data`]; nothing from that batch is persisted. No
//!   placeholder rows are invented.

mod codec;
mod schema;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::metrics::{Dataset, LineCounts, PrDiff};
use ghusage_core::model::{
    ActivityEvent, CheckOutcome, Collection, EventId, EventMeta, EventType, Issue, PrState,
    PullRequest, Repository, ReviewThread, Source, Subject, Visibility,
};
use ghusage_core::time::Period;
use jiff::Timestamp;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Row, Transaction, params};

use codec::{
    actor, actor_kind_str, from_json, nanos, opt_from_json, opt_nanos, opt_secs, opt_time,
    opt_to_json, secs, time, to_json,
};

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cache database is corrupt: {0}")]
    Corrupt(String),
    #[error("cache database schema version {found} is newer than supported version {supported}")]
    TooNew { found: u32, supported: u32 },
    #[error("cache migration from version {from} to {to} failed: {message}")]
    Migration { from: u32, to: u32, message: String },
    #[error("another ghusage sync is already running (lock: {0})")]
    Locked(PathBuf),
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cache data error: {0}")]
    Data(String),
}

impl StorageError {
    /// True when the error comes from missing write access to the cache
    /// file or its directory.
    pub fn is_read_only(&self) -> bool {
        match self {
            Self::Sqlite(e) => matches!(
                e.sqlite_error_code(),
                Some(ErrorCode::ReadOnly | ErrorCode::CannotOpen | ErrorCode::PermissionDenied)
            ),
            Self::Io(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            _ => false,
        }
    }
}

/// Links a pull request to a commit. `is_current` is false when a resync
/// observed that the commit is no longer part of the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCommitLink {
    pub pr_id: String,
    pub repository_id: String,
    pub sha: String,
    pub is_current: bool,
}

/// Result of fetching one pull request's internal activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSnapshot {
    pub pr_id: String,
    pub observed_at: Timestamp,
    /// Collections fetched completely in this sync. For each, stored events
    /// of this PR in that collection whose ids are not in `present` get
    /// `removed_observed_at = observed_at`; present ones get it cleared.
    /// `Checks` covers only the pull request's head commit (its `head_sha`
    /// as stored, including this batch's update): check runs and commit
    /// statuses of earlier head commits are history and are never marked
    /// removed by a later fetch.
    /// For `ReviewComments`, stored threads of this PR not in
    /// `present_threads` are deleted. For `Commits`, stored commit links of
    /// this PR that are not sent as current links in the same batch are
    /// kept but marked `is_current = false`.
    pub complete_collections: Vec<Collection>,
    pub present: HashSet<EventId>,
    pub present_threads: HashSet<String>,
}

/// Everything written by one atomic step of a sync.
#[derive(Debug, Clone, Default)]
pub struct WriteBatch {
    /// Login of the subject the data was fetched for.
    pub account: String,
    pub repositories: Vec<Repository>,
    pub pull_requests: Vec<PullRequest>,
    pub issues: Vec<Issue>,
    pub threads: Vec<ReviewThread>,
    pub events: Vec<ActivityEvent>,
    pub pr_commits: Vec<PrCommitLink>,
    pub pr_snapshots: Vec<PrSnapshot>,
    /// Lines added and deleted by commits.
    pub commit_stats: Vec<CommitStats>,
    /// Latest observed diffs of pull requests, by pull request id.
    pub pr_diffs: Vec<(String, PrDiff)>,
    /// Parents of the commit each merged pull request was merged with, by
    /// pull request id (2: merge commit, 1: squash or rebase).
    pub pr_merge_commit_parents: Vec<(String, u64)>,
    pub coverage: Vec<CoverageRecord>,
    /// `(key, value)` sync cursors for `account`.
    pub cursors: Vec<(String, String)>,
}

/// A commit stored for an account, as [`Store::commits_in`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommit {
    pub repository_id: String,
    /// The SHA in lower case.
    pub sha: String,
    /// How the commit was found.
    pub source: Source,
}

/// Lines added and deleted by one commit, as GitHub reported them at
/// `observed_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStats {
    pub repository_id: String,
    pub sha: String,
    pub counts: LineCounts,
    pub observed_at: Timestamp,
}

/// Bookkeeping of one sync run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRun {
    pub id: i64,
    pub account: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// `running`, `success`, `partial`, `failed`.
    pub status: String,
    pub period_start: Option<Timestamp>,
    pub period_end: Option<Timestamp>,
    /// JSON object with API statistics (request count, retries, ...).
    pub stats_json: Option<String>,
    pub message: Option<String>,
}

/// Sync state of a stored pull request, used for incremental sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSyncState {
    pub pr_id: String,
    pub updated_at: Option<Timestamp>,
    pub synced_at: Option<Timestamp>,
    pub coverage: ghusage_core::coverage::CoverageStatus,
}

/// Size and row counts of the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub path: Option<PathBuf>,
    pub file_size_bytes: u64,
    pub schema_version: u32,
    pub repositories: u64,
    pub pull_requests: u64,
    pub issues: u64,
    pub events: u64,
    pub accounts: u64,
}

/// The cache database.
pub struct Store {
    conn: Connection,
    path: Option<PathBuf>,
}

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Turns SQLite corruption codes into [`StorageError::Corrupt`] with a
/// message that tells the user what to do.
fn classify(e: StorageError) -> StorageError {
    if let StorageError::Sqlite(se) = &e
        && matches!(
            se.sqlite_error_code(),
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
        )
    {
        return StorageError::Corrupt(format!(
            "{se}. The cache file is damaged; move or delete it and sync again to rebuild it"
        ));
    }
    e
}

fn configure(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    Ok(())
}

/// Reads one row of every table so that damaged table pages are detected
/// at open time instead of in the middle of a later operation.
fn probe(conn: &Connection) -> Result<()> {
    for t in schema::DATA_TABLES {
        // Table names are compile-time constants.
        conn.query_row(&format!("SELECT 1 FROM {t} LIMIT 1"), [], |_| Ok(()))
            .optional()?;
    }
    Ok(())
}

fn wal_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// `file:` URI that opens `path` as immutable (no locking, no WAL access).
fn immutable_uri(path: &Path) -> Result<String> {
    let abs = std::path::absolute(path)?;
    let s = abs.to_str().ok_or_else(|| {
        StorageError::Data(format!("cache path is not UTF-8: {}", path.display()))
    })?;
    let mut uri = String::from("file:");
    if !s.starts_with('/') {
        // Windows drive paths: file:/C:/...
        uri.push('/');
    }
    for b in s.bytes() {
        match b {
            b'\\' => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                uri.push(b as char);
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    Ok(uri)
}

/// Creates `dir` (and its parents) if missing; a directory created here is
/// readable only by the user.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    restrict_to_user(dir, 0o700);
    Ok(())
}

/// Best effort: on Unix, sets `path`'s permission bits to `mode`. Fails
/// silently, e.g. for a read-only file the user does not own.
fn restrict_to_user(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o777 != mode
            && meta.permissions().mode() & 0o200 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

fn check_header(path: &Path) -> Result<()> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut buf = Vec::with_capacity(16);
    file.take(16).read_to_end(&mut buf)?;
    if !buf.is_empty() && buf.as_slice() != SQLITE_MAGIC {
        return Err(StorageError::Corrupt(format!(
            "{} is not a SQLite database. Move or delete it and sync again to rebuild the cache",
            path.display()
        )));
    }
    Ok(())
}

const ASSOC_REPO: &str = "INSERT INTO account_repositories (account_id, repository_id) \
                          VALUES (?1, ?2) ON CONFLICT DO NOTHING";
const ASSOC_PR: &str = "INSERT INTO account_pull_requests (account_id, pr_id) \
                        VALUES (?1, ?2) ON CONFLICT DO NOTHING";
const ASSOC_ISSUE: &str = "INSERT INTO account_issues (account_id, issue_id) \
                           VALUES (?1, ?2) ON CONFLICT DO NOTHING";
const ASSOC_EVENT: &str = "INSERT INTO account_events (account_id, event_id) \
                           VALUES (?1, ?2) ON CONFLICT DO NOTHING";

const REPO_COLS: &str = "r.id, r.owner, r.name, r.owner_kind, r.visibility, r.is_fork, \
                         r.viewer_permission, r.accessible, r.url";
const PR_COLS: &str = "p.*";
const ISSUE_COLS: &str = "i.*";
const THREAD_COLS: &str = "t.*";
const EVENT_COLS: &str = "e.*";

include!("store/open.rs");
include!("store/write.rs");
include!("store/read.rs");
include!("store/rows.rs");
include!("store/runs.rs");
include!("store/lock.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_events_are_found_through_the_parent_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let plan: Vec<String> = query_list(
            &store.conn,
            &format!("EXPLAIN QUERY PLAN {}", pr_events_sql()),
            params![1, "PR_x"],
            |r| Ok(r.get::<_, String>(3)?),
        )
        .unwrap();
        assert!(
            plan[0].contains("activity_events_parent_pr"),
            "unexpected plan: {plan:?}"
        );
    }
}
