fn draw_reviews(f: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(r) = app.reviews_report() else {
        return loading(f, area, "Reviews");
    };
    let [tabs_area, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let submitted = r.reviews_submitted.as_deref().unwrap_or(&[]);
    let received = r.reviews_received.as_deref().unwrap_or(&[]);
    let tab = |label: String, on: bool| {
        if on {
            Span::styled(format!("[{label}]"), highlight())
        } else {
            Span::raw(format!(" {label} "))
        }
    };
    let dir = app.review_direction;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" Tab: "),
            tab(
                format!("Submitted ({})", submitted.len()),
                dir == ReviewDirection::Submitted,
            ),
            Span::raw(" "),
            tab(
                format!("Received ({})", received.len()),
                dir == ReviewDirection::Received,
            ),
        ])),
        tabs_area,
    );
    let (rows_all, title) = match dir {
        ReviewDirection::Submitted => (submitted, "Reviews you submitted"),
        ReviewDirection::Received => (received, "Reviews on your pull requests"),
    };
    let list = app.current_list().unwrap_or(ListId::ReviewsSubmitted);
    let vis = app.visible(list);
    let title = format!("{title} · {} shown · Enter for PR detail", vis.len());
    if rows_all.is_empty() {
        return empty(f, table_area, title, &["No reviews in this period."]);
    }
    if vis.is_empty() {
        return empty(f, table_area, title, &[&no_match(app, list)]);
    }
    let fixed = [16, 20, 18];
    let flex = flex_width(table_area, &fixed);
    let pr_w = (flex * 2 / 5).max(8);
    let title_w = flex.saturating_sub(pr_w + 1).max(8);
    let rows: Vec<Row<'_>> = vis
        .iter()
        .map(|&i| {
            let x = &rows_all[i];
            let state = x
                .state.map_or_else(|| "-".into(), |s| s.as_str().to_ascii_lowercase().replace('_', " "));
            let pr = match x.pr_number {
                Some(n) => format!("{}#{n}", x.repository),
                None => x.repository.clone(),
            };
            Row::new(vec![
                ts(&app.zone, Some(x.submitted_at)),
                fit(&x.reviewer, 20),
                state,
                fit(&pr, pr_w),
                fit(x.pr_title.as_deref().unwrap_or("-"), title_w),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(16),
            Constraint::Length(20),
            Constraint::Length(18),
            Constraint::Length(to_u16(pr_w)),
            Constraint::Min(8),
        ],
        &["Submitted", "Reviewer", "State", "PR", "Title"],
    )
    .block(block(title, true));
    stateful_table(f, app, table_area, list, t, true);
}
