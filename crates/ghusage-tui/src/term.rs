//! Terminal setup and restoration, and opening URLs in a browser.

use std::cell::Cell;
use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use ghusage_core::sanitize::validate_github_url;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

static PANIC_RESTORES: AtomicUsize = AtomicUsize::new(0);
static HOOK: Once = Once::new();

thread_local! {
    static QUIET_PANICS: Cell<bool> = const { Cell::new(false) };
}

/// Marks panics on the current thread as handled elsewhere: the panic hook
/// neither restores the terminal nor prints, so a failing background worker
/// cannot tear down the running UI.
pub fn set_quiet_panics(quiet: bool) {
    QUIET_PANICS.with(|q| q.set(quiet));
}

/// Writes the sequences that leave the alternate screen and show the cursor.
pub fn write_restore_sequence<W: Write>(w: &mut W) -> io::Result<()> {
    execute!(w, LeaveAlternateScreen, Show)
}

/// Puts the terminal back into its normal state: raw mode off, main screen,
/// visible cursor. Safe to call when the terminal was never switched.
pub fn restore_terminal() -> io::Result<()> {
    let raw = disable_raw_mode();
    let screen = write_restore_sequence(&mut io::stdout());
    raw.and(screen)
}

/// Installs (once per process) a panic hook that restores the terminal
/// before the previous hook prints the panic message.
pub fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if QUIET_PANICS.with(|q| q.get()) {
                return;
            }
            let _ = restore_terminal();
            PANIC_RESTORES.fetch_add(1, Ordering::SeqCst);
            previous(info);
        }));
    });
}

/// How many times the panic hook has restored the terminal.
pub fn panic_restore_count() -> usize {
    PANIC_RESTORES.load(Ordering::SeqCst)
}

/// Switches to raw mode and the alternate screen.
pub fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    if let Err(e) = execute!(out, EnterAlternateScreen) {
        let _ = restore_terminal();
        return Err(e);
    }
    match Terminal::new(CrosstermBackend::new(out)) {
        Ok(t) => Ok(t),
        Err(e) => {
            let _ = restore_terminal();
            Err(e)
        }
    }
}

/// Restores the terminal when dropped (normal exit and error paths).
pub struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        let _ = restore_terminal();
    }
}

/// Characters that a Windows shell would interpret inside an argument.
const WINDOWS_SHELL_META: &[char] = &['&', '|', '^', '<', '>', '%', '"'];

/// Validates a URL for opening: https on github.com only.
pub fn checked_url(url: &str) -> Result<String, String> {
    let url = validate_github_url(url, &["github.com"]).map_err(|e| e.to_string())?;
    if cfg!(windows) && url.contains(WINDOWS_SHELL_META) {
        return Err("URL contains characters that cannot be passed safely".into());
    }
    Ok(url)
}

/// Opens a validated GitHub URL in the default browser without going
/// through a shell command string.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = checked_url(url)?;
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(&url);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(&url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start the browser: {e}"))?;
    // Reap the launcher without blocking the UI.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
