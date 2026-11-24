impl Store {
    /// Returns coverage records associated with an account.
    pub fn coverage(&self, account: &str) -> Result<Vec<CoverageRecord>> {
        (|| {
            account_id(&self.conn, account)?
                .map_or_else(|| Ok(Vec::new()), |id| coverage_for(&self.conn, id))
        })()
        .map_err(classify)
    }

    /// Returns cached pull request synchronization state.
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

    /// Returns one synchronization cursor for an account.
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

    /// Creates a running sync record and returns its database identifier.
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

    /// Marks a sync record finished and stores its outcome and statistics.
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

    /// Returns the most recently started sync record for an account.
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

    /// Returns database size and record counts.
    pub fn stats(&self) -> Result<CacheStats> {
        (|| {
            let count = |sql: &str| -> Result<u64> {
                let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
                Ok(u64::try_from(n.max(0)).unwrap_or_default())
            };
            let mut file_size_bytes = 0;
            if let Some(path) = &self.path {
                file_size_bytes += std::fs::metadata(path).map_or(0, |m| m.len());
                let mut wal = path.as_os_str().to_os_string();
                wal.push("-wal");
                file_size_bytes += std::fs::metadata(PathBuf::from(wal)).map_or(0, |m| m.len());
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

    /// Returns a metadata value by key.
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

    /// Inserts or replaces a cache metadata value.
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


