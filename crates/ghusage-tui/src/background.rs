//! Background synchronization. Network work runs on its own thread with its
//! own async runtime and its own session; the UI only receives messages.

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use ghusage_app::session::{Session, SessionConfig};
use ghusage_app::sync::SyncProgress;
use ghusage_core::time::Period;

/// Messages from the background worker to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMsg {
    /// Connected and authenticated; `auth` describes the token source.
    Started { auth: Option<String> },
    /// Human-readable progress.
    Progress(String),
    /// Current rate limit, when GitHub reported it.
    RateLimit(String),
    /// The sync ended. `failures` lists what could not be fetched.
    Finished {
        complete: bool,
        failures: Vec<String>,
        summary: String,
    },
    /// The sync could not run or stopped with an error (including network
    /// loss). Cached data stays usable.
    Failed(String),
}

/// Formats a progress notification.
pub fn describe_progress(p: &SyncProgress) -> String {
    match p {
        SyncProgress::Searching(k) => format!("searching {}", k.as_str()),
        SyncProgress::Fetching { kind, done, total } => {
            format!("fetching {} {done}/{total}", kind.as_str())
        }
        SyncProgress::Detail(d) => d.clone(),
        SyncProgress::Finished => "finishing".into(),
    }
}

/// Starts a sync of `period` on a new thread. Errors and panics inside the
/// worker are reported as [`SyncMsg::Failed`]; they never reach the UI
/// thread.
pub fn spawn_sync(
    mut cfg: SessionConfig,
    period: Period,
    concurrency: usize,
    tx: Sender<SyncMsg>,
) -> std::io::Result<JoinHandle<()>> {
    cfg.offline = false;
    std::thread::Builder::new()
        .name("ghusage-sync".into())
        .spawn(move || {
            crate::term::set_quiet_panics(true);
            let panic_tx = tx.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sync_blocking(cfg, period, concurrency, &tx)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = tx.send(SyncMsg::Failed(e));
                }
                Err(payload) => {
                    let what = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown error".into());
                    let _ = panic_tx.send(SyncMsg::Failed(format!("internal error: {what}")));
                }
            }
        })
}

fn sync_blocking(
    cfg: SessionConfig,
    period: Period,
    concurrency: usize,
    tx: &Sender<SyncMsg>,
) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the sync runtime: {e}"))?;
    rt.block_on(async {
        let mut session = Session::open(cfg).await.map_err(|e| e.to_string())?;
        let _ = tx.send(SyncMsg::Started {
            auth: session.token_source_description(),
        });
        let progress = |p: SyncProgress| {
            let _ = tx.send(SyncMsg::Progress(describe_progress(&p)));
        };
        let report = session
            .sync(period, false, concurrency.max(1), &progress)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(r) = session.rate_limit().await.map_err(|e| e.to_string())? {
            let _ = tx.send(SyncMsg::RateLimit(format!(
                "core {}/{} (resets {}), graphql {}/{}, search {}/{}",
                r.core_remaining,
                r.core_limit,
                r.core_reset.strftime("%Y-%m-%d %H:%M UTC"),
                r.graphql_remaining,
                r.graphql_limit,
                r.search_remaining,
                r.search_limit
            )));
        }
        let summary = format!(
            "{} · {} commits, {} pull requests ({} fetched), {} issues · {} requests",
            report.status,
            report.commits_found,
            report.pull_requests_found,
            report.pull_requests_fetched,
            report.issues_found,
            report.api.requests
        );
        let _ = tx.send(SyncMsg::Finished {
            complete: report.is_complete(),
            failures: report.failures.clone(),
            summary,
        });
        Ok(())
    })
}
