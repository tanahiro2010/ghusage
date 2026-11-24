//! Generated-data properties of the cache: resyncing a pull request any
//! number of times leaves exactly the state of the latest fetch.

use std::collections::{BTreeMap, BTreeSet};

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    self, CommitDateBasis, Dataset, Filter, Options, Participation, PrSort,
};
use ghusage_core::model::{
    ActivityEvent, Actor, ActorKind, Collection, EventId, EventMeta, EventType, PrState,
    PullRequest, Repository, ReviewState, ReviewThread, Source, Subject, Visibility,
};
use ghusage_core::time::{Period, Zone};
use ghusage_storage::{PrCommitLink, PrSnapshot, Store, WriteBatch};
use jiff::{SignedDuration, Timestamp};
use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};

const PR: &str = "PR1";
const REPO: &str = "R1";
const COMMENTS: usize = 6;
const REVIEWS: usize = 4;
const THREADS: usize = 4;
const SHAS: usize = 6;

fn config() -> Config {
    Config {
        cases: 32,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/properties.txt",
        ))),
        ..Config::default()
    }
}

fn at(day: u32, hour: u32) -> Timestamp {
    format!("2026-03-{day:02}T{hour:02}:00:00Z")
        .parse()
        .unwrap()
}

fn user(login: &str) -> Actor {
    Actor::new(Some(format!("U_{login}")), Some(login.into()), Some("User"))
}

fn me() -> Subject {
    Subject {
        id: Some("U_me".into()),
        login: "me".into(),
    }
}

fn repository() -> Repository {
    Repository {
        id: REPO.into(),
        owner: "me".into(),
        name: "tool".into(),
        owner_kind: ActorKind::User,
        visibility: Visibility::Public,
        is_fork: false,
        viewer_permission: Some("ADMIN".into()),
        accessible: true,
        url: Some("https://github.com/me/tool".into()),
    }
}

fn sha(k: usize) -> String {
    format!("{k:02}c0ffee{k:02}")
}

/// One fetch of the pull request as GitHub would return it at that moment.
#[derive(Debug, Clone)]
struct Fetch {
    state: u8,
    comments: u8,
    edited: u8,
    reviews: u8,
    review_states: u8,
    threads: u8,
    resolved: u8,
    commits: u8,
}

fn fetch() -> impl Strategy<Value = Fetch> {
    (
        0u8..3,
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
    )
        .prop_map(
            |(state, comments, edited, reviews, review_states, threads, resolved, commits)| Fetch {
                state,
                comments,
                edited,
                reviews,
                review_states,
                threads,
                resolved,
                commits,
            },
        )
}

const fn bit(mask: u8, k: usize) -> bool {
    mask >> k & 1 == 1
}

/// Everything one fetch writes, plus the expected state derived from it.
struct Built {
    pr: PullRequest,
    events: Vec<ActivityEvent>,
    threads: Vec<ReviewThread>,
    shas: BTreeSet<String>,
    observed_at: Timestamp,
}

fn base_event(id: EventId, ty: EventType, actor: Actor, occurred: Timestamp) -> ActivityEvent {
    ActivityEvent {
        entity_id: id.0.clone(),
        event_id: id,
        event_type: ty,
        actor,
        repository_id: REPO.into(),
        parent_pr_id: Some(PR.into()),
        occurred_at: occurred,
        observed_at: occurred,
        source: Source::Graphql,
        url: None,
        meta: EventMeta::default(),
        coverage: CoverageStatus::Complete,
    }
}

/// `merged_before` is whether an earlier fetch already saw the merge; a
/// merge is never undone, so later fetches stay merged.
fn build(f: &Fetch, index: usize, merged_before: bool) -> Built {
    // Fetches happen in April, after all activity (March), one day apart.
    let observed_at = "2026-04-01T00:00:00Z".parse::<Timestamp>().unwrap()
        + SignedDuration::from_secs(
            i64::try_from(index).expect("test index fits i64") * 24 * 60 * 60,
        );
    let merged = merged_before || f.state == 2;
    let state = if merged {
        PrState::Merged
    } else if f.state == 1 {
        PrState::Closed
    } else {
        PrState::Open
    };
    let merged_at = merged.then(|| at(20, 12));

    let mut events = Vec::new();
    events.push(base_event(
        EventId::pr_created(PR),
        EventType::PrOpened,
        user("me"),
        at(2, 9),
    ));
    if merged {
        events.push(base_event(
            EventId::pr_merged(PR),
            EventType::PrMerged,
            user("bob"),
            at(20, 12),
        ));
    }
    for k in (0..COMMENTS).filter(|&k| bit(f.comments, k)) {
        let author = if k % 2 == 0 { "me" } else { "bob" };
        let mut e = base_event(
            EventId::comment(&format!("C{k}")),
            EventType::ConversationComment,
            user(author),
            at(3 + u32::try_from(k).expect("fixture index fits u32"), 10),
        );
        if bit(f.edited, k) {
            e.meta.edited_at = Some(observed_at - SignedDuration::from_secs(60 * 60));
        }
        events.push(e);
    }
    for k in (0..REVIEWS).filter(|&k| bit(f.reviews, k)) {
        // Reviews 0..2 come from other people, review 3 is a self-review.
        let author = ["bob", "carol", "bob", "me"][k];
        let mut e = base_event(
            EventId::review(&format!("RV{k}")),
            EventType::ReviewSubmitted,
            user(author),
            at(10 + u32::try_from(k).expect("fixture index fits u32"), 15),
        );
        e.meta.review_id = Some(format!("RV{k}"));
        // A dismissal or re-request can change what the latest fetch reports.
        e.meta.review_state = Some(if bit(f.review_states, k) {
            ReviewState::Approved
        } else {
            ReviewState::ChangesRequested
        });
        events.push(e);
    }
    let mut threads = Vec::new();
    for k in (0..THREADS).filter(|&k| bit(f.threads, k)) {
        let starter = if k % 2 == 0 { "bob" } else { "me" };
        threads.push(ReviewThread {
            id: format!("TH{k}"),
            pr_id: PR.into(),
            is_resolved: bit(f.resolved, k),
            is_outdated: false,
            resolved_by: bit(f.resolved, k).then(|| user("me")),
            path: Some("src/lib.rs".into()),
            comment_count: 2,
            first_comment_author: Some(user(starter)),
        });
        // My reply in the thread.
        let mut e = base_event(
            EventId::comment(&format!("IC{k}")),
            EventType::InlineComment,
            user("me"),
            at(14 + u32::try_from(k).expect("fixture index fits u32"), 8),
        );
        e.meta.thread_id = Some(format!("TH{k}"));
        e.meta.reply_to = Some(format!("ROOT{k}"));
        e.meta.review_id = Some(format!("RVT{k}"));
        events.push(e);
    }
    let mut shas = BTreeSet::new();
    for k in (0..SHAS).filter(|&k| bit(f.commits, k)) {
        let s = sha(k);
        let mut e = base_event(
            EventId::pr_commit(PR, &s),
            EventType::PrCommit,
            user("me"),
            at(2 + u32::try_from(k).expect("fixture index fits u32"), 11),
        );
        e.entity_id.clone_from(&s);
        e.meta.sha = Some(s.clone());
        e.meta.time_basis = Some("commit_date".into());
        events.push(e);
        shas.insert(s);
    }
    for e in &mut events {
        e.observed_at = observed_at;
    }

    let pr = PullRequest {
        id: PR.into(),
        repository_id: REPO.into(),
        number: 1,
        title: "Change".into(),
        url: "https://github.com/me/tool/pull/1".into(),
        author: user("me"),
        state,
        is_draft: false,
        created_at: at(2, 9),
        updated_at: Some(observed_at - SignedDuration::from_secs(60 * 60)),
        closed_at: (state != PrState::Open).then(|| at(20, 12)),
        merged_at,
        merged_by: merged.then(|| user("bob")),
        // After a force push the head is whatever the newest commit is now.
        head_sha: shas.iter().next_back().cloned(),
        latest_ci: None,
        coverage: CoverageStatus::Complete,
        synced_at: Some(observed_at),
    };
    Built {
        pr,
        events,
        threads,
        shas,
        observed_at,
    }
}

fn batch(b: &Built) -> WriteBatch {
    WriteBatch {
        account: "me".into(),
        repositories: vec![repository()],
        pull_requests: vec![b.pr.clone()],
        threads: b.threads.clone(),
        events: b.events.clone(),
        pr_commits: b
            .shas
            .iter()
            .map(|s| PrCommitLink {
                pr_id: PR.into(),
                repository_id: REPO.into(),
                sha: s.clone(),
                is_current: true,
            })
            .collect(),
        pr_snapshots: vec![PrSnapshot {
            pr_id: PR.into(),
            observed_at: b.observed_at,
            // Every collection except CI checks was fetched completely.
            complete_collections: vec![
                Collection::Commits,
                Collection::Reviews,
                Collection::Comments,
                Collection::ReviewComments,
                Collection::Timeline,
            ],
            present: b.events.iter().map(|e| e.event_id.clone()).collect(),
            present_threads: b.threads.iter().map(|t| t.id.clone()).collect(),
        }],
        ..Default::default()
    }
}

fn march() -> Options {
    Options {
        period: Period::new(at(1, 0), "2026-04-01T00:00:00Z".parse().unwrap()).unwrap(),
        zone: Zone::utc(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
    }
}

fn dataset(s: &Store) -> Dataset {
    s.load_dataset(&me(), true, None).unwrap()
}

fn commit_links(path: &std::path::Path) -> BTreeMap<String, bool> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT sha, is_current FROM pull_request_commits WHERE pr_id = ?1")
        .unwrap();
    stmt.query_map([PR], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

proptest! {
    #![proptest_config(config())]

    /// Applies fetches S1..Sn in order and checks the cache against Sn alone.
    #[test]
    fn resync_state_matches_the_latest_snapshot(fetches in prop::collection::vec(fetch(), 1..=5)) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ghusage.db");
        let mut store = Store::open(&path).unwrap();

        let mut built = Vec::new();
        let mut merged = false;
        for (i, f) in fetches.iter().enumerate() {
            let b = build(f, i, merged);
            merged |= b.pr.state == PrState::Merged;
            store.apply(&batch(&b)).unwrap();
            built.push(b);
        }
        let last = built.last().unwrap();
        let ds = dataset(&store);

        // (a) The pull request's current state is the latest fetch's.
        prop_assert_eq!(&ds.pull_requests[PR], &last.pr);

        // (b) The counted events are exactly the latest fetch's, with its
        // values (edits, review states).
        let latest: BTreeMap<&str, &ActivityEvent> =
            last.events.iter().map(|e| (e.event_id.as_str(), e)).collect();
        let live: BTreeMap<&str, &ActivityEvent> = ds
            .events
            .iter()
            .filter(|e| !e.is_removed())
            .map(|e| (e.event_id.as_str(), e))
            .collect();
        prop_assert_eq!(&live, &latest);
        let threads: Vec<&ReviewThread> = ds.threads.values().collect();
        let mut expected_threads: Vec<&ReviewThread> = last.threads.iter().collect();
        expected_threads.sort_by(|a, b| a.id.cmp(&b.id));
        prop_assert_eq!(threads, expected_threads);

        // (c) Anything seen earlier but missing now is kept as history,
        // marked removed at the first fetch that no longer had it.
        for (i, b) in built.iter().enumerate() {
            for e in &b.events {
                let id = e.event_id.as_str();
                if latest.contains_key(id) {
                    continue;
                }
                let stored = ds.events.iter().find(|s| s.event_id.as_str() == id);
                prop_assert!(stored.is_some(), "{} was dropped", id);
                // The last fetch that still had it, then the next one.
                let last_seen = built
                    .iter()
                    .rposition(|x| x.events.iter().any(|y| y.event_id.as_str() == id))
                    .unwrap();
                prop_assert!(last_seen >= i);
                prop_assert_eq!(
                    stored.unwrap().meta.removed_observed_at,
                    Some(built[last_seen + 1].observed_at),
                    "{}", id
                );
            }
        }
        let ever: BTreeSet<String> = built.iter().flat_map(|b| b.shas.iter().cloned()).collect();
        let expected_links: BTreeMap<String, bool> = ever
            .iter()
            .map(|s| (s.clone(), last.shas.contains(s)))
            .collect();
        prop_assert_eq!(commit_links(&path), expected_links);

        // (d) Aggregates equal those of a fresh cache holding only Sn.
        let mut fresh = Store::open_in_memory().unwrap();
        fresh.apply(&batch(last)).unwrap();
        let fresh_ds = dataset(&fresh);
        let opts = march();
        prop_assert_eq!(metrics::summary(&ds, &opts), metrics::summary(&fresh_ds, &opts));
        prop_assert_eq!(metrics::snapshot(&ds, &opts), metrics::snapshot(&fresh_ds, &opts));
        prop_assert_eq!(
            metrics::pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity),
            metrics::pr_rows(&fresh_ds, &opts, Participation::All, None, PrSort::Activity)
        );
    }
}
