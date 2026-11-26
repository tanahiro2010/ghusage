impl Store {
    /// Applies a batch in a single transaction (all or nothing).
    pub fn apply(&mut self, batch: &WriteBatch) -> Result<()> {
        let tx = self.conn.transaction().map_err(|e| classify(e.into()))?;
        apply_batch(&tx, batch).map_err(classify)?;
        tx.commit().map_err(|e| classify(e.into()))
    }

}

impl Store {
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
                    to_i64(d.stats.additions)?,
                    to_i64(d.stats.deletions)?,
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

}

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

fn apply_batch(tx: &Transaction<'_>, b: &WriteBatch) -> Result<()> {
    let account_id = ensure_account(tx, &b.account)?;
    let viewed_prs: HashSet<&str> = b.viewed_pull_requests.iter().map(String::as_str).collect();
    let mut existing_viewed_prs = HashSet::new();
    for id in &viewed_prs {
        if row_exists(tx, "SELECT 1 FROM pull_requests WHERE id = ?1", id)? {
            existing_viewed_prs.insert(*id);
        }
    }
    let viewed_repositories: HashSet<&str> = b
        .pull_requests
        .iter()
        .filter(|p| viewed_prs.contains(p.id.as_str()))
        .map(|p| p.repository_id.as_str())
        .collect();
    let viewed_commit_events: HashSet<String> = b
        .pr_commits
        .iter()
        .filter(|link| viewed_prs.contains(link.pr_id.as_str()))
        .map(|link| EventId::commit(&link.repository_id, &link.sha).0)
        .collect();
    let viewed_commit_keys: HashSet<(String, String)> = b
        .pr_commits
        .iter()
        .filter(|link| viewed_prs.contains(link.pr_id.as_str()))
        .map(|link| (link.repository_id.clone(), link.sha.to_ascii_lowercase()))
        .collect();
    let active_repositories: HashSet<&str> = b
        .pull_requests
        .iter()
        .filter(|p| !viewed_prs.contains(p.id.as_str()))
        .map(|p| p.repository_id.as_str())
        .chain(b.issues.iter().map(|i| i.repository_id.as_str()))
        .chain(
            b.events
                .iter()
                .filter(|e| {
                    e.parent_pr_id
                        .as_deref()
                        .is_none_or(|id| !viewed_prs.contains(id))
                        && !viewed_commit_events.contains(e.event_id.as_str())
                })
                .map(|e| e.repository_id.as_str()),
        )
        .collect();

    for r in &b.repositories {
        upsert_repository(tx, r, viewed_repositories.contains(r.id.as_str()))?;
        if !viewed_repositories.contains(r.id.as_str())
            || active_repositories.contains(r.id.as_str())
        {
            associate(tx, ASSOC_REPO, account_id, &r.id)?;
        }
    }
    for p in &b.pull_requests {
        require_repository(tx, &p.repository_id, &format!("pull request {:?}", p.id))?;
        upsert_pull_request(tx, p, viewed_prs.contains(p.id.as_str()))?;
        if !viewed_prs.contains(p.id.as_str()) {
            associate(tx, ASSOC_PR, account_id, &p.id)?;
            associate(tx, ASSOC_REPO, account_id, &p.repository_id)?;
        }
    }
    for i in &b.issues {
        require_repository(tx, &i.repository_id, &format!("issue {:?}", i.id))?;
        upsert_issue(tx, i)?;
        associate(tx, ASSOC_ISSUE, account_id, &i.id)?;
        associate(tx, ASSOC_REPO, account_id, &i.repository_id)?;
    }
    for t in &b.threads {
        require_pull_request(tx, &t.pr_id, &format!("review thread {:?}", t.id))?;
        upsert_thread(tx, t, viewed_prs.contains(t.pr_id.as_str()))?;
    }
    for e in &b.events {
        let referrer = format!("event {:?}", e.event_id.as_str());
        require_repository(tx, &e.repository_id, &referrer)?;
        if let Some(pr) = &e.parent_pr_id {
            require_pull_request(tx, pr, &referrer)?;
        }
        let viewed_detail = e
            .parent_pr_id
            .as_deref()
            .is_some_and(|id| viewed_prs.contains(id))
            || viewed_commit_events.contains(e.event_id.as_str());
        let reattributed;
        let stored = match owning_pull_request(tx, e, viewed_detail)? {
            Some(owner) if e.parent_pr_id.as_deref() != Some(owner.as_str()) => {
                reattributed = ActivityEvent {
                    parent_pr_id: Some(owner),
                    ..e.clone()
                };
                &reattributed
            }
            _ => e,
        };
        let changed = upsert_event(tx, stored, viewed_detail)?;
        if changed || !viewed_detail {
            project_event(tx, stored)?;
        }
        if !viewed_detail && e
            .parent_pr_id
            .as_deref()
            .is_none_or(|id| !viewed_prs.contains(id))
        {
            associate(tx, ASSOC_EVENT, account_id, e.event_id.as_str())?;
            associate(tx, ASSOC_REPO, account_id, &e.repository_id)?;
            if let Some(pr) = &e.parent_pr_id {
                associate(tx, ASSOC_PR, account_id, pr)?;
            }
        }
    }
    for pr_id in &b.viewed_pull_requests {
        require_pull_request(tx, pr_id, "viewed pull request")?;
        tx.execute(
            "INSERT INTO account_pull_requests (account_id, pr_id, viewed) VALUES (?1, ?2, 1) \
             ON CONFLICT(account_id, pr_id) DO UPDATE SET \
             viewed = min(account_pull_requests.viewed, excluded.viewed)",
            params![account_id, pr_id],
        )?;
    }

    // Commit links sent in this batch, per pull request.
    let mut current_links: HashMap<&str, HashSet<String>> = HashMap::new();
    for l in &b.pr_commits {
        let referrer = format!("commit link {}:{}", l.pr_id, l.sha);
        require_pull_request(tx, &l.pr_id, &referrer)?;
        require_repository(tx, &l.repository_id, &referrer)?;
        if viewed_prs.contains(l.pr_id.as_str()) {
            insert_pr_commit(tx, l)?;
            continue;
        }
        upsert_pr_commit(tx, l)?;
        if l.is_current {
            current_links
                .entry(l.pr_id.as_str())
                .or_default()
                .insert(l.sha.to_ascii_lowercase());
        }
    }

    for c in &b.commit_stats {
        if viewed_commit_keys.contains(&(c.repository_id.clone(), c.sha.to_ascii_lowercase())) {
            continue;
        }
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
                to_i64(c.counts.additions)?,
                to_i64(c.counts.deletions)?,
                secs(c.observed_at),
                nanos(c.observed_at)
            ],
        )?;
    }
    for (pr_id, parents) in &b.pr_merge_commit_parents {
        if viewed_prs.contains(pr_id.as_str()) && existing_viewed_prs.contains(pr_id.as_str()) {
            continue;
        }
        require_pull_request(tx, pr_id, &format!("pull request merge {pr_id:?}"))?;
        tx.execute(
            "UPDATE pull_requests SET merge_commit_parents = ?2 WHERE id = ?1",
            params![pr_id, to_i64(*parents)?],
        )?;
    }
    for (pr_id, d) in &b.pr_diffs {
        if viewed_prs.contains(pr_id.as_str()) && existing_viewed_prs.contains(pr_id.as_str()) {
            continue;
        }
        require_pull_request(tx, pr_id, &format!("pull request diff {pr_id:?}"))?;
        // An older observation never replaces a newer one.
        tx.execute(
            "UPDATE pull_requests SET additions = ?2, deletions = ?3, changed_files = ?4, \
             diff_observed_at = ?5, diff_observed_at_ns = ?6 \
             WHERE id = ?1 AND (diff_observed_at IS NULL \
             OR (diff_observed_at, diff_observed_at_ns) <= (?5, ?6))",
            params![
                pr_id,
                to_i64(d.additions)?,
                to_i64(d.deletions)?,
                to_i64(d.changed_files)?,
                secs(d.observed_at),
                nanos(d.observed_at)
            ],
        )?;
    }

    let mut removal_time: HashMap<&str, Timestamp> = HashMap::new();
    for s in &b.pr_snapshots {
        if viewed_prs.contains(s.pr_id.as_str()) {
            continue;
        }
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
        .filter(|l| !viewed_prs.contains(l.pr_id.as_str()))
        .map(|l| l.pr_id.as_str())
        .chain(
            b.pr_snapshots
                .iter()
                .filter(|s| !viewed_prs.contains(s.pr_id.as_str()))
                .map(|s| s.pr_id.as_str()),
        )
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

fn upsert_repository(tx: &Transaction<'_>, r: &Repository, preserve_existing: bool) -> Result<()> {
    let conflict = if preserve_existing {
        "ON CONFLICT(id) DO NOTHING"
    } else {
        "ON CONFLICT(id) DO UPDATE SET owner = excluded.owner, name = excluded.name, \
         owner_kind = excluded.owner_kind, visibility = excluded.visibility, \
         is_fork = excluded.is_fork, viewer_permission = excluded.viewer_permission, \
         accessible = excluded.accessible, url = excluded.url"
    };
    tx.execute(
        &format!(
            "INSERT INTO repositories (id, owner, name, owner_kind, visibility, is_fork, \
             viewer_permission, accessible, url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) {conflict}"
        ),
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

fn upsert_pull_request(
    tx: &Transaction<'_>,
    p: &PullRequest,
    preserve_existing: bool,
) -> Result<()> {
    let conflict = if preserve_existing {
        "ON CONFLICT(id) DO NOTHING"
    } else {
        "ON CONFLICT(id) DO UPDATE SET repository_id = excluded.repository_id, \
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
         synced_at_ns = excluded.synced_at_ns"
    };
    tx.execute(
        &format!(
        "INSERT INTO pull_requests (id, repository_id, number, title, url, author_id, \
         author_login, author_kind, state, is_draft, created_at, created_at_ns, updated_at, \
         updated_at_ns, closed_at, closed_at_ns, merged_at, merged_at_ns, merged_by, head_sha, \
         latest_ci, coverage, synced_at, synced_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24) {conflict}"
        ),
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
            p.latest_ci.map(CheckOutcome::as_str),
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

fn upsert_thread(
    tx: &Transaction<'_>,
    t: &ReviewThread,
    preserve_existing: bool,
) -> Result<()> {
    let conflict = if preserve_existing {
        "ON CONFLICT(id) DO NOTHING"
    } else {
        "ON CONFLICT(id) DO UPDATE SET pr_id = excluded.pr_id, \
         is_resolved = excluded.is_resolved, is_outdated = excluded.is_outdated, \
         resolved_by = excluded.resolved_by, path = excluded.path, \
         comment_count = excluded.comment_count, \
         first_comment_author = excluded.first_comment_author"
    };
    tx.execute(
        &format!(
            "INSERT INTO review_threads (id, pr_id, is_resolved, is_outdated, resolved_by, path, \
             comment_count, first_comment_author) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) {conflict}"
        ),
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

/// The pull request an event is stored under. An event seen through two
/// pull requests (the checks of a head commit they share) keeps one owner
/// whatever the order they are fetched in: a pull request synced by an
/// account before one only viewed, then the lowest number, then the lowest
/// id. `None` keeps the event's own parent.
fn owning_pull_request(
    tx: &Transaction<'_>,
    e: &ActivityEvent,
    viewed_detail: bool,
) -> Result<Option<String>> {
    let Some(incoming) = e.parent_pr_id.as_deref() else {
        return Ok(None);
    };
    if viewed_detail {
        return Ok(None);
    }
    let existing: Option<String> = tx
        .query_row(
            "SELECT parent_pr_id FROM activity_events WHERE event_id = ?1",
            params![e.event_id.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(existing) = existing.filter(|x| x != incoming) else {
        return Ok(None);
    };
    Ok(tx
        .query_row(
            "SELECT id FROM pull_requests p WHERE id IN (?1, ?2) \
             ORDER BY NOT EXISTS (SELECT 1 FROM account_pull_requests ap \
                 WHERE ap.pr_id = p.id AND ap.viewed = 0), number, id LIMIT 1",
            params![existing, incoming],
            |r| r.get(0),
        )
        .optional()?)
}

/// Upserts an event by id. A removal observation already stored is kept
/// (the earliest one wins); only a complete resync clears it. A commit
/// author date already stored is kept when the new copy has none.
fn upsert_event(
    tx: &Transaction<'_>,
    e: &ActivityEvent,
    preserve_existing: bool,
) -> Result<bool> {
    let mut meta = e.meta.clone();
    let author_date = meta.author_date.take();
    let removed = meta.removed_observed_at.take();
    let conflict = if preserve_existing {
        "ON CONFLICT(event_id) DO NOTHING"
    } else {
        "ON CONFLICT(event_id) DO UPDATE SET event_type = excluded.event_type, \
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
             THEN excluded.removed_observed_at_ns ELSE activity_events.removed_observed_at_ns END"
    };
    let changed = tx.execute(
        &format!(
        "INSERT INTO activity_events (event_id, event_type, actor_id, actor_login, actor_kind, \
         repository_id, entity_id, parent_pr_id, occurred_at, occurred_at_ns, observed_at, \
         observed_at_ns, author_date, author_date_ns, source, url, meta, coverage, \
         removed_observed_at, removed_observed_at_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20) {conflict}"
        ),
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
    Ok(changed > 0)
}

/// Maintains the entity projections (commits, reviews, comments, checks).
fn project_event(tx: &Transaction<'_>, e: &ActivityEvent) -> Result<()> {
    use EventType::{CommitAuthored, PrCommit, ReviewSubmitted, ConversationComment, InlineComment, ReviewSummary, CheckRun, CommitStatus};
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
                    e.meta
                        .review_state
                        .map(ghusage_core::model::ReviewState::as_str),
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

/// Adds a commit link discovered while viewing a PR without changing an
/// existing relationship or the commit's stored metadata.
fn insert_pr_commit(tx: &Transaction<'_>, l: &PrCommitLink) -> Result<()> {
    let sha = l.sha.to_ascii_lowercase();
    tx.execute(
        "INSERT INTO commits (repository_id, sha) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
        params![l.repository_id, sha],
    )?;
    tx.execute(
        "INSERT INTO pull_request_commits (pr_id, repository_id, sha, is_current) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(pr_id, sha) DO NOTHING",
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
         end_at_ns, status, scope, reason, recorded_at, recorded_at_ns, visibility, generation) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) ON CONFLICT DO NOTHING",
        params![
            account_id,
            c.kind.as_str(),
            secs(c.start),
            nanos(c.start),
            secs(c.end),
            nanos(c.end),
            c.status.as_str(),
            c.description,
            c.reason,
            secs(c.recorded_at),
            nanos(c.recorded_at),
            match c.scope.visibility {
                CoverageVisibility::All => "all",
                CoverageVisibility::PublicOnly => "public_only",
            },
            match c.scope.generation {
                CoverageGeneration::LegacyUnusable => "legacy_unusable",
                CoverageGeneration::CommitsListingSearchV1 => "commits_listing_search_v1",
                CoverageGeneration::PullRequestsV1 => "pull_requests_v1",
                CoverageGeneration::IssuesV1 => "issues_v1",
            },
        ],
    )?;
    Ok(())
}


