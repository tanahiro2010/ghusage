//! Dashboard, detail screens (day, repository, issue) and the navigation
//! between them.

mod common;

use common::*;
use ghusage_app::report;
use ghusage_core::metrics::{Dataset, ReviewDirection};
use ghusage_core::model::{EventMeta, EventType, Issue};
use ghusage_core::time::Period;
use ghusage_tui::app::{App, DashboardPane, DetailTab, ListId, Screen};
use ratatui::crossterm::event::KeyCode;

/// The shared test world plus one issue (with a hostile title) that alice
/// opened and bob closed.
fn with_issue() -> Dataset {
    let mut ds = dataset();
    ds.issues.insert(
        "I1".into(),
        Issue {
            id: "I1".into(),
            repository_id: "R1".into(),
            number: 42,
            title: format!("バグ報告 {MALICIOUS_TITLE}"),
            url: "https://github.com/alice/parser/issues/42".into(),
            author: alice(),
            is_open: false,
            created_at: utc("2026-11-15T01:00:00Z"),
            closed_at: Some(utc("2026-11-16T01:00:00Z")),
        },
    );
    for (id, t, who, at) in [
        (
            "i1o",
            EventType::IssueOpened,
            alice(),
            "2026-11-15T01:00:00Z",
        ),
        ("i1c", EventType::IssueClosed, bob(), "2026-11-16T01:00:00Z"),
    ] {
        let mut e = event(id, t, who, "R1", None, utc(at), EventMeta::default());
        e.entity_id = "I1".into();
        e.url = Some("https://github.com/alice/parser/issues/42".into());
        ds.events.push(e);
    }
    ghusage_core::metrics::sort_events(&mut ds.events);
    ds
}

fn enter(app: &mut App) {
    app.handle_key(code(KeyCode::Enter));
}

fn esc(app: &mut App) {
    app.handle_key(code(KeyCode::Esc));
}

fn tab(app: &mut App) {
    app.handle_key(code(KeyCode::Tab));
}

fn search(app: &mut App, q: &str) {
    app.handle_key(key('/'));
    press(app, q);
    enter(app);
}

fn number_after(line: &str, label: &str) -> u64 {
    let rest = &line[line.find(label).unwrap() + label.len()..];
    rest.split_whitespace().next().unwrap().parse().unwrap()
}

// ----- Dashboard -------------------------------------------------------------

#[test]
fn dashboard_is_the_first_tab_and_shows_everything_at_a_glance() {
    let app = app();
    assert_eq!(app.screen, Screen::Dashboard);
    let text = screen_text(&app, 120, 30);
    for needle in [
        "[1 Dashboard]",
        "At a glance",
        "Reviews submitted",
        "Reviews received",
        "Issues opened",
        "Activity per day",
        "Top repositories",
        "Recently active pull requests",
        "✓ coverage: complete",
        "alice/parser",
        "パ",
        "🚀",
    ] {
        assert!(text.contains(needle), "missing {needle:?}:\n{text}");
    }
}

#[test]
fn dashboard_numbers_are_the_shared_report_numbers() {
    let app = app();
    let expected = report::build(
        &dataset(),
        &app.query_context(),
        report::ReportKind::Overview,
    );
    let m = &expected.metrics;
    assert_eq!(app.base_report().unwrap().metrics, *m);
    let lines = render(&app, 120, 30);
    for (label, value) in [
        ("Commits", m.commits),
        ("PRs created", m.prs_created),
        ("PRs merged", m.prs_merged),
        ("Reviews submitted", m.reviews_submitted.total),
        ("Reviews received", m.reviews_received.total),
        ("Comments written", m.comments_written.total()),
        ("Issues opened", m.issues_opened),
        ("Active days", m.active_days),
    ] {
        let line = lines
            .iter()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no {label}"));
        assert_eq!(number_after(line, label), value, "{label}");
    }
    // Hand check against the test world: c1, c2, c3; P1 and P3 opened;
    // P1 merged; one review given (P2) and one received (P1).
    assert_eq!(
        (m.commits, m.prs_created, m.prs_merged),
        (3, 2, 1),
        "commits / created / merged"
    );
    assert_eq!(
        (m.reviews_submitted.total, m.reviews_received.total),
        (1, 1)
    );
}

#[test]
fn dashboard_lists_pull_requests_most_recently_active_first() {
    let app = app();
    let prs = app.base_report().unwrap().pull_requests.as_ref().unwrap();
    let ids: Vec<&str> = prs.iter().map(|p| p.pr_id.as_str()).collect();
    // Last human events: P3 11-18, P2 11-13, P1 11-12.
    assert_eq!(ids, vec!["P3", "P2", "P1"]);
}

#[test]
fn dashboard_fits_small_terminals() {
    let mut app = app_with(with_issue(), false);
    // 80x24: everything is shown, with short tab titles.
    let text = screen_text(&app, 80, 24);
    for needle in [
        "[1 Dash]",
        "Commits",
        "Activity per day",
        "Top repositories",
        "Recently active",
        "coverage",
    ] {
        assert!(text.contains(needle), "80x24 lacks {needle:?}:\n{text}");
    }
    // Narrower and lower: the lists stack, then the chart and the lists
    // give way to the figures.
    let text = screen_text(&app, 60, 20);
    assert!(text.contains("Commits"), "{text}");
    let text = screen_text(&app, 40, 12);
    assert!(text.contains("Commits"), "{text}");
    assert!(!text.contains("Top repositories"), "{text}");
    for (w, h) in [(80, 24), (60, 20), (40, 12), (30, 8), (20, 6), (10, 3)] {
        assert_clean(&app, w, h);
        for _ in 0..3 {
            tab(&mut app);
            assert_clean(&app, w, h);
        }
    }
}

#[test]
fn dashboard_panes_open_repository_and_pull_request_details() {
    let mut app = app();
    assert_eq!(app.dashboard_pane, DashboardPane::Summary);
    enter(&mut app);
    assert_eq!(app.screen, Screen::Dashboard, "nothing to open on figures");

    tab(&mut app);
    assert_eq!(app.dashboard_pane, DashboardPane::Repositories);
    enter(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);
    assert_eq!(app.tab(), Screen::Dashboard);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Dashboard);

    tab(&mut app);
    assert_eq!(app.dashboard_pane, DashboardPane::PullRequests);
    app.handle_key(key('j'));
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.detail_pr.as_deref(), Some("P2"));
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("[1 Dashboard]"), "{text}");
    assert!(text.contains("› PR Detail"), "{text}");
    esc(&mut app);
    assert_eq!(app.screen, Screen::Dashboard);
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.dashboard_pane, DashboardPane::Repositories);
}

// ----- Day detail --------------------------------------------------------------

#[test]
fn enter_on_a_day_opens_its_activity() {
    let mut app = app();
    press(&mut app, "3");
    search(&mut app, "2026-11-10");
    enter(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    assert_eq!(app.detail_day.as_ref().unwrap().label, "2026-11-10");
    assert_eq!(app.detail_tab, DetailTab::Activity);

    // 11-10 in Tokyo: P1 opened 10:00, commit aaa2 and its PR commit 12:00.
    let r = app.detail_report().unwrap();
    let types: Vec<EventType> = r
        .activity
        .as_ref()
        .unwrap()
        .iter()
        .map(|a| a.event_type)
        .collect();
    assert_eq!(
        types,
        vec![
            EventType::PrOpened,
            EventType::CommitAuthored,
            EventType::PrCommit
        ]
    );
    assert_eq!(app.visible(ListId::DetailActivity).len(), 3);
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("Day 2026-11-10"), "{text}");
    assert!(text.contains("[Activity (3)]"), "{text}");
    assert!(text.contains("Commit"), "{text}");

    tab(&mut app);
    assert_eq!(app.detail_tab, DetailTab::Commits);
    assert_eq!(app.visible(ListId::DetailCommits).len(), 1);
    assert!(screen_text(&app, 140, 30).contains("aaa2"));
    enter(&mut app);
    assert_eq!(
        app.screen,
        Screen::CommitDetail,
        "commits open the commit screen"
    );
    assert_eq!(app.detail_commit.as_ref().unwrap().sha, "aaa2");
    esc(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    assert_eq!(app.detail_tab, DetailTab::Commits);

    tab(&mut app);
    assert_eq!(app.detail_tab, DetailTab::PullRequests);
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.detail_pr.as_deref(), Some("P1"));
    esc(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Daily);
}

#[test]
fn day_detail_numbers_match_the_daily_row_and_the_shared_report() {
    let mut app = app();
    press(&mut app, "3");
    search(&mut app, "2026-11-13");
    let row = app
        .base_report()
        .unwrap()
        .activity_breakdown
        .iter()
        .find(|b| b.bucket.label == "2026-11-13")
        .unwrap()
        .clone();
    enter(&mut app);
    let shown = app.detail_report().unwrap().clone();
    assert_eq!(shown.metrics, row.summary);

    let mut ctx = app.query_context();
    ctx.period = Period::new(row.bucket.start, row.bucket.end).unwrap();
    let expected = report::build_detail(&dataset(), &ctx);
    assert_eq!(shown.metrics, expected.metrics);
    assert_eq!(shown.activity, expected.activity);
    assert_eq!(shown.reviews_submitted, expected.reviews_submitted);

    // The review given on 11-13 leads to its pull request.
    while app.detail_tab != DetailTab::Reviews {
        tab(&mut app);
    }
    assert_eq!(app.detail_reviews().len(), 1);
    assert_eq!(app.detail_reviews()[0].0, ReviewDirection::Submitted);
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.detail_pr.as_deref(), Some("P2"));
    assert_clean(&app, 120, 30);
}

#[test]
fn changing_the_time_range_leaves_a_day_detail() {
    let mut app = app();
    press(&mut app, "3");
    enter(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    app.handle_key(key('t'));
    assert_eq!(app.screen, Screen::Daily);
    // Repository details survive a new range; their numbers follow it.
    let mut app = common::app();
    press(&mut app, "4");
    enter(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);
    app.handle_key(key('t'));
    assert_eq!(app.screen, Screen::RepoDetail);
    assert_eq!(app.detail_report().unwrap().metrics.commits, 0, "today");
}

// ----- Repository detail ----------------------------------------------------------

#[test]
fn repository_detail_shows_its_counts_pull_requests_and_commits() {
    let mut app = app();
    press(&mut app, "4");
    let row = app.base_report().unwrap().repositories[0].clone();
    assert_eq!(row.full_name, "alice/parser");
    enter(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);
    let r = app.detail_report().unwrap();
    assert_eq!(r.metrics, row.summary, "same numbers as the repository row");
    assert_eq!(r.metrics.commits, 2);

    assert_eq!(app.detail_tab, DetailTab::PullRequests);
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("Repository alice/parser"), "{text}");
    assert!(text.contains("[Pull requests (1)]"), "{text}");
    assert!(text.contains("Commits (2)"), "{text}");

    tab(&mut app);
    assert_eq!(app.detail_tab, DetailTab::Commits);
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("aaa1") && text.contains("aaa2"), "{text}");
    // Search works inside detail sections.
    search(&mut app, "aaa1");
    assert_eq!(app.visible(ListId::DetailCommits).len(), 1);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Repositories);

    // `o` opens the repository when a section is empty.
    press(&mut app, "4");
    enter(&mut app);
    while app.detail_tab != DetailTab::Issues {
        tab(&mut app);
    }
    assert_eq!(
        app.selected_url().as_deref(),
        Some("https://github.com/alice/parser")
    );
}

#[test]
fn overview_repositories_open_the_repository_detail() {
    let mut app = app();
    press(&mut app, "2");
    tab(&mut app);
    enter(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Overview);
}

// ----- Reviews and issues -------------------------------------------------------------

#[test]
fn enter_on_a_review_opens_its_pull_request() {
    let mut app = app();
    press(&mut app, "6");
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.detail_pr.as_deref(), Some("P2"));
    assert_eq!(app.tab(), Screen::Reviews);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Reviews);
    tab(&mut app);
    enter(&mut app);
    assert_eq!(app.detail_pr.as_deref(), Some("P1"));
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrTimeline);
    esc(&mut app);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Reviews);
}

#[test]
fn issues_list_and_issue_detail() {
    let mut app = app_with(with_issue(), false);
    press(&mut app, "7");
    assert_eq!(app.screen, Screen::Issues);
    // Only alice's own issue events count: she opened it, bob closed it.
    let rows = app.issues_report().unwrap().issues.clone().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].opened_in_period);
    assert_eq!(rows[0].closed_in_period, 0);
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("alice/parser#42"), "{text}");
    assert!(text.contains("バ"), "{text}");
    assert_clean(&app, 140, 30);
    assert_clean(&app, 40, 10);

    enter(&mut app);
    assert_eq!(app.screen, Screen::IssueDetail);
    let r = app.issue_report().unwrap();
    assert_eq!(r.number, 42);
    let who: Vec<&str> = r.events.iter().map(|e| e.actor.as_str()).collect();
    assert_eq!(who, vec!["alice", "bob"]);
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("Issue Detail"), "{text}");
    assert!(text.contains("closed"), "{text}");
    assert!(text.contains("alice (you)"), "{text}");
    assert_clean(&app, 140, 30);
    assert_clean(&app, 30, 8);
    assert_eq!(
        app.selected_url().as_deref(),
        Some("https://github.com/alice/parser/issues/42")
    );
    esc(&mut app);
    assert_eq!(app.screen, Screen::Issues);
}

#[test]
fn day_activity_opens_issues_too() {
    let mut app = app_with(with_issue(), false);
    press(&mut app, "3");
    search(&mut app, "2026-11-15");
    enter(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    assert_eq!(app.visible(ListId::DetailActivity).len(), 1);
    enter(&mut app);
    assert_eq!(app.screen, Screen::IssueDetail);
    assert_eq!(app.detail_issue.as_deref(), Some("I1"));
    let text = screen_text(&app, 160, 30);
    assert!(text.contains("› Day Detail › Issue Detail"), "{text}");
    // Narrower: the breadcrumb uses short titles.
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("› Day › Issue"), "{text}");
    esc(&mut app);
    esc(&mut app);
    assert_eq!(app.screen, Screen::Daily);
}

// ----- Help and hints ------------------------------------------------------------------

#[test]
fn help_and_status_bar_describe_the_new_keys() {
    let mut app = app();
    app.handle_key(key('?'));
    let text = screen_text(&app, 140, 30);
    assert!(text.contains("1-8"), "{text}");
    assert!(text.contains("Dashboard / Overview"), "{text}");
    app.handle_key(key('?'));
    press(&mut app, "3");
    let status = render(&app, 160, 20).pop().unwrap();
    assert!(status.contains("Enter day detail"), "{status}");
    // Narrow terminals keep the essentials.
    let status = render(&app, 80, 20).pop().unwrap();
    assert!(status.contains("? help"), "{status}");
}

#[test]
fn every_new_screen_renders_at_many_sizes_without_control_characters() {
    let mut app = app_with(with_issue(), false);
    let visit = |app: &mut App| {
        for (w, h) in [(160, 40), (80, 24), (60, 16), (40, 10), (20, 6), (1, 1)] {
            assert_clean(app, w, h);
        }
    };
    visit(&mut app);
    press(&mut app, "3");
    enter(&mut app);
    for _ in 0..6 {
        visit(&mut app);
        tab(&mut app);
    }
    press(&mut app, "4");
    enter(&mut app);
    for _ in 0..6 {
        visit(&mut app);
        tab(&mut app);
    }
    press(&mut app, "7");
    visit(&mut app);
    enter(&mut app);
    visit(&mut app);
}

#[test]
fn detail_screens_use_the_worker_and_cache_their_reports() {
    let mut app = threaded_app(Box::new(ghusage_tui::source::MemorySource::new(dataset())));
    assert!(app.settle(SETTLE));
    let loads = app.dataset_loads();
    press(&mut app, "3");
    enter(&mut app);
    assert!(app.settle(SETTLE));
    assert!(app.detail_report().is_some());
    // The day is computed from the period's dataset; nothing is re-read.
    assert_eq!(app.dataset_loads(), loads);
    let builds = app.report_builds();
    esc(&mut app);
    enter(&mut app);
    assert!(app.settle(SETTLE));
    assert_eq!(app.report_builds(), builds, "cached");
}
