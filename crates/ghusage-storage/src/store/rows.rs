impl Store {
    /// The commits of `account` with committer date in `period`, from every
    /// source, without the commits listed inside pull requests.
    pub fn commits_in(&self, account: &str, period: Period) -> Result<Vec<StoredCommit>> {
        (|| {
            let Some(account_id) = account_id(&self.conn, account)? else {
                return Ok(Vec::new());
            };
            // Coarse filter on whole seconds, then the exact half-open check.
            let mut stmt = self.conn.prepare_cached(
                "SELECT e.event_id, e.repository_id, e.source, e.occurred_at, e.occurred_at_ns \
                 FROM activity_events e \
                 JOIN account_events ae ON ae.event_id = e.event_id \
                 WHERE ae.account_id = ?1 AND e.event_type = ?2 \
                 AND e.parent_pr_id IS NULL AND e.occurred_at BETWEEN ?3 AND ?4 \
                 ORDER BY e.event_id",
            )?;
            let mut rows = stmt.query(params![
                account_id,
                EventType::CommitAuthored.as_str(),
                secs(period.start),
                secs(period.end),
            ])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let at = time(r.get("occurred_at")?, r.get("occurred_at_ns")?)?;
                if !period.contains(at) {
                    continue;
                }
                let source: String = r.get("source")?;
                // `commit:{repository}:{sha}`, with the SHA in lower case.
                let event_id: String = r.get("event_id")?;
                let Some((_, sha)) = event_id.rsplit_once(':') else {
                    return Err(StorageError::Data(format!(
                        "malformed commit event id {event_id:?}"
                    )));
                };
                out.push(StoredCommit {
                    repository_id: r.get("repository_id")?,
                    sha: sha.to_owned(),
                    source: Source::parse(&source).ok_or_else(|| {
                        StorageError::Data(format!("unknown event source {source:?}"))
                    })?,
                });
            }
            Ok(out)
        })()
        .map_err(classify)
    }

    /// Removes the given commits (repository id and SHA) from `account` when
    /// they were found by commit search or a default-branch listing; commits
    /// known through pull requests stay. A repository left with no commits,
    /// pull requests or issues of the account is removed from the account
    /// too. Stored rows stay for other accounts. Returns the number of
    /// commits removed.
    pub fn remove_listed_commits(
        &mut self,
        account: &str,
        commits: &[(String, String)],
    ) -> Result<usize> {
        (|| {
            let tx = self.conn.transaction()?;
            let Some(account_id) = account_id(&tx, account)? else {
                return Ok(0);
            };
            let mut removed = 0;
            let mut repos: Vec<&str> = Vec::new();
            for (repo_id, sha) in commits {
                removed += tx.execute(
                    "DELETE FROM account_events WHERE account_id = ?1 AND event_id = ?2 \
                     AND event_id IN (SELECT event_id FROM activity_events \
                         WHERE event_id = ?2 AND source IN (?3, ?4))",
                    params![
                        account_id,
                        EventId::commit(repo_id, sha).as_str(),
                        Source::Search.as_str(),
                        Source::Rest.as_str(),
                    ],
                )?;
                repos.push(repo_id);
            }
            repos.sort_unstable();
            repos.dedup();
            for repo_id in repos {
                tx.execute(
                    "DELETE FROM account_repositories \
                     WHERE account_id = ?1 AND repository_id = ?2 \
                     AND NOT EXISTS (SELECT 1 FROM account_events ae \
                         JOIN activity_events e ON e.event_id = ae.event_id \
                         WHERE ae.account_id = ?1 AND e.repository_id = ?2) \
                     AND NOT EXISTS (SELECT 1 FROM account_pull_requests ap \
                         JOIN pull_requests p ON p.id = ap.pr_id \
                         WHERE ap.account_id = ?1 AND p.repository_id = ?2) \
                     AND NOT EXISTS (SELECT 1 FROM account_issues ai \
                         JOIN issues i ON i.id = ai.issue_id \
                         WHERE ai.account_id = ?1 AND i.repository_id = ?2)",
                    params![account_id, repo_id],
                )?;
            }
            tx.commit()?;
            Ok(removed)
        })()
        .map_err(classify)
    }

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
                scope: CoverageScope {
                    visibility: match r.get::<_, String>("visibility")?.as_str() {
                        "public_only" => CoverageVisibility::PublicOnly,
                        _ => CoverageVisibility::All,
                    },
                    generation: match r.get::<_, String>("generation")?.as_str() {
                        "commits_listing_search_v1" => CoverageGeneration::CommitsListingSearchV1,
                        "pull_requests_v1" => CoverageGeneration::PullRequestsV1,
                        "issues_v1" => CoverageGeneration::IssuesV1,
                        _ => CoverageGeneration::LegacyUnusable,
                    },
                },
                description: r.get("scope")?,
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



