//! Aggregation and cache-read benchmarks.
//!
//! Run with `cargo bench --bench aggregation`.

use criterion::{Criterion, criterion_group, criterion_main};
use ghusage_app::report::{self, ReportKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Granularity;
use ghusage_storage::{Store, WriteBatch};
use std::hint::black_box;

mod common;
use common::{ctx, data, subject};

fn benches(c: &mut Criterion) {
    let (repos, prs, events) = data();
    let ds = Dataset::new(
        subject(),
        true,
        repos.clone(),
        prs.clone(),
        vec![],
        vec![],
        events.clone(),
        vec![],
    );
    c.bench_function("aggregate 100k events (overview, 1 year)", |b| {
        b.iter(|| black_box(report::build(&ds, &ctx(366), ReportKind::Overview)));
    });
    c.bench_function("aggregate 100k events (daily, 1 year)", |b| {
        b.iter(|| {
            black_box(report::build(
                &ds,
                &ctx(366),
                ReportKind::Breakdown(Granularity::Day),
            ))
        });
    });

    // Cached aggregation: load from SQLite + aggregate.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("bench.db")).unwrap();
    store
        .apply(&WriteBatch {
            account: "me".into(),
            repositories: repos,
            pull_requests: prs,
            events,
            ..Default::default()
        })
        .unwrap();
    for days in [7, 30] {
        let ctx = ctx(days);
        c.bench_function(
            &format!("cached {days}-day aggregation (load + overview)"),
            |b| {
                b.iter(|| {
                    let ds = store
                        .load_dataset(&subject(), true, Some(ctx.period))
                        .unwrap();
                    black_box(report::build(&ds, &ctx, ReportKind::Overview))
                });
            },
        );
    }
}

criterion_group! {
    name = aggregation;
    config = Criterion::default().sample_size(30);
    targets = benches
}
criterion_main!(aggregation);
