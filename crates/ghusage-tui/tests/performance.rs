//! Timing of the first render and of key handling on a large cache.
//! Prints measurements; bounds are generous so slow machines pass.

mod common;

use std::time::{Duration, Instant};

use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::Dataset;
use ghusage_core::model::{
    Actor, ActorKind, EventMeta, EventType, PrState, PullRequest, Repository, Subject, Visibility,
};
use ghusage_tui::source::MemorySource;
use jiff::SignedDuration;
use ratatui::crossterm::event::KeyCode;

fn big_dataset(events: usize) -> Dataset {
    let p = period_30d();
    let repos: Vec<Repository> = (0..50)
        .map(|i| Repository {
            id: format!("R{i}"),
            owner: if i % 3 == 0 {
                "alice".into()
            } else {
                format!("org{i}")
            },
            name: format!("リポジトリ-{i}"),
            owner_kind: ActorKind::User,
            visibility: if i % 4 == 0 {
                Visibility::Private
            } else {
                Visibility::Public
            },
            is_fork: false,
            viewer_permission: None,
            accessible: true,
            url: None,
        })
        .collect();
    let prs: Vec<PullRequest> = (0..2000)
        .map(|i| PullRequest {
            id: format!("P{i}"),
            repository_id: format!("R{}", i % 50),
            number: i as u64 + 1,
            title: format!("機能追加 {i} 🚀"),
            url: format!("https://github.com/o/r/pull/{}", i + 1),
            author: if i % 2 == 0 { alice() } else { bob() },
            state: [PrState::Open, PrState::Merged, PrState::Closed][i % 3],
            is_draft: false,
            created_at: p.start,
            updated_at: None,
            closed_at: None,
            merged_at: None,
            merged_by: None,
            head_sha: None,
            latest_ci: None,
            coverage: CoverageStatus::Complete,
            synced_at: None,
        })
        .collect();
    let span = (p.end - p.start).as_secs();
    let types = [
        EventType::PrCommit,
        EventType::ReviewSubmitted,
        EventType::ConversationComment,
        EventType::InlineComment,
        EventType::Labeled,
        EventType::CommitAuthored,
    ];
    let evs = (0..events).map(|i| {
        let t = types[i % types.len()];
        let pr = i % 2000;
        let actor: Actor = if i % 3 == 0 { alice() } else { bob() };
        let at = p.start + SignedDuration::from_secs((i as i64 * 7919) % span);
        let meta = EventMeta {
            sha: Some(format!("{i:040x}")),
            ..EventMeta::default()
        };
        event(
            &format!("e{i}"),
            t,
            actor,
            &format!("R{}", pr % 50),
            (t != EventType::CommitAuthored)
                .then(|| format!("P{pr}"))
                .as_deref(),
            at,
            meta,
        )
    });
    Dataset::new(
        Subject {
            id: Some("U1".into()),
            login: "alice".into(),
        },
        true,
        repos,
        prs,
        vec![],
        vec![],
        evs.collect::<Vec<_>>(),
        vec![],
    )
}

fn percentile(samples: &mut [Duration], p: usize) -> Duration {
    samples.sort();
    samples[(samples.len() * p / 100).min(samples.len() - 1)]
}

/// One step of the event loop: apply finished results and redraw.
fn frame(app: &mut ghusage_tui::app::App) -> Duration {
    let t = Instant::now();
    app.poll_results();
    let _ = render(app, 160, 50);
    t.elapsed()
}

#[test]
fn first_render_and_interaction_latency_on_100k_events() {
    let source = MemorySource::new(big_dataset(100_000));
    let started = Instant::now();
    let mut app = threaded_app(Box::new(source));
    let _ = render(&app, 160, 50);
    let first_frame = started.elapsed();
    while app.base_report().is_none() {
        frame(&mut app);
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = render(&app, 160, 50);
    let first_data = started.elapsed();

    // First visits to every screen, PR detail and timeline, and time range
    // and filter cycling, each followed by first visits again. No warm-up,
    // so every new view is computed while the UI keeps running.
    let script: Vec<KeyCode> = "3456825"
        .chars()
        .map(KeyCode::Char)
        .chain([KeyCode::Enter, KeyCode::Enter, KeyCode::Esc, KeyCode::Esc])
        .chain("j/t".chars().map(KeyCode::Char))
        .chain([KeyCode::Esc])
        .chain("t23456f456tt5ff6ftt4".chars().map(KeyCode::Char))
        .chain([KeyCode::Enter])
        .chain("jjkk".chars().map(KeyCode::Char))
        .collect();
    let mut keys = Vec::new();
    let mut all = Vec::new();
    let mut longest_wait = Duration::ZERO;
    for k in script {
        let t = Instant::now();
        app.handle_key(code(k));
        app.poll_results();
        let _ = render(&app, 160, 50);
        let d = t.elapsed();
        keys.push(d);
        all.push(d);
        // Run the loop until the view is computed, as the real UI does.
        let wait = Instant::now();
        while app.is_busy() && wait.elapsed() < Duration::from_secs(30) {
            all.push(frame(&mut app));
            std::thread::sleep(Duration::from_millis(15));
        }
        longest_wait = longest_wait.max(wait.elapsed());
        all.push(frame(&mut app));
    }
    let n_keys = keys.len();
    let n_all = all.len();
    let key_p95 = percentile(&mut keys, 95);
    let key_max = *keys.last().unwrap();
    let all_p95 = percentile(&mut all, 95);
    let all_max = *all.last().unwrap();
    println!(
        "100k events: first frame {first_frame:?}, first frame with data {first_data:?}; \
         {n_keys} keys: key+render p95 {key_p95:?} max {key_max:?}; \
         {n_all} UI steps incl. result frames: p95 {all_p95:?} max {all_max:?}; \
         longest wait for a computed view {longest_wait:?}"
    );
    let bound = if cfg!(debug_assertions) {
        Duration::from_millis(500)
    } else {
        Duration::from_millis(50)
    };
    assert!(key_p95 <= bound, "key handling p95 {key_p95:?}");
    assert!(all_p95 <= bound, "UI step p95 {all_p95:?}");
    assert!(first_frame <= Duration::from_secs(5));
}
