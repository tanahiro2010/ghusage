#[derive(Debug)]
struct FetchedPr {
    repository: ghusage_core::model::Repository,
    pr: GqlPullRequest,
    pr_id: String,
    created_at: Timestamp,
    pr_url: String,
    tracker: Tracker,
    head_sha: Option<String>,
    head_commit: Option<HeadCommit>,
    commits: Vec<Value>,
    reviews: Vec<Value>,
    threads: Vec<(ThreadNode, Vec<Value>, Option<u64>)>,
    comments: Vec<Value>,
    timeline: Vec<Value>,
    checks: ChecksResult,
}

async fn collect_graphql_pages(
    t: &Transport,
    pr_id: &str,
    pr: &mut GqlPullRequest,
    tracker: &mut Tracker,
) -> Result<(
    Vec<Value>,
    Vec<Value>,
    Vec<(ThreadNode, Vec<Value>, Option<u64>)>,
    Vec<Value>,
    Vec<Value>,
), ApiError> {
    let commits = collect_connection(t, pr_id, PrConnection::Commits, pr.commits.take(), tracker).await?;
    let reviews = collect_connection(t, pr_id, PrConnection::Reviews, pr.reviews.take(), tracker).await?;
    let thread_values = collect_connection(
        t,
        pr_id,
        PrConnection::Threads,
        pr.review_threads.take(),
        tracker,
    )
    .await?;
    let (threads, bad_threads) = collect_threads(t, thread_values, tracker).await?;
    tracker.skipped(Collection::ReviewComments, "review threads", bad_threads);
    let comments = collect_connection(t, pr_id, PrConnection::Comments, pr.comments.take(), tracker).await?;
    let timeline = collect_connection(t, pr_id, PrConnection::Timeline, pr.timeline_items.take(), tracker).await?;
    Ok((commits, reviews, threads, comments, timeline))
}

async fn collect_threads(
    t: &Transport,
    values: Vec<Value>,
    tracker: &mut Tracker,
) -> Result<(Vec<(ThreadNode, Vec<Value>, Option<u64>)>, usize), ApiError> {
    let mut threads = Vec::new();
    let mut bad = 0;
    for v in values {
        match serde_json::from_value::<ThreadNode>(v) {
            Ok(mut th) if th.id.is_some() => {
                let id = th.id.clone().unwrap_or_default();
                let (comments, total) =
                    collect_thread_comments(t, &id, th.comments.take(), tracker).await?;
                threads.push((th, comments, total));
            }
            _ => bad += 1,
        }
    }
    Ok((threads, bad))
}

async fn collect_pr_checks(
    t: &Transport,
    owner: &str,
    name: &str,
    head_sha: Option<&str>,
    checks_from_graphql: Option<GraphqlChecks>,
    tracker: &mut Tracker,
) -> Result<ChecksResult, ApiError> {
    if let Some(sha) = head_sha { fetch_checks(t, owner, name, sha, checks_from_graphql, tracker).await } else {
        tracker.partial(Collection::Checks, "head commit unknown; checks not fetched");
        Ok(ChecksResult::default())
    }
}

async fn fetch_input(
    t: &Transport,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<FetchedPr, ApiError> {
    let what = format!("pull request {owner}/{name}#{number}");
    let resp = t
        .graphql(
            &queries::pull_request(),
            json!({ "owner": owner, "name": name, "number": number }),
        )
        .await?;
    let repo_value = resp
        .data
        .as_ref()
        .and_then(|d| d.get("repository"))
        .filter(|v| !v.is_null());
    let Some(repo_value) = repo_value else {
        return Err(gql::failure(
            &resp.errors,
            &format!("repository {owner}/{name}"),
        ));
    };
    let pr_value = repo_value.get("pullRequest").filter(|v| !v.is_null());
    let Some(pr_value) = pr_value else {
        return Err(gql::failure(&resp.errors, &what));
    };
    let repo: GqlRepository = gql::from_value(Some(repo_value));
    let repository = repo.to_repository()?;
    let mut pr: GqlPullRequest = serde_json::from_value(pr_value.clone())
        .map_err(|e| ApiError::InvalidResponse(format!("{what}: {e}")))?;
    let missing = |f: &str| ApiError::InvalidResponse(format!("{what} is missing `{f}`"));
    let pr_id = pr.id.clone().ok_or_else(|| missing("id"))?;
    let created_at = time(&pr.created_at).ok_or_else(|| missing("createdAt"))?;
    let pr_url = pr.url.clone().ok_or_else(|| missing("url"))?;

    let mut tracker = Tracker::default();
    mark_errors(&resp.errors, &mut tracker);

    let mut checks_tracker = Tracker::default();
    let head_sha = pr.head_ref_oid.clone();
    let head_commit = pr
        .head_commit
        .as_ref()
        .map(|c| c.parse_nodes::<HeadCommitNode>().0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.commit)
        .find(|c| head_sha.is_none() || c.oid == head_sha);
    let checks_from_graphql = head_commit
        .as_ref()
        .filter(|_| head_sha.is_some())
        .and_then(|c| graphql_checks(c, &resp.errors));

    let graphql_pages = collect_graphql_pages(t, &pr_id, &mut pr, &mut tracker);
    let checks = collect_pr_checks(
        t,
        owner,
        name,
        head_sha.as_deref(),
        checks_from_graphql,
        &mut checks_tracker,
    );

    let (pages, checks) = tokio::join!(graphql_pages, checks);
    let (commits, reviews, threads, comments, timeline) = pages?;
    let checks = checks?;
    tracker.partial.extend(checks_tracker.partial);
    for n in checks_tracker.notes {
        tracker.note(n);
    }

    Ok(FetchedPr {
        repository,
        pr,
        pr_id,
        created_at,
        pr_url,
        tracker,
        head_sha,
        head_commit,
        commits,
        reviews,
        threads,
        comments,
        timeline,
        checks,
    })
}

pub(crate) async fn fetch(
    t: &Transport,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<PrBundle, ApiError> {
    let FetchedPr {
        repository,
        pr,
        pr_id,
        created_at,
        pr_url,
        mut tracker,
        head_sha,
        head_commit,
        commits,
        reviews,
        threads,
        comments,
        timeline,
        checks,
    } = fetch_input(t, owner, name, number).await?;

    let now = Timestamp::now();
    let merged_at = time(&pr.merged_at);
    let mut b = Builder {
        repo_id: repository.id.clone(),
        pr_id: pr_id.clone(),
        now,
        events: Vec::new(),
    };

    // Opened.
    b.push(
        EventType::PrOpened,
        EventId::pr_created(&pr_id),
        actor_of(&pr.author),
        pr_id.clone(),
        created_at,
        Some(pr_url.clone()),
        EventMeta::default(),
        Source::Graphql,
    );

    // Commits.
    let (current_commit_shas, commit_stats) =
        append_commits(commits, &pr_id, &mut b, &mut tracker);

    // Timeline.
    let dismissed_previous = append_timeline(
        timeline,
        &pr_id,
        &pr_url,
        merged_at,
        pr.merged_by.as_ref(),
        &mut b,
        &mut tracker,
    );

    let review_comments = append_reviews(
        reviews,
        &dismissed_previous,
        &mut b,
        &mut tracker,
    );

    let out_threads = append_review_threads(
        threads,
        review_comments,
        &pr_id,
        &mut b,
        &mut tracker,
    );

    append_comments(comments, &mut b, &mut tracker);

    append_checks(checks, &head_sha, &mut b, &mut tracker);

    // Latest CI state of the head commit.
    let latest_ci = head_commit
        .and_then(|c| c.status_check_rollup)
        .and_then(|r| r.state)
        .map(|s| CheckOutcome::from_github(None, Some(&s)));

    Ok(finalize_pr(
        repository,
        pr,
        pr_id,
        pr_url,
        number,
        created_at,
        head_sha,
        latest_ci,
        now,
        tracker,
        b.events,
        out_threads,
        current_commit_shas,
        commit_stats,
    ))
}


fn finalize_pr(
    repository: ghusage_core::model::Repository,
    pr: GqlPullRequest,
    pr_id: String,
    pr_url: String,
    number: u64,
    created_at: Timestamp,
    head_sha: Option<String>,
    latest_ci: Option<CheckOutcome>,
    now: Timestamp,
    tracker: Tracker,
    raw_events: Vec<ActivityEvent>,
    threads: Vec<ReviewThread>,
    current_commit_shas: Vec<String>,
    commit_stats: Vec<(String, LineCounts)>,
) -> PrBundle {
    let complete_collections: Vec<Collection> = Collection::ALL
        .iter()
        .copied()
        .filter(|c| !tracker.partial.contains(c))
        .collect();
    let status = if complete_collections.len() == Collection::ALL.len() {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    let mut events = dedupe(raw_events);
    for e in &mut events {
        e.coverage = match Collection::of(e.event_type) {
            Some(c) if tracker.partial.contains(&c) => CoverageStatus::Partial,
            _ => CoverageStatus::Complete,
        };
    }
    events.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
    let merged_at = time(&pr.merged_at);
    let state = pr.state.as_deref().and_then(PrState::parse).unwrap_or_else(|| {
        if merged_at.is_some() {
            PrState::Merged
        } else if pr.closed_at.is_some() {
            PrState::Closed
        } else {
            PrState::Open
        }
    });
    let pull_request = PullRequest {
        id: pr_id,
        repository_id: repository.id.clone(),
        number: pr.number.unwrap_or(number),
        title: pr.title.clone().unwrap_or_default(),
        url: pr_url,
        author: actor_of(&pr.author),
        state,
        is_draft: pr.is_draft.unwrap_or(false),
        created_at,
        updated_at: time(&pr.updated_at),
        closed_at: time(&pr.closed_at),
        merged_at,
        merged_by: pr.merged_by.as_ref().map(GqlActor::actor),
        head_sha,
        latest_ci,
        coverage: status,
        synced_at: Some(now),
    };
    let diff = match (pr.additions, pr.deletions) {
        (Some(additions), Some(deletions)) => Some(PrDiff {
            additions,
            deletions,
            changed_files: pr.changed_files.unwrap_or(0),
            observed_at: now,
        }),
        _ => None,
    };
    PrBundle {
        repository,
        pull_request,
        events,
        threads,
        current_commit_shas,
        commit_stats,
        diff,
        merge_commit_parents: pr
            .merge_commit
            .as_ref()
            .and_then(|m| m.parents.as_ref())
            .and_then(|p| p.total_count),
        complete_collections,
        status,
        notes: tracker.notes,
    }
}

fn append_commits(
    commits: Vec<Value>,
    pr_id: &str,
    b: &mut Builder,
    tracker: &mut Tracker,
) -> (Vec<String>, Vec<(String, LineCounts)>) {
    let (commit_nodes, bad) = parse_values::<CommitNode>(commits);
    tracker.skipped(Collection::Commits, "commits", bad);
    let mut current_commit_shas = Vec::new();
    let mut commit_stats = Vec::new();
    let mut bad = 0;
    for node in commit_nodes {
        let Some(c) = node.commit else {
            bad += 1;
            continue;
        };
        let (Some(oid), Some(at)) = (c.oid.clone(), time(&c.committed_date)) else {
            bad += 1;
            continue;
        };
        current_commit_shas.push(oid.clone());
        if let (Some(additions), Some(deletions)) = (c.additions, c.deletions) {
            commit_stats.push((
                oid.clone(),
                LineCounts {
                    additions,
                    deletions,
                },
            ));
        }
        let meta = EventMeta {
            sha: Some(oid.clone()),
            author_date: time(&c.authored_date),
            is_merge_commit: c
                .parents
                .as_ref()
                .and_then(|p| p.total_count)
                .map(|n| n > 1),
            committer: GitActor::user_actor(&c.committer),
            time_basis: Some("commit_date".into()),
            message_headline: c
                .message_headline
                .clone()
                .map(|h| crate::http::truncate(&h, 300)),
            ..EventMeta::default()
        };
        b.push(
            EventType::PrCommit,
            EventId::pr_commit(pr_id, &oid),
            GitActor::user_actor(&c.author).unwrap_or_else(Actor::unknown),
            oid,
            at,
            node.url.or(c.url),
            meta,
            Source::Graphql,
        );
    }
    tracker.skipped(Collection::Commits, "commits", bad);
    (current_commit_shas, commit_stats)
}

fn append_timeline(
    timeline: Vec<Value>,
    pr_id: &str,
    pr_url: &str,
    merged_at: Option<Timestamp>,
    merged_by: Option<&GqlActor>,
    b: &mut Builder,
    tracker: &mut Tracker,
) -> HashMap<String, String> {
    let items_source = timeline;
    let (items, bad) = parse_values::<TimelineNode>(items_source);
    tracker.skipped(Collection::Timeline, "timeline items", bad);
    let mut bad = 0;
    let mut saw_merged = false;
    let mut unsupported: HashMap<String, usize> = HashMap::new();
    let mut dismissed_previous: HashMap<String, String> = HashMap::new();
    for item in items {
        let (Some(typename), Some(id), Some(at)) = (
            item.typename.clone(),
            item.id.clone(),
            time(&item.created_at),
        ) else {
            bad += 1;
            continue;
        };
        let actor = actor_of(&item.actor);
        let mut meta = EventMeta::default();
        let ty = match typename.as_str() {
            "MergedEvent" => {
                saw_merged = true;
                b.push(
                    EventType::PrMerged,
                    EventId::pr_merged(pr_id),
                    actor,
                    id,
                    at,
                    item.url,
                    meta,
                    Source::Graphql,
                );
                continue;
            }
            "ClosedEvent" => {
                meta.via_merge = Some(merged_at.is_some_and(|m| {
                    at.duration_since(m).as_secs_f64().abs() <= MERGE_CLOSE_WINDOW_SECS as f64
                }));
                EventType::PrClosed
            }
            "ReopenedEvent" => EventType::PrReopened,
            "ConvertToDraftEvent" => EventType::PrConvertedToDraft,
            "ReadyForReviewEvent" => EventType::PrReadyForReview,
            "HeadRefForcePushedEvent" => {
                let oid = |o: &Option<Oid>| {
                    o.as_ref()
                        .and_then(|o| o.oid.clone())
                        .unwrap_or_else(|| "unknown".into())
                };
                meta.target = Some(format!(
                    "{}->{}",
                    oid(&item.before_commit),
                    oid(&item.after_commit)
                ));
                EventType::PrHeadForcePushed
            }
            "ReviewRequestedEvent" => {
                meta.target = Named::label(&item.requested_reviewer);
                EventType::ReviewRequested
            }
            "ReviewRequestRemovedEvent" => {
                meta.target = Named::label(&item.requested_reviewer);
                EventType::ReviewRequestRemoved
            }
            "AssignedEvent" => {
                meta.target = Named::label(&item.assignee);
                EventType::Assigned
            }
            "UnassignedEvent" => {
                meta.target = Named::label(&item.assignee);
                EventType::Unassigned
            }
            "LabeledEvent" => {
                meta.target = Named::label(&item.label);
                EventType::Labeled
            }
            "UnlabeledEvent" => {
                meta.target = Named::label(&item.label);
                EventType::Unlabeled
            }
            "MilestonedEvent" => {
                meta.target.clone_from(&item.milestone_title);
                EventType::Milestoned
            }
            "DemilestonedEvent" => {
                meta.target.clone_from(&item.milestone_title);
                EventType::Demilestoned
            }
            "RenamedTitleEvent" => {
                meta.previous_title.clone_from(&item.previous_title);
                meta.target.clone_from(&item.current_title);
                EventType::Renamed
            }
            "ReviewDismissedEvent" => {
                let review_id = item.review.as_ref().and_then(|r| r.id.clone());
                if let (Some(rid), Some(prev)) = (&review_id, &item.previous_review_state) {
                    dismissed_previous.insert(rid.clone(), prev.clone());
                }
                meta.review_id = review_id;
                meta.review_state = item
                    .previous_review_state
                    .as_deref()
                    .and_then(ReviewState::parse);
                EventType::ReviewDismissed
            }
            other => {
                *unsupported.entry(other.to_string()).or_default() += 1;
                continue;
            }
        };
        b.push(
            ty,
            EventId::timeline(&id),
            actor,
            id,
            at,
            item.url,
            meta,
            Source::Graphql,
        );
    }
    finish_timeline(
        pr_id,
        pr_url,
        merged_at,
        merged_by,
        b,
        tracker,
        bad,
        saw_merged,
        unsupported,
    );
    dismissed_previous
}

fn finish_timeline(
    pr_id: &str,
    pr_url: &str,
    merged_at: Option<Timestamp>,
    merged_by: Option<&GqlActor>,
    b: &mut Builder,
    tracker: &mut Tracker,
    bad: usize,
    saw_merged: bool,
    unsupported: HashMap<String, usize>,
) {
    tracker.skipped(Collection::Timeline, "timeline items", bad);
    let mut unsupported: Vec<_> = unsupported.into_iter().collect();
    unsupported.sort();
    for (ty, n) in unsupported {
        tracker.note(format!("skipped {n} timeline item(s) of unsupported type {ty}"));
    }
    if !saw_merged && let Some(m) = merged_at {
        b.push(
            EventType::PrMerged,
            EventId::pr_merged(pr_id),
            merged_by.map_or_else(Actor::unknown, GqlActor::actor),
            pr_id.to_owned(),
            m,
            Some(pr_url.to_owned()),
            EventMeta::default(),
            Source::Graphql,
        );
        tracker.note("merge recorded from the pull request's merge time (no merge timeline item)");
    }
}

fn append_reviews(
    reviews: Vec<Value>,
    dismissed_previous: &HashMap<String, String>,
    b: &mut Builder,
    tracker: &mut Tracker,
) -> Vec<(String, Value)> {
    let (review_nodes, bad) = parse_values::<ReviewNode>(reviews);
    tracker.skipped(Collection::Reviews, "reviews", bad);
    let mut bad = 0;
    let mut pending = 0;
    let mut review_comments: Vec<(String, Value)> = Vec::new();
    for r in review_nodes {
        let Some(id) = r.id.clone() else {
            bad += 1;
            continue;
        };
        let state_str = r.state.clone().unwrap_or_default();
        if state_str == "PENDING" {
            pending += 1;
            continue;
        }
        let Some(at) = time(&r.submitted_at) else {
            // Not submitted: a draft review.
            pending += 1;
            continue;
        };
        let mut state = ReviewState::parse(&state_str);
        if state == Some(ReviewState::Dismissed)
            && let Some(prev) = dismissed_previous
                .get(&id)
                .and_then(|p| ReviewState::parse(p))
                .filter(|p| !matches!(p, ReviewState::Dismissed | ReviewState::Pending))
        {
            state = Some(prev);
        }
        let actor = actor_of(&r.author);
        b.push(
            EventType::ReviewSubmitted,
            EventId::review(&id),
            actor.clone(),
            id.clone(),
            at,
            r.url.clone(),
            EventMeta {
                review_state: state,
                review_id: Some(id.clone()),
                ..EventMeta::default()
            },
            Source::Graphql,
        );
        if r.body.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            b.push(
                EventType::ReviewSummary,
                EventId::review_summary(&id),
                actor,
                id.clone(),
                at,
                r.url.clone(),
                EventMeta {
                    review_state: state,
                    review_id: Some(id.clone()),
                    ..EventMeta::default()
                },
                Source::Graphql,
            );
        }
        if let Some(c) = &r.comments {
            for v in c.nodes.iter().flatten() {
                review_comments.push((id.clone(), v.clone()));
            }
        }
    }
    tracker.skipped(Collection::Reviews, "reviews", bad);
    if pending > 0 {
        tracker.note(format!("{pending} pending (unsubmitted) review(s) ignored"));
    }
    review_comments
}

fn append_review_threads(
    threads: Vec<(ThreadNode, Vec<Value>, Option<u64>)>,
    review_comments: Vec<(String, Value)>,
    pr_id: &str,
    b: &mut Builder,
    tracker: &mut Tracker,
) -> Vec<ReviewThread> {
    let mut out_threads: Vec<ReviewThread> = Vec::new();
    let mut bad = 0;
    for (th, comment_values, total) in threads {
        let total: Option<u64> = total;
        let thread_id = th.id.clone().unwrap_or_default();
        let (comment_nodes, bad_c) = parse_values::<CommentNode>(comment_values);
        bad += bad_c;
        let mut first_author = None;
        let mut fetched = 0u32;
        let mut pending = 0u32;
        for c in comment_nodes {
            fetched += 1;
            match inline_comment(b, &c, Some(&thread_id), None) {
                CommentOutcome::Added => {
                    if first_author.is_none() {
                        first_author = Some(actor_of(&c.author));
                    }
                }
                // Unsubmitted comments are visible only to their author's
                // token and are not part of the thread yet.
                CommentOutcome::Pending => pending += 1,
                CommentOutcome::Bad => bad += 1,
            }
        }
        let fetched_all = total.is_none_or(|n| n <= u64::from(fetched));
        if fetched_all && pending == fetched {
            // A thread made only of unsubmitted comments does not exist for
            // anyone else.
            continue;
        }
        out_threads.push(ReviewThread {
            id: thread_id,
            pr_id: pr_id.to_owned(),
            is_resolved: th.is_resolved.unwrap_or(false),
            is_outdated: th.is_outdated.unwrap_or(false),
            resolved_by: th.resolved_by.as_ref().map(GqlActor::actor),
            path: th.path,
            comment_count: total
                .map_or(fetched, |n| u32::try_from(n).unwrap_or(u32::MAX))
                .saturating_sub(pending),
            first_comment_author: first_author,
        });
    }
    for (review_id, v) in review_comments {
        match serde_json::from_value::<CommentNode>(v) {
            Ok(c) => {
                if matches!(inline_comment(b, &c, None, Some(&review_id)), CommentOutcome::Bad) {
                    bad += 1;
                }
            }
            Err(_) => bad += 1,
        }
    }
    tracker.skipped(Collection::ReviewComments, "inline comments", bad);
    out_threads
}

fn append_comments(comments: Vec<Value>, b: &mut Builder, tracker: &mut Tracker) {
    let (comment_nodes, bad) = parse_values::<CommentNode>(comments);
    tracker.skipped(Collection::Comments, "comments", bad);
    let mut bad = 0;
    for c in comment_nodes {
        let (Some(id), Some(at)) = (c.id.clone(), time(&c.created_at)) else {
            bad += 1;
            continue;
        };
        b.push(
            EventType::ConversationComment,
            EventId::comment(&id),
            actor_of(&c.author),
            id,
            at,
            c.url.clone(),
            EventMeta {
                edited_at: time(&c.last_edited_at),
                ..EventMeta::default()
            },
            Source::Graphql,
        );
    }
    tracker.skipped(Collection::Comments, "comments", bad);
}

fn append_checks(
    checks: ChecksResult,
    head_sha: &Option<String>,
    b: &mut Builder,
    tracker: &mut Tracker,
) {
    tracker.skipped(Collection::Checks, "check runs", checks.bad_runs);
    let mut bad = 0;
    for r in checks.runs {
        let Some(node_id) = r.node_id.clone() else {
            bad += 1;
            continue;
        };
        let at = r
            .completed_at
            .as_deref()
            .and_then(parse_time)
            .or_else(|| r.started_at.as_deref().and_then(parse_time));
        // Queued runs have no time yet; they are not events.
        let Some(at) = at else { continue };
        let actor = r.app.map_or_else(Actor::unknown, |(id, slug)| {
            Actor::new(id, slug, Some("Bot"))
        });
        b.push(
            EventType::CheckRun,
            EventId::check_run(&node_id),
            actor,
            node_id,
            at,
            r.url,
            EventMeta {
                sha: head_sha.clone(),
                check_name: r.name,
                check_outcome: Some(CheckOutcome::from_github(
                    r.status.as_deref(),
                    r.conclusion.as_deref(),
                )),
                check_suite_id: r.suite_id.map(|id| id.to_string()),
                ..EventMeta::default()
            },
            r.source,
        );
    }
    let (statuses, bad_s) = parse_values::<RestStatus>(checks.statuses);
    bad += bad_s;
    for s in statuses {
        let (Some(node_id), Some(at)) = (
            s.node_id.clone(),
            s.created_at.as_deref().and_then(parse_time),
        ) else {
            bad += 1;
            continue;
        };
        b.push(
            EventType::CommitStatus,
            EventId::commit_status(&node_id),
            s.creator
                .as_ref()
                .map_or_else(Actor::unknown, RestUser::actor),
            node_id,
            at,
            None,
            EventMeta {
                sha: head_sha.clone(),
                check_name: s.context.clone(),
                check_outcome: Some(CheckOutcome::from_github(None, s.state.as_deref())),
                ..EventMeta::default()
            },
            Source::Rest,
        );
    }
    tracker.skipped(Collection::Checks, "checks", bad);
}
