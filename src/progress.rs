//! Live sync progress on stderr: a spinner line with the state of each data
//! kind, and below it the latest thing the sync did. Shown only when stderr
//! is a terminal, and erased when the sync ends.

use std::io::{IsTerminal, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ghusage_app::sync::SyncProgress;
use ghusage_core::coverage::DataKind;

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const TICK: Duration = Duration::from_millis(100);

/// What is known about one data kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum KindState {
    #[default]
    Waiting,
    Searching,
    Fetching {
        done: usize,
        total: usize,
    },
}

#[derive(Debug, Default)]
struct State {
    prs: KindState,
    commits: KindState,
    issues: KindState,
    detail: String,
    finished: bool,
}

impl State {
    fn slot(&mut self, k: DataKind) -> &mut KindState {
        match k {
            DataKind::PullRequests => &mut self.prs,
            DataKind::Commits => &mut self.commits,
            DataKind::Issues => &mut self.issues,
        }
    }

    fn apply(&mut self, p: SyncProgress) {
        match p {
            SyncProgress::Searching(k) => *self.slot(k) = KindState::Searching,
            SyncProgress::Fetching { kind, done, total } => {
                *self.slot(kind) = KindState::Fetching { done, total }
            }
            SyncProgress::Detail(d) => self.detail = ghusage_core::sanitize::sanitize_line(&d),
            SyncProgress::Finished => self.finished = true,
        }
    }
}

/// Status line and detail line for one frame.
fn frame(state: &State, tick: usize, elapsed: Duration, width: usize) -> (String, String) {
    let kind = |label: &str, s: &KindState| match s {
        KindState::Waiting => None,
        KindState::Searching => Some(format!("{label} searching")),
        KindState::Fetching { done, total } => Some(format!("{label} {done}/{total}")),
    };
    let parts: Vec<String> = [
        kind("pull requests", &state.prs),
        kind("commits", &state.commits),
        kind("issues", &state.issues),
    ]
    .into_iter()
    .flatten()
    .collect();
    let secs = elapsed.as_secs();
    let mut status = format!(
        "{} Syncing with GitHub {}:{:02}",
        FRAMES[tick % FRAMES.len()],
        secs / 60,
        secs % 60
    );
    if !parts.is_empty() {
        status.push_str(" · ");
        status.push_str(&parts.join(" · "));
    }
    let detail = if state.detail.is_empty() {
        String::new()
    } else {
        format!("  ↳ {}", state.detail)
    };
    (truncate(&status, width), truncate(&detail, width))
}

/// Cuts `s` to at most `width` characters, marking the cut with `…`.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn terminal_width() -> usize {
    ratatui::crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(20)
}

/// Draws the progress until it is dropped or the sync finishes.
pub struct Progress {
    shared: Option<Arc<(Mutex<State>, Condvar)>>,
    painter: Option<JoinHandle<()>>,
}

impl Progress {
    /// Starts drawing on stderr if it is a terminal; otherwise does nothing.
    pub fn start() -> Self {
        if !std::io::stderr().is_terminal() {
            return Self {
                shared: None,
                painter: None,
            };
        }
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let s = Arc::clone(&shared);
        let painter = std::thread::spawn(move || paint(&s));
        Self {
            shared: Some(shared),
            painter: Some(painter),
        }
    }

    pub fn update(&self, p: SyncProgress) {
        if let Some(shared) = &self.shared {
            let (lock, cv) = &**shared;
            if let Ok(mut st) = lock.lock() {
                st.apply(p);
            }
            cv.notify_all();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.update(SyncProgress::Finished);
        if let Some(h) = self.painter.take() {
            let _ = h.join();
        }
    }
}

fn paint(shared: &(Mutex<State>, Condvar)) {
    let (lock, cv) = shared;
    let started = Instant::now();
    let mut tick = 0;
    let mut drawn = false;
    let mut err = std::io::stderr();
    loop {
        let Ok(st) = lock.lock() else { return };
        let Ok((st, _)) = cv.wait_timeout(st, TICK) else {
            return;
        };
        if st.finished {
            if drawn {
                // Erase both lines and leave the cursor where it started.
                let _ = write!(err, "\r\x1b[2K\n\x1b[2K\x1b[1A\r");
                let _ = err.flush();
            }
            return;
        }
        // Wait a moment before drawing, so quick syncs leave no trace.
        if started.elapsed() < Duration::from_millis(300) {
            continue;
        }
        let (status, detail) = frame(&st, tick, started.elapsed(), terminal_width() - 1);
        drop(st);
        let _ = write!(
            err,
            "\r\x1b[2K{status}\n\x1b[2K\x1b[2m{detail}\x1b[0m\x1b[1A\r"
        );
        let _ = err.flush();
        drawn = true;
        tick += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_shows_each_started_kind_and_the_latest_detail() {
        let mut st = State::default();
        st.apply(SyncProgress::Searching(DataKind::PullRequests));
        st.apply(SyncProgress::Fetching {
            kind: DataKind::Commits,
            done: 3,
            total: 40,
        });
        st.apply(SyncProgress::Detail("octo/app#12".into()));
        let (status, detail) = frame(&st, 1, Duration::from_secs(75), 200);
        assert_eq!(
            status,
            "⠙ Syncing with GitHub 1:15 · pull requests searching · commits 3/40"
        );
        assert_eq!(detail, "  ↳ octo/app#12");
    }

    #[test]
    fn frame_fits_the_terminal_width() {
        let mut st = State::default();
        st.apply(SyncProgress::Detail("x".repeat(100)));
        let (status, detail) = frame(&st, 0, Duration::ZERO, 20);
        assert_eq!(status.chars().count(), 20);
        assert!(status.ends_with('…'));
        assert_eq!(detail.chars().count(), 20);
    }

    #[test]
    fn finished_is_remembered() {
        let mut st = State::default();
        st.apply(SyncProgress::Finished);
        assert!(st.finished);
    }
}
