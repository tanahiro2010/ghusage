//! Hand-built activity datasets with expected results worked out by hand from
//! the product's metric definitions.

mod common;

use common::*;
use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind, summarize};
use ghusage_core::metrics::{
    self, CommitDateBasis, Dataset, Filter, Ownership, Participation, PrSort, ReviewDirection,
    Summary, VisibilityFilter, by_repository, pr_detail, pr_rows, review_rows, snapshot, summary,
};
use ghusage_core::model::{
    ActivityEvent, Actor, EventId, EventType, Issue, PrState, ReviewState, Source, Visibility,
};
use ghusage_core::time::Granularity;

fn review(
    repo_id: &str,
    pr_id: &str,
    review_id: &str,
    actor: Actor,
    state: ReviewState,
    at: &str,
) -> ActivityEvent {
    with_meta(
        event(
            EventId::review(review_id),
            EventType::ReviewSubmitted,
            actor,
            repo_id,
            Some(pr_id),
            utc(at),
        ),
        |m| {
            m.review_state = Some(state);
            m.review_id = Some(review_id.into());
        },
    )
}

fn comment(
    kind: EventType,
    repo_id: &str,
    pr_id: &str,
    comment_id: &str,
    actor: Actor,
    at: &str,
) -> ActivityEvent {
    let id = if kind == EventType::ReviewSummary {
        EventId::review_summary(comment_id)
    } else {
        EventId::comment(comment_id)
    };
    event(id, kind, actor, repo_id, Some(pr_id), utc(at))
}

fn inline_reply(
    repo_id: &str,
    pr_id: &str,
    comment_id: &str,
    thread_id: &str,
    reply_to: Option<&str>,
    actor: Actor,
    at: &str,
) -> ActivityEvent {
    with_meta(
        comment(
            EventType::InlineComment,
            repo_id,
            pr_id,
            comment_id,
            actor,
            at,
        ),
        |m| {
            m.thread_id = Some(thread_id.into());
            m.reply_to = reply_to.map(str::to_string);
        },
    )
}

fn lifecycle(
    id: EventId,
    t: EventType,
    repo_id: &str,
    pr_id: &str,
    actor: Actor,
    at: &str,
) -> ActivityEvent {
    event(id, t, actor, repo_id, Some(pr_id), utc(at))
}

fn tool() -> ghusage_core::model::Repository {
    repo("R_tool", "me", "tool", Visibility::Public)
}

fn november_utc() -> metrics::Options {
    options("2026-11-01T00:00:00Z", "2026-12-01T00:00:00Z", zone("UTC"))
}

// ---------------------------------------------------------------------------
// Empty and single-commit datasets
// ---------------------------------------------------------------------------

#[test]
fn empty_activity_reports_zero_everywhere() {
    let ds = dataset(vec![tool()], vec![], vec![], vec![]);
    let opts = local_days("2026-11-01", "2026-11-08", zone("Asia/Tokyo"));

    assert_eq!(summary(&ds, &opts), Summary::default());
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    // 2026-11-01 .. 2026-11-07 inclusive = 7 local days, each empty.
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|b| b.summary == Summary::default()));
    assert!(by_repository(&ds, &opts).is_empty());
    assert!(pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity).is_empty());
    assert_eq!(snapshot(&ds, &opts), metrics::Snapshot::default());
}

#[test]
fn single_commit_counts_once_on_its_committer_day() {
    let ds = dataset(
        vec![tool()],
        vec![],
        vec![],
        vec![commit("R_tool", "abc123", me(), "2026-11-20T10:00:00Z")],
    );
    let opts = local_days("2026-11-19", "2026-11-22", zone("UTC"));
    let s = summary(&ds, &opts);
    assert_eq!(s.commits, 1);
    assert_eq!(s.merge_commits, 0);
    assert_eq!(s.active_days, 1);
    assert_eq!(s.pr_internal_events, 0);
    assert_eq!(s.last_activity, Some(utc("2026-11-20T10:00:00Z")));

    let per_day: Vec<u64> = metrics::breakdown(&ds, &opts, Granularity::Day)
        .iter()
        .map(|b| b.summary.commits)
        .collect();
    // 11-19: 0, 11-20: 1, 11-21: 0
    assert_eq!(per_day, vec![0, 1, 0]);

    let repos = by_repository(&ds, &opts);
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].full_name, "me/tool");
    assert_eq!(repos[0].summary.commits, 1);
}

#[test]
fn merge_commit_is_identified_separately() {
    let merge = with_meta(commit("R_tool", "m1", me(), "2026-11-20T10:00:00Z"), |m| {
        m.is_merge_commit = Some(true)
    });
    let plain = commit("R_tool", "p1", me(), "2026-11-20T11:00:00Z");
    let ds = dataset(vec![tool()], vec![], vec![], vec![merge, plain]);
    let s = summary(&ds, &november_utc());
    // Two distinct commits, one of which is a merge commit.
    assert_eq!(s.commits, 2);
    assert_eq!(s.merge_commits, 1);
}

#[test]
fn author_date_basis_moves_commit_to_author_day() {
    // Authored 11-18, committed (rebased) 11-20.
    let c = with_meta(commit("R_tool", "abc", me(), "2026-11-20T10:00:00Z"), |m| {
        m.author_date = Some(utc("2026-11-18T09:00:00Z"))
    });
    let ds = dataset(vec![tool()], vec![], vec![], vec![c]);
    let mut opts = local_days("2026-11-17", "2026-11-21", zone("UTC"));
    let committer: Vec<u64> = metrics::breakdown(&ds, &opts, Granularity::Day)
        .iter()
        .map(|b| b.summary.commits)
        .collect();
    // days 17,18,19,20 -> committer date 20
    assert_eq!(committer, vec![0, 0, 0, 1]);
    opts.commit_basis = CommitDateBasis::Author;
    let author: Vec<u64> = metrics::breakdown(&ds, &opts, Granularity::Day)
        .iter()
        .map(|b| b.summary.commits)
        .collect();
    assert_eq!(author, vec![0, 1, 0, 0]);
}

#[test]
fn pr_row_last_activity_follows_the_commit_date_basis() {
    // The only PR event is a commit authored 11-18 and committed 11-20.
    let c = with_meta(
        pr_commit("R_tool", "P1", "abc", me(), "2026-11-20T10:00:00Z"),
        |m| m.author_date = Some(utc("2026-11-18T09:00:00Z")),
    );
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![],
        vec![c],
    );
    let mut opts = november_utc();
    let row = &pr_rows(&ds, &opts, Participation::All, None, PrSort::Updated)[0];
    assert_eq!(row.last_activity, Some(utc("2026-11-20T10:00:00Z")));
    opts.commit_basis = CommitDateBasis::Author;
    let row = &pr_rows(&ds, &opts, Participation::All, None, PrSort::Updated)[0];
    assert_eq!(row.last_activity, Some(utc("2026-11-18T09:00:00Z")));
    assert_eq!(summary(&ds, &opts).last_activity, row.last_activity);
}

// ---------------------------------------------------------------------------
// Commit identity
// ---------------------------------------------------------------------------

#[test]
fn duplicate_sha_in_one_repository_counts_once() {
    let events = vec![
        commit("R_tool", "abc123", me(), "2026-11-20T10:00:00Z"),
        commit("R_tool", "abc123", me(), "2026-11-20T10:00:00Z"),
        // Same SHA in a different letter case is the same commit.
        commit("R_tool", "ABC123", me(), "2026-11-20T10:00:00Z"),
    ];
    let ds = dataset(vec![tool()], vec![], vec![], events);
    assert_eq!(summary(&ds, &november_utc()).commits, 1);
}

#[test]
fn same_sha_in_two_repositories_counts_twice() {
    let fork = repo("R_fork", "me", "tool-fork", Visibility::Public);
    let events = vec![
        commit("R_tool", "abc123", me(), "2026-11-20T10:00:00Z"),
        commit("R_fork", "abc123", me(), "2026-11-20T10:00:00Z"),
    ];
    let ds = dataset(vec![tool(), fork], vec![], vec![], events);
    // (repository, sha) identifies a commit: 2 distinct pairs.
    assert_eq!(summary(&ds, &november_utc()).commits, 2);
}

#[test]
fn commit_seen_globally_and_inside_a_pull_request_counts_once_globally() {
    let pr = pull("P1", "R_tool", 1, me());
    let events = vec![
        commit("R_tool", "abc", me(), "2026-11-20T10:00:00Z"),
        pr_commit("R_tool", "P1", "abc", me(), "2026-11-20T10:00:00Z"),
        commit("R_tool", "def", me(), "2026-11-20T11:00:00Z"),
        pr_commit("R_tool", "P1", "def", me(), "2026-11-20T11:00:00Z"),
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let s = summary(&ds, &november_utc());
    // Global: {abc, def} = 2. PR commits: {abc, def} in P1 = 2.
    assert_eq!(s.commits, 2);
    assert_eq!(s.pr_commits, 2);
}

/// The global commit set is defined as "commits found by commit search" united
/// with "commits of fetched pull requests" authored by the user, unique by
/// (repository, sha). A commit only known from a pull request still belongs
/// to the global set, exactly once.
#[test]
fn pull_request_commits_authored_by_user_join_the_global_commit_set() {
    let pr = pull("P1", "R_tool", 1, me());
    let events = vec![
        // found by commit search
        commit("R_tool", "abc", me(), "2026-11-20T10:00:00Z"),
        // also in the pull request
        pr_commit("R_tool", "P1", "abc", me(), "2026-11-20T10:00:00Z"),
        // only in the pull request (e.g. not on the default branch); sync
        // stores the user's pull request commits as authored commits too
        pr_commit("R_tool", "P1", "def", me(), "2026-11-20T11:00:00Z"),
        commit("R_tool", "def", me(), "2026-11-20T11:00:00Z"),
        // in the pull request but written by someone else
        pr_commit("R_tool", "P1", "fff", user("alice"), "2026-11-20T12:00:00Z"),
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let s = summary(&ds, &november_utc());
    // {abc} ∪ {abc, def} = {abc, def} -> 2 (alice's fff is not the user's)
    assert_eq!(s.commits, 2);
    assert_eq!(s.pr_commits, 2);
}

#[test]
fn multiple_repositories_are_counted_and_ordered_by_activity() {
    let a = repo("R_a", "me", "alpha", Visibility::Public);
    let b = repo("R_b", "acme", "beta", Visibility::Private);
    let c = repo("R_c", "other", "gamma", Visibility::Public);
    let mut events = Vec::new();
    // alpha: 2 commits, beta: 1 commit, gamma: 3 commits
    for (r, n) in [("R_a", 2), ("R_b", 1), ("R_c", 3)] {
        for i in 0..n {
            events.push(commit(
                r,
                &format!("{r}-{i}"),
                me(),
                &format!("2026-11-1{i}T08:00:00Z"),
            ));
        }
    }
    let ds = dataset(vec![a, b, c], vec![], vec![], events);
    let opts = november_utc();
    assert_eq!(summary(&ds, &opts).commits, 6); // 2 + 1 + 3
    let rows = by_repository(&ds, &opts);
    let names: Vec<(&str, u64)> = rows
        .iter()
        .map(|r| (r.full_name.as_str(), r.summary.commits))
        .collect();
    assert_eq!(
        names,
        vec![("other/gamma", 3), ("me/alpha", 2), ("acme/beta", 1)]
    );
    assert_eq!(rows.iter().map(|r| r.summary.commits).sum::<u64>(), 6);
}

// ---------------------------------------------------------------------------
// Pull request lifecycle
// ---------------------------------------------------------------------------

/// Mirrors the product example: a pull request is created late on Nov 20
/// (Tokyo), three review responses and the merge happen on Nov 21 (Tokyo).
fn created_then_merged_next_tokyo_day() -> Dataset {
    let mut pr = pull("P1", "R_tool", 123, me());
    pr.state = PrState::Merged;
    pr.created_at = utc("2026-11-20T14:30:00Z");
    pr.merged_at = Some(utc("2026-11-20T15:30:00Z"));
    pr.merged_by = Some(user("alice"));
    let events = vec![
        // Tokyo 11-20 23:30
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            "2026-11-20T14:30:00Z",
        ),
        // Tokyo 11-20 23:40 — alice starts a review thread
        inline_reply(
            "R_tool",
            "P1",
            "c-a1",
            "T1",
            None,
            user("alice"),
            "2026-11-20T14:40:00Z",
        ),
        // Tokyo 11-21 00:05, 00:10, 00:20 — the author replies three times
        inline_reply(
            "R_tool",
            "P1",
            "c-m1",
            "T1",
            Some("c-a1"),
            me(),
            "2026-11-20T15:05:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "c-m2",
            "T1",
            Some("c-a1"),
            me(),
            "2026-11-20T15:10:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "c-m3",
            "T1",
            Some("c-a1"),
            me(),
            "2026-11-20T15:20:00Z",
        ),
        // Tokyo 11-21 00:30 — alice merges
        lifecycle(
            EventId::pr_merged("P1"),
            EventType::PrMerged,
            "R_tool",
            "P1",
            user("alice"),
            "2026-11-20T15:30:00Z",
        ),
    ];
    dataset(
        vec![tool()],
        vec![pr],
        vec![thread("T1", "P1", Some(user("alice")))],
        events,
    )
}

#[test]
fn created_and_merged_on_different_tokyo_days() {
    let ds = created_then_merged_next_tokyo_day();
    let opts = local_days("2026-11-20", "2026-11-22", zone("Asia/Tokyo"));
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    assert_eq!(days.len(), 2);

    assert_eq!(days[0].bucket.key, date("2026-11-20"));
    assert_eq!(days[0].summary.prs_created, 1);
    assert_eq!(days[0].summary.prs_merged, 0);
    assert_eq!(days[0].summary.review_responses, 0);
    // alice's thread opener arrives on the user's PR on 11-20
    assert_eq!(days[0].summary.comments_received.inline, 1);

    assert_eq!(days[1].bucket.key, date("2026-11-21"));
    assert_eq!(days[1].summary.prs_created, 0);
    assert_eq!(days[1].summary.prs_merged, 1);
    assert_eq!(days[1].summary.review_responses, 3);
    assert_eq!(days[1].summary.comments_written.inline, 3);

    let s = summary(&ds, &opts);
    assert_eq!(s.prs_created, 1);
    assert_eq!(s.prs_merged, 1);
    assert_eq!(s.merges_performed, 0); // alice merged
    assert_eq!(s.merges_unknown_actor, 0);
    assert_eq!(s.active_prs, 1);
    assert_eq!(s.active_prs_all_participants, 1);
    // user's PR events: opened + 3 replies = 4
    assert_eq!(s.pr_internal_events, 4);
    // Tokyo: active on 11-20 (opened) and 11-21 (replies)
    assert_eq!(s.active_days, 2);
}

#[test]
fn created_and_merged_fall_on_one_utc_day() {
    let ds = created_then_merged_next_tokyo_day();
    let opts = local_days("2026-11-20", "2026-11-22", zone("UTC"));
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    // All events are between 14:30Z and 15:30Z on 11-20.
    assert_eq!(days[0].summary.prs_created, 1);
    assert_eq!(days[0].summary.prs_merged, 1);
    assert_eq!(days[0].summary.review_responses, 3);
    assert_eq!(days[1].summary, Summary::default());
    assert_eq!(summary(&ds, &opts).active_days, 1);
}

#[test]
fn merge_in_a_later_period_is_not_counted_at_creation() {
    let ds = created_then_merged_next_tokyo_day();
    // Only the Tokyo creation day.
    let opts = local_days("2026-11-20", "2026-11-21", zone("Asia/Tokyo"));
    let s = summary(&ds, &opts);
    assert_eq!(s.prs_created, 1);
    // The PR's current state is merged, but the merge happened later.
    assert_eq!(s.prs_merged, 0);
}

#[test]
fn merges_by_author_by_user_and_by_unknown_are_kept_apart() {
    let mut prs = Vec::new();
    let mut events = Vec::new();
    // (pr, author, merger)
    let cases: [(&str, Actor, Actor); 5] = [
        ("PA", me(), user("alice")),    // user's PR merged by someone else
        ("PB", me(), me()),             // user's PR merged by the user
        ("PC", user("alice"), me()),    // someone else's PR merged by the user
        ("PD", me(), Actor::unknown()), // user's PR, merger unknown
        ("PE", user("alice"), user("bob")),
    ];
    for (i, (id, author, merger)) in cases.into_iter().enumerate() {
        prs.push(pull(id, "R_tool", i as u64 + 1, author));
        events.push(lifecycle(
            EventId::pr_merged(id),
            EventType::PrMerged,
            "R_tool",
            id,
            merger,
            "2026-11-10T12:00:00Z",
        ));
    }
    let ds = dataset(vec![tool()], prs, vec![], events);
    let s = summary(&ds, &november_utc());
    // user's PRs merged: PA, PB, PD
    assert_eq!(s.prs_merged, 3);
    // merges the user performed: PB, PC
    assert_eq!(s.merges_performed, 2);
    // user's PRs with unknown merger: PD
    assert_eq!(s.merges_unknown_actor, 1);
}

#[test]
fn closed_without_merge_is_distinct_from_close_by_merge() {
    let prs = vec![
        pull("PF", "R_tool", 1, me()),
        pull("PG", "R_tool", 2, me()),
        pull("PH", "R_tool", 3, user("alice")),
    ];
    let closed = |id: &str, pr: &str, via_merge: bool, actor: Actor| {
        with_meta(
            lifecycle(
                EventId::timeline(id),
                EventType::PrClosed,
                "R_tool",
                pr,
                actor,
                "2026-11-10T12:00:00Z",
            ),
            |m| m.via_merge = Some(via_merge),
        )
    };
    let events = vec![
        // PF: closed without merge, then reopened
        closed("close-f", "PF", false, me()),
        lifecycle(
            EventId::timeline("reopen-f"),
            EventType::PrReopened,
            "R_tool",
            "PF",
            me(),
            "2026-11-11T12:00:00Z",
        ),
        // PG: merged (closed as part of the merge)
        lifecycle(
            EventId::pr_merged("PG"),
            EventType::PrMerged,
            "R_tool",
            "PG",
            user("alice"),
            "2026-11-10T12:00:00Z",
        ),
        closed("close-g", "PG", true, user("alice")),
        // PH: alice's PR closed by the user — not the user's PR
        closed("close-h", "PH", false, me()),
    ];
    let ds = dataset(vec![tool()], prs, vec![], events);
    let s = summary(&ds, &november_utc());
    assert_eq!(s.prs_closed_without_merge, 1); // PF
    assert_eq!(s.prs_merged, 1); // PG
    assert_eq!(s.prs_reopened, 1); // PF
}

#[test]
fn squash_merge_counts_the_squashed_commit_and_keeps_branch_commits_as_pr_commits() {
    let mut pr = pull("P7", "R_tool", 7, me());
    pr.state = PrState::Merged;
    let events = vec![
        lifecycle(
            EventId::pr_created("P7"),
            EventType::PrOpened,
            "R_tool",
            "P7",
            me(),
            "2026-11-10T09:00:00Z",
        ),
        // branch commits, also reported as the user's commits
        commit("R_tool", "c1", me(), "2026-11-10T09:10:00Z"),
        commit("R_tool", "c2", me(), "2026-11-10T09:20:00Z"),
        commit("R_tool", "c3", me(), "2026-11-10T09:30:00Z"),
        pr_commit("R_tool", "P7", "c1", me(), "2026-11-10T09:10:00Z"),
        pr_commit("R_tool", "P7", "c2", me(), "2026-11-10T09:20:00Z"),
        pr_commit("R_tool", "P7", "c3", me(), "2026-11-10T09:30:00Z"),
        lifecycle(
            EventId::pr_merged("P7"),
            EventType::PrMerged,
            "R_tool",
            "P7",
            user("alice"),
            "2026-11-11T10:00:00Z",
        ),
        // squash commit on the default branch: authored by the user,
        // committed by the web flow, a single-parent (non-merge) commit
        with_meta(commit("R_tool", "sq1", me(), "2026-11-11T10:00:00Z"), |m| {
            m.is_merge_commit = Some(false);
            m.committer = Some(user("web-flow"));
        }),
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let s = summary(&ds, &november_utc());
    assert_eq!(s.commits, 4); // c1, c2, c3, sq1
    assert_eq!(s.merge_commits, 0);
    assert_eq!(s.pr_commits, 3); // c1..c3 in P7
    assert_eq!(s.prs_created, 1);
    assert_eq!(s.prs_merged, 1);
    assert_eq!(s.merges_performed, 0);
}

// ---------------------------------------------------------------------------
// Reviews
// ---------------------------------------------------------------------------

fn reviews_dataset() -> Dataset {
    let prs = vec![
        pull("P2", "R_tool", 2, me()),
        pull("P3", "R_tool", 3, user("alice")),
    ];
    let events = vec![
        // received on the user's PR P2
        review(
            "R_tool",
            "P2",
            "r1",
            user("alice"),
            ReviewState::Approved,
            "2026-11-10T10:00:00Z",
        ),
        review(
            "R_tool",
            "P2",
            "r2",
            user("alice"),
            ReviewState::ChangesRequested,
            "2026-11-10T11:00:00Z",
        ),
        review(
            "R_tool",
            "P2",
            "r3",
            user("alice"),
            ReviewState::Commented,
            "2026-11-10T12:00:00Z",
        ),
        review(
            "R_tool",
            "P2",
            "r4",
            user("alice"),
            ReviewState::Pending,
            "2026-11-10T13:00:00Z",
        ),
        review(
            "R_tool",
            "P2",
            "r5",
            user("bob"),
            ReviewState::Dismissed,
            "2026-11-10T14:00:00Z",
        ),
        review(
            "R_tool",
            "P2",
            "r6",
            bot("review-helper[bot]"),
            ReviewState::Commented,
            "2026-11-10T15:00:00Z",
        ),
        // bob's dismissal of a review on P2 is a separate action
        lifecycle(
            EventId::timeline("dismiss-1"),
            EventType::ReviewDismissed,
            "R_tool",
            "P2",
            user("bob"),
            "2026-11-10T16:00:00Z",
        ),
        // submitted by the user on alice's PR P3
        review(
            "R_tool",
            "P3",
            "s1",
            me(),
            ReviewState::Approved,
            "2026-11-11T10:00:00Z",
        ),
        review(
            "R_tool",
            "P3",
            "s2",
            me(),
            ReviewState::ChangesRequested,
            "2026-11-11T11:00:00Z",
        ),
        review(
            "R_tool",
            "P3",
            "s3",
            me(),
            ReviewState::Commented,
            "2026-11-11T12:00:00Z",
        ),
        review(
            "R_tool",
            "P3",
            "s4",
            me(),
            ReviewState::Pending,
            "2026-11-11T13:00:00Z",
        ),
        // the user dismisses alice's own self-review on P3
        lifecycle(
            EventId::timeline("dismiss-2"),
            EventType::ReviewDismissed,
            "R_tool",
            "P3",
            me(),
            "2026-11-11T14:00:00Z",
        ),
    ];
    dataset(vec![tool()], prs, vec![], events)
}

#[test]
fn reviews_submitted_and_received_are_separate_and_pending_is_ignored() {
    let ds = reviews_dataset();
    let opts = november_utc();
    let s = summary(&ds, &opts);

    // submitted: s1..s3 (s4 pending) = 3
    assert_eq!(s.reviews_submitted.total, 3);
    assert_eq!(s.reviews_submitted.approved, 1);
    assert_eq!(s.reviews_submitted.changes_requested, 1);
    assert_eq!(s.reviews_submitted.commented, 1);
    assert_eq!(s.reviews_submitted.dismissed_unknown_state, 0);
    // three reviews on one PR -> one reviewed PR
    assert_eq!(s.reviewed_prs, 1);

    // received: r1,r2,r3,r5,r6 (r4 pending) = 5
    assert_eq!(s.reviews_received.total, 5);
    assert_eq!(s.reviews_received.approved, 1);
    assert_eq!(s.reviews_received.changes_requested, 1);
    assert_eq!(s.reviews_received.commented, 2); // r3 + bot r6
    assert_eq!(s.reviews_received.dismissed_unknown_state, 1); // r5
    assert_eq!(s.reviews_received_from_bots, 1);

    // only the user's own dismissal is the user's action
    assert_eq!(s.review_dismissals_performed, 1);

    assert_eq!(review_rows(&ds, &opts, ReviewDirection::Submitted).len(), 3);
    assert_eq!(review_rows(&ds, &opts, ReviewDirection::Received).len(), 5);

    let detail = pr_detail(&ds, "P2", &[]).unwrap();
    assert_eq!(detail.reviews.total, 5);
    let row = pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity)
        .into_iter()
        .find(|r| r.pr_id == "P2")
        .unwrap();
    assert_eq!(row.reviews, 5);
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

#[test]
fn conversation_inline_and_review_summary_written_vs_received() {
    let prs = vec![
        pull("P4", "R_tool", 4, me()),
        pull("P5", "R_tool", 5, user("alice")),
    ];
    let alice_conv = comment(
        EventType::ConversationComment,
        "R_tool",
        "P4",
        "ac1",
        user("alice"),
        "2026-11-12T09:00:00Z",
    );
    let mut alice_conv_rest = alice_conv.clone();
    alice_conv_rest.source = Source::Rest;
    alice_conv_rest.observed_at = utc("2027-06-02T00:00:00Z");
    let events = vec![
        // received on P4 (user's PR)
        alice_conv,
        alice_conv_rest, // same comment fetched from another API
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P4",
            "ac2",
            user("alice"),
            "2026-11-12T09:05:00Z",
        ),
        inline_reply(
            "R_tool",
            "P4",
            "ai1",
            "TA",
            None,
            user("alice"),
            "2026-11-12T09:10:00Z",
        ),
        review(
            "R_tool",
            "P4",
            "ar1",
            user("alice"),
            ReviewState::Commented,
            "2026-11-12T09:15:00Z",
        ),
        comment(
            EventType::ReviewSummary,
            "R_tool",
            "P4",
            "ar1",
            user("alice"),
            "2026-11-12T09:15:00Z",
        ),
        // written by the user on P4
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P4",
            "mc1",
            me(),
            "2026-11-12T10:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P4",
            "mi1",
            "TA",
            Some("ai1"),
            me(),
            "2026-11-12T10:05:00Z",
        ),
        inline_reply(
            "R_tool",
            "P4",
            "mi2",
            "TM",
            None,
            me(),
            "2026-11-12T10:10:00Z",
        ),
        // written by the user on alice's P5: a review with a summary body
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P5",
            "mc2",
            me(),
            "2026-11-12T11:00:00Z",
        ),
        review(
            "R_tool",
            "P5",
            "mr1",
            me(),
            ReviewState::Commented,
            "2026-11-12T11:05:00Z",
        ),
        comment(
            EventType::ReviewSummary,
            "R_tool",
            "P5",
            "mr1",
            me(),
            "2026-11-12T11:05:00Z",
        ),
        // alice on her own PR: neither written nor received by the user
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P5",
            "ac3",
            user("alice"),
            "2026-11-12T12:00:00Z",
        ),
    ];
    let threads = vec![
        thread("TA", "P4", Some(user("alice"))),
        thread("TM", "P4", Some(me())),
    ];
    let ds = dataset(vec![tool()], prs, threads, events);
    let opts = november_utc();
    let s = summary(&ds, &opts);

    // written: conversation mc1, mc2 = 2; inline mi1, mi2 = 2; summary mr1 = 1
    assert_eq!(s.comments_written.conversation, 2);
    assert_eq!(s.comments_written.inline, 2);
    assert_eq!(s.comments_written.review_summary, 1);
    assert_eq!(s.comments_written.total(), 5);
    // received on P4: conversation ac1 (once), ac2 = 2; inline ai1 = 1; summary ar1 = 1
    assert_eq!(s.comments_received.conversation, 2);
    assert_eq!(s.comments_received.inline, 1);
    assert_eq!(s.comments_received.review_summary, 1);
    assert_eq!(s.comments_received.total(), 4);
    // the review with a summary body is one review, not two
    assert_eq!(s.reviews_submitted.total, 1);
    assert_eq!(s.reviews_received.total, 1);
    // only mi1 replies in a thread someone else started on the user's PR
    assert_eq!(s.review_responses, 1);

    let rows = metrics::comment_rows(&ds, &opts);
    // 5 written + 4 received
    assert_eq!(rows.len(), 9);
    assert_eq!(rows.iter().filter(|r| r.written_by_subject).count(), 5);
}

#[test]
fn review_responses_only_count_replies_on_own_prs_in_threads_started_by_others() {
    let prs = vec![
        pull("P1", "R_tool", 1, me()),
        pull("P2", "R_tool", 2, user("alice")),
    ];
    let threads = vec![
        thread("Ta", "P1", Some(user("alice"))), // started by alice on user's PR
        thread("Tb", "P1", Some(me())),          // started by the user
        thread("Tc", "P2", Some(user("bob"))),   // on alice's PR
    ];
    let events = vec![
        inline_reply(
            "R_tool",
            "P1",
            "a1",
            "Ta",
            None,
            user("alice"),
            "2026-11-05T10:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "m1",
            "Ta",
            Some("a1"),
            me(),
            "2026-11-05T10:01:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "m2",
            "Ta",
            Some("a1"),
            me(),
            "2026-11-05T10:02:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "m3",
            "Tb",
            None,
            me(),
            "2026-11-05T10:03:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "a2",
            "Tb",
            Some("m3"),
            user("alice"),
            "2026-11-05T10:04:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "m4",
            "Tb",
            Some("a2"),
            me(),
            "2026-11-05T10:05:00Z",
        ),
        inline_reply(
            "R_tool",
            "P2",
            "b1",
            "Tc",
            None,
            user("bob"),
            "2026-11-05T10:06:00Z",
        ),
        inline_reply(
            "R_tool",
            "P2",
            "m5",
            "Tc",
            Some("b1"),
            me(),
            "2026-11-05T10:07:00Z",
        ),
        // a conversation comment is never a review response
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P1",
            "m6",
            me(),
            "2026-11-05T10:08:00Z",
        ),
        // a later commit is not treated as a response either
        pr_commit("R_tool", "P1", "fix1", me(), "2026-11-05T10:09:00Z"),
    ];
    let ds = dataset(vec![tool()], prs, threads, events);
    let s = summary(&ds, &november_utc());
    // m1, m2 only
    assert_eq!(s.review_responses, 2);
    // all inline comments by the user: m1..m5 = 5
    assert_eq!(s.comments_written.inline, 5);
}

// ---------------------------------------------------------------------------
// Review threads (current state)
// ---------------------------------------------------------------------------

#[test]
fn resolved_and_outdated_threads_are_reported_from_current_state() {
    let prs = vec![pull("P4", "R_tool", 4, me()), pull("P9", "R_tool", 9, me())];
    let mk = |id: &str, pr: &str, resolved: bool, outdated: bool| {
        let mut t = thread(id, pr, Some(user("alice")));
        t.is_resolved = resolved;
        t.is_outdated = outdated;
        if resolved {
            t.resolved_by = Some(me());
        }
        t
    };
    let threads = vec![
        mk("T1", "P4", true, true),
        mk("T2", "P4", false, false),
        mk("T3", "P4", true, false),
        mk("T4", "P4", false, true),
        // P9 has no activity in the period
        mk("T5", "P9", false, false),
    ];
    let events = vec![comment(
        EventType::ConversationComment,
        "R_tool",
        "P4",
        "c1",
        user("alice"),
        "2026-11-15T10:00:00Z",
    )];
    let ds = dataset(vec![tool()], prs, threads, events);
    let snap = snapshot(&ds, &november_utc());
    // P4 only: total 4, open T2+T4 = 2, resolved T1+T3 = 2, outdated T1+T4 = 2
    assert_eq!(snap.threads.total, 4);
    assert_eq!(snap.threads.open, 2);
    assert_eq!(snap.threads.resolved, 2);
    assert_eq!(snap.threads.outdated, 2);
    // both PRs are open and authored by the user
    assert_eq!(snap.open_prs, 2);

    let detail = pr_detail(&ds, "P4", &[]).unwrap();
    assert_eq!(detail.threads, snap.threads);
    // resolving a thread is current state, not an activity of the period
    let s = summary(&ds, &november_utc());
    assert_eq!(s.pr_internal_events, 0);
}

// ---------------------------------------------------------------------------
// Force push
// ---------------------------------------------------------------------------

#[test]
fn force_pushed_away_commit_is_excluded_from_counts_but_shown_as_removed() {
    let pr = pull("P6", "R_tool", 6, me());
    let removed = with_meta(
        pr_commit("R_tool", "P6", "c2", me(), "2026-11-03T10:00:00Z"),
        |m| m.removed_observed_at = Some(utc("2026-11-04T12:00:00Z")),
    );
    let events = vec![
        lifecycle(
            EventId::pr_created("P6"),
            EventType::PrOpened,
            "R_tool",
            "P6",
            me(),
            "2026-11-03T09:00:00Z",
        ),
        pr_commit("R_tool", "P6", "c1", me(), "2026-11-03T09:30:00Z"),
        removed,
        lifecycle(
            EventId::timeline("fp1"),
            EventType::PrHeadForcePushed,
            "R_tool",
            "P6",
            me(),
            "2026-11-04T11:00:00Z",
        ),
        pr_commit("R_tool", "P6", "c3", me(), "2026-11-04T10:59:00Z"),
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let opts = november_utc();
    let s = summary(&ds, &opts);
    assert_eq!(s.pr_commits, 2); // c1, c3
    assert_eq!(s.force_pushes, 1);
    // opened + c1 + c3 + force push = 4 (removed c2 excluded)
    assert_eq!(s.pr_internal_events, 4);

    let detail = pr_detail(&ds, "P6", &[]).unwrap();
    assert_eq!(detail.commits, 2);
    assert_eq!(detail.removed_commits, 1);
    let shown_removed: Vec<&ActivityEvent> =
        detail.timeline.iter().filter(|e| e.is_removed()).collect();
    assert_eq!(shown_removed.len(), 1);
    assert_eq!(shown_removed[0].meta.sha.as_deref(), Some("c2"));

    let row = &pr_rows(&ds, &opts, Participation::Subject, None, PrSort::Activity)[0];
    assert_eq!(row.commits, 2);
    assert_eq!(row.events, 4);
}

// ---------------------------------------------------------------------------
// Time zones and DST
// ---------------------------------------------------------------------------

#[test]
fn tokyo_and_utc_put_a_boundary_commit_on_different_days() {
    // 2026-11-20T15:00Z = 2026-11-21 00:00 in Tokyo
    let ds = dataset(
        vec![tool()],
        vec![],
        vec![],
        vec![
            commit("R_tool", "a", me(), "2026-11-20T14:59:59Z"),
            commit("R_tool", "b", me(), "2026-11-20T15:00:00Z"),
        ],
    );
    let tokyo = local_days("2026-11-20", "2026-11-22", zone("Asia/Tokyo"));
    let per: Vec<u64> = metrics::breakdown(&ds, &tokyo, Granularity::Day)
        .iter()
        .map(|b| b.summary.commits)
        .collect();
    assert_eq!(per, vec![1, 1]); // a on 11-20, b on 11-21

    let utc_days = local_days("2026-11-20", "2026-11-22", zone("UTC"));
    let per: Vec<u64> = metrics::breakdown(&ds, &utc_days, Granularity::Day)
        .iter()
        .map(|b| b.summary.commits)
        .collect();
    assert_eq!(per, vec![2, 0]); // both on 11-20 UTC
}

#[test]
fn spring_forward_day_has_23_hours_and_keeps_its_events() {
    let ny = zone("America/New_York");
    let ds = dataset(
        vec![tool()],
        vec![],
        vec![],
        vec![
            commit("R_tool", "a", me(), "2026-03-08T04:59:59Z"), // 03-07 23:59:59 EST
            commit("R_tool", "b", me(), "2026-03-08T05:00:00Z"), // 03-08 00:00 EST
            commit("R_tool", "c", me(), "2026-03-09T03:59:59Z"), // 03-08 23:59:59 EDT
            commit("R_tool", "d", me(), "2026-03-09T04:00:00Z"), // 03-09 00:00 EDT
        ],
    );
    let opts = local_days("2026-03-07", "2026-03-10", ny);
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    let hours: Vec<i64> = days
        .iter()
        .map(|b| (b.bucket.end - b.bucket.start).num_hours())
        .collect();
    assert_eq!(hours, vec![24, 23, 24]);
    let per: Vec<u64> = days.iter().map(|b| b.summary.commits).collect();
    assert_eq!(per, vec![1, 2, 1]);
}

#[test]
fn fall_back_day_has_25_hours_and_keeps_its_events() {
    let ny = zone("America/New_York");
    let ds = dataset(
        vec![tool()],
        vec![],
        vec![],
        vec![
            commit("R_tool", "a", me(), "2026-11-01T04:00:00Z"), // 11-01 00:00 EDT
            commit("R_tool", "b", me(), "2026-11-01T05:30:00Z"), // 01:30 EDT (first)
            commit("R_tool", "c", me(), "2026-11-01T06:30:00Z"), // 01:30 EST (second)
            commit("R_tool", "d", me(), "2026-11-02T04:59:59Z"), // 11-01 23:59:59 EST
            commit("R_tool", "e", me(), "2026-11-02T05:00:00Z"), // 11-02 00:00 EST
        ],
    );
    let opts = local_days("2026-11-01", "2026-11-03", ny);
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    let hours: Vec<i64> = days
        .iter()
        .map(|b| (b.bucket.end - b.bucket.start).num_hours())
        .collect();
    assert_eq!(hours, vec![25, 24]);
    let per: Vec<u64> = days.iter().map(|b| b.summary.commits).collect();
    assert_eq!(per, vec![4, 1]);
}

// ---------------------------------------------------------------------------
// Bots, unknown actors and automation
// ---------------------------------------------------------------------------

#[test]
fn bot_commit_is_not_the_users_commit() {
    let renovate = Actor::new(
        Some("B_renovate".into()),
        Some("renovate[bot]".into()),
        Some("User"),
    );
    assert!(renovate.is_bot()); // the [bot] suffix marks a bot
    let ds = dataset(
        vec![tool()],
        vec![],
        vec![],
        vec![
            commit(
                "R_tool",
                "a",
                bot("dependabot[bot]"),
                "2026-11-05T10:00:00Z",
            ),
            commit("R_tool", "b", renovate, "2026-11-05T11:00:00Z"),
            commit("R_tool", "c", me(), "2026-11-06T11:00:00Z"),
        ],
    );
    let s = summary(&ds, &november_utc());
    assert_eq!(s.commits, 1); // only c
    assert_eq!(s.active_days, 1);
}

#[test]
fn unknown_actor_is_never_the_user() {
    let subject_without_id = ghusage_core::model::Subject {
        id: None,
        login: "me".into(),
    };
    assert!(!Actor::unknown().is(&subject()));
    assert!(!Actor::unknown().is(&subject_without_id));

    let prs = vec![
        pull("P1", "R_tool", 1, me()),
        pull("P2", "R_tool", 2, Actor::unknown()),
    ];
    let events = vec![
        commit("R_tool", "a", Actor::unknown(), "2026-11-05T10:00:00Z"),
        lifecycle(
            EventId::pr_created("P2"),
            EventType::PrOpened,
            "R_tool",
            "P2",
            Actor::unknown(),
            "2026-11-05T10:00:00Z",
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P1",
            "u1",
            Actor::unknown(),
            "2026-11-05T11:00:00Z",
        ),
        review(
            "R_tool",
            "P1",
            "ur1",
            Actor::unknown(),
            ReviewState::Approved,
            "2026-11-05T12:00:00Z",
        ),
        lifecycle(
            EventId::timeline("label-u"),
            EventType::Labeled,
            "R_tool",
            "P1",
            Actor::unknown(),
            "2026-11-05T12:30:00Z",
        ),
        lifecycle(
            EventId::pr_merged("P1"),
            EventType::PrMerged,
            "R_tool",
            "P1",
            Actor::unknown(),
            "2026-11-05T13:00:00Z",
        ),
    ];
    let ds = dataset(vec![tool()], prs, vec![], events);
    let s = summary(&ds, &november_utc());
    assert_eq!(s.commits, 0);
    assert_eq!(s.prs_created, 0);
    assert_eq!(s.comments_written.total(), 0);
    assert_eq!(s.reviews_submitted.total, 0);
    assert_eq!(s.metadata_actions, 0);
    assert_eq!(s.merges_performed, 0);
    assert_eq!(s.pr_internal_events, 0);
    assert_eq!(s.active_days, 0);
    assert_eq!(s.active_prs, 0);
    // on the user's P1: received 1 comment, 1 review, merged by unknown
    assert_eq!(s.comments_received.conversation, 1);
    assert_eq!(s.reviews_received.total, 1);
    assert_eq!(s.prs_merged, 1);
    assert_eq!(s.merges_unknown_actor, 1);
    // P1 and P2 both had activity
    assert_eq!(s.active_prs_all_participants, 2);
    let detail = pr_detail(&ds, "P1", &[]).unwrap();
    assert!(!detail.participants.iter().any(|p| p == "me"));
}

#[test]
fn ci_results_are_never_human_activity() {
    let pr = pull("P1", "R_tool", 1, me());
    let check = |id: &str, kind: EventType, actor: Actor, outcome, at: &str| {
        let eid = if kind == EventType::CheckRun {
            EventId::check_run(id)
        } else {
            EventId::commit_status(id)
        };
        with_meta(
            event(eid, kind, actor, "R_tool", Some("P1"), utc(at)),
            |m| m.check_outcome = Some(outcome),
        )
    };
    use ghusage_core::model::CheckOutcome::*;
    let events = vec![
        check(
            "k1",
            EventType::CheckRun,
            bot("github-actions[bot]"),
            Success,
            "2026-11-05T10:00:00Z",
        ),
        check(
            "k2",
            EventType::CheckRun,
            bot("github-actions[bot]"),
            Failure,
            "2026-11-05T10:01:00Z",
        ),
        check(
            "k3",
            EventType::CheckRun,
            me(),
            Pending,
            "2026-11-05T10:02:00Z",
        ),
        check(
            "s1",
            EventType::CommitStatus,
            me(),
            Success,
            "2026-11-06T10:00:00Z",
        ),
        check(
            "s2",
            EventType::CommitStatus,
            me(),
            Cancelled,
            "2026-11-06T10:01:00Z",
        ),
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let opts = november_utc();
    let s = summary(&ds, &opts);
    assert_eq!(s.ci.passed, 2); // k1, s1
    assert_eq!(s.ci.failed, 1); // k2
    assert_eq!(s.ci.pending, 1); // k3
    assert_eq!(s.ci.other, 1); // s2 cancelled
    assert_eq!(s.pr_internal_events, 0);
    assert_eq!(s.active_days, 0);
    assert_eq!(s.active_prs, 0);
    assert_eq!(s.active_prs_all_participants, 0);
    assert_eq!(s.last_activity, None);
    assert!(pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity).is_empty());
}

// ---------------------------------------------------------------------------
// Identity across sources and timeline ordering
// ---------------------------------------------------------------------------

#[test]
fn same_event_from_several_sources_counts_once() {
    let mut pr = pull("P1", "R_tool", 1, me());
    pr.state = PrState::Merged;
    let merged = lifecycle(
        EventId::pr_merged("P1"),
        EventType::PrMerged,
        "R_tool",
        "P1",
        user("alice"),
        "2026-11-05T10:00:00Z",
    );
    let mut merged_rest = merged.clone();
    merged_rest.source = Source::Rest;
    merged_rest.observed_at = utc("2027-06-03T00:00:00Z");
    let labeled = with_meta(
        lifecycle(
            EventId::timeline("LE_1"),
            EventType::Labeled,
            "R_tool",
            "P1",
            me(),
            "2026-11-05T09:00:00Z",
        ),
        |m| m.target = Some("bug".into()),
    );
    let mut labeled_search = labeled.clone();
    labeled_search.source = Source::Search;
    let events = vec![
        merged,
        merged_rest,
        labeled.clone(),
        labeled_search,
        labeled,
    ];
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let opts = november_utc();
    let s = summary(&ds, &opts);
    assert_eq!(s.prs_merged, 1);
    assert_eq!(s.metadata_actions, 1);
    let tl = metrics::timeline(&ds, "P1", &[]);
    assert_eq!(tl.len(), 2); // label, merge
    let row = &pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity)[0];
    assert_eq!(row.events, 2);
}

fn same_time_events() -> Vec<ActivityEvent> {
    let t = "2026-11-05T10:00:00Z";
    vec![
        lifecycle(
            EventId::pr_merged("P1"),
            EventType::PrMerged,
            "R_tool",
            "P1",
            user("alice"),
            t,
        ),
        review(
            "R_tool",
            "P1",
            "rv",
            user("alice"),
            ReviewState::Approved,
            t,
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P1",
            "cm",
            me(),
            t,
        ),
        pr_commit("R_tool", "P1", "aaa", me(), t),
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            t,
        ),
        lifecycle(
            EventId::timeline("lab"),
            EventType::Labeled,
            "R_tool",
            "P1",
            me(),
            t,
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P1",
            "cm2",
            user("bob"),
            "2026-11-05T09:00:00Z",
        ),
    ]
}

#[test]
fn timeline_order_is_deterministic_for_same_timestamp_events() {
    let base = same_time_events();
    let reference: Vec<String> = {
        let ds = dataset(
            vec![tool()],
            vec![pull("P1", "R_tool", 1, me())],
            vec![],
            base.clone(),
        );
        metrics::timeline(&ds, "P1", &[])
            .iter()
            .map(|e| e.event_id.0.clone())
            .collect()
    };
    // every rotation and the reversal of the input give the same order
    let n = base.len();
    for k in 0..n {
        for reverse in [false, true] {
            let mut v = base.clone();
            v.rotate_left(k);
            if reverse {
                v.reverse();
            }
            let ds = dataset(vec![tool()], vec![pull("P1", "R_tool", 1, me())], vec![], v);
            let got: Vec<String> = metrics::timeline(&ds, "P1", &[])
                .iter()
                .map(|e| e.event_id.0.clone())
                .collect();
            assert_eq!(got, reference);
        }
    }
    // earlier event first; at one instant a PR is opened before it is merged
    assert_eq!(reference[0], "comment:cm2");
    let pos = |id: &str| reference.iter().position(|x| x == id).unwrap();
    assert_eq!(pos("pr:P1:created"), 1);
    assert!(pos("pr:P1:created") < pos("review:rv"));
    assert!(pos("review:rv") < pos("pr:P1:merged"));
}

#[test]
fn pr_detail_timeline_filter_keeps_counts_intact() {
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![],
        same_time_events(),
    );
    let all = pr_detail(&ds, "P1", &[]).unwrap();
    assert_eq!(all.timeline.len(), 7);
    let only = pr_detail(
        &ds,
        "P1",
        &[EventType::ConversationComment, EventType::ReviewSubmitted],
    )
    .unwrap();
    // two conversation comments + one review
    assert_eq!(only.timeline.len(), 3);
    assert!(only.timeline.iter().all(|e| matches!(
        e.event_type,
        EventType::ConversationComment | EventType::ReviewSubmitted
    )));
    assert_eq!(only.reviews, all.reviews);
    assert_eq!(only.comments, all.comments);
    assert_eq!(only.commits, all.commits);
    assert_eq!(only.participants, all.participants);
    assert_eq!(all.comments.conversation, 2);
    assert_eq!(all.reviews.total, 1);
    assert_eq!(all.commits, 1);
    assert_eq!(
        metrics::timeline(&ds, "P1", &[EventType::PrMerged]).len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Repository filters and ownership
// ---------------------------------------------------------------------------

fn filter_dataset(subject_is_viewer: bool) -> Dataset {
    let mut app = repo("R_app", "acme", "app", Visibility::Private);
    app.viewer_permission = Some("WRITE".into()); // org member, not admin
    let mut infra = repo("R_infra", "acme", "infra", Visibility::Private);
    infra.viewer_permission = Some("ADMIN".into());
    let mut handbook = repo("R_hb", "acme", "handbook", Visibility::Internal);
    handbook.viewer_permission = Some("READ".into());
    let mut lib = repo("R_lib", "other", "lib", Visibility::Public);
    lib.viewer_permission = Some("READ".into());
    let repos = vec![tool(), app, infra, handbook, lib];
    // commits: tool 1, app 2, infra 4, handbook 16, lib 8
    let mut events = Vec::new();
    for (r, n) in [
        ("R_tool", 1),
        ("R_app", 2),
        ("R_infra", 4),
        ("R_hb", 16),
        ("R_lib", 8),
    ] {
        for i in 0..n {
            events.push(commit(r, &format!("{r}-{i}"), me(), "2026-11-05T10:00:00Z"));
        }
    }
    Dataset::new(
        subject(),
        subject_is_viewer,
        repos,
        vec![],
        Vec::<Issue>::new(),
        vec![],
        events,
        vec![],
    )
}

fn commits_with(ds: &Dataset, filter: Filter) -> u64 {
    let mut opts = november_utc();
    opts.filter = filter;
    summary(ds, &opts).commits
}

#[test]
fn repository_owner_and_visibility_filters() {
    let ds = filter_dataset(true);
    assert_eq!(commits_with(&ds, Filter::default()), 31); // 1+2+4+16+8
    let by_name = Filter {
        repositories: vec!["ACME/App".into()],
        ..Filter::default()
    };
    assert_eq!(commits_with(&ds, by_name), 2);
    let by_id = Filter {
        repositories: vec!["R_lib".into()],
        ..Filter::default()
    };
    assert_eq!(commits_with(&ds, by_id), 8);
    let owners = Filter {
        owners: vec!["acme".into()],
        ..Filter::default()
    };
    assert_eq!(commits_with(&ds, owners), 22); // 2+4+16
    let public = Filter {
        visibility: Some(VisibilityFilter::Public),
        ..Filter::default()
    };
    assert_eq!(commits_with(&ds, public), 9); // tool 1 + lib 8
    let private = Filter {
        visibility: Some(VisibilityFilter::Private),
        ..Filter::default()
    };
    assert_eq!(commits_with(&ds, private), 22); // app 2 + infra 4 + internal handbook 16
}

#[test]
fn ownership_is_not_inferred_from_organization_membership() {
    let owned = Filter {
        ownership: Some(Ownership::Owned),
        ..Filter::default()
    };
    let external = Filter {
        ownership: Some(Ownership::External),
        ..Filter::default()
    };
    // viewer: owned = own account (tool 1) + administered infra 4 = 5
    let ds = filter_dataset(true);
    assert_eq!(commits_with(&ds, owned.clone()), 5);
    assert_eq!(commits_with(&ds, external.clone()), 26); // 2 + 16 + 8
    let rows = by_repository(&ds, &november_utc());
    let owned_names: Vec<&str> = rows
        .iter()
        .filter(|r| r.owned)
        .map(|r| r.full_name.as_str())
        .collect();
    assert_eq!(owned_names, vec!["acme/infra", "me/tool"]);

    // not the viewer: the viewer's permission says nothing about the subject
    let ds = filter_dataset(false);
    assert_eq!(commits_with(&ds, owned), 1);
    assert_eq!(commits_with(&ds, external), 30);
}

#[test]
fn cross_repository_contribution_from_a_fork() {
    let mut fork = repo("R_fork", "me", "lib", Visibility::Public);
    fork.is_fork = true;
    let upstream = repo("R_lib", "other", "lib", Visibility::Public);
    let pr = pull("PX", "R_lib", 42, me());
    let events = vec![
        commit("R_fork", "x1", me(), "2026-11-05T09:00:00Z"),
        lifecycle(
            EventId::pr_created("PX"),
            EventType::PrOpened,
            "R_lib",
            "PX",
            me(),
            "2026-11-05T10:00:00Z",
        ),
        pr_commit("R_lib", "PX", "x1", me(), "2026-11-05T09:00:00Z"),
    ];
    let ds = dataset(vec![fork, upstream], vec![pr], vec![], events);
    let opts = november_utc();
    let s = summary(&ds, &opts);
    assert_eq!(s.commits, 1);
    assert_eq!(s.pr_commits, 1);
    assert_eq!(s.prs_created, 1);

    let rows = by_repository(&ds, &opts);
    let lib = rows.iter().find(|r| r.repository_id == "R_lib").unwrap();
    assert!(!lib.owned);
    assert_eq!(lib.summary.prs_created, 1);
    assert_eq!(lib.summary.commits, 0);
    let fork_row = rows.iter().find(|r| r.repository_id == "R_fork").unwrap();
    assert!(fork_row.owned);
    assert_eq!(fork_row.summary.commits, 1);

    let mut ext = opts.clone();
    ext.filter.ownership = Some(Ownership::External);
    let s = summary(&ds, &ext);
    assert_eq!((s.commits, s.prs_created), (0, 1));
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

#[test]
fn coverage_with_equal_recording_times_does_not_depend_on_record_order() {
    let rec = |status| CoverageRecord {
        kind: DataKind::Commits,
        start: utc("2026-11-01T00:00:00Z"),
        end: utc("2026-12-01T00:00:00Z"),
        status,
        scope: "commits".into(),
        reason: None,
        recorded_at: utc("2026-11-21T00:00:00Z"),
    };
    let a = [rec(CoverageStatus::Partial), rec(CoverageStatus::Complete)];
    let b = [rec(CoverageStatus::Complete), rec(CoverageStatus::Partial)];
    let s = |r: &[CoverageRecord]| {
        summarize(
            r,
            DataKind::Commits,
            utc("2026-11-05T00:00:00Z"),
            utc("2026-11-10T00:00:00Z"),
        )
        .status
    };
    assert_eq!(s(&a), s(&b));
}

#[test]
fn conflicting_coverage_records_with_equal_recording_times_are_not_complete() {
    let rec = |status| CoverageRecord {
        kind: DataKind::Commits,
        start: utc("2026-11-01T00:00:00Z"),
        end: utc("2026-12-01T00:00:00Z"),
        status,
        scope: "commits".into(),
        reason: None,
        recorded_at: utc("2026-11-21T00:00:00Z"),
    };
    for records in [
        [rec(CoverageStatus::Partial), rec(CoverageStatus::Complete)],
        [rec(CoverageStatus::Complete), rec(CoverageStatus::Partial)],
    ] {
        let s = summarize(
            &records,
            DataKind::Commits,
            utc("2026-11-05T00:00:00Z"),
            utc("2026-11-10T00:00:00Z"),
        );
        assert_ne!(s.status, CoverageStatus::Complete);
    }
}

// ---------------------------------------------------------------------------
// Large dataset
// ---------------------------------------------------------------------------

#[test]
fn hundred_thousand_events_aggregate_to_expected_totals() {
    let start = utc("2026-01-01T00:00:00Z");
    let prs: Vec<_> = (0..1000)
        .map(|i| pull(&format!("P{i}"), "R_tool", i + 1, user("alice")))
        .collect();
    let mut events = Vec::with_capacity(100_000);
    for i in 0..100_000u32 {
        // one event every 5 minutes: 100,000 * 300 s ≈ 347 days, all in 2026
        let at = start + chrono::Duration::seconds(i64::from(i) * 300);
        let pr = format!("P{}", i % 1000);
        let e = match i % 5 {
            // 2 of every 5: the user's commits -> 40,000
            0 | 1 => {
                let sha = format!("s{i}");
                let mut e = commit("R_tool", &sha, me(), "2026-01-01T00:00:00Z");
                e.occurred_at = at;
                e
            }
            // 1 of 5: alice's commits -> 20,000
            2 => {
                let mut e = commit(
                    "R_tool",
                    &format!("s{i}"),
                    user("alice"),
                    "2026-01-01T00:00:00Z",
                );
                e.occurred_at = at;
                e
            }
            // 1 of 5: the user's conversation comments on 1,000 PRs -> 20,000
            3 => event(
                EventId::comment(&format!("c{i}")),
                EventType::ConversationComment,
                me(),
                "R_tool",
                Some(&pr),
                at,
            ),
            // 1 of 5: CI check runs -> 20,000
            _ => with_meta(
                event(
                    EventId::check_run(&format!("k{i}")),
                    EventType::CheckRun,
                    bot("github-actions[bot]"),
                    "R_tool",
                    Some(&pr),
                    at,
                ),
                |m| m.check_outcome = Some(ghusage_core::model::CheckOutcome::Success),
            ),
        };
        events.push(e);
    }
    let ds = dataset(vec![tool()], prs, vec![], events);
    assert_eq!(ds.events.len(), 100_000);
    let opts = options("2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z", zone("UTC"));
    let s = summary(&ds, &opts);
    assert_eq!(s.commits, 40_000);
    assert_eq!(s.comments_written.conversation, 20_000);
    assert_eq!(s.ci.passed, 20_000);
    assert_eq!(s.pr_internal_events, 20_000);
    // comments use i ≡ 3 (mod 5), so i mod 1000 ∈ {3, 8, 13, ..., 998}:
    // 1000 / 5 = 200 distinct PRs.
    assert_eq!(s.active_prs, 200);
    // check runs (i ≡ 4 mod 5) touch 200 other PRs but are automation
    assert_eq!(s.active_prs_all_participants, 200);
    let days = metrics::breakdown(&ds, &opts, Granularity::Day);
    assert_eq!(days.len(), 365);
    assert_eq!(days.iter().map(|b| b.summary.commits).sum::<u64>(), 40_000);
    assert_eq!(
        days.iter()
            .map(|b| b.summary.comments_written.conversation)
            .sum::<u64>(),
        20_000
    );
}
