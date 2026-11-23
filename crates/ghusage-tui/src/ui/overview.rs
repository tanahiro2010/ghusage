fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.base_report() else {
        return loading(f, area, "Overview");
    };
    let [metrics_area, chart_area, repos_area] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(5),
        Constraint::Min(3),
    ])
    .areas(area);
    draw_overview_summary(f, app, r, metrics_area);
    draw_overview_activity(f, r, chart_area);
    draw_overview_repositories(f, app, r, repos_area);
}

fn draw_overview_summary(f: &mut Frame, app: &App, r: &Report, area: Rect) {
    let s = &r.metrics;
    let rows = vec![
        (
            "Commits",
            s.commits.to_string(),
            "PRs created",
            s.prs_created.to_string(),
        ),
        (
            "PRs merged",
            s.prs_merged.to_string(),
            "Reviews submitted",
            s.reviews_submitted.total.to_string(),
        ),
        (
            "Comments written",
            s.comments_written.total().to_string(),
            "Active PRs",
            s.active_prs.to_string(),
        ),
        (
            "Reviews received",
            s.reviews_received.total.to_string(),
            "Open PRs (now)",
            r.snapshot.open_prs.to_string(),
        ),
        (
            "Issues opened",
            s.issues_opened.to_string(),
            "Active days",
            s.active_days.to_string(),
        ),
        (
            "Lines added",
            line_cells(&s.diff_stats)[0].clone(),
            "Lines deleted",
            line_cells(&s.diff_stats)[1].clone(),
        ),
    ];
    let rows: Vec<Row> = rows
        .into_iter()
        .map(|(a, av, b, bv)| {
            Row::new(vec![
                Cell::from(a),
                Cell::from(Span::styled(av, bold())),
                Cell::from(b),
                Cell::from(Span::styled(bv, bold())),
            ])
        })
        .collect();
    let t = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(8),
            Constraint::Length(18),
            Constraint::Length(8),
        ],
    )
    .block(block(
        "Summary".into(),
        app.overview_pane == OverviewPane::Summary,
    ));
        f.render_widget(t, area);
}

fn draw_overview_activity(f: &mut Frame, r: &Report, area: Rect) {
    // Activity timeline: one bar per day, newest on the right.
    let values: Vec<u64> = r
        .activity_breakdown
        .iter()
        .map(|b| activity(&b.summary))
        .collect();
    let total: u64 = values.iter().sum();
    let inner_w = area.width.saturating_sub(2) as usize;
    let shown = &values[values.len().saturating_sub(inner_w)..];
    let first = r
        .activity_breakdown
        .len()
        .checked_sub(shown.len())
        .and_then(|i| r.activity_breakdown.get(i))
        .map(|b| b.bucket.label.clone())
        .unwrap_or_default();
    let last = r
        .activity_breakdown
        .last()
        .map(|b| b.bucket.label.clone())
        .unwrap_or_default();
    let max = shown.iter().copied().max().unwrap_or(0);
    let title = format!("Activity per day · {first} → {last} · total {total} · peak {max}");
    if total == 0 {
        empty(
            f,
            area,
            "Activity per day".into(),
            &["No activity in this period. Press t to change the time range or r to refresh."],
        );
    } else {
        f.render_widget(
            Sparkline::default()
                .block(block(fit(&title, inner_w.saturating_sub(4)), false))
                .data(shown)
                .style(Style::new().fg(Color::Cyan)),
            area,
        );
    }


}

fn draw_overview_repositories(f: &mut Frame, app: &App, r: &Report, area: Rect) {
    let focused = app.overview_pane == OverviewPane::Repositories;
    let title = "Top repositories (Tab to focus, Enter for detail)".to_string();
    if r.repositories.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No repository activity in this period."],
        );
    }
    let name_w = flex_width(area, &[16, 7, 5, 7, 8]);
    let rows: Vec<Row> = r
        .repositories
        .iter()
        .map(|x| {
            let s = &x.summary;
            Row::new(vec![
                fit(&x.full_name, name_w),
                visibility_text(x.visibility.as_str(), x.owned),
                s.commits.to_string(),
                s.prs_created.to_string(),
                s.reviews_submitted.total.to_string(),
                s.comments_written.total().to_string(),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Min(8),
            Constraint::Length(16),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(8),
        ],
        &[
            "Repository",
            "Visibility",
            "Commits",
            "PRs",
            "Reviews",
            "Comments",
        ],
    )
    .block(block(title, focused));
    stateful_table(f, app, area, ListId::OverviewRepos, t, focused);
}
