//! Profile reads and report aggregation against a copied ghusage cache.
//!
//! Set `GHUSAGE_BENCH_DATA_DIR` to the directory containing `ghusage.db` and
//! `GHUSAGE_BENCH_UNTIL` to a fixed RFC 3339 timestamp before running:
//!
//! ```sh
//! cargo bench --bench cache_profile
//! ```

use std::env;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::session::{Session, SessionConfig};
use ghusage_core::metrics::{CommitDateBasis, Filter};
use ghusage_core::time::{Granularity, Period, Zone};
use ghusage_storage::Store;
use jiff::{SignedDuration, Timestamp};

const WARMUP: usize = 5;
const RUNS: usize = 50;

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    let rank = (percentile * sorted.len())
        .div_ceil(100)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}

fn measure(name: &str, mut f: impl FnMut()) {
    for _ in 0..WARMUP {
        f();
    }
    let mut times: Vec<_> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "{name}: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms ({RUNS} runs after {WARMUP} warm-up)",
        percentile(&times, 50).as_secs_f64() * 1e3,
        percentile(&times, 95).as_secs_f64() * 1e3,
        times[times.len() - 1].as_secs_f64() * 1e3,
    );
}

fn period(until: Timestamp, days: i64) -> Period {
    Period::new(
        until - SignedDuration::from_secs(days * 24 * 60 * 60),
        until,
    )
    .expect("benchmark period is valid")
}

fn context(period: Period, synced_at: Option<Timestamp>) -> QueryContext {
    QueryContext {
        period,
        zone: Zone::parse("Asia/Tokyo").expect("timezone is valid"),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
        redact_private: false,
        synced_at,
    }
}

fn main() {
    if !env::args().any(|arg| arg == "--bench") {
        println!("cache profiling runs with `cargo bench --bench cache_profile`; skipped");
        return;
    }

    let data_dir = PathBuf::from(
        env::var_os("GHUSAGE_BENCH_DATA_DIR")
            .expect("set GHUSAGE_BENCH_DATA_DIR to a disposable cache copy"),
    );
    let until: Timestamp = env::var("GHUSAGE_BENCH_UNTIL")
        .expect("set GHUSAGE_BENCH_UNTIL to a fixed RFC 3339 timestamp")
        .parse()
        .expect("GHUSAGE_BENCH_UNTIL is RFC 3339");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create Tokio runtime");
    let session = runtime
        .block_on(Session::open(SessionConfig {
            data_dir: Some(data_dir.clone()),
            offline: true,
            ..Default::default()
        }))
        .expect("open the copied cache offline");
    let identity = session.identity();
    let cache = session.cache_info();
    let synced_at = session.last_synced_at().expect("read sync timestamp");
    println!(
        "cache: {} events, {} pull requests, {} repositories; until {until}; timezone Asia/Tokyo",
        cache.cached_events.unwrap_or_default(),
        cache.cached_pull_requests.unwrap_or_default(),
        cache.cached_repositories.unwrap_or_default(),
    );

    let periods = [
        (7, period(until, 7)),
        (30, period(until, 30)),
        (366, period(until, 366)),
        (3000, period(until, 3000)),
    ];
    for (days, p) in periods {
        let ctx = context(p, synced_at);
        let dataset = session.dataset(Some(p)).expect("load cached dataset");
        println!("loaded {days}-day dataset: {} events", dataset.events.len());
        measure(&format!("cache load ({days} days)"), || {
            black_box(session.dataset(Some(ctx.period)).expect("load cache"));
        });
        measure(
            &format!("report aggregation ({days} days, overview)"),
            || {
                black_box(report::build(&dataset, &ctx, ReportKind::Overview));
            },
        );
        measure(&format!("cache load + overview ({days} days)"), || {
            let ds = session.dataset(Some(ctx.period)).expect("load cache");
            black_box(report::build(&ds, &ctx, ReportKind::Overview));
        });
    }

    let full = period(until, 3000);
    let full_ctx = context(full, synced_at);
    let full_dataset = session
        .dataset(Some(full))
        .expect("load full-history dataset");
    for (name, kind) in [
        ("overview", ReportKind::Overview),
        ("daily", ReportKind::Breakdown(Granularity::Day)),
        ("weekly", ReportKind::Breakdown(Granularity::Week)),
        ("monthly", ReportKind::Breakdown(Granularity::Month)),
        ("repositories", ReportKind::Repositories),
    ] {
        measure(&format!("report aggregation (3000 days, {name})"), || {
            black_box(report::build(&full_dataset, &full_ctx, kind));
        });
    }

    let mut ids: Vec<_> = Store::open(&data_dir.join("ghusage.db"))
        .expect("open copied cache for PR selection")
        .pull_request_ids(&identity.login, false)
        .expect("list cached pull requests")
        .into_iter()
        .collect();
    ids.sort();
    if let Some(pr_id) = ids.first() {
        measure("pull request detail cache read", || {
            black_box(session.pr_dataset(pr_id).expect("load PR detail"));
        });
    } else {
        println!("pull request detail cache read: skipped (no cached pull requests)");
    }
}
