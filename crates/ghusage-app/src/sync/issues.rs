/// Finds issues updated in `ranges` and fetches the changed ones. Like
/// pull requests, each range is searched from its start with no upper
/// bound (see [`search_prs`]). The
/// repositories of the issues found are sent to `issue_repos` as soon as
/// every range was searched.
async fn sync_issues(
    sh: &Shared<'_, '_, '_>,
    ranges: &[Period],
    issue_repos: tokio::sync::oneshot::Sender<Vec<RepositoryRef>>,
) -> Result<()> {
    let (client, subject, opts, progress) = (sh.client, sh.subject, sh.opts, sh.progress);
    let mut searched = Vec::new();
    for &range in ranges {
        progress(SyncProgress::Searching(DataKind::Issues));
        progress(SyncProgress::Detail(format!(
            "searching issues updated since {}",
            range.start.strftime("%Y-%m-%d")
        )));
        let found = client
            .search_issues(
                &format!("involves:{}", subject.login),
                IssueKind::Issue,
                range.start,
                opts.public_only,
            )
            .await?;
        searched.push((range, found));
    }
    let mut repos: Vec<RepositoryRef> = Vec::new();
    let mut seen_names = HashSet::new();
    for r in searched.iter().flat_map(|(_, found)| &found.items) {
        let repo = RepositoryRef::new(r.owner.clone(), r.name.clone());
        if seen_names.insert(repo.clone()) {
            repos.push(repo);
        }
    }
    drop(issue_repos.send(repos));
    for (range, found) in searched {
        sh.report.borrow_mut().issues_found += found.items.len() as u64;
        // An issue whose update time is unchanged since it was last fetched
        // completely has no new history.
        let mut changed = Vec::new();
        for r in found.items {
            let key = issue_cursor(&r.node_id);
            let seen = sh.store.borrow().cursor(&subject.login, &key)?;
            if !opts.full && seen.as_deref() == Some(r.updated_at.to_string().as_str()) {
                sh.report.borrow_mut().issues_unchanged += 1;
            } else {
                changed.push(r);
            }
        }
        let mut failures = Vec::new();
        let total = changed.len();
        progress(SyncProgress::Fetching {
            kind: DataKind::Issues,
            done: 0,
            total,
        });
        let mut stream = futures::stream::iter(changed.into_iter().map(|r| async move {
            let res = client.fetch_issue(&r.owner, &r.name, r.number).await;
            (r, res)
        }))
        .buffer_unordered(opts.concurrency.clamp(1, 16));
        let mut done = 0;
        while let Some((r, res)) = stream.next().await {
            done += 1;
            progress(SyncProgress::Fetching {
                kind: DataKind::Issues,
                done,
                total,
            });
            progress(SyncProgress::Detail(format!(
                "issue {}/{}#{}",
                r.owner, r.name, r.number
            )));
            match res {
                Ok(bundle) => {
                    if !bundle.status.is_complete() {
                        failures.push(format!(
                            "{}/{}#{}: issue history partially fetched",
                            r.owner, r.name, r.number
                        ));
                    }
                    let cursors = if bundle.status.is_complete() {
                        vec![(issue_cursor(&r.node_id), r.updated_at.to_string())]
                    } else {
                        Vec::new()
                    };
                    sh.writable().await?;
                    sh.store.borrow_mut().apply(&WriteBatch {
                        account: subject.login.clone(),
                        repositories: vec![bundle.repository],
                        issues: vec![bundle.issue],
                        events: bundle.events,
                        cursors,
                        ..Default::default()
                    })?;
                    sh.report.borrow_mut().issues_fetched += 1;
                }
                Err(e @ ApiError::Unauthorized) => return Err(e.into()),
                Err(e) => {
                    if matches!(e, ApiError::NotFound(_) | ApiError::Forbidden(_)) {
                        sh.writable().await?;
                        sh.store
                            .borrow_mut()
                            .set_repository_accessible(&r.owner, &r.name, false)?;
                    }
                    failures.push(format!("{}/{}#{}: {e}", r.owner, r.name, r.number));
                }
            }
        }
        drop(stream);
        let mut reasons: Vec<String> = found.reason.into_iter().collect();
        if !failures.is_empty() {
            reasons.push(format!("{} issue(s) failed", failures.len()));
        }
        let status = if failures.is_empty() {
            found.status
        } else {
            CoverageStatus::combine([found.status, CoverageStatus::Partial])
        };
        let reason = (!reasons.is_empty()).then(|| reasons.join("; "));
        let rec = record(DataKind::Issues, range, status, reason, subject, opts);
        sh.writable().await?;
        sh.store.borrow_mut().apply(&WriteBatch {
            account: subject.login.clone(),
            coverage: vec![rec.clone()],
            ..Default::default()
        })?;
        let mut report = sh.report.borrow_mut();
        report.coverage.push(rec);
        report.failures.extend(failures);
    }
    Ok(())
}

