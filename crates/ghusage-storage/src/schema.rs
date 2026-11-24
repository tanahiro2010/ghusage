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
    v5_repair_viewed_participation,
];

#[expect(
    clippy::cast_possible_truncation,
    reason = "The migration list is a compile-time constant with five entries."
)]
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
    conn.query_row("PRAGMA user_version", [], |r| {
        let version = r.get::<_, i64>(0)?;
        u32::try_from(version.max(0))
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, version))
    })
}

/// Migrates `conn` to `steps.len()`. When `backup_dir_file` is given and the
/// database already holds a schema older than the target, a copy of the
/// database is written next to it as `<file>.bak-v<from>` first.
pub(crate) fn migrate(
    conn: &mut Connection,
    steps: &[Migration],
    backup_of: Option<&Path>,
) -> Result<()> {
    let target = u32::try_from(steps.len())
        .map_err(|_| StorageError::Data("too many schema migrations".into()))?;
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
    for (i, step) in steps
        .iter()
        .enumerate()
        .skip(usize::try_from(from).unwrap_or(usize::MAX))
    {
        let to = u32::try_from(i + 1)
            .map_err(|_| StorageError::Data("too many schema migrations".into()))?;
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
        r"
CREATE TABLE commit_details (
    repository     TEXT NOT NULL COLLATE NOCASE,
    sha            TEXT NOT NULL,
    fetched_at     INTEGER NOT NULL,
    fetched_at_ns  INTEGER NOT NULL,
    detail         TEXT NOT NULL,
    PRIMARY KEY (repository, sha)
) WITHOUT ROWID;
",
    )
}

/// Lines added and deleted per commit, how merged pull requests were merged,
/// and the latest observed diff of each pull request. `NULL` means not known
/// (never zero by default).
fn v3_diff_stats(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r"
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
",
    )
}

/// Adds explicit coverage dimensions and viewed state, then folds legacy
/// synthetic `+viewed` accounts into their real account without moving events.
fn v4_domain_models(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r"
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
ON CONFLICT(account_id, pr_id) DO UPDATE SET
    viewed = min(account_pull_requests.viewed, excluded.viewed);
DELETE FROM accounts WHERE login LIKE '%+viewed';
",
    )
}

/// Restores normal PR relationships in v4 caches only when persisted GitHub
/// data proves participation. Ambiguous viewed relationships are retained.
fn v5_repair_viewed_participation(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(
        r"
UPDATE account_pull_requests AS ap
SET viewed = 0
FROM accounts AS a, pull_requests AS p
WHERE ap.account_id = a.id
  AND ap.pr_id = p.id
  AND ap.viewed = 1
  AND (
      p.author_login = a.login COLLATE NOCASE
      OR EXISTS (
          SELECT 1 FROM account_events ae
          JOIN activity_events e ON e.event_id = ae.event_id
          WHERE ae.account_id = ap.account_id AND e.parent_pr_id = ap.pr_id
      )
      OR EXISTS (
          SELECT 1 FROM activity_events e
          WHERE e.parent_pr_id = ap.pr_id
            AND e.actor_login = a.login COLLATE NOCASE
      )
      OR EXISTS (
          SELECT 1 FROM pull_request_reviews r
          WHERE r.pr_id = ap.pr_id AND r.author_login = a.login COLLATE NOCASE
      )
      OR EXISTS (
          SELECT 1 FROM pull_request_comments c
          WHERE c.pr_id = ap.pr_id AND c.author_login = a.login COLLATE NOCASE
      )
      OR EXISTS (
          SELECT 1 FROM pull_request_commits pc
          JOIN commits c ON c.repository_id = pc.repository_id AND c.sha = pc.sha
          WHERE pc.pr_id = ap.pr_id AND c.author_login = a.login COLLATE NOCASE
      )
      OR EXISTS (
          SELECT 1 FROM activity_events e
          WHERE e.parent_pr_id = ap.pr_id
            AND e.event_type IN ('review_requested', 'assigned')
            AND json_valid(e.meta)
            AND json_extract(e.meta, '$.target') = a.login COLLATE NOCASE
      )
  );
",
    )
}

const V1_SQL: &str = r"
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
";

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
            r"
INSERT INTO repositories(id, owner, name, owner_kind, visibility, is_fork, accessible)
VALUES ('R1', 'owner', 'repo', 'User', 'public', 0, 1),
       ('R2', 'owner', 'viewed-only', 'User', 'public', 0, 1);
INSERT INTO accounts(login, created_at) VALUES
    ('alice', 1), ('alice+viewed', 2), ('bob', 3), ('bob+viewed', 4);
INSERT INTO pull_requests(id, repository_id, number, title, url, author_login, author_kind, state,
    is_draft, created_at, created_at_ns, coverage)
VALUES ('P1', 'R1', 1, 'title', 'url', 'bob', 'User', 'open', 0, 1, 0, 'complete'),
       ('P2', 'R1', 2, 'view only', 'url', 'bob', 'User', 'open', 0, 1, 0, 'complete'),
       ('P4', 'R1', 4, 'bob view only', 'url', 'alice', 'User', 'open', 0, 1, 0, 'complete');
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P1' FROM accounts WHERE login = 'alice';
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P1' FROM accounts WHERE login = 'alice+viewed';
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P2' FROM accounts WHERE login = 'alice+viewed';
INSERT INTO account_pull_requests(account_id, pr_id)
SELECT id, 'P4' FROM accounts WHERE login = 'bob+viewed';
INSERT INTO account_repositories(account_id, repository_id)
SELECT id, 'R1' FROM accounts WHERE login = 'alice';
INSERT INTO account_repositories(account_id, repository_id)
SELECT id, 'R2' FROM accounts WHERE login = 'alice+viewed';
INSERT INTO coverage_intervals(account_id, data_kind, start_at, start_at_ns, end_at,
    end_at_ns, status, scope, recorded_at, recorded_at_ns)
SELECT id, 'commits', 1, 0, 2, 0, 'complete',
    'commits authored (repository listing + commit search; other repositories by commit search alone, copies excluded)', 3, 0
FROM accounts WHERE login = 'alice';
INSERT INTO coverage_intervals(account_id, data_kind, start_at, start_at_ns, end_at,
    end_at_ns, status, scope, recorded_at, recorded_at_ns)
SELECT id, 'commits', 2, 0, 3, 0, 'complete', 'unknown scope', 3, 1
FROM accounts WHERE login = 'alice';
INSERT INTO coverage_intervals(account_id, data_kind, start_at, start_at_ns, end_at,
    end_at_ns, status, scope, recorded_at, recorded_at_ns)
SELECT id, kind, start_at, 0, start_at + 1, 0, 'complete', scope, start_at + 2, 0
FROM accounts CROSS JOIN (
    SELECT 'commits' AS kind, 4 AS start_at,
        'commits authored by alice on the default branches of repositories alice owns or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests' AS scope
    UNION ALL SELECT 'commits', 6,
        'commits authored by alice on the default branches of repositories alice owns or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests (public repositories only)'
    UNION ALL SELECT 'pull_requests', 8,
        'pull requests involving or reviewed by alice updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks'
    UNION ALL SELECT 'pull_requests', 10,
        'pull requests involving or reviewed by alice updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks (public repositories only)'
    UNION ALL SELECT 'issues', 12,
        'issues involving alice updated in range, with open/close/reopen history'
    UNION ALL SELECT 'issues', 14,
        'issues involving alice updated in range, with open/close/reopen history (public repositories only)'
    UNION ALL SELECT 'commits', 16,
        'commits authored by alice on the default branches of repositories alice pushed to or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests'
    UNION ALL SELECT 'commits', 18,
        'commits authored by alice on the default branches of repositories alice pushed to or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests (public repositories only)'
) legacy_scopes
WHERE accounts.login = 'alice';
",
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
        // Existing genuine participation must win over a legacy viewed row.
        assert_eq!(viewed, 0);
        assert_eq!(account_count, 0);
        let p2_viewed: i64 = conn.query_row(
            "SELECT ap.viewed FROM account_pull_requests ap JOIN accounts a ON a.id = ap.account_id \
             WHERE a.login = 'alice' AND ap.pr_id = 'P2'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(
            p2_viewed, 1,
            "a view without normal participation stays viewed-only"
        );
        let bob_viewed: i64 = conn.query_row(
            "SELECT ap.viewed FROM account_pull_requests ap JOIN accounts a ON a.id = ap.account_id \
             WHERE a.login = 'bob' AND ap.pr_id = 'P4'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(
            bob_viewed, 1,
            "alice's migration must not alter bob's relationship"
        );
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
        let repo_ids: Vec<String> = conn
            .prepare(
                "SELECT r.id FROM account_repositories ar JOIN repositories r ON r.id = ar.repository_id \
                 JOIN accounts a ON a.id = ar.account_id WHERE a.login = 'alice' ORDER BY r.id",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            repo_ids,
            ["R1"],
            "legacy view-only repositories are not activity"
        );
        let generations: Vec<(String, String, String, String)> = conn
            .prepare(
                "SELECT data_kind, visibility, generation, scope \
                 FROM coverage_intervals ORDER BY start_at",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            generations,
            [
                ("commits".into(), "all".into(), "commits_listing_search_v1".into(), "commits authored (repository listing + commit search; other repositories by commit search alone, copies excluded)".into()),
                ("commits".into(), "all".into(), "legacy_unusable".into(), "unknown scope".into()),
                ("commits".into(), "all".into(), "commits_listing_search_v1".into(), "commits authored by alice on the default branches of repositories alice owns or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests".into()),
                ("commits".into(), "public_only".into(), "commits_listing_search_v1".into(), "commits authored by alice on the default branches of repositories alice owns or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests (public repositories only)".into()),
                ("pull_requests".into(), "all".into(), "pull_requests_v1".into(), "pull requests involving or reviewed by alice updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks".into()),
                ("pull_requests".into(), "public_only".into(), "pull_requests_v1".into(), "pull requests involving or reviewed by alice updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks (public repositories only)".into()),
                ("issues".into(), "all".into(), "issues_v1".into(), "issues involving alice updated in range, with open/close/reopen history".into()),
                ("issues".into(), "public_only".into(), "issues_v1".into(), "issues involving alice updated in range, with open/close/reopen history (public repositories only)".into()),
                ("commits".into(), "all".into(), "commits_listing_search_v1".into(), "commits authored by alice on the default branches of repositories alice pushed to or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests".into()),
                ("commits".into(), "public_only".into(), "commits_listing_search_v1".into(), "commits authored by alice on the default branches of repositories alice pushed to or contributed to (repository listing + commit search; other repositories by commit search alone, copies excluded), plus commits of synced pull requests (public repositories only)".into()),
            ]
        );
    }

    #[test]
    fn v4_repair_restores_only_proven_participation_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(
            &mut conn,
            &[
                v1_initial,
                v2_commit_details,
                v3_diff_stats,
                v4_domain_models,
            ],
            None,
        )
        .unwrap();
        conn.execute_batch(
            r#"
INSERT INTO repositories(id, owner, name, owner_kind, visibility, is_fork, accessible)
VALUES ('R1', 'owner', 'repo', 'User', 'public', 0, 1);
INSERT INTO accounts(login, created_at) VALUES ('alice', 1), ('bob', 1);
INSERT INTO pull_requests(id, repository_id, number, title, url, author_login, author_kind,
    state, is_draft, created_at, created_at_ns, coverage)
VALUES ('P1','R1',1,'t','u','alice','User','open',0,1,0,'complete'),
       ('P2','R1',2,'t','u','bob','User','open',0,1,0,'complete'),
       ('P3','R1',3,'t','u','bob','User','open',0,1,0,'complete'),
       ('P4','R1',4,'t','u','alice','User','open',0,1,0,'complete'),
       ('P5','R1',5,'t','u','bob','User','open',0,1,0,'complete'),
       ('P6','R1',6,'t','u','bob','User','open',0,1,0,'complete'),
       ('P7','R1',7,'t','u','bob','User','open',0,1,0,'complete'),
       ('P8','R1',8,'t','u','bob','User','open',0,1,0,'complete');
INSERT INTO account_pull_requests(account_id, pr_id, viewed)
SELECT a.id, p.id, 1 FROM accounts a CROSS JOIN pull_requests p
WHERE a.login = 'alice' AND p.id <> 'P4';
INSERT INTO account_pull_requests(account_id, pr_id, viewed)
SELECT a.id, 'P4', 1 FROM accounts a WHERE a.login = 'bob';
INSERT INTO activity_events(event_id,event_type,actor_login,actor_kind,repository_id,entity_id,
    parent_pr_id,occurred_at,occurred_at_ns,observed_at,observed_at_ns,source,meta,coverage)
VALUES ('E2','review_submitted','reviewer','User','R1','E2','P2',1,0,1,0,'graphql','{}','complete'),
       ('E5','review_submitted','reviewer','User','R1','E5','P5',1,0,1,0,'graphql','{}','complete'),
       ('E6','conversation_comment','reviewer','User','R1','E6','P6',1,0,1,0,'graphql','{}','complete'),
       ('E8','review_requested','reviewer','User','R1','E8','P8',1,0,1,0,'graphql','{"target":"alice"}','complete');
INSERT INTO account_events(account_id,event_id)
SELECT id, 'E2' FROM accounts WHERE login = 'alice';
INSERT INTO pull_request_reviews(event_id,review_id,pr_id,author_login,state,submitted_at)
VALUES ('E5','RV5','P5','alice','APPROVED',1);
INSERT INTO pull_request_comments(event_id,comment_id,pr_id,kind,author_login,created_at)
VALUES ('E6','C6','P6','conversation','alice',1);
INSERT INTO commits(repository_id,sha,author_login) VALUES ('R1','abc','alice');
INSERT INTO pull_request_commits(pr_id,repository_id,sha,is_current) VALUES ('P7','R1','abc',1);
"#,
        )
        .unwrap();

        migrate(&mut conn, MIGRATIONS, None).unwrap();
        let relationships = |conn: &Connection, account: &str| {
            let mut stmt = conn
                .prepare(
                    "SELECT pr_id, viewed FROM account_pull_requests ap \
                     JOIN accounts a ON a.id = ap.account_id WHERE a.login = ?1 ORDER BY pr_id",
                )
                .unwrap();
            stmt.query_map([account], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert_eq!(
            relationships(&conn, "alice"),
            [
                ("P1".into(), 0),
                ("P2".into(), 0),
                ("P3".into(), 1),
                ("P5".into(), 0),
                ("P6".into(), 0),
                ("P7".into(), 0),
                ("P8".into(), 0),
            ]
        );
        assert_eq!(relationships(&conn, "bob"), [("P4".into(), 1)]);

        let tx = conn.transaction().unwrap();
        v5_repair_viewed_participation(&tx).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            relationships(&conn, "alice"),
            [
                ("P1".into(), 0),
                ("P2".into(), 0),
                ("P3".into(), 1),
                ("P5".into(), 0),
                ("P6".into(), 0),
                ("P7".into(), 0),
                ("P8".into(), 0),
            ]
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
