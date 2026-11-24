use crate::commands::open_cache::open;
use crate::error::Result;
use crate::session::{SessionConfig, credentials};
use ghusage_core::time::Zone;
use ghusage_github::GitHubClient;

/// Outcome of one diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DoctorState {
    /// Check completed successfully.
    Passed,
    /// Check found a problem.
    Failed,
    /// Check was skipped by configuration.
    Skipped,
}

/// One diagnostic result and its display message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorCheck {
    /// Check outcome.
    pub state: DoctorState,
    /// Human-readable diagnostic result.
    pub message: String,
}

/// Full diagnostic result for the cache and GitHub connection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DoctorReport {
    /// Whether no diagnostic failed.
    pub ok: bool,
    /// Suggested process exit code.
    pub exit_code: i32,
    /// Individual diagnostic results.
    pub checks: Vec<DoctorCheck>,
}

/// Checks cache integrity, credentials, authentication, and rate limits.
pub async fn doctor(cfg: &SessionConfig, zone: Zone) -> Result<DoctorReport> {
    let mut checks = Vec::new();
    match open(cfg) {
        Ok(cache) => {
            let path = cache.data_dir.as_ref().map_or_else(
                || "(in memory)".into(),
                |dir| crate::paths::db_path(dir).display().to_string(),
            );
            checks.push(DoctorCheck {
                state: DoctorState::Passed,
                message: format!("cache opened: {path}"),
            });
            match cache.store.integrity_check() {
                Ok(problems) if problems.is_empty() => checks.push(DoctorCheck {
                    state: DoctorState::Passed,
                    message: "cache integrity: ok".into(),
                }),
                Ok(problems) => checks.push(DoctorCheck {
                    state: DoctorState::Failed,
                    message: format!("cache integrity problems: {}", problems.join("; ")),
                }),
                Err(error) => checks.push(DoctorCheck {
                    state: DoctorState::Failed,
                    message: format!("cache integrity check failed: {error}"),
                }),
            }
            if let Ok(Some(login)) = cache.store.meta(crate::sync::META_VIEWER_LOGIN)
                && let Ok(Some(run)) = cache.store.last_sync_run(&login)
            {
                checks.push(DoctorCheck {
                    state: if run.status == "failed" {
                        DoctorState::Failed
                    } else {
                        DoctorState::Passed
                    },
                    message: format!(
                        "last sync: {} ({})",
                        run.finished_at.map_or_else(
                            || "not finished".into(),
                            |time| zone.format(time, "%Y-%m-%d %H:%M")
                        ),
                        run.status
                    ),
                });
            }
        }
        Err(error) => checks.push(DoctorCheck {
            state: DoctorState::Failed,
            message: format!("cache: {error}"),
        }),
    }
    if cfg.offline {
        checks.push(DoctorCheck {
            state: DoctorState::Skipped,
            message: "offline: GitHub checks skipped".into(),
        });
    } else {
        match credentials(cfg.token_file.as_deref()) {
            Ok(credentials) => {
                checks.push(DoctorCheck {
                    state: DoctorState::Passed,
                    message: format!("credentials found: {}", credentials.source.describe()),
                });
                match GitHubClient::new(Some(credentials), cfg.client.clone()) {
                    Ok(client) => {
                        match client.viewer().await {
                            Ok(viewer) => checks.push(DoctorCheck {
                                state: DoctorState::Passed,
                                message: format!("authenticated as {}", viewer.login),
                            }),
                            Err(error) => checks.push(DoctorCheck {
                                state: DoctorState::Failed,
                                message: format!("authentication: {error}"),
                            }),
                        }
                        match client.rate_limit().await {
                            Ok(rate) => checks.push(DoctorCheck {
                                state: if rate.core_remaining > 0 {
                                    DoctorState::Passed
                                } else {
                                    DoctorState::Failed
                                },
                                message: format!(
                                    "rate limit: core {}/{}, search {}/{}, graphql {}/{}",
                                    rate.core_remaining,
                                    rate.core_limit,
                                    rate.search_remaining,
                                    rate.search_limit,
                                    rate.graphql_remaining,
                                    rate.graphql_limit
                                ),
                            }),
                            Err(error) => checks.push(DoctorCheck {
                                state: DoctorState::Failed,
                                message: format!("rate limit: {error}"),
                            }),
                        }
                    }
                    Err(error) => checks.push(DoctorCheck {
                        state: DoctorState::Failed,
                        message: format!("client: {error}"),
                    }),
                }
            }
            Err(error) => checks.push(DoctorCheck {
                state: DoctorState::Failed,
                message: format!("credentials: {error}"),
            }),
        }
    }
    let ok = checks
        .iter()
        .all(|check| check.state != DoctorState::Failed);
    Ok(DoctorReport {
        ok,
        exit_code: i32::from(!ok),
        checks,
    })
}
