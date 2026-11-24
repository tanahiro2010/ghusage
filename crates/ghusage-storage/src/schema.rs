//! Schema definition and migrations.
//!
//! The schema version lives in `PRAGMA user_version`. All pending migration
//! steps run inside one transaction, so a failing step leaves the database
//! exactly as it was before the migration started.

use std::path::Path;

use rusqlite::Connection;

use crate::{Result, StorageError};

/// One migration step: brings the schema from version `index` to `index + 1`.
pub(crate) type Migration = fn(&rusqlite::Transaction<'_>) -> rusqlite::Result<()>;

/// Every migration step, in order. The current schema version is its length.
pub(crate) const MIGRATIONS: &[Migration] = &[
    v1_initial,
    v2_commit_details,
    v3_diff_stats,
    v4_domain_models,
];

pub(crate) const CURRENT_VERSION: u32 = MIGRATIONS.len() as u32;

/// Data tables, children before parents, so they can be emptied in order.
pub(crate) const DATA_TABLES: &[&str] = &[
    "pull_request_reviews",
    "pull_request_comments",
    "check_runs",
    "account_events",
    "account_pull_requests",
    "account_issues",
    "account_repositories",
    "pull_request_commits",
    "review_threads",
    "activity_events",
    "commits",
    "pull_requests",
    "issues",
    "coverage_intervals",
    "sync_cursors",
    "sync_runs",
    "repositories",
    "accounts",
    "commit_details",
    "cache_metadata",
];

pub(crate) fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map(|v| v.max(0) as u32)
}

/// Migrates `conn` to `steps.len()`. When `backup_dir_file` is given and the
/// database already holds a schema older than the target, a copy of the
/// database is written next to it as `<file>.bak-v<from>` first.
pub(crate) fn migrate(
    conn: &mut Connection,
    steps: &[Migration],
    backup_of: Option<&Path>,
) -> Result<()> {
    let target = steps.len() as u32;
    let from = user_version(conn)?;
    if from > target {
        return Err(StorageError::TooNew {
            found: from,
            supported: target,
        });
    }
    if from == target {
        return Ok(());
    }
    if from > 0
        && let Some(path) = backup_of
    {
        backup(conn, path, from)?;
    }
    let tx = conn.transaction()?;
    for (i, step) in steps.iter().enumerate().skip(from as usize) {
        let to = i as u32 + 1;
        step(&tx).map_err(|e| StorageError::Migration {
            from,
            to,
            message: e.to_string(),
        })?;
    }
    // `user_version` cannot take a bound parameter; `target` is an integer
    // computed here, never user input.
    tx.execute_batch(&format!("PRAGMA user_version = {target}"))?;
    tx.commit().map_err(|e| StorageError::Migration {
        from,
        to: target,
        message: e.to_string(),
    })?;
    tracing::info!(from, to = target, "cache schema migrated");
    Ok(())
}

fn backup(conn: &Connection, path: &Path, from: u32) -> Result<()> {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".bak-v{from}"));
    let dest = path.with_file_name(name);
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }
    let dest_str = dest.to_str().ok_or_else(|| {
        StorageError::Data(format!("backup path is not UTF-8: {}", dest.display()))
    })?;
    conn.execute("VACUUM INTO ?1", [dest_str])?;
    tracing::info!(backup = %dest.display(), "cache backed up before migration");
    Ok(())
}

fn v1_initial(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(V1_SQL)
}

/// Commits fetched on demand with their files and diffs. The detail is
/// content, not activity, so it is kept apart from `activity_events` and
/// never affects counts.
fn v2_commit_details(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
CREATE TABLE commit_details (
    repository     TEXT NOT NULL COLLATE NOCASE,
    sha            TEXT NOT NULL,
    fetched_at     INTEGER NOT NULL,
    fetched_at_ns  INTEGER NOT NULL,
    detail         TEXT NOT NULL,
    PRIMARY KEY (repository, sha)
) WITHOUT ROWID;
"#,
    )
}

/// Lines added and deleted per commit, how merged pull requests were merged,
/// and the latest observed diff of each pull request. `NULL` means not known
/// (never zero by default).
fn v3_diff_stats(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
ALTER TABLE commits ADD COLUMN additions INTEGER;
ALTER TABLE commits ADD COLUMN deletions INTEGER;
ALTER TABLE commits ADD COLUMN stats_observed_at INTEGER;
ALTER TABLE commits ADD COLUMN stats_observed_at_ns INTEGER;
ALTER TABLE pull_requests ADD COLUMN additions INTEGER;
ALTER TABLE pull_requests ADD COLUMN deletions INTEGER;
ALTER TABLE pull_requests ADD COLUMN changed_files INTEGER;
ALTER TABLE pull_requests ADD COLUMN diff_observed_at INTEGER;
ALTER TABLE pull_requests ADD COLUMN diff_observed_at_ns INTEGER;
ALTER TABLE pull_requests ADD COLUMN merge_commit_parents INTEGER;
"#,
    )
}

/// Adds explicit coverage dimensions and viewed state, then folds legacy
/// synthetic `+viewed` accounts into their real account without moving events.
fn v4_domain_models(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r#"
ALTER TABLE coverage_intervals ADD COLUMN visibility TEXT NOT NULL DEFAULT 'all';
ALTER TABLE coverage_intervals ADD COLUMN generation TEXT NOT NULL DEFAULT 'legacy_unusable';
ALTER TABLE account_pull_requests ADD COLUMN viewed INTEGER NOT NULL DEFAULT 0;
CREATE INDEX account_pull_requests_account_viewed
    ON account_pull_requests(account_id, viewed, pr_id);
DROP INDEX coverage_intervals_unique;
UPDATE coverage_intervals SET visibility = CASE
    WHEN scope LIKE '%(public repositories only)' THEN 'public_only' ELSE 'all' END;
UPDATE coverage_intervals SET generation = CASE
    WHEN data_kind = 'commits' AND scope LIKE '%(repository listing + commit search; other repositories by commit search alone, copies excluded)%' THEN 'commits_listing_search_v1'
    WHEN data_kind = 'pull_requests' AND scope LIKE 'pull requests involving or reviewed by %' THEN 'pull_requests_v1'
    WHEN data_kind = 'issues' AND scope LIKE 'issues involving %' THEN 'issues_v1'
    ELSE 'legacy_unusable' END;
CREATE UNIQUE INDEX coverage_intervals_unique ON coverage_intervals(
    account_id, data_kind, start_at, start_at_ns, end_at, end_at_ns, status, scope,
    visibility, generation, ifnull(reason, ''), recorded_at, recorded_at_ns
);
UPDATE account_pull_requests SET viewed = 1 WHERE account_id IN
    (SELECT id FROM accounts WHERE login LIKE '%+viewed');
INSERT OR IGNORE INTO accounts(login, created_at)
SELECT substr(login, 1, length(login) - length('+viewed')), created_at
FROM accounts WHERE login LIKE '%+viewed';
INSERT INTO account_pull_requests (account_id, pr_id, viewed)
SELECT real.id, ap.pr_id, 1 FROM account_pull_requests ap
JOIN accounts old ON old.id = ap.account_id AND old.login LIKE '%+viewed'
JOIN accounts real ON real.login = substr(old.login, 1, length(old.login) - length('+viewed'))
ON CONFLICT(account_id, pr_id) DO UPDATE SET viewed = 1;
INSERT OR IGNORE INTO account_repositories (account_id, repository_id)
SELECT real.id, ar.repository_id FROM account_repositories ar
JOIN accounts old ON old.id = ar.account_id AND old.login LIKE '%+viewed'
JOIN accounts real ON real.login = substr(old.login, 1, length(old.login) - length('+viewed'));
DELETE FROM accounts WHERE login LIKE '%+viewed';
"#,
    )
}

const V1_SQL: &str = r#"
CREATE TABLE accounts (
    id          INTEGER PRIMARY KEY,
    login       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at  INTEGER NOT NULL
);

CREATE TABLE repositories (
    id                 TEXT PRIMARY KEY,
    owner              TEXT NOT NULL,
    name               TEXT NOT NULL,
    owner_kind         TEXT NOT NULL,
    visibility         TEXT NOT NULL,
    is_fork            INTEGER NOT NULL,
    viewer_permission  TEXT,
    accessible         INTEGER NOT NULL,
    url                TEXT
);
CREATE INDEX repositories_full_name
    ON repositories(owner COLLATE NOCASE, name COLLATE NOCASE);

CREATE TABLE account_repositories (
    account_id     INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    repository_id  TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, repository_id)
) WITHOUT ROWID;

CREATE TABLE commits (
    repository_id    TEXT NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    sha              TEXT NOT NULL,
    author_login     TEXT,
    authored_at      INTEGER,
    committed_at     INTEGER,
    is_merge_commit  INTEGER,
    PRIMARY KEY (repository_id, sha)
) WITHOUT ROWID;

CREATE TABLE pull_requests (
    id              TEXT PRIMARY KEY,
    repository_id   TEXT NOT NULL REFERENCES repositories(id),
    number          INTEGER NOT NULL,
    title           TEXT NOT NULL,
    url             TEXT NOT NULL,
    author_id       TEXT,
    author_login    TEXT,
    author_kind     TEXT NOT NULL,
    state           TEXT NOT NULL,
    is_draft        INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    created_at_ns   INTEGER NOT NULL,
    updated_at      INTEGER,
    updated_at_ns   INTEGER,
    closed_at       INTEGER,
    closed_at_ns    INTEGER,
    merged_at       INTEGER,
    merged_at_ns    INTEGER,
    merged_by       TEXT,
    head_sha        TEXT,
    latest_ci       TEXT,
    coverage        TEXT NOT NULL,
    synced_at       INTEGER,
    synced_at_ns    INTEGER
);
CREATE INDEX pull_requests_repository ON pull_requests(repository_id, number);

CREATE TABLE account_pull_requests (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pr_id       TEXT NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, pr_id)
) WITHOUT ROWID;

CREATE TABLE pull_request_commits (
    pr_id          TEXT NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    repository_id  TEXT NOT NULL,
    sha            TEXT NOT NULL,
    is_current     INTEGER NOT NULL,
    PRIMARY KEY (pr_id, sha),
    FOREIGN KEY (repository_id, sha) REFERENCES commits(repository_id, sha)
) WITHOUT ROWID;
CREATE INDEX pull_request_commits_commit ON pull_request_commits(repository_id, sha);

CREATE TABLE review_threads (
    id                    TEXT PRIMARY KEY,
    pr_id                 TEXT NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    is_resolved           INTEGER NOT NULL,
    is_outdated           INTEGER NOT NULL,
    resolved_by           TEXT,
    path                  TEXT,
    comment_count         INTEGER NOT NULL,
    first_comment_author  TEXT
);
CREATE INDEX review_threads_pr ON review_threads(pr_id);

CREATE TABLE issues (
    id             TEXT PRIMARY KEY,
    repository_id  TEXT NOT NULL REFERENCES repositories(id),
    number         INTEGER NOT NULL,
    title          TEXT NOT NULL,
    url            TEXT NOT NULL,
    author_id      TEXT,
    author_login   TEXT,
    author_kind    TEXT NOT NULL,
    is_open        INTEGER NOT NULL,
    created_at     INTEGER NOT NULL,
    created_at_ns  INTEGER NOT NULL,
    closed_at      INTEGER,
    closed_at_ns   INTEGER
);
CREATE INDEX issues_repository ON issues(repository_id, number);

CREATE TABLE account_issues (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    issue_id    TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, issue_id)
) WITHOUT ROWID;

CREATE TABLE activity_events (
    event_id                TEXT NOT NULL PRIMARY KEY,
    event_type              TEXT NOT NULL,
    actor_id                TEXT,
    actor_login             TEXT,
    actor_kind              TEXT NOT NULL,
    repository_id           TEXT NOT NULL REFERENCES repositories(id),
    entity_id               TEXT NOT NULL,
    parent_pr_id            TEXT REFERENCES pull_requests(id),
    occurred_at             INTEGER NOT NULL,
    occurred_at_ns          INTEGER NOT NULL,
    observed_at             INTEGER NOT NULL,
    observed_at_ns          INTEGER NOT NULL,
    author_date             INTEGER,
    author_date_ns          INTEGER,
    source                  TEXT NOT NULL,
    url                     TEXT,
    meta                    TEXT NOT NULL,
    coverage                TEXT NOT NULL,
    removed_observed_at     INTEGER,
    removed_observed_at_ns  INTEGER
);
CREATE INDEX activity_events_occurred ON activity_events(occurred_at);
CREATE INDEX activity_events_author_date ON activity_events(author_date)
    WHERE author_date IS NOT NULL;
CREATE INDEX activity_events_parent_pr ON activity_events(parent_pr_id)
    WHERE parent_pr_id IS NOT NULL;
CREATE INDEX activity_events_repository ON activity_events(repository_id, occurred_at);

CREATE TABLE account_events (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    event_id    TEXT NOT NULL REFERENCES activity_events(event_id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, event_id)
) WITHOUT ROWID;
CREATE INDEX account_events_event ON account_events(event_id);

CREATE TABLE pull_request_reviews (
    event_id      TEXT PRIMARY KEY REFERENCES activity_events(event_id) ON DELETE CASCADE,
    review_id     TEXT NOT NULL,
    pr_id         TEXT REFERENCES pull_requests(id) ON DELETE CASCADE,
    author_login  TEXT,
    state         TEXT,
    submitted_at  INTEGER NOT NULL
);
CREATE INDEX pull_request_reviews_pr ON pull_request_reviews(pr_id);

CREATE TABLE pull_request_comments (
    event_id      TEXT PRIMARY KEY REFERENCES activity_events(event_id) ON DELETE CASCADE,
    comment_id    TEXT NOT NULL,
    pr_id         TEXT REFERENCES pull_requests(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL,
    thread_id     TEXT,
    author_login  TEXT,
    created_at    INTEGER NOT NULL,
    edited_at     INTEGER
);
CREATE INDEX pull_request_comments_pr ON pull_request_comments(pr_id);

CREATE TABLE check_runs (
    event_id     TEXT PRIMARY KEY REFERENCES activity_events(event_id) ON DELETE CASCADE,
    node_id      TEXT NOT NULL,
    pr_id        TEXT REFERENCES pull_requests(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    name         TEXT,
    outcome      TEXT,
    occurred_at  INTEGER NOT NULL
);
CREATE INDEX check_runs_pr ON check_runs(pr_id);

CREATE TABLE sync_runs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    started_at       INTEGER NOT NULL,
    started_at_ns    INTEGER NOT NULL,
    finished_at      INTEGER,
    finished_at_ns   INTEGER,
    status           TEXT NOT NULL,
    period_start     INTEGER,
    period_start_ns  INTEGER,
    period_end       INTEGER,
    period_end_ns    INTEGER,
    stats_json       TEXT,
    message          TEXT
);
CREATE INDEX sync_runs_account ON sync_runs(account_id, started_at);

CREATE TABLE sync_cursors (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (account_id, key)
) WITHOUT ROWID;

CREATE TABLE coverage_intervals (
    id                INTEGER PRIMARY KEY,
    account_id        INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    data_kind         TEXT NOT NULL,
    start_at          INTEGER NOT NULL,
    start_at_ns       INTEGER NOT NULL,
    end_at            INTEGER NOT NULL,
    end_at_ns         INTEGER NOT NULL,
    status            TEXT NOT NULL,
    scope             TEXT NOT NULL,
    reason            TEXT,
    recorded_at       INTEGER NOT NULL,
    recorded_at_ns    INTEGER NOT NULL
);
CREATE UNIQUE INDEX coverage_intervals_unique ON coverage_intervals(
    account_id, data_kind, start_at, start_at_ns, end_at, end_at_ns, status, scope,
    ifnull(reason, ''), recorded_at, recorded_at_ns
);

CREATE TABLE cache_metadata (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn failing_step(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
        tx.execute_batch("CREATE TABLE half_done (x INTEGER)")?;
        tx.execute_batch("THIS IS NOT SQL")
    }

    fn second_step(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
        tx.execute_batch("CREATE TABLE added_later (x INTEGER)")
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    #[test]
    fn empty_database_migrates_to_current_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, MIGRATIONS, None).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
        for t in DATA_TABLES {
            assert!(table_exists(&conn, t), "missing table {t}");
        }
        // Running again is a no-op.
        migrate(&mut conn, MIGRATIONS, None).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn failing_migration_step_leaves_database_unchanged() {
        let mut conn = Connection::open_in_memory().unwrap();
        let err = migrate(&mut conn, &[v1_initial, failing_step], None).unwrap_err();
        assert!(matches!(
            err,
            StorageError::Migration { from: 0, to: 2, .. }
        ));
        assert_eq!(user_version(&conn).unwrap(), 0);
        assert!(!table_exists(&conn, "activity_events"));
        assert!(!table_exists(&conn, "half_done"));
    }

    #[test]
    fn failing_upgrade_keeps_existing_data_and_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &[v1_initial], None).unwrap();
        conn.execute(
            "INSERT INTO cache_metadata(key, value) VALUES (?1, ?2)",
            ["k", "v"],
        )
        .unwrap();
        let err = migrate(&mut conn, &[v1_initial, failing_step], None).unwrap_err();
        assert!(matches!(
            err,
            StorageError::Migration { from: 1, to: 2, .. }
        ));
        assert_eq!(user_version(&conn).unwrap(), 1);
        assert!(!table_exists(&conn, "half_done"));
        let v: String = conn
            .query_row(
                "SELECT value FROM cache_metadata WHERE key = 'k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "v");
    }

    #[test]
    fn version_1_cache_gains_commit_details_and_keeps_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn, &[v1_initial], Some(&path)).unwrap();
        conn.execute(
            "INSERT INTO cache_metadata(key, value) VALUES (?1, ?2)",
            ["k", "v"],
        )
        .unwrap();
        assert!(!table_exists(&conn, "commit_details"));
        migrate(&mut conn, MIGRATIONS, Some(&path)).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
        assert!(table_exists(&conn, "commit_details"));
        let v: String = conn
            .query_row(
                "SELECT value FROM cache_metadata WHERE key = 'k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "v");
        assert!(dir.path().join("cache.db.bak-v1").exists());
    }

    #[test]
    fn version_2_cache_gains_unknown_diff_statistics() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &[v1_initial, v2_commit_details], None).unwrap();
        conn.execute_batch(
            "INSERT INTO repositories (id, owner, name, owner_kind, visibility, is_fork, \
             accessible) VALUES ('R1', 'o', 'r', 'User', 'public', 0, 1); \
             INSERT INTO commits (repository_id, sha, author_login) VALUES ('R1', 'abc', 'me');",
        )
        .unwrap();
        migrate(&mut conn, MIGRATIONS, None).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
        // Existing commits keep their data; their statistics are unknown
        // (NULL), not zero.
        let (login, additions): (String, Option<i64>) = conn
            .query_row(
                "SELECT author_login, additions FROM commits WHERE sha = 'abc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(login, "me");
        assert_eq!(additions, None);
    }

    #[test]
    fn v3_upgrade_structures_coverage_and_merges_viewed_account() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(
            &mut conn,
            &[v1_initial, v2_commit_details, v3_diff_stats],
            None,
        )
        .unwrap();
        conn.execute_batch(
            r#"
INSERT INTO repositories(id, owner, name, owner_kind, visibility, is_fork, accessible)
VALUES ('R1', 'owner', 'repo', 'User', 'public', 0, 1);
INSERT INTO accounts(login, created_at) VALUES ('alice', 1), ('alice+viewed', 2);
INSERT INTO pull_requests(id, repository_id, number, title, url, author_kind, state,
    is_draft, created_at, created_at_ns, coverage)
VALUES ('P1', 'R1', 1, 'title', 'url', 'User', 'open', 0, 1, 0, 'complete');
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P1' FROM accounts WHERE login = 'alice';
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P1' FROM accounts WHERE login = 'alice+viewed';
INSERT INTO account_repositories(account_id, repository_id)
SELECT id, 'R1' FROM accounts WHERE login = 'alice+viewed';
INSERT INTO coverage_intervals(account_id, data_kind, start_at, start_at_ns, end_at,
    end_at_ns, status, scope, recorded_at, recorded_at_ns)
SELECT id, 'commits', 1, 0, 2, 0, 'complete',
    'commits authored (repository listing + commit search; other repositories by commit search alone, copies excluded)', 3, 0
FROM accounts WHERE login = 'alice';
INSERT INTO coverage_intervals(account_id, data_kind, start_at, start_at_ns, end_at,
    end_at_ns, status, scope, recorded_at, recorded_at_ns)
SELECT id, 'commits', 2, 0, 3, 0, 'complete', 'unknown scope', 3, 1
FROM accounts WHERE login = 'alice';
"#,
        ).unwrap();

        migrate(&mut conn, MIGRATIONS, None).unwrap();
        let (viewed, account_count): (i64, i64) = conn
            .query_row(
                "SELECT ap.viewed, (SELECT count(*) FROM accounts WHERE login = 'alice+viewed') \
             FROM account_pull_requests ap JOIN accounts a ON a.id = ap.account_id \
             WHERE a.login = 'alice' AND ap.pr_id = 'P1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(viewed, 1);
        assert_eq!(account_count, 0);
        let repo_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM account_repositories ar \
                 JOIN accounts a ON a.id = ar.account_id \
                 WHERE a.login = 'alice' AND ar.repository_id = 'R1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repo_count, 1);
        let generations: Vec<String> = conn
            .prepare("SELECT generation FROM coverage_intervals ORDER BY start_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            generations,
            ["commits_listing_search_v1", "legacy_unusable"]
        );
    }

    #[test]
    fn newer_schema_is_rejected() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        let err = migrate(&mut conn, MIGRATIONS, None).unwrap_err();
        assert!(matches!(
            err,
            StorageError::TooNew {
                found: 99,
                supported: CURRENT_VERSION
            }
        ));
    }

    #[test]
    fn upgrading_an_existing_file_writes_a_backup_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn, &[v1_initial], Some(&path)).unwrap();
        assert!(!dir.path().join("cache.db.bak-v1").exists());
        migrate(&mut conn, &[v1_initial, second_step], Some(&path)).unwrap();
        assert_eq!(user_version(&conn).unwrap(), 2);
        let backup = dir.path().join("cache.db.bak-v1");
        assert!(backup.exists());
        let b = Connection::open(&backup).unwrap();
        assert_eq!(user_version(&b).unwrap(), 1);
        assert!(!table_exists(&b, "added_later"));
    }
}
