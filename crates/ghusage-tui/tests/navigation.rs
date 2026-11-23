//! Key presses and the state changes they cause.

mod common;

use common::*;
use ghusage_core::metrics::{Ownership, ReviewDirection, VisibilityFilter};
use ghusage_core::time::resolve_period;
use ghusage_tui::app::{
    DetailTab, Effect, FilterPreset, ListId, OverviewPane, Screen, TimeRange, TimelineFilter,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[test]
fn number_keys_switch_screens() {
    let mut app = app();
    assert_eq!(app.screen, Screen::Dashboard, "the dashboard comes first");
    let expected = [
        ('2', Screen::Overview),
        ('3', Screen::Daily),
        ('4', Screen::Repositories),
        ('5', Screen::PullRequests),
        ('6', Screen::Reviews),
        ('7', Screen::Issues),
        ('8', Screen::System),
        ('1', Screen::Dashboard),
    ];
    for (k, s) in expected {
        app.handle_key(key(k));
        assert_eq!(app.screen, s, "key {k}");
    }
}

#[test]
fn enter_opens_pr_detail_then_timeline_and_esc_goes_back() {
    let mut app = app();
    press(&mut app, "5");
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::PrDetail);
    let id = app.detail_pr.clone().unwrap();
    assert_eq!(app.pr_report().unwrap().pr_id, id);
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::PrTimeline);
    assert_ne!(app.visible(ListId::Timeline), Vec::<usize>::new());
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.screen, Screen::PrDetail);
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.screen, Screen::PullRequests);
}

#[test]
fn pr_detail_shows_counts_of_the_selected_pull_request() {
    let mut app = app();
    press(&mut app, "5");
    app.handle_key(key('/'));
    press(&mut app, "パーサー");
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(code(KeyCode::Enter));
    let r = app.pr_report().unwrap();
    assert_eq!(r.number, 1);
    assert_eq!(r.commits, 1);
    assert_eq!(r.reviews.total, 1);
    assert_eq!(r.comments.inline, 1);
    let text = screen_text(&app, 140, 40);
    assert!(text.contains("merged"), "{text}");
    assert!(text.contains("Review threads"), "{text}");
}

#[test]
fn selection_moves_and_stops_at_the_ends() {
    let mut app = app();
    press(&mut app, "4");
    let n = app.visible(ListId::Repositories).len();
    assert_eq!(n, 3);
    assert_eq!(app.selected(ListId::Repositories), 0);
    app.handle_key(key('k'));
    assert_eq!(
        app.selected(ListId::Repositories),
        0,
        "no move above the first row"
    );
    app.handle_key(key('j'));
    app.handle_key(code(KeyCode::Down));
    assert_eq!(app.selected(ListId::Repositories), 2);
    app.handle_key(key('j'));
    assert_eq!(
        app.selected(ListId::Repositories),
        2,
        "no move past the last row"
    );
    app.handle_key(code(KeyCode::Up));
    assert_eq!(app.selected(ListId::Repositories), 1);
    app.handle_key(key('G'));
    assert_eq!(app.selected(ListId::Repositories), 2);
    app.handle_key(key('g'));
    assert_eq!(app.selected(ListId::Repositories), 0);
}

#[test]
fn moving_the_selection_does_not_recompute_reports() {
    let mut app = app();
    press(&mut app, "5");
    let builds = app.report_builds();
    let loads = app.dataset_loads();
    for _ in 0..20 {
        app.handle_key(key('j'));
        app.handle_key(key('k'));
        let _ = render(&app, 120, 30);
    }
    assert_eq!(app.report_builds(), builds);
    assert_eq!(app.dataset_loads(), loads);
    // Going back to a visited screen uses the cached report.
    press(&mut app, "25");
    let after = app.report_builds();
    press(&mut app, "25");
    assert_eq!(app.report_builds(), after);
}

#[test]
fn search_filters_lists_incrementally() {
    let mut app = app();
    press(&mut app, "4");
    app.handle_key(key('/'));
    assert!(app.search_mode);
    press(&mut app, "ウェブ");
    assert_eq!(app.visible(ListId::Repositories).len(), 1);
    app.handle_key(code(KeyCode::Backspace));
    app.handle_key(code(KeyCode::Backspace));
    app.handle_key(code(KeyCode::Backspace));
    assert_eq!(app.visible(ListId::Repositories).len(), 3);
    // Keys typed while searching do not act as commands.
    press(&mut app, "alice");
    assert!(!app.should_quit);
    assert_eq!(app.screen, Screen::Repositories);
    assert_eq!(app.visible(ListId::Repositories).len(), 2);
    app.handle_key(code(KeyCode::Enter));
    assert!(!app.search_mode);
    assert_eq!(app.search_query(ListId::Repositories), "alice");
    assert!(screen_text(&app, 120, 20).contains("2 shown of 3"));
    // Esc clears a kept search.
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.visible(ListId::Repositories).len(), 3);

    // Search while typing, then Esc cancels.
    app.handle_key(key('/'));
    press(&mut app, "zzz");
    assert_eq!(app.visible(ListId::Repositories), Vec::<usize>::new());
    assert!(screen_text(&app, 120, 20).contains("No matches"));
    app.handle_key(code(KeyCode::Esc));
    assert!(!app.search_mode);
    assert_eq!(app.visible(ListId::Repositories).len(), 3);
}

#[test]
fn search_matches_pull_request_titles_and_reviewers() {
    let mut app = app();
    press(&mut app, "5");
    app.handle_key(key('/'));
    press(&mut app, "PRIVATE");
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.visible(ListId::PullRequests).len(), 1);

    press(&mut app, "6");
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.review_direction, ReviewDirection::Received);
    app.handle_key(key('/'));
    press(&mut app, "bob");
    assert_eq!(app.visible(ListId::ReviewsReceived).len(), 1);
    app.handle_key(code(KeyCode::Backspace));
    app.handle_key(code(KeyCode::Backspace));
    app.handle_key(code(KeyCode::Backspace));
    press(&mut app, "nobody");
    assert_eq!(app.visible(ListId::ReviewsReceived), Vec::<usize>::new());
}

#[test]
fn time_range_cycles_and_recomputes_the_period() {
    let mut app = app();
    assert_eq!(app.time_range, TimeRange::Initial);
    let before = app.base_report().unwrap().metrics.commits;
    assert_eq!(before, 3);
    let builds = app.report_builds();

    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Today);
    let today = resolve_period(Some("today"), None, "today", &zone(), now()).unwrap();
    assert_eq!(app.period, today);
    assert!(app.report_builds() > builds, "new period is computed");
    assert_eq!(app.base_report().unwrap().metrics.commits, 0);

    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Last7Days);
    assert_eq!(app.base_report().unwrap().metrics.commits, 1);
    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Last30Days);
    assert_eq!(app.base_report().unwrap().metrics.commits, 3);
    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Last90Days);
    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::ThisMonth);
    let month = resolve_period(Some("1m"), None, "1m", &zone(), now()).unwrap();
    assert_eq!(app.period, month);
    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Initial);
    assert_eq!(app.period, period_30d());
    assert!(screen_text(&app, 160, 20).contains("range: as started"));
}

#[test]
fn filter_cycles_through_repository_presets() {
    let mut app = app();
    let order = [
        FilterPreset::Public,
        FilterPreset::Private,
        FilterPreset::Owned,
        FilterPreset::External,
        FilterPreset::All,
    ];
    let mut commits = Vec::new();
    for want in order {
        app.handle_key(key('f'));
        assert_eq!(app.filter_preset, want);
        commits.push(app.base_report().unwrap().metrics.commits);
    }
    // public: R1 (2 commits); private: R3 (1); owned: R1+R3 (3); external: R2 (0).
    assert_eq!(commits, vec![2, 1, 3, 0, 3]);
    assert_eq!(
        FilterPreset::Public.apply(&Default::default()).visibility,
        Some(VisibilityFilter::Public)
    );
    assert_eq!(
        FilterPreset::External.apply(&Default::default()).ownership,
        Some(Ownership::External)
    );
}

#[test]
fn help_toggles_and_blocks_other_keys() {
    let mut app = app();
    app.handle_key(key('?'));
    assert!(app.show_help);
    app.handle_key(key('4'));
    assert_eq!(app.screen, Screen::Dashboard, "keys are ignored under help");
    app.handle_key(key('?'));
    assert!(!app.show_help);
    app.handle_key(key('?'));
    app.handle_key(code(KeyCode::Esc));
    assert!(!app.show_help);
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut app = app();
    assert_eq!(app.handle_key(key('q')), Some(Effect::Quit));
    assert!(app.should_quit);
    let mut app = common::app();
    let e = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(e, Some(Effect::Quit));
}

#[test]
fn key_releases_are_ignored() {
    let mut app = app();
    let mut k = key('4');
    k.kind = KeyEventKind::Release;
    app.handle_key(k);
    assert_eq!(app.screen, Screen::Dashboard);
}

#[test]
fn tab_switches_panes() {
    let mut app = app();
    press(&mut app, "2");
    assert_eq!(app.overview_pane, OverviewPane::Summary);
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.overview_pane, OverviewPane::Repositories);
    app.handle_key(key('j'));
    assert_eq!(app.selected(ListId::OverviewRepos), 1);
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.overview_pane, OverviewPane::Summary);

    press(&mut app, "6");
    assert_eq!(app.review_direction, ReviewDirection::Submitted);
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.review_direction, ReviewDirection::Received);
    assert!(screen_text(&app, 120, 20).contains("[Received (1)]"));
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.review_direction, ReviewDirection::Submitted);
}

#[test]
fn timeline_event_type_filter_cycles_with_tab() {
    let mut app = app();
    press(&mut app, "5");
    app.handle_key(key('/'));
    press(&mut app, "#1");
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::PrTimeline);
    let all = app.visible(ListId::Timeline).len();
    assert_eq!(
        all, 6,
        "opened, commit, review, inline comment, check, merged"
    );
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.timeline_filter, TimelineFilter::Commits);
    assert_eq!(app.visible(ListId::Timeline).len(), 1);
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.timeline_filter, TimelineFilter::Reviews);
    assert_eq!(app.visible(ListId::Timeline).len(), 1);
    app.handle_key(code(KeyCode::BackTab));
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.timeline_filter, TimelineFilter::All);
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.timeline_filter, TimelineFilter::Checks);
    assert_eq!(app.visible(ListId::Timeline).len(), 1);
    assert!(screen_text(&app, 140, 30).contains("[CI]"));
}

#[test]
fn enter_on_a_repository_opens_its_detail_with_its_pull_requests() {
    let mut app = app();
    press(&mut app, "4");
    app.handle_key(key('/'));
    press(&mut app, "parser");
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.screen, Screen::RepoDetail);
    assert_eq!(app.detail_repo.as_ref().unwrap().full_name, "alice/parser");
    assert_eq!(app.detail_tab, DetailTab::PullRequests);
    assert_eq!(app.visible(ListId::DetailPullRequests).len(), 1);
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.screen, Screen::Repositories);
}

#[test]
fn open_validates_the_url_first() {
    let mut app = app();
    press(&mut app, "5");
    match app.handle_key(key('o')) {
        Some(Effect::OpenUrl(u)) => assert!(u.starts_with("https://github.com/"), "{u}"),
        other => panic!("expected an URL to open, got {other:?}"),
    }
    press(&mut app, "4");
    match app.handle_key(key('o')) {
        Some(Effect::OpenUrl(u)) => assert!(u.starts_with("https://github.com/"), "{u}"),
        other => panic!("expected an URL to open, got {other:?}"),
    }
    // Nothing selectable on the system screen.
    press(&mut app, "8");
    assert_eq!(app.handle_key(key('o')), None);
    assert!(app.message.is_some());
}

#[test]
fn open_refuses_urls_outside_github() {
    let mut ds = dataset();
    for pr in ds.pull_requests.values_mut() {
        pr.url = "javascript:alert(1)".into();
    }
    let mut app = app_with(ds, false);
    press(&mut app, "5");
    assert_eq!(app.handle_key(key('o')), None);
    assert!(app.message.as_deref().unwrap().contains("not opening"));
}

#[test]
fn refresh_reloads_from_cache_and_starts_sync_only_when_online() {
    let mut app = app();
    let loads = app.dataset_loads();
    assert_eq!(app.handle_key(key('r')), None);
    assert!(app.dataset_loads() > loads, "cache is read again");
    assert!(app.message.as_deref().unwrap().contains("offline"));

    let mut app = app_with(dataset(), true);
    match app.handle_key(key('r')) {
        Some(Effect::StartSync(p)) => assert_eq!(p, app.period),
        other => panic!("expected a sync, got {other:?}"),
    }
    assert!(app.sync.running);
    assert_eq!(app.handle_key(key('r')), None, "one sync at a time");
}

#[test]
fn idle_warm_up_makes_later_screen_switches_cache_hits() {
    let mut app = app();
    while app.warm_up() {}
    let builds = app.report_builds();
    press(&mut app, "12345678");
    assert_eq!(app.report_builds(), builds);
    assert!(!app.warm_up());
}
