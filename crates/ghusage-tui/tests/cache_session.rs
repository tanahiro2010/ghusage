//! The UI reading a real on-disk cache through an offline session.

mod common;

use common::*;
use ghusage_app::paths;
use ghusage_app::session::{Session, SessionConfig};
use ghusage_app::sync::{META_VIEWER_ID, META_VIEWER_LOGIN};
use ghusage_storage::{Store, WriteBatch};
use ghusage_tui::app::{App, Screen};
use ghusage_tui::source::SessionSource;
use ratatui::crossterm::event::KeyCode;

fn seeded_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let ds = dataset();
    let mut store = Store::open(&paths::db_path(dir.path())).unwrap();
    store.set_meta(META_VIEWER_LOGIN, "alice").unwrap();
    store.set_meta(META_VIEWER_ID, "U1").unwrap();
    store
        .apply(&WriteBatch {
            account: "alice".into(),
            repositories: ds.repositories.values().cloned().collect(),
            pull_requests: ds.pull_requests.values().cloned().collect(),
            events: ds.events.clone(),
            coverage: ds.coverage.clone(),
            ..WriteBatch::default()
        })
        .unwrap();
    dir
}

fn open_app(dir: &std::path::Path) -> App {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let session = rt
        .block_on(Session::open(SessionConfig {
            data_dir: Some(dir.to_path_buf()),
            offline: true,
            ..SessionConfig::default()
        }))
        .unwrap();
    let mut app = threaded_app(Box::new(SessionSource::new(session)));
    assert!(app.settle(SETTLE));
    app
}

fn hit(app: &mut App, k: ratatui::crossterm::event::KeyEvent) {
    app.handle_key(k);
    assert!(app.settle(SETTLE));
}

#[test]
fn cached_data_is_shown_with_the_same_numbers_as_in_memory() {
    let dir = seeded_dir();
    let mut from_disk = open_app(dir.path());
    let mut in_memory = app();
    assert_eq!(
        from_disk.base_report().unwrap().metrics,
        in_memory.base_report().unwrap().metrics
    );
    assert_eq!(from_disk.base_report().unwrap().metrics.commits, 3);

    for k in ['4', '5'] {
        hit(&mut from_disk, key(k));
        in_memory.handle_key(key(k));
    }
    assert_eq!(
        from_disk.reviews_report().unwrap().reviews_submitted,
        in_memory.reviews_report().unwrap().reviews_submitted
    );

    // PR detail is read from the cache by id.
    hit(&mut from_disk, key('4'));
    hit(&mut from_disk, code(KeyCode::Enter));
    assert_eq!(from_disk.screen, Screen::PrDetail);
    assert!(from_disk.pr_report().is_some());
    assert_clean(&from_disk, 120, 40);

    hit(&mut from_disk, key('6'));
    let text = screen_text(&from_disk, 140, 40);
    assert!(text.contains("alice (authenticated user)"), "{text}");
    assert!(text.contains("events"), "{text}");
    assert!(
        text.contains(&dir.path().display().to_string()[..20]),
        "{text}"
    );
}
