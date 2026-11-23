//! Synthetic activity used by the UI tests.
#![allow(dead_code)]

use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::metrics::{Dataset, Filter};
use ghusage_core::model::{
    ActivityEvent, Actor, ActorKind, EventId, EventMeta, EventType, PrState, PullRequest,
    Repository, ReviewState, Source, Subject, Visibility,
};
use ghusage_core::time::{Period, Zone, resolve_period};
use ghusage_tui::app::{App, AppConfig};
use ghusage_tui::source::{DataSource, MemorySource};
use jiff::{SignedDuration, Timestamp};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub const MALICIOUS_TITLE: &str =
    "\u{1b}[31mred\u{1b}[0m \u{1b}]0;pwned\u{7}title\u{9b}2J\u{202e}evil\r\nnext";

pub fn utc(s: &str) -> Timestamp {
    s.parse().unwrap()
}

pub fn now() -> Timestamp {
    utc("2026-11-21T12:00:00Z")
}

pub fn zone() -> Zone {
    Zone::parse("Asia/Tokyo").unwrap()
}

pub fn period_30d() -> Period {
    resolve_period(Some("30d"), None, "30d", &zone(), now()).unwrap()
}

pub fn alice() -> Actor {
    Actor::new(Some("U1".into()), Some("alice".into()), Some("User"))
}

pub fn bob() -> Actor {
    Actor::new(Some("U2".into()), Some("bob".into()), Some("User"))
}

pub fn bot() -> Actor {
    Actor::new(
        Some("B1".into()),
        Some("ci-helper[bot]".into()),
        Some("Bot"),
    )
}

fn repo(id: &str, owner: &str, name: &str, vis: Visibility) -> Repository {
    Repository {
        id: id.into(),
        owner: owner.into(),
        name: name.into(),
        owner_kind: ActorKind::User,
        visibility: vis,
        is_fork: false,
        viewer_permission: None,
        accessible: true,
        url: Some(format!("https://github.com/{owner}/{name}")),
    }
}

fn pr(
    id: &str,
    repo: &str,
    number: u64,
    title: &str,
    author: Actor,
    state: PrState,
    created: &str,
) -> PullRequest {
    let created = utc(created);
    PullRequest {
        id: id.into(),
        repository_id: repo.into(),
        number,
        title: title.into(),
        url: format!("https://github.com/x/y/pull/{number}"),
        author,
        state,
        is_draft: false,
        created_at: created,
        updated_at: Some(created + SignedDuration::from_secs(24 * 60 * 60)),
        closed_at: (state != PrState::Open)
            .then(|| created + SignedDuration::from_secs(2 * 24 * 60 * 60)),
        merged_at: (state == PrState::Merged)
            .then(|| created + SignedDuration::from_secs(2 * 24 * 60 * 60)),
        merged_by: (state == PrState::Merged).then(alice),
        head_sha: None,
        latest_ci: None,
        coverage: CoverageStatus::Complete,
        synced_at: Some(now()),
    }
}

pub fn event(
    id: &str,
    t: EventType,
    actor: Actor,
    repo: &str,
    pr: Option<&str>,
    at: Timestamp,
    meta: EventMeta,
) -> ActivityEvent {
    ActivityEvent {
        event_id: EventId(id.into()),
        event_type: t,
        actor,
        repository_id: repo.into(),
        entity_id: pr.unwrap_or(id).into(),
        parent_pr_id: pr.map(Into::into),
        occurred_at: at,
        observed_at: at,
        source: Source::Graphql,
        url: Some("https://github.com/x/y/pull/1#event".into()),
        meta,
        coverage: CoverageStatus::Complete,
    }
}

fn sha(s: &str) -> EventMeta {
    EventMeta {
        sha: Some(s.into()),
        ..EventMeta::default()
    }
}

fn review(state: ReviewState) -> EventMeta {
    EventMeta {
        review_state: Some(state),
        ..EventMeta::default()
    }
}

fn coverage(start: Timestamp, end: Timestamp) -> Vec<CoverageRecord> {
    DataKind::ALL
        .iter()
        .map(|k| CoverageRecord {
            kind: *k,
            start,
            end,
            status: CoverageStatus::Complete,
            scope: "test".into(),
            reason: None,
            recorded_at: now(),
        })
        .collect()
}

/// A small world with Japanese, emoji and hostile text.
pub fn dataset() -> Dataset {
    let repos = vec![
        repo("R1", "alice", "parser", Visibility::Public),
        repo("R2", "org", "ウェブアプリ", Visibility::Public),
        repo("R3", "alice", "secret", Visibility::Private),
    ];
    let prs = vec![
        pr(
            "P1",
            "R1",
            1,
            "パーサーの修正 🚀",
            alice(),
            PrState::Merged,
            "2026-11-10T01:00:00Z",
        ),
        pr(
            "P2",
            "R2",
            7,
            MALICIOUS_TITLE,
            bob(),
            PrState::Open,
            "2026-11-12T01:00:00Z",
        ),
        pr(
            "P3",
            "R3",
            3,
            "private work",
            alice(),
            PrState::Open,
            "2026-11-18T01:00:00Z",
        ),
    ];
    let t = |s: &str| utc(s);
    let events = vec![
        event(
            "c1",
            EventType::CommitAuthored,
            alice(),
            "R1",
            None,
            t("2026-11-09T03:00:00Z"),
            sha("aaa1"),
        ),
        event(
            "c2",
            EventType::CommitAuthored,
            alice(),
            "R1",
            None,
            t("2026-11-10T03:00:00Z"),
            sha("aaa2"),
        ),
        event(
            "c3",
            EventType::CommitAuthored,
            alice(),
            "R3",
            None,
            t("2026-11-18T03:00:00Z"),
            sha("aaa3"),
        ),
        event(
            "p1o",
            EventType::PrOpened,
            alice(),
            "R1",
            Some("P1"),
            t("2026-11-10T01:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p1c",
            EventType::PrCommit,
            alice(),
            "R1",
            Some("P1"),
            t("2026-11-10T03:00:00Z"),
            sha("aaa2"),
        ),
        event(
            "p1r",
            EventType::ReviewSubmitted,
            bob(),
            "R1",
            Some("P1"),
            t("2026-11-11T01:00:00Z"),
            review(ReviewState::Approved),
        ),
        event(
            "p1i",
            EventType::InlineComment,
            bob(),
            "R1",
            Some("P1"),
            t("2026-11-11T01:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p1k",
            EventType::CheckRun,
            bot(),
            "R1",
            Some("P1"),
            t("2026-11-11T02:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p1m",
            EventType::PrMerged,
            alice(),
            "R1",
            Some("P1"),
            t("2026-11-12T01:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p2o",
            EventType::PrOpened,
            bob(),
            "R2",
            Some("P2"),
            t("2026-11-12T01:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p2r",
            EventType::ReviewSubmitted,
            alice(),
            "R2",
            Some("P2"),
            t("2026-11-13T01:00:00Z"),
            review(ReviewState::ChangesRequested),
        ),
        event(
            "p2c",
            EventType::ConversationComment,
            alice(),
            "R2",
            Some("P2"),
            t("2026-11-13T02:00:00Z"),
            EventMeta::default(),
        ),
        event(
            "p3o",
            EventType::PrOpened,
            alice(),
            "R3",
            Some("P3"),
            t("2026-11-18T01:00:00Z"),
            EventMeta::default(),
        ),
    ];
    let p = period_30d();
    Dataset::new(
        Subject {
            id: Some("U1".into()),
            login: "alice".into(),
        },
        true,
        repos,
        prs,
        vec![],
        vec![],
        events,
        coverage(
            p.start - SignedDuration::from_secs(400 * 24 * 60 * 60),
            p.end,
        ),
    )
}

pub fn empty_dataset() -> Dataset {
    Dataset::new(
        Subject {
            id: Some("U1".into()),
            login: "alice".into(),
        },
        true,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    )
}

pub fn config(background_sync: bool) -> AppConfig {
    AppConfig {
        period: period_30d(),
        zone: zone(),
        filter: Filter::default(),
        background_sync,
    }
}

/// UI state computing reports synchronously, for step-by-step checks.
pub fn app_with(ds: Dataset, background_sync: bool) -> App {
    let mut app = App::inline(
        Box::new(MemorySource::new(ds)),
        config(background_sync),
        Box::new(now),
    );
    app.prepare();
    app
}

/// UI state computing reports on a worker thread, as the real UI does.
pub fn threaded_app(source: Box<dyn DataSource + Send>) -> App {
    let mut app = App::with_clock(source, config(false), Box::new(now)).unwrap();
    app.prepare();
    app
}

pub const SETTLE: std::time::Duration = std::time::Duration::from_secs(30);

pub fn app() -> App {
    app_with(dataset(), false)
}

pub fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

pub fn code(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

pub fn press(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.handle_key(key(c));
    }
}

/// Renders the app and returns the screen as lines of text.
pub fn render(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ghusage_tui::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            let mut line = String::new();
            for x in 0..w {
                line.push_str(buf[(x, y)].symbol());
            }
            line
        })
        .collect()
}

pub fn screen_text(app: &App, w: u16, h: u16) -> String {
    render(app, w, h).join("\n")
}

/// No escape or other control characters anywhere in the rendered buffer.
pub fn assert_clean(app: &App, w: u16, h: u16) {
    let text = screen_text(app, w, h);
    for c in text.chars() {
        assert!(
            c == '\n' || !c.is_control(),
            "control character {c:?} rendered on screen:\n{text}"
        );
        assert!(
            !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'),
            "bidi override rendered"
        );
    }
}
