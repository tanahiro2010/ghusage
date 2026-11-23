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

include!("ui/shared.rs");
include!("ui/dashboard.rs");
include!("ui/overview.rs");
include!("ui/repositories.rs");
include!("ui/pull_requests.rs");
include!("ui/reviews.rs");
include!("ui/issues.rs");

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

include!("ui/draw_scope_detail.rs");

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
