//! Loading commits for the commit detail screen. Commits are read from the
//! cache, or fetched from GitHub when missing and the UI is online, on a
//! thread of their own with its own runtime and session, through the same
//! service as `ghusage commit`. The UI only receives messages.

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use ghusage_app::commit::{self, CommitTarget};
use ghusage_app::session::{Session, SessionConfig};
use ghusage_core::time::Zone;

use crate::commit_view::{CommitFailure, CommitKey, CommitMsg, CommitView};

/// Starts the commit loader. Send keys to the returned sender; each one is
/// answered with a [`CommitMsg`] on `tx`. The thread stops when the sender
/// is dropped.
pub fn spawn(
    cfg: SessionConfig,
    online: bool,
    zone: Zone,
    tx: Sender<CommitMsg>,
) -> std::io::Result<Sender<CommitKey>> {
    let (req_tx, req_rx) = mpsc::channel::<CommitKey>();
    std::thread::Builder::new()
        .name("ghusage-commits".into())
        .spawn(move || {
            crate::term::set_quiet_panics(true);
            let mut loader = Loader::new(cfg, online, zone);
            for key in req_rx {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loader.load(&key)))
                        .unwrap_or_else(|_| {
                            loader.session = None;
                            Err(CommitFailure {
                                offline: false,
                                message: "internal error while loading this commit".into(),
                            })
                        });
                if tx.send(CommitMsg { key, result }).is_err() {
                    return;
                }
            }
        })?;
    Ok(req_tx)
}

/// Loads commits with one session, opened on first use.
pub struct Loader {
    cfg: SessionConfig,
    online: bool,
    zone: Zone,
    runtime: Option<tokio::runtime::Runtime>,
    session: Option<Session>,
}

impl Loader {
    pub fn new(cfg: SessionConfig, online: bool, zone: Zone) -> Self {
        Self {
            cfg,
            online,
            zone,
            runtime: None,
            session: None,
        }
    }

    /// The commit from the cache, or from GitHub when online.
    pub fn load(&mut self, key: &CommitKey) -> Result<Arc<CommitView>, CommitFailure> {
        let fail = |message: String| CommitFailure {
            offline: false,
            message,
        };
        let Some(target) = CommitTarget::from_parts(&key.repository, &key.sha) else {
            return Err(fail(format!(
                "{}@{} is not a commit that can be looked up",
                key.repository, key.sha
            )));
        };
        if self.runtime.is_none() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| fail(format!("cannot start the commit loader: {e}")))?;
            self.runtime = Some(rt);
        }
        let rt = self.runtime.as_ref().expect("set above");
        let (cfg, online, zone) = (&self.cfg, self.online, self.zone);
        let session = &mut self.session;
        rt.block_on(async {
            if session.is_none() {
                *session = Some(open_session(cfg, online).await.map_err(fail)?);
            }
            let s = session.as_mut().expect("set above");
            let result = commit::load(s, &target).await;
            if online && s.is_offline() {
                // Try to reach GitHub again for the next commit.
                *session = None;
            }
            match result {
                Ok(loaded) => Ok(Arc::new(CommitView::build(
                    &loaded.detail,
                    loaded.source,
                    zone,
                ))),
                Err(e) => Err(CommitFailure {
                    offline: e.is_offline(),
                    message: e.to_string(),
                }),
            }
        })
    }
}

/// An online session when `online`, falling back to the cache alone when
/// GitHub cannot be used (the reason is kept for messages).
async fn open_session(cfg: &SessionConfig, online: bool) -> Result<Session, String> {
    let mut c = cfg.clone();
    c.offline = !online;
    match Session::open(c).await {
        Ok(s) => Ok(s),
        Err(e) if online => {
            let mut c = cfg.clone();
            c.offline = true;
            let mut s = Session::open(c).await.map_err(|e| e.to_string())?;
            s.offline_reason = Some(e.to_string());
            Ok(s)
        }
        Err(e) => Err(e.to_string()),
    }
}
