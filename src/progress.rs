//! Live sync progress on stderr: a spinner line with the state of each data
//! kind and how many requests run in parallel, the requests in flight, and
//! the latest thing the sync did. Shown only when stderr is a terminal, and
//! erased when the sync ends.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ghusage_app::session::{ApiActivity, ApiActivitySnapshot};
use ghusage_app::sync::SyncProgress;
use ghusage_core::coverage::DataKind;

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const TICK: Duration = Duration::from_millis(100);
/// Requests in flight listed one per line; the rest are counted.
const SHOWN_REQUESTS: usize = 4;

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
    const fn slot(&mut self, k: DataKind) -> &mut KindState {
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

/// The lines of one frame: status, requests in flight, latest step.
fn frame(
    state: &State,
    activity: Option<&ApiActivitySnapshot>,
    tick: usize,
    elapsed: Duration,
    width: usize,
) -> Vec<String> {
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
    if let Some(a) = activity {
        let _ = write!(
            status,
            " · {}/{} in parallel · {} requests done",
            a.running.len(),
            a.limit,
            a.done
        );
    }
    if !parts.is_empty() {
        status.push_str(" · ");
        status.push_str(&parts.join(" · "));
    }
    let mut lines = vec![truncate(&status, width)];
    if let Some(a) = activity {
        for r in a.running.iter().take(SHOWN_REQUESTS) {
            let line = format!(
                "  ▸ {} {:.1}s",
                ghusage_core::sanitize::sanitize_line(&r.label),
                r.elapsed.as_secs_f64()
            );
            lines.push(truncate(&line, width));
        }
        if a.running.len() > SHOWN_REQUESTS {
            lines.push(format!("  … {} more", a.running.len() - SHOWN_REQUESTS));
        }
    }
    if !state.detail.is_empty() {
        lines.push(truncate(&format!("  ↳ {}", state.detail), width));
    }
    lines
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
        .map_or(80, |(w, _)| w as usize)
        .max(20)
}

/// Draws the progress until it is dropped or the sync finishes.
pub(crate) struct Progress {
    shared: Option<Arc<Shared>>,
    painter: Option<JoinHandle<()>>,
}

/// Sync progress plus, when known, the client's requests in flight.
type Shared = (Mutex<State>, Condvar, Option<ApiActivity>);

impl Progress {
    /// Starts drawing on stderr if it is a terminal; otherwise does nothing.
    /// `activity` adds the requests in flight.
    pub(crate) fn start(activity: Option<ApiActivity>) -> Self {
        if !std::io::stderr().is_terminal() {
            return Self {
                shared: None,
                painter: None,
            };
        }
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new(), activity));
        let s = Arc::clone(&shared);
        let painter = std::thread::spawn(move || paint(&s));
        Self {
            shared: Some(shared),
            painter: Some(painter),
        }
    }

    pub(crate) fn update(&self, p: SyncProgress) {
        if let Some(shared) = &self.shared {
            let (lock, cv, _) = &**shared;
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
            drop(h.join());
        }
    }
}

fn paint(shared: &Shared) {
    let (lock, cv, activity) = shared;
    let started = Instant::now();
    let mut tick = 0;
    let mut drawn = 0usize;
    let mut err = std::io::stderr();
    loop {
        let Ok(st) = lock.lock() else { return };
        let Ok((st, _)) = cv.wait_timeout(st, TICK) else {
            return;
        };
        if st.finished {
            if drawn > 0 {
                drop(write!(err, "{}", redraw(&[], drawn)));
                drop(err.flush());
            }
            return;
        }
        // Wait a moment before drawing, so quick syncs leave no trace.
        if started.elapsed() < Duration::from_millis(300) {
            continue;
        }
        let snapshot = activity.as_ref().map(ApiActivity::snapshot);
        let lines = frame(
            &st,
            snapshot.as_ref(),
            tick,
            started.elapsed(),
            terminal_width() - 1,
        );
        drop(st);
        drop(write!(err, "{}", redraw(&lines, drawn)));
        drop(err.flush());
        drawn = lines.len();
        tick += 1;
    }
}

/// Escape sequences that replace the `previous` lines drawn last time with
/// `lines` (the first in normal text, the rest dimmed) and put the cursor
/// back at the start of the first line.
fn redraw(lines: &[String], previous: usize) -> String {
    let total = lines.len().max(previous).max(1);
    let mut out = String::from("\r");
    for i in 0..total {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("\x1b[2K");
        match lines.get(i) {
            Some(line) if i == 0 => out.push_str(line),
            Some(line) => {
                let _ = write!(out, "\x1b[2m{line}\x1b[0m");
            }
            None => {}
        }
    }
    if total > 1 {
        let _ = write!(out, "\x1b[{}A", total - 1);
    }
    out.push('\r');
    out
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
        let lines = frame(&st, None, 1, Duration::from_secs(75), 200);
        assert_eq!(
            lines,
            vec![
                "⠙ Syncing with GitHub 1:15 · pull requests searching · commits 3/40".to_string(),
                "  ↳ octo/app#12".to_string(),
            ]
        );
    }

    #[test]
    fn frame_fits_the_terminal_width() {
        let mut st = State::default();
        st.apply(SyncProgress::Detail("x".repeat(100)));
        let lines = frame(&st, None, 0, Duration::ZERO, 20);
        assert_eq!(lines[0].chars().count(), 20);
        assert!(lines[0].ends_with('…'));
        assert_eq!(lines[1].chars().count(), 20);
    }

    #[test]
    fn frame_lists_requests_in_flight_and_counts_the_rest() {
        let st = State::default();
        let running = (0..6)
            .map(|i| ghusage_app::session::ApiInFlight {
                label: format!("search commits p{}", i + 2),
                elapsed: Duration::from_millis(1200),
            })
            .collect();
        let a = ApiActivitySnapshot {
            running,
            limit: 8,
            done: 41,
        };
        let lines = frame(&st, Some(&a), 0, Duration::from_secs(3), 200);
        assert_eq!(
            lines[0],
            "⠋ Syncing with GitHub 0:03 · 6/8 in parallel · 41 requests done"
        );
        assert_eq!(lines[1], "  ▸ search commits p2 1.2s");
        assert_eq!(lines.len(), 1 + 4 + 1);
        assert_eq!(lines[5], "  … 2 more");
    }

    #[test]
    fn redraw_clears_lines_left_from_a_taller_frame() {
        let out = redraw(&["a".to_string()], 3);
        // Three lines cleared, only the first written, cursor back up two.
        assert_eq!(out.matches("\x1b[2K").count(), 3);
        assert!(out.ends_with("\x1b[2A\r"));
        assert_eq!(redraw(&[], 1), "\r\x1b[2K\r");
    }

    #[test]
    fn finished_is_remembered() {
        let mut st = State::default();
        st.apply(SyncProgress::Finished);
        assert!(st.finished);
    }
}
