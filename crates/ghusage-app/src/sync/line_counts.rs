/// Fetches line counts for the subject's non-merge commits.
///
/// Missing counts are requested in batches of up to 100 commits per GraphQL
/// query. Existing counts are reused, and commits outside the period, merge
/// commits, and repositories hidden by `public_only` are skipped.
pub async fn fetch_line_counts(
    client: &GitHubClient,
    store: &mut Store,
    subject: &Subject,
    subject_is_viewer: bool,
    period: Period,
    public_only: bool,
    progress: &dyn Fn(SyncProgress),
) -> Result<LineCountSync> {
    let ds = store.load_dataset(subject, subject_is_viewer, Some(period))?;
    let mut report = LineCountSync::default();
    let mut seen: HashSet<&str> = HashSet::new();
    // CommitRef -> repository id, for storing the answers.
    let mut wanted: Vec<(CommitRef, String)> = Vec::new();
    for e in &ds.events {
        if e.event_type != EventType::CommitAuthored
            || !e.actor.is(subject)
            || e.meta.is_merge_commit == Some(true)
            || !(period.contains(e.occurred_at)
                || e.meta.author_date.is_some_and(|d| period.contains(d)))
            || !seen.insert(e.event_id.as_str())
        {
            continue;
        }
        let Some(repo) = ds.repositories.get(&e.repository_id) else {
            continue;
        };
        if public_only && !repo.visibility.is_public() {
            continue;
        }
        // Commits whose changes are counted through other commits are not
        // part of the totals, so their counts are not needed.
        if !ds.counts_lines(e.event_id.as_str()) {
            continue;
        }
        report.commits += 1;
        if ds.commit_stats.contains_key(e.event_id.as_str()) {
            report.already_known += 1;
            continue;
        }
        let Some(sha) = e.meta.sha.clone() else {
            report.unavailable += 1;
            continue;
        };
        wanted.push((
            CommitRef {
                owner: repo.owner.clone(),
                name: repo.name.clone(),
                sha: sha.to_ascii_lowercase(),
            },
            repo.id.clone(),
        ));
    }
    let total = wanted.len();
    let now = Timestamp::now();
    let mut done = 0;
    for chunk in wanted.chunks(500) {
        progress(SyncProgress::Fetching {
            kind: DataKind::Commits,
            done,
            total,
        });
        progress(SyncProgress::Detail(format!(
            "line counts of {done}/{total} commits"
        )));
        let refs: Vec<CommitRef> = chunk.iter().map(|(c, _)| c.clone()).collect();
        let repo_of: HashMap<&CommitRef, &str> =
            chunk.iter().map(|(c, id)| (c, id.as_str())).collect();
        let answers = client.commit_line_counts(&refs).await?;
        let mut batch = WriteBatch {
            account: subject.login.clone(),
            ..Default::default()
        };
        for (c, counts) in answers {
            match (counts, repo_of.get(&c)) {
                (Some(counts), Some(repo_id)) => {
                    report.fetched += 1;
                    batch.commit_stats.push(CommitStats {
                        repository_id: (*repo_id).to_string(),
                        sha: c.sha,
                        counts,
                        observed_at: now,
                    });
                }
                _ => report.unavailable += 1,
            }
        }
        store.apply(&batch)?;
        done += chunk.len();
    }
    progress(SyncProgress::Fetching {
        kind: DataKind::Commits,
        done,
        total,
    });
    Ok(report)
}
