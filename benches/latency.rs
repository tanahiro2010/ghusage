//! Latency percentiles for cached aggregation, measured as wall-clock time
//! over repeated runs after a warm-up. Run with
//! `cargo bench --bench latency`.

mod common;
use common::*;

use ghusage_app::report::{self, ReportKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Granularity;
use ghusage_storage::{Store, WriteBatch};
use std::hint::black_box;
use std::time::{Duration, Instant};

const WARMUP: usize = 5;
const RUNS: usize = 50;

/// Nearest-rank percentile.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn measure(name: &str, target: Duration, mut f: impl FnMut()) -> bool {
    for _ in 0..WARMUP {
        f();
    }
    let mut v: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    v.sort();
    let p50 = percentile(&v, 0.5);
    let p95 = percentile(&v, 0.95);
    let ok = p95 <= target;
    println!(
        "{name}: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms (target p95 <= {} ms) {}",
        p50.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        v[v.len() - 1].as_secs_f64() * 1e3,
        target.as_millis(),
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

fn main() {
    // `cargo bench` passes `--bench`; ignore arguments.
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
    println!(
        "dataset: {EVENTS} events, 2000 pull requests, 40 repositories, {RUNS} runs after {WARMUP} warm-up"
    );
    let mut all = true;
    all &= measure(
        "100k event aggregation (overview, 1 year)",
        Duration::from_secs(1),
        || {
            black_box(report::build(&ds, &ctx(366), ReportKind::Overview));
        },
    );
    all &= measure(
        "100k event aggregation (daily, 1 year)",
        Duration::from_secs(1),
        || {
            black_box(report::build(
                &ds,
                &ctx(366),
                ReportKind::Breakdown(Granularity::Day),
            ));
        },
    );
    for (days, ms) in [(7, 200), (30, 500)] {
        let c = ctx(days);
        all &= measure(
            &format!("cached {days}-day aggregation (load + overview)"),
            Duration::from_millis(ms),
            || {
                let ds = store
                    .load_dataset(&subject(), true, Some(c.period))
                    .unwrap();
                black_box(report::build(&ds, &c, ReportKind::Overview));
            },
        );
    }
    if !all {
        std::process::exit(1);
    }
}
