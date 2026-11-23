//! Hand-built activity datasets with expected results worked out by hand from
//! the product's metric definitions.

mod common;

use common::*;
use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind, summarize};
use ghusage_core::metrics::{
    self, CommitDateBasis, Dataset, Filter, Ownership, Participation, PrRow, PrSort, RepoSummary,
    ReviewDirection, Summary, VisibilityFilter, by_repository, pr_detail, pr_rows, review_rows,
    snapshot, summary,
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
    assert_eq!(by_repository(&ds, &opts), Vec::<RepoSummary>::new());
    assert_eq!(
        pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity),
        Vec::<PrRow>::new()
    );
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
        m.is_merge_commit = Some(true);
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
        m.author_date = Some(utc("2026-11-18T09:00:00Z"));
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
        .map(|b| (b.bucket.end - b.bucket.start).as_hours())
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
        .map(|b| (b.bucket.end - b.bucket.start).as_hours())
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
    assert_eq!(
        pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity),
        Vec::<PrRow>::new()
    );
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

    let mut ext = opts;
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
        let at = start + jiff::SignedDuration::from_secs(i64::from(i) * 300);
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

// ---------------------------------------------------------------------------
// Commit authors, discussion participants and threads of a pull request
// ---------------------------------------------------------------------------

/// PR P1: alice commits a1 and a2 (a1 seen twice), bob commits b1, carol's
/// c1 was force-pushed away. carol only adds a label. alice writes a
/// conversation comment; bob writes two inline comments in thread T1; dave
/// writes one inline comment in thread T2.
fn discussion_dataset() -> Dataset {
    let removed = with_meta(
        pr_commit("R_tool", "P1", "c1", user("carol"), "2026-11-02T09:00:00Z"),
        |m| m.removed_observed_at = Some(utc("2026-11-03T00:00:00Z")),
    );
    let events = vec![
        pr_commit("R_tool", "P1", "a1", user("alice"), "2026-11-02T10:00:00Z"),
        pr_commit("R_tool", "P1", "A1", user("alice"), "2026-11-02T10:00:00Z"),
        pr_commit("R_tool", "P1", "a2", user("alice"), "2026-11-02T11:00:00Z"),
        pr_commit("R_tool", "P1", "b1", user("bob"), "2026-11-02T12:00:00Z"),
        removed,
        lifecycle(
            EventId::timeline("label-1"),
            EventType::Labeled,
            "R_tool",
            "P1",
            user("carol"),
            "2026-11-03T09:00:00Z",
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P1",
            "ic1",
            user("alice"),
            "2026-11-03T10:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "rc1",
            "T1",
            None,
            user("bob"),
            "2026-11-03T11:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "rc2",
            "T1",
            Some("rc1"),
            user("bob"),
            "2026-11-03T12:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P1",
            "rc3",
            "T2",
            None,
            user("dave"),
            "2026-11-03T13:00:00Z",
        ),
    ];
    dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, user("alice"))],
        vec![],
        events,
    )
}

#[test]
fn pr_detail_breaks_commits_down_by_author() {
    let d = pr_detail(&discussion_dataset(), "P1", &[]).unwrap();
    // a1 (once), a2 by alice; b1 by bob; carol's c1 is no longer in the PR.
    assert_eq!(d.commits, 3);
    let authors: Vec<(String, u64)> = d
        .commit_authors
        .iter()
        .map(|a| (a.author.clone(), a.commits))
        .collect();
    assert_eq!(
        authors,
        vec![("alice".to_string(), 2), ("bob".to_string(), 1)]
    );
}

#[test]
fn discussion_participants_count_only_comment_authors() {
    let ds = discussion_dataset();
    let d = pr_detail(&ds, "P1", &[]).unwrap();
    // carol only labeled (and her commit was removed); not part of the discussion.
    assert_eq!(d.discussion_participants, vec!["alice", "bob", "dave"]);
    assert_eq!(d.participants, vec!["alice", "bob", "carol", "dave"]);
    let row = &pr_rows(
        &ds,
        &november_utc(),
        Participation::All,
        None,
        PrSort::Activity,
    )[0];
    assert_eq!(row.discussion_participants, 3);
    assert_eq!(row.participants, 4);
}

#[test]
fn intensity_reports_threads_per_pr() {
    let ds = discussion_dataset();
    let opts = november_utc();
    let row = &pr_rows(&ds, &opts, Participation::All, None, PrSort::Activity)[0];
    // Inline comments in T1 (twice) and T2.
    assert_eq!(row.threads, 2);
    let i = metrics::intensity(&ds, &opts, Participation::All);
    assert_eq!(i.threads_per_pr.unwrap().max, 2.0);
    assert_eq!(i.discussion_participants_per_pr.unwrap().max, 3.0);
}

// ---------------------------------------------------------------------------
// Pull request lifecycle timings
// ---------------------------------------------------------------------------

/// A pull request by the user with its opened and (optional) merge events;
/// the user merges it.
fn timed_pull(
    id: &str,
    number: u64,
    created: &str,
    merged: Option<&str>,
) -> (ghusage_core::model::PullRequest, Vec<ActivityEvent>) {
    let mut pr = pull(id, "R_tool", number, me());
    pr.created_at = utc(created);
    let mut events = vec![lifecycle(
        EventId::pr_created(id),
        EventType::PrOpened,
        "R_tool",
        id,
        me(),
        created,
    )];
    if let Some(m) = merged {
        pr.state = PrState::Merged;
        pr.merged_at = Some(utc(m));
        events.push(lifecycle(
            EventId::pr_merged(id),
            EventType::PrMerged,
            "R_tool",
            id,
            me(),
            m,
        ));
    }
    (pr, events)
}

#[test]
fn creation_to_merge_time_is_merged_minus_created() {
    // P1: 11-03 09:00 -> 11-05 15:30 = 2 days 6 h 30 min
    //     = 2*86400 + 6*3600 + 30*60 = 172800 + 21600 + 1800 = 196200 s.
    let (p1, e1) = timed_pull(
        "P1",
        1,
        "2026-11-03T09:00:00Z",
        Some("2026-11-05T15:30:00Z"),
    );
    // P2: still open -> no creation-to-merge time.
    let (p2, e2) = timed_pull("P2", 2, "2026-11-10T00:00:00Z", None);
    // P3: 11-06 00:00 -> 01:00 = 3600 s.
    let (p3, e3) = timed_pull(
        "P3",
        3,
        "2026-11-06T00:00:00Z",
        Some("2026-11-06T01:00:00Z"),
    );
    let events = [e1, e2, e3].concat();
    let ds = dataset(vec![tool()], vec![p1, p2, p3], vec![], events);

    let t1 = metrics::pr_timing(&ds, &ds.pull_requests["P1"]);
    assert_eq!(t1.creation_to_merge_secs, Some(196_200));
    let t2 = metrics::pr_timing(&ds, &ds.pull_requests["P2"]);
    assert_eq!(t2.creation_to_merge_secs, None);
    assert_eq!(
        pr_detail(&ds, "P1", &[])
            .unwrap()
            .timing
            .creation_to_merge_secs,
        Some(196_200)
    );

    // Over the period only merged pull requests contribute: {3600, 196200}.
    // count 2, min 3600, max 196200, median = mean = (3600 + 196200) / 2 = 99900.
    let stats = metrics::timing_stats(&ds, &november_utc())
        .creation_to_merge
        .unwrap();
    assert_eq!(stats.count, 2);
    assert_eq!(stats.min, 3_600.0);
    assert_eq!(stats.max, 196_200.0);
    assert_eq!(stats.median, 99_900.0);
    assert_eq!(stats.mean, 99_900.0);
}

#[test]
fn time_to_first_review_ignores_own_and_unknown_reviews() {
    let (pr, mut events) = timed_pull("P1", 1, "2026-11-03T09:00:00Z", None);
    events.extend([
        // the author's own review is not a review by someone else
        review(
            "R_tool",
            "P1",
            "rv-me",
            me(),
            ReviewState::Commented,
            "2026-11-03T09:05:00Z",
        ),
        // an unknown reviewer (deleted account) cannot be told apart from the author
        review(
            "R_tool",
            "P1",
            "rv-ghost",
            Actor::unknown(),
            ReviewState::Approved,
            "2026-11-03T09:20:00Z",
        ),
        // first review by someone else: 10:15 - 09:00 = 75 min = 4500 s
        review(
            "R_tool",
            "P1",
            "rv-bob",
            user("bob"),
            ReviewState::Approved,
            "2026-11-03T10:15:00Z",
        ),
        review(
            "R_tool",
            "P1",
            "rv-alice",
            user("alice"),
            ReviewState::Approved,
            "2026-11-03T11:00:00Z",
        ),
    ]);
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let t = metrics::pr_timing(&ds, &ds.pull_requests["P1"]);
    assert_eq!(t.time_to_first_review_secs, Some(4_500));
    let stats = metrics::timing_stats(&ds, &november_utc())
        .time_to_first_review
        .unwrap();
    assert_eq!(stats.count, 1);
    assert_eq!(stats.median, 4_500.0);
}

#[test]
fn pr_timing_uses_observed_events_only() {
    let (pr, mut events) = timed_pull(
        "P1",
        1,
        "2026-11-03T09:00:00Z",
        Some("2026-11-03T12:00:00Z"),
    );
    events.extend([
        // 09:10 self-review: ignored for the first review
        review(
            "R_tool",
            "P1",
            "rv-me",
            me(),
            ReviewState::Commented,
            "2026-11-03T09:10:00Z",
        ),
        // 09:30 carol's pending review was never submitted
        review(
            "R_tool",
            "P1",
            "rv-carol",
            user("carol"),
            ReviewState::Pending,
            "2026-11-03T09:30:00Z",
        ),
        // 09:45 dave's review only carries a reply in an existing thread
        review(
            "R_tool",
            "P1",
            "rv-dave",
            user("dave"),
            ReviewState::Commented,
            "2026-11-03T09:45:00Z",
        ),
        with_meta(
            inline_reply(
                "R_tool",
                "P1",
                "c-dave",
                "T1",
                Some("c-root"),
                user("dave"),
                "2026-11-03T09:45:00Z",
            ),
            |m| m.review_id = Some("rv-dave".into()),
        ),
        // 10:00 alice's review: the first real review by someone else
        review(
            "R_tool",
            "P1",
            "rv-alice",
            user("alice"),
            ReviewState::ChangesRequested,
            "2026-11-03T10:00:00Z",
        ),
        // 10:30 the author's next activity after that review
        pr_commit("R_tool", "P1", "c1", me(), "2026-11-03T10:30:00Z"),
        // 12:30 a check run is automation, not activity
        with_meta(
            event(
                EventId::check_run("cr1"),
                EventType::CheckRun,
                bot("github-actions[bot]"),
                "R_tool",
                Some("P1"),
                utc("2026-11-03T12:30:00Z"),
            ),
            |m| m.check_outcome = Some(ghusage_core::model::CheckOutcome::Success),
        ),
        // 13:00 a commit later force-pushed away is no longer observed
        with_meta(
            pr_commit("R_tool", "P1", "c2", me(), "2026-11-03T13:00:00Z"),
            |m| m.removed_observed_at = Some(utc("2026-11-04T00:00:00Z")),
        ),
    ]);
    let ds = dataset(vec![tool()], vec![pr], vec![], events);
    let t = metrics::pr_timing(&ds, &ds.pull_requests["P1"]);
    // first review: 10:00 - 09:00 = 3600 s (the 09:10 own, 09:30 pending and
    // 09:45 reply-only reviews are skipped)
    assert_eq!(t.time_to_first_review_secs, Some(3_600));
    // review -> next author activity: 10:30 - 10:00 = 1800 s
    assert_eq!(t.review_to_next_author_activity_secs, Some(1_800));
    // creation -> merge: 12:00 - 09:00 = 10800 s
    assert_eq!(t.creation_to_merge_secs, Some(10_800));
    // first -> last observed activity: opened 09:00 .. merged 12:00 = 10800 s
    // (the 12:30 check run and the removed 13:00 commit do not count)
    assert_eq!(t.first_to_last_activity_secs, Some(10_800));
}

// ---------------------------------------------------------------------------
// Pull requests spanning a period boundary
// ---------------------------------------------------------------------------

fn opened_in_october_merged_at(merged: &str) -> Dataset {
    let (pr, events) = timed_pull("P1", 1, "2026-10-28T10:00:00Z", Some(merged));
    dataset(vec![tool()], vec![pr], vec![], events)
}

#[test]
fn pr_created_before_the_period_and_merged_inside_counts_only_the_merge() {
    // Opened 10-28 (October), merged by the user 11-02: November sees only
    // the merge. The user's merge is activity on the PR, so it is active.
    let ds = opened_in_october_merged_at("2026-11-02T10:00:00Z");
    let s = summary(&ds, &november_utc());
    assert_eq!(s.prs_created, 0);
    assert_eq!(s.prs_merged, 1);
    assert_eq!(s.merges_performed, 1);
    assert_eq!(s.active_prs, 1);
    let rows = pr_rows(
        &ds,
        &november_utc(),
        Participation::Subject,
        None,
        PrSort::Activity,
    );
    assert_eq!(rows.len(), 1);
    // October sees only the creation.
    let october = options("2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z", zone("UTC"));
    let s = summary(&ds, &october);
    assert_eq!(s.prs_created, 1);
    assert_eq!(s.prs_merged, 0);

    // Periods are half-open [start, end): a merge exactly at the start of
    // November belongs to November ...
    let ds = opened_in_october_merged_at("2026-11-01T00:00:00Z");
    assert_eq!(summary(&ds, &november_utc()).prs_merged, 1);
    assert_eq!(summary(&ds, &october).prs_merged, 0);
    // ... and a merge exactly at its end belongs to December.
    let ds = opened_in_october_merged_at("2026-12-01T00:00:00Z");
    assert_eq!(summary(&ds, &november_utc()).prs_merged, 0);
    let december = options("2026-12-01T00:00:00Z", "2027-01-01T00:00:00Z", zone("UTC"));
    assert_eq!(summary(&ds, &december).prs_merged, 1);
}

// ---------------------------------------------------------------------------
// Commit authorship
// ---------------------------------------------------------------------------

#[test]
fn author_and_committer_both_the_user_count_once() {
    let events = vec![
        // authored and committed by the user
        with_meta(commit("R_tool", "aaa", me(), "2026-11-10T10:00:00Z"), |m| {
            m.committer = Some(me());
        }),
        // the same commit inside a pull request (sync stores both)
        pr_commit("R_tool", "P1", "aaa", me(), "2026-11-10T10:00:00Z"),
        // authored by alice and only committed by the user: not the user's commit
        with_meta(
            commit("R_tool", "bbb", user("alice"), "2026-11-10T11:00:00Z"),
            |m| m.committer = Some(me()),
        ),
    ];
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![],
        events,
    );
    let s = summary(&ds, &november_utc());
    // {aaa} once; bbb is alice's
    assert_eq!(s.commits, 1);
    assert_eq!(s.pr_commits, 1);
}

// ---------------------------------------------------------------------------
// Issues
// ---------------------------------------------------------------------------

fn issue_event(id: EventId, t: EventType, repo_id: &str, actor: Actor, at: &str) -> ActivityEvent {
    event(id, t, actor, repo_id, None, utc(at))
}

#[test]
fn issue_opened_counts_on_its_creation_day_in_the_zone() {
    let mut i1 = issue("I1", "R_tool", 1, me());
    i1.created_at = utc("2026-11-14T15:30:00Z");
    i1.is_open = false;
    i1.closed_at = Some(utc("2026-11-16T03:00:00Z"));
    let mut events = vec![
        issue_event(
            EventId::issue_created("I1"),
            EventType::IssueOpened,
            "R_tool",
            me(),
            "2026-11-14T15:30:00Z",
        ),
        issue_event(
            EventId::timeline("close-1"),
            EventType::IssueClosed,
            "R_tool",
            me(),
            "2026-11-16T03:00:00Z",
        ),
        // someone else's issue is not the user's
        issue_event(
            EventId::issue_created("I2"),
            EventType::IssueOpened,
            "R_tool",
            user("alice"),
            "2026-11-14T15:30:00Z",
        ),
    ];
    // Issue events refer to their issue.
    events[0].entity_id = "I1".into();
    events[1].entity_id = "I1".into();
    events[2].entity_id = "I2".into();
    let ds = Dataset::new(
        subject(),
        true,
        vec![tool()],
        vec![],
        vec![i1],
        vec![],
        events,
        Vec::new(),
    );
    let per_day = |zone_name: &str| -> Vec<(u64, u64)> {
        let opts = local_days("2026-11-14", "2026-11-17", zone(zone_name));
        metrics::breakdown(&ds, &opts, Granularity::Day)
            .iter()
            .map(|d| (d.summary.issues_opened, d.summary.issues_closed))
            .collect()
    };
    // Tokyo (UTC+9): opened 11-14 15:30Z = 11-15 00:30; closed 11-16 03:00Z
    // = 11-16 12:00. Days 14, 15, 16.
    assert_eq!(per_day("Asia/Tokyo"), vec![(0, 0), (1, 0), (0, 1)]);
    // UTC: opened 11-14, closed 11-16.
    assert_eq!(per_day("UTC"), vec![(1, 0), (0, 0), (0, 1)]);

    // The closing day alone has no opening.
    let close_day = local_days("2026-11-16", "2026-11-17", zone("Asia/Tokyo"));
    let s = summary(&ds, &close_day);
    assert_eq!((s.issues_opened, s.issues_closed), (0, 1));
    let rows = metrics::issue_rows(
        &ds,
        &local_days("2026-11-15", "2026-11-16", zone("Asia/Tokyo")),
    );
    let row = rows.iter().find(|r| r.issue_id == "I1").unwrap();
    assert!(row.opened_in_period);
    assert_eq!(row.closed_in_period, 0);
}

#[test]
fn issues_are_counted_per_repository() {
    let app = repo("R_app", "acme", "app", Visibility::Public);
    let events = vec![
        issue_event(
            EventId::issue_created("I1"),
            EventType::IssueOpened,
            "R_tool",
            me(),
            "2026-11-02T10:00:00Z",
        ),
        issue_event(
            EventId::issue_created("I2"),
            EventType::IssueOpened,
            "R_tool",
            me(),
            "2026-11-03T10:00:00Z",
        ),
        issue_event(
            EventId::timeline("close-i1"),
            EventType::IssueClosed,
            "R_tool",
            me(),
            "2026-11-04T10:00:00Z",
        ),
        issue_event(
            EventId::issue_created("I3"),
            EventType::IssueOpened,
            "R_app",
            me(),
            "2026-11-05T10:00:00Z",
        ),
        // bob's issue in acme/app is not the user's
        issue_event(
            EventId::issue_created("I4"),
            EventType::IssueOpened,
            "R_app",
            user("bob"),
            "2026-11-05T11:00:00Z",
        ),
    ];
    let ds = dataset(vec![tool(), app], vec![], vec![], events);
    let opts = november_utc();
    let rows = by_repository(&ds, &opts);
    let of = |name: &str| {
        let s = &rows.iter().find(|r| r.full_name == name).unwrap().summary;
        (s.issues_opened, s.issues_closed)
    };
    // me/tool: opened I1, I2 and closed I1; acme/app: opened I3.
    assert_eq!(of("me/tool"), (2, 1));
    assert_eq!(of("acme/app"), (1, 0));
    let s = summary(&ds, &opts);
    assert_eq!((s.issues_opened, s.issues_closed), (3, 1));
}

// ---------------------------------------------------------------------------
// Commits and threads shared or repeated
// ---------------------------------------------------------------------------

#[test]
fn same_commit_in_two_pull_requests_keeps_both_links() {
    let events = vec![
        commit("R_tool", "s1", me(), "2026-11-10T10:00:00Z"),
        pr_commit("R_tool", "P1", "s1", me(), "2026-11-10T10:00:00Z"),
        pr_commit("R_tool", "P2", "s1", me(), "2026-11-10T10:00:00Z"),
    ];
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me()), pull("P2", "R_tool", 2, me())],
        vec![],
        events,
    );
    let s = summary(&ds, &november_utc());
    // One commit globally, linked to two pull requests: (P1, s1), (P2, s1).
    assert_eq!(s.commits, 1);
    assert_eq!(s.pr_commits, 2);
    assert_eq!(pr_detail(&ds, "P1", &[]).unwrap().commits, 1);
    assert_eq!(pr_detail(&ds, "P2", &[]).unwrap().commits, 1);
    let rows = pr_rows(
        &ds,
        &november_utc(),
        Participation::All,
        None,
        PrSort::Activity,
    );
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.commits == 1));
}

#[test]
fn duplicate_review_thread_counts_once() {
    let opener = inline_reply(
        "R_tool",
        "P1",
        "c1",
        "T1",
        None,
        user("alice"),
        "2026-11-10T10:00:00Z",
    );
    let events = vec![
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            "2026-11-10T09:00:00Z",
        ),
        opener.clone(),
        // the same comment fetched again
        opener,
    ];
    let t = thread("T1", "P1", Some(user("alice")));
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![t.clone(), t],
        events,
    );
    // One thread id -> one thread; one comment id -> one comment.
    assert_eq!(snapshot(&ds, &november_utc()).threads.total, 1);
    let d = pr_detail(&ds, "P1", &[]).unwrap();
    assert_eq!(d.threads.total, 1);
    assert_eq!(d.comments.inline, 1);
    let row = &pr_rows(
        &ds,
        &november_utc(),
        Participation::All,
        None,
        PrSort::Activity,
    )[0];
    assert_eq!(row.threads, 1);
    assert_eq!(row.comments, 1);
    assert_eq!(summary(&ds, &november_utc()).comments_received.inline, 1);
}

// ---------------------------------------------------------------------------
// Other people's coordination
// ---------------------------------------------------------------------------

#[test]
fn others_coordination_is_shown_but_not_counted() {
    let events = vec![
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            "2026-11-10T09:00:00Z",
        ),
        with_meta(
            lifecycle(
                EventId::timeline("rr-1"),
                EventType::ReviewRequested,
                "R_tool",
                "P1",
                user("alice"),
                "2026-11-10T09:05:00Z",
            ),
            |m| m.target = Some("bob".into()),
        ),
        with_meta(
            lifecycle(
                EventId::timeline("as-1"),
                EventType::Assigned,
                "R_tool",
                "P1",
                user("alice"),
                "2026-11-10T09:06:00Z",
            ),
            |m| m.target = Some("me".into()),
        ),
        lifecycle(
            EventId::timeline("as-2"),
            EventType::Assigned,
            "R_tool",
            "P1",
            Actor::unknown(),
            "2026-11-10T09:07:00Z",
        ),
        lifecycle(
            EventId::timeline("lb-1"),
            EventType::Labeled,
            "R_tool",
            "P1",
            user("alice"),
            "2026-11-10T09:08:00Z",
        ),
    ];
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![],
        events,
    );
    let s = summary(&ds, &november_utc());
    // Being assigned or having reviewers requested by alice (or by an
    // unknown actor) is not the user's action, even on the user's own PR.
    assert_eq!(s.coordination_actions, 0);
    assert_eq!(s.metadata_actions, 0);
    // The user's only pull request event is opening it.
    assert_eq!(s.pr_internal_events, 1);
    // All five events are on the timeline, in time order.
    let types: Vec<EventType> = metrics::timeline(&ds, "P1", &[])
        .iter()
        .map(|e| e.event_type)
        .collect();
    assert_eq!(
        types,
        vec![
            EventType::PrOpened,
            EventType::ReviewRequested,
            EventType::Assigned,
            EventType::Assigned,
            EventType::Labeled,
        ]
    );
}

// ---------------------------------------------------------------------------
// Activity intensity
// ---------------------------------------------------------------------------

/// P1 (user's): opened only (plus a check run, which is automation).
///   events 1, reviews 0, comments 0, commits 0, participants 1, days 1
/// P2 (user's): opened and commit c1 on 11-03; alice approves and comments
///   on 11-04.
///   events 4, reviews 1, comments 1, commits 1, participants 2, days 2
/// P3 (alice's): alice commits, opens and requests a review on 11-05; the
///   user reviews on 11-05 and comments inline on 11-06; alice replies on
///   11-06; bob comments on 11-07.
///   events 7, reviews 1, comments 3, commits 1, participants 3, days 3
/// P4 (bob's): bob opens and comments on 11-08; the user is not involved.
///   events 2, reviews 0, comments 1, commits 0, participants 1, days 1
fn intensity_dataset() -> Dataset {
    let events = vec![
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            "2026-11-02T10:00:00Z",
        ),
        with_meta(
            event(
                EventId::check_run("cr1"),
                EventType::CheckRun,
                bot("github-actions[bot]"),
                "R_tool",
                Some("P1"),
                utc("2026-11-02T10:05:00Z"),
            ),
            |m| m.check_outcome = Some(ghusage_core::model::CheckOutcome::Success),
        ),
        lifecycle(
            EventId::pr_created("P2"),
            EventType::PrOpened,
            "R_tool",
            "P2",
            me(),
            "2026-11-03T10:00:00Z",
        ),
        pr_commit("R_tool", "P2", "c1", me(), "2026-11-03T11:00:00Z"),
        review(
            "R_tool",
            "P2",
            "rv-a",
            user("alice"),
            ReviewState::Approved,
            "2026-11-04T10:00:00Z",
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P2",
            "ic-a",
            user("alice"),
            "2026-11-04T11:00:00Z",
        ),
        pr_commit("R_tool", "P3", "a1", user("alice"), "2026-11-05T09:00:00Z"),
        lifecycle(
            EventId::pr_created("P3"),
            EventType::PrOpened,
            "R_tool",
            "P3",
            user("alice"),
            "2026-11-05T10:00:00Z",
        ),
        lifecycle(
            EventId::timeline("rr-p3"),
            EventType::ReviewRequested,
            "R_tool",
            "P3",
            user("alice"),
            "2026-11-05T10:30:00Z",
        ),
        review(
            "R_tool",
            "P3",
            "rv-me",
            me(),
            ReviewState::Commented,
            "2026-11-05T12:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P3",
            "rc-me",
            "T3",
            None,
            me(),
            "2026-11-06T10:00:00Z",
        ),
        inline_reply(
            "R_tool",
            "P3",
            "rc-a",
            "T3",
            Some("rc-me"),
            user("alice"),
            "2026-11-06T11:00:00Z",
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P3",
            "ic-b",
            user("bob"),
            "2026-11-07T10:00:00Z",
        ),
        lifecycle(
            EventId::pr_created("P4"),
            EventType::PrOpened,
            "R_tool",
            "P4",
            user("bob"),
            "2026-11-08T10:00:00Z",
        ),
        comment(
            EventType::ConversationComment,
            "R_tool",
            "P4",
            "ic-b4",
            user("bob"),
            "2026-11-08T11:00:00Z",
        ),
    ];
    dataset(
        vec![tool()],
        vec![
            pull("P1", "R_tool", 1, me()),
            pull("P2", "R_tool", 2, me()),
            pull("P3", "R_tool", 3, user("alice")),
            pull("P4", "R_tool", 4, user("bob")),
        ],
        vec![thread("T3", "P3", Some(me()))],
        events,
    )
}

/// (count, min, median, mean, p90, max)
fn stat(s: Option<metrics::Stats>) -> (u64, f64, f64, f64, f64, f64) {
    let s = s.expect("statistics");
    (s.count, s.min, s.median, s.mean, s.p90, s.max)
}

#[test]
fn intensity_statistics_over_active_prs() {
    let ds = intensity_dataset();
    let opts = november_utc();

    // The user took part in P1, P2 and P3.
    let i = metrics::intensity(&ds, &opts, Participation::Subject);
    assert_eq!(i.prs, 3);
    // events {1, 4, 7}: median 4, mean 12/3 = 4, p90 = rank ceil(2.7) = 3 -> 7
    assert_eq!(stat(i.events_per_pr), (3, 1.0, 4.0, 4.0, 7.0, 7.0));
    // reviews {0, 1, 1}: mean 2/3
    assert_eq!(stat(i.reviews_per_pr), (3, 0.0, 1.0, 2.0 / 3.0, 1.0, 1.0));
    // comments {0, 1, 3}: mean 4/3
    assert_eq!(stat(i.comments_per_pr), (3, 0.0, 1.0, 4.0 / 3.0, 3.0, 3.0));
    // commits {0, 1, 1}
    assert_eq!(stat(i.commits_per_pr), (3, 0.0, 1.0, 2.0 / 3.0, 1.0, 1.0));
    // participants {1, 2, 3}
    assert_eq!(stat(i.participants_per_pr), (3, 1.0, 2.0, 2.0, 3.0, 3.0));
    // active days {1, 2, 3}
    assert_eq!(stat(i.active_days_per_pr), (3, 1.0, 2.0, 2.0, 3.0, 3.0));

    // All participants adds bob's P4.
    let i = metrics::intensity(&ds, &opts, Participation::All);
    assert_eq!(i.prs, 4);
    // events {1, 2, 4, 7}: median (2 + 4) / 2 = 3, mean 14/4 = 3.5,
    // p90 = rank ceil(3.6) = 4 -> 7
    assert_eq!(stat(i.events_per_pr), (4, 1.0, 3.0, 3.5, 7.0, 7.0));
    // active days {1, 2, 3, 1}: sorted {1, 1, 2, 3}, median 1.5, mean 7/4
    assert_eq!(stat(i.active_days_per_pr), (4, 1.0, 1.5, 1.75, 3.0, 3.0));
}

#[test]
fn intensity_is_empty_without_pull_request_activity() {
    let ds = dataset(vec![tool()], vec![], vec![], vec![]);
    let i = metrics::intensity(&ds, &november_utc(), Participation::All);
    assert_eq!(i.prs, 0);
    assert_eq!(i.events_per_pr, None);
    assert_eq!(i.active_days_per_pr, None);
}

#[test]
fn statistics_use_the_middle_pair_and_a_nearest_rank_percentile() {
    use metrics::Stats;
    assert_eq!(Stats::from_values(vec![]), None);
    // sorted {1, 2, 3, 4}: median (2 + 3) / 2 = 2.5, mean 2.5,
    // p90 = rank ceil(3.6) = 4 -> 4
    assert_eq!(
        stat(Stats::from_values(vec![4.0, 1.0, 3.0, 2.0])),
        (4, 1.0, 2.5, 2.5, 4.0, 4.0)
    );
    // a single value is every statistic
    assert_eq!(
        stat(Stats::from_values(vec![5.0])),
        (1, 5.0, 5.0, 5.0, 5.0, 5.0)
    );
    // 1..=10: median (5 + 6) / 2 = 5.5, mean 5.5, p90 = rank 9 -> 9
    assert_eq!(
        stat(Stats::from_values((1..=10).map(f64::from).collect())),
        (10, 1.0, 5.5, 5.5, 9.0, 10.0)
    );
}

// ---------------------------------------------------------------------------
// Reviews on their submission days
// ---------------------------------------------------------------------------

#[test]
fn reviews_land_on_their_submission_days() {
    let events = vec![
        // Tokyo 11-20 10:00
        lifecycle(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            "R_tool",
            "P1",
            me(),
            "2026-11-20T01:00:00Z",
        ),
        // Tokyo 11-21 12:00
        review(
            "R_tool",
            "P1",
            "rv-a",
            user("alice"),
            ReviewState::Approved,
            "2026-11-21T03:00:00Z",
        ),
        // Tokyo 11-22 01:00 (still 11-21 in UTC)
        review(
            "R_tool",
            "P1",
            "rv-b",
            user("bob"),
            ReviewState::Commented,
            "2026-11-21T16:00:00Z",
        ),
    ];
    let ds = dataset(
        vec![tool()],
        vec![pull("P1", "R_tool", 1, me())],
        vec![],
        events,
    );
    let per_day = |zone_name: &str| -> (Vec<u64>, Vec<u64>) {
        let opts = local_days("2026-11-20", "2026-11-23", zone(zone_name));
        let days = metrics::breakdown(&ds, &opts, Granularity::Day);
        (
            days.iter()
                .map(|d| d.summary.reviews_received.total)
                .collect(),
            days.iter().map(|d| d.summary.prs_created).collect(),
        )
    };
    assert_eq!(per_day("Asia/Tokyo"), (vec![0, 1, 1], vec![1, 0, 0]));
    assert_eq!(per_day("UTC"), (vec![0, 2, 0], vec![1, 0, 0]));
}
