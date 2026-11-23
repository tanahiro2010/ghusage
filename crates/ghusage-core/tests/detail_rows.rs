//! Listings behind the detail views (commits, activity, issue detail).
//! Expected values are worked out by hand from the dataset below.

mod common;

use common::*;
use ghusage_core::metrics::{
    CommitDateBasis, Dataset, Filter, ReviewDirection, activity_rows, comment_rows, commit_rows,
    issue_detail, review_rows, summary,
};
use ghusage_core::model::{
    ActivityEvent, EventId, EventType, Issue, ReviewState, Subject, Visibility,
};

fn issue_event(id: &str, t: EventType, issue: &str, actor_login: &str, at: &str) -> ActivityEvent {
    let actor = if actor_login == "me" {
        me()
    } else {
        user(actor_login)
    };
    let repo = if issue == "I1" { "R1" } else { "R2" };
    let mut e = event(EventId::timeline(id), t, actor, repo, None, utc(at));
    e.entity_id = issue.into();
    e
}

/// Two repositories, two pull requests, two issues; period is
/// 2026-03-01..2026-03-03 (UTC).
///
/// | # | event                         | actor | when (UTC)        | listed? |
/// |---|-------------------------------|-------|-------------------|---------|
/// | 1 | commit a1 in R1               | me    | 03-01 10:00       | yes     |
/// | 2 | commit a1 again (same id)     | me    | 03-01 10:00       | dedup   |
/// | 3 | commit b1 in R2 (author 02-28)| me    | 03-02 09:00       | yes     |
/// | 4 | commit c1 in R1               | bob   | 03-01 11:00       | no      |
/// | 5 | commit d1 in R1               | me    | 03-05 (outside)   | no      |
/// | 6 | P1 opened                     | me    | 03-01 09:00       | yes     |
/// | 7 | P1 commit a1                  | me    | 03-01 10:05       | yes     |
/// | 8 | review on P1 (approved)       | bob   | 03-01 12:00       | yes (received) |
/// | 9 | inline comment on P1          | bob   | 03-01 12:00       | yes (received) |
/// |10 | check run on P1               | bot   | 03-01 13:00       | no (CI) |
/// |11 | review on P2 (changes req.)   | me    | 03-02 08:00       | yes     |
/// |12 | comment on P2 (bob's PR)      | carol | 03-02 08:30       | no      |
/// |13 | P1 merged                     | me    | 03-02 10:00       | yes     |
/// |14 | I1 opened                     | me    | 03-02 11:00       | yes     |
/// |15 | I2 closed                     | me    | 03-02 12:00       | yes     |
/// |16 | I2 opened                     | bob   | 02-20 (outside)   | no      |
/// |17 | pending review on P2          | me    | 03-02 13:00       | no      |
fn world() -> Dataset {
    let repos = vec![
        repo("R1", "me", "app", Visibility::Public),
        repo("R2", "org", "lib", Visibility::Public),
    ];
    let prs = vec![pull("P1", "R1", 1, me()), pull("P2", "R2", 2, user("bob"))];
    let issues: Vec<Issue> = vec![
        issue("I1", "R1", 5, me()),
        issue("I2", "R2", 6, user("bob")),
    ];
    let review = |id: &str, pr: &str, repo: &str, who, state, at: &str| {
        with_meta(
            event(
                EventId::review(id),
                EventType::ReviewSubmitted,
                who,
                repo,
                Some(pr),
                utc(at),
            ),
            |m| {
                m.review_state = Some(state);
                m.review_id = Some(id.into());
            },
        )
    };
    let events = vec![
        commit("R1", "a1", me(), "2026-03-01T10:00:00Z"),
        commit("R1", "a1", me(), "2026-03-01T10:00:00Z"),
        with_meta(commit("R2", "b1", me(), "2026-03-02T09:00:00Z"), |m| {
            m.author_date = Some(utc("2026-02-28T23:00:00Z"));
        }),
        commit("R1", "c1", user("bob"), "2026-03-01T11:00:00Z"),
        commit("R1", "d1", me(), "2026-03-05T10:00:00Z"),
        event(
            EventId::pr_created("P1"),
            EventType::PrOpened,
            me(),
            "R1",
            Some("P1"),
            utc("2026-03-01T09:00:00Z"),
        ),
        pr_commit("R1", "P1", "a1", me(), "2026-03-01T10:05:00Z"),
        review(
            "RV1",
            "P1",
            "R1",
            user("bob"),
            ReviewState::Approved,
            "2026-03-01T12:00:00Z",
        ),
        event(
            EventId::comment("C1"),
            EventType::InlineComment,
            user("bob"),
            "R1",
            Some("P1"),
            utc("2026-03-01T12:00:00Z"),
        ),
        event(
            EventId::check_run("K1"),
            EventType::CheckRun,
            bot("ci[bot]"),
            "R1",
            Some("P1"),
            utc("2026-03-01T13:00:00Z"),
        ),
        review(
            "RV2",
            "P2",
            "R2",
            me(),
            ReviewState::ChangesRequested,
            "2026-03-02T08:00:00Z",
        ),
        event(
            EventId::comment("C2"),
            EventType::ConversationComment,
            user("carol"),
            "R2",
            Some("P2"),
            utc("2026-03-02T08:30:00Z"),
        ),
        event(
            EventId::pr_merged("P1"),
            EventType::PrMerged,
            me(),
            "R1",
            Some("P1"),
            utc("2026-03-02T10:00:00Z"),
        ),
        issue_event(
            "E14",
            EventType::IssueOpened,
            "I1",
            "me",
            "2026-03-02T11:00:00Z",
        ),
        issue_event(
            "E15",
            EventType::IssueClosed,
            "I2",
            "me",
            "2026-03-02T12:00:00Z",
        ),
        issue_event(
            "E16",
            EventType::IssueOpened,
            "I2",
            "bob",
            "2026-02-20T12:00:00Z",
        ),
        review(
            "RV3",
            "P2",
            "R2",
            me(),
            ReviewState::Pending,
            "2026-03-02T13:00:00Z",
        ),
    ];
    Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        repos,
        prs,
        issues,
        Vec::new(),
        events,
        Vec::new(),
    )
}

fn period() -> ghusage_core::metrics::Options {
    options("2026-03-01T00:00:00Z", "2026-03-03T00:00:00Z", zone("UTC"))
}

#[test]
fn commit_rows_list_each_counted_commit_once() {
    let ds = world();
    let opts = period();
    let rows = commit_rows(&ds, &opts);
    let shas: Vec<&str> = rows.iter().map(|r| r.sha.as_deref().unwrap()).collect();
    // a1 (deduplicated) and b1; bob's c1 and the out-of-period d1 are not ours.
    assert_eq!(shas, vec!["a1", "b1"]);
    assert_eq!(rows[0].repository, "me/app");
    assert_eq!(rows[1].repository, "org/lib");
    assert_eq!(rows[1].committed_at, utc("2026-03-02T09:00:00Z"));
    assert_eq!(rows.len() as u64, summary(&ds, &opts).commits);
}

#[test]
fn commit_rows_follow_the_commit_date_basis() {
    let ds = world();
    let mut opts = period();
    opts.commit_basis = CommitDateBasis::Author;
    let rows = commit_rows(&ds, &opts);
    // b1 was authored on 02-28, before the period.
    let shas: Vec<&str> = rows.iter().map(|r| r.sha.as_deref().unwrap()).collect();
    assert_eq!(shas, vec!["a1"]);
    assert_eq!(summary(&ds, &opts).commits, 1);
}

#[test]
fn commit_rows_respect_the_repository_filter_and_a_single_day() {
    let ds = world();
    let mut opts = period();
    opts.filter = Filter {
        repositories: vec!["org/lib".into()],
        ..Filter::default()
    };
    let rows = commit_rows(&ds, &opts);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sha.as_deref(), Some("b1"));

    let day = options("2026-03-01T00:00:00Z", "2026-03-02T00:00:00Z", zone("UTC"));
    let rows = commit_rows(&ds, &day);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sha.as_deref(), Some("a1"));
}

#[test]
fn activity_rows_list_own_events_and_feedback_received_in_order() {
    let ds = world();
    let opts = period();
    let rows = activity_rows(&ds, &opts);
    let types: Vec<EventType> = rows.iter().map(|r| r.event_type).collect();
    // Same instant (03-01 12:00): the inline comment sorts before the review.
    assert_eq!(
        types,
        vec![
            EventType::PrOpened,
            EventType::CommitAuthored,
            EventType::PrCommit,
            EventType::InlineComment,
            EventType::ReviewSubmitted,
            EventType::ReviewSubmitted,
            EventType::CommitAuthored,
            EventType::PrMerged,
            EventType::IssueOpened,
            EventType::IssueClosed,
        ]
    );
    let by_subject: Vec<bool> = rows.iter().map(|r| r.by_subject).collect();
    assert_eq!(
        by_subject,
        vec![true, true, true, false, false, true, true, true, true, true]
    );
    // Pull request events carry the pull request; issue events the issue.
    assert_eq!(rows[0].pr_id.as_deref(), Some("P1"));
    assert_eq!(rows[0].number, Some(1));
    assert_eq!(rows[0].title.as_deref(), Some("Pull request 1"));
    assert_eq!(rows[1].pr_id, None);
    assert_eq!(rows[1].number, None);
    assert_eq!(rows[8].issue_id.as_deref(), Some("I1"));
    assert_eq!(rows[8].number, Some(5));
    assert_eq!(rows[9].issue_id.as_deref(), Some("I2"));
    assert_eq!(rows[9].title.as_deref(), Some("Issue 6"));
    assert_eq!(rows[9].pr_id, None);

    // Listed reviews match the counted reviews.
    let s = summary(&ds, &opts);
    let reviews = |mine: bool| {
        rows.iter()
            .filter(|r| r.event_type == EventType::ReviewSubmitted && r.by_subject == mine)
            .count() as u64
    };
    assert_eq!(reviews(true), s.reviews_submitted.total);
    assert_eq!(reviews(false), s.reviews_received.total);
    assert_eq!(
        (s.reviews_submitted.total, s.reviews_received.total),
        (1, 1)
    );
}

#[test]
fn activity_rows_of_one_day_and_one_repository() {
    let ds = world();
    let day = options("2026-03-02T00:00:00Z", "2026-03-03T00:00:00Z", zone("UTC"));
    let types: Vec<EventType> = activity_rows(&ds, &day)
        .iter()
        .map(|r| r.event_type)
        .collect();
    assert_eq!(
        types,
        vec![
            EventType::ReviewSubmitted,
            EventType::CommitAuthored,
            EventType::PrMerged,
            EventType::IssueOpened,
            EventType::IssueClosed,
        ]
    );

    let mut opts = period();
    opts.filter = Filter {
        repositories: vec!["R2".into()],
        ..Filter::default()
    };
    let ids: Vec<String> = activity_rows(&ds, &opts)
        .into_iter()
        .map(|r| r.event_id)
        .collect();
    assert_eq!(
        ids,
        vec![
            EventId::review("RV2").0,
            EventId::commit("R2", "b1").0,
            EventId::timeline("E15").0,
        ]
    );
}

#[test]
fn issue_detail_lists_the_issue_events_in_the_period() {
    let ds = world();
    let d = issue_detail(&ds, &period(), "I2").unwrap();
    assert_eq!(d.issue.number, 6);
    assert_eq!(d.repository, "org/lib");
    // Bob opened it on 02-20, before the period.
    assert_eq!(d.events.len(), 1);
    assert_eq!(d.events[0].event_type, EventType::IssueClosed);
    assert!(d.events[0].by_subject);

    let wide = options("2026-02-01T00:00:00Z", "2026-03-03T00:00:00Z", zone("UTC"));
    let d = issue_detail(&ds, &wide, "I2").unwrap();
    let who: Vec<&str> = d.events.iter().map(|e| e.actor.as_str()).collect();
    assert_eq!(who, vec!["bob", "me"]);
    assert!(issue_detail(&ds, &wide, "nope").is_none());
}

#[test]
fn review_and_comment_rows_carry_their_pull_request() {
    let ds = world();
    let opts = period();
    let submitted = review_rows(&ds, &opts, ReviewDirection::Submitted);
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].pr_id.as_deref(), Some("P2"));
    let received = review_rows(&ds, &opts, ReviewDirection::Received);
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].pr_id.as_deref(), Some("P1"));
    let comments = comment_rows(&ds, &opts);
    // Bob's inline comment on my P1; Carol's comment is on Bob's P2.
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].pr_id.as_deref(), Some("P1"));
}
