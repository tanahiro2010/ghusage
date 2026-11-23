fn activity_chart(f: &mut Frame, r: &Report, area: Rect) {
    let values: Vec<u64> = r
        .activity_breakdown
        .iter()
        .map(|b| activity(&b.summary))
        .collect();
    let total: u64 = values.iter().sum();
    let inner_w = area.width.saturating_sub(2) as usize;
    if total == 0 {
        return empty(
            f,
            area,
            "Activity per day".into(),
            &["No activity in this period. Press t to change the time range or r to refresh."],
        );
    }
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
    let peak = shown.iter().copied().max().unwrap_or(0);
    let title = format!("Activity per day · {first} → {last} · total {total} · peak {peak}");
    f.render_widget(
        Sparkline::default()
            .block(block(fit(&title, inner_w.saturating_sub(4)), false))
            .data(shown)
            .style(Style::new().fg(Color::Cyan)),
        area,
    );
}

fn draw_dashboard(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.base_report() else {
        return loading(f, area, "Dashboard");
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let cols = match inner_w {
        w if w >= 72 => 4,
        w if w >= 36 => 2,
        _ => 1,
    };
    let figures = headline(r);
    let fig_rows = figures.len().div_ceil(cols) as u16;
    let head_h = fig_rows + 2;
    const CHART_H: u16 = 5;
    const LISTS_MIN: u16 = 5;
    let h = area.height;
    let show_chart = h >= head_h + 1 + CHART_H + LISTS_MIN;
    let show_lists = h >= head_h + 1 + LISTS_MIN;
    let mut constraints = vec![Constraint::Length(head_h), Constraint::Length(1)];
    if show_chart {
        constraints.push(Constraint::Length(CHART_H));
    }
    if show_lists {
        constraints.push(Constraint::Min(LISTS_MIN));
    }
    let areas = Layout::vertical(constraints).split(area);

    let title = format!(
        "At a glance · {} → {}",
        app.zone.format(app.period.start, "%Y-%m-%d"),
        app.zone.format(app.period.end, "%Y-%m-%d")
    );
    let summary_focus = app.dashboard_pane == DashboardPane::Summary;
    f.render_widget(
        Paragraph::new(figure_lines(&figures, cols, inner_w / cols))
            .block(block(fit(&title, inner_w.saturating_sub(4)), summary_focus)),
        areas[0],
    );
    if areas.len() > 1 {
        f.render_widget(
            Paragraph::new(fit(&coverage_line(app, r), area.width as usize)),
            areas[1],
        );
    }
    let mut next = 2;
    if show_chart {
        activity_chart(f, r, areas[next]);
        next += 1;
    }
    if !show_lists {
        return;
    }
    let lists = areas[next];
    if lists.width >= 70 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(lists);
        dashboard_repos(f, app, r, left);
        dashboard_prs(f, app, r, right);
    } else if lists.height >= 10 {
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(lists);
        dashboard_repos(f, app, r, top);
        dashboard_prs(f, app, r, bottom);
    } else if app.dashboard_pane == DashboardPane::PullRequests {
        // Too small for both: show the focused one.
        dashboard_prs(f, app, r, lists);
    } else {
        dashboard_repos(f, app, r, lists);
    }
}
