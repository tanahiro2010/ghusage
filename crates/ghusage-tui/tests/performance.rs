//! Timing of the first render and of key handling on a large cache.
//! Prints measurements; bounds are generous so slow machines pass.

mod common;

use std::time::{Duration, Instant};

use chrono::Duration as Days;
use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::Dataset;
use ghusage_core::model::{
    Actor, ActorKind, EventMeta, EventType, PrState, PullRequest, Repository, Subject, Visibility,
};
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
    let span = (p.end - p.start).num_seconds();
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
        let at = p.start + Days::seconds((i as i64 * 7919) % span);
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

#[test]
fn first_render_and_key_latency_on_100k_events() {
    let ds = big_dataset(100_000);
    let started = Instant::now();
    let app = app_with(ds, false);
    let built = started.elapsed();
    let _ = render(&app, 160, 50);
    let first = started.elapsed();
    let mut app = app;

    // Cached interaction: selection movement + redraw.
    app.handle_key(key('3'));
    let _ = render(&app, 160, 50);
    let mut samples = Vec::new();
    for i in 0..60 {
        let t = Instant::now();
        app.handle_key(key(if i % 2 == 0 { 'j' } else { 'k' }));
        let _ = render(&app, 160, 50);
        samples.push(t.elapsed());
    }
    samples.sort();
    let p95 = samples[samples.len() * 95 / 100];

    // Screens computed for the first time.
    let t = Instant::now();
    app.handle_key(key('4'));
    let _ = render(&app, 160, 50);
    let pr_list = t.elapsed();
    let t = Instant::now();
    app.handle_key(code(KeyCode::Enter));
    let _ = render(&app, 160, 50);
    let pr_detail = t.elapsed();

    println!(
        "100k events: state + overview report {built:?}, first render total {first:?}, \
         cached key+render p95 {p95:?}, PR list first visit {pr_list:?}, PR detail {pr_detail:?}"
    );
    assert!(first < Duration::from_secs(30));
    assert!(p95 < Duration::from_secs(1));
}
