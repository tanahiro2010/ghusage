//! Errors opening the cache are returned as application errors, with the
//! same exit codes as other commands, before the terminal is touched.

mod common;

use common::*;
use ghusage_app::session::SessionConfig;
use ghusage_app::{AppError, exit};
use ghusage_core::metrics::Filter;
use ghusage_tui::{TuiError, TuiOptions, run};

fn options(dir: &std::path::Path, user: Option<&str>) -> TuiOptions {
    TuiOptions {
        period: period_30d(),
        zone: zone(),
        filter: Filter::default(),
        session: SessionConfig {
            data_dir: Some(dir.to_path_buf()),
            offline: true,
            user: user.map(Into::into),
            ..SessionConfig::default()
        },
        background_sync: false,
        concurrency: 1,
        commit_basis: ghusage_core::metrics::CommitDateBasis::default(),
    }
}

#[test]
fn missing_cached_user_is_an_application_error() {
    let dir = tempfile::tempdir().unwrap();
    match run(options(dir.path(), None)) {
        Err(TuiError::App(e @ AppError::Other(_))) => {
            assert_eq!(e.exit_code(), exit::GENERAL);
            assert!(e.to_string().contains("no cached data"), "{e}");
        }
        other => panic!("expected an application error, got {other:?}"),
    }
}

#[test]
fn corrupt_cache_keeps_the_cache_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        ghusage_app::paths::db_path(dir.path()),
        b"definitely not a database, just some bytes to make the header invalid",
    )
    .unwrap();
    match run(options(dir.path(), Some("alice"))) {
        Err(TuiError::App(e)) => assert_eq!(e.exit_code(), exit::CACHE, "{e}"),
        other => panic!("expected an application error, got {other:?}"),
    }
}
