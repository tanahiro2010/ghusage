fn draw_daily(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.base_report() else {
        return loading(f, area, "Daily");
    };
    let list = ListId::Daily;
    let vis = app.visible(list);
    let title = format!(
        "Daily · {} days · {} active · newest first · Enter for day detail",
        r.activity_breakdown.len(),
        r.metrics.active_days
    );
    if r.activity_breakdown.is_empty() {
        return empty(f, area, title, &["No days in this period."]);
    }
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let b = &r.activity_breakdown[i];
            let s = &b.summary;
            let quiet = activity(s) == 0;
            let [added, deleted] = line_cells(&s.diff_stats);
            let row = Row::new(vec![
                b.bucket.label.clone(),
                s.commits.to_string(),
                s.prs_created.to_string(),
                s.prs_merged.to_string(),
                s.reviews_submitted.total.to_string(),
                s.reviews_received.total.to_string(),
                s.comments_written.total().to_string(),
                s.pr_commits.to_string(),
                s.issues_opened.to_string(),
                s.active_prs.to_string(),
                added,
                deleted,
            ]);
            if quiet { row.style(dim()) } else { row }
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
        &[
            "Day",
            "Commits",
            "PRs",
            "Merged",
            "Reviews",
            "Received",
            "Comments",
            "PR commits",
            "Issues",
            "Active PRs",
            "+Lines",
            "-Lines",
        ],
    )
    .block(block(title, true));
    stateful_table(f, app, area, list, t, true);
}

fn draw_repositories(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.base_report() else {
        return loading(f, area, "Repositories");
    };
    let list = ListId::Repositories;
    let vis = app.visible(list);
    let title = format!(
        "Repositories · {} shown of {} · Enter for detail",
        vis.len(),
        r.repositories.len()
    );
    if r.repositories.is_empty() {
        return empty(
            f,
            area,
            title,
            &[
                "No repository activity in this period. Press t to change the time range or f to change the repository filter.",
            ],
        );
    }
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    // Columns after the name: header, width. On a narrow terminal the
    // least important ones are dropped (in `DROP_ORDER` order) so the
    // repository name stays readable.
    const COLS: [(&str, u16); 12] = [
        ("Visibility", 16),
        ("Commits", 7),
        ("PRs", 5),
        ("Merged", 6),
        ("Reviews", 7),
        ("Comments", 8),
        ("Issues", 6),
        ("Active PRs", 10),
        ("PR events", 9),
        ("+Lines", 8),
        ("-Lines", 8),
        ("Last activity", 16),
    ];
    const DROP_ORDER: [usize; 8] = [8, 7, 6, 10, 9, 3, 4, 0];
    const MIN_NAME: usize = 32;
    let mut keep = [true; COLS.len()];
    let widths_of = |keep: &[bool; COLS.len()]| -> Vec<u16> {
        COLS.iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|((_, w), _)| *w)
            .collect()
    };
    for &i in &DROP_ORDER {
        if flex_width(area, &widths_of(&keep)) >= MIN_NAME {
            break;
        }
        keep[i] = false;
    }
    let fixed = widths_of(&keep);
    let name_w = flex_width(area, &fixed);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let x = &r.repositories[i];
            let s = &x.summary;
            let mut name = x.full_name.clone();
            if !x.accessible {
                name.push_str(" (no access)");
            }
            let cells = [
                visibility_text(x.visibility.as_str(), x.owned),
                s.commits.to_string(),
                s.prs_created.to_string(),
                s.prs_merged.to_string(),
                s.reviews_submitted.total.to_string(),
                s.comments_written.total().to_string(),
                s.issues_opened.to_string(),
                s.active_prs.to_string(),
                s.pr_internal_events.to_string(),
                line_cells(&s.diff_stats)[0].clone(),
                line_cells(&s.diff_stats)[1].clone(),
                ts(&app.zone, s.last_activity),
            ];
            let mut row = vec![fit(&name, name_w)];
            row.extend(
                cells
                    .into_iter()
                    .zip(keep)
                    .filter(|(_, k)| *k)
                    .map(|(c, _)| c),
            );
            Row::new(row)
        })
        .collect();
    let mut widths = vec![Constraint::Min(8)];
    widths.extend(fixed.iter().map(|w| Constraint::Length(*w)));
    let mut headers = vec!["Repository"];
    headers.extend(
        COLS.iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|((h, _), _)| *h),
    );
    let t = styled_table(rows, widths, &headers).block(block(title, true));
    stateful_table(f, app, area, list, t, true);
}
