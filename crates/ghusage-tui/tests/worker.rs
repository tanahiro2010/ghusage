//! Reports are computed on a worker thread: the UI keeps responding while
//! they are computed, and results made stale by a reload or a newer request
//! are thrown away.

#[expect(
    dead_code,
    reason = "Shared fixtures are compiled separately for each integration test target, and this target uses only a subset."
)]
mod common;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Period;
use ghusage_tui::app::{Screen, TimeRange};
use ghusage_tui::source::{DataSource, MemorySource, SystemInfo};
use ratatui::crossterm::event::KeyCode;

/// A cache whose period reads wait until the test lets them through.
struct GatedSource {
    data: Arc<Mutex<Dataset>>,
    gate: Mutex<Receiver<()>>,
    started: Mutex<Sender<()>>,
}

impl DataSource for GatedSource {
    fn dataset(&self, _period: Period) -> Result<Dataset, String> {
        let snapshot = self.data.lock().unwrap().clone();
        let _ = self.started.lock().unwrap().send(());
        let _ = self.gate.lock().unwrap().recv();
        Ok(snapshot)
    }
    fn pr_dataset(&self, _pr_id: &str) -> Result<Dataset, String> {
        Ok(self.data.lock().unwrap().clone())
    }
    fn system_info(&self) -> SystemInfo {
        SystemInfo {
            login: "alice".into(),
            ..SystemInfo::default()
        }
    }
}

struct Gate {
    data: Arc<Mutex<Dataset>>,
    open: Sender<()>,
    started: Receiver<()>,
}

fn gated() -> (GatedSource, Gate) {
    let data = Arc::new(Mutex::new(dataset()));
    let (open, gate) = mpsc::channel();
    let (started_tx, started) = mpsc::channel();
    (
        GatedSource {
            data: data.clone(),
            gate: Mutex::new(gate),
            started: Mutex::new(started_tx),
        },
        Gate {
            data,
            open,
            started,
        },
    )
}

#[test]
fn ui_responds_and_shows_a_placeholder_while_a_report_is_computed() {
    let (source, gate) = gated();
    let mut app = threaded_app(Box::new(source));
    gate.started.recv().unwrap();
    assert!(app.is_computing());

    let text = screen_text(&app, 120, 30);
    assert!(text.contains("Computing…"), "{text}");
    assert!(text.contains("⧗ computing"), "{text}");

    for k in ['3', '4', 'j', '?', '?', '8', '2'] {
        let t = Instant::now();
        app.handle_key(key(k));
        app.poll_results();
        drop(render(&app, 120, 30));
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "key {k} took {:?}",
            t.elapsed()
        );
    }
    assert_eq!(app.screen, Screen::Overview);

    gate.open.send(()).unwrap();
    assert!(app.settle(SETTLE));
    assert_eq!(app.base_report().unwrap().metrics.commits, 3);
    assert!(!screen_text(&app, 120, 30).contains("Computing"));
}

#[test]
fn results_computed_before_a_reload_are_discarded() {
    let (source, gate) = gated();
    let mut app = threaded_app(Box::new(source));
    // The first computation has read the old data and is waiting.
    gate.started.recv().unwrap();

    // New data arrives in the cache, then the user reloads.
    *gate.data.lock().unwrap() = empty_dataset();
    app.handle_key(key('r'));
    for _ in 0..2 {
        gate.open.send(()).unwrap();
    }
    assert!(app.settle(SETTLE));
    assert_eq!(
        app.report_builds(),
        1,
        "only the report from the reloaded data is used"
    );
    assert_eq!(app.base_report().unwrap().metrics.commits, 0);
}

#[test]
fn superseded_requests_are_skipped_when_cycling_quickly() {
    let (source, gate) = gated();
    let mut app = threaded_app(Box::new(source));
    gate.started.recv().unwrap();

    // Two quick presses while the first report is still being computed.
    app.handle_key(key('t'));
    app.handle_key(key('t'));
    assert_eq!(app.time_range, TimeRange::Last7Days);
    for _ in 0..5 {
        gate.open.send(()).unwrap();
    }
    assert!(app.settle(SETTLE));
    assert_eq!(app.base_report().unwrap().metrics.commits, 1);
    assert_eq!(
        app.dataset_loads(),
        2,
        "the start period and the last 7 days; today was skipped"
    );
}

#[test]
fn background_computation_shows_the_same_numbers_as_inline() {
    let mut threaded = threaded_app(Box::new(MemorySource::new(dataset())));
    let mut inline = app();
    for k in ['1', '2', '3', '4', '5', '6', '7'] {
        threaded.handle_key(key(k));
        inline.handle_key(key(k));
        assert!(threaded.settle(SETTLE));
        assert_eq!(
            threaded.current_report().unwrap().metrics,
            inline.current_report().unwrap().metrics
        );
    }
    assert_eq!(
        threaded.reviews_report().unwrap().reviews_received,
        inline.reviews_report().unwrap().reviews_received
    );
    threaded.handle_key(key('5'));
    threaded.handle_key(code(KeyCode::Enter));
    assert!(threaded.settle(SETTLE));
    assert_eq!(threaded.screen, Screen::PrDetail);
    assert!(threaded.pr_report().is_some());
    threaded.handle_key(code(KeyCode::Enter));
    assert_eq!(threaded.screen, Screen::PrTimeline);
}

#[test]
fn warm_up_computes_other_screens_in_the_background() {
    let mut app = threaded_app(Box::new(MemorySource::new(dataset())));
    assert!(app.settle(SETTLE));
    assert!(app.warm_up());
    assert!(app.settle(SETTLE));
    let builds = app.report_builds();
    for k in ['1', '2', '3', '4', '5', '6', '7', '8'] {
        app.handle_key(key(k));
        assert!(!app.is_computing(), "screen {k} was precomputed");
    }
    assert_eq!(app.report_builds(), builds);
}
