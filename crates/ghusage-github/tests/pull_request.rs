//! Pull request fetching and normalization, issues and repositories.

mod common;

use common::*;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{
    ActorKind, CheckOutcome, Collection, EventId, EventType, PrState, ReviewState, Source,
    Visibility,
};
use ghusage_github::{ApiError, PrBundle};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR_ID: &str = "PR_kwDOwidget42";
const REPO_ID: &str = "R_kgDOwidget";

fn oid(n: u64) -> String {
    sha(n)
}

fn acme() -> Value {
    gql_actor("Organization", "acme", "O_acme")
}

fn repo() -> Value {
    gql_repo(REPO_ID, acme(), "widget", "PUBLIC", false)
}

fn bot(login: &str) -> Value {
    gql_actor("Bot", login, &format!("BOT_{login}"))
}

/// A merged pull request with a bit of everything:
/// opened 2024-03-01 by alice, merged 2024-03-04 by bob; force-pushed once;
/// reviews by bob, carol (dismissed) and alice's reply; dave's pending review;
/// two threads (one resolved, one outdated); conversation comments by alice,
/// a bot and a deleted account.
fn rich_pr() -> Value {
    let thread1_c1 = review_comment(
        "PRRC_1",
        user("bob"),
        "2024-03-02T10:58:00Z",
        "PRR_1",
        None,
        None,
    );
    let thread1_c2 = review_comment(
        "PRRC_2",
        user("alice"),
        "2024-03-02T13:00:00Z",
        "PRR_5",
        Some("PRRC_1"),
        None,
    );
    let thread2_c1 = review_comment(
        "PRRC_3",
        user("carol"),
        "2024-03-02T11:59:00Z",
        "PRR_2",
        None,
        Some("2024-03-02T12:30:00Z"),
    );
    json!({
        "id": PR_ID,
        "number": 42,
        "title": "Add widget",
        "url": "https://github.com/acme/widget/pull/42",
        "state": "MERGED",
        "isDraft": false,
        "createdAt": "2024-03-01T09:00:00Z",
        "updatedAt": "2024-03-04T15:31:00Z",
        "closedAt": "2024-03-04T15:30:00Z",
        "mergedAt": "2024-03-04T15:30:00Z",
        "headRefOid": oid(3),
        "author": user("alice"),
        "mergedBy": user("bob"),
        "headCommit": { "nodes": [ { "commit": head_commit_checks() } ] },
        "commits": connection(vec![
            pr_commit("acme", "widget", 42, &oid(1), Some("alice"), "2024-02-28T23:00:00Z", "2024-03-01T08:00:00Z", 1),
            pr_commit("acme", "widget", 42, &oid(3), Some("alice"), "2024-03-02T09:00:00Z", "2024-03-02T09:59:00Z", 1),
        ], false, Some("c2")),
        "reviews": connection(vec![
            review("PRR_1", "COMMENTED", "Looks good overall", Some("2024-03-02T11:00:00Z"), user("bob"), vec![thread1_c1.clone()]),
            review("PRR_2", "DISMISSED", "", Some("2024-03-02T12:00:00Z"), user("carol"), vec![thread2_c1.clone()]),
            review("PRR_5", "COMMENTED", "", Some("2024-03-02T13:00:00Z"), user("alice"), vec![thread1_c2.clone()]),
            review("PRR_3", "APPROVED", "  ", Some("2024-03-04T15:00:00Z"), user("bob"), vec![]),
            review("PRR_4", "PENDING", "draft thoughts", None, user("dave"), vec![]),
        ], false, Some("r5")),
        "reviewThreads": connection(vec![
            thread("PRRT_1", true, false, Some(user("alice")), "src/widget.rs", vec![thread1_c1, thread1_c2]),
            thread("PRRT_2", false, true, None, "src/lib.rs", vec![thread2_c1]),
        ], false, Some("t2")),
        "comments": connection(vec![
            issue_comment("IC_1", user("alice"), "2024-03-01T10:00:00Z", None),
            issue_comment("IC_2", bot("github-actions"), "2024-03-01T10:01:00Z", None),
            issue_comment("IC_3", Value::Null, "2024-03-01T10:02:00Z", Some("2024-03-01T11:00:00Z")),
        ], false, Some("ic3")),
        "timelineItems": connection(vec![
            tl("ReviewRequestedEvent", "RRE_1", "2024-03-01T09:05:00Z", user("alice"), json!({ "requestedReviewer": { "login": "bob" } })),
            tl("ReviewRequestedEvent", "RRE_2", "2024-03-01T09:05:00Z", user("alice"), json!({ "requestedReviewer": { "slug": "core" } })),
            tl("ReviewRequestRemovedEvent", "RRRE_1", "2024-03-01T09:06:00Z", user("alice"), json!({ "requestedReviewer": null })),
            tl("LabeledEvent", "LE_1", "2024-03-01T09:07:00Z", user("alice"), json!({ "label": { "name": "bug" } })),
            tl("UnlabeledEvent", "ULE_1", "2024-03-01T09:08:00Z", user("alice"), json!({ "label": { "name": "bug" } })),
            tl("AssignedEvent", "AE_1", "2024-03-01T09:09:00Z", user("alice"), json!({ "assignee": { "login": "alice" } })),
            tl("UnassignedEvent", "UAE_1", "2024-03-01T09:10:00Z", user("alice"), json!({ "assignee": { "login": "alice" } })),
            tl("MilestonedEvent", "MIE_1", "2024-03-01T09:11:00Z", user("alice"), json!({ "milestoneTitle": "v1.0" })),
            tl("DemilestonedEvent", "DME_1", "2024-03-01T09:12:00Z", user("alice"), json!({ "milestoneTitle": "v1.0" })),
            tl("ConvertToDraftEvent", "CTDE_1", "2024-03-01T09:13:00Z", user("alice"), json!({ "url": "https://github.com/acme/widget/pull/42#event-1" })),
            tl("ReadyForReviewEvent", "RFRE_1", "2024-03-01T09:14:00Z", user("alice"), json!({ "url": "https://github.com/acme/widget/pull/42#event-2" })),
            tl("HeadRefForcePushedEvent", "HRFPE_1", "2024-03-02T10:00:00Z", user("alice"), json!({ "beforeCommit": { "oid": oid(2) }, "afterCommit": { "oid": oid(3) } })),
            tl("RenamedTitleEvent", "RTE_1", "2024-03-02T10:05:00Z", user("alice"), json!({ "previousTitle": "WIP: widget", "currentTitle": "Add widget" })),
            tl("ReviewDismissedEvent", "RDE_1", "2024-03-03T08:00:00Z", user("alice"), json!({ "review": { "id": "PRR_2" }, "previousReviewState": "CHANGES_REQUESTED", "url": "https://github.com/acme/widget/pull/42#event-3" })),
            tl("MergedEvent", "ME_1", "2024-03-04T15:30:00Z", user("bob"), json!({ "url": "https://github.com/acme/widget/pull/42#event-4" })),
            tl("ClosedEvent", "CE_1", "2024-03-04T15:30:00Z", user("bob"), json!({ "url": "https://github.com/acme/widget/pull/42#event-5" })),
            json!({ "__typename": "SomeFutureEvent", "id": "SFE_1", "createdAt": "2024-03-04T16:00:00Z", "brandNewField": 1 }),
        ], false, Some("tl17"))
    })
}

fn pr_response(pr: Value) -> Value {
    let mut r = repo();
    r["pullRequest"] = pr;
    gql_data(json!({ "repository": r }))
}

/// The head commit as the pull request query returns it: the same check runs
/// as `check_runs()` (a failed build re-run in a new suite and a queued
/// lint) and the fact that it has commit statuses.
fn head_commit_checks() -> Value {
    let app = json!({ "id": "MDM6QXBwMTUzNjg=", "slug": "github-actions" });
    let suite = |db_id: u64, runs: Vec<Value>| json!({ "databaseId": db_id, "app": app, "checkRuns": connection(runs, false, Some("Y3Vyc29yOnYyOpHPAAAAAQ==")) });
    json!({
        "oid": oid(3),
        "statusCheckRollup": { "state": "SUCCESS" },
        "status": { "state": "SUCCESS" },
        "checkSuites": connection(vec![
            suite(901, vec![json!({
                "id": "CR_1", "name": "build", "status": "COMPLETED", "conclusion": "FAILURE",
                "startedAt": "2024-03-02T10:00:30Z", "completedAt": "2024-03-02T10:00:50Z",
                "permalink": "https://github.com/acme/widget/actions/runs/1/job/1"
            })]),
            suite(902, vec![json!({
                "id": "CR_2", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS",
                "startedAt": "2024-03-02T10:01:00Z", "completedAt": "2024-03-02T10:05:00Z",
                "permalink": "https://github.com/acme/widget/actions/runs/2/job/2"
            })]),
            suite(903, vec![json!({
                "id": "CR_3", "name": "lint", "status": "QUEUED", "conclusion": null,
                "startedAt": null, "completedAt": null,
                "permalink": "https://github.com/acme/widget/actions/runs/3/job/3"
            })]),
            // An app that was asked to check the commit but created no runs.
            json!({ "databaseId": 904, "app": { "id": "MDM6QXBwOTQyNg==", "slug": "dependabot" },
                    "checkRuns": connection(vec![], false, None) }),
        ], false, Some("Y3Vyc29yOnYyOpHPAAAABA==")),
    })
}

/// REST check runs of the head commit, used when the pull request query
/// cannot return them all.
fn check_runs() -> Value {
    let app = json!({ "id": 15368, "slug": "github-actions", "node_id": "MDM6QXBwMTUzNjg=", "name": "GitHub Actions", "owner": rest_user("github", "O_github", "Organization") });
    json!({
        "total_count": 3,
        "check_runs": [
            {
                "id": 2, "node_id": "CR_2", "head_sha": oid(3), "name": "build", "status": "completed",
                "conclusion": "success", "started_at": "2024-03-02T10:01:00Z", "completed_at": "2024-03-02T10:05:00Z",
                "html_url": "https://github.com/acme/widget/actions/runs/2/job/2", "check_suite": { "id": 902 },
                "app": app, "output": { "title": null, "summary": null }
            },
            {
                "id": 1, "node_id": "CR_1", "head_sha": oid(3), "name": "build", "status": "completed",
                "conclusion": "failure", "started_at": "2024-03-02T10:00:30Z", "completed_at": "2024-03-02T10:00:50Z",
                "html_url": "https://github.com/acme/widget/actions/runs/1/job/1", "check_suite": { "id": 901 },
                "app": app
            },
            {
                "id": 3, "node_id": "CR_3", "head_sha": oid(3), "name": "lint", "status": "queued",
                "conclusion": null, "started_at": null, "completed_at": null,
                "html_url": "https://github.com/acme/widget/actions/runs/3/job/3", "check_suite": { "id": 903 },
                "app": app
            }
        ]
    })
}

fn statuses() -> Value {
    json!([
        {
            "url": format!("https://api.github.com/repos/acme/widget/statuses/{}", oid(3)),
            "avatar_url": null, "id": 11, "node_id": "SC_11", "state": "success",
            "description": "Build passed", "target_url": "https://ci.example.com/11",
            "context": "ci/legacy", "created_at": "2024-03-02T10:06:00Z", "updated_at": "2024-03-02T10:06:00Z",
            "creator": rest_user("ci-user", "U_ci", "User")
        }
    ])
}

async fn mount_main(server: &MockServer, body: Value) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestDetail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mounts the REST checks of the head commit: commit statuses and, for the
/// fallback path, check runs.
async fn mount_checks(server: &MockServer, head: &str) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{head}/check-runs"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(check_runs()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/repos/acme/widget/commits/{head}/statuses")))
        .respond_with(ResponseTemplate::new(200).set_body_json(statuses()))
        .mount(server)
        .await;
}

async fn fetch_rich() -> PrBundle {
    let server = MockServer::start().await;
    mount_main(&server, pr_response(rich_pr())).await;
    mount_checks(&server, &oid(3)).await;
    client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap()
}

fn of_type(b: &PrBundle, t: EventType) -> Vec<&ghusage_core::model::ActivityEvent> {
    b.events.iter().filter(|e| e.event_type == t).collect()
}

fn find<'a>(b: &'a PrBundle, id: &EventId) -> &'a ghusage_core::model::ActivityEvent {
    b.events
        .iter()
        .find(|e| &e.event_id == id)
        .unwrap_or_else(|| panic!("missing {id}"))
}

#[tokio::test]
async fn fully_fetched_pull_request_is_complete() {
    let b = fetch_rich().await;
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(b.complete_collections.len(), Collection::ALL.len());
    assert!(
        b.events
            .iter()
            .all(|e| e.coverage == CoverageStatus::Complete)
    );
    assert_eq!(b.pull_request.coverage, CoverageStatus::Complete);
    // The unsupported timeline item is skipped and noted.
    assert!(b.notes.iter().any(|n| n.contains("SomeFutureEvent")));
    // Every event belongs to the base repository and the pull request.
    assert!(b.events.iter().all(|e| e.repository_id == REPO_ID));
    assert!(
        b.events
            .iter()
            .all(|e| e.parent_pr_id.as_deref() == Some(PR_ID))
    );
    // Ids are unique.
    let ids: std::collections::HashSet<_> = b.events.iter().map(|e| &e.event_id).collect();
    assert_eq!(ids.len(), b.events.len());
    // Known actors always carry their node id.
    assert!(
        b.events
            .iter()
            .filter(|e| !e.actor.is_unknown())
            .all(|e| e.actor.id.is_some())
    );
    // Ordered by time.
    assert!(
        b.events
            .windows(2)
            .all(|w| w[0].occurred_at <= w[1].occurred_at)
    );
}

#[tokio::test]
async fn commit_message_headlines_are_kept_with_pull_request_commits() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["commits"]["nodes"][0]["commit"]["messageHeadline"] = json!("fix: parse empty input");
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    let commits = of_type(&b, EventType::PrCommit);
    let first = commits
        .iter()
        .find(|e| e.meta.sha.as_deref() == Some(oid(1).as_str()))
        .unwrap();
    assert_eq!(
        first.meta.message_headline.as_deref(),
        Some("fix: parse empty input")
    );
    let second = commits
        .iter()
        .find(|e| e.meta.sha.as_deref() == Some(oid(3).as_str()))
        .unwrap();
    assert_eq!(second.meta.message_headline, None);
}

#[tokio::test]
async fn repository_and_snapshot_are_normalized() {
    let b = fetch_rich().await;
    let r = &b.repository;
    assert_eq!(r.id, REPO_ID);
    assert_eq!((r.owner.as_str(), r.name.as_str()), ("acme", "widget"));
    assert_eq!(r.owner_kind, ActorKind::Organization);
    assert_eq!(r.visibility, Visibility::Public);
    assert_eq!(r.viewer_permission.as_deref(), Some("WRITE"));
    assert!(r.accessible);
    let pr = &b.pull_request;
    assert_eq!(pr.id, PR_ID);
    assert_eq!(pr.repository_id, REPO_ID);
    assert_eq!(pr.number, 42);
    assert_eq!(pr.state, PrState::Merged);
    assert_eq!(pr.head_sha.as_deref(), Some(oid(3).as_str()));
    assert_eq!(pr.latest_ci, Some(CheckOutcome::Success));
    assert!(pr.synced_at.is_some());
}

#[tokio::test]
async fn pr_created_and_merged_on_different_days() {
    let b = fetch_rich().await;
    let opened = find(&b, &EventId::pr_created(PR_ID));
    assert_eq!(opened.event_type, EventType::PrOpened);
    assert_eq!(opened.occurred_at, ts("2024-03-01T09:00:00Z"));
    assert_eq!(opened.actor.login.as_deref(), Some("alice"));
    let merged = find(&b, &EventId::pr_merged(PR_ID));
    assert_eq!(merged.event_type, EventType::PrMerged);
    assert_eq!(merged.occurred_at, ts("2024-03-04T15:30:00Z"));
    // The merger is not confused with the author.
    assert_eq!(merged.actor.login.as_deref(), Some("bob"));
    assert_eq!(merged.entity_id, "ME_1");
    assert_eq!(merged.source, Source::Graphql);
    assert_eq!(of_type(&b, EventType::PrMerged).len(), 1);
    assert_eq!(b.pull_request.author.login.as_deref(), Some("alice"));
    assert_eq!(
        b.pull_request
            .merged_by
            .as_ref()
            .and_then(|a| a.login.as_deref()),
        Some("bob")
    );
    assert_eq!(b.pull_request.created_at, ts("2024-03-01T09:00:00Z"));
    assert_eq!(b.pull_request.merged_at, Some(ts("2024-03-04T15:30:00Z")));
}

#[tokio::test]
async fn close_that_is_part_of_a_merge_is_marked() {
    let b = fetch_rich().await;
    let closed = find(&b, &EventId::timeline("CE_1"));
    assert_eq!(closed.event_type, EventType::PrClosed);
    assert_eq!(closed.meta.via_merge, Some(true));
}

#[tokio::test]
async fn close_without_merge_is_marked_as_not_merged() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["state"] = json!("CLOSED");
    pr["mergedAt"] = Value::Null;
    pr["mergedBy"] = Value::Null;
    pr["closedAt"] = json!("2024-03-04T15:30:00Z");
    pr["timelineItems"] = connection(
        vec![
            tl(
                "ClosedEvent",
                "CE_a",
                "2024-03-02T15:30:00Z",
                user("bob"),
                json!({}),
            ),
            tl(
                "ReopenedEvent",
                "RE_a",
                "2024-03-03T15:30:00Z",
                user("alice"),
                json!({}),
            ),
            tl(
                "ClosedEvent",
                "CE_b",
                "2024-03-04T15:30:00Z",
                user("alice"),
                json!({}),
            ),
        ],
        false,
        None,
    );
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.pull_request.state, PrState::Closed);
    assert_eq!(
        of_type(&b, EventType::PrMerged),
        Vec::<&ghusage_core::model::ActivityEvent>::new()
    );
    let closes = of_type(&b, EventType::PrClosed);
    assert_eq!(closes.len(), 2);
    assert!(closes.iter().all(|e| e.meta.via_merge == Some(false)));
    assert_eq!(
        find(&b, &EventId::timeline("CE_a")).actor.id.as_deref(),
        Some("U_bob")
    );
}

#[tokio::test]
async fn early_close_of_a_later_merged_pull_request_is_not_part_of_the_merge() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    let mut items = pr["timelineItems"]["nodes"].as_array().unwrap().clone();
    items.push(tl(
        "ClosedEvent",
        "CE_early",
        "2024-03-02T08:00:00Z",
        user("alice"),
        json!({}),
    ));
    items.push(tl(
        "ReopenedEvent",
        "RE_early",
        "2024-03-02T08:30:00Z",
        user("alice"),
        json!({}),
    ));
    pr["timelineItems"] = connection(items, false, None);
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(
        find(&b, &EventId::timeline("CE_early")).meta.via_merge,
        Some(false)
    );
    assert_eq!(
        find(&b, &EventId::timeline("CE_1")).meta.via_merge,
        Some(true)
    );
    assert!(
        of_type(&b, EventType::PrClosed)
            .iter()
            .all(|e| e.meta.via_merge.is_some())
    );
}

#[tokio::test]
async fn multiple_reviews_including_pending_and_dismissed() {
    let b = fetch_rich().await;
    let reviews = of_type(&b, EventType::ReviewSubmitted);
    let mut ids: Vec<_> = reviews.iter().map(|e| e.entity_id.as_str()).collect();
    ids.sort_unstable();
    // The pending review is not a submission.
    assert_eq!(ids, vec!["PRR_1", "PRR_2", "PRR_3", "PRR_5"]);
    let state = |id: &str| find(&b, &EventId::review(id)).meta.review_state;
    assert_eq!(state("PRR_1"), Some(ReviewState::Commented));
    assert_eq!(state("PRR_3"), Some(ReviewState::Approved));
    // The dismissed review keeps its original state from the dismissal event.
    assert_eq!(state("PRR_2"), Some(ReviewState::ChangesRequested));
    let r1 = find(&b, &EventId::review("PRR_1"));
    assert_eq!(r1.occurred_at, ts("2024-03-02T11:00:00Z"));
    assert_eq!(r1.actor.login.as_deref(), Some("bob"));
    let dismissed = find(&b, &EventId::timeline("RDE_1"));
    assert_eq!(dismissed.event_type, EventType::ReviewDismissed);
    assert_eq!(dismissed.meta.review_id.as_deref(), Some("PRR_2"));
    assert_eq!(dismissed.actor.login.as_deref(), Some("alice"));
    assert!(b.notes.iter().any(|n| n.contains("pending")));
}

#[tokio::test]
async fn inline_conversation_and_summary_comments_are_kept_apart() {
    let b = fetch_rich().await;
    // Only a non-blank review body is a summary.
    let summaries = of_type(&b, EventType::ReviewSummary);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].event_id, EventId::review_summary("PRR_1"));
    assert_eq!(summaries[0].occurred_at, ts("2024-03-02T11:00:00Z"));

    // Inline comments appear once although they are listed under both the
    // review and the thread.
    let inline = of_type(&b, EventType::InlineComment);
    let mut ids: Vec<_> = inline.iter().map(|e| e.entity_id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["PRRC_1", "PRRC_2", "PRRC_3"]);
    let reply = find(&b, &EventId::comment("PRRC_2"));
    assert_eq!(reply.meta.thread_id.as_deref(), Some("PRRT_1"));
    assert_eq!(reply.meta.review_id.as_deref(), Some("PRR_5"));
    assert_eq!(reply.meta.reply_to.as_deref(), Some("PRRC_1"));
    let edited = find(&b, &EventId::comment("PRRC_3"));
    assert_eq!(edited.meta.edited_at, Some(ts("2024-03-02T12:30:00Z")));
    assert_eq!(edited.occurred_at, ts("2024-03-02T11:59:00Z"));

    let conv = of_type(&b, EventType::ConversationComment);
    assert_eq!(conv.len(), 3);
    let c3 = find(&b, &EventId::comment("IC_3"));
    assert_eq!(c3.meta.edited_at, Some(ts("2024-03-01T11:00:00Z")));
}

#[tokio::test]
async fn bot_and_unknown_actors() {
    let b = fetch_rich().await;
    let by_bot = find(&b, &EventId::comment("IC_2"));
    assert_eq!(by_bot.actor.kind, ActorKind::Bot);
    assert!(by_bot.actor.is_bot());
    let deleted = find(&b, &EventId::comment("IC_3"));
    assert!(deleted.actor.is_unknown());
    assert_eq!(deleted.actor.login, None);
}

#[tokio::test]
async fn review_threads_resolved_and_outdated() {
    let b = fetch_rich().await;
    assert_eq!(b.threads.len(), 2);
    let t1 = b.threads.iter().find(|t| t.id == "PRRT_1").unwrap();
    assert!(t1.is_resolved);
    assert!(!t1.is_outdated);
    assert_eq!(
        t1.resolved_by.as_ref().and_then(|a| a.login.as_deref()),
        Some("alice")
    );
    assert_eq!(t1.comment_count, 2);
    assert_eq!(
        t1.first_comment_author
            .as_ref()
            .and_then(|a| a.login.as_deref()),
        Some("bob")
    );
    assert_eq!(t1.path.as_deref(), Some("src/widget.rs"));
    assert_eq!(t1.pr_id, PR_ID);
    let t2 = b.threads.iter().find(|t| t.id == "PRRT_2").unwrap();
    assert!(!t2.is_resolved);
    assert!(t2.is_outdated);
    assert_eq!(t2.resolved_by, None);
}

#[tokio::test]
async fn force_push_and_current_commit_set() {
    let b = fetch_rich().await;
    assert_eq!(b.current_commit_shas, vec![oid(1), oid(3)]);
    let pushed = find(&b, &EventId::timeline("HRFPE_1"));
    assert_eq!(pushed.event_type, EventType::PrHeadForcePushed);
    assert_eq!(pushed.meta.target, Some(format!("{}->{}", oid(2), oid(3))));
    let commits = of_type(&b, EventType::PrCommit);
    assert_eq!(commits.len(), 2);
    // The replaced commit is not invented.
    assert!(commits.iter().all(|e| e.entity_id != oid(2)));
    let c3 = find(&b, &EventId::pr_commit(PR_ID, &oid(3)));
    // Commit time, not a guessed push time.
    assert_eq!(c3.occurred_at, ts("2024-03-02T09:59:00Z"));
    assert_eq!(c3.meta.author_date, Some(ts("2024-03-02T09:00:00Z")));
    assert_eq!(c3.meta.time_basis.as_deref(), Some("commit_date"));
    assert_eq!(c3.meta.is_merge_commit, Some(false));
    assert_eq!(c3.actor.login.as_deref(), Some("alice"));
    assert_eq!(
        c3.url.as_deref(),
        Some(format!("https://github.com/acme/widget/pull/42/commits/{}", oid(3)).as_str())
    );
}

#[tokio::test]
async fn coordination_and_metadata_events() {
    let b = fetch_rich().await;
    let target = |id: &str| find(&b, &EventId::timeline(id)).meta.target.clone();
    assert_eq!(target("RRE_1").as_deref(), Some("bob"));
    assert_eq!(target("RRE_2").as_deref(), Some("core"));
    assert_eq!(target("RRRE_1"), None);
    assert_eq!(target("LE_1").as_deref(), Some("bug"));
    assert_eq!(target("AE_1").as_deref(), Some("alice"));
    assert_eq!(target("MIE_1").as_deref(), Some("v1.0"));
    let renamed = find(&b, &EventId::timeline("RTE_1"));
    assert_eq!(renamed.event_type, EventType::Renamed);
    assert_eq!(renamed.meta.previous_title.as_deref(), Some("WIP: widget"));
    assert_eq!(renamed.meta.target.as_deref(), Some("Add widget"));
    let ty = |id: &str| find(&b, &EventId::timeline(id)).event_type;
    assert_eq!(ty("RRRE_1"), EventType::ReviewRequestRemoved);
    assert_eq!(ty("ULE_1"), EventType::Unlabeled);
    assert_eq!(ty("UAE_1"), EventType::Unassigned);
    assert_eq!(ty("DME_1"), EventType::Demilestoned);
    assert_eq!(ty("CTDE_1"), EventType::PrConvertedToDraft);
    assert_eq!(ty("RFRE_1"), EventType::PrReadyForReview);
}

#[tokio::test]
async fn checks_and_statuses_of_the_head_commit() {
    let b = fetch_rich().await;
    let runs = of_type(&b, EventType::CheckRun);
    // The queued run has no time yet; re-runs are distinct runs.
    assert_eq!(runs.len(), 2);
    let ok = find(&b, &EventId::check_run("CR_2"));
    assert_eq!(ok.meta.check_outcome, Some(CheckOutcome::Success));
    assert_eq!(ok.meta.check_name.as_deref(), Some("build"));
    assert_eq!(ok.meta.check_suite_id.as_deref(), Some("902"));
    assert_eq!(ok.occurred_at, ts("2024-03-02T10:05:00Z"));
    assert_eq!(ok.source, Source::Graphql);
    assert!(ok.actor.is_bot());
    let failed = find(&b, &EventId::check_run("CR_1"));
    assert_eq!(failed.meta.check_outcome, Some(CheckOutcome::Failure));
    assert_eq!(failed.meta.check_suite_id.as_deref(), Some("901"));
    let status = find(&b, &EventId::commit_status("SC_11"));
    assert_eq!(status.event_type, EventType::CommitStatus);
    assert_eq!(status.meta.check_name.as_deref(), Some("ci/legacy"));
    assert_eq!(status.meta.check_outcome, Some(CheckOutcome::Success));
    assert_eq!(
        b.events
            .iter()
            .filter(|e| e.event_type.is_automation())
            .count(),
        3
    );
}

/// REST paths requested from the server, in order.
async fn rest_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() != "/graphql")
        .map(|r| r.url.path().to_string())
        .collect()
}

#[tokio::test]
async fn head_commit_check_runs_come_from_the_pull_request_query() {
    let server = MockServer::start().await;
    mount_main(&server, pr_response(rich_pr())).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{}/check-runs",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(check_runs()))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{}/statuses",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(statuses()))
        .expect(1)
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    // Only the statuses, which GraphQL cannot list in full, use REST.
    assert_eq!(
        rest_paths(&server).await,
        vec![format!("/repos/acme/widget/commits/{}/statuses", oid(3))]
    );
    let runs = of_type(&b, EventType::CheckRun);
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| r.source == Source::Graphql));
    let ok = find(&b, &EventId::check_run("CR_2"));
    assert_eq!(ok.actor.id.as_deref(), Some("MDM6QXBwMTUzNjg="));
    assert_eq!(ok.actor.login.as_deref(), Some("github-actions"));
    assert_eq!(
        ok.url.as_deref(),
        Some("https://github.com/acme/widget/actions/runs/2/job/2")
    );
    assert_eq!(ok.meta.sha.as_deref(), Some(oid(3).as_str()));
    let status = find(&b, &EventId::commit_status("SC_11"));
    assert_eq!(status.source, Source::Rest);
}

#[tokio::test]
async fn head_commit_without_statuses_needs_no_rest_request() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["headCommit"]["nodes"][0]["commit"]["status"] = Value::Null;
    mount_main(&server, pr_response(pr)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(rest_paths(&server).await, Vec::<String>::new());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(of_type(&b, EventType::CheckRun).len(), 2);
    assert_eq!(
        of_type(&b, EventType::CommitStatus),
        Vec::<&ghusage_core::model::ActivityEvent>::new()
    );
}

/// Fetches the rich pull request with a modified head commit and returns
/// the bundle and the REST paths requested.
async fn fetch_with_head_commit(
    edit: impl FnOnce(&mut Value),
    errors: Option<Value>,
) -> (PrBundle, Vec<String>) {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    edit(&mut pr["headCommit"]["nodes"][0]["commit"]);
    let mut body = pr_response(pr);
    if let Some(errors) = errors {
        body["errors"] = errors;
    }
    mount_main(&server, body).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    (b, rest_paths(&server).await)
}

/// A change to the head commit of the pull request query.
type HeadCommitEdit = Box<dyn FnOnce(&mut Value)>;

#[tokio::test]
async fn incomplete_check_data_in_the_query_falls_back_to_rest() {
    let runs_path = format!("/repos/acme/widget/commits/{}/check-runs", oid(3));
    let statuses_path = format!("/repos/acme/widget/commits/{}/statuses", oid(3));
    let cases: Vec<(&str, HeadCommitEdit, Option<Value>)> = vec![
        (
            "more check runs in a suite than one page",
            Box::new(|c: &mut Value| {
                c["checkSuites"]["nodes"][1]["checkRuns"]["pageInfo"]["hasNextPage"] = json!(true);
            }),
            None,
        ),
        (
            "more check suites than one page",
            Box::new(|c: &mut Value| c["checkSuites"]["pageInfo"]["hasNextPage"] = json!(true)),
            None,
        ),
        (
            "check suites not returned",
            Box::new(|c: &mut Value| {
                c.as_object_mut().unwrap().remove("checkSuites");
            }),
            None,
        ),
        (
            "null check run",
            Box::new(|c: &mut Value| {
                c["checkSuites"]["nodes"][0]["checkRuns"]["nodes"][0] = Value::Null;
            }),
            None,
        ),
        (
            "error inside the check suites",
            Box::new(|c: &mut Value| c["checkSuites"] = Value::Null),
            Some(json!([{
                "type": "FORBIDDEN",
                "path": ["repository", "pullRequest", "headCommit", "nodes", 0, "commit", "checkSuites"],
                "locations": [{ "line": 1, "column": 1 }],
                "message": "Resource not accessible by integration"
            }])),
        ),
        (
            "error without a path",
            Box::new(|_: &mut Value| {}),
            Some(json!([{ "message": "Something went wrong while executing your query." }])),
        ),
    ];
    for (what, edit, errors) in cases {
        let (b, paths) = fetch_with_head_commit(edit, errors).await;
        // The REST runs are the same as the GraphQL ones; none are lost
        // and none are counted twice.
        assert_eq!(
            paths,
            vec![runs_path.clone(), statuses_path.clone()],
            "{what}"
        );
        let runs = of_type(&b, EventType::CheckRun);
        assert_eq!(runs.len(), 2, "{what}");
        assert!(runs.iter().all(|r| r.source == Source::Rest), "{what}");
        assert_eq!(
            find(&b, &EventId::check_run("CR_2"))
                .meta
                .check_suite_id
                .as_deref(),
            Some("902"),
            "{what}"
        );
        assert!(
            b.complete_collections.contains(&Collection::Checks),
            "{what}"
        );
        assert!(
            !b.notes.iter().any(|n| n.contains("latest CI state")),
            "{what}: {:?}",
            b.notes
        );
    }
}

/// CI counts of a fetched pull request, as the summary reports them.
fn ci_counts(b: &PrBundle) -> ghusage_core::metrics::CiCounts {
    use ghusage_core::metrics::{CommitDateBasis, Dataset, Filter, Options, summary};
    let ds = Dataset::new(
        ghusage_core::model::Subject {
            id: None,
            login: "alice".into(),
        },
        true,
        vec![b.repository.clone()],
        vec![b.pull_request.clone()],
        vec![],
        b.threads.clone(),
        b.events.clone(),
        vec![],
    );
    let opts = Options {
        period: ghusage_core::time::Period::new(
            ts("2024-03-01T00:00:00Z"),
            ts("2024-04-01T00:00:00Z"),
        )
        .unwrap(),
        zone: ghusage_core::time::Zone::utc(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::default(),
    };
    summary(&ds, &opts).ci
}

#[tokio::test]
async fn check_runs_request_all_attempts_not_only_latest() {
    // A build that failed and was re-run successfully in the same suite:
    // both attempts happened and both are counted.
    let attempt = |id: &str, conclusion: &str, done: &str| {
        json!({ "id": id, "name": "build", "status": "COMPLETED", "conclusion": conclusion,
                "startedAt": "2024-03-02T10:00:00Z", "completedAt": done,
                "permalink": format!("https://github.com/acme/widget/runs/{id}") })
    };
    let rerun_suite = |more: bool| {
        connection(
            vec![
                json!({ "databaseId": 77, "app": null, "checkRuns": connection(
                vec![
                    attempt("CR_try1", "FAILURE", "2024-03-02T10:01:00Z"),
                    attempt("CR_try2", "SUCCESS", "2024-03-02T10:09:00Z"),
                ],
                more,
                Some("Y3Vyc29yOnYyOpHPAAAAAg=="),
            ) }),
            ],
            false,
            Some("Y3Vyc29yOnYyOpHPAAAAAQ=="),
        )
    };
    let runs_path = format!("/repos/acme/widget/commits/{}/check-runs", oid(3));
    let statuses_path = format!("/repos/acme/widget/commits/{}/statuses", oid(3));
    let assert_both_attempts = |b: &PrBundle, source: Source, what: &str| {
        assert_eq!(b.status, CoverageStatus::Complete, "{what}: {:?}", b.notes);
        let runs = of_type(b, EventType::CheckRun);
        assert_eq!(runs.len(), 2, "{what}");
        assert!(runs.iter().all(|r| r.source == source), "{what}");
        assert_eq!(
            find(b, &EventId::check_run("CR_try1")).meta.check_outcome,
            Some(CheckOutcome::Failure),
            "{what}"
        );
        assert_eq!(
            find(b, &EventId::check_run("CR_try2")).meta.check_outcome,
            Some(CheckOutcome::Success),
            "{what}"
        );
        let ci = ci_counts(b);
        assert_eq!((ci.failed, ci.passed), (1, 1), "{what}");
    };

    // From the pull request query: it must ask for every attempt.
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["headCommit"]["nodes"][0]["commit"]["checkSuites"] = rerun_suite(false);
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestDetail"))
        .and(body_string_contains("checkType: ALL"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_response(pr)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(statuses_path.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_both_attempts(&b, Source::Graphql, "pull request query");

    // From the REST fallback: it must ask for `filter=all` (the endpoint's
    // default returns only the latest run of each name).
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["headCommit"]["nodes"][0]["commit"]["checkSuites"] = rerun_suite(true);
    mount_main(&server, pr_response(pr)).await;
    let rest_run = |id: u64, node: &str, conclusion: &str, done: &str| {
        json!({ "id": id, "node_id": node, "head_sha": oid(3), "name": "build", "status": "completed",
                "conclusion": conclusion, "started_at": "2024-03-02T10:00:00Z", "completed_at": done,
                "html_url": format!("https://github.com/acme/widget/runs/{id}"),
                "check_suite": { "id": 77 }, "app": null })
    };
    Mock::given(method("GET"))
        .and(path(runs_path.clone()))
        .and(wiremock::matchers::query_param("filter", "all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 2,
            "check_runs": [
                rest_run(2, "CR_try2", "success", "2024-03-02T10:09:00Z"),
                rest_run(1, "CR_try1", "failure", "2024-03-02T10:01:00Z"),
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(statuses_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_both_attempts(&b, Source::Rest, "REST fallback");
}

#[tokio::test]
async fn graphql_partial_errors_make_the_collection_partial() {
    let server = MockServer::start().await;
    let mut body = pr_response(rich_pr());
    body["errors"] = json!([{
        "type": "SOMETHING_WENT_WRONG",
        "path": ["repository", "pullRequest", "reviews", "nodes", 1, "author"],
        "locations": [{ "line": 9, "column": 7 }],
        "message": "Something went wrong while executing your query."
    }]);
    mount_main(&server, body).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Partial);
    assert!(!b.complete_collections.contains(&Collection::Reviews));
    assert!(b.complete_collections.contains(&Collection::Timeline));
    assert_eq!(b.pull_request.coverage, CoverageStatus::Partial);
    let review = find(&b, &EventId::review("PRR_1"));
    assert_eq!(review.coverage, CoverageStatus::Partial);
    assert_eq!(
        find(&b, &EventId::comment("IC_1")).coverage,
        CoverageStatus::Complete
    );
    assert!(b.notes.iter().any(|n| n.contains("Something went wrong")));
}

/// A pull request whose connections need follow-up pages.
fn paged_pr() -> Value {
    let mut pr = rich_pr();
    let timeline: Vec<Value> = (0..100)
        .map(|i| {
            tl(
                "LabeledEvent",
                &format!("LE_p1_{i}"),
                "2024-03-01T12:00:00Z",
                user("alice"),
                json!({ "label": { "name": format!("l{i}") } }),
            )
        })
        .collect();
    pr["timelineItems"] = connection(timeline, true, Some("tl-100"));
    let comments: Vec<Value> = (0..100)
        .map(|i| {
            issue_comment(
                &format!("IC_p1_{i}"),
                user("bob"),
                "2024-03-01T12:00:00Z",
                None,
            )
        })
        .collect();
    pr["comments"] = connection(comments, true, Some("ic-100"));
    let first_thread_comments: Vec<Value> = (0..50)
        .map(|i| {
            review_comment(
                &format!("TC_p1_{i}"),
                user("carol"),
                "2024-03-02T12:00:00Z",
                "PRR_2",
                None,
                None,
            )
        })
        .collect();
    pr["reviewThreads"] = connection(
        vec![json!({
            "id": "PRRT_big",
            "isResolved": false,
            "isOutdated": false,
            "path": "src/big.rs",
            "resolvedBy": null,
            "comments": { "totalCount": 60, "pageInfo": page_info(true, Some("tc-50")), "nodes": first_thread_comments }
        })],
        false,
        Some("t1"),
    );
    pr
}

async fn mount_page(server: &MockServer, query: &str, after: &str, body: Value) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains(format!("query {query}")))
        .and(body_partial_json(
            json!({ "variables": { "after": after } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn nested_connections_are_paginated() {
    let server = MockServer::start().await;
    mount_main(&server, pr_response(paged_pr())).await;
    mount_checks(&server, &oid(3)).await;
    let tl2: Vec<Value> = (0..20)
        .map(|i| {
            tl(
                "LabeledEvent",
                &format!("LE_p2_{i}"),
                "2024-03-01T13:00:00Z",
                user("alice"),
                json!({ "label": { "name": "x" } }),
            )
        })
        .collect();
    mount_page(
        &server,
        "PullRequestTimelinePage",
        "tl-100",
        gql_data(json!({ "node": { "timelineItems": connection(tl2, false, Some("tl-120")) } })),
    )
    .await;
    let ic2: Vec<Value> = (0..50)
        .map(|i| {
            issue_comment(
                &format!("IC_p2_{i}"),
                user("bob"),
                "2024-03-01T13:00:00Z",
                None,
            )
        })
        .collect();
    mount_page(
        &server,
        "PullRequestCommentsPage",
        "ic-100",
        gql_data(json!({ "node": { "comments": connection(ic2, false, Some("ic-150")) } })),
    )
    .await;
    let tc2: Vec<Value> = (0..10)
        .map(|i| {
            review_comment(
                &format!("TC_p2_{i}"),
                user("carol"),
                "2024-03-02T13:00:00Z",
                "PRR_2",
                None,
                None,
            )
        })
        .collect();
    mount_page(
        &server,
        "ReviewThreadCommentsPage",
        "tc-50",
        gql_data(json!({ "node": { "comments": { "totalCount": 60, "pageInfo": page_info(false, Some("tc-60")), "nodes": tc2 } } })),
    )
    .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(of_type(&b, EventType::Labeled).len(), 120);
    assert_eq!(of_type(&b, EventType::ConversationComment).len(), 150);
    // 60 thread comments plus the inline comments listed under reviews.
    let inline = of_type(&b, EventType::InlineComment);
    assert_eq!(
        inline
            .iter()
            .filter(|e| e.entity_id.starts_with("TC_"))
            .count(),
        60
    );
    assert_eq!(b.threads[0].comment_count, 60);
    // The follow-up queries asked for the pull request by node id.
    let reqs = server.received_requests().await.unwrap();
    let followups: Vec<Value> = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["variables"]["after"].is_string())
        .collect();
    assert_eq!(followups.len(), 3);
    assert!(
        followups
            .iter()
            .any(|b| b["variables"]["id"] == json!(PR_ID))
    );
    assert!(
        followups
            .iter()
            .any(|b| b["variables"]["id"] == json!("PRRT_big"))
    );
}

#[tokio::test]
async fn failure_on_a_later_page_is_partial_never_complete() {
    let server = MockServer::start().await;
    mount_main(&server, pr_response(paged_pr())).await;
    mount_checks(&server, &oid(3)).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestTimelinePage"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    mount_page(
        &server,
        "PullRequestCommentsPage",
        "ic-100",
        gql_data(json!({ "node": { "comments": connection(vec![], false, None) } })),
    )
    .await;
    mount_page(
        &server,
        "ReviewThreadCommentsPage",
        "tc-50",
        gql_data(json!({ "node": { "comments": { "totalCount": 50, "pageInfo": page_info(false, None), "nodes": [] } } })),
    )
    .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Partial);
    assert!(!b.complete_collections.contains(&Collection::Timeline));
    assert!(b.complete_collections.contains(&Collection::Comments));
    assert!(
        of_type(&b, EventType::Labeled)
            .iter()
            .all(|e| e.coverage == CoverageStatus::Partial)
    );
    assert!(b.notes.iter().any(|n| n.contains("timelineItems")));
}

#[tokio::test]
async fn graphql_errors_on_a_later_page_are_partial() {
    let server = MockServer::start().await;
    mount_main(&server, pr_response(paged_pr())).await;
    mount_checks(&server, &oid(3)).await;
    mount_page(
        &server,
        "PullRequestTimelinePage",
        "tl-100",
        json!({ "data": { "node": null }, "errors": [{ "type": "NOT_FOUND", "path": ["node"], "message": "Could not resolve to a node" }] }),
    )
    .await;
    mount_page(
        &server,
        "PullRequestCommentsPage",
        "ic-100",
        gql_data(json!({ "node": { "comments": connection(vec![], false, None) } })),
    )
    .await;
    mount_page(
        &server,
        "ReviewThreadCommentsPage",
        "tc-50",
        gql_data(json!({ "node": { "comments": { "totalCount": 50, "pageInfo": page_info(false, None), "nodes": [] } } })),
    )
    .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert!(!b.complete_collections.contains(&Collection::Timeline));
    assert_eq!(b.status, CoverageStatus::Partial);
}

#[tokio::test]
async fn squash_merge_has_merge_event_but_no_merge_commit() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    // The squash commit created on the base branch is not a PR commit.
    pr["commits"] = connection(
        vec![
            pr_commit(
                "acme",
                "widget",
                42,
                &oid(1),
                Some("alice"),
                "2024-03-01T08:00:00Z",
                "2024-03-01T08:00:00Z",
                1,
            ),
            pr_commit(
                "acme",
                "widget",
                42,
                &oid(3),
                Some("alice"),
                "2024-03-02T09:00:00Z",
                "2024-03-02T09:00:00Z",
                1,
            ),
        ],
        false,
        None,
    );
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(of_type(&b, EventType::PrMerged).len(), 1);
    let commits = of_type(&b, EventType::PrCommit);
    assert_eq!(commits.len(), 2);
    assert!(
        commits
            .iter()
            .all(|c| c.meta.is_merge_commit == Some(false))
    );
}

#[tokio::test]
async fn merge_without_timeline_item_comes_from_merge_time() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["timelineItems"] = connection(vec![], false, None);
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    let merged = find(&b, &EventId::pr_merged(PR_ID));
    assert_eq!(merged.occurred_at, ts("2024-03-04T15:30:00Z"));
    assert_eq!(merged.actor.login.as_deref(), Some("bob"));
    assert_eq!(merged.entity_id, PR_ID);
}

#[tokio::test]
async fn unknown_actor_for_deleted_author_and_unlinked_commit() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["author"] = Value::Null;
    pr["mergedBy"] = Value::Null;
    pr["timelineItems"] = connection(
        vec![tl(
            "MergedEvent",
            "ME_9",
            "2024-03-04T15:30:00Z",
            Value::Null,
            json!({}),
        )],
        false,
        None,
    );
    pr["commits"] = connection(
        vec![pr_commit(
            "acme",
            "widget",
            42,
            &oid(3),
            None,
            "2024-03-02T09:00:00Z",
            "2024-03-02T09:00:00Z",
            1,
        )],
        false,
        None,
    );
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert!(b.pull_request.author.is_unknown());
    assert_eq!(b.pull_request.merged_by, None);
    assert!(find(&b, &EventId::pr_created(PR_ID)).actor.is_unknown());
    assert!(find(&b, &EventId::pr_merged(PR_ID)).actor.is_unknown());
    assert!(
        find(&b, &EventId::pr_commit(PR_ID, &oid(3)))
            .actor
            .is_unknown()
    );
}

#[tokio::test]
async fn cross_repository_contribution_belongs_to_the_base_repository() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["author"] = user("outside-contributor");
    let mut base = gql_repo(
        "R_upstream",
        gql_actor("User", "maintainer", "U_maintainer"),
        "tool",
        "PUBLIC",
        true,
    );
    base["pullRequest"] = pr;
    mount_main(&server, gql_data(json!({ "repository": base }))).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/maintainer/tool/commits/{}/check-runs",
            oid(3)
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "total_count": 0, "check_runs": [] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/maintainer/tool/commits/{}/statuses",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("maintainer", "tool", 42)
        .await
        .unwrap();
    assert_eq!(b.repository.id, "R_upstream");
    assert_eq!(b.repository.owner, "maintainer");
    assert_eq!(b.repository.owner_kind, ActorKind::User);
    assert!(b.repository.is_fork);
    assert!(b.events.iter().all(|e| e.repository_id == "R_upstream"));
    assert_eq!(
        b.pull_request.author.login.as_deref(),
        Some("outside-contributor")
    );
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
}

#[tokio::test]
async fn forbidden_checks_leave_only_checks_partial() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["headCommit"]["nodes"][0]["commit"]["checkSuites"] = Value::Null;
    let mut body = pr_response(pr);
    body["errors"] = json!([{
        "type": "FORBIDDEN",
        "path": ["repository", "pullRequest", "headCommit", "nodes", 0, "commit", "checkSuites"],
        "locations": [{ "line": 1, "column": 1 }],
        "message": "Resource not accessible by personal access token"
    }]);
    mount_main(&server, body).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{}/check-runs",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "Resource not accessible by personal access token",
            "documentation_url": "https://docs.github.com/rest/checks/runs",
            "status": "403"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{}/statuses",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(statuses()))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Partial);
    assert_eq!(b.complete_collections.len(), 5);
    assert!(!b.complete_collections.contains(&Collection::Checks));
    assert!(b.notes.iter().any(|n| n.contains("check runs")));
    assert_eq!(of_type(&b, EventType::CommitStatus).len(), 1);
}

#[tokio::test]
async fn paginated_check_runs_are_all_fetched() {
    let server = MockServer::start().await;
    // The query returns the first 100 runs of one suite; the rest are
    // fetched over REST rather than truncated.
    let mut pr = rich_pr();
    let first_page: Vec<Value> = (1..=100)
        .map(|i| {
            json!({ "id": format!("CR_p{i}"), "name": format!("job{i}"), "status": "COMPLETED",
                    "conclusion": "SUCCESS", "startedAt": "2024-03-02T10:00:00Z",
                    "completedAt": "2024-03-02T10:01:00Z", "permalink": "https://github.com/acme/widget/runs/1" })
        })
        .collect();
    pr["headCommit"]["nodes"][0]["commit"]["checkSuites"] = connection(
        vec![json!({ "databaseId": 1, "app": null,
                     "checkRuns": connection(first_page, true, Some("Y3Vyc29yOnYyOpHPAAAAZA==")) })],
        false,
        Some("Y3Vyc29yOnYyOpHPAAAAAQ=="),
    );
    mount_main(&server, pr_response(pr)).await;
    let runs_path = format!("/repos/acme/widget/commits/{}/check-runs", oid(3));
    let mk = |i: u64| {
        json!({ "id": i, "node_id": format!("CR_p{i}"), "name": format!("job{i}"), "status": "completed",
                "conclusion": "success", "started_at": "2024-03-02T10:00:00Z", "completed_at": "2024-03-02T10:01:00Z",
                "html_url": "https://github.com/acme/widget/runs/1", "check_suite": { "id": 1 }, "app": null })
    };
    Mock::given(method("GET"))
        .and(path(runs_path.clone()))
        .and(wiremock::matchers::query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "total_count": 101, "check_runs": [mk(101)] })),
        )
        .mount(&server)
        .await;
    let first: Vec<Value> = (1..=100).map(mk).collect();
    Mock::given(method("GET"))
        .and(path(runs_path.clone()))
        .and(wiremock::matchers::query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "total_count": 101, "check_runs": first }))
                .insert_header(
                    "link",
                    next_link(&server, &format!("{runs_path}?per_page=100&page=2")).as_str(),
                ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/acme/widget/commits/{}/statuses",
            oid(3)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    let runs = of_type(&b, EventType::CheckRun);
    assert_eq!(runs.len(), 101);
    assert!(runs.iter().all(|r| r.actor.is_unknown()));
}

#[tokio::test]
async fn missing_pull_request_is_not_found() {
    let server = MockServer::start().await;
    let mut r = repo();
    r["pullRequest"] = Value::Null;
    mount_main(
        &server,
        json!({
            "data": { "repository": r },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["repository", "pullRequest"],
                "locations": [{ "line": 5, "column": 5 }],
                "message": "Could not resolve to a PullRequest with the number of 4242."
            }]
        }),
    )
    .await;
    let err = client(&server)
        .fetch_pull_request("acme", "widget", 4242)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn pull_request_missing_required_field_is_invalid() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["createdAt"] = Value::Null;
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let err = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidResponse(_)), "{err:?}");
}

#[tokio::test]
async fn null_nodes_and_items_without_ids_are_skipped_as_partial() {
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    pr["comments"]["nodes"].as_array_mut().unwrap().extend([
        Value::Null,
        json!({ "createdAt": "2024-03-01T10:00:00Z", "author": null }),
    ]);
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(of_type(&b, EventType::ConversationComment).len(), 3);
    assert!(!b.complete_collections.contains(&Collection::Comments));
    assert_eq!(b.status, CoverageStatus::Partial);
}

#[tokio::test]
async fn inaccessible_private_repository_is_not_found_not_unauthorized() {
    let server = MockServer::start().await;
    mount_main(
        &server,
        json!({
            "data": { "repository": null },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["repository"],
                "locations": [{ "line": 2, "column": 3 }],
                "message": "Could not resolve to a Repository with the name 'acme/secret'."
            }]
        }),
    )
    .await;
    let err = client(&server)
        .fetch_pull_request("acme", "secret", 1)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    assert!(!err.is_auth());
}

#[tokio::test]
async fn graphql_unauthorized_is_an_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/graphql"
        })))
        .mount(&server)
        .await;
    let err = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap_err();
    assert!(err.is_auth());
    assert_no_token(&err, &err.to_string());
}

// ---------------------------------------------------------------------------
// Issues and repositories
// ---------------------------------------------------------------------------

fn issue_response(item: Value) -> Value {
    let mut r = repo();
    r["issueOrPullRequest"] = item;
    gql_data(json!({ "repository": r }))
}

#[tokio::test]
async fn issue_open_close_and_reopen_history() {
    let server = MockServer::start().await;
    let issue = json!({
        "__typename": "Issue",
        "id": "I_7",
        "number": 7,
        "title": "Widget breaks",
        "url": "https://github.com/acme/widget/issues/7",
        "state": "OPEN",
        "createdAt": "2024-02-01T10:00:00Z",
        "closedAt": null,
        "author": user("carol"),
        "timelineItems": connection(vec![
            tl("ClosedEvent", "CE_7", "2024-02-02T10:00:00Z", bot("github-actions"), json!({ "url": "https://github.com/acme/widget/issues/7#event-1" })),
            tl("ReopenedEvent", "RE_7", "2024-02-03T10:00:00Z", user("carol"), json!({})),
        ], false, Some("x"))
    });
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query IssueDetail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_response(issue)))
        .mount(&server)
        .await;
    let b = client(&server)
        .fetch_issue("acme", "widget", 7)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Complete);
    assert_eq!(b.issue.id, "I_7");
    assert_eq!(b.issue.repository_id, REPO_ID);
    assert!(b.issue.is_open);
    assert_eq!(b.issue.created_at, ts("2024-02-01T10:00:00Z"));
    let types: Vec<_> = b.events.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            EventType::IssueOpened,
            EventType::IssueClosed,
            EventType::IssueReopened
        ]
    );
    assert_eq!(b.events[0].event_id, EventId::issue_created("I_7"));
    assert_eq!(b.events[1].event_id, EventId::timeline("CE_7"));
    assert!(b.events[1].actor.is_bot());
    assert!(b.events.iter().all(|e| e.parent_pr_id.is_none()));
}

#[tokio::test]
async fn issue_fetch_on_a_pull_request_number_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query IssueDetail"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(issue_response(json!({ "__typename": "PullRequest" }))),
        )
        .mount(&server)
        .await;
    let err = client(&server)
        .fetch_issue("acme", "widget", 42)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidResponse(_)), "{err:?}");
    assert!(err.to_string().contains("pull request"));
}

#[tokio::test]
async fn issue_timeline_pages_are_followed() {
    let server = MockServer::start().await;
    let first: Vec<Value> = (0..100)
        .map(|i| {
            tl(
                if i % 2 == 0 {
                    "ClosedEvent"
                } else {
                    "ReopenedEvent"
                },
                &format!("E_{i}"),
                "2024-02-02T10:00:00Z",
                user("carol"),
                json!({}),
            )
        })
        .collect();
    let issue = json!({
        "__typename": "Issue", "id": "I_8", "number": 8, "title": "t",
        "url": "https://github.com/acme/widget/issues/8", "state": "CLOSED",
        "createdAt": "2024-02-01T10:00:00Z", "closedAt": "2024-02-05T10:00:00Z",
        "author": user("carol"), "timelineItems": connection(first, true, Some("i-100"))
    });
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query IssueDetail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_response(issue)))
        .mount(&server)
        .await;
    mount_page(
        &server,
        "IssueTimelinePage",
        "i-100",
        gql_data(json!({ "node": { "timelineItems": connection(vec![
            tl("ClosedEvent", "E_last", "2024-02-05T10:00:00Z", user("carol"), json!({}))
        ], false, Some("i-101")) } })),
    )
    .await;
    let b = client(&server)
        .fetch_issue("acme", "widget", 8)
        .await
        .unwrap();
    assert_eq!(b.events.len(), 102);
    assert!(!b.issue.is_open);
    assert_eq!(b.status, CoverageStatus::Complete);
}

#[tokio::test]
async fn repository_metadata_and_visibility() {
    let server = MockServer::start().await;
    let mut r = gql_repo(
        "R_secret",
        gql_actor("User", "alice", "U_alice"),
        "secret",
        "PRIVATE",
        false,
    );
    r["viewerPermission"] = json!("ADMIN");
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query RepositoryDetail"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(gql_data(json!({ "repository": r }))),
        )
        .mount(&server)
        .await;
    let repo = client(&server).repository("alice", "secret").await.unwrap();
    assert_eq!(repo.id, "R_secret");
    assert_eq!(repo.visibility, Visibility::Private);
    assert_eq!(repo.viewer_permission.as_deref(), Some("ADMIN"));
    assert_eq!(repo.owner_kind, ActorKind::User);
    assert!(repo.accessible);
}

#[tokio::test]
async fn missing_repository_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": null },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["repository"],
                "locations": [{ "line": 2, "column": 3 }],
                "message": "Could not resolve to a Repository with the name 'acme/gone'."
            }]
        })))
        .mount(&server)
        .await;
    let err = client(&server)
        .repository("acme", "gone")
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn saml_protected_repository_is_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": null },
            "errors": [{
                "type": "FORBIDDEN",
                "path": ["repository"],
                "message": "Resource protected by organization SAML enforcement."
            }]
        })))
        .mount(&server)
        .await;
    let err = client(&server).repository("acme", "sso").await.unwrap_err();
    assert!(matches!(err, ApiError::Forbidden(_)), "{err:?}");
    assert!(!err.is_auth());
}

#[tokio::test]
async fn unknown_fields_and_enum_values_are_tolerated() {
    let baseline = fetch_rich().await;
    let server = MockServer::start().await;
    let mut pr = rich_pr();
    // Fields GitHub may add at any level.
    pr["brandNewField"] = json!({ "nested": [1, 2, 3] });
    pr["commits"]["nodes"][0]["commit"]["newCommitField"] = json!("x");
    pr["reviews"]["nodes"][0]["reviewNewField"] = json!(true);
    pr["reviewThreads"]["nodes"][0]["threadNewField"] = json!(null);
    pr["comments"]["nodes"][0]["reactionGroups"] = json!([]);
    // A review in a state this version does not know. It has a submission
    // time, so it is a submitted review whose state is unknown.
    let mut reviews = pr["reviews"]["nodes"].as_array().unwrap().clone();
    reviews.push(review(
        "PRR_6",
        "SOME_FUTURE_STATE",
        "",
        Some("2024-03-03T09:00:00Z"),
        user("erin"),
        vec![],
    ));
    pr["reviews"] = connection(reviews, false, Some("r6"));
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    // Every known collection is still complete.
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    // Every event type keeps its count, except one more submitted review.
    for t in EventType::ALL {
        let expected = of_type(&baseline, t).len() + usize::from(t == EventType::ReviewSubmitted);
        assert_eq!(of_type(&b, t).len(), expected, "{t:?}");
    }
    let r6 = find(&b, &EventId::review("PRR_6"));
    assert_eq!(r6.meta.review_state, None);
    assert_eq!(r6.actor.login.as_deref(), Some("erin"));
    assert_eq!(b.threads.len(), baseline.threads.len());
}

/// A pull request whose commits, reviews and review threads all exceed one
/// page: 250 commits, 120 reviews and 110 threads.
///
/// Pages as GitHub returns them (commits and reviews 100 per page, threads
/// 50 per page):
/// - commits: 100 (cursor `c-100`) + 100 (`c-200`) + 50 = 250
/// - reviews: 100 (`r-100`) + 20 = 120
/// - threads: 50 (`t-50`) + 50 (`t-100`) + 10 = 110
struct BigPr {
    main: Value,
    commit_pages: [Vec<Value>; 3],
    review_page: Vec<Value>,
    thread_pages: [Vec<Value>; 3],
}

fn big_commit(i: u64) -> Value {
    // The last commit is the head commit of the pull request.
    let o = if i == 249 { oid(3) } else { oid(10_000 + i) };
    let t = format!(
        "2024-03-0{}T{:02}:{:02}:00Z",
        1 + i / 100,
        (i / 60) % 24,
        i % 60
    );
    pr_commit("acme", "widget", 42, &o, Some("alice"), &t, &t, 1)
}

fn big_review(i: u64) -> Value {
    let t = format!("2024-03-03T{:02}:{:02}:00Z", i / 60, i % 60);
    // An empty body: each review is one submission and no summary comment.
    review(
        &format!("PRR_big_{i}"),
        "COMMENTED",
        "",
        Some(&t),
        user("bob"),
        vec![],
    )
}

fn big_thread(i: u64) -> Value {
    let t = format!("2024-03-03T{:02}:{:02}:30Z", i / 60, i % 60);
    let c = review_comment(
        &format!("PRRC_big_{i}"),
        user("carol"),
        &t,
        &format!("PRR_big_{}", i % 120),
        None,
        None,
    );
    thread(
        &format!("PRRT_big_{i}"),
        false,
        false,
        None,
        "src/big.rs",
        vec![c],
    )
}

fn big_pr() -> BigPr {
    let commits: Vec<Value> = (0..250).map(big_commit).collect();
    let reviews: Vec<Value> = (0..120).map(big_review).collect();
    let threads: Vec<Value> = (0..110).map(big_thread).collect();
    let mut pr = rich_pr();
    pr["commits"] = connection(commits[0..100].to_vec(), true, Some("c-100"));
    pr["reviews"] = connection(reviews[0..100].to_vec(), true, Some("r-100"));
    pr["reviewThreads"] = connection(threads[0..50].to_vec(), true, Some("t-50"));
    // The dismissal in the timeline refers to a review that is not part of
    // this fixture; it is dropped so only the paged reviews remain.
    let timeline: Vec<Value> = pr["timelineItems"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["__typename"] != "ReviewDismissedEvent")
        .cloned()
        .collect();
    pr["timelineItems"] = connection(timeline, false, Some("tl16"));
    BigPr {
        main: pr_response(pr),
        commit_pages: [
            commits[0..100].to_vec(),
            commits[100..200].to_vec(),
            commits[200..250].to_vec(),
        ],
        review_page: reviews[100..120].to_vec(),
        thread_pages: [
            threads[0..50].to_vec(),
            threads[50..100].to_vec(),
            threads[100..110].to_vec(),
        ],
    }
}

fn node_page(field: &str, nodes: Vec<Value>, next: Option<&str>) -> Value {
    let conn = connection(nodes, next.is_some(), Some(next.unwrap_or("end")));
    gql_data(json!({ "node": { field: conn } }))
}

/// Mounts every follow-up page of `big_pr` except those after the cursors
/// in `skip`.
async fn mount_big_pages(server: &MockServer, big: &BigPr, skip: &[&str]) {
    let pages: Vec<(&str, &str, Value)> = vec![
        (
            "PullRequestCommitsPage",
            "c-100",
            node_page("commits", big.commit_pages[1].clone(), Some("c-200")),
        ),
        (
            "PullRequestCommitsPage",
            "c-200",
            node_page("commits", big.commit_pages[2].clone(), None),
        ),
        (
            "PullRequestReviewsPage",
            "r-100",
            node_page("reviews", big.review_page.clone(), None),
        ),
        (
            "PullRequestReviewThreadsPage",
            "t-50",
            node_page("reviewThreads", big.thread_pages[1].clone(), Some("t-100")),
        ),
        (
            "PullRequestReviewThreadsPage",
            "t-100",
            node_page("reviewThreads", big.thread_pages[2].clone(), None),
        ),
    ];
    for (query, after, body) in pages {
        if !skip.contains(&after) {
            mount_page(server, query, after, body).await;
        }
    }
}

/// The `after` cursors of all follow-up requests, sorted.
async fn follow_up_cursors(server: &MockServer) -> Vec<String> {
    let mut afters: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter_map(|b| b["variables"]["after"].as_str().map(str::to_string))
        .collect();
    afters.sort();
    afters
}

#[tokio::test]
async fn pull_request_commits_reviews_and_threads_beyond_100_are_paginated() {
    let server = MockServer::start().await;
    let big = big_pr();
    assert_eq!(big.thread_pages[0].len(), 50);
    mount_main(&server, big.main.clone()).await;
    mount_checks(&server, &oid(3)).await;
    mount_big_pages(&server, &big, &[]).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(b.complete_collections.len(), Collection::ALL.len());
    // 100 + 100 + 50 commits, each a distinct SHA, the last one the head.
    assert_eq!(of_type(&b, EventType::PrCommit).len(), 250);
    let shas: std::collections::HashSet<_> = b.current_commit_shas.iter().collect();
    assert_eq!(b.current_commit_shas.len(), 250);
    assert_eq!(shas.len(), 250);
    assert!(b.current_commit_shas.contains(&oid(3)));
    // 100 + 20 submitted reviews; none has a body, so no summaries.
    assert_eq!(of_type(&b, EventType::ReviewSubmitted).len(), 120);
    assert_eq!(
        of_type(&b, EventType::ReviewSummary),
        Vec::<&ghusage_core::model::ActivityEvent>::new()
    );
    // 50 + 50 + 10 threads with one inline comment each.
    assert_eq!(b.threads.len(), 110);
    assert_eq!(
        of_type(&b, EventType::InlineComment)
            .iter()
            .filter(|e| e.entity_id.starts_with("PRRC_big_"))
            .count(),
        110
    );
    // Follow-up pages: commits 2, reviews 1, threads 2; each asked once.
    assert_eq!(
        follow_up_cursors(&server).await,
        ["c-100", "c-200", "r-100", "t-100", "t-50"]
    );
}

#[tokio::test]
async fn failing_later_page_of_commits_reviews_or_threads_is_partial() {
    // Each case fails one later page with a persistent server error and
    // expects that collection, and only it, to be incomplete, keeping the
    // items of the pages fetched before it.
    let cases: [(&str, &str, Collection, EventType, &str, usize); 3] = [
        // Only the first page of commits (100) was fetched.
        (
            "c-100",
            "PullRequestCommitsPage",
            Collection::Commits,
            EventType::PrCommit,
            "",
            100,
        ),
        // The 20 reviews of the second page are missing: 100 remain.
        (
            "r-100",
            "PullRequestReviewsPage",
            Collection::Reviews,
            EventType::ReviewSubmitted,
            "PRR_big_",
            100,
        ),
        // Threads pages one and two (50 + 50) were fetched, the third failed.
        (
            "t-100",
            "PullRequestReviewThreadsPage",
            Collection::ReviewComments,
            EventType::InlineComment,
            "PRRC_big_",
            100,
        ),
    ];
    for (after, query, collection, ty, prefix, fetched) in cases {
        let server = MockServer::start().await;
        let big = big_pr();
        mount_main(&server, big.main.clone()).await;
        mount_checks(&server, &oid(3)).await;
        mount_big_pages(&server, &big, &[after]).await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_string_contains(format!("query {query}")))
            .and(body_partial_json(
                json!({ "variables": { "after": after } }),
            ))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let b = client(&server)
            .fetch_pull_request("acme", "widget", 42)
            .await
            .unwrap();
        assert_eq!(b.status, CoverageStatus::Partial, "{after}: {:?}", b.notes);
        assert!(!b.complete_collections.contains(&collection), "{after}");
        assert_eq!(
            b.complete_collections.len(),
            Collection::ALL.len() - 1,
            "{after}: {:?}",
            b.complete_collections
        );
        let events: Vec<_> = of_type(&b, ty)
            .into_iter()
            .filter(|e| e.entity_id.starts_with(prefix))
            .collect();
        assert_eq!(events.len(), fetched, "{after}");
        assert!(
            events.iter().all(|e| e.coverage == CoverageStatus::Partial),
            "{after}"
        );
        // The server error was retried a bounded number of times: the test
        // client allows 3 attempts.
        let tries = follow_up_cursors(&server)
            .await
            .into_iter()
            .filter(|c| c == after)
            .count();
        assert_eq!(tries, 3, "{after}");
    }
}

#[tokio::test]
async fn threads_with_only_pending_comments_are_not_counted() {
    // Thread 1 gets an extra unsubmitted reply; thread 3 holds only an
    // unsubmitted comment, which only its author's token can see.
    let mut pending_reply = review_comment(
        "PRRC_P1",
        user("dave"),
        "2024-03-03T09:00:00Z",
        "PRR_4",
        Some("PRRC_1"),
        None,
    );
    pending_reply["state"] = json!("PENDING");
    let mut pending_only = review_comment(
        "PRRC_P2",
        user("dave"),
        "2024-03-03T09:05:00Z",
        "PRR_4",
        None,
        None,
    );
    pending_only["state"] = json!("PENDING");
    let mut pr = rich_pr();
    let mut t1 = pr["reviewThreads"]["nodes"][0].clone();
    t1["comments"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(pending_reply);
    t1["comments"]["totalCount"] = json!(3);
    pr["reviewThreads"]["nodes"][0] = t1;
    pr["reviewThreads"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(thread(
            "PRRT_3",
            false,
            false,
            None,
            "src/new.rs",
            vec![pending_only],
        ));
    pr["reviewThreads"]["totalCount"] = json!(3);

    let server = MockServer::start().await;
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    let mut ids: Vec<&str> = b.threads.iter().map(|t| t.id.as_str()).collect();
    ids.sort_unstable();
    // Only the two threads with submitted comments.
    assert_eq!(ids, vec!["PRRT_1", "PRRT_2"]);
    let t1 = b.threads.iter().find(|t| t.id == "PRRT_1").unwrap();
    // PRRC_1 and PRRC_2; the pending reply is not counted.
    assert_eq!(t1.comment_count, 2);
    assert_eq!(b.status, CoverageStatus::Complete);
}

#[tokio::test]
async fn pull_request_diff_and_commit_line_counts_are_kept() {
    let mut pr = rich_pr();
    pr["additions"] = json!(120);
    pr["deletions"] = json!(30);
    pr["changedFiles"] = json!(4);
    pr["commits"]["nodes"][0]["commit"]["additions"] = json!(9);
    pr["commits"]["nodes"][0]["commit"]["deletions"] = json!(2);
    let first = pr["commits"]["nodes"][0]["commit"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    let server = MockServer::start().await;
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    let b = client(&server)
        .fetch_pull_request("acme", "widget", 42)
        .await
        .unwrap();
    let d = b.diff.expect("pull request diff");
    assert_eq!((d.additions, d.deletions, d.changed_files), (120, 30, 4));
    // Only the commit GitHub gave counts for has them.
    assert_eq!(b.commit_stats.len(), 1);
    assert_eq!(b.commit_stats[0].0, first);
    assert_eq!(
        (b.commit_stats[0].1.additions, b.commit_stats[0].1.deletions),
        (9, 2)
    );
}

#[tokio::test]
async fn without_counts_from_github_nothing_is_assumed() {
    let b = fetch_rich().await;
    assert_eq!(b.diff, None);
    assert_eq!(b.commit_stats, Vec::new());
}

#[tokio::test]
async fn commit_line_counts_are_fetched_in_one_query_per_batch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "r0": {
                    "c0": { "additions": 5, "deletions": 1 },
                    "c1": null
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let refs: Vec<ghusage_github::CommitRef> = [1, 2]
        .iter()
        .map(|n| ghusage_github::CommitRef {
            owner: "acme".into(),
            name: "widget".into(),
            sha: sha(*n),
        })
        .chain(std::iter::once(ghusage_github::CommitRef {
            owner: "acme".into(),
            name: "bad\"name".into(),
            sha: sha(3),
        }))
        .collect();
    let out = client(&server).commit_line_counts(&refs).await.unwrap();
    let get = |s: &str| {
        out.iter()
            .find(|(c, _)| c.sha == s && c.name == "widget")
            .map(|(_, l)| *l)
    };
    assert_eq!(
        get(&sha(1)).flatten().map(|l| (l.additions, l.deletions)),
        Some((5, 1))
    );
    // Not returned: unknown, not zero.
    assert_eq!(get(&sha(2)), Some(None));
    // An invalid name is never sent and is unknown.
    assert!(
        out.iter()
            .any(|(c, l)| c.name == "bad\"name" && l.is_none())
    );
}

// ---------------------------------------------------------------------------
// Several pull requests in one query
// ---------------------------------------------------------------------------

/// [`rich_pr`] as pull request `number` of acme/widget.
fn numbered_pr(number: u64) -> Value {
    let mut pr = rich_pr();
    pr["id"] = json!(format!("PR_{number}"));
    pr["number"] = json!(number);
    pr["url"] = json!(format!("https://github.com/acme/widget/pull/{number}"));
    pr
}

/// The repository as one alias of the batched query returns it, with `pr`
/// as its pull request.
fn repo_with(pr: Value) -> Value {
    let mut r = repo();
    r["pullRequest"] = pr;
    r
}

async fn mount_batch(server: &MockServer, resp: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestBatch"))
        .respond_with(resp)
        .mount(server)
        .await;
}

/// Mounts the single pull request query for `number`.
async fn mount_single(server: &MockServer, number: u64, pr: Value) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestDetail"))
        .and(body_partial_json(
            json!({ "variables": { "number": number } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_response(pr)))
        .mount(server)
        .await;
}

/// Number of single pull request queries sent for `number`.
async fn single_requests(server: &MockServer, number: u64) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| {
            b["query"]
                .as_str()
                .is_some_and(|q| q.contains("query PullRequestDetail"))
                && b["variables"]["number"] == json!(number)
        })
        .count()
}

/// A bundle without the times of the fetch itself, for comparing two
/// fetches of the same data.
fn timeless(mut b: PrBundle) -> PrBundle {
    let t = ts("2000-01-01T00:00:00Z");
    for e in &mut b.events {
        e.observed_at = t;
    }
    b.pull_request.synced_at = None;
    if let Some(d) = &mut b.diff {
        d.observed_at = t;
    }
    b
}

/// The bundle of `pr` fetched on its own with the single query.
async fn fetched_alone(pr: Value) -> PrBundle {
    let server = MockServer::start().await;
    let number = pr["number"].as_u64().unwrap();
    mount_main(&server, pr_response(pr)).await;
    mount_checks(&server, &oid(3)).await;
    timeless(
        client(&server)
            .fetch_pull_request("acme", "widget", number)
            .await
            .unwrap(),
    )
}

fn keys(numbers: &[u64]) -> Vec<(&'static str, &'static str, u64)> {
    numbers.iter().map(|n| ("acme", "widget", *n)).collect()
}

#[tokio::test]
async fn batched_pull_requests_match_single_fetches() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(gql_data(json!({
            "r0": repo_with(numbered_pr(41)),
            "r1": repo_with(numbered_pr(43)),
        }))),
    )
    .await;
    mount_checks(&server, &oid(3)).await;
    let out = client(&server).fetch_pull_requests(&keys(&[41, 43])).await;
    assert_eq!(out.len(), 2);
    let mut out = out.into_iter();
    let a = timeless(out.next().unwrap().unwrap());
    let b = timeless(out.next().unwrap().unwrap());
    assert_eq!(a, fetched_alone(numbered_pr(41)).await);
    assert_eq!(b, fetched_alone(numbered_pr(43)).await);
    // One query asked for both, by number.
    let reqs = server.received_requests().await.unwrap();
    let batches: Vec<Value> = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| {
            b["query"]
                .as_str()
                .is_some_and(|q| q.contains("query PullRequestBatch"))
        })
        .collect();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0]["variables"],
        json!({ "o0": "acme", "n0": "widget", "k0": 41, "o1": "acme", "n1": "widget", "k1": 43 })
    );
    assert_eq!(single_requests(&server, 41).await, 0);
}

#[tokio::test]
async fn missing_pull_request_in_a_batch_is_not_found_alone() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "r0": repo_with(numbered_pr(41)),
                "r1": repo_with(Value::Null),
                "r2": repo_with(numbered_pr(43)),
            },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["r1", "pullRequest"],
                "locations": [{ "line": 5, "column": 5 }],
                "message": "Could not resolve to a PullRequest with the number of 4242."
            }]
        })),
    )
    .await;
    mount_checks(&server, &oid(3)).await;
    let out = client(&server)
        .fetch_pull_requests(&keys(&[41, 4242, 43]))
        .await;
    assert_eq!(out.len(), 3);
    let mut out = out.into_iter();
    let a = timeless(out.next().unwrap().unwrap());
    let missing = out.next().unwrap().unwrap_err();
    let c = timeless(out.next().unwrap().unwrap());
    assert!(matches!(missing, ApiError::NotFound(_)), "{missing:?}");
    assert_eq!(a.status, CoverageStatus::Complete, "{:?}", a.notes);
    assert_eq!(a, fetched_alone(numbered_pr(41)).await);
    assert_eq!(c, fetched_alone(numbered_pr(43)).await);
    // The error belonged to one pull request: nothing was asked again.
    for n in [41, 4242, 43] {
        assert_eq!(single_requests(&server, n).await, 0);
    }
}

#[tokio::test]
async fn missing_repository_in_a_batch_is_not_found_alone() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "r0": null, "r1": repo_with(numbered_pr(43)) },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["r0"],
                "locations": [{ "line": 2, "column": 3 }],
                "message": "Could not resolve to a Repository with the name 'acme/secret'."
            }]
        })),
    )
    .await;
    mount_checks(&server, &oid(3)).await;
    let out = client(&server)
        .fetch_pull_requests(&[("acme", "secret", 1), ("acme", "widget", 43)])
        .await;
    let mut out = out.into_iter();
    let err = out.next().unwrap().unwrap_err();
    assert!(matches!(err, ApiError::NotFound(_)), "{err:?}");
    assert!(!err.is_auth());
    let b = out.next().unwrap().unwrap();
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
}

#[tokio::test]
async fn errors_in_a_batch_make_only_their_pull_request_partial() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "r0": repo_with(numbered_pr(41)),
                "r1": repo_with(numbered_pr(43)),
            },
            "errors": [{
                "type": "SOMETHING_WENT_WRONG",
                "path": ["r0", "pullRequest", "reviews", "nodes", 1, "author"],
                "locations": [{ "line": 9, "column": 7 }],
                "message": "Something went wrong while executing your query."
            }]
        })),
    )
    .await;
    mount_checks(&server, &oid(3)).await;
    let out = client(&server).fetch_pull_requests(&keys(&[41, 43])).await;
    let mut out = out.into_iter();
    let a = out.next().unwrap().unwrap();
    let b = out.next().unwrap().unwrap();
    assert_eq!(a.status, CoverageStatus::Partial);
    assert!(!a.complete_collections.contains(&Collection::Reviews));
    assert!(a.complete_collections.contains(&Collection::Commits));
    assert!(a.notes.iter().any(|n| n.contains("Something went wrong")));
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(b.complete_collections.len(), Collection::ALL.len());
}

/// Responses that cannot be attributed to single pull requests: every pull
/// request of the batch is fetched again on its own.
#[tokio::test]
async fn unattributable_batch_results_are_fetched_again_one_by_one() {
    let cases: Vec<(&str, ResponseTemplate)> = vec![
        (
            "error without a path",
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "r0": repo_with(numbered_pr(41)),
                    "r1": repo_with(numbered_pr(43)),
                },
                "errors": [{ "message": "Something went wrong while executing your query." }]
            })),
        ),
        (
            "error on an unknown alias",
            ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "r0": repo_with(numbered_pr(41)),
                    "r1": repo_with(numbered_pr(43)),
                },
                "errors": [{ "type": "SOMETHING_WENT_WRONG", "path": ["r7", "pullRequest"], "message": "?" }]
            })),
        ),
        ("server error", ResponseTemplate::new(502)),
        ("no data", ResponseTemplate::new(200).set_body_json(json!({}))),
    ];
    let alone = [
        fetched_alone(numbered_pr(41)).await,
        fetched_alone(numbered_pr(43)).await,
    ];
    for (what, resp) in cases {
        let server = MockServer::start().await;
        mount_batch(&server, resp).await;
        mount_single(&server, 41, numbered_pr(41)).await;
        mount_single(&server, 43, numbered_pr(43)).await;
        mount_checks(&server, &oid(3)).await;
        let out: Vec<PrBundle> = client(&server)
            .fetch_pull_requests(&keys(&[41, 43]))
            .await
            .into_iter()
            .map(|r| timeless(r.unwrap()))
            .collect();
        assert_eq!(out, alone, "{what}");
        assert_eq!(single_requests(&server, 41).await, 1, "{what}");
        assert_eq!(single_requests(&server, 43).await, 1, "{what}");
    }
}

#[tokio::test]
async fn pull_request_left_out_of_a_batch_is_fetched_again_alone() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(gql_data(json!({
            "r0": repo_with(numbered_pr(41)),
        }))),
    )
    .await;
    mount_single(&server, 43, numbered_pr(43)).await;
    mount_checks(&server, &oid(3)).await;
    let out: Vec<PrBundle> = client(&server)
        .fetch_pull_requests(&keys(&[41, 43]))
        .await
        .into_iter()
        .map(|r| timeless(r.unwrap()))
        .collect();
    assert_eq!(out[0], fetched_alone(numbered_pr(41)).await);
    assert_eq!(out[1], fetched_alone(numbered_pr(43)).await);
    assert_eq!(single_requests(&server, 41).await, 0);
    assert_eq!(single_requests(&server, 43).await, 1);
}

#[tokio::test]
async fn rejected_credentials_fail_the_whole_batch_without_retrying() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials",
            "documentation_url": "https://docs.github.com/graphql"
        })))
        .mount(&server)
        .await;
    let out = client(&server).fetch_pull_requests(&keys(&[41, 43])).await;
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|r| r.as_ref().unwrap_err().is_auth()));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn later_pages_of_a_batched_pull_request_are_fetched() {
    let server = MockServer::start().await;
    mount_batch(
        &server,
        ResponseTemplate::new(200).set_body_json(gql_data(json!({
            "r0": repo_with(numbered_pr(41)),
            "r1": repo_with(paged_pr()),
        }))),
    )
    .await;
    mount_checks(&server, &oid(3)).await;
    let tl2: Vec<Value> = (0..20)
        .map(|i| {
            tl(
                "LabeledEvent",
                &format!("LE_p2_{i}"),
                "2024-03-01T13:00:00Z",
                user("alice"),
                json!({ "label": { "name": "x" } }),
            )
        })
        .collect();
    mount_page(
        &server,
        "PullRequestTimelinePage",
        "tl-100",
        gql_data(json!({ "node": { "timelineItems": connection(tl2, false, Some("tl-120")) } })),
    )
    .await;
    let ic2: Vec<Value> = (0..50)
        .map(|i| {
            issue_comment(
                &format!("IC_p2_{i}"),
                user("bob"),
                "2024-03-01T13:00:00Z",
                None,
            )
        })
        .collect();
    mount_page(
        &server,
        "PullRequestCommentsPage",
        "ic-100",
        gql_data(json!({ "node": { "comments": connection(ic2, false, Some("ic-150")) } })),
    )
    .await;
    let tc2: Vec<Value> = (0..10)
        .map(|i| {
            review_comment(
                &format!("TC_p2_{i}"),
                user("carol"),
                "2024-03-02T13:00:00Z",
                "PRR_2",
                None,
                None,
            )
        })
        .collect();
    mount_page(
        &server,
        "ReviewThreadCommentsPage",
        "tc-50",
        gql_data(json!({ "node": { "comments": { "totalCount": 60, "pageInfo": page_info(false, Some("tc-60")), "nodes": tc2 } } })),
    )
    .await;
    let out = client(&server).fetch_pull_requests(&keys(&[41, 42])).await;
    let mut out = out.into_iter();
    let small = out.next().unwrap().unwrap();
    let b = out.next().unwrap().unwrap();
    assert_eq!(small.status, CoverageStatus::Complete, "{:?}", small.notes);
    assert_eq!(b.status, CoverageStatus::Complete, "{:?}", b.notes);
    assert_eq!(of_type(&b, EventType::Labeled).len(), 120);
    assert_eq!(of_type(&b, EventType::ConversationComment).len(), 150);
    assert_eq!(b.threads[0].comment_count, 60);
    // The later pages were asked for by the paged pull request's node id.
    let reqs = server.received_requests().await.unwrap();
    let followups: Vec<Value> = reqs
        .iter()
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(|b| b["variables"]["after"].is_string())
        .collect();
    assert_eq!(followups.len(), 3);
    assert!(
        followups
            .iter()
            .any(|b| b["variables"]["id"] == json!(PR_ID))
    );
}
