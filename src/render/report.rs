pub(crate) fn report(r: &Report, kind: ReportKind, details: bool, style: Style) -> String {
    let zone = Zone::parse(&r.timezone).unwrap_or(Zone::utc());
    let mut out = String::new();
    match kind {
        ReportKind::Overview => render_overview(&mut out, r, &zone, style),
        ReportKind::Breakdown(g) => render_breakdown(&mut out, r, g, &zone, style),
        ReportKind::Repositories => render_repositories(&mut out, r, &zone, style),
        ReportKind::PullRequests { .. } => render_pull_requests(&mut out, r, details, &zone, style),
        ReportKind::Reviews => render_reviews(&mut out, r, &zone, style),
        ReportKind::Comments => render_comments(&mut out, r, &zone, style),
        ReportKind::Issues => render_issues(&mut out, r, &zone, style),
    }
    out.push('\n');
    out.push_str(&coverage_lines(&r.coverage, r.synced_at, &zone, style));
    out
}

fn render_overview(out: &mut String, r: &Report, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Overview", style));
    out.push_str(&kv(&summary_lines(&r.metrics, r.snapshot.open_prs)));
    if !r.repositories.is_empty() {
        out.push('\n');
        out.push_str(&style.bold("Top repositories\n"));
        out.push_str(&repo_table(r, Some(5)).render(style));
        out.push_str(&style.dim(&line_note(
            r.repositories.iter().take(5).map(|x| x.summary.diff_stats),
        )));
    }
}

fn render_breakdown(
    out: &mut String,
    r: &Report,
    granularity: ghusage_core::time::Granularity,
    zone: &Zone,
    style: Style,
) {
    let title = match granularity {
        ghusage_core::time::Granularity::Day => "Daily",
        ghusage_core::time::Granularity::Week => "Weekly",
        ghusage_core::time::Granularity::Month => "Monthly",
        ghusage_core::time::Granularity::Year => "Yearly",
    };
    out.push_str(&header(r, zone, title, style));
    out.push_str(&breakdown_table(r, true).render(style));
    out.push_str(&style.dim(&line_note(
        r.activity_breakdown
            .iter()
            .map(|b| b.summary.diff_stats)
            .chain([r.metrics.diff_stats]),
    )));
}

fn render_repositories(out: &mut String, r: &Report, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Repositories", style));
    let t = repo_table(r, None);
    if t.is_empty() {
        out.push_str("No repository activity in this period.\n");
    } else {
        out.push_str(&t.render(style));
        out.push_str(&style.dim("* owned by you\n"));
        out.push_str(&style.dim(&line_note(
            r.repositories.iter().map(|x| x.summary.diff_stats),
        )));
    }
}

fn render_pull_requests(out: &mut String, r: &Report, details: bool, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Pull requests", style));
    render_pull_request_table(out, r, style);
    if details {
        render_pull_request_details(out, r, style);
    }
}

fn render_pull_request_table(out: &mut String, r: &Report, style: Style) {
    let mut t = Table::new(&[
        "Repository", "#", "Title", "State", "Author", "Events", "Yours", "Commits",
        "Reviews", "Comments", "People", "CI", "Changes",
    ])
    .numeric(&[1, 5, 6, 7, 8, 9, 10, 12])
    .limit(0, 32)
    .limit(2, 40)
    .limit(4, 16);
    for p in r.pull_requests.iter().flatten() {
        let state = if p.is_draft && p.state == ghusage_core::model::PrState::Open {
            "draft".to_string()
        } else {
            p.state.as_str().to_string()
        };
        t.row(vec![
            p.repository.clone(),
            p.number.to_string(),
            p.title.clone(),
            state,
            p.author.clone(),
            p.events.to_string(),
            p.events_by_subject.to_string(),
            p.commits.to_string(),
            p.reviews.to_string(),
            p.comments.to_string(),
            p.participants.to_string(),
            p.latest_ci
                .map_or("-", ghusage_core::model::CheckOutcome::as_str)
                .to_string(),
            p.diff.map_or_else(
                || "?".into(),
                |d| format!("+{} -{}", d.additions, d.deletions),
            ),
        ]);
    }
    if t.is_empty() {
        out.push_str("No pull request activity in this period.\n");
    } else {
        out.push_str(&t.render(style));
        out.push_str(&style.dim(
            "Changes: each pull request's base-to-head diff when last synced, not changes made in the period (? not known)\n",
        ));
    }
}

fn render_pull_request_details(out: &mut String, r: &Report, style: Style) {
    out.push('\n');
    out.push_str(&style.bold("Activity per pull request\n"));
    if let Some(i) = &r.intensity {
        let lines = [
            stats_line("Events", &i.events_per_pr, 1.0, ""),
            stats_line("Reviews", &i.reviews_per_pr, 1.0, ""),
            stats_line("Comments", &i.comments_per_pr, 1.0, ""),
            stats_line("Commits", &i.commits_per_pr, 1.0, ""),
            stats_line("Participants", &i.participants_per_pr, 1.0, ""),
            stats_line("Discussion participants", &i.discussion_participants_per_pr, 1.0, ""),
            stats_line("Review threads", &i.threads_per_pr, 1.0, ""),
            stats_line("Active days", &i.active_days_per_pr, 1.0, ""),
        ];
        let refs: Vec<(&str, String)> = lines.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        out.push_str(&kv(&refs));
    }
    out.push_str(&style.bold("Lifecycle timing (your pull requests)\n"));
    if let Some(tm) = &r.timing {
        let lines = [
            stats_line("Time to first review", &tm.time_to_first_review, 3600.0, "h"),
            stats_line("Creation to merge", &tm.creation_to_merge, 3600.0, "h"),
            stats_line("Review to your next activity", &tm.review_to_next_author_activity, 3600.0, "h"),
            stats_line("First to last activity", &tm.first_to_last_activity, 3600.0, "h"),
        ];
        let refs: Vec<(&str, String)> = lines.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        out.push_str(&kv(&refs));
        out.push_str(&style.dim(
            "  \"Review to your next activity\" is the next observed event, not necessarily a response.\n",
        ));
    }
    out.push_str(&style.dim(
        "  These are descriptive statistics, not measures of quality or productivity.\n",
    ));
}

fn render_reviews(out: &mut String, r: &Report, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Reviews", style));
    let s = &r.metrics;
    out.push_str(&kv(&[
        (
            "Submitted",
            format!(
                "{} on {} PRs (approved {}, changes requested {}, commented {}, dismissed {})",
                s.reviews_submitted.total,
                s.reviewed_prs,
                s.reviews_submitted.approved,
                s.reviews_submitted.changes_requested,
                s.reviews_submitted.commented,
                s.reviews_submitted.dismissed_unknown_state
            ),
        ),
        (
            "Received",
            format!(
                "{} (approved {}, changes requested {}, commented {}; {} from bots)",
                s.reviews_received.total,
                s.reviews_received.approved,
                s.reviews_received.changes_requested,
                s.reviews_received.commented,
                s.reviews_received_from_bots
            ),
        ),
    ]));
    for (title, rows) in [
        ("Submitted", r.reviews_submitted.as_deref().unwrap_or(&[])),
        ("Received", r.reviews_received.as_deref().unwrap_or(&[])),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(&style.bold(&format!("{title}\n")));
        let mut tb = Table::new(&["Time", "Reviewer", "State", "Repository", "#", "Title"])
            .numeric(&[4])
            .limit(3, 32)
            .limit(5, 40);
        for x in rows {
            tb.row(vec![
                zone.format(x.submitted_at, "%Y-%m-%d %H:%M"),
                x.reviewer.clone(),
                x.state.map(|s| s.as_str().to_ascii_lowercase()).unwrap_or_default(),
                x.repository.clone(),
                x.pr_number.map(|n| n.to_string()).unwrap_or_default(),
                x.pr_title.clone().unwrap_or_default(),
            ]);
        }
        out.push_str(&tb.render(style));
    }
}

fn render_comments(out: &mut String, r: &Report, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Comments", style));
    out.push_str(&kv(&summary_lines(&r.metrics, r.snapshot.open_prs)[8..11]));
    let rows = r.comments.as_deref().unwrap_or(&[]);
    if !rows.is_empty() {
        out.push('\n');
        let mut tb = Table::new(&["Time", "Author", "Type", "Yours", "Repository", "#", "Edited"])
            .numeric(&[5])
            .limit(4, 32);
        for c in rows {
            tb.row(vec![
                zone.format(c.created_at, "%Y-%m-%d %H:%M"),
                c.author.clone(),
                c.kind.label().to_string(),
                if c.written_by_subject { "yes" } else { "" }.into(),
                c.repository.clone(),
                c.pr_number.map(|n| n.to_string()).unwrap_or_default(),
                if c.edited { "edited" } else { "" }.into(),
            ]);
        }
        out.push_str(&tb.render(style));
    }
}

fn render_issues(out: &mut String, r: &Report, zone: &Zone, style: Style) {
    out.push_str(&header(r, zone, "Issues", style));
    let s = &r.metrics;
    out.push_str(&kv(&[
        ("Issues", format!("opened {}, closed {}, reopened {}", s.issues_opened, s.issues_closed, s.issues_reopened)),
        ("Open issues you authored (current)", r.snapshot.open_issues_authored.to_string()),
    ]));
    let rows = r.issues.as_deref().unwrap_or(&[]);
    if !rows.is_empty() {
        out.push('\n');
        let mut tb = Table::new(&["Repository", "#", "Title", "State", "Opened", "Closed", "Reopened"])
            .numeric(&[1, 5, 6])
            .limit(0, 32)
            .limit(2, 48);
        for i in rows {
            tb.row(vec![
                i.repository.clone(),
                i.number.to_string(),
                i.title.clone(),
                if i.is_open { "open" } else { "closed" }.into(),
                if i.opened_in_period { "yes" } else { "" }.into(),
                i.closed_in_period.to_string(),
                i.reopened_in_period.to_string(),
            ]);
        }
        out.push_str(&tb.render(style));
    }
}
