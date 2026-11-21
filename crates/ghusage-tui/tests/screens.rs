//! Rendering of every screen from synthetic data.

mod common;

use common::*;
use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_core::metrics::{CommitDateBasis, Filter};
use ghusage_tui::app::Screen;
use ratatui::crossterm::event::KeyCode;

fn all_screens(app: &mut ghusage_tui::app::App) -> Vec<(Screen, String)> {
    let mut out = Vec::new();
    for k in ['1', '2', '3', '4', '5', '6'] {
        app.handle_key(key(k));
        out.push((app.screen, screen_text(app, 160, 40)));
    }
    app.handle_key(key('4'));
    app.handle_key(code(KeyCode::Enter));
    out.push((app.screen, screen_text(app, 160, 40)));
    app.handle_key(code(KeyCode::Enter));
    out.push((app.screen, screen_text(app, 160, 40)));
    out
}

#[test]
fn every_screen_renders_with_its_title() {
    let mut app = app();
    let shots = all_screens(&mut app);
    let screens: Vec<Screen> = shots.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        screens,
        vec![
            Screen::Overview,
            Screen::Daily,
            Screen::Repositories,
            Screen::PullRequests,
            Screen::Reviews,
            Screen::System,
            Screen::PrDetail,
            Screen::PrTimeline,
        ]
    );
    let expect = [
        "Summary",
        "Daily",
        "Repositories",
        "Pull Requests you were active in",
        "Reviews you submitted",
        "Coverage for the selected period",
        "PR Detail",
        "timeline",
    ];
    for ((screen, text), needle) in shots.iter().zip(expect) {
        assert!(
            text.contains(needle),
            "{screen:?} lacks {needle:?}:\n{text}"
        );
    }
}

#[test]
fn japanese_and_emoji_titles_are_drawn() {
    let mut app = app();
    press(&mut app, "4");
    let text = screen_text(&app, 160, 30);
    assert!(text.contains("パ"), "{text}");
    assert!(text.contains("🚀"), "{text}");
    press(&mut app, "3");
    let text = screen_text(&app, 160, 30);
    assert!(text.contains("org/ウ"), "{text}");
}

#[test]
fn hostile_text_never_reaches_the_terminal_as_control_characters() {
    let mut app = app();
    for k in ['1', '2', '3', '4', '5', '6'] {
        app.handle_key(key(k));
        assert_clean(&app, 160, 40);
        assert_clean(&app, 40, 10);
    }
    // Open the pull request with the hostile title.
    press(&mut app, "4");
    app.handle_key(key('/'));
    press(&mut app, "#7");
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::PrDetail);
    let text = screen_text(&app, 160, 40);
    assert!(text.contains("red title"), "{text}");
    assert_clean(&app, 160, 40);
    app.handle_key(code(KeyCode::Enter));
    assert_clean(&app, 160, 40);
}

#[test]
fn empty_data_shows_friendly_messages() {
    let mut app = app_with(empty_dataset(), false);
    for k in ['1', '2', '3', '4', '5', '6'] {
        app.handle_key(key(k));
        let text = screen_text(&app, 120, 30);
        assert!(!text.contains("Loading"), "{text}");
        let _ = screen_text(&app, 40, 10);
    }
    press(&mut app, "1");
    assert!(screen_text(&app, 120, 30).contains("No activity in this period"));
    press(&mut app, "3");
    assert!(screen_text(&app, 120, 30).contains("No repository activity"));
    press(&mut app, "4");
    assert!(screen_text(&app, 120, 30).contains("No pull request activity"));
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::PullRequests, "nothing to open");
    press(&mut app, "5");
    assert!(screen_text(&app, 120, 30).contains("No reviews in this period"));
}

#[test]
fn small_and_large_terminals_render() {
    let mut app = app();
    for (w, h) in [(40, 10), (200, 60), (20, 6), (10, 3), (1, 1)] {
        for k in ['1', '2', '3', '4', '5', '6'] {
            app.handle_key(key(k));
            let _ = render(&app, w, h);
        }
        press(&mut app, "4");
        app.handle_key(code(KeyCode::Enter));
        let _ = render(&app, w, h);
        app.handle_key(code(KeyCode::Enter));
        let _ = render(&app, w, h);
        app.handle_key(key('?'));
        let _ = render(&app, w, h);
        app.handle_key(key('?'));
        app.handle_key(code(KeyCode::Esc));
        app.handle_key(code(KeyCode::Esc));
    }
}

#[test]
fn overview_shows_the_same_numbers_as_the_shared_report() {
    let app = app();
    let ctx = QueryContext {
        period: period_30d(),
        zone: zone(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::default(),
        redact_private: false,
        synced_at: None,
    };
    let expected = report::build(&dataset(), &ctx, ReportKind::Overview);
    let shown = app.base_report().unwrap();
    assert_eq!(shown.metrics, expected.metrics);
    assert_eq!(expected.metrics.commits, 3);

    let lines = render(&app, 120, 30);
    let row = |label: &str| {
        lines
            .iter()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no {label} row"))
            .clone()
    };
    let number_after = |line: &str, label: &str| -> u64 {
        let rest = &line[line.find(label).unwrap() + label.len()..];
        rest.split_whitespace().next().unwrap().parse().unwrap()
    };
    let m = &expected.metrics;
    for (label, value) in [
        ("Commits", m.commits),
        ("PRs created", m.prs_created),
        ("PRs merged", m.prs_merged),
        ("Reviews submitted", m.reviews_submitted.total),
        ("Comments written", m.comments_written.total()),
        ("Active PRs", m.active_prs),
    ] {
        assert_eq!(number_after(&row(label), label), value, "{label}");
    }
}

#[test]
fn pull_request_list_matches_the_shared_report() {
    let mut app = app();
    press(&mut app, "4");
    let ctx = app.query_context();
    let kind = ReportKind::PullRequests {
        participation: ghusage_core::metrics::Participation::Subject,
        state: None,
        sort: ghusage_core::metrics::PrSort::Activity,
    };
    let expected = report::build(&dataset(), &ctx, kind);
    assert_eq!(
        app.pr_list_report().unwrap().pull_requests,
        expected.pull_requests
    );
}

#[test]
fn status_bar_says_data_is_cached_and_shows_coverage() {
    let app = app();
    let lines = render(&app, 160, 20);
    let status = lines.last().unwrap();
    assert!(status.contains("cached"), "{status}");
    assert!(status.contains("coverage: complete"), "{status}");
    assert!(status.contains("offline"), "{status}");
}

#[test]
fn colors_are_not_the_only_carrier_of_state() {
    let mut app = app();
    press(&mut app, "4");
    let text = screen_text(&app, 160, 20);
    assert!(text.contains("merged"), "{text}");
    assert!(text.contains("open"), "{text}");
    // The selected row carries a symbol, not only a highlight color.
    assert!(text.contains("▶"), "{text}");
    // The active screen tab is bracketed.
    assert!(text.contains("[4 Pull Requests]"), "{text}");
}

#[test]
fn system_screen_reports_unknown_rate_limit_offline() {
    let mut app = app();
    press(&mut app, "6");
    let text = screen_text(&app, 160, 40);
    assert!(text.contains("Rate limit"), "{text}");
    assert!(text.contains("unknown"), "{text}");
    assert!(text.contains("offline"), "{text}");
    assert!(text.contains("commits"), "{text}");
    assert!(text.contains("pull_requests"), "{text}");
}

#[test]
fn help_overlay_lists_keys() {
    let mut app = app();
    app.handle_key(key('?'));
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("Help"));
    assert!(text.contains("Cycle time range"));
}
