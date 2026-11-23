fn draw_issues(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.issues_report() else {
        return loading(f, area, "Issues");
    };
    let rows_all = r.issues.as_deref().unwrap_or(&[]);
    let list = ListId::Issues;
    let vis = app.visible(list);
    let title = format!(
        "Issues you were active in · {} shown of {} · Enter for detail",
        vis.len(),
        rows_all.len()
    );
    if rows_all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No issue activity in this period. Press t to change the time range."],
        );
    }
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let t = issue_table(rows_all, &vis, area).block(block(title, true));
    stateful_table(f, app, area, list, t, true);
}

fn draw_issue_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.issue_report() else {
        if app.issue_lookup_done() {
            return empty(
                f,
                area,
                "Issue Detail".into(),
                &["This issue is not in the cache. Press r to refresh or Esc to go back."],
            );
        }
        return loading(f, area, "Issue Detail");
    };
    let z = &app.zone;
    let w = area.width.saturating_sub(26) as usize;
    let lines = vec![
        Line::styled(
            fit(
                &format!("{}#{} {}", r.repository, r.number, r.title),
                area.width.saturating_sub(2) as usize,
            ),
            bold(),
        ),
        kv("Repository", fit(&r.repository, w)),
        kv("Author", fit(&r.author, w)),
        kv("State (current)", issue_state_text(r.is_open).to_string()),
        kv("Created", ts(z, Some(r.created_at))),
        kv("Closed", ts(z, r.closed_at)),
        kv("URL", fit(&r.url, w)),
    ];
    let info_h = (lines.len() as u16 + 2).min(area.height);
    let [info, events] =
        Layout::vertical([Constraint::Length(info_h), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(lines).block(block("Issue Detail".into(), false)),
        info,
    );
    if events.height < 3 {
        return;
    }
    let list = ListId::IssueEvents;
    let vis = app.visible(list);
    let title = format!(
        "Events in the selected period · {} of {}",
        vis.len(),
        r.events.len()
    );
    if r.events.is_empty() {
        return empty(
            f,
            events,
            title,
            &["No events of this issue in the selected period."],
        );
    }
    if vis.is_empty() {
        return empty(f, events, title, &[&no_match(app, list)]);
    }
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let e = &r.events[i];
            Row::new(vec![
                z.format(e.occurred_at, "%Y-%m-%d %H:%M"),
                fit(&actor_text(&e.actor, e.by_subject), 30),
                e.event_type.label().to_string(),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(16),
            Constraint::Length(30),
            Constraint::Min(8),
        ],
        &["Time", "Actor", "Event"],
    )
    .block(block(title, true));
    stateful_table(f, app, events, list, t, true);
}
