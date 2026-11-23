pub fn report_csv(r: &Report, kind: ReportKind) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    let overall = r.overall_coverage().as_str();
    match kind {
        ReportKind::Overview | ReportKind::Breakdown(_) => write_period_csv(&mut w, r),
        ReportKind::Repositories => write_repository_csv(&mut w, r, overall),
        ReportKind::PullRequests { .. } => write_pull_request_csv(&mut w, r, overall),
        ReportKind::Reviews => write_review_csv(&mut w, r, overall),
        ReportKind::Comments => write_comment_csv(&mut w, r, overall),
        ReportKind::Issues => write_issue_csv(&mut w, r, overall),
    }
    finish(w)
}

fn write_period_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report) {
    w.write_record([
        "period", "start", "end", "commits", "merge_commits", "prs_created", "prs_merged",
        "merges_performed", "prs_closed_without_merge", "reviews_submitted", "reviewed_prs",
        "reviews_received", "comments_written", "comments_received", "review_responses",
        "pr_commits", "issues_opened", "issues_closed", "active_prs", "pr_internal_events",
        "ci_passed", "ci_failed", "additions", "deletions", "line_count_commits",
        "line_count_commits_known", "line_count_commits_excluded", "line_count_coverage", "coverage",
    ])
    .ok();
    for b in &r.activity_breakdown {
        let s = &b.summary;
        let l = line_cols(&s.diff_stats);
        let cov = bucket_coverage(r, b.bucket.start, b.bucket.end);
        w.write_record([
            b.bucket.label.clone(), b.bucket.start.to_string(), b.bucket.end.to_string(),
            s.commits.to_string(), s.merge_commits.to_string(), s.prs_created.to_string(),
            s.prs_merged.to_string(), s.merges_performed.to_string(), s.prs_closed_without_merge.to_string(),
            s.reviews_submitted.total.to_string(), s.reviewed_prs.to_string(), s.reviews_received.total.to_string(),
            s.comments_written.total().to_string(), s.comments_received.total().to_string(), s.review_responses.to_string(),
            s.pr_commits.to_string(), s.issues_opened.to_string(), s.issues_closed.to_string(),
            s.active_prs.to_string(), s.pr_internal_events.to_string(), s.ci.passed.to_string(), s.ci.failed.to_string(),
            l[0].clone(), l[1].clone(), l[2].clone(), l[3].clone(), l[4].clone(), l[5].clone(), cov.as_str().to_string(),
        ])
        .ok();
    }
}

fn write_repository_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report, overall: &str) {
    w.write_record([
        "repository", "visibility", "owned", "accessible", "commits", "prs_created", "prs_merged",
        "reviews_submitted", "comments_written", "issues_opened", "active_prs", "pr_internal_events",
        "additions", "deletions", "line_count_commits", "line_count_commits_known",
        "line_count_commits_excluded", "line_count_coverage", "last_activity", "period_coverage",
    ])
    .ok();
    for x in &r.repositories {
        let s = &x.summary;
        let l = line_cols(&s.diff_stats);
        w.write_record([
            x.full_name.clone(), x.visibility.as_str().to_string(), x.owned.to_string(), x.accessible.to_string(),
            s.commits.to_string(), s.prs_created.to_string(), s.prs_merged.to_string(), s.reviews_submitted.total.to_string(),
            s.comments_written.total().to_string(), s.issues_opened.to_string(), s.active_prs.to_string(), s.pr_internal_events.to_string(),
            l[0].clone(), l[1].clone(), l[2].clone(), l[3].clone(), l[4].clone(), l[5].clone(), ts(s.last_activity), overall.to_string(),
        ])
        .ok();
    }
}

fn write_pull_request_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report, overall: &str) {
    w.write_record([
        "repository", "number", "title", "state", "draft", "author", "created_at", "merged_at", "events",
        "events_by_you", "commits", "reviews", "comments", "participants", "discussion_participants", "threads",
        "last_activity", "latest_ci", "url", "period_coverage",
    ])
    .ok();
    for p in r.pull_requests.iter().flatten() {
        w.write_record([
            p.repository.clone(), p.number.to_string(), text(&p.title), p.state.as_str().to_string(), p.is_draft.to_string(),
            text(&p.author), p.created_at.to_string(), ts(p.merged_at), p.events.to_string(), p.events_by_subject.to_string(),
            p.commits.to_string(), p.reviews.to_string(), p.comments.to_string(), p.participants.to_string(),
            p.discussion_participants.to_string(), p.threads.to_string(), ts(p.last_activity),
            opt(p.latest_ci.map(ghusage_core::model::CheckOutcome::as_str)), p.url.clone(), overall.to_string(),
        ])
        .ok();
    }
}

fn write_review_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report, overall: &str) {
    w.write_record(["direction", "submitted_at", "reviewer", "state", "repository", "pr_number", "url", "period_coverage"]).ok();
    for (dir, rows) in [
        ("submitted", r.reviews_submitted.as_deref().unwrap_or(&[])),
        ("received", r.reviews_received.as_deref().unwrap_or(&[])),
    ] {
        for x in rows {
            w.write_record([
                dir.to_string(), x.submitted_at.to_string(), text(&x.reviewer),
                opt(x.state.map(ghusage_core::model::ReviewState::as_str)), x.repository.clone(),
                opt(x.pr_number), opt(x.url.clone()), overall.to_string(),
            ])
            .ok();
        }
    }
}

fn write_comment_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report, overall: &str) {
    w.write_record(["created_at", "author", "kind", "written_by_you", "repository", "pr_number", "edited", "url", "period_coverage"]).ok();
    for c in r.comments.iter().flatten() {
        w.write_record([
            c.created_at.to_string(), text(&c.author), c.kind.as_str().to_string(), c.written_by_subject.to_string(),
            c.repository.clone(), opt(c.pr_number), c.edited.to_string(), opt(c.url.clone()), overall.to_string(),
        ])
        .ok();
    }
}

fn write_issue_csv(w: &mut csv::Writer<Vec<u8>>, r: &Report, overall: &str) {
    w.write_record(["repository", "number", "title", "open", "opened_in_period", "closed_in_period", "reopened_in_period", "url", "period_coverage"]).ok();
    for i in r.issues.iter().flatten() {
        w.write_record([
            i.repository.clone(), i.number.to_string(), text(&i.title), i.is_open.to_string(), i.opened_in_period.to_string(),
            i.closed_in_period.to_string(), i.reopened_in_period.to_string(), i.url.clone(), overall.to_string(),
        ])
        .ok();
    }
}
