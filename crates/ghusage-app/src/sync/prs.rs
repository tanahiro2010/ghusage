/// What every sync phase works with.
struct Shared<'a, 's> {
    client: &'a GitHubClient,
    store: &'a RefCell<&'s mut Store>,
    report: &'a RefCell<SyncReport>,
    subject: &'a Subject,
    opts: &'a SyncOptions,
    progress: &'a dyn Fn(SyncProgress),
}

type SearchStatus = Vec<(Period, CoverageStatus, Option<String>)>;

/// Finds pull requests updated in `ranges`.
async fn search_prs(
    sh: &Shared<'_, '_>,
    ranges: &[Period],
) -> Result<(BTreeMap<String, IssueRef>, SearchStatus)> {
    let (client, subject, opts) = (sh.client, sh.subject, sh.opts);
    let mut refs: BTreeMap<String, IssueRef> = BTreeMap::new();
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
            for r in found.items {
                refs.insert(r.node_id.clone(), r);
            }
        }
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        search_status.push((*range, CoverageStatus::combine(statuses), reason));
    }
    sh.report.borrow_mut().pull_requests_found = refs.len() as u64;
    Ok((refs, search_status))
}

/// Fetches changed and stale pull requests and records pull request
/// coverage for the searched ranges.
async fn fetch_prs(
    sh: &Shared<'_, '_>,
    refs: &BTreeMap<String, IssueRef>,
    search_status: SearchStatus,
    states: &HashMap<String, (Option<Timestamp>, CoverageStatus)>,
    stale_open: Vec<(String, PrKey)>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let mut to_fetch: Vec<PrKey> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in refs.values() {
        let unchanged = matches!(
            states.get(&r.node_id),
            Some((Some(u), CoverageStatus::Complete)) if *u == r.updated_at
        );
        if unchanged && !opts.full {
            sh.report.borrow_mut().pull_requests_unchanged += 1;
            continue;
        }
        if seen.insert(r.node_id.clone()) {
            to_fetch.push((RepositoryRef::new(r.owner.clone(), r.name.clone()), r.number));
        }
    }
    for (id, key) in stale_open {
        if seen.insert(id) {
            to_fetch.push(key);
        }
    }

    let mut pr_failures: Vec<String> = Vec::new();
    let mut pr_partial: Vec<String> = Vec::new();
    let total = to_fetch.len();
    let mut done = 0;
    progress(SyncProgress::Fetching {
        kind: DataKind::PullRequests,
        done,
        total,
    });
    let mut stream = futures::stream::iter(to_fetch.into_iter().map(|(repo, num)| async move {
        let res = client.fetch_pull_request(&repo.owner, &repo.name, num).await;
        (repo.owner, repo.name, num, res)
    }))
    .buffer_unordered(opts.concurrency.clamp(1, 16));
    while let Some((o, n, num, res)) = stream.next().await {
        done += 1;
        progress(SyncProgress::Fetching {
            kind: DataKind::PullRequests,
            done,
            total,
        });
        progress(SyncProgress::Detail(format!("pull request {o}/{n}#{num}")));
        match res {
            Ok(bundle) => {
                if !bundle.status.is_complete() {
                    pr_partial.push(format!("{o}/{n}#{num}: {}", bundle.notes.join(", ")));
                }
                let batch = pr_batch(subject, bundle);
                sh.store.borrow_mut().apply(&batch)?;
                sh.report.borrow_mut().pull_requests_fetched += 1;
            }
            Err(e @ ApiError::Unauthorized) => return Err(e.into()),
            Err(e) => {
                if matches!(e, ApiError::NotFound(_) | ApiError::Forbidden(_)) {
                    // Keep earlier data but show that access was lost.
                    sh.store
                        .borrow_mut()
                        .set_repository_accessible(&o, &n, false)?;
                }
                pr_failures.push(format!("{o}/{n}#{num}: {e}"));
            }
        }
    }
    drop(stream);
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
        let status = if pr_failures.is_empty() && pr_partial.is_empty() {
            status
        } else {
            CoverageStatus::combine([status, CoverageStatus::Partial])
        };
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        let rec = record(DataKind::PullRequests, range, status, reason, subject, opts);
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
