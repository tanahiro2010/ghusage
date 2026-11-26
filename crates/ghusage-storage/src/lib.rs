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
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ghusage_core::coverage::{
    CoverageGeneration, CoverageRecord, CoverageScope, CoverageStatus, CoverageVisibility, DataKind,
};
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

/// Result of a cache operation.
pub type Result<T> = std::result::Result<T, StorageError>;

/// Errors returned while opening, reading, or writing the cache.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// SQLite rejected or failed an operation.
    #[error("cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The cache file is corrupt or is not a valid database.
    #[error("cache database is corrupt: {0}")]
    Corrupt(String),
    /// The cache was written by a newer schema than this version supports.
    #[error("cache database schema version {found} is newer than supported version {supported}")]
    TooNew {
        /// Schema version found in the database.
        found: u32,
        /// Newest schema version supported by this binary.
        supported: u32,
    },
    /// A schema migration failed.
    #[error("cache migration from version {from} to {to} failed: {message}")]
    Migration {
        /// Schema version before the failed migration.
        from: u32,
        /// Schema version the migration was applying.
        to: u32,
        /// Underlying migration error.
        message: String,
    },
    /// Another process holds the sync lock.
    #[error("another ghusage sync is already running (lock: {0})")]
    Locked(PathBuf),
    /// The cache could not be read or written.
    #[error("cache I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Stored or supplied data violates cache invariants.
    #[error("cache data error: {0}")]
    Data(String),
}

impl StorageError {
    /// True when the error comes from missing write access to the cache
    /// file or its directory.
    #[must_use]
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
    /// GitHub node ID of the pull request.
    pub pr_id: String,
    /// GitHub node ID of the repository.
    pub repository_id: String,
    /// Commit SHA.
    pub sha: String,
    /// Whether this commit is currently in the pull request.
    pub is_current: bool,
}

/// Result of fetching one pull request's internal activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSnapshot {
    /// GitHub node ID of the pull request.
    pub pr_id: String,
    /// Time this snapshot was fetched.
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
    /// Event identifiers present in the completed collections.
    pub present: HashSet<EventId>,
    /// Thread identifiers present in the fetched thread collection.
    pub present_threads: HashSet<String>,
}

/// Everything written by one atomic step of a sync.
#[derive(Debug, Clone, Default)]
pub struct WriteBatch {
    /// Login of the subject the data was fetched for.
    pub account: String,
    /// Repository snapshots to store.
    pub repositories: Vec<Repository>,
    /// Pull request snapshots to store.
    pub pull_requests: Vec<PullRequest>,
    /// Issue snapshots to store.
    pub issues: Vec<Issue>,
    /// Review thread snapshots to store.
    pub threads: Vec<ReviewThread>,
    /// Activity events to upsert.
    pub events: Vec<ActivityEvent>,
    /// Pull request commit membership to update.
    pub pr_commits: Vec<PrCommitLink>,
    /// Complete pull request snapshots for reconciliation.
    pub pr_snapshots: Vec<PrSnapshot>,
    /// Pull requests cached only because the account opened their detail.
    pub viewed_pull_requests: Vec<String>,
    /// Lines added and deleted by commits.
    /// Commit line statistics to store.
    pub commit_stats: Vec<CommitStats>,
    /// Latest observed diffs of pull requests, by pull request id.
    pub pr_diffs: Vec<(String, PrDiff)>,
    /// Parents of the commit each merged pull request was merged with, by
    /// pull request id (2: merge commit, 1: squash or rebase).
    pub pr_merge_commit_parents: Vec<(String, u64)>,
    /// Coverage records to store.
    pub coverage: Vec<CoverageRecord>,
    /// `(key, value)` sync cursors for `account`.
    pub cursors: Vec<(String, String)>,
}

/// A commit stored for an account, as [`Store::commits_in`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommit {
    /// Repository containing the commit.
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
    /// Repository containing the commit.
    pub repository_id: String,
    /// Commit SHA.
    pub sha: String,
    /// Added and deleted line counts.
    pub counts: LineCounts,
    /// Time the statistics were fetched.
    pub observed_at: Timestamp,
}

/// Bookkeeping of one sync run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRun {
    /// Database identifier of this run.
    pub id: i64,
    /// Account synchronized by this run.
    pub account: String,
    /// Run start time.
    pub started_at: Timestamp,
    /// Run completion time, when finished.
    pub finished_at: Option<Timestamp>,
    /// `running`, `success`, `partial` (finished with some data missing),
    /// `incomplete` (a whole phase, such as the issue search, failed; the
    /// others finished) or `failed`.
    pub status: String,
    /// Start of the requested period, when supplied.
    pub period_start: Option<Timestamp>,
    /// End of the requested period, when supplied.
    pub period_end: Option<Timestamp>,
    /// JSON object with API statistics (request count, retries, ...).
    pub stats_json: Option<String>,
    /// Human-readable run message, when present.
    pub message: Option<String>,
}

/// Sync state of a stored pull request, used for incremental sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSyncState {
    /// GitHub node ID of the pull request.
    pub pr_id: String,
    /// Latest pull request update time known to GitHub.
    pub updated_at: Option<Timestamp>,
    /// Time internal activity was last synchronized.
    pub synced_at: Option<Timestamp>,
    /// Current completeness of internal pull request data.
    pub coverage: CoverageStatus,
}

/// Size and row counts of the cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Cache path, when file-backed.
    pub path: Option<PathBuf>,
    /// Size of the database file in bytes.
    pub file_size_bytes: u64,
    /// Current SQLite schema version.
    pub schema_version: u32,
    /// Number of repository records.
    pub repositories: u64,
    /// Number of pull request records.
    pub pull_requests: u64,
    /// Number of issue records.
    pub issues: u64,
    /// Number of activity event records.
    pub events: u64,
    /// Number of account records.
    pub accounts: u64,
}

/// The cache database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: Option<PathBuf>,
}

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SQLITE_CACHE_SIZE_KIB: i32 = 32 * 1024;

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
    // A larger page cache reduces repeated reads when the TUI loads reports.
    conn.pragma_update(None, "cache_size", -SQLITE_CACHE_SIZE_KIB)?;
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
            _ => {
                let _ = write!(uri, "%{b:02X}");
            }
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
    restrict_dir_to_user(dir);
    Ok(())
}

/// Like [`create_private_dir`], but also makes an existing `dir` readable
/// only by the user. Only for a directory that ghusage owns, never for one
/// the user chose.
pub fn create_or_restrict_private_dir(dir: &Path) -> Result<()> {
    create_private_dir(dir)?;
    restrict_dir_to_user(dir);
    Ok(())
}

/// Best effort: on Unix, makes the directory `dir` readable only by the
/// user (`0700`). A symbolic link at `dir` is never followed: the directory
/// is opened without following one and its permissions are changed through
/// that handle, so replacing `dir` with a link cannot redirect the change.
fn restrict_dir_to_user(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let Ok(handle) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(dir)
        else {
            return;
        };
        if let Ok(meta) = handle.metadata()
            && meta.is_dir()
            && meta.permissions().mode() & 0o777 != 0o700
            && meta.permissions().mode() & 0o200 != 0
        {
            drop(handle.set_permissions(std::fs::Permissions::from_mode(0o700)));
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// On Unix, returns the permission bits of `dir` when other users (its
/// group or everyone) can access it. `None` elsewhere or when it is private.
#[must_use]
pub fn shared_dir_mode(dir: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir).ok()?.permissions().mode() & 0o777;
        (mode & 0o077 != 0).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
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
            drop(std::fs::set_permissions(
                path,
                std::fs::Permissions::from_mode(mode),
            ));
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
const ASSOC_PR: &str = "INSERT INTO account_pull_requests (account_id, pr_id, viewed) \
                        VALUES (?1, ?2, 0) ON CONFLICT(account_id, pr_id) DO UPDATE SET viewed = 0";
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
    fn sqlite_integer_conversion_rejects_values_outside_i64() {
        assert_eq!(to_i64(i64::MAX.cast_unsigned()).unwrap(), i64::MAX);
        assert!(to_i64(u64::MAX).is_err());
    }

    #[test]
    fn pull_request_events_are_found_through_the_parent_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let plan: Vec<String> = query_list(
            &store.conn,
            &format!("EXPLAIN QUERY PLAN {}", pr_events_sql(1)),
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
