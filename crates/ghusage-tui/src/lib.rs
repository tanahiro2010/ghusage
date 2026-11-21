//! Interactive terminal UI for ghusage.
//!
//! The crate is split into:
//! - [`app`]: UI state, key handling and report caching (no terminal, no
//!   network); numbers come from `ghusage_app::report`, like the CLI.
//! - [`ui`]: rendering of an [`app::App`] into a ratatui frame.
//! - [`source`]: read access to the local cache.
//! - [`background`]: GitHub sync on a separate thread with its own runtime,
//!   reporting over a channel.
//! - [`term`]: terminal setup/restore, panic hook and URL opening.

pub mod app;
pub mod background;
mod compute;
pub mod source;
pub mod term;
pub mod ui;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use ghusage_app::AppError;
use ghusage_app::session::{Session, SessionConfig};
use ghusage_core::metrics::Filter;
use ghusage_core::time::{Period, Zone};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{self, Event};

use crate::app::{App, AppConfig, Effect};
use crate::background::SyncMsg;
use crate::source::SessionSource;

/// Options for the terminal UI.
#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub period: Period,
    pub zone: Zone,
    pub filter: Filter,
    /// Session configuration; the UI reads the cache offline.
    pub session: SessionConfig,
    /// Fill missing data from GitHub in the background.
    pub background_sync: bool,
    pub concurrency: usize,
}

/// Why the UI could not run.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// The cache could not be opened (no cached user, unreadable database,
    /// ...). Carries the same error, and exit code, as other commands.
    #[error(transparent)]
    App(#[from] AppError),
    /// The terminal could not be set up or drawn to.
    #[error("terminal UI error: {0}")]
    Terminal(#[from] io::Error),
}

/// Runs the UI until the user quits. The terminal is restored on exit and
/// on panic.
///
/// The UI runs on its own thread so it can be called from inside an async
/// runtime. It never waits for the network, and reports are computed on a
/// separate worker thread so keys are handled immediately.
pub fn run(opts: TuiOptions) -> Result<(), TuiError> {
    let handle = std::thread::Builder::new()
        .name("ghusage-tui".into())
        .spawn(move || run_here(opts))?;
    match handle.join() {
        Ok(r) => r,
        Err(_) => Err(TuiError::Terminal(io::Error::other(
            "the terminal UI stopped unexpectedly",
        ))),
    }
}

fn run_here(opts: TuiOptions) -> Result<(), TuiError> {
    let mut cfg = opts.session.clone();
    cfg.offline = true;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let session = rt.block_on(Session::open(cfg))?;
    drop(rt);

    let mut app = App::new(
        Box::new(SessionSource::new(session)),
        AppConfig {
            period: opts.period,
            zone: opts.zone,
            filter: opts.filter.clone(),
            background_sync: opts.background_sync,
        },
    )?;
    app.prepare();

    term::install_panic_hook();
    let mut terminal = term::enter()?;
    let guard = term::RestoreGuard;
    let result = event_loop(&mut terminal, &mut app, &opts);
    drop(guard);
    Ok(result?)
}

fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    opts: &TuiOptions,
) -> io::Result<()>
where
    B::Error: Into<io::Error>,
{
    let (tx, rx) = mpsc::channel::<SyncMsg>();
    let start_sync = |app: &mut App, effect: Option<Effect>| {
        if let Some(Effect::StartSync(period)) = effect
            && let Err(e) =
                background::spawn_sync(opts.session.clone(), period, opts.concurrency, tx.clone())
        {
            app.on_sync(SyncMsg::Failed(format!("cannot start sync: {e}")));
        }
    };
    if opts.background_sync {
        let effect = app.start_sync();
        start_sync(app, effect);
    }
    let mut dirty = true;
    while !app.should_quit {
        while let Ok(msg) = rx.try_recv() {
            app.on_sync(msg);
            dirty = true;
        }
        if app.poll_results() {
            dirty = true;
        }
        if dirty {
            app.prepare();
            terminal.draw(|f| ui::draw(f, app)).map_err(Into::into)?;
            dirty = false;
        }
        // Poll often while results are expected so they show up quickly.
        let wait = if app.is_busy() || app.sync.running {
            Duration::from_millis(15)
        } else {
            Duration::from_millis(100)
        };
        if !event::poll(wait)? {
            if !app.is_busy() {
                // Idle: precompute the other screens' reports.
                app.warm_up();
            }
            continue;
        }
        match event::read()? {
            Event::Key(key) => {
                match app.handle_key(key) {
                    Some(Effect::OpenUrl(url)) => {
                        if let Err(e) = term::open_url(&url) {
                            app.message = Some(e);
                        }
                    }
                    e @ Some(Effect::StartSync(_)) => start_sync(app, e),
                    Some(Effect::Quit) | None => {}
                }
                dirty = true;
            }
            Event::Resize(..) => {
                terminal.autoresize().map_err(Into::into)?;
                dirty = true;
            }
            _ => {}
        }
    }
    Ok(())
}
