//! Property-based checks of the aggregation engine over randomly generated
//! activity: order independence, idempotent import, partition consistency,
//! time-zone safety, unique-count stability and coverage integrity.
//!
//! Failing cases are saved under `proptest-regressions/` and replayed first on
//! the next run.

#[expect(
    dead_code,
    reason = "Shared fixtures are compiled separately for each integration test target, and this target uses only a subset."
)]
mod common;

use common::*;
use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind, summarize};
use ghusage_core::metrics::{
    self, BucketSummary, CommitDateBasis, Dataset, Filter, Options, Participation, PrRow, PrSort,
    RepoSummary, Summary,
};
use ghusage_core::model::{
    ActivityEvent, Actor, CheckOutcome, EventId, EventType, Issue, PrState, PullRequest,
    ReviewState, ReviewThread, Source, Visibility,
};
use ghusage_core::time::{Granularity, Period, Zone};
use jiff::{SignedDuration, Timestamp};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use std::collections::{BTreeSet, HashSet};

fn config() -> Config {
    Config {
        cases: 256,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/properties.txt",
        ))),
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------
// Fixed world: repositories, actors, pull requests
// ---------------------------------------------------------------------------

const ZONES: [&str; 5] = [
    "Asia/Tokyo",
    "UTC",
    "America/New_York",
    "Europe/London",
    "Australia/Lord_Howe",
];
const REPOS: [&str; 3] = ["R0", "R1", "R2"];
/// Roughly two years of seconds starting at `base()`.
const SPAN: i64 = 2 * 365 * 86_400;
const SHAS: usize = 12;

fn base() -> Timestamp {
    utc("2025-01-01T00:00:00Z")
}

fn actors() -> Vec<Actor> {
    vec![
        me(),
        // the user identified by login only, in another letter case
        Actor::new(None, Some("ME".into()), Some("User")),
        user("alice"),
        user("bob"),
        bot("dependabot[bot]"),
        Actor::unknown(),
    ]
}

fn is_me(a: &Actor) -> bool {
    a.id.as_deref() == Some("U_me")
        || a.login
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("me"))
}

fn repositories() -> Vec<ghusage_core::model::Repository> {
    let mut r1 = repo("R1", "acme", "app", Visibility::Private);
    r1.viewer_permission = Some("WRITE".into());
    vec![
        repo("R0", "me", "tool", Visibility::Public),
        r1,
        repo("R2", "other", "lib", Visibility::Public),
    ]
}

/// (pr id, repository, author index into `actors()`)
const PRS: [(&str, &str, usize); 6] = [
    ("P0", "R0", 0),
    ("P1", "R0", 2),
    ("P2", "R1", 0),
    ("P3", "R1", 3),
    ("P4", "R2", 5),
    ("P5", "R2", 1),
];

fn pull_requests(states: &[u8]) -> Vec<PullRequest> {
    let a = actors();
    PRS.iter()
        .enumerate()
        .map(|(i, (id, r, author))| {
            let mut p = pull(id, r, i as u64 + 1, a[*author].clone());
            p.state = match states.get(i).copied().unwrap_or(0) % 3 {
                0 => PrState::Open,
                1 => PrState::Closed,
                _ => PrState::Merged,
            };
            p
        })
        .collect()
}

fn threads(starters: &[usize]) -> Vec<ReviewThread> {
    let a = actors();
    starters
        .iter()
        .enumerate()
        .map(|(i, s)| thread(&format!("T{i}"), PRS[i % PRS.len()].0, Some(a[*s].clone())))
        .collect()
}

// ---------------------------------------------------------------------------
// Random events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RawEvent {
    ty: usize,
    actor: usize,
    pr: usize,
    repo: usize,
    secs: i64,
    snap: u8,
    sha: usize,
    flags: u16,
    author_shift: i64,
}

fn raw_event() -> impl Strategy<Value = RawEvent> {
    (
        0..EventType::ALL.len(),
        0..6usize,
        0..PRS.len(),
        0..REPOS.len(),
        0..SPAN,
        0..4u8,
        0..SHAS,
        any::<u16>(),
        0..(5 * 86_400i64),
    )
        .prop_map(
            |(ty, actor, pr, repo, secs, snap, sha, flags, author_shift)| RawEvent {
                ty,
                actor,
                pr,
                repo,
                secs,
                snap,
                sha,
                flags,
                author_shift,
            },
        )
}

fn timestamp(secs: i64, snap: u8) -> Timestamp {
    // Bias some instants onto hour/day boundaries, where off-by-one errors live.
    let s = match snap {
        0 => secs - secs % 86_400,
        1 => secs - secs % 3_600,
        2 => secs - secs % 1_800,
        _ => secs,
    };
    base() + SignedDuration::from_secs(s)
}

fn build_event(i: usize, r: &RawEvent) -> ActivityEvent {
    use EventType::*;
    let ty = EventType::ALL[r.ty];
    let actor = actors()[r.actor].clone();
    let (pr_id, pr_repo, _) = PRS[r.pr];
    let at = timestamp(r.secs, r.snap);
    let sha = format!("{:040x}", r.sha + 1);
    let n = format!("n{i}");
    let f = r.flags;
    let (id, repo_id, pr): (EventId, &str, Option<&str>) = match ty {
        CommitAuthored => (EventId::commit(REPOS[r.repo], &sha), REPOS[r.repo], None),
        IssueOpened => (EventId::issue_created(&n), REPOS[r.repo], None),
        IssueClosed | IssueReopened => (EventId::timeline(&n), REPOS[r.repo], None),
        PrOpened => (EventId::pr_created(pr_id), pr_repo, Some(pr_id)),
        PrMerged => (EventId::pr_merged(pr_id), pr_repo, Some(pr_id)),
        PrCommit => (EventId::pr_commit(pr_id, &sha), pr_repo, Some(pr_id)),
        ReviewSubmitted => (EventId::review(&n), pr_repo, Some(pr_id)),
        ReviewSummary => (EventId::review_summary(&n), pr_repo, Some(pr_id)),
        ConversationComment | InlineComment => (EventId::comment(&n), pr_repo, Some(pr_id)),
        CheckRun => (EventId::check_run(&n), pr_repo, Some(pr_id)),
        CommitStatus => (EventId::commit_status(&n), pr_repo, Some(pr_id)),
        _ => (EventId::timeline(&n), pr_repo, Some(pr_id)),
    };
    let mut e = event(id, ty, actor, repo_id, pr, at);
    if matches!(ty, IssueOpened | IssueClosed | IssueReopened) {
        e.entity_id = format!("I{}", r.sha);
    }
    let m = &mut e.meta;
    if matches!(ty, CommitAuthored | PrCommit) {
        m.sha = Some(sha);
        m.author_date = Some(at - SignedDuration::from_secs(r.author_shift));
        m.is_merge_commit = Some(f & 1 == 1);
    }
    if ty == ReviewSubmitted {
        m.review_state = Some(
            [
                ReviewState::Approved,
                ReviewState::ChangesRequested,
                ReviewState::Commented,
                ReviewState::Dismissed,
                ReviewState::Pending,
            ][usize::from(f % 5)],
        );
    }
    if ty == InlineComment {
        m.thread_id = Some(format!("T{}", (f >> 3) % 8));
        if f & 4 == 4 {
            m.reply_to = Some("parent".into());
        }
    }
    if matches!(ty, CheckRun | CommitStatus) {
        m.check_outcome = Some(
            [
                CheckOutcome::Success,
                CheckOutcome::Failure,
                CheckOutcome::Pending,
                CheckOutcome::Neutral,
                CheckOutcome::Skipped,
                CheckOutcome::Cancelled,
                CheckOutcome::Unknown,
            ][usize::from(f % 7)],
        );
        if f & 0x100 == 0x100 {
            m.check_outcome = None;
        }
    }
    if ty == PrClosed {
        m.via_merge = Some(f & 2 == 2);
    }
    // about one event in sixteen was observed as removed on a later resync
    if (f >> 12) == 0xF {
        m.removed_observed_at = Some(utc(OBSERVED));
    }
    e
}

/// Builds events with unique ids (the first event generated for an id wins).
fn unique_events(raw: &[RawEvent]) -> Vec<ActivityEvent> {
    let mut seen = HashSet::new();
    raw.iter()
        .enumerate()
        .map(|(i, r)| build_event(i, r))
        .filter(|e| seen.insert(e.event_id.clone()))
        .collect()
}

#[derive(Debug, Clone)]
struct World {
    events: Vec<ActivityEvent>,
    pr_states: Vec<u8>,
    thread_starters: Vec<usize>,
}

impl World {
    fn dataset_with(&self, events: Vec<ActivityEvent>) -> Dataset {
        Dataset::new(
            subject(),
            true,
            repositories(),
            pull_requests(&self.pr_states),
            Vec::<Issue>::new(),
            threads(&self.thread_starters),
            events,
            Vec::new(),
        )
    }
    fn dataset(&self) -> Dataset {
        self.dataset_with(self.events.clone())
    }
}

fn world() -> impl Strategy<Value = World> {
    (
        prop::collection::vec(raw_event(), 0..120),
        prop::collection::vec(any::<u8>(), PRS.len()),
        prop::collection::vec(0..6usize, 8),
    )
        .prop_map(|(raw, pr_states, thread_starters)| World {
            events: unique_events(&raw),
            pr_states,
            thread_starters,
        })
}

fn opts_strategy() -> impl Strategy<Value = Options> {
    (0..SPAN, 1..SPAN, 0..ZONES.len(), any::<bool>(), 0..4u8).prop_map(
        |(start, len, z, author_basis, snap)| {
            let start_t = timestamp(start, snap);
            let end_t = (start_t + SignedDuration::from_secs(len))
                .min(base() + SignedDuration::from_secs(SPAN));
            let (s, e) = if start_t < end_t {
                (start_t, end_t)
            } else {
                (base(), base() + SignedDuration::from_secs(SPAN))
            };
            Options {
                period: Period::new(s, e).unwrap(),
                zone: Zone::parse(ZONES[z]).unwrap(),
                filter: Filter::default(),
                commit_basis: if author_basis {
                    CommitDateBasis::Author
                } else {
                    CommitDateBasis::Committer
                },
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Result helpers
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Results {
    summary: Summary,
    days: Vec<BucketSummary>,
    weeks: Vec<BucketSummary>,
    months: Vec<BucketSummary>,
    repos: Vec<RepoSummary>,
    rows_subject: Vec<PrRow>,
    rows_all: Vec<PrRow>,
}

fn results(ds: &Dataset, o: &Options) -> Results {
    Results {
        summary: metrics::summary(ds, o),
        days: metrics::breakdown(ds, o, Granularity::Day),
        weeks: metrics::breakdown(ds, o, Granularity::Week),
        months: metrics::breakdown(ds, o, Granularity::Month),
        repos: metrics::by_repository(ds, o),
        rows_subject: metrics::pr_rows(ds, o, Participation::Subject, None, PrSort::Activity),
        rows_all: metrics::pr_rows(ds, o, Participation::All, None, PrSort::Activity),
    }
}

/// Counts that add up across a partition of time.
fn additive(s: &Summary) -> Vec<(&'static str, u64)> {
    vec![
        ("commits", s.commits),
        ("merge_commits", s.merge_commits),
        ("prs_created", s.prs_created),
        ("prs_merged", s.prs_merged),
        ("merges_performed", s.merges_performed),
        ("merges_unknown_actor", s.merges_unknown_actor),
        ("prs_closed_without_merge", s.prs_closed_without_merge),
        ("prs_reopened", s.prs_reopened),
        ("reviews_submitted.total", s.reviews_submitted.total),
        ("reviews_submitted.approved", s.reviews_submitted.approved),
        (
            "reviews_submitted.changes_requested",
            s.reviews_submitted.changes_requested,
        ),
        ("reviews_submitted.commented", s.reviews_submitted.commented),
        (
            "reviews_submitted.dismissed",
            s.reviews_submitted.dismissed_unknown_state,
        ),
        ("reviews_received.total", s.reviews_received.total),
        ("reviews_received_from_bots", s.reviews_received_from_bots),
        ("review_dismissals_performed", s.review_dismissals_performed),
        (
            "comments_written.conversation",
            s.comments_written.conversation,
        ),
        ("comments_written.inline", s.comments_written.inline),
        (
            "comments_written.review_summary",
            s.comments_written.review_summary,
        ),
        (
            "comments_received.conversation",
            s.comments_received.conversation,
        ),
        ("comments_received.inline", s.comments_received.inline),
        (
            "comments_received.review_summary",
            s.comments_received.review_summary,
        ),
        ("review_responses", s.review_responses),
        ("pr_commits", s.pr_commits),
        ("force_pushes", s.force_pushes),
        ("coordination_actions", s.coordination_actions),
        ("metadata_actions", s.metadata_actions),
        ("issues_opened", s.issues_opened),
        ("issues_closed", s.issues_closed),
        ("issues_reopened", s.issues_reopened),
        ("pr_internal_events", s.pr_internal_events),
        ("ci.passed", s.ci.passed),
        ("ci.failed", s.ci.failed),
        ("ci.pending", s.ci.pending),
        ("ci.other", s.ci.other),
    ]
}

fn sum_additive<'a>(parts: impl Iterator<Item = &'a Summary>) -> Vec<(&'static str, u64)> {
    let mut acc = additive(&Summary::default());
    for s in parts {
        for (a, b) in acc.iter_mut().zip(additive(s)) {
            a.1 += b.1;
        }
    }
    acc
}

fn countable(e: &ActivityEvent) -> bool {
    e.meta.removed_observed_at.is_none()
        && !(e.event_type == EventType::ReviewSubmitted
            && e.meta.review_state == Some(ReviewState::Pending))
}

fn time_of(e: &ActivityEvent, basis: CommitDateBasis) -> Timestamp {
    match (e.event_type, basis) {
        (EventType::CommitAuthored | EventType::PrCommit, CommitDateBasis::Author) => {
            e.meta.author_date.unwrap_or(e.occurred_at)
        }
        _ => e.occurred_at,
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config())]

    /// Input order never changes any result.
    #[test]
    fn shuffled_input_gives_identical_results(
        (w, shuffled) in world().prop_flat_map(|w| {
            let ev = w.events.clone();
            (Just(w), Just(ev).prop_shuffle())
        }),
        o in opts_strategy(),
    ) {
        let a = w.dataset();
        let b = w.dataset_with(shuffled);
        prop_assert_eq!(results(&a, &o), results(&b, &o));
        for (id, _, _) in PRS {
            prop_assert_eq!(
                metrics::pr_detail(&a, id, &[]),
                metrics::pr_detail(&b, id, &[])
            );
        }
    }

    /// Importing the same events again, any number of times and from other
    /// sources, never changes the results.
    #[test]
    fn duplicated_events_change_nothing(
        w in world(),
        dups in prop::collection::vec((any::<prop::sample::Index>(), 1..4usize, 0..3u8), 0..40),
        o in opts_strategy(),
    ) {
        prop_assume!(!w.events.is_empty());
        let mut events = w.events.clone();
        for (idx, times, variant) in dups {
            let original = idx.get(&w.events);
            for k in 0..times {
                let mut copy = original.clone();
                match variant {
                    0 => {}
                    1 => {
                        copy.source = Source::Rest;
                        copy.observed_at = utc("2027-01-01T00:00:00Z") + SignedDuration::from_hours(i64::try_from(k).expect("test index fits i64"));
                    }
                    _ => {
                        copy.source = Source::Search;
                        copy.observed_at = utc(OBSERVED) + SignedDuration::from_hours(i64::try_from(k).expect("test index fits i64") + 1);
                    }
                }
                events.push(copy);
            }
        }
        let once = w.dataset();
        let many = w.dataset_with(events);
        prop_assert_eq!(once.events.len(), many.events.len());
        prop_assert_eq!(results(&once, &o), results(&many, &o));
    }

    /// Splitting a period into consecutive half-open parts: additive counts
    /// of the parts sum to the whole.
    #[test]
    fn additive_counts_sum_over_any_split(
        w in world(),
        o in opts_strategy(),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
    ) {
        let ds = w.dataset();
        let len = (o.period.end - o.period.start).as_secs();
        let mut points: BTreeSet<Timestamp> = cuts
            .iter()
            .map(|c| o.period.start + SignedDuration::from_secs(i64::try_from(c.index(usize::try_from(len).expect("test period fits usize"))).expect("test index fits i64")))
            .filter(|t| *t > o.period.start)
            .collect();
        points.insert(o.period.start);
        points.insert(o.period.end);
        let points: Vec<_> = points.into_iter().collect();
        let parts: Vec<Summary> = points
            .windows(2)
            .map(|p| {
                let mut po = o.clone();
                po.period = Period::new(p[0], p[1]).unwrap();
                metrics::summary(&ds, &po)
            })
            .collect();
        let whole = metrics::summary(&ds, &o);
        prop_assert_eq!(sum_additive(parts.iter()), additive(&whole));
        prop_assert_eq!(
            parts.iter().filter_map(|s| s.last_activity).max(),
            whole.last_activity
        );
    }

    /// Daily, weekly and monthly buckets partition the period in every zone
    /// and their counts add up to the period summary.
    #[test]
    fn calendar_buckets_sum_to_the_summary(w in world(), o in opts_strategy()) {
        let ds = w.dataset();
        let whole = metrics::summary(&ds, &o);
        for g in [Granularity::Day, Granularity::Week, Granularity::Month] {
            let b = metrics::breakdown(&ds, &o, g);
            prop_assert!(!b.is_empty());
            prop_assert_eq!(b[0].bucket.start, o.period.start);
            prop_assert_eq!(b[b.len() - 1].bucket.end, o.period.end);
            for pair in b.windows(2) {
                prop_assert_eq!(pair[0].bucket.end, pair[1].bucket.start);
                prop_assert!(pair[0].bucket.key < pair[1].bucket.key);
            }
            prop_assert_eq!(sum_additive(b.iter().map(|x| &x.summary)), additive(&whole));
            // a local date belongs to exactly one day/week/month bucket
            prop_assert_eq!(
                b.iter().map(|x| x.summary.active_days).sum::<u64>(),
                whole.active_days
            );
            prop_assert_eq!(
                b.iter().filter_map(|x| x.summary.last_activity).max(),
                whole.last_activity
            );
        }
    }

    /// Every event belongs to exactly one repository, so per-repository rows
    /// add up to the period summary for every additive count. A pull request
    /// also lives in one repository, so unique pull request counts add up too.
    #[test]
    fn repository_rows_sum_to_the_summary(w in world(), o in opts_strategy()) {
        let ds = w.dataset();
        let whole = metrics::summary(&ds, &o);
        let rows = metrics::by_repository(&ds, &o);
        let ids: HashSet<&str> = rows.iter().map(|r| r.repository_id.as_str()).collect();
        prop_assert_eq!(ids.len(), rows.len(), "one row per repository");
        prop_assert_eq!(sum_additive(rows.iter().map(|r| &r.summary)), additive(&whole));
        let unique = |f: fn(&Summary) -> u64| rows.iter().map(|r| f(&r.summary)).sum::<u64>();
        prop_assert_eq!(unique(|s| s.reviewed_prs), whole.reviewed_prs);
        prop_assert_eq!(unique(|s| s.active_prs), whole.active_prs);
        prop_assert_eq!(
            unique(|s| s.active_prs_all_participants),
            whole.active_prs_all_participants
        );
        prop_assert_eq!(
            rows.iter().filter_map(|r| r.summary.last_activity).max(),
            whole.last_activity
        );
        // Active days are not additive (one day can have activity in two
        // repositories) but no repository can have more than the whole.
        for r in &rows {
            prop_assert!(r.summary.active_days <= whole.active_days);
        }
    }

    /// Converting UTC instants into any zone loses no event: per-day counts in
    /// every zone add up to the same totals.
    #[test]
    fn no_event_is_lost_in_any_zone(w in world(), o in opts_strategy()) {
        let ds = w.dataset();
        let mut reference = o.clone();
        reference.zone = Zone::utc();
        let expected = additive(&metrics::summary(&ds, &reference));
        for name in ZONES {
            let mut zo = o.clone();
            zo.zone = Zone::parse(name).unwrap();
            let days = metrics::breakdown(&ds, &zo, Granularity::Day);
            prop_assert_eq!(sum_additive(days.iter().map(|x| &x.summary)), expected.clone(), "zone {}", name);
            let covered_secs: i64 = days.iter().map(|d| (d.bucket.end - d.bucket.start).as_secs()).sum();
            prop_assert_eq!(covered_secs, (o.period.end - o.period.start).as_secs());
        }
    }

    /// Counts derived directly from the definitions: reviews and comments land
    /// in their own fields, commits count distinct (repository, sha).
    #[test]
    fn counts_match_their_definitions(w in world(), o in opts_strategy()) {
        let ds = w.dataset();
        let s = metrics::summary(&ds, &o);
        let in_scope: Vec<&ActivityEvent> = w
            .events
            .iter()
            .filter(|e| countable(e) && o.period.contains(time_of(e, o.commit_basis)))
            .collect();
        let mine = |t: EventType| {
            in_scope.iter().filter(|e| e.event_type == t && is_me(&e.actor)).count() as u64
        };
        let commits: HashSet<(&str, &str)> = in_scope
            .iter()
            .filter(|e| e.event_type == EventType::CommitAuthored && is_me(&e.actor))
            .map(|e| (e.repository_id.as_str(), e.meta.sha.as_deref().unwrap()))
            .collect();
        prop_assert_eq!(s.commits, commits.len() as u64);
        prop_assert_eq!(s.prs_created, mine(EventType::PrOpened));
        prop_assert_eq!(s.reviews_submitted.total, mine(EventType::ReviewSubmitted));
        prop_assert_eq!(s.comments_written.conversation, mine(EventType::ConversationComment));
        prop_assert_eq!(s.comments_written.inline, mine(EventType::InlineComment));
        prop_assert_eq!(s.comments_written.review_summary, mine(EventType::ReviewSummary));
        prop_assert_eq!(s.issues_opened, mine(EventType::IssueOpened));
        prop_assert_eq!(s.force_pushes, mine(EventType::PrHeadForcePushed));
        let ci = in_scope.iter().filter(|e| e.event_type.is_automation()).count() as u64;
        prop_assert_eq!(s.ci.passed + s.ci.failed + s.ci.pending + s.ci.other, ci);
        // human activity inside pull requests never includes automation
        let human_pr = in_scope
            .iter()
            .filter(|e| is_me(&e.actor) && e.event_type.is_pr_event() && !e.event_type.is_automation())
            .count() as u64;
        prop_assert_eq!(s.pr_internal_events, human_pr);
        let reviewed: HashSet<&str> = in_scope
            .iter()
            .filter(|e| e.event_type == EventType::ReviewSubmitted && is_me(&e.actor))
            .filter_map(|e| e.parent_pr_id.as_deref())
            .collect();
        prop_assert_eq!(s.reviewed_prs, reviewed.len() as u64);
    }

    /// Review submissions and review summaries / inline comments are separate
    /// fields: adding one never moves the other.
    #[test]
    fn review_submissions_and_review_comments_stay_separate(
        w in world(),
        o in opts_strategy(),
        pr in 0..PRS.len(),
        at in any::<prop::sample::Index>(),
        kind in 0..3usize,
    ) {
        let len = usize::try_from((o.period.end - o.period.start).as_secs()).expect("test period fits usize");
        let t = o.period.start + SignedDuration::from_secs(i64::try_from(at.index(len)).expect("test index fits i64"));
        let (pr_id, repo_id, _) = PRS[pr];
        let (ty, id) = match kind {
            0 => (EventType::ReviewSubmitted, EventId::review("added")),
            1 => (EventType::ReviewSummary, EventId::review_summary("added")),
            _ => (EventType::InlineComment, EventId::comment("added")),
        };
        let mut e = event(id, ty, me(), repo_id, Some(pr_id), t);
        e.meta.review_state = Some(ReviewState::Commented);
        let before = metrics::summary(&w.dataset(), &o);
        let mut events = w.events.clone();
        events.push(e);
        let after = metrics::summary(&w.dataset_with(events), &o);
        let dw = after.comments_written.total() - before.comments_written.total();
        let dr = after.reviews_submitted.total - before.reviews_submitted.total;
        match ty {
            EventType::ReviewSubmitted => prop_assert_eq!((dr, dw), (1, 0)),
            EventType::ReviewSummary => {
                prop_assert_eq!((dr, dw), (0, 1));
                prop_assert_eq!(after.comments_written.review_summary, before.comments_written.review_summary + 1);
            }
            _ => {
                prop_assert_eq!((dr, dw), (0, 1));
                prop_assert_eq!(after.comments_written.inline, before.comments_written.inline + 1);
            }
        }
        prop_assert_eq!(after.reviews_received, before.reviews_received);
    }

    /// Any number of reviews on one pull request moves unique PR counts by at
    /// most one, and not at all if the PR was already counted.
    #[test]
    fn many_reviews_on_one_pr_keep_unique_pr_counts(
        w in world(),
        o in opts_strategy(),
        pr in 0..PRS.len(),
        added in prop::collection::vec((0..6usize, 0..5usize, any::<prop::sample::Index>()), 1..20),
    ) {
        let (pr_id, repo_id, author) = PRS[pr];
        let pr_is_mine = is_me(&actors()[author]);
        let len = usize::try_from((o.period.end - o.period.start).as_secs()).expect("test period fits usize");
        let states = [
            ReviewState::Approved,
            ReviewState::ChangesRequested,
            ReviewState::Commented,
            ReviewState::Dismissed,
            ReviewState::Pending,
        ];
        let base_ds = w.dataset();
        let before = metrics::summary(&base_ds, &o);
        let was_active = metrics::pr_rows(&base_ds, &o, Participation::All, None, PrSort::Activity)
            .iter()
            .any(|r| r.pr_id == pr_id);
        let mut events = w.events.clone();
        let (mut mine, mut others) = (0u64, 0u64);
        for (k, (a, st, at)) in added.iter().enumerate() {
            let actor = actors()[*a].clone();
            let state = states[*st];
            if state != ReviewState::Pending {
                if is_me(&actor) { mine += 1 } else { others += 1 }
            }
            let mut e = event(
                EventId::review(&format!("extra-{k}")),
                EventType::ReviewSubmitted,
                actor,
                repo_id,
                Some(pr_id),
                o.period.start + SignedDuration::from_secs(i64::try_from(at.index(len)).expect("test index fits i64")),
            );
            e.meta.review_state = Some(state);
            events.push(e);
        }
        let after = metrics::summary(&w.dataset_with(events), &o);
        prop_assert!(after.reviewed_prs >= before.reviewed_prs);
        prop_assert!(after.reviewed_prs - before.reviewed_prs <= 1);
        if mine > 0 {
            prop_assert!(after.reviewed_prs >= 1);
        } else {
            prop_assert_eq!(after.reviewed_prs, before.reviewed_prs);
        }
        prop_assert_eq!(after.reviews_submitted.total, before.reviews_submitted.total + mine);
        let expected_received = if pr_is_mine { others } else { 0 };
        prop_assert_eq!(after.reviews_received.total, before.reviews_received.total + expected_received);
        prop_assert!(after.active_prs_all_participants - before.active_prs_all_participants <= 1);
        if was_active {
            prop_assert_eq!(after.active_prs_all_participants, before.active_prs_all_participants);
        }
        prop_assert!(after.active_prs - before.active_prs <= 1);
    }

    /// The same SHA seen as an authored commit and as a pull request commit
    /// never inflates the global commit count.
    #[test]
    fn pr_copies_of_commits_never_double_count(
        w in world(),
        o in opts_strategy(),
        target in prop::collection::vec(0..PRS.len(), 0..64),
    ) {
        let mut events = w.events.clone();
        for (k, e) in w
            .events
            .iter()
            .filter(|e| e.event_type == EventType::CommitAuthored)
            .enumerate()
        {
            let (pr_id, _, _) = PRS[target.get(k).copied().unwrap_or(0)];
            let sha = e.meta.sha.clone().unwrap();
            let mut c = e.clone();
            c.event_id = EventId::pr_commit(pr_id, &sha);
            c.event_type = EventType::PrCommit;
            c.parent_pr_id = Some(pr_id.into());
            events.push(c);
        }
        let before = metrics::summary(&w.dataset(), &o);
        let after = metrics::summary(&w.dataset_with(events), &o);
        prop_assert_eq!(after.commits, before.commits);
        prop_assert_eq!(after.merge_commits, before.merge_commits);
    }

    /// A pull request created in one period and merged in a later one is
    /// never counted as merged in the creation period.
    #[test]
    fn later_merge_is_not_counted_at_creation(
        created in 0..(SPAN / 2),
        delay in 1..(300 * 86_400i64),
        z in 0..ZONES.len(),
        merger in 0..6usize,
    ) {
        let zone = Zone::parse(ZONES[z]).unwrap();
        let t1 = base() + SignedDuration::from_secs(created);
        let t2 = t1 + SignedDuration::from_secs(delay);
        let mut pr = pull("PM", "R0", 1, me());
        pr.state = PrState::Merged;
        pr.created_at = t1;
        pr.merged_at = Some(t2);
        let events = vec![
            event(EventId::pr_created("PM"), EventType::PrOpened, me(), "R0", Some("PM"), t1),
            event(EventId::pr_merged("PM"), EventType::PrMerged, actors()[merger].clone(), "R0", Some("PM"), t2),
        ];
        let ds = Dataset::new(subject(), true, repositories(), vec![pr], Vec::<Issue>::new(), vec![], events, vec![]);
        // creation period: from the start of the creation day up to the merge
        let start = zone.start_of_day(zone.local_date(t1));
        let creation = Options {
            period: Period::new(start, t2).unwrap(),
            zone: zone.clone(),
            filter: Filter::default(),
            commit_basis: CommitDateBasis::Committer,
        };
        let s = metrics::summary(&ds, &creation);
        prop_assert_eq!(s.prs_created, 1);
        prop_assert_eq!(s.prs_merged, 0);
        // daily view: created on t1's local date, merged on t2's local date
        let mut days = creation;
        days.period = Period::new(start, zone.start_of_day(zone.local_date(t2) + SignedDuration::from_hours(24))).unwrap();
        for b in metrics::breakdown(&ds, &days, Granularity::Day) {
            prop_assert_eq!(b.summary.prs_created, u64::from(b.bucket.key == zone.local_date(t1)));
            prop_assert_eq!(b.summary.prs_merged, u64::from(b.bucket.key == zone.local_date(t2)));
        }
    }
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

const STATUSES: [CoverageStatus; 5] = [
    CoverageStatus::Complete,
    CoverageStatus::Partial,
    CoverageStatus::NotAvailable,
    CoverageStatus::NotYetSynced,
    CoverageStatus::Unknown,
];

fn hour(h: i64) -> Timestamp {
    utc("2026-01-01T00:00:00Z") + SignedDuration::from_hours(h)
}

#[derive(Debug, Clone)]
struct RawRecord {
    kind: usize,
    start: i64,
    len: i64,
    status: usize,
}

fn raw_record() -> impl Strategy<Value = RawRecord> {
    // Mostly the queried kind; complete records are favored so full covers
    // happen often enough to matter.
    (
        prop_oneof![4 => Just(0usize), 1 => 1..3usize],
        0..200i64,
        1..150i64,
        prop_oneof![3 => Just(0usize), 2 => 1..5usize],
    )
        .prop_map(|(kind, start, len, status)| RawRecord {
            kind,
            start,
            len,
            status,
        })
}

fn records(raw: &[RawRecord]) -> Vec<CoverageRecord> {
    raw.iter()
        .enumerate()
        .map(|(i, r)| CoverageRecord {
            kind: DataKind::ALL[r.kind],
            start: hour(r.start),
            end: hour(r.start + r.len),
            status: STATUSES[r.status],
            scope: format!("scope-{}", r.kind).as_str().into(),
            description: format!("scope-{}", r.kind),
            reason: None,
            // distinct recording times; later records were recorded later
            recorded_at: hour(10_000 + i64::try_from(i).expect("test index fits i64")),
        })
        .collect()
}

/// Status of one instant: the latest-recorded record covering it, or
/// "not yet synced" when nothing covers it.
fn status_at(recs: &[CoverageRecord], t: Timestamp) -> CoverageStatus {
    recs.iter()
        .filter(|r| r.kind == DataKind::Commits && r.start <= t && t < r.end)
        .max_by_key(|r| r.recorded_at)
        .map_or(CoverageStatus::NotYetSynced, |r| r.status)
}

proptest! {
    #![proptest_config(config())]

    /// Coverage is complete exactly when every instant of the range is covered
    /// by a complete record that was not superseded by a later, incomplete one.
    #[test]
    fn coverage_is_complete_only_with_a_full_complete_cover(
        (raw, order) in prop::collection::vec(raw_record(), 0..8).prop_flat_map(|raw| {
            let idx: Vec<usize> = (0..raw.len()).collect();
            (Just(raw), Just(idx).prop_shuffle())
        }),
        qs in 0..250i64,
        ql in 1..150i64,
    ) {
        let recs = records(&raw);
        let (start, end) = (hour(qs), hour(qs + ql));
        let s = summarize(&recs, DataKind::Commits, start, end);

        // every hour of the range (all boundaries are whole hours)
        let per_hour: Vec<CoverageStatus> = (qs..qs + ql).map(|h| status_at(&recs, hour(h))).collect();
        let all_complete = per_hour.iter().all(|x| x.is_complete());
        prop_assert_eq!(s.status.is_complete(), all_complete);
        if per_hour.contains(&CoverageStatus::NotYetSynced) {
            prop_assert!(!s.status.is_complete(), "a gap must not be complete");
        }
        if s.status.is_complete() {
            prop_assert_eq!(s.gaps().count(), 0);
        } else {
            prop_assert!(s.gaps().count() > 0);
        }

        // The most recently recorded overlapping record being incomplete is
        // enough to rule out a complete status.
        let latest = recs
            .iter()
            .filter(|r| r.kind == DataKind::Commits && r.start < end && r.end > start)
            .max_by_key(|r| r.recorded_at);
        if let Some(r) = latest
            && !r.status.is_complete()
        {
            prop_assert!(!s.status.is_complete());
        }

        // segments partition the range and agree with per-instant status
        prop_assert!(!s.segments.is_empty());
        prop_assert_eq!(s.segments[0].start, start);
        prop_assert_eq!(s.segments[s.segments.len() - 1].end, end);
        for w in s.segments.windows(2) {
            prop_assert_eq!(w[0].end, w[1].start);
        }
        for seg in &s.segments {
            let mut t = seg.start;
            while t < seg.end {
                prop_assert_eq!(seg.status, status_at(&recs, t));
                t += SignedDuration::from_hours(1);
            }
        }

        // record order is irrelevant when recording times differ
        let reordered: Vec<CoverageRecord> = order.iter().map(|&i| recs[i].clone()).collect();
        let s2 = summarize(&reordered, DataKind::Commits, start, end);
        prop_assert_eq!(s.status, s2.status);
        prop_assert_eq!(s.segments, s2.segments);
    }
}
