//! Profile reads and report aggregation using a disposable SQLite backup.
//!
//! Set `GHUSAGE_BENCH_SOURCE_DB` to a source database and
//! `GHUSAGE_BENCH_UNTIL` to a fixed RFC 3339 timestamp before running. The
//! source is opened read-only, backed up into a temporary directory, and only
//! that disposable copy is opened by ghusage. Opening the copy may migrate its
//! schema; the source schema version is checked before and after.
//!
//! ```sh
//! GHUSAGE_BENCH_SOURCE_DB=/path/to/ghusage.db \
//! GHUSAGE_BENCH_UNTIL=2026-11-25T00:00:00Z \
//! cargo bench --bench cache_profile
//! ```

use std::env;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::session::{Session, SessionConfig};
use ghusage_core::metrics::{CommitDateBasis, Filter};
use ghusage_core::time::{Granularity, Period, Zone};
use ghusage_storage::Store;
use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, OpenFlags, backup::Backup};

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

fn schema_version(path: &Path) -> rusqlite::Result<u32> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

fn make_disposable_copy(source: &Path, destination: &Path) -> rusqlite::Result<u32> {
    let source_conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let source_version =
        source_conn.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    let mut destination_conn = Connection::open(destination)?;
    {
        let backup = Backup::new(&source_conn, &mut destination_conn)?;
        backup.run_to_completion(32, Duration::from_millis(50), None)?;
    }
    let destination_version =
        destination_conn.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    assert_eq!(
        destination_version, source_version,
        "SQLite backup schema version differs from its read-only source"
    );
    Ok(source_version)
}

fn main() {
    if !env::args().any(|arg| arg == "--bench") {
        println!("cache profiling runs with `cargo bench --bench cache_profile`; skipped");
        return;
    }

    let source_db = PathBuf::from(
        env::var_os("GHUSAGE_BENCH_SOURCE_DB")
            .expect("set GHUSAGE_BENCH_SOURCE_DB to an existing cache database"),
    );
    let disposable_dir = tempfile::tempdir().expect("create disposable cache directory");
    let data_dir = disposable_dir.path().to_path_buf();
    let disposable_db = data_dir.join("ghusage.db");
    let source_schema_before = make_disposable_copy(&source_db, &disposable_db)
        .expect("make a read-only SQLite backup into the disposable directory");
    let copy_schema_before = schema_version(&disposable_db)
        .expect("read disposable cache schema before opening ghusage");
    assert_eq!(source_schema_before, copy_schema_before);
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
    let source_schema_after =
        schema_version(&source_db).expect("recheck read-only source schema after opening copy");
    let copy_schema_after =
        schema_version(&disposable_db).expect("read disposable cache schema after opening it");
    assert_eq!(
        source_schema_before, source_schema_after,
        "opening the disposable cache changed the source schema"
    );
    println!(
        "using disposable SQLite backup; source schema v{source_schema_before} unchanged, copy schema v{copy_schema_before} -> v{copy_schema_after} after ghusage open"
    );
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
