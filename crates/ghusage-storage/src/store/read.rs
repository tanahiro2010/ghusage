impl Store {
    /// Pull request IDs associated with `account`, optionally limited to
    /// relationships marked as viewed-only.
    pub fn pull_request_ids(&self, account: &str, viewed_only: bool) -> Result<HashSet<String>> {
        let Some(account_id) = account_id(&self.conn, account)? else {
            return Ok(HashSet::new());
        };
        let viewed = i64::from(viewed_only);
        query_list(
            &self.conn,
            "SELECT pr_id FROM account_pull_requests WHERE account_id = ?1 AND viewed = ?2",
            params![account_id, viewed],
            |r| Ok(r.get(0)?),
        )
        .map(|ids| ids.into_iter().collect())
        .map_err(classify)
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
                 WHERE ap.account_id = ?1 AND ap.viewed = 0"
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
                 WHERE ap.account_id = ?1 AND ap.viewed = 0"
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
            "p.id IN (SELECT pr_id FROM account_pull_requests \
             WHERE account_id = ?1 AND viewed = 0)",
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


}

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
        "p.id IN (SELECT pr_id FROM account_pull_requests \
         WHERE account_id = ?1 AND viewed = 0)",
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
                additions: u64::try_from(r.get::<_, i64>(2)?.max(0)).unwrap_or_default(),
                deletions: u64::try_from(r.get::<_, i64>(3)?.max(0)).unwrap_or_default(),
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
            Ok(u64::try_from(r.get::<_, Option<i64>>(i)?.unwrap_or(0).max(0))
                .unwrap_or_default())
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



