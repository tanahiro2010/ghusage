//! Latency percentiles for cached aggregation, measured as wall-clock time
//! over repeated runs after a warm-up. Run with
//! `cargo bench --bench latency`.

mod common;
use common::{EVENTS, ctx, data, subject};

use ghusage_app::report::{self, ReportKind};
use ghusage_core::metrics::Dataset;
use ghusage_core::time::Granularity;
use ghusage_storage::{Store, WriteBatch};
use std::hint::black_box;
use std::time::{Duration, Instant};

const WARMUP: usize = 5;
const RUNS: usize = 50;

/// Nearest-rank percentile.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "The benchmark passes positive percentiles over a nonempty, small fixed sample."
)]
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
    // `cargo test --all-targets` also runs this file, in a debug build and
    // without `--bench`; percentiles are only meaningful under `cargo bench`.
    if !std::env::args().any(|a| a == "--bench") {
        println!("latency percentiles are measured with `cargo bench --bench latency`; skipped");
        return;
    }
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
    let year = ctx(366);
    all &= measure(
        "100k event aggregation (overview, 1 year)",
        Duration::from_secs(1),
        || {
            black_box(report::build(&ds, &year, ReportKind::Overview));
        },
    );
    all &= measure(
        "100k event aggregation (daily, 1 year)",
        Duration::from_secs(1),
        || {
            black_box(report::build(
                &ds,
                &year,
                ReportKind::Breakdown(Granularity::Day),
            ));
        },
    );
    for (name, granularity) in [
        ("weekly", Granularity::Week),
        ("monthly", Granularity::Month),
    ] {
        all &= measure(
            &format!("100k event aggregation ({name}, 1 year)"),
            Duration::from_secs(1),
            || {
                black_box(report::build(
                    &ds,
                    &year,
                    ReportKind::Breakdown(granularity),
                ));
            },
        );
    }
    all &= measure(
        "100k event aggregation (repositories, 1 year)",
        Duration::from_secs(1),
        || {
            black_box(report::build(&ds, &year, ReportKind::Repositories));
        },
    );
    for (days, ms) in [(7, 200), (30, 500)] {
        let c = ctx(days);
        all &= measure(
            &format!("cached {days}-day database load"),
            Duration::from_millis(ms),
            || {
                black_box(
                    store
                        .load_dataset(&subject(), true, Some(c.period))
                        .unwrap(),
                );
            },
        );
        let loaded = store
            .load_dataset(&subject(), true, Some(c.period))
            .unwrap();
        all &= measure(
            &format!("cached {days}-day report aggregation (preloaded)"),
            Duration::from_millis(ms),
            || {
                black_box(report::build(&loaded, &c, ReportKind::Overview));
            },
        );
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
    all &= measure(
        "cached pull request detail load (PR_0)",
        Duration::from_millis(200),
        || {
            black_box(store.load_pr_dataset(&subject(), true, "PR_0").unwrap());
        },
    );
    if !all {
        std::process::exit(1);
    }
}
