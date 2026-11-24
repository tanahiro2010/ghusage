/// Draws the shared screen chrome and current view.
pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    if area.width < 20 || area.height < 6 {
        f.render_widget(
            Paragraph::new("terminal too small").wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [header, tabs, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, app, header);
    draw_tabs(f, app, tabs);
    if let Some(err) = app.load_error() {
        let p = Paragraph::new(vec![
            Line::styled("! Could not read the cache", bold()),
            Line::raw(clean(err)),
            Line::raw(""),
            Line::raw("Press r to retry."),
        ])
        .wrap(Wrap { trim: true })
        .block(block(app.screen.title().into(), false));
        f.render_widget(p, body);
    } else {
        match app.screen {
            Screen::Dashboard => draw_dashboard(f, app, body),
            Screen::Overview => draw_overview(f, app, body),
            Screen::DayDetail | Screen::RepoDetail => draw_scope_detail(f, app, body),
            Screen::Issues => draw_issues(f, app, body),
            Screen::IssueDetail => draw_issue_detail(f, app, body),
            Screen::Daily => draw_daily(f, app, body),
            Screen::Repositories => draw_repositories(f, app, body),
            Screen::PullRequests => draw_pull_requests(f, app, body),
            Screen::PrDetail => draw_pr_detail(f, app, body),
            Screen::PrTimeline => draw_pr_timeline(f, app, body),
            Screen::CommitDetail => draw_commit_detail(f, app, body),
            Screen::Reviews => draw_reviews(f, app, body),
            Screen::System => draw_system(f, app, body),
        }
    }
    draw_status(f, app, status);
    if app.show_help {
        draw_help(f, area);
    }
}

fn draw_header(f: &mut Frame<'_>, app: &App, area: Rect) {
    let z = &app.zone;
    let text = format!(
        " ghusage · {} · {} → {} ({}) · range: {} · repos: {}",
        clean(&app.system.login),
        z.format(app.period.start, "%Y-%m-%d %H:%M"),
        z.format(app.period.end, "%Y-%m-%d %H:%M"),
        z.name(),
        app.time_range.label(),
        app.filter_preset.label(),
    );
    f.render_widget(
        Paragraph::new(fit(&text, area.width as usize)).style(bold()),
        area,
    );
}

fn tab_spans(app: &App, width: usize) -> Vec<Span<'static>> {
    let tab = app.tab();
    let build = |style: u8| -> Vec<(String, bool)> {
        Screen::TOP
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let on = *s == tab;
                let label = match style {
                    0 => format!("{} {}", i + 1, s.title()),
                    1 => format!("{} {}", i + 1, s.short_title()),
                    _ if on => format!("{} {}", i + 1, s.short_title()),
                    _ => format!("{}", i + 1),
                };
                (label, on)
            })
            .collect()
    };
    let len = |v: &[(String, bool)], gap: usize| -> usize {
        v.iter().map(|(l, _)| display_width(l) + 2 + gap).sum()
    };
    let mut labels = build(0);
    let mut gap = 1;
    if len(&labels, gap) > width {
        labels = build(1);
    }
    if len(&labels, gap) > width {
        labels = build(2);
        gap = 0;
    }
    let mut spans = Vec::new();
    for (label, on) in labels {
        if on {
            spans.push(Span::styled(format!("[{label}]"), highlight()));
        } else {
            spans.push(Span::raw(format!(" {label} ")));
        }
        if gap > 0 {
            spans.push(Span::raw(" "));
        }
    }
    spans
}

fn draw_tabs(f: &mut Frame<'_>, app: &App, area: Rect) {
    let width = area.width as usize;
    let mut spans = tab_spans(app, width);
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let crumbs = app.breadcrumb();
    if !crumbs.is_empty() {
        let full: Vec<&str> = crumbs.iter().map(|s| s.title()).collect();
        let short: Vec<&str> = crumbs.iter().map(|s| s.short_title()).collect();
        let room = width.saturating_sub(used);
        let mut text = format!("› {}", full.join(" › "));
        if display_width(&text) > room {
            text = format!("› {}", short.join(" › "));
        }
        spans.push(Span::styled(fit(&text, room), bold()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Returns the key hints for the current screen.
pub const fn key_hint(app: &App) -> &'static str {
    match app.screen {
        Screen::Dashboard => "Tab pane · Enter open",
        Screen::Overview => "Tab pane · Enter repo",
        Screen::Daily => "Enter day detail · / search",
        Screen::Repositories => "Enter repo detail · / search",
        Screen::PullRequests => "Enter PR detail · / search",
        Screen::Reviews => "Tab sub/recv · Enter PR",
        Screen::Issues => "Enter issue detail · / search",
        Screen::DayDetail | Screen::RepoDetail => "Tab section · Enter open · Esc back",
        Screen::PrDetail => "Tab commits · Enter open · o open · Esc back",
        Screen::CommitDetail => "j/k scroll · ] [ file · o open · Esc back",
        Screen::PrTimeline => "Tab type · o open · Esc back",
        Screen::IssueDetail => "o open · Esc back",
        Screen::System => "r refresh",
    }
}

fn draw_status(f: &mut Frame<'_>, app: &App, area: Rect) {
    if app.search_mode {
        let q = app
            .current_list()
            .map(|l| app.search_query(l).to_string())
            .unwrap_or_default();
        let text = format!(" search: {}▏  Enter keep · Esc clear", clean(&q));
        f.render_widget(
            Paragraph::new(fit(&text, area.width as usize)).style(highlight()),
            area,
        );
        return;
    }
    let mut parts = vec!["cached".to_string()];
    if let Some(c) = app.coverage() {
        parts.push(format!("coverage: {}", coverage_text(c)));
    }
    parts.push(match app.system.synced_at {
        Some(t) => format!("synced {}", app.zone.format(t, "%m-%d %H:%M")),
        None => "never synced".into(),
    });
    if app.is_computing() {
        parts.push("⧗ computing".into());
    }
    if app.screen == Screen::CommitDetail
        && matches!(app.commit_state(), Some(CommitState::Loading))
    {
        parts.push("⧗ loading commit".into());
    }
    if app.sync.running {
        parts.push(format!(
            "⟳ sync: {}",
            clean(app.sync.progress.as_deref().unwrap_or("running"))
        ));
    } else if let Some(e) = &app.sync.last_error {
        parts.push(format!("! sync failed: {}", clean(e)));
    } else if !app.sync.warnings.is_empty() {
        parts.push(format!(
            "! sync incomplete ({} warnings)",
            app.sync.warnings.len()
        ));
    } else if !app.background_sync {
        parts.push("offline".into());
    }
    if let Some(m) = &app.message {
        parts.push(clean(m));
    }
    let base = "  ? help · q quit ";
    let with_hint = format!("  {} ·{}", key_hint(app), &base[1..]);
    let right = if area.width >= 110 {
        with_hint.as_str()
    } else {
        base
    };
    let left_w = (area.width as usize).saturating_sub(display_width(right));
    let left = fit(&format!(" {}", parts.join(" · ")), left_w);
    let pad = left_w.saturating_sub(display_width(&left));
    let line = Line::from(vec![
        Span::raw(left),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, dim()),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
    );
}

fn empty(f: &mut Frame<'_>, area: Rect, title: String, lines: &[&str]) {
    let text: Vec<Line<'_>> = lines.iter().map(|l| Line::raw(l.to_string())).collect();
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(block(title, false)),
        area,
    );
}

fn loading(f: &mut Frame<'_>, area: Rect, title: &str) {
    empty(
        f,
        area,
        title.into(),
        &["Computing… other screens and keys keep working meanwhile."],
    );
}

fn no_match(app: &App, list: ListId) -> String {
    format!(
        "No matches for \"{}\". Press Esc to clear the search.",
        clean(app.search_query(list))
    )
}

fn stateful_table(
    f: &mut Frame<'_>,
    app: &App,
    area: Rect,
    list: ListId,
    table: Table<'_>,
    focused: bool,
) {
    let sel = app.selected(list);
    let offset = app.offsets.borrow().get(&list).copied().unwrap_or(0);
    let mut state = TableState::new()
        .with_offset(offset)
        .with_selected(focused.then_some(sel));
    f.render_stateful_widget(table, area, &mut state);
    app.offsets.borrow_mut().insert(list, state.offset());
}

fn styled_table<'a>(rows: Vec<Row<'a>>, widths: Vec<Constraint>, header: &[&'a str]) -> Table<'a> {
    Table::new(rows, widths)
        .header(Row::new(header.iter().map(|h| Cell::from(*h))).style(bold()))
        .row_highlight_style(highlight())
        .highlight_symbol(SELECT_SYMBOL)
}

fn line_cells(d: &ghusage_core::metrics::DiffTotals) -> [String; 2] {
    use ghusage_core::metrics::DiffCoverage;
    let mark = |n: u64| match d.coverage {
        DiffCoverage::NoCommits | DiffCoverage::Complete => n.to_string(),
        DiffCoverage::Partial => format!("~{n}"),
        DiffCoverage::Unavailable => "?".into(),
    };
    [mark(d.additions), mark(d.deletions)]
}

fn flex_width(area: Rect, fixed: &[u16]) -> usize {
    let inner = area.width.saturating_sub(2) as usize;
    let used: usize = fixed.iter().map(|w| *w as usize + 1).sum::<usize>() + SELECT_SYMBOL.len();
    inner.saturating_sub(used).max(8)
}

fn visibility_text(vis: &str, owned: bool) -> String {
    format!("{vis} · {}", if owned { "owned" } else { "external" })
}

fn kv(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<22}"), bold()),
        Span::raw(value),
    ])
}

fn doc_line(r: &DocRow, width: usize) -> Line<'static> {
    const KEY_W: usize = 16;
    match r.kind {
        RowKind::Field => Line::from(vec![
            Span::styled(format!("{:<KEY_W$}", r.key), bold()),
            Span::raw(fit(&r.text, width.saturating_sub(KEY_W))),
        ]),
        kind => {
            let style = match kind {
                RowKind::Title | RowKind::Total => bold(),
                RowKind::Heading => bold().add_modifier(Modifier::UNDERLINED),
                RowKind::FileHeader => bold().fg(Color::Yellow),
                RowKind::Hunk => Style::new().fg(Color::Cyan),
                RowKind::Added => Style::new().fg(Color::Green),
                RowKind::Removed => Style::new().fg(Color::Red),
                RowKind::Warning => Style::new().fg(Color::Yellow),
                RowKind::Note => dim(),
                _ => Style::new(),
            };
            Line::styled(fit(&r.text, width), style)
        }
    }
}

const fn headline(r: &Report) -> [(&'static str, &'static str, u64); 8] {
    let s = &r.metrics;
    [
        ("Commits", "Commits", s.commits),
        ("PRs created", "PRs created", s.prs_created),
        ("PRs merged", "PRs merged", s.prs_merged),
        (
            "Reviews submitted",
            "Rev. submitted",
            s.reviews_submitted.total,
        ),
        (
            "Reviews received",
            "Rev. received",
            s.reviews_received.total,
        ),
        ("Comments written", "Comments", s.comments_written.total()),
        ("Issues opened", "Issues", s.issues_opened),
        ("Active days", "Active days", s.active_days),
    ]
}

fn figure_lines(figures: &[(&str, &str, u64)], cols: usize, cell_w: usize) -> Vec<Line<'static>> {
    figures
        .chunks(cols.max(1))
        .map(|row| {
            let mut spans = Vec::new();
            for (long, short, v) in row {
                let v = v.to_string();
                let room = cell_w.saturating_sub(display_width(&v) + 2);
                let label = if display_width(long) <= room {
                    long.to_string()
                } else if display_width(short) <= room {
                    short.to_string()
                } else {
                    fit(short, room)
                };
                let pad = cell_w.saturating_sub(display_width(&label) + display_width(&v) + 2);
                spans.push(Span::raw(format!("{label} {}", " ".repeat(pad))));
                spans.push(Span::styled(v, bold()));
                spans.push(Span::raw(" "));
            }
            Line::from(spans)
        })
        .collect()
}

fn coverage_line(app: &App, r: &Report) -> String {
    let overall = r.overall_coverage();
    let mark = if overall == CoverageStatus::Complete {
        "✓"
    } else {
        "!"
    };
    let mut parts = vec![format!("{mark} coverage: {}", coverage_text(overall))];
    for c in &r.coverage {
        if c.status != CoverageStatus::Complete {
            let gaps = c.gaps().count();
            parts.push(if gaps > 0 {
                format!(
                    "{} {} ({gaps} gap(s))",
                    c.kind.as_str(),
                    coverage_text(c.status)
                )
            } else {
                format!("{} {}", c.kind.as_str(), coverage_text(c.status))
            });
        }
    }
    parts.push(match app.system.synced_at {
        Some(t) => format!("synced {}", app.zone.format(t, "%Y-%m-%d %H:%M")),
        None => "never synced".into(),
    });
    parts.join(" · ")
}

fn dashboard_repos(f: &mut Frame<'_>, app: &App, r: &Report, area: Rect) {
    let focused = app.dashboard_pane == DashboardPane::Repositories;
    let title = "Top repositories".to_string();
    if r.repositories.is_empty() {
        return empty(f, area, title, &["No repository activity in this period."]);
    }
    let wide = area.width >= 50;
    let fixed: &[u16] = if wide { &[7, 4, 7] } else { &[7] };
    let name_w = flex_width(area, fixed);
    let rows: Vec<Row<'_>> = r
        .repositories
        .iter()
        .map(|x| {
            let s = &x.summary;
            let mut cells = vec![fit(&x.full_name, name_w), s.commits.to_string()];
            if wide {
                cells.push(s.prs_created.to_string());
                cells.push(s.reviews_submitted.total.to_string());
            }
            Row::new(cells)
        })
        .collect();
    let mut widths = vec![Constraint::Min(8)];
    widths.extend(fixed.iter().map(|w| Constraint::Length(*w)));
    let header: &[&str] = if wide {
        &["Repository", "Commits", "PRs", "Reviews"]
    } else {
        &["Repository", "Commits"]
    };
    let t = styled_table(rows, widths, header).block(block(title, focused));
    stateful_table(f, app, area, ListId::DashboardRepos, t, focused);
}

fn dashboard_prs(f: &mut Frame<'_>, app: &App, r: &Report, area: Rect) {
    let focused = app.dashboard_pane == DashboardPane::PullRequests;
    let title = "Recently active pull requests".to_string();
    let rows_all = r.pull_requests.as_deref().unwrap_or(&[]);
    if rows_all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No pull request activity in this period."],
        );
    }
    let wide = area.width >= 60;
    let fixed: &[u16] = if wide { &[9, 11] } else { &[9] };
    let flex = flex_width(area, fixed);
    let pr_w = (flex * 2 / 5).max(6);
    let title_w = flex.saturating_sub(pr_w + 1).max(4);
    let rows: Vec<Row<'_>> = rows_all
        .iter()
        .map(|p| {
            let mut cells = vec![
                Cell::from(Span::styled(
                    pr_state_text(p.state, p.is_draft),
                    pr_state_style(p.state),
                )),
                Cell::from(fit(&format!("{}#{}", p.repository, p.number), pr_w)),
                Cell::from(fit(&p.title, title_w)),
            ];
            if wide {
                cells.push(Cell::from(
                    p.last_activity.map_or_else(|| "-".into(), |t| app.zone.format(t, "%m-%d %H:%M")),
                ));
            }
            Row::new(cells)
        })
        .collect();
    let mut widths = vec![
        Constraint::Length(9),
        Constraint::Length(to_u16(pr_w)),
        Constraint::Min(4),
    ];
    if wide {
        widths.push(Constraint::Length(11));
    }
    let header: &[&str] = if wide {
        &["State", "PR", "Title", "Last"]
    } else {
        &["State", "PR", "Title"]
    };
    let t = styled_table(rows, widths, header).block(block(title, focused));
    stateful_table(f, app, area, ListId::DashboardPrs, t, focused);
}

const fn issue_state_text(open: bool) -> &'static str {
    if open { "○ open" } else { "✓ closed" }
}

fn issue_table<'a>(rows_all: &'a [IssueRow], vis: &[usize], area: Rect) -> Table<'a> {
    let fixed = [9, 6, 6, 8];
    let flex = flex_width(area, &fixed);
    let issue_w = (flex * 2 / 5).max(8);
    let title_w = flex.saturating_sub(issue_w + 1).max(8);
    let rows: Vec<Row<'_>> = vis
        .iter()
        .map(|&i| {
            let x = &rows_all[i];
            Row::new(vec![
                issue_state_text(x.is_open).to_string(),
                fit(&format!("{}#{}", x.repository, x.number), issue_w),
                fit(&x.title, title_w),
                if x.opened_in_period { "yes" } else { "-" }.to_string(),
                x.closed_in_period.to_string(),
                x.reopened_in_period.to_string(),
            ])
        })
        .collect();
    styled_table(
        rows,
        vec![
            Constraint::Length(9),
            Constraint::Length(to_u16(issue_w)),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(8),
        ],
        &["State", "Issue", "Title", "Opened", "Closed", "Reopened"],
    )
}

fn actor_text(name: &str, by_subject: bool) -> String {
    if by_subject {
        format!("{} (you)", clean(name))
    } else {
        clean(name)
    }
}
