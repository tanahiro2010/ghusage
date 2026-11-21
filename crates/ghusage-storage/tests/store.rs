//! Behavior of the cache database as seen by the rest of the application.

use std::collections::HashSet;

use chrono::{DateTime, Duration, TimeZone, Utc};
use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::model::{
    ActivityEvent, Actor, ActorKind, Collection, EventId, EventMeta, EventType, Issue, PrState,
    PullRequest, Repository, ReviewThread, Source, Subject, Visibility,
};
use ghusage_core::time::Period;
use ghusage_storage::{PrCommitLink, PrSnapshot, StorageError, Store, SyncLock, WriteBatch};

fn t(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, day, hour, 0, 0).unwrap()
}

fn user(login: &str) -> Actor {
    Actor::new(Some(format!("U_{login}")), Some(login.into()), Some("User"))
}

fn subject(login: &str) -> Subject {
    Subject {
        id: Some(format!("U_{login}")),
        login: login.into(),
    }
}

fn repo(id: &str, owner: &str, name: &str) -> Repository {
    Repository {
        id: id.into(),
        owner: owner.into(),
        name: name.into(),
        owner_kind: ActorKind::User,
        visibility: Visibility::Public,
        is_fork: false,
        viewer_permission: Some("ADMIN".into()),
        accessible: true,
        url: Some(format!("https://github.com/{owner}/{name}")),
    }
}

fn pr(id: &str, repo_id: &str, number: u64) -> PullRequest {
    PullRequest {
        id: id.into(),
        repository_id: repo_id.into(),
        number,
        title: format!("Change {number}"),
        url: format!("https://github.com/o/r/pull/{number}"),
        author: user("alice"),
        state: PrState::Open,
        is_draft: false,
        created_at: t(1, 9),
        updated_at: Some(t(2, 9)),
        closed_at: None,
        merged_at: None,
        merged_by: None,
        head_sha: Some("abc".into()),
        latest_ci: None,
        coverage: CoverageStatus::Complete,
        synced_at: Some(t(3, 0)),
    }
}

fn event(
    id: EventId,
    ty: EventType,
    repo_id: &str,
    pr: Option<&str>,
    at: DateTime<Utc>,
) -> ActivityEvent {
    ActivityEvent {
        event_id: id.clone(),
        event_type: ty,
        actor: user("alice"),
        repository_id: repo_id.into(),
        entity_id: id.0.clone(),
        parent_pr_id: pr.map(Into::into),
        occurred_at: at,
        observed_at: t(10, 0),
        source: Source::Graphql,
        url: None,
        meta: EventMeta::default(),
        coverage: CoverageStatus::Complete,
    }
}

fn commit_event(repo_id: &str, sha: &str, at: DateTime<Utc>) -> ActivityEvent {
    let mut e = event(
        EventId::commit(repo_id, sha),
        EventType::CommitAuthored,
        repo_id,
        None,
        at,
    );
    e.entity_id = sha.into();
    e.meta.sha = Some(sha.into());
    e.source = Source::Search;
    e
}

fn pr_commit_event(pr_id: &str, repo_id: &str, sha: &str, at: DateTime<Utc>) -> ActivityEvent {
    let mut e = event(
        EventId::pr_commit(pr_id, sha),
        EventType::PrCommit,
        repo_id,
        Some(pr_id),
        at,
    );
    e.entity_id = sha.into();
    e.meta.sha = Some(sha.into());
    e.meta.time_basis = Some("commit_date".into());
    e
}

fn comment_event(pr_id: &str, comment: &str, at: DateTime<Utc>) -> ActivityEvent {
    event(
        EventId::comment(comment),
        EventType::ConversationComment,
        "R1",
        Some(pr_id),
        at,
    )
}

fn thread(id: &str, pr_id: &str) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        pr_id: pr_id.into(),
        is_resolved: false,
        is_outdated: false,
        resolved_by: None,
        path: Some("src/main.rs".into()),
        comment_count: 1,
        first_comment_author: Some(user("bob")),
    }
}

fn base_batch() -> WriteBatch {
    let mut review = event(
        EventId::review("RV1"),
        EventType::ReviewSubmitted,
        "R1",
        Some("PR1"),
        t(2, 12),
    );
    review.meta.review_state = Some(ghusage_core::model::ReviewState::Approved);
    WriteBatch {
        account: "alice".into(),
        repositories: vec![repo("R1", "alice", "tool"), repo("R2", "acme", "lib")],
        pull_requests: vec![pr("PR1", "R1", 1)],
        issues: vec![Issue {
            id: "I1".into(),
            repository_id: "R2".into(),
            number: 7,
            title: "Bug".into(),
            url: "https://github.com/acme/lib/issues/7".into(),
            author: user("alice"),
            is_open: true,
            created_at: t(1, 8),
            closed_at: None,
        }],
        threads: vec![thread("TH1", "PR1")],
        events: vec![
            commit_event("R1", "aaa111", t(1, 10)),
            event(
                EventId::pr_created("PR1"),
                EventType::PrOpened,
                "R1",
                Some("PR1"),
                t(1, 9),
            ),
            review,
            comment_event("PR1", "C1", t(2, 13)),
            event(
                EventId::issue_created("I1"),
                EventType::IssueOpened,
                "R2",
                None,
                t(1, 8),
            ),
        ],
        pr_commits: vec![],
        pr_snapshots: vec![],
        coverage: vec![CoverageRecord {
            kind: DataKind::Commits,
            start: t(1, 0),
            end: t(8, 0),
            status: CoverageStatus::Complete,
            scope: "commit search".into(),
            reason: None,
            recorded_at: t(10, 0),
        }],
        cursors: vec![("commits".into(), "cursor-1".into())],
    }
}

fn store_with_base() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.apply(&base_batch()).unwrap();
    s
}

fn alice_dataset(s: &Store) -> ghusage_core::metrics::Dataset {
    s.load_dataset(&subject("alice"), true, None).unwrap()
}

fn event_by_id<'a>(ds: &'a ghusage_core::metrics::Dataset, id: &EventId) -> &'a ActivityEvent {
    ds.events
        .iter()
        .find(|e| &e.event_id == id)
        .unwrap_or_else(|| panic!("event {id} not loaded"))
}

// ---------------------------------------------------------------------------
// Writing

#[test]
fn stored_data_reads_back_unchanged() {
    let s = store_with_base();
    let ds = alice_dataset(&s);
    let b = base_batch();
    assert_eq!(ds.repositories.len(), 2);
    assert_eq!(ds.repositories["R1"], b.repositories[0]);
    assert_eq!(ds.pull_requests["PR1"], b.pull_requests[0]);
    assert_eq!(ds.issues["I1"], b.issues[0]);
    assert_eq!(ds.threads["TH1"], b.threads[0]);
    assert_eq!(ds.coverage, b.coverage);
    assert_eq!(ds.events.len(), b.events.len());
    for e in &b.events {
        assert_eq!(event_by_id(&ds, &e.event_id), e);
    }
    assert_eq!(
        s.cursor("alice", "commits").unwrap().as_deref(),
        Some("cursor-1")
    );
    assert_eq!(
        s.cursor("ALICE", "commits").unwrap().as_deref(),
        Some("cursor-1")
    );
    assert_eq!(s.cursor("alice", "missing").unwrap(), None);
    assert!(s.integrity_check().unwrap().is_empty());
}

#[test]
fn sub_second_timestamps_round_trip_exactly() {
    let mut s = store_with_base();
    let precise = t(4, 1) + Duration::nanoseconds(123_456_789);
    let mut e = commit_event("R1", "bbb222", precise);
    e.meta.author_date = Some(precise - Duration::milliseconds(1500));
    e.observed_at = precise;
    let mut b = WriteBatch {
        account: "alice".into(),
        ..Default::default()
    };
    b.events.push(e.clone());
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(event_by_id(&ds, &e.event_id), &e);
}

#[test]
fn reapplying_the_same_batch_changes_nothing() {
    let mut s = store_with_base();
    let before = s.stats().unwrap();
    let ds_before = alice_dataset(&s);
    s.apply(&base_batch()).unwrap();
    s.apply(&base_batch()).unwrap();
    let after = s.stats().unwrap();
    assert_eq!(
        (
            before.repositories,
            before.pull_requests,
            before.issues,
            before.events,
            before.accounts
        ),
        (
            after.repositories,
            after.pull_requests,
            after.issues,
            after.events,
            after.accounts
        )
    );
    let ds_after = alice_dataset(&s);
    assert_eq!(ds_before.events, ds_after.events);
    assert_eq!(ds_before.coverage, ds_after.coverage);
    assert_eq!(ds_after.coverage.len(), 1);
}

#[test]
fn same_commit_from_two_sources_is_stored_once() {
    let mut s = store_with_base();
    // The same commit observed through pull request data instead of search.
    let mut again = commit_event("R1", "AAA111", t(1, 10));
    again.source = Source::Graphql;
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![again],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    let commits: Vec<_> = ds
        .events
        .iter()
        .filter(|e| e.event_type == EventType::CommitAuthored)
        .collect();
    assert_eq!(commits.len(), 1);
}

#[test]
fn same_sha_in_two_repositories_is_two_commits() {
    let mut s = store_with_base();
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![
            commit_event("R1", "ccc333", t(3, 1)),
            commit_event("R2", "ccc333", t(3, 1)),
        ],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    let n = ds
        .events
        .iter()
        .filter(|e| e.meta.sha.as_deref() == Some("ccc333"))
        .count();
    assert_eq!(n, 2);
}

#[test]
fn renamed_repository_keeps_its_id_and_data() {
    let mut s = store_with_base();
    let b = WriteBatch {
        account: "alice".into(),
        repositories: vec![repo("R1", "alice-org", "tool-renamed")],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(ds.repositories.len(), 2);
    assert_eq!(ds.repositories["R1"].full_name(), "alice-org/tool-renamed");
    assert_eq!(
        s.find_pr("ALICE-ORG", "Tool-Renamed", 1)
            .unwrap()
            .as_deref(),
        Some("PR1")
    );
    assert_eq!(s.find_pr("alice", "tool", 1).unwrap(), None);
    assert_eq!(ds.pull_requests["PR1"].repository_id, "R1");
}

#[test]
fn pull_request_found_case_insensitively() {
    let s = store_with_base();
    assert_eq!(
        s.find_pr("Alice", "TOOL", 1).unwrap().as_deref(),
        Some("PR1")
    );
    assert_eq!(s.find_pr("alice", "tool", 2).unwrap(), None);
}

#[test]
fn later_merge_updates_the_pull_request() {
    let mut s = store_with_base();
    let mut merged = pr("PR1", "R1", 1);
    merged.state = PrState::Merged;
    merged.merged_at = Some(t(5, 0));
    merged.merged_by = Some(user("bob"));
    let b = WriteBatch {
        account: "alice".into(),
        pull_requests: vec![merged.clone()],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert_eq!(alice_dataset(&s).pull_requests["PR1"], merged);
    let states = s.pr_sync_states().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].pr_id, "PR1");
    assert_eq!(states[0].updated_at, merged.updated_at);
    assert_eq!(states[0].synced_at, merged.synced_at);
}

#[test]
fn repository_can_be_marked_inaccessible_keeping_its_data() {
    let mut s = store_with_base();
    assert!(s.set_repository_accessible("ALICE", "Tool", false).unwrap());
    assert!(
        !s.set_repository_accessible("nobody", "none", false)
            .unwrap()
    );
    let ds = alice_dataset(&s);
    assert!(!ds.repositories["R1"].accessible);
    assert!(ds.pull_requests.contains_key("PR1"));
    assert!(ds.events.iter().any(|e| e.repository_id == "R1"));
}

// ---------------------------------------------------------------------------
// Removal observations

fn snapshot(
    pr_id: &str,
    at: DateTime<Utc>,
    colls: &[Collection],
    present: &[EventId],
) -> PrSnapshot {
    PrSnapshot {
        pr_id: pr_id.into(),
        observed_at: at,
        complete_collections: colls.to_vec(),
        present: present.iter().cloned().collect(),
        present_threads: HashSet::new(),
    }
}

#[test]
fn comment_missing_from_complete_resync_is_marked_removed_then_restored() {
    let mut s = store_with_base();
    let c1 = EventId::comment("C1");
    let review = EventId::review("RV1");

    // Comments fetched completely, C1 absent: marked removed. Reviews were not
    // fetched completely, so the review is untouched.
    let b = WriteBatch {
        account: "alice".into(),
        pr_snapshots: vec![snapshot("PR1", t(11, 0), &[Collection::Comments], &[])],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(
        event_by_id(&ds, &c1).meta.removed_observed_at,
        Some(t(11, 0))
    );
    assert!(!event_by_id(&ds, &review).is_removed());

    // A later resync still missing it keeps the earliest observation.
    let b = WriteBatch {
        account: "alice".into(),
        pr_snapshots: vec![snapshot("PR1", t(12, 0), &[Collection::Comments], &[])],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(
        event_by_id(&ds, &c1).meta.removed_observed_at,
        Some(t(11, 0))
    );

    // Re-upserting the event without a complete resync does not clear it.
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![comment_event("PR1", "C1", t(2, 13))],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert!(event_by_id(&alice_dataset(&s), &c1).is_removed());

    // When it is seen again in a complete resync, the mark is cleared.
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![comment_event("PR1", "C1", t(2, 13))],
        pr_snapshots: vec![snapshot(
            "PR1",
            t(13, 0),
            &[Collection::Comments],
            std::slice::from_ref(&c1),
        )],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert!(!event_by_id(&alice_dataset(&s), &c1).is_removed());
}

#[test]
fn review_thread_missing_from_complete_resync_is_deleted() {
    let mut s = store_with_base();
    let mut b = WriteBatch {
        account: "alice".into(),
        threads: vec![thread("TH2", "PR1")],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert_eq!(alice_dataset(&s).threads.len(), 2);

    let mut snap = snapshot("PR1", t(11, 0), &[Collection::ReviewComments], &[]);
    snap.present_threads.insert("TH2".into());
    b.threads.clear();
    b.pr_snapshots = vec![snap];
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(ds.threads.keys().collect::<Vec<_>>(), vec!["TH2"]);
}

#[test]
fn force_push_keeps_old_commit_link_and_marks_its_event_removed() {
    let mut s = store_with_base();
    let old = EventId::pr_commit("PR1", "old1");
    let new = EventId::pr_commit("PR1", "new1");
    let link = |sha: &str, current: bool| PrCommitLink {
        pr_id: "PR1".into(),
        repository_id: "R1".into(),
        sha: sha.into(),
        is_current: current,
    };

    let b = WriteBatch {
        account: "alice".into(),
        events: vec![pr_commit_event("PR1", "R1", "old1", t(1, 11))],
        pr_commits: vec![link("old1", true)],
        pr_snapshots: vec![snapshot(
            "PR1",
            t(10, 0),
            &[Collection::Commits],
            std::slice::from_ref(&old),
        )],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert!(!event_by_id(&alice_dataset(&s), &old).is_removed());

    // After a force push only the new commit is current.
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![pr_commit_event("PR1", "R1", "new1", t(4, 11))],
        pr_commits: vec![link("new1", true)],
        pr_snapshots: vec![snapshot(
            "PR1",
            t(11, 0),
            &[Collection::Commits],
            std::slice::from_ref(&new),
        )],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(
        event_by_id(&ds, &old).meta.removed_observed_at,
        Some(t(11, 0))
    );
    assert!(!event_by_id(&ds, &new).is_removed());
    // The history of the old commit is kept.
    assert_eq!(
        ds.events
            .iter()
            .filter(|e| e.event_type == EventType::PrCommit)
            .count(),
        2
    );
    assert!(s.integrity_check().unwrap().is_empty());

    // An explicit non-current link also marks the event removed.
    let b = WriteBatch {
        account: "alice".into(),
        pr_commits: vec![link("new1", false)],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    assert!(event_by_id(&alice_dataset(&s), &new).is_removed());
}

// ---------------------------------------------------------------------------
// Atomicity and validation

#[test]
fn failing_batch_persists_nothing() {
    let mut s = store_with_base();
    let before = s.stats().unwrap();
    let mut b = WriteBatch {
        account: "alice".into(),
        repositories: vec![repo("R3", "x", "new-repo")],
        pull_requests: vec![pr("PR9", "R3", 9)],
        events: vec![commit_event("R3", "ddd444", t(5, 0))],
        coverage: vec![CoverageRecord {
            kind: DataKind::Issues,
            start: t(1, 0),
            end: t(2, 0),
            status: CoverageStatus::Partial,
            scope: "issues".into(),
            reason: Some("rate limited".into()),
            recorded_at: t(11, 0),
        }],
        cursors: vec![("commits".into(), "cursor-2".into())],
        ..Default::default()
    };
    // The last event refers to a repository that does not exist anywhere.
    b.events.push(commit_event("R404", "eee555", t(5, 0)));
    let err = s.apply(&b).unwrap_err();
    assert!(matches!(err, StorageError::Data(_)), "{err}");
    assert!(err.to_string().contains("R404"));

    let after = s.stats().unwrap();
    assert_eq!(before, after);
    assert_eq!(
        s.cursor("alice", "commits").unwrap().as_deref(),
        Some("cursor-1")
    );
    assert_eq!(s.coverage("alice").unwrap().len(), 1);
    assert_eq!(s.find_pr("x", "new-repo", 9).unwrap(), None);
}

#[test]
fn events_of_an_unknown_pull_request_are_rejected() {
    let mut s = store_with_base();
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![comment_event("PR404", "C9", t(3, 0))],
        ..Default::default()
    };
    assert!(matches!(s.apply(&b), Err(StorageError::Data(_))));
    assert_eq!(s.stats().unwrap().events, 5);
}

#[test]
fn hostile_looking_text_is_stored_verbatim() {
    let mut s = store_with_base();
    let evil = "'; DROP TABLE activity_events; --";
    let mut p = pr("PR2", "R1", 2);
    p.title = evil.into();
    let mut e = comment_event("PR2", evil, t(3, 0));
    e.actor = Actor::new(None, Some(evil.into()), None);
    e.url = Some(format!("https://example.com/?q={evil}"));
    e.meta.target = Some("\" OR 1=1 --".into());
    let b = WriteBatch {
        account: evil.into(),
        repositories: vec![repo("R9", evil, "x\"y")],
        pull_requests: vec![p.clone()],
        events: vec![e.clone()],
        cursors: vec![(evil.into(), evil.into())],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    s.set_meta(evil, evil).unwrap();
    assert_eq!(s.meta(evil).unwrap().as_deref(), Some(evil));
    assert_eq!(s.cursor(evil, evil).unwrap().as_deref(), Some(evil));
    let ds = s.load_dataset(&subject(evil), false, None).unwrap();
    assert_eq!(ds.pull_requests["PR2"].title, evil);
    assert_eq!(event_by_id(&ds, &e.event_id), &e);
    assert_eq!(ds.repositories["R9"].owner, evil);
    assert_eq!(s.find_pr(evil, "x\"y", 2).unwrap(), None);
    assert_eq!(s.stats().unwrap().events, 6);
    assert!(s.integrity_check().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Loading

#[test]
fn range_selects_by_event_time_or_commit_author_date() {
    let mut s = store_with_base();
    // Committed on the 20th, authored on the 5th.
    let mut rebased = commit_event("R1", "fff666", t(20, 0));
    rebased.meta.author_date = Some(t(5, 0));
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![rebased.clone(), commit_event("R1", "ggg777", t(25, 0))],
        ..Default::default()
    };
    s.apply(&b).unwrap();

    let first_week = Period::new(t(1, 0), t(8, 0)).unwrap();
    let ds = s
        .load_dataset(&subject("alice"), true, Some(first_week))
        .unwrap();
    let ids: HashSet<_> = ds.events.iter().map(|e| e.event_id.clone()).collect();
    assert_eq!(ids.len(), 6, "{ids:?}");
    assert!(ids.contains(&rebased.event_id));
    assert!(!ids.contains(&EventId::commit("R1", "ggg777")));
    // Entities are loaded regardless of the range.
    assert_eq!(ds.pull_requests.len(), 1);
    assert_eq!(ds.issues.len(), 1);

    // The end of the range is exclusive, also at sub-second precision.
    let edge = Period::new(t(1, 0), t(1, 10)).unwrap();
    let ds = s.load_dataset(&subject("alice"), true, Some(edge)).unwrap();
    assert!(
        !ds.events
            .iter()
            .any(|e| e.event_id == EventId::commit("R1", "aaa111"))
    );
    let just_after = Period::new(t(1, 10), t(1, 10) + Duration::nanoseconds(1)).unwrap();
    let ds = s
        .load_dataset(&subject("alice"), true, Some(just_after))
        .unwrap();
    assert_eq!(ds.events.len(), 1);
}

#[test]
fn accounts_do_not_see_each_others_data() {
    let mut s = store_with_base();
    let b = WriteBatch {
        account: "bob".into(),
        repositories: vec![repo("R1", "alice", "tool"), repo("RB", "bob", "own")],
        events: vec![commit_event("RB", "bbb000", t(2, 0))],
        coverage: vec![],
        cursors: vec![("commits".into(), "bob-cursor".into())],
        ..Default::default()
    };
    s.apply(&b).unwrap();

    let bob = s.load_dataset(&subject("bob"), false, None).unwrap();
    assert_eq!(bob.events.len(), 1);
    assert_eq!(bob.repositories.len(), 2);
    assert!(bob.pull_requests.is_empty());
    assert!(bob.coverage.is_empty());

    let alice = alice_dataset(&s);
    assert_eq!(alice.events.len(), 5);
    assert!(!alice.repositories.contains_key("RB"));
    assert_eq!(
        s.cursor("alice", "commits").unwrap().as_deref(),
        Some("cursor-1")
    );
    assert_eq!(
        s.cursor("bob", "commits").unwrap().as_deref(),
        Some("bob-cursor")
    );

    let nobody = s.load_dataset(&subject("carol"), false, None).unwrap();
    assert!(nobody.events.is_empty() && nobody.repositories.is_empty());
    // Shared repository rows are not duplicated.
    assert_eq!(s.stats().unwrap().repositories, 3);
}

#[test]
fn pull_request_dataset_contains_all_its_activity() {
    let s = store_with_base();
    let ds = s.load_pr_dataset(&subject("alice"), true, "PR1").unwrap();
    assert_eq!(ds.pull_requests.len(), 1);
    assert_eq!(ds.events.len(), 3);
    assert!(
        ds.events
            .iter()
            .all(|e| e.parent_pr_id.as_deref() == Some("PR1"))
    );
    assert_eq!(ds.threads.len(), 1);
    assert_eq!(ds.repositories.keys().collect::<Vec<_>>(), vec!["R1"]);
    assert_eq!(ds.coverage.len(), 1);
    let none = s.load_pr_dataset(&subject("alice"), true, "PR404").unwrap();
    assert!(none.pull_requests.is_empty() && none.events.is_empty());
}

// ---------------------------------------------------------------------------
// Sync bookkeeping, stats, clearing

#[test]
fn sync_runs_are_recorded() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.last_sync_run("alice").unwrap(), None);
    let period = Period::new(t(1, 0), t(8, 0)).unwrap();
    let first = s.begin_sync_run("alice", Some(period), t(9, 0)).unwrap();
    s.finish_sync_run(first, "success", t(9, 1), Some("{\"requests\":3}"), None)
        .unwrap();
    let second = s.begin_sync_run("alice", None, t(10, 0)).unwrap();
    let run = s.last_sync_run("Alice").unwrap().unwrap();
    assert_eq!(run.id, second);
    assert_eq!(run.status, "running");
    assert_eq!(run.finished_at, None);
    s.finish_sync_run(second, "failed", t(10, 2), None, Some("network down"))
        .unwrap();
    let run = s.last_sync_run("alice").unwrap().unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(run.message.as_deref(), Some("network down"));
    assert_eq!(run.finished_at, Some(t(10, 2)));
    assert!(matches!(
        s.finish_sync_run(999, "success", t(10, 3), None, None),
        Err(StorageError::Data(_))
    ));
    assert_eq!(s.last_sync_run("bob").unwrap(), None);
}

#[test]
fn stats_report_counts_and_file_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    let mut s = Store::open(&path).unwrap();
    s.apply(&base_batch()).unwrap();
    let st = s.stats().unwrap();
    assert_eq!(st.path.as_deref(), Some(path.as_path()));
    assert!(st.file_size_bytes > 0);
    assert_eq!(st.schema_version, Store::supported_schema_version());
    assert_eq!(s.schema_version().unwrap(), st.schema_version);
    assert_eq!(
        (
            st.repositories,
            st.pull_requests,
            st.issues,
            st.events,
            st.accounts
        ),
        (2, 1, 1, 5, 1)
    );
}

#[test]
fn clearing_removes_data_but_keeps_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    let mut s = Store::open(&path).unwrap();
    s.apply(&base_batch()).unwrap();
    s.set_meta("k", "v").unwrap();
    s.begin_sync_run("alice", None, t(9, 0)).unwrap();
    s.clear().unwrap();
    let st = s.stats().unwrap();
    assert_eq!(
        (
            st.repositories,
            st.pull_requests,
            st.issues,
            st.events,
            st.accounts
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(st.schema_version, Store::supported_schema_version());
    assert_eq!(s.meta("k").unwrap(), None);
    assert_eq!(s.last_sync_run("alice").unwrap(), None);
    assert!(s.integrity_check().unwrap().is_empty());
    // Still usable afterwards.
    s.apply(&base_batch()).unwrap();
    assert_eq!(s.stats().unwrap().events, 5);
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.stats().unwrap().events, 5);
}

// ---------------------------------------------------------------------------
// Files on disk: persistence, corruption, versions, permissions, locking

#[test]
fn data_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("ghusage.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.apply(&base_batch()).unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(alice_dataset(&s).events.len(), 5);
    assert!(s.integrity_check().unwrap().is_empty());
}

#[test]
fn file_that_is_not_a_database_is_reported_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    std::fs::write(
        &path,
        b"this is definitely not sqlite, just some garbage bytes \x00\xff",
    )
    .unwrap();
    let err = Store::open(&path).err().expect("garbage must not open");
    assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
    assert!(err.to_string().contains("not a SQLite database"), "{err}");
    // The file is left alone.
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(b"this is definitely")
    );
}

#[test]
fn truncated_database_is_reported_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    {
        let mut s = Store::open(&path).unwrap();
        let mut b = base_batch();
        // Enough rows to span many pages.
        for i in 0..2000 {
            b.events
                .push(commit_event("R1", &format!("{i:040x}"), t(3, 0)));
        }
        s.apply(&b).unwrap();
    }
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 8192);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(len / 3).unwrap();
    drop(file);

    let result = Store::open(&path).and_then(|s| {
        s.load_dataset(&subject("alice"), true, None)?;
        let problems = s.integrity_check()?;
        if problems.is_empty() {
            Ok(())
        } else {
            Err(StorageError::Corrupt(problems.join("; ")))
        }
    });
    let err = result.expect_err("a truncated database must be detected");
    assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
}

#[test]
fn database_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE future (x); PRAGMA user_version = 4242;")
            .unwrap();
    }
    let err = Store::open(&path)
        .err()
        .expect("newer schema must be refused");
    match err {
        StorageError::TooNew { found, supported } => {
            assert_eq!(found, 4242);
            assert_eq!(supported, Store::supported_schema_version());
        }
        other => panic!("unexpected error {other:?}"),
    }
    // Nothing was changed.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 4242);
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn running_as_root() -> bool {
    // Permission checks do not apply to the superuser.
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[cfg(unix)]
#[test]
fn read_only_cache_returns_an_error_and_keeps_data() {
    if running_as_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.apply(&base_batch()).unwrap();
    }
    set_mode(&path, 0o444);
    set_mode(dir.path(), 0o555);

    let mut extra = WriteBatch {
        account: "alice".into(),
        ..Default::default()
    };
    extra.events.push(commit_event("R1", "zzz999", t(6, 0)));
    match Store::open(&path) {
        Ok(mut s) => assert!(s.apply(&extra).is_err()),
        Err(e) => assert!(!e.to_string().is_empty()),
    }

    set_mode(dir.path(), 0o755);
    set_mode(&path, 0o644);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.stats().unwrap().events, 5);
    assert!(s.integrity_check().unwrap().is_empty());
}

#[test]
fn second_sync_lock_is_refused_until_the_first_is_released() {
    let dir = tempfile::tempdir().unwrap();
    let first = SyncLock::acquire(dir.path()).unwrap();
    assert_eq!(first.path(), dir.path().join("sync.lock"));
    match SyncLock::acquire(dir.path()) {
        Err(StorageError::Locked(p)) => assert_eq!(p, dir.path().join("sync.lock")),
        Err(other) => panic!("unexpected error {other:?}"),
        Ok(_) => panic!("second lock must be refused"),
    }
    drop(first);
    let again = SyncLock::acquire(dir.path()).unwrap();
    drop(again);
}

#[test]
fn lock_is_refused_across_threads() {
    let dir = tempfile::tempdir().unwrap();
    let _held = SyncLock::acquire(dir.path()).unwrap();
    let d = dir.path().to_path_buf();
    let other = std::thread::spawn(move || SyncLock::acquire(&d).map(|_| ()))
        .join()
        .unwrap();
    assert!(matches!(other, Err(StorageError::Locked(_))));
}

#[test]
fn two_connections_can_read_while_one_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    let mut writer = Store::open(&path).unwrap();
    writer.apply(&base_batch()).unwrap();
    let reader = Store::open(&path).unwrap();
    let mut b = WriteBatch {
        account: "alice".into(),
        ..Default::default()
    };
    b.events.push(commit_event("R1", "hhh888", t(6, 0)));
    writer.apply(&b).unwrap();
    assert_eq!(alice_dataset(&reader).events.len(), 6);
}

#[test]
fn actor_kinds_and_unknown_actors_round_trip() {
    let mut s = store_with_base();
    let mut e = comment_event("PR1", "C2", t(3, 0));
    e.actor = Actor::unknown();
    let mut e2 = comment_event("PR1", "C3", t(3, 1));
    e2.actor = Actor::new(Some("B1".into()), Some("ci[bot]".into()), None);
    let b = WriteBatch {
        account: "alice".into(),
        events: vec![e.clone(), e2.clone()],
        ..Default::default()
    };
    s.apply(&b).unwrap();
    let ds = alice_dataset(&s);
    assert_eq!(event_by_id(&ds, &e.event_id).actor, Actor::unknown());
    assert_eq!(event_by_id(&ds, &e2.event_id).actor.kind, ActorKind::Bot);
}
