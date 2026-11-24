//! Messages from the background sync update the UI without blocking it.

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use ghusage_tui::app::Screen;
use ghusage_tui::background::SyncMsg;

#[test]
fn progress_and_completion_update_the_status_bar() {
    let mut app = app_with(dataset(), true);
    app.start_sync();
    assert!(app.sync.running);
    let (tx, rx) = mpsc::channel();
    tx.send(SyncMsg::Started {
        auth: Some("GH_TOKEN".into()),
    })
    .unwrap();
    tx.send(SyncMsg::Progress("fetching pull_requests 3/10".into()))
        .unwrap();
    while let Ok(m) = rx.try_recv() {
        app.on_sync(m);
    }
    let status = render(&app, 160, 20).pop().unwrap();
    assert!(
        status.contains("sync: fetching pull_requests 3/10"),
        "{status}"
    );

    // The UI keeps responding while the sync runs.
    app.handle_key(key('5'));
    assert_eq!(app.screen, Screen::PullRequests);

    let loads = app.dataset_loads();
    tx.send(SyncMsg::RateLimit("core 4990/5000".into()))
        .unwrap();
    tx.send(SyncMsg::Finished {
        complete: false,
        failures: vec!["P9: \u{1b}[2Jtimeline unavailable".into()],
        summary: "partial".into(),
    })
    .unwrap();
    while let Ok(m) = rx.try_recv() {
        app.on_sync(m);
    }
    assert!(!app.sync.running);
    assert!(
        app.dataset_loads() > loads,
        "finished sync reloads the cache"
    );
    let status = render(&app, 160, 20).pop().unwrap();
    assert!(status.contains("sync incomplete (1 warnings)"), "{status}");

    app.handle_key(key('8'));
    let text = screen_text(&app, 160, 40);
    assert!(text.contains("token from GH_TOKEN"), "{text}");
    assert!(text.contains("core 4990/5000"), "{text}");
    assert!(text.contains("timeline unavailable"), "{text}");
    assert_clean(&app, 160, 40);
}

#[test]
fn network_loss_is_shown_and_cached_data_stays() {
    let mut app = app_with(dataset(), true);
    app.start_sync();
    app.on_sync(SyncMsg::Failed(
        "error sending request: connection reset".into(),
    ));
    assert!(!app.sync.running);
    let status = render(&app, 200, 20).pop().unwrap();
    assert!(
        status.contains("! sync failed: error sending request"),
        "{status}"
    );
    assert_eq!(app.base_report().unwrap().metrics.commits, 3);
    app.handle_key(key('8'));
    assert!(screen_text(&app, 160, 40).contains("connection reset"));
    // A new sync can be started afterwards.
    assert!(app.start_sync().is_some());
}

#[test]
fn draining_messages_never_blocks() {
    let mut app = app_with(dataset(), true);
    let (tx, rx) = mpsc::channel::<SyncMsg>();
    let started = Instant::now();
    for i in 0..1000 {
        tx.send(SyncMsg::Progress(format!("fetching commits {i}/1000")))
            .unwrap();
    }
    while let Ok(m) = rx.try_recv() {
        app.on_sync(m);
    }
    // An empty channel returns immediately.
    assert!(rx.try_recv().is_err());
    drop(render(&app, 120, 30));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        app.sync.progress.as_deref(),
        Some("fetching commits 999/1000")
    );
}
