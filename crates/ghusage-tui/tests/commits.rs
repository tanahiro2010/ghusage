//! Commits on PR Detail, the commit screen and its diff.

mod common;

use std::fmt::Write as _;
use std::sync::Arc;

use common::*;
use ghusage_app::commit::CommitSource;
use ghusage_core::commit::{CommitDetail, CommitFile, CommitPerson, DiffStats, FileStatus};
use ghusage_core::metrics::Dataset;
use ghusage_core::model::{EventMeta, EventType, Visibility};
use ghusage_tui::app::{App, CommitState, DetailTab, Effect, PrPane, Screen};
use ghusage_tui::commit_view::{
    CommitFailure, CommitKey, CommitMsg, CommitView, MAX_FILE_LINES, RowKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;

const SHA_A: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const SHA_B: &str = "b1b2c3d4e5f60718293a4b5c6d7e8f9012345678";

/// The shared world plus two more commits in alice's pull request #1: one
/// with a headline, and one with a hostile headline that a force push
/// removed from the pull request.
fn world() -> Dataset {
    let mut ds = dataset();
    let mut a = event(
        "p1c-a",
        EventType::PrCommit,
        alice(),
        "R1",
        Some("P1"),
        utc("2026-11-10T04:00:00Z"),
        EventMeta {
            sha: Some(SHA_A.into()),
            message_headline: Some("fix: パーサーの修正".into()),
            ..EventMeta::default()
        },
    );
    a.url = Some(format!(
        "https://github.com/alice/parser/pull/1/commits/{SHA_A}"
    ));
    let mut b = event(
        "p1c-b",
        EventType::PrCommit,
        bob(),
        "R1",
        Some("P1"),
        utc("2026-11-10T05:00:00Z"),
        EventMeta {
            sha: Some(SHA_B.into()),
            message_headline: Some(format!("wip {MALICIOUS_TITLE}")),
            removed_observed_at: Some(utc("2026-11-11T00:00:00Z")),
            ..EventMeta::default()
        },
    );
    b.url = None;
    ds.events.push(a);
    ds.events.push(b);
    ghusage_core::metrics::sort_events(&mut ds.events);
    ds
}

fn enter(app: &mut App) -> Option<Effect> {
    app.handle_key(code(KeyCode::Enter))
}

fn esc(app: &mut App) {
    app.handle_key(code(KeyCode::Esc));
}

/// Pull Requests → PR #1 detail.
fn open_pr1(app: &mut App) {
    press(app, "5");
    app.handle_key(key('/'));
    press(app, "#1");
    enter(app);
    enter(app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.pr_report().unwrap().number, 1);
}

fn key_a() -> CommitKey {
    CommitKey {
        repository: "alice/parser".into(),
        sha: SHA_A.into(),
    }
}

fn file(name: &str, status: FileStatus, patch: Option<&str>, add: u64, del: u64) -> CommitFile {
    CommitFile {
        filename: name.into(),
        previous_filename: None,
        status,
        additions: add,
        deletions: del,
        changes: add + del,
        patch: patch.map(Into::into),
    }
}

/// A commit with two files with diffs and one binary file.
fn detail() -> CommitDetail {
    CommitDetail {
        repository: "alice/parser".into(),
        sha: SHA_A.into(),
        url: Some(format!("https://github.com/alice/parser/commit/{SHA_A}")),
        message: "fix: パーサーの修正\n\nEmpty input no longer panics.\nSecond body line.".into(),
        author: CommitPerson {
            name: Some("Alice Example".into()),
            login: Some("alice".into()),
            date: Some(utc("2026-11-10T03:00:00Z")),
        },
        committer: CommitPerson {
            name: Some("GitHub".into()),
            login: Some("web-flow".into()),
            date: Some(utc("2026-11-10T04:00:00Z")),
        },
        parents: vec!["c0ffee0000000000000000000000000000000000".into()],
        stats: DiffStats {
            additions: 4,
            deletions: 2,
            total: 6,
        },
        files: vec![
            file(
                "src/parser.rs",
                FileStatus::Modified,
                Some("@@ -1,3 +1,3 @@ fn parse()\n context line\n-removed line\n+added line"),
                1,
                1,
            ),
            file("logo.png", FileStatus::Added, None, 0, 0),
            {
                let mut f = file(
                    "src/lexer.rs",
                    FileStatus::Renamed,
                    Some("@@ -10,2 +10,4 @@\n keep\n-gone\n+new one\n+new two\n+new three"),
                    3,
                    1,
                );
                f.previous_filename = Some("src/tokens.rs".into());
                f
            },
        ],
        files_truncated: false,
        visibility: Visibility::Public,
        fetched_at: utc("2026-11-21T00:00:00Z"),
    }
}

fn view(d: &CommitDetail) -> Arc<CommitView> {
    Arc::new(CommitView::build(d, CommitSource::Github, zone()))
}

/// PR #1 detail, commits focused, the commit `SHA_A` opened and loaded.
fn commit_screen(d: CommitDetail) -> App {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    app.handle_key(code(KeyCode::Tab));
    press(&mut app, "j");
    let effect = enter(&mut app);
    assert_eq!(effect, Some(Effect::LoadCommit(key_a())));
    app.on_commit(CommitMsg {
        key: key_a(),
        result: Ok(view(&d)),
    });
    app
}

/// Text without spaces: wide characters occupy two cells, the second of
/// which reads as a space in the test buffer.
fn squash(s: &str) -> String {
    s.chars().filter(|c| *c != ' ').collect()
}

/// The foreground color of the first cell of the first line containing
/// `needle`.
fn color_of(app: &App, w: u16, h: u16, needle: &str) -> Color {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ghusage_tui::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    for y in 0..h {
        let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(byte) = line.find(needle) {
            let col =
                u16::try_from(line[..byte].chars().count()).expect("column fits terminal width");
            return buf[(col, y)].fg;
        }
    }
    panic!("{needle:?} not on screen:\n{}", screen_text(app, w, h));
}

// ----- PR Detail ---------------------------------------------------------------

#[test]
fn pr_detail_lists_commits_and_marks_removed_ones() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    let r = app.pr_report().unwrap();
    // aaa2 from the shared world, then the two added here; oldest first.
    let shas: Vec<&str> = r.commit_list.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, vec!["aaa2", SHA_A, SHA_B]);
    assert_eq!(
        r.commit_list.iter().map(|c| c.removed).collect::<Vec<_>>(),
        vec![false, false, true]
    );
    // Removed commits are listed but not counted.
    assert_eq!((r.commits, r.removed_commits), (2, 1));

    let text = screen_text(&app, 140, 45);
    assert!(text.contains("Commits (3, 1 removed)"), "{text}");
    assert!(text.contains("a1b2c3d"), "{text}");
    assert!(squash(&text).contains("fix:パーサーの修正"), "{text}");
    let line = text.lines().find(|l| l.contains("b1b2c3d")).unwrap();
    assert!(line.contains("removed"), "{line}");
    assert!(line.contains("bob"), "{line}");
    assert!(line.contains("red title"), "{line}");
    assert_clean(&app, 140, 45);
}

#[test]
fn tab_selects_commits_and_enter_opens_the_commit_then_esc_returns() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    assert_eq!(app.pr_pane, PrPane::Summary);
    app.handle_key(code(KeyCode::Tab));
    assert_eq!(app.pr_pane, PrPane::Commits);
    press(&mut app, "j");
    let effect = enter(&mut app);
    assert_eq!(app.screen, Screen::CommitDetail);
    assert_eq!(app.detail_commit, Some(key_a()));
    assert_eq!(effect, Some(Effect::LoadCommit(key_a())));
    assert!(matches!(app.commit_state(), Some(CommitState::Loading)));
    // What the cache already knows is shown while loading.
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("alice/parser@a1b2c3d"), "{text}");
    assert!(squash(&text).contains("fix:パーサーの修正"), "{text}");
    assert!(text.contains("Loading the commit"), "{text}");
    assert!(text.contains("› Commit"), "{text}");

    esc(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(app.pr_pane, PrPane::Commits, "focus is kept");
    // Opening it again while it loads does not ask twice.
    assert_eq!(enter(&mut app), None);
    assert_eq!(app.screen, Screen::CommitDetail);
    esc(&mut app);
    // Back on the summary, Enter still opens the timeline.
    app.handle_key(code(KeyCode::BackTab));
    assert_eq!(app.pr_pane, PrPane::Summary);
    enter(&mut app);
    assert_eq!(app.screen, Screen::PrTimeline);
    esc(&mut app);
    esc(&mut app);
    assert_eq!(app.screen, Screen::PullRequests);
}

#[test]
fn removed_commit_opens_with_its_removed_state() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    app.handle_key(code(KeyCode::Tab));
    press(&mut app, "G");
    enter(&mut app);
    assert_eq!(app.detail_commit.as_ref().unwrap().sha, SHA_B);
    assert!(app.commit_seed.removed);
    let text = screen_text(&app, 120, 30);
    assert!(
        text.contains("removed (no longer part of the pull request)"),
        "{text}"
    );
    assert_clean(&app, 120, 30);
}

#[test]
fn o_opens_the_selected_commit_or_the_pull_request() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    assert_eq!(
        app.handle_key(key('o')),
        Some(Effect::OpenUrl("https://github.com/x/y/pull/1".into()))
    );
    app.handle_key(code(KeyCode::Tab));
    press(&mut app, "j");
    assert_eq!(
        app.handle_key(key('o')),
        Some(Effect::OpenUrl(format!(
            "https://github.com/alice/parser/pull/1/commits/{SHA_A}"
        )))
    );
    // Without a URL in the cache the commit page is built from the SHA.
    press(&mut app, "j");
    assert_eq!(
        app.handle_key(key('o')),
        Some(Effect::OpenUrl(format!(
            "https://github.com/alice/parser/commit/{SHA_B}"
        )))
    );
}

#[test]
fn pr_commits_can_be_searched() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    app.handle_key(code(KeyCode::Tab));
    app.handle_key(key('/'));
    press(&mut app, "パーサー");
    enter(&mut app);
    assert_eq!(app.visible(ghusage_tui::app::ListId::PrCommits).len(), 1);
    enter(&mut app);
    assert_eq!(app.detail_commit, Some(key_a()));
}

// ----- Commit screen --------------------------------------------------------------

#[test]
fn commit_screen_shows_facts_message_files_and_diff() {
    let app = commit_screen(detail());
    assert_eq!(app.screen, Screen::CommitDetail);
    let text = screen_text(&app, 120, 60);
    for needle in [
        "alice/parser@a1b2c3d — fix: パーサーの修正",
        SHA_A,
        "Alice Example (alice)",
        "GitHub (web-flow)",
        "Author date",
        "Committer date",
        "2026-11-10 12:00:00 JST",
        "c0ffee0",
        "3 file(s), +4 −2",
        "Empty input no longer panics.",
        "Second body line.",
        "Files (3)",
        "src/tokens.rs → src/lexer.rs",
        "renamed",
        "binary file or diff too large",
        "── [1/3] modified src/parser.rs (+1 −1)",
        "@@ -1,3 +1,3 @@ fn parse()",
        "-removed line",
        "+added line",
    ] {
        assert!(
            squash(&text).contains(&squash(needle)),
            "missing {needle:?}:\n{text}"
        );
    }
    let total = text.lines().find(|l| l.contains("total")).unwrap();
    assert!(total.contains("+4") && total.contains("−2"), "{total}");
    assert!(!text.contains("merge commit"), "{text}");
}

#[test]
fn diff_lines_are_colored_by_kind() {
    let app = commit_screen(detail());
    assert_eq!(color_of(&app, 120, 60, "+added line"), Color::Green);
    assert_eq!(color_of(&app, 120, 60, "-removed line"), Color::Red);
    assert_eq!(color_of(&app, 120, 60, "@@ -1,3 +1,3 @@"), Color::Cyan);
    assert_eq!(color_of(&app, 120, 60, " context line"), Color::Reset);
}

#[test]
fn commit_screen_scrolls_and_jumps_between_files() {
    let mut app = commit_screen(detail());
    let rows = app.commit_view().unwrap().rows.len();
    let files = app.commit_view().unwrap().file_rows.clone();
    assert_eq!(files.len(), 3);
    // 12 rows tall: 10 rows of content fit.
    let h = 12 + 3;
    screen_text(&app, 100, h);
    let page = 10;
    press(&mut app, "j");
    assert_eq!(app.commit_scroll, 1);
    press(&mut app, "k");
    press(&mut app, "k");
    assert_eq!(app.commit_scroll, 0, "stops at the top");
    app.handle_key(code(KeyCode::PageDown));
    assert_eq!(app.commit_scroll, page);
    app.handle_key(code(KeyCode::PageUp));
    assert_eq!(app.commit_scroll, 0);
    press(&mut app, "G");
    assert_eq!(app.commit_scroll, rows - page, "last page");
    press(&mut app, "j");
    assert_eq!(app.commit_scroll, rows - page, "stops at the bottom");
    press(&mut app, "g");
    assert_eq!(app.commit_scroll, 0);

    press(&mut app, "]");
    assert_eq!(app.commit_scroll, files[0]);
    let text = screen_text(&app, 100, h);
    assert!(text.contains("file 1/3"), "{text}");
    assert!(
        text.lines()
            .nth(3)
            .unwrap()
            .contains("── [1/3] modified src/parser.rs"),
        "the file header is the first row:\n{text}"
    );
    press(&mut app, "]");
    assert_eq!(app.commit_scroll, files[1]);
    assert!(screen_text(&app, 100, h).contains("file 2/3"));
    press(&mut app, "[");
    assert_eq!(app.commit_scroll, files[0]);
    press(&mut app, "[");
    assert_eq!(app.commit_scroll, 0, "back before the first file");
    assert!(screen_text(&app, 100, h).contains("lines 1–10 of"));
}

#[test]
fn esc_leaves_the_commit_screen_and_reopening_keeps_the_loaded_commit() {
    let mut app = commit_screen(detail());
    esc(&mut app);
    assert_eq!(app.screen, Screen::PrDetail);
    assert_eq!(enter(&mut app), None, "already loaded");
    assert!(app.commit_view().is_some());
    assert_eq!(
        app.handle_key(key('o')),
        Some(Effect::OpenUrl(format!(
            "https://github.com/alice/parser/commit/{SHA_A}"
        )))
    );
}

#[test]
fn hostile_commit_content_never_reaches_the_terminal() {
    let mut d = detail();
    d.message = format!("{MALICIOUS_TITLE}\n\nbody \u{1b}]8;;https://evil\u{7}link\u{1b}]8;;\u{7}");
    d.author.name = Some(format!("evil {MALICIOUS_TITLE}"));
    d.files[0].filename = format!("src/{MALICIOUS_TITLE}.rs");
    d.files[0].patch = Some(format!(
        "@@ -1 +1 @@ {MALICIOUS_TITLE}\n-old \u{1b}[2J\u{1b}[H\n+new \u{9b}31m\u{7}\t\tindented"
    ));
    let mut app = commit_screen(d);
    for (w, h) in [(120, 60), (80, 24), (40, 10)] {
        assert_clean(&app, w, h);
    }
    let text = screen_text(&app, 120, 60);
    assert!(text.contains("red title"), "{text}");
    assert!(
        text.contains("+new         indented"),
        "tabs keep their width:\n{text}"
    );
    // Every scroll position is clean too.
    let rows = app.commit_view().unwrap().rows.len();
    for _ in 0..rows {
        press(&mut app, "j");
        assert_clean(&app, 80, 24);
    }
}

#[test]
fn small_terminals_show_the_commit_screens_without_breaking() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    let text = screen_text(&app, 80, 24);
    assert!(text.contains("PR Detail"), "{text}");
    assert!(text.contains("Commits (3, 1 removed)"), "{text}");
    app.handle_key(code(KeyCode::Tab));
    let text = screen_text(&app, 80, 24);
    assert!(text.contains("a1b2c3d"), "{text}");
    assert!(text.contains("removed"), "{text}");
    for (w, h) in [(80, 24), (60, 15), (40, 10), (20, 6)] {
        assert_clean(&app, w, h);
    }
    press(&mut app, "j");
    enter(&mut app);
    app.on_commit(CommitMsg {
        key: key_a(),
        result: Ok(view(&detail())),
    });
    let text = screen_text(&app, 80, 24);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 24);
    assert!(lines[2].contains("Commit a1b2c3d"), "{text}");
    assert!(text.contains("alice/parser@a1b2c3d"), "{text}");
    assert!(lines[23].contains("? help"), "status bar stays:\n{text}");
    for (w, h) in [(80, 24), (60, 15), (40, 10), (20, 6)] {
        assert_clean(&app, w, h);
    }
}

#[test]
fn offline_and_failed_loads_explain_themselves() {
    let mut app = app_with(world(), false);
    open_pr1(&mut app);
    app.handle_key(code(KeyCode::Tab));
    press(&mut app, "j");
    enter(&mut app);
    assert!(screen_text(&app, 120, 30).contains("⧗ loading commit"));
    app.on_commit(CommitMsg {
        key: key_a(),
        result: Err(CommitFailure {
            offline: true,
            message: "commit alice/parser@a1b2 is not in the cache (offline mode)".into(),
        }),
    });
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("Not available offline"), "{text}");
    assert!(
        squash(&text).contains("fix:パーサーの修正"),
        "cached facts stay:\n{text}"
    );
    assert!(!text.contains("⧗ loading commit"), "{text}");

    // r asks again.
    assert_eq!(app.handle_key(key('r')), Some(Effect::LoadCommit(key_a())));
    app.on_commit(CommitMsg {
        key: key_a(),
        result: Err(CommitFailure {
            offline: false,
            message: format!("access denied: {MALICIOUS_TITLE}"),
        }),
    });
    let text = screen_text(&app, 120, 30);
    assert!(text.contains("Could not load this commit"), "{text}");
    assert!(text.contains("access denied"), "{text}");
    assert_clean(&app, 120, 30);
    // Leaving and opening it again retries.
    esc(&mut app);
    assert_eq!(enter(&mut app), Some(Effect::LoadCommit(key_a())));
}

#[test]
fn large_diffs_are_cut_and_still_draw_quickly() {
    let mut d = detail();
    let mut patch = String::from("@@ -1,60000 +1,60000 @@\n");
    for i in 0..60_000 {
        let _ = writeln!(patch, "+line {i}");
    }
    d.files[0].patch = Some(patch.clone());
    d.files[0].additions = 60_000;
    d.files[2].patch = Some(patch);
    let v = view(&d);
    let shown = |file: usize| {
        let start = v.file_rows[file];
        let end = v.file_rows.get(file + 1).copied().unwrap_or(v.rows.len());
        v.rows[start..end]
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Added | RowKind::Hunk))
            .count()
    };
    assert_eq!(shown(0), MAX_FILE_LINES);
    assert_eq!(shown(2), MAX_FILE_LINES);
    assert_eq!(v.hidden_lines, 2 * (60_001 - MAX_FILE_LINES));
    assert!(
        v.rows
            .iter()
            .any(|r| r.text.contains("more line(s) not shown; press o"))
    );
    let mut app = commit_screen(d);
    let started = std::time::Instant::now();
    press(&mut app, "G");
    for _ in 0..20 {
        screen_text(&app, 160, 50);
        press(&mut app, "[");
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "drawing took {:?}",
        started.elapsed()
    );
}

// ----- Day and repository details ----------------------------------------------------

#[test]
fn commits_of_day_and_repository_details_open_the_commit_screen() {
    let mut app = app_with(world(), false);
    // Repository detail of alice/parser: its commits section.
    press(&mut app, "4");
    app.handle_key(key('/'));
    press(&mut app, "parser");
    enter(&mut app);
    enter(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);
    while app.detail_tab != DetailTab::Commits {
        app.handle_key(code(KeyCode::Tab));
    }
    let effect = enter(&mut app);
    assert_eq!(app.screen, Screen::CommitDetail);
    let k = app.detail_commit.clone().unwrap();
    assert_eq!(k.repository, "alice/parser");
    assert_eq!(effect, Some(Effect::LoadCommit(k)));
    assert_eq!(app.commit_seed.author.as_deref(), Some("alice"));
    esc(&mut app);
    assert_eq!(app.screen, Screen::RepoDetail);

    // Day detail of 2026-11-10.
    press(&mut app, "3");
    app.handle_key(key('/'));
    press(&mut app, "11-10");
    enter(&mut app);
    enter(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
    while app.detail_tab != DetailTab::Commits {
        app.handle_key(code(KeyCode::Tab));
    }
    enter(&mut app);
    assert_eq!(app.screen, Screen::CommitDetail);
    assert_eq!(app.detail_commit.as_ref().unwrap().sha, "aaa2");
    esc(&mut app);
    assert_eq!(app.screen, Screen::DayDetail);
}

// ----- Loading through the shared service ---------------------------------------------

fn seeded_cache() -> tempfile::TempDir {
    use ghusage_app::sync::{META_VIEWER_ID, META_VIEWER_LOGIN};
    use ghusage_storage::{Store, WriteBatch};
    let dir = tempfile::tempdir().unwrap();
    let ds = world();
    let mut store = Store::open(&ghusage_app::paths::db_path(dir.path())).unwrap();
    store.set_meta(META_VIEWER_LOGIN, "alice").unwrap();
    store.set_meta(META_VIEWER_ID, "U1").unwrap();
    store
        .apply(&WriteBatch {
            account: "alice".into(),
            repositories: ds.repositories.values().cloned().collect(),
            pull_requests: ds.pull_requests.values().cloned().collect(),
            events: ds.events.clone(),
            coverage: ds.coverage,
            ..WriteBatch::default()
        })
        .unwrap();
    dir
}

#[test]
fn offline_loader_reads_cached_commits_and_reports_missing_ones() {
    use ghusage_app::session::SessionConfig;
    use ghusage_tui::commit_fetch::Loader;
    let dir = seeded_cache();
    let cfg = SessionConfig {
        data_dir: Some(dir.path().to_path_buf()),
        offline: true,
        ..SessionConfig::default()
    };
    let mut loader = Loader::new(cfg, false, zone());
    let missing = loader.load(&key_a()).unwrap_err();
    assert!(missing.offline, "{missing:?}");
    assert!(
        missing.message.contains("not in the cache"),
        "{}",
        missing.message
    );

    let mut store = ghusage_storage::Store::open(&ghusage_app::paths::db_path(dir.path())).unwrap();
    store.put_commit_detail(&detail()).unwrap();
    drop(store);
    // Abbreviated SHAs find the cached commit as well.
    let v = loader
        .load(&CommitKey {
            repository: "alice/parser".into(),
            sha: "a1b2c3d".into(),
        })
        .unwrap();
    assert_eq!(v.sha, SHA_A);
    assert!(v.rows.iter().any(|r| r.text.contains("from the cache")));
    assert!(v.rows.iter().any(|r| r.text == "+added line"));

    // Invalid references are refused without touching anything.
    let bad = loader
        .load(&CommitKey {
            repository: "alice/parser".into(),
            sha: "zz".into(),
        })
        .unwrap_err();
    assert!(!bad.offline);
}

#[test]
fn background_loader_answers_over_a_channel() {
    use ghusage_app::session::SessionConfig;
    let dir = seeded_cache();
    let cfg = SessionConfig {
        data_dir: Some(dir.path().to_path_buf()),
        offline: true,
        ..SessionConfig::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let req = ghusage_tui::commit_fetch::spawn(cfg, false, zone(), tx).unwrap();
    req.send(key_a()).unwrap();
    let msg = rx.recv_timeout(SETTLE).unwrap();
    assert_eq!(msg.key, key_a());
    assert!(msg.result.unwrap_err().offline);
}
