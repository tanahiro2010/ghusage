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
            StorageError::Sqlite(e) => matches!(
                e.sqlite_error_code(),
                Some(ErrorCode::ReadOnly | ErrorCode::CannotOpen | ErrorCode::PermissionDenied)
            ),
            StorageError::Io(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
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
                uri.push(b as char)
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

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it to
    /// the current schema. Detects corruption and newer schemas.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path).map_err(classify)
    }

    fn open_inner(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            create_private_dir(parent)?;
        }
        check_header(path)?;
        let mut conn = Connection::open(path)?;
        // The cache holds private repository names and titles. SQLite gives
        // its journal files the same permissions as the database.
        restrict_to_user(path, 0o600);
        configure(&conn)?;
        schema::migrate(&mut conn, schema::MIGRATIONS, Some(path))?;
        probe(&conn)?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
        })
    }

    /// Opens an existing database for reading only, for when the file or
    /// its directory is not writable. Every write through the returned
    /// store fails. The schema must already be current.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        Self::open_read_only_inner(path).map_err(classify)
    }

    fn open_read_only_inner(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} does not exist", path.display()),
            )));
        }
        check_header(path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match Connection::open_with_flags(path, flags) {
            Ok(c) if Self::readable(&c) => c,
            _ => {
                // A WAL database cannot be read without creating its shared
                // memory file. When no write-ahead log is left over, the main
                // file holds every committed change and can be read as
                // immutable.
                let wal = wal_path(path);
                if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
                    return Err(StorageError::Data(format!(
                        "{} has unsaved changes in {} and cannot be read without write access",
                        path.display(),
                        wal.display()
                    )));
                }
                Connection::open_with_flags(
                    immutable_uri(path)?,
                    flags | OpenFlags::SQLITE_OPEN_URI,
                )?
            }
        };
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let version = schema::user_version(&conn)?;
        if version > schema::CURRENT_VERSION {
            return Err(StorageError::TooNew {
                found: version,
                supported: schema::CURRENT_VERSION,
            });
        }
        if version < schema::CURRENT_VERSION {
            return Err(StorageError::Migration {
                from: version,
                to: schema::CURRENT_VERSION,
                message: "the cache needs upgrading but is not writable".into(),
            });
        }
        probe(&conn)?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
        })
    }

    fn readable(conn: &Connection) -> bool {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_ok()
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        schema::migrate(&mut conn, schema::MIGRATIONS, None)?;
        Ok(Self { conn, path: None })
    }

    /// Highest schema version this build can read and write.
    pub fn supported_schema_version() -> u32 {
        schema::CURRENT_VERSION
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.conn)
            .map_err(StorageError::from)
            .map_err(classify)
    }

    /// Runs SQLite integrity and foreign-key checks; returns problems found.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        self.integrity_check_inner().map_err(classify)
    }

    fn integrity_check_inner(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let mut stmt = self.conn.prepare("PRAGMA quick_check")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let msg = row?;
            if msg != "ok" {
                problems.push(msg);
            }
        }
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (table, rowid, parent) = row?;
            let at = rowid.map(|r| format!(" (row {r})")).unwrap_or_default();
            problems.push(format!(
                "foreign key violation in {table}{at}: missing row in {parent}"
            ));
        }
        Ok(problems)
    }

    /// Applies a batch in a single transaction (all or nothing).
    pub fn apply(&mut self, batch: &WriteBatch) -> Result<()> {
        let tx = self.conn.transaction().map_err(|e| classify(e.into()))?;
        apply_batch(&tx, batch).map_err(classify)?;
        tx.commit().map_err(|e| classify(e.into()))
    }

    /// Loads the dataset for `subject`. With `range`, only events whose
    /// `occurred_at` or commit author date falls in the range are loaded;
    /// all repositories, pull requests, issues, threads and coverage records
    /// for the account are always loaded.
    pub fn load_dataset(
        &self,
        subject: &Subject,
        subject_is_viewer: bool,
        range: Option<Period>,
    ) -> Result<Dataset> {
        self.load_dataset_inner(subject, subject_is_viewer, range)
            .map_err(classify)
    }

    fn load_dataset_inner(
        &self,
        subject: &Subject,
        subject_is_viewer: bool,
        range: Option<Period>,
    ) -> Result<Dataset> {
        let Some(account_id) = account_id(&self.conn, &subject.login)? else {
            return Ok(Dataset::new(
                subject.clone(),
                subject_is_viewer,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ));
        };
        let repositories = query_list(
            &self.conn,
            &format!(
                "SELECT {REPO_COLS} FROM repositories r \
                 JOIN account_repositories ar ON ar.repository_id = r.id \
                 WHERE ar.account_id = ?1"
            ),
            params![account_id],
            repo_from_row,
        )?;
        let pull_requests = query_list(
            &self.conn,
            &format!(
                "SELECT {PR_COLS} FROM pull_requests p \
                 JOIN account_pull_requests ap ON ap.pr_id = p.id \
                 WHERE ap.account_id = ?1"
            ),
            params![account_id],
            pr_from_row,
        )?;
        let issues = query_list(
            &self.conn,
            &format!(
                "SELECT {ISSUE_COLS} FROM issues i \
                 JOIN account_issues ai ON ai.issue_id = i.id \
                 WHERE ai.account_id = ?1"
            ),
            params![account_id],
            issue_from_row,
        )?;
        let threads = query_list(
            &self.conn,
            &format!(
                "SELECT {THREAD_COLS} FROM review_threads t \
                 JOIN account_pull_requests ap ON ap.pr_id = t.pr_id \
                 WHERE ap.account_id = ?1"
            ),
            params![account_id],
            thread_from_row,
        )?;
        let events = match range {
            None => query_events(
                &self.conn,
                &format!(
                    "SELECT {EVENT_COLS} FROM activity_events e \
                     JOIN account_events ae ON ae.event_id = e.event_id \
                     WHERE ae.account_id = ?1"
                ),
                params![account_id],
            )?,
            Some(period) => {
                // Coarse filter on whole seconds (uses the indexes), then an
                // exact half-open check including sub-second precision.
                let lo = secs(period.start);
                let hi = secs(period.end);
                let mut events = query_events(
                    &self.conn,
                    &format!(
                        "SELECT {EVENT_COLS} FROM activity_events e \
                         JOIN account_events ae ON ae.event_id = e.event_id \
                         WHERE ae.account_id = ?1 \
                         AND ((e.occurred_at BETWEEN ?2 AND ?3) \
                              OR (e.author_date BETWEEN ?2 AND ?3))"
                    ),
                    params![account_id, lo, hi],
                )?;
                events.retain(|e| {
                    period.contains(e.occurred_at)
                        || e.meta.author_date.is_some_and(|d| period.contains(d))
                });
                // The subject's pull requests active in the period come with
                // all their events, so lifetime timings (computed only for the
                // subject's own pull requests) see what happened before the
                // period. Counting always applies the period itself.
                let mut seen: HashSet<String> =
                    events.iter().map(|e| e.event_id.0.clone()).collect();
                let own: HashSet<&str> = pull_requests
                    .iter()
                    .filter(|pr| pr.author.is(subject))
                    .map(|pr| pr.id.as_str())
                    .collect();
                let active: HashSet<String> = events
                    .iter()
                    .filter_map(|e| e.parent_pr_id.as_deref())
                    .filter(|id| own.contains(id))
                    .map(str::to_owned)
                    .collect();
                let mut stmt = self.conn.prepare_cached(&pr_events_sql())?;
                let cols = EventCols::of(&stmt)?;
                let mut active: Vec<String> = active.into_iter().collect();
                active.sort();
                for pr in active {
                    let mut rows = stmt.query(params![account_id, pr])?;
                    while let Some(row) = rows.next()? {
                        let e = cols.event(row)?;
                        if seen.insert(e.event_id.0.clone()) {
                            events.push(e);
                        }
                    }
                }
                events
            }
        };
        let coverage = coverage_for(&self.conn, account_id)?;
        let (commit_stats, pr_diffs) = diff_stats_for(&self.conn, account_id)?;
        let merged_by_merge_commit = merged_by_merge_commit_where(
            &self.conn,
            "p.id IN (SELECT pr_id FROM account_pull_requests WHERE account_id = ?1)",
            params![account_id],
        )?;
        Ok(Dataset::new(
            subject.clone(),
            subject_is_viewer,
            repositories,
            pull_requests,
            issues,
            threads,
            events,
            coverage,
        )
        .with_diff_stats(commit_stats, pr_diffs)
        .with_line_count_rule(&merged_by_merge_commit))
    }

    /// Loads every event of one pull request (any time) plus entities.
    pub fn load_pr_dataset(
        &self,
        subject: &Subject,
        subject_is_viewer: bool,
        pr_id: &str,
    ) -> Result<Dataset> {
        self.load_pr_dataset_inner(subject, subject_is_viewer, pr_id)
            .map_err(classify)
    }

    fn load_pr_dataset_inner(
        &self,
        subject: &Subject,
        subject_is_viewer: bool,
        pr_id: &str,
    ) -> Result<Dataset> {
        let pull_requests = query_list(
            &self.conn,
            &format!("SELECT {PR_COLS} FROM pull_requests p WHERE p.id = ?1"),
            params![pr_id],
            pr_from_row,
        )?;
        let events = query_events(
            &self.conn,
            &format!("SELECT {EVENT_COLS} FROM activity_events e WHERE e.parent_pr_id = ?1"),
            params![pr_id],
        )?;
        let threads = query_list(
            &self.conn,
            &format!("SELECT {THREAD_COLS} FROM review_threads t WHERE t.pr_id = ?1"),
            params![pr_id],
            thread_from_row,
        )?;
        let repositories = query_list(
            &self.conn,
            &format!(
                "SELECT {REPO_COLS} FROM repositories r WHERE r.id IN ( \
                 SELECT repository_id FROM pull_requests WHERE id = ?1 \
                 UNION SELECT repository_id FROM activity_events WHERE parent_pr_id = ?1)"
            ),
            params![pr_id],
            repo_from_row,
        )?;
        let coverage = match account_id(&self.conn, &subject.login)? {
            Some(id) => coverage_for(&self.conn, id)?,
            None => Vec::new(),
        };
        let commit_stats = commit_stats_where(
            &self.conn,
            "(c.repository_id, c.sha) IN \
             (SELECT repository_id, sha FROM pull_request_commits WHERE pr_id = ?1)",
            params![pr_id],
        )?;
        let pr_diffs = pr_diffs_where(&self.conn, "p.id = ?1", params![pr_id])?;
        let merged_by_merge_commit =
            merged_by_merge_commit_where(&self.conn, "p.id = ?1", params![pr_id])?;
        Ok(Dataset::new(
            subject.clone(),
            subject_is_viewer,
            repositories,
            pull_requests,
            Vec::new(),
            threads,
            events,
            coverage,
        )
        .with_diff_stats(commit_stats, pr_diffs)
        .with_line_count_rule(&merged_by_merge_commit))
    }

    /// Finds a stored pull request by repository full name and number.
    /// Owner and name are matched case-insensitively.
    pub fn find_pr(&self, owner: &str, name: &str, number: u64) -> Result<Option<String>> {
        let number = to_i64(number)?;
        self.conn
            .query_row(
                "SELECT p.id FROM pull_requests p \
                 JOIN repositories r ON r.id = p.repository_id \
                 WHERE r.owner = ?1 COLLATE NOCASE AND r.name = ?2 COLLATE NOCASE \
                 AND p.number = ?3 \
                 ORDER BY p.synced_at DESC LIMIT 1",
                params![owner, name, number],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| classify(e.into()))
    }

    /// A stored repository matched case-insensitively by owner and name.
    pub fn repository_by_name(&self, owner: &str, name: &str) -> Result<Option<Repository>> {
        (|| {
            let mut stmt = self.conn.prepare(
                "SELECT * FROM repositories \
                 WHERE owner = ?1 COLLATE NOCASE AND name = ?2 COLLATE NOCASE LIMIT 1",
            )?;
            let mut rows = stmt.query(params![owner, name])?;
            match rows.next()? {
                Some(r) => Ok(Some(repo_from_row(r)?)),
                None => Ok(None),
            }
        })()
        .map_err(classify)
    }

    /// Stores (or replaces) a fetched commit detail.
    pub fn put_commit_detail(&mut self, d: &ghusage_core::commit::CommitDetail) -> Result<()> {
        (|| {
            self.conn.execute(
                "INSERT INTO commit_details (repository, sha, fetched_at, fetched_at_ns, detail) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT (repository, sha) DO UPDATE SET \
                 fetched_at = excluded.fetched_at, fetched_at_ns = excluded.fetched_at_ns, \
                 detail = excluded.detail",
                params![
                    d.repository,
                    d.sha.to_ascii_lowercase(),
                    secs(d.fetched_at),
                    nanos(d.fetched_at),
                    to_json(d)?
                ],
            )?;
            // The totals also describe the commit for reports, when the
            // repository is known.
            self.conn.execute(
                "UPDATE commits SET additions = ?1, deletions = ?2, stats_observed_at = ?3, \
                 stats_observed_at_ns = ?4 WHERE sha = ?5 AND repository_id IN \
                 (SELECT id FROM repositories WHERE owner || '/' || name = ?6 COLLATE NOCASE)",
                params![
                    d.stats.additions as i64,
                    d.stats.deletions as i64,
                    secs(d.fetched_at),
                    nanos(d.fetched_at),
                    d.sha.to_ascii_lowercase(),
                    d.repository
                ],
            )?;
            Ok(())
        })()
        .map_err(classify)
    }

    /// A stored commit detail of `repository` (`owner/name`, any case) by
    /// full or abbreviated SHA. An abbreviation matching more than one
    /// stored commit finds nothing.
    pub fn commit_detail(
        &self,
        repository: &str,
        sha: &str,
    ) -> Result<Option<ghusage_core::commit::CommitDetail>> {
        (|| {
            let sha = sha.to_ascii_lowercase();
            let mut stmt = self.conn.prepare(
                "SELECT detail FROM commit_details \
                 WHERE repository = ?1 AND substr(sha, 1, length(?2)) = ?2 LIMIT 2",
            )?;
            let found: Vec<String> = stmt
                .query_map(params![repository, sha], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            match found.as_slice() {
                [one] => Ok(Some(from_json("commit detail", one)?)),
                _ => Ok(None),
            }
        })()
        .map_err(classify)
    }

    /// Marks a stored repository (matched case-insensitively by owner and
    /// name) as accessible or not, keeping all of its data. Returns whether
    /// a repository was found.
    pub fn set_repository_accessible(
        &mut self,
        owner: &str,
        name: &str,
        accessible: bool,
    ) -> Result<bool> {
        self.conn
            .execute(
                "UPDATE repositories SET accessible = ?3 \
                 WHERE owner = ?1 COLLATE NOCASE AND name = ?2 COLLATE NOCASE",
                params![owner, name, accessible],
            )
            .map(|n| n > 0)
            .map_err(|e| classify(e.into()))
    }

    pub fn coverage(&self, account: &str) -> Result<Vec<CoverageRecord>> {
        (|| match account_id(&self.conn, account)? {
            Some(id) => coverage_for(&self.conn, id),
            None => Ok(Vec::new()),
        })()
        .map_err(classify)
    }

    pub fn pr_sync_states(&self) -> Result<Vec<PrSyncState>> {
        query_list(
            &self.conn,
            "SELECT id, updated_at, updated_at_ns, synced_at, synced_at_ns, coverage \
             FROM pull_requests ORDER BY id",
            [],
            |r| {
                Ok(PrSyncState {
                    pr_id: r.get("id")?,
                    updated_at: opt_time(r.get("updated_at")?, r.get("updated_at_ns")?)?,
                    synced_at: opt_time(r.get("synced_at")?, r.get("synced_at_ns")?)?,
                    coverage: CoverageStatus::parse(&r.get::<_, String>("coverage")?),
                })
            },
        )
        .map_err(classify)
    }

    pub fn cursor(&self, account: &str, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT c.value FROM sync_cursors c JOIN accounts a ON a.id = c.account_id \
                 WHERE a.login = ?1 AND c.key = ?2",
                params![account, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| classify(e.into()))
    }

    pub fn begin_sync_run(
        &mut self,
        account: &str,
        period: Option<Period>,
        started_at: Timestamp,
    ) -> Result<i64> {
        (|| {
            let tx = self.conn.transaction()?;
            let account_id = ensure_account(&tx, account)?;
            tx.execute(
                "INSERT INTO sync_runs (account_id, started_at, started_at_ns, status, \
                 period_start, period_start_ns, period_end, period_end_ns) \
                 VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7)",
                params![
                    account_id,
                    secs(started_at),
                    nanos(started_at),
                    opt_secs(period.map(|p| p.start)),
                    opt_nanos(period.map(|p| p.start)),
                    opt_secs(period.map(|p| p.end)),
                    opt_nanos(period.map(|p| p.end)),
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(id)
        })()
        .map_err(classify)
    }

    pub fn finish_sync_run(
        &mut self,
        id: i64,
        status: &str,
        finished_at: Timestamp,
        stats_json: Option<&str>,
        message: Option<&str>,
    ) -> Result<()> {
        let n = self
            .conn
            .execute(
                "UPDATE sync_runs SET status = ?2, finished_at = ?3, finished_at_ns = ?4, \
                 stats_json = ?5, message = ?6 WHERE id = ?1",
                params![
                    id,
                    status,
                    secs(finished_at),
                    nanos(finished_at),
                    stats_json,
                    message
                ],
            )
            .map_err(|e| classify(e.into()))?;
        if n == 0 {
            return Err(StorageError::Data(format!("unknown sync run {id}")));
        }
        Ok(())
    }

    pub fn last_sync_run(&self, account: &str) -> Result<Option<SyncRun>> {
        self.conn
            .query_row(
                "SELECT s.*, a.login FROM sync_runs s JOIN accounts a ON a.id = s.account_id \
                 WHERE a.login = ?1 ORDER BY s.started_at DESC, s.started_at_ns DESC, s.id DESC \
                 LIMIT 1",
                params![account],
                |r| Ok(sync_run_from_row(r)),
            )
            .optional()
            .map_err(|e| classify(e.into()))?
            .transpose()
            .map_err(classify)
    }

    pub fn stats(&self) -> Result<CacheStats> {
        (|| {
            let count = |sql: &str| -> Result<u64> {
                let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
                Ok(n.max(0) as u64)
            };
            let mut file_size_bytes = 0;
            if let Some(path) = &self.path {
                file_size_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let mut wal = path.as_os_str().to_os_string();
                wal.push("-wal");
                file_size_bytes += std::fs::metadata(PathBuf::from(wal))
                    .map(|m| m.len())
                    .unwrap_or(0);
            }
            Ok(CacheStats {
                path: self.path.clone(),
                file_size_bytes,
                schema_version: schema::user_version(&self.conn)?,
                repositories: count("SELECT count(*) FROM repositories")?,
                pull_requests: count("SELECT count(*) FROM pull_requests")?,
                issues: count("SELECT count(*) FROM issues")?,
                events: count("SELECT count(*) FROM activity_events")?,
                accounts: count("SELECT count(*) FROM accounts")?,
            })
        })()
        .map_err(classify)
    }

    /// Deletes all cached GitHub data. Never touches credentials (which are
    /// not stored in the database).
    pub fn clear(&mut self) -> Result<()> {
        (|| {
            let tx = self.conn.transaction()?;
            for t in schema::DATA_TABLES {
                // Table names are compile-time constants.
                tx.execute(&format!("DELETE FROM {t}"), [])?;
            }
            tx.execute("DELETE FROM sqlite_sequence", [])?;
            tx.commit()?;
            if let Err(e) = self.conn.execute_batch("VACUUM") {
                tracing::warn!(error = %e, "could not compact the cache after clearing");
            }
            Ok(())
        })()
        .map_err(classify)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM cache_metadata WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| classify(e.into()))
    }

    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO cache_metadata (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(|e| classify(e.into()))
    }
}

// ---------------------------------------------------------------------------
// Writing

fn to_i64(n: u64) -> Result<i64> {
    i64::try_from(n).map_err(|_| StorageError::Data(format!("number {n} is out of range")))
}

fn account_id(conn: &Connection, login: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM accounts WHERE login = ?1",
            params![login],
            |r| r.get(0),
        )
        .optional()?)
}

fn ensure_account(tx: &Transaction<'_>, login: &str) -> Result<i64> {
    if login.trim().is_empty() {
        return Err(StorageError::Data("account login must not be empty".into()));
    }
    tx.execute(
        "INSERT INTO accounts (login, created_at) VALUES (?1, ?2) \
         ON CONFLICT(login) DO NOTHING",
        params![login, Timestamp::now().as_second()],
    )?;
    Ok(account_id(tx, login)?.expect("account row was just ensured"))
}

fn row_exists(tx: &Transaction<'_>, sql: &str, id: &str) -> Result<bool> {
    Ok(tx
        .query_row(sql, params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn require_repository(tx: &Transaction<'_>, id: &str, referrer: &str) -> Result<()> {
    if row_exists(tx, "SELECT 1 FROM repositories WHERE id = ?1", id)? {
        Ok(())
    } else {
        Err(StorageError::Data(format!(
            "{referrer} references repository {id:?}, which is neither in the batch nor stored"
        )))
    }
}

fn require_pull_request(tx: &Transaction<'_>, id: &str, referrer: &str) -> Result<()> {
    if row_exists(tx, "SELECT 1 FROM pull_requests WHERE id = ?1", id)? {
        Ok(())
    } else {
        Err(StorageError::Data(format!(
            "{referrer} references pull request {id:?}, which is neither in the batch nor stored"
        )))
    }
}

fn associate(tx: &Transaction<'_>, sql: &str, account_id: i64, id: &str) -> Result<()> {
    tx.execute(sql, params![account_id, id])?;
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

fn apply_batch(tx: &Transaction<'_>, b: &WriteBatch) -> Result<()> {
    let account_id = ensure_account(tx, &b.account)?;

    for r in &b.repositories {
        upsert_repository(tx, r)?;
        associate(tx, ASSOC_REPO, account_id, &r.id)?;
    }
    for p in &b.pull_requests {
        require_repository(tx, &p.repository_id, &format!("pull request {:?}", p.id))?;
        upsert_pull_request(tx, p)?;
        associate(tx, ASSOC_PR, account_id, &p.id)?;
        associate(tx, ASSOC_REPO, account_id, &p.repository_id)?;
    }
    for i in &b.issues {
        require_repository(tx, &i.repository_id, &format!("issue {:?}", i.id))?;
        upsert_issue(tx, i)?;
        associate(tx, ASSOC_ISSUE, account_id, &i.id)?;
        associate(tx, ASSOC_REPO, account_id, &i.repository_id)?;
    }
    for t in &b.threads {
        require_pull_request(tx, &t.pr_id, &format!("review thread {:?}", t.id))?;
        upsert_thread(tx, t)?;
    }
    for e in &b.events {
        let referrer = format!("event {:?}", e.event_id.as_str());
        require_repository(tx, &e.repository_id, &referrer)?;
        if let Some(pr) = &e.parent_pr_id {
            require_pull_request(tx, pr, &referrer)?;
        }
        upsert_event(tx, e)?;
        project_event(tx, e)?;
        associate(tx, ASSOC_EVENT, account_id, e.event_id.as_str())?;
        associate(tx, ASSOC_REPO, account_id, &e.repository_id)?;
        if let Some(pr) = &e.parent_pr_id {
            associate(tx, ASSOC_PR, account_id, pr)?;
        }
    }

    // Commit links sent in this batch, per pull request.
    let mut current_links: HashMap<&str, HashSet<String>> = HashMap::new();
    for l in &b.pr_commits {
        let referrer = format!("commit link {}:{}", l.pr_id, l.sha);
        require_pull_request(tx, &l.pr_id, &referrer)?;
        require_repository(tx, &l.repository_id, &referrer)?;
        upsert_pr_commit(tx, l)?;
        if l.is_current {
            current_links
                .entry(l.pr_id.as_str())
                .or_default()
                .insert(l.sha.to_ascii_lowercase());
        }
    }

    for c in &b.commit_stats {
        require_repository(
            tx,
            &c.repository_id,
            &format!("commit statistics {}:{}", c.repository_id, c.sha),
        )?;
        tx.execute(
            "INSERT INTO commits (repository_id, sha, additions, deletions, stats_observed_at, \
             stats_observed_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(repository_id, sha) DO UPDATE SET \
             additions = excluded.additions, deletions = excluded.deletions, \
             stats_observed_at = excluded.stats_observed_at, \
             stats_observed_at_ns = excluded.stats_observed_at_ns",
            params![
                c.repository_id,
                c.sha.to_ascii_lowercase(),
                c.counts.additions as i64,
                c.counts.deletions as i64,
                secs(c.observed_at),
                nanos(c.observed_at)
            ],
        )?;
    }
    for (pr_id, parents) in &b.pr_merge_commit_parents {
        require_pull_request(tx, pr_id, &format!("pull request merge {pr_id:?}"))?;
        tx.execute(
            "UPDATE pull_requests SET merge_commit_parents = ?2 WHERE id = ?1",
            params![pr_id, *parents as i64],
        )?;
    }
    for (pr_id, d) in &b.pr_diffs {
        require_pull_request(tx, pr_id, &format!("pull request diff {pr_id:?}"))?;
        // An older observation never replaces a newer one.
        tx.execute(
            "UPDATE pull_requests SET additions = ?2, deletions = ?3, changed_files = ?4, \
             diff_observed_at = ?5, diff_observed_at_ns = ?6 \
             WHERE id = ?1 AND (diff_observed_at IS NULL \
             OR (diff_observed_at, diff_observed_at_ns) <= (?5, ?6))",
            params![
                pr_id,
                d.additions as i64,
                d.deletions as i64,
                d.changed_files as i64,
                secs(d.observed_at),
                nanos(d.observed_at)
            ],
        )?;
    }

    let mut removal_time: HashMap<&str, Timestamp> = HashMap::new();
    for s in &b.pr_snapshots {
        apply_snapshot(tx, s, current_links.get(s.pr_id.as_str()))?;
        removal_time.insert(s.pr_id.as_str(), s.observed_at);
    }

    // Commits that are no longer part of their pull request keep their link
    // (history) but their commit event is marked removed.
    let now = Timestamp::now();
    let mut stmt = tx.prepare(
        "SELECT pr_id, sha FROM pull_request_commits WHERE is_current = 0 AND pr_id = ?1",
    )?;
    let mut prs: Vec<&str> = b
        .pr_commits
        .iter()
        .map(|l| l.pr_id.as_str())
        .chain(b.pr_snapshots.iter().map(|s| s.pr_id.as_str()))
        .collect();
    prs.sort_unstable();
    prs.dedup();
    for pr in prs {
        let at = removal_time.get(pr).copied().unwrap_or(now);
        let stale: Vec<(String, String)> = stmt
            .query_map(params![pr], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (pr_id, sha) in stale {
            mark_removed(tx, EventId::pr_commit(&pr_id, &sha).as_str(), at)?;
        }
    }

    for c in &b.coverage {
        insert_coverage(tx, account_id, c)?;
    }
    for (key, value) in &b.cursors {
        tx.execute(
            "INSERT INTO sync_cursors (account_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(account_id, key) DO UPDATE SET value = excluded.value",
            params![account_id, key, value],
        )?;
    }
    Ok(())
}

fn upsert_repository(tx: &Transaction<'_>, r: &Repository) -> Result<()> {
    tx.execute(
        "INSERT INTO repositories (id, owner, name, owner_kind, visibility, is_fork, \
         viewer_permission, accessible, url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, name = excluded.name, \
         owner_kind = excluded.owner_kind, visibility = excluded.visibility, \
         is_fork = excluded.is_fork, viewer_permission = excluded.viewer_permission, \
         accessible = excluded.accessible, url = excluded.url",
        params![
            r.id,
            r.owner,
            r.name,
            actor_kind_str(r.owner_kind)?,
            r.visibility.as_str(),
            r.is_fork,
            r.viewer_permission,
            r.accessible,
            r.url
        ],
    )?;
    Ok(())
}

fn upsert_pull_request(tx: &Transaction<'_>, p: &PullRequest) -> Result<()> {
    tx.execute(
        "INSERT INTO pull_requests (id, repository_id, number, title, url, author_id, \
         author_login, author_kind, state, is_draft, created_at, created_at_ns, updated_at, \
         updated_at_ns, closed_at, closed_at_ns, merged_at, merged_at_ns, merged_by, head_sha, \
         latest_ci, coverage, synced_at, synced_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24) \
         ON CONFLICT(id) DO UPDATE SET repository_id = excluded.repository_id, \
         number = excluded.number, title = excluded.title, url = excluded.url, \
         author_id = excluded.author_id, author_login = excluded.author_login, \
         author_kind = excluded.author_kind, state = excluded.state, \
         is_draft = excluded.is_draft, created_at = excluded.created_at, \
         created_at_ns = excluded.created_at_ns, updated_at = excluded.updated_at, \
         updated_at_ns = excluded.updated_at_ns, closed_at = excluded.closed_at, \
         closed_at_ns = excluded.closed_at_ns, merged_at = excluded.merged_at, \
         merged_at_ns = excluded.merged_at_ns, merged_by = excluded.merged_by, \
         head_sha = excluded.head_sha, latest_ci = excluded.latest_ci, \
         coverage = excluded.coverage, synced_at = excluded.synced_at, \
         synced_at_ns = excluded.synced_at_ns",
        params![
            p.id,
            p.repository_id,
            to_i64(p.number)?,
            p.title,
            p.url,
            p.author.id,
            p.author.login,
            actor_kind_str(p.author.kind)?,
            p.state.as_str(),
            p.is_draft,
            secs(p.created_at),
            nanos(p.created_at),
            opt_secs(p.updated_at),
            opt_nanos(p.updated_at),
            opt_secs(p.closed_at),
            opt_nanos(p.closed_at),
            opt_secs(p.merged_at),
            opt_nanos(p.merged_at),
            opt_to_json(p.merged_by.as_ref())?,
            p.head_sha,
            p.latest_ci.map(|c| c.as_str()),
            p.coverage.as_str(),
            opt_secs(p.synced_at),
            opt_nanos(p.synced_at),
        ],
    )?;
    Ok(())
}

fn upsert_issue(tx: &Transaction<'_>, i: &Issue) -> Result<()> {
    tx.execute(
        "INSERT INTO issues (id, repository_id, number, title, url, author_id, author_login, \
         author_kind, is_open, created_at, created_at_ns, closed_at, closed_at_ns) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
         ON CONFLICT(id) DO UPDATE SET repository_id = excluded.repository_id, \
         number = excluded.number, title = excluded.title, url = excluded.url, \
         author_id = excluded.author_id, author_login = excluded.author_login, \
         author_kind = excluded.author_kind, is_open = excluded.is_open, \
         created_at = excluded.created_at, created_at_ns = excluded.created_at_ns, \
         closed_at = excluded.closed_at, closed_at_ns = excluded.closed_at_ns",
        params![
            i.id,
            i.repository_id,
            to_i64(i.number)?,
            i.title,
            i.url,
            i.author.id,
            i.author.login,
            actor_kind_str(i.author.kind)?,
            i.is_open,
            secs(i.created_at),
            nanos(i.created_at),
            opt_secs(i.closed_at),
            opt_nanos(i.closed_at),
        ],
    )?;
    Ok(())
}

fn upsert_thread(tx: &Transaction<'_>, t: &ReviewThread) -> Result<()> {
    tx.execute(
        "INSERT INTO review_threads (id, pr_id, is_resolved, is_outdated, resolved_by, path, \
         comment_count, first_comment_author) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(id) DO UPDATE SET pr_id = excluded.pr_id, \
         is_resolved = excluded.is_resolved, is_outdated = excluded.is_outdated, \
         resolved_by = excluded.resolved_by, path = excluded.path, \
         comment_count = excluded.comment_count, \
         first_comment_author = excluded.first_comment_author",
        params![
            t.id,
            t.pr_id,
            t.is_resolved,
            t.is_outdated,
            opt_to_json(t.resolved_by.as_ref())?,
            t.path,
            t.comment_count,
            opt_to_json(t.first_comment_author.as_ref())?,
        ],
    )?;
    Ok(())
}

/// Upserts an event by id. A removal observation already stored is kept
/// (the earliest one wins); only a complete resync clears it. A commit
/// author date already stored is kept when the new copy has none.
fn upsert_event(tx: &Transaction<'_>, e: &ActivityEvent) -> Result<()> {
    let mut meta = e.meta.clone();
    let author_date = meta.author_date.take();
    let removed = meta.removed_observed_at.take();
    tx.execute(
        "INSERT INTO activity_events (event_id, event_type, actor_id, actor_login, actor_kind, \
         repository_id, entity_id, parent_pr_id, occurred_at, occurred_at_ns, observed_at, \
         observed_at_ns, author_date, author_date_ns, source, url, meta, coverage, \
         removed_observed_at, removed_observed_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20) \
         ON CONFLICT(event_id) DO UPDATE SET event_type = excluded.event_type, \
         actor_id = excluded.actor_id, actor_login = excluded.actor_login, \
         actor_kind = excluded.actor_kind, repository_id = excluded.repository_id, \
         entity_id = excluded.entity_id, \
         parent_pr_id = coalesce(excluded.parent_pr_id, activity_events.parent_pr_id), \
         occurred_at = excluded.occurred_at, occurred_at_ns = excluded.occurred_at_ns, \
         observed_at = excluded.observed_at, observed_at_ns = excluded.observed_at_ns, \
         author_date = CASE WHEN excluded.author_date IS NULL \
             THEN activity_events.author_date ELSE excluded.author_date END, \
         author_date_ns = CASE WHEN excluded.author_date IS NULL \
             THEN activity_events.author_date_ns ELSE excluded.author_date_ns END, \
         source = excluded.source, url = coalesce(excluded.url, activity_events.url), \
         meta = excluded.meta, coverage = excluded.coverage, \
         removed_observed_at = CASE \
             WHEN activity_events.removed_observed_at IS NULL THEN excluded.removed_observed_at \
             WHEN excluded.removed_observed_at IS NULL THEN activity_events.removed_observed_at \
             WHEN (excluded.removed_observed_at, excluded.removed_observed_at_ns) \
                  < (activity_events.removed_observed_at, activity_events.removed_observed_at_ns) \
             THEN excluded.removed_observed_at ELSE activity_events.removed_observed_at END, \
         removed_observed_at_ns = CASE \
             WHEN activity_events.removed_observed_at IS NULL THEN excluded.removed_observed_at_ns \
             WHEN excluded.removed_observed_at IS NULL THEN activity_events.removed_observed_at_ns \
             WHEN (excluded.removed_observed_at, excluded.removed_observed_at_ns) \
                  < (activity_events.removed_observed_at, activity_events.removed_observed_at_ns) \
             THEN excluded.removed_observed_at_ns ELSE activity_events.removed_observed_at_ns END",
        params![
            e.event_id.as_str(),
            e.event_type.as_str(),
            e.actor.id,
            e.actor.login,
            actor_kind_str(e.actor.kind)?,
            e.repository_id,
            e.entity_id,
            e.parent_pr_id,
            secs(e.occurred_at),
            nanos(e.occurred_at),
            secs(e.observed_at),
            nanos(e.observed_at),
            opt_secs(author_date),
            opt_nanos(author_date),
            e.source.as_str(),
            e.url,
            to_json(&meta)?,
            e.coverage.as_str(),
            opt_secs(removed),
            opt_nanos(removed),
        ],
    )?;
    Ok(())
}

/// Maintains the entity projections (commits, reviews, comments, checks).
fn project_event(tx: &Transaction<'_>, e: &ActivityEvent) -> Result<()> {
    use EventType::*;
    let id = e.event_id.as_str();
    match e.event_type {
        CommitAuthored | PrCommit => {
            let sha = e.meta.sha.as_deref().unwrap_or(&e.entity_id);
            let author = (e.event_type == CommitAuthored)
                .then(|| e.actor.login.clone())
                .flatten();
            tx.execute(
                "INSERT INTO commits (repository_id, sha, author_login, authored_at, \
                 committed_at, is_merge_commit) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(repository_id, sha) DO UPDATE SET \
                 author_login = coalesce(excluded.author_login, commits.author_login), \
                 authored_at = coalesce(excluded.authored_at, commits.authored_at), \
                 committed_at = coalesce(excluded.committed_at, commits.committed_at), \
                 is_merge_commit = coalesce(excluded.is_merge_commit, commits.is_merge_commit)",
                params![
                    e.repository_id,
                    sha.to_ascii_lowercase(),
                    author,
                    opt_secs(e.meta.author_date),
                    secs(e.occurred_at),
                    e.meta.is_merge_commit,
                ],
            )?;
        }
        ReviewSubmitted => {
            tx.execute(
                "INSERT INTO pull_request_reviews (event_id, review_id, pr_id, author_login, \
                 state, submitted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(event_id) DO UPDATE SET review_id = excluded.review_id, \
                 pr_id = excluded.pr_id, author_login = excluded.author_login, \
                 state = excluded.state, submitted_at = excluded.submitted_at",
                params![
                    id,
                    e.meta.review_id.as_deref().unwrap_or(&e.entity_id),
                    e.parent_pr_id,
                    e.actor.login,
                    e.meta.review_state.map(|s| s.as_str()),
                    secs(e.occurred_at),
                ],
            )?;
        }
        ConversationComment | InlineComment | ReviewSummary => {
            tx.execute(
                "INSERT INTO pull_request_comments (event_id, comment_id, pr_id, kind, \
                 thread_id, author_login, created_at, edited_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(event_id) DO UPDATE SET comment_id = excluded.comment_id, \
                 pr_id = excluded.pr_id, kind = excluded.kind, thread_id = excluded.thread_id, \
                 author_login = excluded.author_login, created_at = excluded.created_at, \
                 edited_at = excluded.edited_at",
                params![
                    id,
                    e.entity_id,
                    e.parent_pr_id,
                    e.event_type.as_str(),
                    e.meta.thread_id,
                    e.actor.login,
                    secs(e.occurred_at),
                    opt_secs(e.meta.edited_at),
                ],
            )?;
        }
        CheckRun | CommitStatus => {
            tx.execute(
                "INSERT INTO check_runs (event_id, node_id, pr_id, kind, name, outcome, \
                 occurred_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(event_id) DO UPDATE SET node_id = excluded.node_id, \
                 pr_id = excluded.pr_id, kind = excluded.kind, name = excluded.name, \
                 outcome = excluded.outcome, occurred_at = excluded.occurred_at",
                params![
                    id,
                    e.entity_id,
                    e.parent_pr_id,
                    e.event_type.as_str(),
                    e.meta.check_name,
                    e.meta.check_outcome.map(CheckOutcome::as_str),
                    secs(e.occurred_at),
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn upsert_pr_commit(tx: &Transaction<'_>, l: &PrCommitLink) -> Result<()> {
    let sha = l.sha.to_ascii_lowercase();
    tx.execute(
        "INSERT INTO commits (repository_id, sha) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        params![l.repository_id, sha],
    )?;
    tx.execute(
        "INSERT INTO pull_request_commits (pr_id, repository_id, sha, is_current) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(pr_id, sha) DO UPDATE SET \
         repository_id = excluded.repository_id, is_current = excluded.is_current",
        params![l.pr_id, l.repository_id, sha, l.is_current],
    )?;
    Ok(())
}

/// Sets the removal observation of an event unless one is already stored.
fn mark_removed(tx: &Transaction<'_>, event_id: &str, at: Timestamp) -> Result<()> {
    tx.execute(
        "UPDATE activity_events SET removed_observed_at = ?2, removed_observed_at_ns = ?3 \
         WHERE event_id = ?1 AND removed_observed_at IS NULL",
        params![event_id, secs(at), nanos(at)],
    )?;
    Ok(())
}

fn apply_snapshot(
    tx: &Transaction<'_>,
    s: &PrSnapshot,
    current_links: Option<&HashSet<String>>,
) -> Result<()> {
    if s.complete_collections.is_empty() {
        return Ok(());
    }
    let complete: HashSet<Collection> = s.complete_collections.iter().copied().collect();
    // Checks are only fetched for the current head commit, so a complete
    // checks fetch says nothing about the checks of earlier heads.
    let head_sha: Option<String> = if complete.contains(&Collection::Checks) {
        tx.query_row(
            "SELECT head_sha FROM pull_requests WHERE id = ?1",
            params![s.pr_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .map(|h| h.to_ascii_lowercase())
    } else {
        None
    };
    let stored: Vec<(String, String, bool, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT event_id, event_type, removed_observed_at IS NOT NULL, \
             json_extract(meta, '$.sha') FROM activity_events WHERE parent_pr_id = ?1",
        )?;
        stmt.query_map(params![s.pr_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    for (id, ty, removed, sha) in stored {
        let ty = EventType::parse(&ty)
            .ok_or_else(|| StorageError::Data(format!("unknown stored event type {ty:?}")))?;
        let Some(coll) = Collection::of(ty) else {
            continue;
        };
        if !complete.contains(&coll) {
            continue;
        }
        let present = s.present.contains(&EventId(id.clone()));
        if present && removed {
            tx.execute(
                "UPDATE activity_events SET removed_observed_at = NULL, \
                 removed_observed_at_ns = NULL WHERE event_id = ?1",
                params![id],
            )?;
        } else if !present && !removed {
            let of_fetched_head = coll != Collection::Checks
                || matches!(
                    (&head_sha, &sha),
                    (Some(head), Some(sha)) if sha.eq_ignore_ascii_case(head)
                );
            if of_fetched_head {
                mark_removed(tx, &id, s.observed_at)?;
            }
        }
    }

    if complete.contains(&Collection::ReviewComments) {
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM review_threads WHERE pr_id = ?1")?;
            stmt.query_map(params![s.pr_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in ids {
            if !s.present_threads.contains(&id) {
                tx.execute("DELETE FROM review_threads WHERE id = ?1", params![id])?;
            }
        }
    }

    if complete.contains(&Collection::Commits) {
        let empty = HashSet::new();
        let current = current_links.unwrap_or(&empty);
        let shas: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT sha FROM pull_request_commits WHERE pr_id = ?1 AND is_current = 1",
            )?;
            stmt.query_map(params![s.pr_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for sha in shas {
            if !current.contains(&sha) {
                tx.execute(
                    "UPDATE pull_request_commits SET is_current = 0 \
                     WHERE pr_id = ?1 AND sha = ?2",
                    params![s.pr_id, sha],
                )?;
            }
        }
    }
    Ok(())
}

fn insert_coverage(tx: &Transaction<'_>, account_id: i64, c: &CoverageRecord) -> Result<()> {
    tx.execute(
        "INSERT INTO coverage_intervals (account_id, data_kind, start_at, start_at_ns, end_at, \
         end_at_ns, status, scope, reason, recorded_at, recorded_at_ns) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ON CONFLICT DO NOTHING",
        params![
            account_id,
            c.kind.as_str(),
            secs(c.start),
            nanos(c.start),
            secs(c.end),
            nanos(c.end),
            c.status.as_str(),
            c.scope,
            c.reason,
            secs(c.recorded_at),
            nanos(c.recorded_at),
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading

const REPO_COLS: &str = "r.id, r.owner, r.name, r.owner_kind, r.visibility, r.is_fork, \
                         r.viewer_permission, r.accessible, r.url";
const PR_COLS: &str = "p.*";
const ISSUE_COLS: &str = "i.*";
const THREAD_COLS: &str = "t.*";
const EVENT_COLS: &str = "e.*";

fn query_list<T, P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    f: impl Fn(&Row<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(f(row)?);
    }
    Ok(out)
}

/// Events of one pull request seen by an account. `CROSS JOIN` keeps
/// SQLite from driving the join from `account_events`, which would scan every
/// event of the account once per pull request.
fn pr_events_sql() -> String {
    format!(
        "SELECT {EVENT_COLS} FROM activity_events e \
         CROSS JOIN account_events ae ON ae.event_id = e.event_id \
         WHERE e.parent_pr_id = ?2 AND ae.account_id = ?1"
    )
}

/// Known commit statistics of the account's repositories and diffs of its
/// pull requests.
#[allow(clippy::type_complexity)]
fn diff_stats_for(
    conn: &Connection,
    account_id: i64,
) -> Result<(HashMap<String, LineCounts>, BTreeMap<String, PrDiff>)> {
    let commits = commit_stats_where(
        conn,
        "c.repository_id IN \
         (SELECT repository_id FROM account_repositories WHERE account_id = ?1)",
        params![account_id],
    )?;
    let prs = pr_diffs_where(
        conn,
        "p.id IN (SELECT pr_id FROM account_pull_requests WHERE account_id = ?1)",
        params![account_id],
    )?;
    Ok((commits, prs))
}

/// Commit statistics matching `filter` (over `commits c`), keyed by the
/// commit's event id.
fn commit_stats_where<P: rusqlite::Params>(
    conn: &Connection,
    filter: &str,
    params: P,
) -> Result<HashMap<String, LineCounts>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT c.repository_id, c.sha, c.additions, c.deletions FROM commits c \
         WHERE c.additions IS NOT NULL AND c.deletions IS NOT NULL AND {filter}"
    ))?;
    let mut rows = stmt.query(params)?;
    let mut out = HashMap::new();
    while let Some(r) = rows.next()? {
        let repo: String = r.get(0)?;
        let sha: String = r.get(1)?;
        out.insert(
            EventId::commit(&repo, &sha).0,
            LineCounts {
                additions: r.get::<_, i64>(2)?.max(0) as u64,
                deletions: r.get::<_, i64>(3)?.max(0) as u64,
            },
        );
    }
    Ok(out)
}

/// Pull requests matching `filter` (over `pull_requests p`) that were merged
/// with a merge commit.
fn merged_by_merge_commit_where<P: rusqlite::Params>(
    conn: &Connection,
    filter: &str,
    params: P,
) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT p.id FROM pull_requests p WHERE p.merge_commit_parents >= 2 AND {filter}"
    ))?;
    let ids = stmt
        .query_map(params, |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    Ok(ids)
}

/// Latest observed diffs of the pull requests matching `filter` (over
/// `pull_requests p`).
fn pr_diffs_where<P: rusqlite::Params>(
    conn: &Connection,
    filter: &str,
    params: P,
) -> Result<BTreeMap<String, PrDiff>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT p.id, p.additions, p.deletions, p.changed_files, p.diff_observed_at, \
         p.diff_observed_at_ns FROM pull_requests p \
         WHERE p.diff_observed_at IS NOT NULL AND {filter}"
    ))?;
    let mut rows = stmt.query(params)?;
    let mut out = BTreeMap::new();
    while let Some(r) = rows.next()? {
        let n = |i: usize| -> Result<u64> {
            Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0).max(0) as u64)
        };
        out.insert(
            r.get::<_, String>(0)?,
            PrDiff {
                additions: n(1)?,
                deletions: n(2)?,
                changed_files: n(3)?,
                observed_at: time(r.get(4)?, r.get(5)?)?,
            },
        );
    }
    Ok(out)
}

fn coverage_for(conn: &Connection, account_id: i64) -> Result<Vec<CoverageRecord>> {
    query_list(
        conn,
        "SELECT * FROM coverage_intervals WHERE account_id = ?1 \
         ORDER BY recorded_at, recorded_at_ns, id",
        params![account_id],
        |r| {
            let kind: String = r.get("data_kind")?;
            Ok(CoverageRecord {
                kind: DataKind::parse(&kind)
                    .ok_or_else(|| StorageError::Data(format!("unknown data kind {kind:?}")))?,
                start: time(r.get("start_at")?, r.get("start_at_ns")?)?,
                end: time(r.get("end_at")?, r.get("end_at_ns")?)?,
                status: CoverageStatus::parse(&r.get::<_, String>("status")?),
                scope: r.get("scope")?,
                reason: r.get("reason")?,
                recorded_at: time(r.get("recorded_at")?, r.get("recorded_at_ns")?)?,
            })
        },
    )
}

fn repo_from_row(r: &Row<'_>) -> Result<Repository> {
    Ok(Repository {
        id: r.get("id")?,
        owner: r.get("owner")?,
        name: r.get("name")?,
        owner_kind: codec::parse_enum("owner kind", &r.get::<_, String>("owner_kind")?)?,
        visibility: Visibility::parse(&r.get::<_, String>("visibility")?),
        is_fork: r.get("is_fork")?,
        viewer_permission: r.get("viewer_permission")?,
        accessible: r.get("accessible")?,
        url: r.get("url")?,
    })
}

fn pr_from_row(r: &Row<'_>) -> Result<PullRequest> {
    let state: String = r.get("state")?;
    Ok(PullRequest {
        id: r.get("id")?,
        repository_id: r.get("repository_id")?,
        number: r.get::<_, i64>("number")?.max(0) as u64,
        title: r.get("title")?,
        url: r.get("url")?,
        author: actor(
            r.get("author_id")?,
            r.get("author_login")?,
            &r.get::<_, String>("author_kind")?,
        )?,
        state: PrState::parse(&state)
            .ok_or_else(|| StorageError::Data(format!("unknown stored PR state {state:?}")))?,
        is_draft: r.get("is_draft")?,
        created_at: time(r.get("created_at")?, r.get("created_at_ns")?)?,
        updated_at: opt_time(r.get("updated_at")?, r.get("updated_at_ns")?)?,
        closed_at: opt_time(r.get("closed_at")?, r.get("closed_at_ns")?)?,
        merged_at: opt_time(r.get("merged_at")?, r.get("merged_at_ns")?)?,
        merged_by: opt_from_json("merged-by actor", r.get("merged_by")?)?,
        head_sha: r.get("head_sha")?,
        latest_ci: r
            .get::<_, Option<String>>("latest_ci")?
            .map(|s| CheckOutcome::parse(&s)),
        coverage: CoverageStatus::parse(&r.get::<_, String>("coverage")?),
        synced_at: opt_time(r.get("synced_at")?, r.get("synced_at_ns")?)?,
    })
}

fn issue_from_row(r: &Row<'_>) -> Result<Issue> {
    Ok(Issue {
        id: r.get("id")?,
        repository_id: r.get("repository_id")?,
        number: r.get::<_, i64>("number")?.max(0) as u64,
        title: r.get("title")?,
        url: r.get("url")?,
        author: actor(
            r.get("author_id")?,
            r.get("author_login")?,
            &r.get::<_, String>("author_kind")?,
        )?,
        is_open: r.get("is_open")?,
        created_at: time(r.get("created_at")?, r.get("created_at_ns")?)?,
        closed_at: opt_time(r.get("closed_at")?, r.get("closed_at_ns")?)?,
    })
}

fn thread_from_row(r: &Row<'_>) -> Result<ReviewThread> {
    Ok(ReviewThread {
        id: r.get("id")?,
        pr_id: r.get("pr_id")?,
        is_resolved: r.get("is_resolved")?,
        is_outdated: r.get("is_outdated")?,
        resolved_by: opt_from_json("resolved-by actor", r.get("resolved_by")?)?,
        path: r.get("path")?,
        comment_count: r.get("comment_count")?,
        first_comment_author: opt_from_json(
            "first comment author",
            r.get("first_comment_author")?,
        )?,
    })
}

/// Column positions of an event query, looked up once per statement instead
/// of by name on every row (the name lookup dominated large loads).
struct EventCols {
    event_id: usize,
    event_type: usize,
    actor_id: usize,
    actor_login: usize,
    actor_kind: usize,
    repository_id: usize,
    entity_id: usize,
    parent_pr_id: usize,
    occurred_at: usize,
    occurred_at_ns: usize,
    observed_at: usize,
    observed_at_ns: usize,
    source: usize,
    url: usize,
    meta: usize,
    author_date: usize,
    author_date_ns: usize,
    removed_observed_at: usize,
    removed_observed_at_ns: usize,
    coverage: usize,
}

impl EventCols {
    fn of(stmt: &rusqlite::Statement<'_>) -> Result<Self> {
        let i = |name: &str| stmt.column_index(name);
        Ok(Self {
            event_id: i("event_id")?,
            event_type: i("event_type")?,
            actor_id: i("actor_id")?,
            actor_login: i("actor_login")?,
            actor_kind: i("actor_kind")?,
            repository_id: i("repository_id")?,
            entity_id: i("entity_id")?,
            parent_pr_id: i("parent_pr_id")?,
            occurred_at: i("occurred_at")?,
            occurred_at_ns: i("occurred_at_ns")?,
            observed_at: i("observed_at")?,
            observed_at_ns: i("observed_at_ns")?,
            source: i("source")?,
            url: i("url")?,
            meta: i("meta")?,
            author_date: i("author_date")?,
            author_date_ns: i("author_date_ns")?,
            removed_observed_at: i("removed_observed_at")?,
            removed_observed_at_ns: i("removed_observed_at_ns")?,
            coverage: i("coverage")?,
        })
    }

    fn event(&self, r: &Row<'_>) -> Result<ActivityEvent> {
        let ty: String = r.get(self.event_type)?;
        let source: String = r.get(self.source)?;
        let mut meta: EventMeta = from_json("event metadata", &r.get::<_, String>(self.meta)?)?;
        meta.author_date = opt_time(r.get(self.author_date)?, r.get(self.author_date_ns)?)?;
        meta.removed_observed_at = opt_time(
            r.get(self.removed_observed_at)?,
            r.get(self.removed_observed_at_ns)?,
        )?;
        Ok(ActivityEvent {
            event_id: EventId(r.get(self.event_id)?),
            event_type: EventType::parse(&ty)
                .ok_or_else(|| StorageError::Data(format!("unknown stored event type {ty:?}")))?,
            actor: actor(
                r.get(self.actor_id)?,
                r.get(self.actor_login)?,
                &r.get::<_, String>(self.actor_kind)?,
            )?,
            repository_id: r.get(self.repository_id)?,
            entity_id: r.get(self.entity_id)?,
            parent_pr_id: r.get(self.parent_pr_id)?,
            occurred_at: time(r.get(self.occurred_at)?, r.get(self.occurred_at_ns)?)?,
            observed_at: time(r.get(self.observed_at)?, r.get(self.observed_at_ns)?)?,
            source: Source::parse(&source)
                .ok_or_else(|| StorageError::Data(format!("unknown stored source {source:?}")))?,
            url: r.get(self.url)?,
            meta,
            coverage: CoverageStatus::parse(&r.get::<_, String>(self.coverage)?),
        })
    }
}

fn query_events<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<ActivityEvent>> {
    let mut stmt = conn.prepare(sql)?;
    let cols = EventCols::of(&stmt)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(cols.event(row)?);
    }
    Ok(out)
}

fn sync_run_from_row(r: &Row<'_>) -> Result<SyncRun> {
    Ok(SyncRun {
        id: r.get("id")?,
        account: r.get("login")?,
        started_at: time(r.get("started_at")?, r.get("started_at_ns")?)?,
        finished_at: opt_time(r.get("finished_at")?, r.get("finished_at_ns")?)?,
        status: r.get("status")?,
        period_start: opt_time(r.get("period_start")?, r.get("period_start_ns")?)?,
        period_end: opt_time(r.get("period_end")?, r.get("period_end_ns")?)?,
        stats_json: r.get("stats_json")?,
        message: r.get("message")?,
    })
}

// ---------------------------------------------------------------------------
// Sync lock

/// Exclusive lock held for the duration of a sync. Released on drop.
pub struct SyncLock {
    file: File,
    path: PathBuf,
}

impl SyncLock {
    /// Acquires `<dir>/sync.lock` without blocking; fails with
    /// [`StorageError::Locked`] if another process holds it.
    pub fn acquire(dir: &Path) -> Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join("sync.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => return Err(StorageError::Locked(path)),
            Err(fs4::TryLockError::Error(e)) => return Err(StorageError::Io(e)),
        }
        // Best effort: record the holder's process id for diagnostics.
        if file.set_len(0).is_ok() {
            let _ = writeln!(file, "{}", std::process::id());
        }
        Ok(Self { file, path })
    }

    /// Path of the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

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
