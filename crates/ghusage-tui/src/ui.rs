//! Rendering. Pure functions of [`App`] state: nothing here reads the cache
//! or the network. Every string that came from GitHub goes through
//! `sanitize_line` / `truncate_width` before it reaches the terminal, and
//! no information is carried by color alone.

use chrono::{DateTime, Utc};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{ReviewDirection, Summary};
use ghusage_core::model::{ActorKind, PrState};
use ghusage_core::sanitize::{display_width, sanitize_line, truncate_width};
use ghusage_core::time::Zone;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState, Wrap,
};

use crate::app::{App, ListId, OverviewPane, Screen, TimelineFilter};

const SELECT_SYMBOL: &str = "▶ ";

fn clean(s: &str) -> String {
    sanitize_line(s)
}

fn fit(s: &str, width: usize) -> String {
    truncate_width(s, width)
}

fn ts(zone: Zone, t: Option<DateTime<Utc>>) -> String {
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

/// Activity measure used for the timeline chart.
pub fn activity(s: &Summary) -> u64 {
    s.commits + s.pr_internal_events + s.issues_opened + s.issues_closed + s.issues_reopened
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
            Screen::Overview => draw_overview(f, app, body),
            Screen::Daily => draw_daily(f, app, body),
            Screen::Repositories => draw_repositories(f, app, body),
            Screen::PullRequests => draw_pull_requests(f, app, body),
            Screen::PrDetail => draw_pr_detail(f, app, body),
            Screen::PrTimeline => draw_pr_timeline(f, app, body),
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
    let z = app.zone;
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

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (i, s) in Screen::TOP.iter().enumerate() {
        let label = format!("{} {}", i + 1, s.title());
        if app.screen.top() == *s {
            spans.push(Span::styled(format!("[{label}]"), highlight()));
        } else {
            spans.push(Span::raw(format!(" {label} ")));
        }
        spans.push(Span::raw(" "));
    }
    match app.screen {
        Screen::PrDetail => spans.push(Span::styled("› Detail", bold())),
        Screen::PrTimeline => spans.push(Span::styled("› Detail › Timeline", bold())),
        _ => {}
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let right = "  ? help · q quit ";
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
    empty(f, area, title.into(), &["Loading…"]);
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
        Constraint::Length(7),
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
    let title = "Top repositories (Tab to focus, Enter for PRs)".to_string();
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
        "Daily · {} days · {} active · newest first",
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
        "Repositories · {} shown of {}",
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
    let fixed = [16, 7, 5, 6, 7, 8, 6, 10, 9, 16];
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
            Row::new(vec![
                fit(&name, name_w),
                visibility_text(x.visibility.as_str(), x.owned),
                s.commits.to_string(),
                s.prs_created.to_string(),
                s.prs_merged.to_string(),
                s.reviews_submitted.total.to_string(),
                s.comments_written.total().to_string(),
                s.issues_opened.to_string(),
                s.active_prs.to_string(),
                s.pr_internal_events.to_string(),
                ts(app.zone, s.last_activity),
            ])
        })
        .collect();
    let mut widths = vec![Constraint::Min(8)];
    widths.extend(fixed.iter().map(|w| Constraint::Length(*w)));
    let t = styled_table(
        rows,
        widths,
        &[
            "Repository",
            "Visibility",
            "Commits",
            "PRs",
            "Merged",
            "Reviews",
            "Comments",
            "Issues",
            "Active PRs",
            "PR events",
            "Last activity",
        ],
    )
    .block(block(title, true));
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
                Cell::from(ts(app.zone, p.last_activity)),
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
        if app.detail_pr.is_some() {
            return empty(
                f,
                area,
                "PR Detail".into(),
                &["This pull request is not in the cache. Press r to refresh or Esc to go back."],
            );
        }
        return loading(f, area, "PR Detail");
    };
    let z = app.zone;
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
            "Review → author reply",
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
        Line::styled("Enter: timeline · o: open on GitHub · Esc: back", dim()),
    ];
    f.render_widget(
        Paragraph::new(lines).block(block("PR Detail".into(), true)),
        area,
    );
}

// ----- PR timeline -----------------------------------------------------------

fn draw_pr_timeline(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_report() else {
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
    let title = format!("{title} · {} shown", vis.len());
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
                ts(app.zone, Some(x.submitted_at)),
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

// ----- System ----------------------------------------------------------------

fn draw_system(f: &mut Frame, app: &App, area: Rect) {
    let z = app.zone;
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
        None => lines.push(Line::raw("  loading…")),
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
        "1-6",
        "Overview / Daily / Repositories / Pull Requests / Reviews / System",
    ),
    (
        "Enter",
        "Open detail (PR list → PR detail → timeline; repo → its PRs)",
    ),
    ("Esc", "Back / clear search"),
    ("j / Down", "Next item"),
    ("k / Up", "Previous item"),
    ("PgDn PgUp g G", "Page down / up, first, last"),
    (
        "Tab / Shift+Tab",
        "Next / previous pane (Reviews: submitted/received)",
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
    let w = area.width.saturating_sub(4).min(86);
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
        Paragraph::new(lines).block(block("Help".into(), true)),
        rect,
    );
}
