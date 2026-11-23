//! Rendering. Pure functions of [`App`] state: nothing here reads the cache
//! or the network. Every string that came from GitHub goes through
//! `sanitize_line` / `truncate_width` before it reaches the terminal, and
//! no information is carried by color alone.

use ghusage_app::report::Report;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{IssueRow, ReviewDirection, Summary};
use ghusage_core::model::{ActorKind, PrState};
use ghusage_core::sanitize::{display_width, sanitize_line, truncate_width};
use ghusage_core::time::Zone;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState, Wrap,
};

use crate::app::{
    App, CommitState, DashboardPane, DetailTab, ListId, OverviewPane, PrPane, Screen,
    TimelineFilter,
};
use crate::commit_view::{Row as DocRow, RowKind};

const SELECT_SYMBOL: &str = "▶ ";

fn clean(s: &str) -> String {
    sanitize_line(s)
}

fn fit(s: &str, width: usize) -> String {
    truncate_width(s, width)
}

fn ts(zone: &Zone, t: Option<Timestamp>) -> String {
    t.map(|t| zone.format(t, "%Y-%m-%d %H:%M"))
        .unwrap_or_else(|| "-".into())
}

fn dur(secs: Option<i64>) -> String {
    match secs {
        None => "-".into(),
        Some(s) if s < 3600 => format!("{}m", s / 60),
        Some(s) if s < 86400 => format!("{:.1}h", s as f64 / 3600.0),
        Some(s) => format!("{:.1}d", s as f64 / 86400.0),
    }
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn coverage_text(c: CoverageStatus) -> &'static str {
    match c {
        CoverageStatus::Complete => "complete",
        CoverageStatus::Partial => "partial",
        CoverageStatus::NotAvailable => "not available",
        CoverageStatus::NotYetSynced => "not yet synced",
        CoverageStatus::Unknown => "unknown",
    }
}

fn pr_state_text(state: PrState, draft: bool) -> &'static str {
    match (state, draft) {
        (PrState::Open, true) => "◌ draft",
        (PrState::Open, false) => "○ open",
        (PrState::Merged, _) => "✓ merged",
        (PrState::Closed, _) => "✕ closed",
    }
}

fn pr_state_style(state: PrState) -> Style {
    match state {
        PrState::Open => Style::new().fg(Color::Green),
        PrState::Merged => Style::new().fg(Color::Magenta),
        PrState::Closed => Style::new().fg(Color::Red),
    }
}

/// Activity measure used for the timeline chart (the same measure that
/// ranks repositories).
pub fn activity(s: &Summary) -> u64 {
    ghusage_core::metrics::activity_weight(s)
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn highlight() -> Style {
    Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

fn block(title: String, focused: bool) -> Block<'static> {
    let title = if focused {
        Span::styled(format!(" ▸ {title} "), bold())
    } else {
        Span::raw(format!(" {title} "))
    };
    Block::new().borders(Borders::ALL).title(title)
}

/// Draws the whole UI.
pub fn draw(f: &mut Frame, app: &App) {
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

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
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

/// Tab labels: full titles, short titles, or numbers only (the active tab
/// keeps its title), whichever fits.
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

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
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

/// Key hints for the current screen, shown in the status bar.
pub fn key_hint(app: &App) -> &'static str {
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

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
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

fn empty(f: &mut Frame, area: Rect, title: String, lines: &[&str]) {
    let text: Vec<Line> = lines.iter().map(|l| Line::raw(l.to_string())).collect();
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(block(title, false)),
        area,
    );
}

fn loading(f: &mut Frame, area: Rect, title: &str) {
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

/// Renders a table with a remembered scroll offset.
fn stateful_table(f: &mut Frame, app: &App, area: Rect, list: ListId, table: Table, focused: bool) {
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

/// Width left for a flexible column after fixed ones.
/// Added and deleted lines of the counted commits: `~` for a sum over only
/// some commits, `?` when none has known counts.
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

// ----- Overview -------------------------------------------------------

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
    f.render_widget(t, metrics_area);

    // Activity timeline: one bar per day, newest on the right.
    let values: Vec<u64> = r
        .activity_breakdown
        .iter()
        .map(|b| activity(&b.summary))
        .collect();
    let total: u64 = values.iter().sum();
    let inner_w = chart_area.width.saturating_sub(2) as usize;
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
            chart_area,
            "Activity per day".into(),
            &["No activity in this period. Press t to change the time range or r to refresh."],
        );
    } else {
        f.render_widget(
            Sparkline::default()
                .block(block(fit(&title, inner_w.saturating_sub(4)), false))
                .data(shown)
                .style(Style::new().fg(Color::Cyan)),
            chart_area,
        );
    }

    let focused = app.overview_pane == OverviewPane::Repositories;
    let title = "Top repositories (Tab to focus, Enter for detail)".to_string();
    if r.repositories.is_empty() {
        return empty(
            f,
            repos_area,
            title,
            &["No repository activity in this period."],
        );
    }
    let name_w = flex_width(repos_area, &[16, 7, 5, 7, 8]);
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
    stateful_table(f, app, repos_area, ListId::OverviewRepos, t, focused);
}

fn visibility_text(vis: &str, owned: bool) -> String {
    format!("{vis} · {}", if owned { "owned" } else { "external" })
}

// ----- Daily ----------------------------------------------------------

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

// ----- Repositories -----------------------------------------------------

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

// ----- Pull requests ------------------------------------------------------

fn draw_pull_requests(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_list_report() else {
        return loading(f, area, "Pull Requests");
    };
    let rows_all = r.pull_requests.as_deref().unwrap_or(&[]);
    let list = ListId::PullRequests;
    let vis = app.visible(list);
    let title = format!(
        "Pull Requests you were active in · {} shown of {} · Enter for detail",
        vis.len(),
        rows_all.len()
    );
    if rows_all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No pull request activity in this period. Press t to change the time range."],
        );
    }
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let fixed = [9, 6, 5, 7, 7, 8, 6, 16];
    let flex = flex_width(area, &fixed);
    let repo_w = (flex * 2 / 5).max(8);
    let title_w = flex.saturating_sub(repo_w + 1).max(8);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let p = &rows_all[i];
            Row::new(vec![
                Cell::from(Span::styled(
                    pr_state_text(p.state, p.is_draft),
                    pr_state_style(p.state),
                )),
                Cell::from(fit(&format!("{}#{}", p.repository, p.number), repo_w)),
                Cell::from(fit(&p.title, title_w)),
                Cell::from(p.events.to_string()),
                Cell::from(p.events_by_subject.to_string()),
                Cell::from(p.commits.to_string()),
                Cell::from(p.reviews.to_string()),
                Cell::from(p.comments.to_string()),
                Cell::from(p.participants.to_string()),
                Cell::from(ts(&app.zone, p.last_activity)),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(9),
            Constraint::Length(repo_w as u16),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(16),
        ],
        &[
            "State",
            "PR",
            "Title",
            "Events",
            "Yours",
            "Commits",
            "Reviews",
            "Comments",
            "People",
            "Last activity",
        ],
    )
    .block(block(title, true));
    stateful_table(f, app, area, list, t, true);
}

// ----- PR detail -----------------------------------------------------------

fn kv(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<22}"), bold()),
        Span::raw(value),
    ])
}

fn draw_pr_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_report() else {
        if app.pr_lookup_done() {
            return empty(
                f,
                area,
                "PR Detail".into(),
                &["This pull request is not in the cache. Press r to refresh or Esc to go back."],
            );
        }
        return loading(f, area, "PR Detail");
    };
    let z = &app.zone;
    let w = area.width.saturating_sub(26) as usize;
    let state = match (r.state, r.is_draft) {
        (PrState::Open, true) => "open (draft)".to_string(),
        (s, _) => s.as_str().to_string(),
    };
    let merged = match (&r.merged_at, &r.merged_by) {
        (Some(m), Some(b)) => format!("{} by {}", ts(z, Some(*m)), clean(b)),
        (Some(m), None) => format!("{} (merger unknown)", ts(z, Some(*m))),
        _ => "-".into(),
    };
    let lines = vec![
        Line::styled(
            fit(
                &format!("{}#{} {}", r.repository, r.number, r.title),
                area.width.saturating_sub(2) as usize,
            ),
            bold(),
        ),
        Line::raw(""),
        kv("Title", fit(&r.title, w)),
        kv("Repository", fit(&r.repository, w)),
        kv("Author", fit(&r.author, w)),
        kv("State (current)", state),
        kv("Created", ts(z, Some(r.created_at))),
        kv("Merged", fit(&merged, w)),
        kv(
            "Commits",
            if r.removed_commits > 0 {
                format!("{} ({} no longer in the PR)", r.commits, r.removed_commits)
            } else {
                r.commits.to_string()
            },
        ),
        kv(
            "Changes (now)",
            fit(
                &match &r.diff {
                    Some(d) => format!(
                        "+{} -{} in {} files (base to head as of {})",
                        d.additions,
                        d.deletions,
                        d.changed_files,
                        ts(z, Some(d.observed_at))
                    ),
                    None => "unknown (sync again to fetch it)".into(),
                },
                w,
            ),
        ),
        kv(
            "Reviews",
            format!(
                "{} (approved {}, changes requested {}, commented {})",
                r.reviews.total,
                r.reviews.approved,
                r.reviews.changes_requested,
                r.reviews.commented
            ),
        ),
        kv(
            "Comments",
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                r.comments.total(),
                r.comments.conversation,
                r.comments.inline,
                r.comments.review_summary
            ),
        ),
        kv(
            "Review threads",
            format!(
                "{} (open {}, resolved {}, outdated {})",
                r.review_threads.total,
                r.review_threads.open,
                r.review_threads.resolved,
                r.review_threads.outdated
            ),
        ),
        kv(
            "Latest CI",
            r.latest_ci
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "-".into()),
        ),
        kv("Participants", fit(&r.participants.join(", "), w)),
        kv(
            "Time to first review",
            dur(r.timing.time_to_first_review_secs),
        ),
        kv("Creation to merge", dur(r.timing.creation_to_merge_secs)),
        kv(
            "Review → next author activity",
            dur(r.timing.review_to_next_author_activity_secs),
        ),
        kv(
            "First → last activity",
            dur(r.timing.first_to_last_activity_secs),
        ),
        kv("Timeline events", r.timeline.len().to_string()),
        kv(
            "Coverage",
            format!(
                "{} (synced {})",
                coverage_text(r.coverage),
                ts(z, r.synced_at)
            ),
        ),
        kv("URL", fit(&r.url, w)),
        Line::raw(""),
        Line::styled(
            match app.pr_pane {
                PrPane::Summary => "Enter: timeline · Tab: commits · o: open on GitHub · Esc: back",
                PrPane::Commits => {
                    "Enter: open commit · Tab: summary · o: open commit on GitHub · Esc: back"
                }
            },
            dim(),
        ),
    ];
    let focus_commits = app.pr_pane == PrPane::Commits;
    let h = area.height;
    let wanted = (r.commit_list.len() as u16).saturating_add(3).max(4);
    let list_h = if h >= 12 {
        let cap = if focus_commits { h - 4 } else { h / 2 };
        wanted.min(cap).max(4)
    } else if focus_commits {
        h
    } else {
        0
    };
    let [info_area, list_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(list_h)]).areas(area);
    if info_area.height > 0 {
        f.render_widget(
            Paragraph::new(lines).block(block("PR Detail".into(), !focus_commits)),
            info_area,
        );
    }
    if list_area.height > 0 {
        draw_pr_commits(f, app, r, list_area, focus_commits);
    }
}

/// The commits of the pull request on PR Detail. Commits that are no
/// longer part of it are marked `removed`.
fn draw_pr_commits(
    f: &mut Frame,
    app: &App,
    r: &ghusage_app::report::PrReport,
    area: Rect,
    focused: bool,
) {
    let list = ListId::PrCommits;
    let all = &r.commit_list;
    let removed = all.iter().filter(|c| c.removed).count();
    let mut title = format!("Commits ({}", all.len());
    if removed > 0 {
        title.push_str(&format!(", {removed} removed"));
    }
    title.push(')');
    title.push_str(if focused {
        " · Enter to open · o on GitHub"
    } else {
        " · Tab to select"
    });
    if all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No commits recorded for this pull request."],
        );
    }
    let vis = app.visible(list);
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let wide = area.width >= 70;
    let fixed: &[u16] = if wide { &[8, 7, 11, 14, 13] } else { &[8, 7] };
    let msg_w = flex_width(area, fixed);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let c = &all[i];
            let mark = if c.removed {
                "removed"
            } else if c.is_merge_commit {
                "merge"
            } else {
                ""
            };
            let mut cells = vec![
                Cell::from(ghusage_core::commit::short_sha(&c.sha).to_string()),
                Cell::from(mark),
            ];
            if wide {
                cells.push(Cell::from(app.zone.format(c.committed_at, "%m-%d %H:%M")));
                cells.push(Cell::from(fit(&c.author, 14)));
                cells.push(Cell::from(match c.lines {
                    Some(l) => format!("+{} -{}", l.additions, l.deletions),
                    None => "?".into(),
                }));
            }
            cells.push(Cell::from(fit(c.headline.as_deref().unwrap_or("-"), msg_w)));
            let row = Row::new(cells);
            if c.removed { row.style(dim()) } else { row }
        })
        .collect();
    let mut widths: Vec<Constraint> = fixed.iter().map(|w| Constraint::Length(*w)).collect();
    widths.push(Constraint::Min(8));
    let header: &[&str] = if wide {
        &["SHA", "", "Committed", "Author", "Lines", "Message"]
    } else {
        &["SHA", "", "Message"]
    };
    let t = styled_table(rows, widths, header).block(block(title, focused));
    stateful_table(f, app, area, list, t, focused);
}

// ----- Commit detail ---------------------------------------------------------

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

fn draw_commit_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(key) = &app.detail_commit else {
        return empty(f, area, "Commit".into(), &["No commit selected."]);
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    app.commit_view_height.set(inner_h.max(1));
    let short = ghusage_core::commit::short_sha(&key.sha).to_string();
    if let Some(CommitState::Ready(v)) = app.commit_state() {
        let len = v.rows.len();
        let top = app.commit_scroll.min(len.saturating_sub(inner_h));
        let end = (top + inner_h).min(len);
        // Only the rows on screen are turned into lines.
        let lines: Vec<Line> = v.rows[top..end]
            .iter()
            .map(|r| doc_line(r, inner_w))
            .collect();
        let mut title = format!(
            "Commit {} · lines {}–{} of {len}",
            clean(&short),
            top + 1,
            end
        );
        match v.file_at(top) {
            Some(i) => title.push_str(&format!(" · file {}/{}", i + 1, v.file_rows.len())),
            None => title.push_str(&format!(" · {} file(s)", v.file_rows.len())),
        }
        f.render_widget(
            Paragraph::new(lines).block(block(fit(&title, inner_w.saturating_sub(4)), true)),
            area,
        );
        return;
    }
    // Not loaded (yet): show what the cached activity already tells.
    let seed = &app.commit_seed;
    let w = inner_w.saturating_sub(22);
    let mut lines = vec![
        Line::styled(
            fit(
                &format!(
                    "{}@{} — {}",
                    key.repository,
                    short,
                    seed.headline.as_deref().unwrap_or("")
                ),
                inner_w,
            ),
            bold(),
        ),
        Line::raw(""),
        kv("Repository", fit(&key.repository, w)),
        kv("SHA", fit(&key.sha, w)),
        kv("Author", fit(seed.author.as_deref().unwrap_or("-"), w)),
        kv("Date", ts(&app.zone, seed.date)),
    ];
    if seed.is_merge {
        lines.push(kv("Parents", "merge commit".into()));
    }
    if seed.removed {
        lines.push(kv(
            "In the pull request",
            "removed (no longer part of the pull request)".into(),
        ));
    }
    lines.push(Line::raw(""));
    match app.commit_state() {
        Some(CommitState::Failed(e)) if e.offline => {
            lines.push(Line::styled(
                "! Not available offline: this commit's files and diff are not in the cache.",
                bold(),
            ));
            lines.push(Line::raw(
                "Run ghusage tui without --offline (with GitHub reachable) and open it again, or press r to retry.",
            ));
            lines.push(Line::styled(clean(&e.message), dim()));
        }
        Some(CommitState::Failed(e)) => {
            lines.push(Line::styled("! Could not load this commit.", bold()));
            lines.push(Line::raw(clean(&e.message)));
            lines.push(Line::raw("Press r to try again or o to open it on GitHub."));
        }
        _ => {
            lines.push(Line::raw(
                "⧗ Loading the commit (from the cache, or from GitHub when online)…",
            ));
            lines.push(Line::styled(
                "Other screens and keys keep working meanwhile; Esc goes back.",
                dim(),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(format!("Commit {}", clean(&short)), true)),
        area,
    );
}

// ----- PR timeline -----------------------------------------------------------

fn draw_pr_timeline(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_report() else {
        if !app.pr_lookup_done() {
            return loading(f, area, "PR Timeline");
        }
        return empty(
            f,
            area,
            "PR Timeline".into(),
            &["This pull request is not in the cache. Press Esc to go back."],
        );
    };
    let [tabs_area, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let mut spans = vec![Span::raw(" type (Tab): ")];
    for t in TimelineFilter::CYCLE {
        if t == app.timeline_filter {
            spans.push(Span::styled(format!("[{}]", t.label()), highlight()));
        } else {
            spans.push(Span::raw(format!(" {} ", t.label())));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    let list = ListId::Timeline;
    let vis = app.visible(list);
    let title = format!(
        "{}#{} timeline · {} of {} events",
        fit(&r.repository, 40),
        r.number,
        vis.len(),
        r.timeline.len()
    );
    if r.timeline.is_empty() {
        return empty(
            f,
            table_area,
            title,
            &["No events recorded for this pull request."],
        );
    }
    if vis.is_empty() {
        let msg = if app.search_query(list).is_empty() {
            "No events of this type. Press Tab to change the type.".to_string()
        } else {
            no_match(app, list)
        };
        return empty(f, table_area, title, &[&msg]);
    }
    let detail_w = flex_width(table_area, &[19, 24, 22]);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let e = &r.timeline[i];
            let mut actor = e.actor.clone();
            if e.actor_kind == ActorKind::Bot && !actor.ends_with("[bot]") {
                actor.push_str(" (bot)");
            }
            let mut detail = e.detail.clone().unwrap_or_default();
            if e.removed_observed_at.is_some() && !detail.contains("no longer present") {
                detail.push_str(" (removed)");
            }
            Row::new(vec![
                app.zone.format(e.occurred_at, "%Y-%m-%d %H:%M:%S"),
                fit(&actor, 24),
                e.label.clone(),
                fit(&detail, detail_w),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(19),
            Constraint::Length(24),
            Constraint::Length(22),
            Constraint::Min(8),
        ],
        &["Time", "Actor", "Event", "Detail"],
    )
    .block(block(title, true));
    stateful_table(f, app, table_area, list, t, true);
}

// ----- Reviews -------------------------------------------------------------

fn draw_reviews(f: &mut Frame, app: &App, area: Rect) {
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
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let x = &rows_all[i];
            let state = x
                .state
                .map(|s| s.as_str().to_ascii_lowercase().replace('_', " "))
                .unwrap_or_else(|| "-".into());
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
            Constraint::Length(pr_w as u16),
            Constraint::Min(8),
        ],
        &["Submitted", "Reviewer", "State", "PR", "Title"],
    )
    .block(block(title, true));
    stateful_table(f, app, table_area, list, t, true);
}

// ----- Dashboard ---------------------------------------------------------

/// Headline figures of the dashboard: (label, short label, value).
fn headline(r: &Report) -> [(&'static str, &'static str, u64); 8] {
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

/// Lines of `cols` figure cells, each `cell_w` wide.
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

/// One-line coverage state of a report. Problems are spelled out, not
/// only colored.
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

/// Bar chart of daily activity, newest on the right.
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

fn dashboard_repos(f: &mut Frame, app: &App, r: &Report, area: Rect) {
    let focused = app.dashboard_pane == DashboardPane::Repositories;
    let title = "Top repositories".to_string();
    if r.repositories.is_empty() {
        return empty(f, area, title, &["No repository activity in this period."]);
    }
    let wide = area.width >= 50;
    let fixed: &[u16] = if wide { &[7, 4, 7] } else { &[7] };
    let name_w = flex_width(area, fixed);
    let rows: Vec<Row> = r
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

fn dashboard_prs(f: &mut Frame, app: &App, r: &Report, area: Rect) {
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
    let rows: Vec<Row> = rows_all
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
                    p.last_activity
                        .map(|t| app.zone.format(t, "%m-%d %H:%M"))
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            Row::new(cells)
        })
        .collect();
    let mut widths = vec![
        Constraint::Length(9),
        Constraint::Length(pr_w as u16),
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

// ----- Issues ---------------------------------------------------------------

fn issue_state_text(open: bool) -> &'static str {
    if open { "○ open" } else { "✓ closed" }
}

fn issue_table<'a>(rows_all: &'a [IssueRow], vis: &[usize], area: Rect) -> Table<'a> {
    let fixed = [9, 6, 6, 8];
    let flex = flex_width(area, &fixed);
    let issue_w = (flex * 2 / 5).max(8);
    let title_w = flex.saturating_sub(issue_w + 1).max(8);
    let rows: Vec<Row> = vis
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
            Constraint::Length(issue_w as u16),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(8),
        ],
        &["State", "Issue", "Title", "Opened", "Closed", "Reopened"],
    )
}

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

fn actor_text(name: &str, by_subject: bool) -> String {
    if by_subject {
        format!("{} (you)", clean(name))
    } else {
        clean(name)
    }
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

// ----- Day and repository detail ------------------------------------------------

fn detail_count(r: &Report, tab: DetailTab) -> usize {
    match tab {
        DetailTab::Activity => r.activity.as_ref().map_or(0, Vec::len),
        DetailTab::Commits => r.commits.as_ref().map_or(0, Vec::len),
        DetailTab::PullRequests => r.pull_requests.as_ref().map_or(0, Vec::len),
        DetailTab::Reviews => {
            r.reviews_submitted.as_ref().map_or(0, Vec::len)
                + r.reviews_received.as_ref().map_or(0, Vec::len)
        }
        DetailTab::Comments => r.comments.as_ref().map_or(0, Vec::len),
        DetailTab::Issues => r.issues.as_ref().map_or(0, Vec::len),
    }
}

fn draw_scope_detail(f: &mut Frame, app: &App, area: Rect) {
    let name = match app.screen {
        Screen::DayDetail => app
            .detail_day
            .as_ref()
            .map(|d| format!("Day {}", d.label))
            .unwrap_or_else(|| "Day".into()),
        _ => app
            .detail_repo
            .as_ref()
            .map(|r| format!("Repository {}", clean(&r.full_name)))
            .unwrap_or_else(|| "Repository".into()),
    };
    let Some(r) = app.detail_report() else {
        return loading(f, area, app.screen.title());
    };
    let s = &r.metrics;
    let summary = [
        format!(
            "Commits {} · PRs created {} · merged {} · Reviews submitted {} · received {}",
            s.commits,
            s.prs_created,
            s.prs_merged,
            s.reviews_submitted.total,
            s.reviews_received.total
        ),
        format!(
            "Comments written {} · received {} · Issues opened {} · closed {} · Active PRs {} · coverage: {}",
            s.comments_written.total(),
            s.comments_received.total(),
            s.issues_opened,
            s.issues_closed,
            s.active_prs,
            coverage_text(r.overall_coverage())
        ),
    ];
    let show_summary = area.height >= 9;
    let constraints = if show_summary {
        vec![
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(1),
        ]
    } else {
        vec![Constraint::Length(1), Constraint::Min(1)]
    };
    let areas = Layout::vertical(constraints).split(area);
    let (tabs_area, list_area) = if show_summary {
        let inner = area.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = summary.iter().map(|l| Line::raw(fit(l, inner))).collect();
        f.render_widget(
            Paragraph::new(lines).block(block(fit(&name, inner.saturating_sub(4)), false)),
            areas[0],
        );
        (areas[1], areas[2])
    } else {
        (areas[0], areas[1])
    };

    let full: usize = app
        .detail_tabs()
        .iter()
        .map(|t| display_width(t.label()) + 8)
        .sum::<usize>()
        + 16;
    let short = full > tabs_area.width as usize;
    let mut spans = vec![Span::raw(if short { " " } else { " section (Tab): " })];
    for t in app.detail_tabs() {
        let name = if short { t.short_label() } else { t.label() };
        let label = format!("{name} ({})", detail_count(r, *t));
        if *t == app.detail_tab {
            spans.push(Span::styled(format!("[{label}]"), highlight()));
        } else {
            spans.push(Span::raw(format!(" {label} ")));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    let list = app.detail_tab.list();
    let vis = app.visible(list);
    let title = format!(
        "{} · {} · {} shown",
        fit(&name, 50),
        app.detail_tab.label(),
        vis.len()
    );
    if detail_count(r, app.detail_tab) == 0 {
        let msg = format!(
            "No {} here in this period. Press Tab for another section.",
            app.detail_tab.label().to_lowercase()
        );
        return empty(f, list_area, title, &[&msg]);
    }
    if vis.is_empty() {
        return empty(f, list_area, title, &[&no_match(app, list)]);
    }
    let z = &app.zone;
    let t = match app.detail_tab {
        DetailTab::Activity => {
            let rows_all = r.activity.as_deref().unwrap_or(&[]);
            let flex = flex_width(list_area, &[11, 20, 18]);
            let where_w = (flex * 2 / 5).max(8);
            let title_w = flex.saturating_sub(where_w + 1).max(8);
            let rows: Vec<Row> = vis
                .iter()
                .map(|&i| {
                    let a = &rows_all[i];
                    let place = match a.number {
                        Some(n) => format!("{}#{n}", a.repository),
                        None => a.repository.clone(),
                    };
                    Row::new(vec![
                        z.format(a.occurred_at, "%m-%d %H:%M"),
                        fit(&actor_text(&a.actor, a.by_subject), 20),
                        a.event_type.label().to_string(),
                        fit(&place, where_w),
                        fit(a.title.as_deref().unwrap_or("-"), title_w),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(11),
                    Constraint::Length(20),
                    Constraint::Length(18),
                    Constraint::Length(where_w as u16),
                    Constraint::Min(8),
                ],
                &["Time", "Actor", "Event", "Where", "Title"],
            )
        }
        DetailTab::Commits => {
            let rows_all = r.commits.as_deref().unwrap_or(&[]);
            let repo_w = flex_width(list_area, &[16, 12, 5]);
            let rows: Vec<Row> = vis
                .iter()
                .map(|&i| {
                    let c = &rows_all[i];
                    let sha: String = c.sha.as_deref().unwrap_or("-").chars().take(10).collect();
                    Row::new(vec![
                        z.format(c.committed_at, "%Y-%m-%d %H:%M"),
                        fit(&sha, 12),
                        fit(&c.repository, repo_w),
                        if c.is_merge_commit { "merge" } else { "" }.to_string(),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(16),
                    Constraint::Length(12),
                    Constraint::Min(8),
                    Constraint::Length(5),
                ],
                &["Committed", "SHA", "Repository", "Merge"],
            )
        }
        DetailTab::PullRequests => {
            let rows_all = r.pull_requests.as_deref().unwrap_or(&[]);
            let fixed = [9, 6, 5, 7, 8, 11];
            let flex = flex_width(list_area, &fixed);
            let pr_w = (flex * 2 / 5).max(8);
            let title_w = flex.saturating_sub(pr_w + 1).max(8);
            let rows: Vec<Row> = vis
                .iter()
                .map(|&i| {
                    let p = &rows_all[i];
                    Row::new(vec![
                        Cell::from(Span::styled(
                            pr_state_text(p.state, p.is_draft),
                            pr_state_style(p.state),
                        )),
                        Cell::from(fit(&format!("{}#{}", p.repository, p.number), pr_w)),
                        Cell::from(fit(&p.title, title_w)),
                        Cell::from(p.events.to_string()),
                        Cell::from(p.events_by_subject.to_string()),
                        Cell::from(p.reviews.to_string()),
                        Cell::from(p.comments.to_string()),
                        Cell::from(
                            p.last_activity
                                .map(|t| z.format(t, "%m-%d %H:%M"))
                                .unwrap_or_else(|| "-".into()),
                        ),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(9),
                    Constraint::Length(pr_w as u16),
                    Constraint::Min(8),
                    Constraint::Length(6),
                    Constraint::Length(5),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(11),
                ],
                &[
                    "State", "PR", "Title", "Events", "Yours", "Reviews", "Comments", "Last",
                ],
            )
        }
        DetailTab::Reviews => {
            let rows_all = app.detail_reviews();
            let flex = flex_width(list_area, &[9, 11, 18, 18]);
            let pr_w = (flex * 2 / 5).max(8);
            let rows: Vec<Row> = vis
                .iter()
                .map(|&i| {
                    let (dir, x) = rows_all[i];
                    let state = x
                        .state
                        .map(|s| s.as_str().to_ascii_lowercase().replace('_', " "))
                        .unwrap_or_else(|| "-".into());
                    let pr = match x.pr_number {
                        Some(n) => format!("{}#{n}", x.repository),
                        None => x.repository.clone(),
                    };
                    Row::new(vec![
                        match dir {
                            ReviewDirection::Submitted => "given",
                            ReviewDirection::Received => "received",
                        }
                        .to_string(),
                        z.format(x.submitted_at, "%m-%d %H:%M"),
                        fit(&x.reviewer, 18),
                        state,
                        fit(&pr, pr_w),
                        fit(
                            x.pr_title.as_deref().unwrap_or("-"),
                            flex.saturating_sub(pr_w + 1).max(8),
                        ),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(18),
                    Constraint::Length(18),
                    Constraint::Length(pr_w as u16),
                    Constraint::Min(8),
                ],
                &["Direction", "Submitted", "Reviewer", "State", "PR", "Title"],
            )
        }
        DetailTab::Comments => {
            let rows_all = r.comments.as_deref().unwrap_or(&[]);
            let pr_w = flex_width(list_area, &[11, 20, 15, 9]);
            let rows: Vec<Row> = vis
                .iter()
                .map(|&i| {
                    let c = &rows_all[i];
                    let pr = match c.pr_number {
                        Some(n) => format!("{}#{n}", c.repository),
                        None => c.repository.clone(),
                    };
                    let mut kind = c.kind.label().to_string();
                    if c.edited {
                        kind.push_str(" ✎");
                    }
                    Row::new(vec![
                        z.format(c.created_at, "%m-%d %H:%M"),
                        fit(&c.author, 20),
                        kind,
                        if c.written_by_subject {
                            "written"
                        } else {
                            "received"
                        }
                        .to_string(),
                        fit(&pr, pr_w),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(11),
                    Constraint::Length(20),
                    Constraint::Length(15),
                    Constraint::Length(9),
                    Constraint::Min(8),
                ],
                &["Time", "Author", "Kind", "Direction", "PR"],
            )
        }
        DetailTab::Issues => {
            let rows_all = r.issues.as_deref().unwrap_or(&[]);
            issue_table(rows_all, &vis, list_area)
        }
    }
    .block(block(title, true));
    stateful_table(f, app, list_area, list, t, true);
}

// ----- System ----------------------------------------------------------------

fn draw_system(f: &mut Frame, app: &App, area: Rect) {
    let z = &app.zone;
    let s = &app.system;
    let w = area.width.saturating_sub(26) as usize;
    let mut lines = vec![
        kv(
            "Account",
            format!(
                "{}{}",
                clean(&s.login),
                if s.subject_is_viewer {
                    " (authenticated user)"
                } else {
                    ""
                }
            ),
        ),
        kv(
            "Mode",
            if app.background_sync {
                "online · reading the cache, syncing in the background".into()
            } else {
                "offline · cache only, GitHub is not contacted".into()
            },
        ),
        kv(
            "Authentication",
            match (&app.sync.auth, app.background_sync) {
                (Some(a), _) => format!("token from {}", clean(a)),
                (None, true) if app.sync.running => "connecting…".into(),
                (None, true) if app.sync.last_error.is_some() => {
                    "sync could not start (see errors below)".into()
                }
                (None, true) => "not checked yet (press r to sync)".into(),
                (None, false) => "not used (offline)".into(),
            },
        ),
        kv(
            "Rate limit",
            app.sync
                .rate_limit
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "unknown".into()),
        ),
        kv(
            "Last sync",
            match &s.last_sync {
                Some(l) => fit(
                    &format!(
                        "{} · {}{}",
                        ts(z, l.finished_at.or(Some(l.started_at))),
                        l.status,
                        l.message
                            .as_deref()
                            .map(|m| format!(" · {m}"))
                            .unwrap_or_default()
                    ),
                    w,
                ),
                None => "never".into(),
            },
        ),
        kv(
            "Sync now",
            if app.sync.running {
                format!(
                    "running · {}",
                    clean(app.sync.progress.as_deref().unwrap_or(""))
                )
            } else if let Some(sum) = &app.sync.last_summary {
                fit(&format!("last run this session: {sum}"), w)
            } else {
                "idle".into()
            },
        ),
        kv(
            "Cache",
            match s.cache_size_bytes {
                Some(b) => format!(
                    "{} · {} events, {} pull requests, {} repositories",
                    bytes(b),
                    s.cached_events.unwrap_or(0),
                    s.cached_pull_requests.unwrap_or(0),
                    s.cached_repositories.unwrap_or(0)
                ),
                None => "unknown".into(),
            },
        ),
        kv(
            "Data directory",
            s.data_dir
                .as_ref()
                .map(|d| fit(&d.display().to_string(), w))
                .unwrap_or_else(|| "in memory".into()),
        ),
        Line::raw(""),
        Line::styled("Coverage for the selected period", bold()),
    ];
    match app.base_report() {
        Some(r) => {
            for c in &r.coverage {
                let gaps = c.gaps().count();
                lines.push(kv(
                    &format!("  {}", c.kind.as_str()),
                    format!(
                        "{}{}",
                        coverage_text(c.status),
                        if gaps > 0 {
                            format!(" · {gaps} gap(s)")
                        } else {
                            String::new()
                        }
                    ),
                ));
            }
        }
        None => lines.push(Line::raw("  computing…")),
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Warnings and errors", bold()));
    let mut any = false;
    if let Some(e) = &s.error {
        lines.push(Line::raw(format!("  ! cache: {}", fit(e, w))));
        any = true;
    }
    if let Some(e) = &app.sync.last_error {
        lines.push(Line::raw(format!("  ! sync failed: {}", fit(e, w))));
        any = true;
    }
    for warn in app.sync.warnings.iter().take(20) {
        lines.push(Line::raw(format!("  ! {}", fit(warn, w))));
        any = true;
    }
    if app.sync.warnings.len() > 20 {
        lines.push(Line::raw(format!(
            "  … and {} more",
            app.sync.warnings.len() - 20
        )));
    }
    if !any {
        lines.push(Line::raw("  none"));
    }
    f.render_widget(
        Paragraph::new(lines).block(block("System".into(), true)),
        area,
    );
}

// ----- Help --------------------------------------------------------------------

/// Key bindings shown in the help overlay.
pub const HELP: &[(&str, &str)] = &[
    (
        "1-8",
        "Dashboard / Overview / Daily / Repositories / Pull Requests / Reviews / Issues / System",
    ),
    (
        "Enter",
        "Open detail: day, repository, PR (then timeline), issue, commit; review → its PR",
    ),
    ("Esc", "Back to the previous screen / clear search"),
    ("j / Down", "Next item (Commit: scroll down)"),
    ("k / Up", "Previous item (Commit: scroll up)"),
    ("PgDn PgUp g G", "Page down / up, first, last"),
    ("] / [", "Commit: next / previous file of the diff"),
    (
        "Tab / Shift+Tab",
        "Next / previous pane or section (Reviews: submitted/received; PR: commits)",
    ),
    ("/", "Search the current list"),
    (
        "r",
        "Reload from cache (and sync in the background when online)",
    ),
    ("t", "Cycle time range"),
    (
        "f",
        "Cycle repository filter: all/public/private/owned/external",
    ),
    ("o", "Open the selected item on GitHub"),
    ("?", "Toggle this help"),
    ("q", "Quit"),
];

fn draw_help(f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(4).min(112);
    let h = area.height.saturating_sub(2).min(HELP.len() as u16 + 4);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let rect = Rect::new(x, y, w, h);
    f.render_widget(Clear, rect);
    let mut lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:<16}"), bold()),
                Span::raw(*v),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled("Press ? or Esc to close", dim()));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block("Help".into(), true)),
        rect,
    );
}
