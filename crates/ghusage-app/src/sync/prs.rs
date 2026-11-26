/// What every sync phase works with.
struct Shared<'a, 's, 'c> {
    client: &'a GitHubClient,
    store: &'a RefCell<&'s mut Store>,
    report: &'a RefCell<SyncReport>,
    subject: &'a Subject,
    opts: &'a SyncOptions,
    progress: &'a dyn Fn(SyncProgress),
    /// Pending confirmation of `subject` (see [`run`]).
    check: Option<ViewerCheck<'c>>,
    /// The sync record, created by the first write.
    run_id: &'a Cell<Option<i64>>,
}

impl Shared<'_, '_, '_> {
    /// Waits until the sync may write to the store: `subject` is
    /// confirmed and the sync record exists. Every write goes through here.
    async fn writable(&self) -> Result<()> {
        if !confirmed(self.check.as_ref(), self.subject).await {
            return Err(AppError::Other(UNCONFIRMED.into()));
        }
        if self.run_id.get().is_none() {
            let id = self.store.borrow_mut().begin_sync_run(
                &self.subject.login,
                Some(self.opts.period),
                self.opts.now,
            )?;
            self.run_id.set(Some(id));
        }
        Ok(())
    }
}

type SearchStatus = Vec<(Period, CoverageStatus, Option<String>)>;

/// Pull requests to fetch, in groups as they are found.
type PrQueue = futures::channel::mpsc::UnboundedSender<Vec<PrKey>>;

/// Finds pull requests updated in `ranges` and queues the ones to fetch.
/// Changed ones are queued as soon as the search that found them returns,
/// so fetching them does not wait for the remaining searches (one of which
/// may wait for a rate limit). Once every search is done, the repositories
/// found are sent to `pr_repos`, and pull requests whose last found version
/// is changed but was not queued yet, and stale open ones, are queued.
async fn search_prs(
    sh: &Shared<'_, '_, '_>,
    ranges: &[Period],
    states: &HashMap<String, (Option<Timestamp>, CoverageStatus)>,
    stale_open: Vec<(String, PrKey)>,
    pr_repos: tokio::sync::oneshot::Sender<Vec<RepositoryRef>>,
    queue: PrQueue,
    queued: &Cell<usize>,
) -> Result<SearchStatus> {
    let (client, subject, opts) = (sh.client, sh.subject, sh.opts);
    let unchanged = |r: &IssueRef| {
        !opts.full
            && matches!(
                states.get(&r.node_id),
                Some((Some(u), CoverageStatus::Complete)) if *u == r.updated_at
            )
    };
    let key = |r: &IssueRef| (RepositoryRef::new(r.owner.clone(), r.name.clone()), r.number);
    let send = |keys: Vec<PrKey>| {
        if !keys.is_empty() {
            queued.set(queued.get() + keys.len());
            // The receiver is only gone when fetching failed, which fails
            // the sync anyway.
            drop(queue.unbounded_send(keys));
        }
    };
    let mut refs: BTreeMap<String, IssueRef> = BTreeMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut search_status = Vec::new();
    for range in ranges {
        (sh.progress)(SyncProgress::Searching(DataKind::PullRequests));
        let mut statuses = Vec::new();
        let mut reasons = Vec::new();
        for q in [
            format!("involves:{}", subject.login),
            format!("reviewed-by:{}", subject.login),
        ] {
            (sh.progress)(SyncProgress::Detail(format!(
                "searching pull requests: {q} updated since {}",
                range.start.strftime("%Y-%m-%d")
            )));
            let found = client
                .search_issues(
                    &q,
                    IssueKind::PullRequest,
                    search_window(*range, opts),
                    opts.public_only,
                )
                .await?;
            statuses.push(found.status);
            reasons.extend(found.reason);
            let mut keys = Vec::new();
            for r in found.items {
                if !unchanged(&r) && seen.insert(r.node_id.clone()) {
                    keys.push(key(&r));
                }
                refs.insert(r.node_id.clone(), r);
            }
            send(keys);
        }
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        search_status.push((*range, CoverageStatus::combine(statuses), reason));
    }
    sh.report.borrow_mut().pull_requests_found = refs.len() as u64;
    let mut repos: Vec<RepositoryRef> = Vec::new();
    let mut seen_names = HashSet::new();
    for r in refs.values() {
        let repo = RepositoryRef::new(r.owner.clone(), r.name.clone());
        if seen_names.insert(repo.clone()) {
            repos.push(repo);
        }
    }
    drop(pr_repos.send(repos));
    let mut keys = Vec::new();
    for r in refs.values() {
        if seen.contains(&r.node_id) {
            continue;
        }
        if unchanged(r) {
            sh.report.borrow_mut().pull_requests_unchanged += 1;
        } else {
            seen.insert(r.node_id.clone());
            keys.push(key(r));
        }
    }
    for (id, key) in stale_open {
        if seen.insert(id) {
            keys.push(key);
        }
    }
    send(keys);
    Ok(search_status)
}

/// Fetches the pull requests queued by [`search_prs`] until it is done, and
/// returns the failures and the partially fetched pull requests.
async fn fetch_prs(
    sh: &Shared<'_, '_, '_>,
    queue: futures::channel::mpsc::UnboundedReceiver<Vec<PrKey>>,
    queued: &Cell<usize>,
) -> Result<(Vec<String>, Vec<String>)> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let mut pr_failures: Vec<String> = Vec::new();
    let mut pr_partial: Vec<String> = Vec::new();
    let mut done = 0;
    let concurrency = opts.concurrency.clamp(1, 16);
    let batches = queue.flat_map(|keys| {
        // Several pull requests per query, but not so many that fewer
        // queries run at once than are allowed.
        let batch = keys.len().div_ceil(concurrency).clamp(1, PULL_REQUEST_BATCH);
        futures::stream::iter(keys.chunks(batch).map(<[PrKey]>::to_vec).collect::<Vec<_>>())
    });
    let mut stream = batches.map(|keys| async move {
        let prs: Vec<(&str, &str, u64)> = keys
            .iter()
            .map(|(repo, num)| (repo.owner.as_str(), repo.name.as_str(), *num))
            .collect();
        let responses = client.fetch_pull_requests(&prs).await;
        keys.into_iter()
            .zip(responses)
            .map(|((repo, num), response)| (repo.owner, repo.name, num, response))
            .collect::<Vec<_>>()
    })
    .buffer_unordered(concurrency)
    .flat_map(futures::stream::iter);
    while let Some((o, n, num, res)) = stream.next().await {
        done += 1;
        progress(SyncProgress::Fetching {
            kind: DataKind::PullRequests,
            done,
            total: queued.get(),
        });
        progress(SyncProgress::Detail(format!("pull request {o}/{n}#{num}")));
        match res {
            Ok(bundle) => {
                if !bundle.status.is_complete() {
                    pr_partial.push(format!("{o}/{n}#{num}: {}", bundle.notes.join(", ")));
                }
                let batch = pr_batch(subject, bundle);
                sh.writable().await?;
                sh.store.borrow_mut().apply(&batch)?;
                sh.report.borrow_mut().pull_requests_fetched += 1;
            }
            Err(e @ ApiError::Unauthorized) => return Err(e.into()),
            Err(e) => {
                if matches!(e, ApiError::NotFound(_) | ApiError::Forbidden(_)) {
                    // Keep earlier data but show that access was lost.
                    sh.writable().await?;
                    sh.store
                        .borrow_mut()
                        .set_repository_accessible(&o, &n, false)?;
                }
                pr_failures.push(format!("{o}/{n}#{num}: {e}"));
            }
        }
    }
    drop(stream);
    progress(SyncProgress::Fetching {
        kind: DataKind::PullRequests,
        done,
        total: queued.get(),
    });
    Ok((pr_failures, pr_partial))
}

/// Records pull request coverage for the searched ranges once every pull
/// request found was fetched.
async fn record_pr_coverage(
    sh: &Shared<'_, '_, '_>,
    search_status: SearchStatus,
    pr_failures: Vec<String>,
    pr_partial: Vec<String>,
) -> Result<()> {
    let (subject, opts) = (sh.subject, sh.opts);
    for (range, status, reason) in search_status {
        let mut reasons: Vec<String> = reason.into_iter().collect();
        if !pr_failures.is_empty() {
            reasons.push(format!("{} pull request(s) failed", pr_failures.len()));
        }
        if !pr_partial.is_empty() {
            reasons.push(format!(
                "{} pull request(s) partially fetched",
                pr_partial.len()
            ));
        }
        let pr_coverage_status = if pr_failures.is_empty() && pr_partial.is_empty() {
            status
        } else {
            CoverageStatus::combine([status, CoverageStatus::Partial])
        };
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        let rec = record(
            DataKind::PullRequests,
            range,
            pr_coverage_status,
            reason,
            subject,
            opts,
        );
        sh.writable().await?;
        sh.store.borrow_mut().apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        sh.report.borrow_mut().coverage.push(rec);
    }
    let mut report = sh.report.borrow_mut();
    report.failures.extend(pr_failures);
    report.failures.extend(pr_partial);
    Ok(())
}
