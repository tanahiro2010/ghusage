//! End-to-end tests of the built binary against a mocked GitHub API:
//! initial sync, offline reports, incremental resync, the pull request
//! timeline and terminal-safe rendering of untrusted content.
//!
//! The fixture is described in `support::World::base`. Expected numbers
//! below are derived by hand from that description.

mod support;

use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{
    body_partial_json, body_string_contains, method, path, query_param, query_param_contains,
    query_param_is_missing,
};
use wiremock::{Mock, ResponseTemplate};

/// Every report command, as offline JSON over the standard period.
const REPORTS: [&str; 7] = [
    "overview", "daily", "prs", "reviews", "comments", "issues", "repos",
];

#[tokio::test(flavor = "multi_thread")]
async fn initial_sync_then_offline_reports_match_the_fixture() {
    let world = World::base();
    let server = serve(&world).await;
    let env = Env::new(&server);

    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "complete");
    // Commit search returns 5 items (including the bot's and the unlinked one).
    assert_eq!(s["commits_found"], 5);
    assert_eq!(s["pull_requests_found"], 3);
    assert_eq!(s["pull_requests_fetched"], 3);
    assert_eq!(s["issues_fetched"], 2);
    // Head commit check runs come with the pull request query, and no head
    // commit has commit statuses, so no REST checks request is needed.
    let reqs = requests(&server).await;
    assert!(
        !reqs.iter().any(|r| {
            let p = r.url.path();
            p.ends_with("/check-runs") || p.ends_with("/statuses")
        }),
        "{:?}",
        reqs.iter().map(|r| r.url.path()).collect::<Vec<_>>()
    );

    let before = requests(&server).await.len();

    // --- overview ---
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0).assert_clean();
    let o = o.json();
    assert_eq!(o["account"]["login"], ME);
    assert_eq!(o["period"]["start"], "2024-03-04T00:00:00+00:00");
    // --until is inclusive of the whole named day.
    assert_eq!(o["period"]["end"], "2024-03-11T00:00:00+00:00");
    let m = &o["metrics"];
    // Search: sha 1, 2, 3 (sha 4 is a bot's, sha 5 has no user) plus my
    // pull request commits sha 11, 12 (PR A) and 21 (PR B) = 6.
    assert_eq!(m["commits"], 6);
    // sha 2 has two parents.
    assert_eq!(m["merge_commits"], 1);
    // PR A and PR B are mine.
    assert_eq!(m["prs_created"], 2);
    // PR B merged 03-06 (by me).
    assert_eq!(m["prs_merged"], 1);
    assert_eq!(m["merges_performed"], 1);
    // PR B's close is part of its merge.
    assert_eq!(m["prs_closed_without_merge"], 0);
    // PRR_C1 counts; my PRR_A3 only replies in carol's thread.
    assert_eq!(
        m["reviews_submitted"],
        json!({"total": 1, "approved": 0, "changes_requested": 1, "commented": 0, "dismissed_unknown_state": 0})
    );
    assert_eq!(m["reviewed_prs"], 1);
    // bob APPROVED and carol CHANGES_REQUESTED on PR A.
    assert_eq!(
        m["reviews_received"],
        json!({"total": 2, "approved": 1, "changes_requested": 1, "commented": 0, "dismissed_unknown_state": 0})
    );
    // Written: IC_A1; inline PRRC_A2 + PRRC_C1; summary of PRR_C1.
    assert_eq!(
        m["comments_written"],
        json!({"total": 4, "conversation": 1, "inline": 2, "review_summary": 1})
    );
    // Received on my PRs: IC_A2 (bob) + IC_A3 (bot); inline PRRC_A1;
    // summary of carol's PRR_A2.
    assert_eq!(
        m["comments_received"],
        json!({"total": 4, "conversation": 2, "inline": 1, "review_summary": 1})
    );
    // PRRC_A2 replies in a thread carol started on my PR.
    assert_eq!(m["review_responses"], 1);
    // sha 11, 12, 21 (bot sha 13, unlinked sha 14 and bob's sha 31 excluded).
    assert_eq!(m["pr_commits"], 3);
    assert_eq!(m["force_pushes"], 0);
    // I_1 opened by me; closed: I_1 (03-07) and I_2 (03-06); bob reopened I_2.
    assert_eq!(m["issues_opened"], 1);
    assert_eq!(m["issues_closed"], 2);
    assert_eq!(m["issues_reopened"], 0);
    assert_eq!(m["active_prs"], 3);
    assert_eq!(m["ci"]["passed"], 1);
    // 03-04, 03-05, 03-06, 03-07.
    assert_eq!(m["active_days"], 4);
    assert_eq!(m["last_activity"], "2024-03-07T23:30:00Z");
    for kind in ["commits", "pull_requests", "issues"] {
        assert_eq!(coverage_status(&o, kind), "complete", "{kind}");
    }

    // --- daily ---
    let d = env.offline_json(&["daily"]).await;
    d.expect_code(0);
    let d = d.json();
    assert_eq!(d["activity_breakdown"].as_array().unwrap().len(), 7);
    // (date, commits, pr_commits, prs_merged)
    // 03-04: sha 1, sha 11 | 03-05: sha 2, sha 12 | 03-06: sha 21, PR B merged
    // 03-07: sha 3
    for (date, commits, pr_commits, merged) in [
        ("2024-03-04", 2, 1, 0),
        ("2024-03-05", 2, 1, 0),
        ("2024-03-06", 1, 1, 1),
        ("2024-03-07", 1, 0, 0),
        ("2024-03-08", 0, 0, 0),
        ("2024-03-09", 0, 0, 0),
        ("2024-03-10", 0, 0, 0),
    ] {
        let b = &bucket(&d, date)["summary"];
        assert_eq!(b["commits"], commits, "{date} commits");
        assert_eq!(b["pr_commits"], pr_commits, "{date} pr_commits");
        assert_eq!(b["prs_merged"], merged, "{date} prs_merged");
    }
    let total: u64 = d["activity_breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["summary"]["commits"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 6, "daily totals add up to the period total");

    // --- prs ---
    let p = env.offline_json(&["prs"]).await;
    p.expect_code(0);
    let p = p.json();
    assert_eq!(pr_numbers(&p), vec![3, 10, 20]);
    let row = |n: u64| -> Value {
        p["pull_requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["number"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(row(10)["state"], "open");
    assert_eq!(row(3)["state"], "merged");
    assert_eq!(row(3)["merged_at"], "2024-03-06T09:00:00Z");
    assert_eq!(row(20)["author"], "bob");

    // --- reviews ---
    let rv = env.offline_json(&["reviews"]).await;
    rv.expect_code(0);
    let rv = rv.json();
    let ids = |k: &str| -> Vec<String> {
        rv[k]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["event_id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids("reviews_submitted"), vec!["review:PRR_C1"]);
    // Sorted by submission time: carol 03-05, bob 03-06.
    assert_eq!(
        ids("reviews_received"),
        vec!["review:PRR_A2", "review:PRR_A1"]
    );

    // --- comments ---
    let c = env.offline_json(&["comments"]).await;
    c.expect_code(0);
    let c = c.json();
    let rows = c["comments"].as_array().unwrap();
    let mine: Vec<&str> = rows
        .iter()
        .filter(|r| r["written_by_subject"] == true)
        .map(|r| r["event_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        mine,
        vec![
            "comment:IC_A1",
            "comment:PRRC_A2",
            "comment:PRRC_C1",
            "review-summary:PRR_C1"
        ]
    );
    // Received: IC_A2, IC_A3, PRRC_A1 and carol's review summary.
    assert_eq!(rows.len(), 8);

    // --- repos ---
    let rp = env.offline_json(&["repos"]).await;
    rp.expect_code(0);
    let rp = rp.json();
    let repos: Vec<(String, bool, u64)> = rp["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["full_name"].as_str().unwrap().to_string(),
                r["owned"].as_bool().unwrap(),
                r["summary"]["commits"].as_u64().unwrap(),
            )
        })
        .collect();
    // widget: sha 1, 11, 12; dotfiles: sha 2, 3, 21.
    assert_eq!(
        repos,
        vec![
            ("acme/widget".to_string(), false, 3),
            ("me/dotfiles".to_string(), true, 3)
        ]
    );

    // --- every report command, and offline never talks to GitHub ---
    for cmd in REPORTS {
        let r = env.offline_json(&[cmd]).await;
        r.expect_code(0).assert_clean();
        r.json();
        let table = env
            .run(&[cmd, "--since", SINCE, "--until", UNTIL, "--offline"])
            .await;
        table.expect_code(0).assert_clean();
    }
    let pr = env
        .run(&[
            "pr",
            "https://github.com/acme/widget/pull/10",
            "--offline",
            "--format",
            "json",
        ])
        .await;
    pr.expect_code(0);
    assert_eq!(pr.json()["number"], 10);
    assert_eq!(
        requests(&server).await.len(),
        before,
        "offline commands must not contact GitHub"
    );
}

#[tokio::test(flavor = "multi_thread")]

async fn issues_report_lists_issues_closed_in_the_period() {
    let world = World::base();
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let r = env.offline_json(&["issues"]).await;
    r.expect_code(0);
    let r = r.json();
    let rows: Vec<(u64, bool, u64, u64)> = r["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["number"].as_u64().unwrap(),
                i["opened_in_period"].as_bool().unwrap(),
                i["closed_in_period"].as_u64().unwrap(),
                i["reopened_in_period"].as_u64().unwrap(),
            )
        })
        .collect();
    // acme/widget#7: closed by me 03-06 (bob's reopen is not mine);
    // me/dotfiles#5: opened 03-05 and closed 03-07 by me.
    assert_eq!(rows, vec![(7, false, 1, 0), (5, true, 1, 0)]);
}

/// The base world after more activity on PR A (acme/widget#10):
/// - force push on 03-08 10:00 by me replaces sha 12 with sha 15 (me,
///   03-08 10:00), which is the new head
/// - dave APPROVED on 03-08 09:00; bob commented IC_A4 on 03-08 12:00
/// - bob's IC_A2 was edited on 03-08 13:00; my IC_A1 was deleted
/// - bob merged it on 03-09 10:00 (merge and close at the same instant)
fn world_after_merge() -> World {
    let mut w = World::base();
    let a = w.pr_mut(10);
    a.updated_at = "2024-03-09T10:00:00Z".into();
    let g = &mut a.gql;
    g["state"] = json!("MERGED");
    g["updatedAt"] = json!("2024-03-09T10:00:00Z");
    g["mergedAt"] = json!("2024-03-09T10:00:00Z");
    g["closedAt"] = json!("2024-03-09T10:00:00Z");
    g["mergedBy"] = user("bob");
    g["headRefOid"] = json!(sha(15));
    g["headCommit"]["nodes"][0]["commit"]["oid"] = json!(sha(15));
    // The merge puts my sha 11 on the default branch of acme/widget.
    let wr = widget();
    w.repo_mut("acme", "widget").default_branch.insert(
        0,
        list_commit(
            &wr.rest,
            &sha(11),
            Some((ME, ME_ID, "User")),
            "2024-03-04T08:30:00Z",
            1,
        ),
    );
    let a = w.pr_mut(10);
    let g = &mut a.gql;
    g["commits"] = connection(vec![
        pr_commit(
            "acme/widget",
            10,
            &sha(11),
            Some((ME, ME_ID)),
            "2024-03-04T08:30:00Z",
        ),
        pr_commit(
            "acme/widget",
            10,
            &sha(13),
            Some(("dependabot[bot]", "BOT_dependabot")),
            "2024-03-05T10:00:00Z",
        ),
        pr_commit("acme/widget", 10, &sha(14), None, "2024-03-05T11:00:00Z"),
        pr_commit(
            "acme/widget",
            10,
            &sha(15),
            Some((ME, ME_ID)),
            "2024-03-08T10:00:00Z",
        ),
    ]);
    g["reviews"]["nodes"].as_array_mut().unwrap().push(review(
        "PRR_A4",
        "APPROVED",
        "",
        "2024-03-08T09:00:00Z",
        user("dave"),
        vec![],
    ));
    g["comments"] = connection(vec![
        issue_comment(
            "IC_A2",
            user("bob"),
            "2024-03-04T11:00:00Z",
            Some("2024-03-08T13:00:00Z"),
        ),
        issue_comment("IC_A3", bot("github-actions"), "2024-03-04T11:05:00Z", None),
        issue_comment("IC_A4", user("bob"), "2024-03-08T12:00:00Z", None),
    ]);
    let tl_items = g["timelineItems"]["nodes"].as_array_mut().unwrap();
    tl_items.push(tl(
        "HeadRefForcePushedEvent",
        "HRF_A",
        "2024-03-08T10:00:00Z",
        user(ME),
        json!({ "beforeCommit": { "oid": sha(12) }, "afterCommit": { "oid": sha(15) } }),
    ));
    tl_items.push(tl(
        "MergedEvent",
        "ME_A",
        "2024-03-09T10:00:00Z",
        user("bob"),
        json!({ "url": "https://github.com/acme/widget/pull/10#event-9" }),
    ));
    tl_items.push(tl(
        "ClosedEvent",
        "CE_A",
        "2024-03-09T10:00:00Z",
        user("bob"),
        json!({ "url": "https://github.com/acme/widget/pull/10#event-10" }),
    ));
    a.check_runs = vec![check_run(
        "CR_A2",
        &sha(15),
        "build",
        "success",
        "2024-03-08T10:30:00Z",
    )];
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn incremental_sync_refetches_only_changed_pull_requests() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);

    // First sync covers 03-04 through 03-07 while PR A is still open.
    let r = env
        .run(&[
            "sync",
            "--since",
            SINCE,
            "--until",
            "2024-03-07",
            "--format",
            "json",
        ])
        .await;
    r.expect_code(0);
    assert_eq!(r.json()["pull_requests_fetched"], 3);
    let early = env
        .run(&[
            "prs",
            "--state",
            "merged",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--offline",
            "--format",
            "json",
        ])
        .await;
    assert_eq!(pr_numbers(&early.json()), vec![3]);

    // GitHub moves on; the second sync extends the period to 03-10.
    reserve(&server, &world_after_merge()).await;
    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["pull_requests_found"], 3);
    // Only PR A's updated_at changed.
    assert_eq!(s["pull_requests_fetched"], 1, "{s}");
    assert_eq!(s["pull_requests_unchanged"], 2, "{s}");
    // Neither issue changed, so neither is fetched again.
    assert_eq!(s["issues_found"], 2, "{s}");
    assert_eq!(s["issues_fetched"], 0, "{s}");
    assert_eq!(s["issues_unchanged"], 2, "{s}");
    let reqs = requests(&server).await;
    assert_eq!(pr_detail_requests(&reqs, 10), 1);
    assert_eq!(pr_detail_requests(&reqs, 3), 0);
    assert_eq!(pr_detail_requests(&reqs, 20), 0);

    // A pull request merged after it was first synced is now merged.
    let merged = env
        .run(&[
            "prs",
            "--state",
            "merged",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--offline",
            "--format",
            "json",
        ])
        .await;
    merged.expect_code(0);
    assert_eq!(pr_numbers(&merged.json()), vec![3, 10]);

    // ...and counts as merged on the merge day, not the creation day.
    let d = env.offline_json(&["daily"]).await.json();
    assert_eq!(bucket(&d, "2024-03-04")["summary"]["prs_merged"], 0);
    assert_eq!(bucket(&d, "2024-03-06")["summary"]["prs_merged"], 1);
    assert_eq!(bucket(&d, "2024-03-09")["summary"]["prs_merged"], 1);
    // Force push: sha 12 (03-05) is gone from the PR, sha 15 (03-08) is in.
    assert_eq!(bucket(&d, "2024-03-05")["summary"]["pr_commits"], 0);
    assert_eq!(bucket(&d, "2024-03-08")["summary"]["pr_commits"], 1);

    let o = env.offline_json(&["overview"]).await.json();
    let m = &o["metrics"];
    assert_eq!(m["prs_merged"], 2);
    // bob, carol, and now dave.
    assert_eq!(m["reviews_received"]["total"], 3);
    assert_eq!(m["reviews_received"]["approved"], 2);
    // My only conversation comment (IC_A1) was deleted.
    assert_eq!(m["comments_written"]["conversation"], 0);
    // Received conversation: IC_A2, IC_A3, IC_A4.
    assert_eq!(m["comments_received"]["conversation"], 3);
    // sha 11, 15 (PR A) and 21 (PR B).
    assert_eq!(m["pr_commits"], 3);
    assert_eq!(m["force_pushes"], 1);

    let c = env.offline_json(&["comments"]).await.json();
    let rows = c["comments"].as_array().unwrap();
    let find = |id: &str| rows.iter().find(|r| r["event_id"] == id).cloned();
    assert!(find("comment:IC_A1").is_none(), "deleted comment listed");
    assert_eq!(find("comment:IC_A2").unwrap()["edited"], true);
    assert_eq!(find("comment:IC_A4").unwrap()["edited"], false);

    let pr = env
        .run(&[
            "pr",
            "https://github.com/acme/widget/pull/10",
            "--offline",
            "--format",
            "json",
        ])
        .await;
    pr.expect_code(0);
    let pr = pr.json();
    assert_eq!(pr["state"], "merged");
    assert_eq!(pr["merged_by"], "bob");
    // Current commits: 11, 13, 14, 15; sha 12 was force-pushed away.
    assert_eq!(pr["commits"], 4);
    assert_eq!(pr["removed_commits"], 1);

    // Re-fetching everything again never changes the numbers.
    let again = env.sync(&["--full"]).await;
    again.expect_code(0);
    // --full refetches unchanged issues too.
    assert_eq!(again.json()["issues_fetched"], 2);
    let o2 = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"], o2["metrics"]);
}

fn timeline_ids(r: &Value) -> Vec<String> {
    r["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn pr_timeline_is_sorted_deterministic_and_filterable() {
    let server = serve(&world_after_merge()).await;
    let env = Env::new(&server);
    let url = "https://github.com/acme/widget/pull/10";

    let first = env
        .run(&["pr", url, "--timeline", "--format", "json"])
        .await;
    first.expect_code(0).assert_clean();
    let t1 = first.json();
    assert_eq!(t1["number"], 10);
    let entries = t1["timeline"].as_array().unwrap();
    assert!(entries.len() >= 15, "{t1}");
    let times: Vec<chrono::DateTime<chrono::FixedOffset>> = entries
        .iter()
        .map(|e| {
            let s = e["occurred_at"].as_str().unwrap();
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap_or_else(|_| panic!("not an ISO 8601 timestamp: {s}"))
        })
        .collect();
    assert!(
        times.windows(2).all(|w| w[0] <= w[1]),
        "timeline not sorted by time: {times:?}"
    );
    // Ties exist (merge and close at 03-09 10:00; my reply and its review at
    // 03-05 15:00) and their order must be stable across runs.
    assert!(times.windows(2).any(|w| w[0] == w[1]));
    let ids = timeline_ids(&t1);
    let merged = ids.iter().position(|i| i.contains("merged")).unwrap();
    let closed = ids.iter().position(|i| i.contains("CE_A")).unwrap();
    assert_eq!(times[merged], times[closed]);

    for _ in 0..2 {
        let again = env
            .run(&["pr", url, "--timeline", "--format", "json", "--offline"])
            .await;
        again.expect_code(0);
        assert_eq!(timeline_ids(&again.json()), ids);
    }

    // Event type filter.
    let f = env
        .run(&[
            "pr",
            url,
            "--timeline",
            "--format",
            "json",
            "--offline",
            "--event-type",
            "review_submitted",
        ])
        .await;
    f.expect_code(0);
    let f = f.json();
    let types: Vec<&str> = f["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_type"].as_str().unwrap())
        .collect();
    assert!(!types.is_empty());
    assert!(types.iter().all(|t| *t == "review_submitted"), "{types:?}");
    let reviews: Vec<String> = timeline_ids(&f);
    for id in ["review:PRR_A1", "review:PRR_A2", "review:PRR_A4"] {
        assert!(
            reviews.contains(&id.to_string()),
            "{id} missing: {reviews:?}"
        );
    }

    let two = env
        .run(&[
            "pr",
            url,
            "--timeline",
            "--format",
            "json",
            "--offline",
            "--event-type",
            "review_submitted,inline_comment",
        ])
        .await
        .json();
    let mut kinds: Vec<String> = two["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_type"].as_str().unwrap().to_string())
        .collect();
    kinds.sort();
    kinds.dedup();
    assert_eq!(kinds, vec!["inline_comment", "review_submitted"]);

    // Unknown event types are rejected as invalid arguments.
    env.run(&["pr", url, "--offline", "--event-type", "nonsense"])
        .await
        .expect_code(2);

    // Table timeline renders too.
    env.run(&["pr", url, "--timeline", "--offline"])
        .await
        .expect_code(0)
        .assert_clean();
}

#[tokio::test(flavor = "multi_thread")]
async fn bot_and_unknown_actor_commits_are_not_counted_as_mine() {
    // Only commits by a bot and by an unlinked git identity, both in commit
    // search and inside a pull request I opened.
    let mut world = World::base();
    let w = widget();
    world.commits = vec![
        search_commit(
            &w.rest,
            &sha(4),
            Some(("dependabot[bot]", "BOT_dependabot", "Bot")),
            "2024-03-05T08:00:00Z",
            1,
        ),
        search_commit(&w.rest, &sha(5), None, "2024-03-06T08:00:00Z", 1),
    ];
    // No default branch has a commit of mine either.
    for r in &mut world.repos {
        r.default_branch.clear();
    }
    world.prs = vec![pr_a()];
    world.pr_mut(10).gql["commits"] = connection(vec![
        pr_commit(
            "acme/widget",
            10,
            &sha(13),
            Some(("dependabot[bot]", "BOT_dependabot")),
            "2024-03-05T10:00:00Z",
        ),
        pr_commit("acme/widget", 10, &sha(14), None, "2024-03-05T11:00:00Z"),
    ]);
    world.issues = vec![];
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["commits"], 0);
    assert_eq!(o["metrics"]["pr_commits"], 0);
    // The pull request itself still has both commits.
    let p = env.offline_json(&["prs"]).await.json();
    assert_eq!(p["pull_requests"][0]["commits"], 2);
}

/// Terminal control: ESC, BEL, C1 CSI, bidi overrides, line separators.
const EVIL: &str =
    "Fix \u{1b}[31mred\u{1b}[0m \u{1b}]0;owned\u{7} \u{202e}desrever\u{202c} \u{9b}2J end\u{2028}x";

fn has_unsafe_chars(s: &str) -> Option<char> {
    s.chars().find(|c| {
        (c.is_control() && *c != '\n')
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}')
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn untrusted_text_is_sanitized_in_tables_and_escaped_in_json() {
    let mut world = World::base();
    {
        let a = world.pr_mut(10);
        a.gql["title"] = json!(EVIL);
        let items = a.gql["timelineItems"]["nodes"].as_array_mut().unwrap();
        items.push(tl(
            "LabeledEvent",
            "LE_A",
            "2024-03-04T09:06:00Z",
            user(ME),
            json!({ "label": { "name": format!("lbl{EVIL}") } }),
        ));
        items.push(tl(
            "RenamedTitleEvent",
            "RT_A",
            "2024-03-04T09:07:00Z",
            user(ME),
            json!({ "previousTitle": format!("old{EVIL}"), "currentTitle": EVIL }),
        ));
    }
    world.issues[0].gql["title"] = json!(EVIL);
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    for cmd in REPORTS {
        let r = env
            .run(&[cmd, "--since", SINCE, "--until", UNTIL, "--offline"])
            .await;
        r.expect_code(0).assert_clean();
        if let Some(c) = has_unsafe_chars(&r.stdout) {
            panic!(
                "{cmd}: unsafe character {c:?} in table output:\n{}",
                r.stdout
            );
        }
    }
    let pr = env
        .run(&[
            "pr",
            "https://github.com/acme/widget/pull/10",
            "--timeline",
            "--offline",
        ])
        .await;
    pr.expect_code(0);
    assert!(pr.stdout.contains("red"), "{}", pr.stdout);
    if let Some(c) = has_unsafe_chars(&pr.stdout) {
        panic!("pr: unsafe character {c:?}:\n{}", pr.stdout);
    }
    let csv = env
        .run(&[
            "prs",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--offline",
            "--format",
            "csv",
        ])
        .await;
    csv.expect_code(0);
    assert!(!csv.stdout_bytes.contains(&0x1b), "ESC in CSV output");

    // JSON keeps the original text, safely escaped.
    let j = env.offline_json(&["prs"]).await;
    j.expect_code(0);
    assert!(
        !j.stdout_bytes.contains(&0x1b),
        "raw ESC byte in JSON output"
    );
    assert!(
        !j.stdout_bytes.contains(&0x07),
        "raw BEL byte in JSON output"
    );
    let j = j.json();
    let title = j["pull_requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["number"] == 10)
        .unwrap()["title"]
        .clone();
    assert_eq!(title, json!(EVIL));
}

#[tokio::test(flavor = "multi_thread")]
async fn full_resync_of_the_same_period_reflects_new_activity() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["reviews_received"]["total"], 2);

    reserve(&server, &world_after_merge()).await;
    env.sync(&["--full"]).await.expect_code(0);
    let o = env.offline_json(&["overview"]).await.json();
    // dave's approval and the merge of PR A now show up.
    assert_eq!(o["metrics"]["reviews_received"]["total"], 3);
    assert_eq!(o["metrics"]["prs_merged"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_reports_flag_periods_that_were_never_synced() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = requests(&server).await.len();
    // February was never synced.
    let j = env
        .run(&[
            "overview",
            "--since",
            "2024-02-01",
            "--until",
            "2024-02-07",
            "--offline",
            "--format",
            "json",
        ])
        .await;
    j.expect_code(0);
    let j = j.json();
    for kind in ["commits", "pull_requests", "issues"] {
        assert_ne!(coverage_status(&j, kind), "complete", "{kind}");
    }
    let t = env
        .run(&[
            "overview",
            "--since",
            "2024-02-01",
            "--until",
            "2024-02-07",
            "--offline",
        ])
        .await;
    t.expect_code(0);
    assert!(t.stderr.contains("not been synced"), "{}", t.stderr);
    env.run(&[
        "overview",
        "--since",
        "2024-02-01",
        "--until",
        "2024-02-07",
        "--offline",
        "--require-complete",
    ])
    .await
    .expect_code(6);
    assert_eq!(requests(&server).await.len(), before);
}

fn request_count(reqs: &[wiremock::Request], p: &str) -> usize {
    reqs.iter().filter(|r| r.url.path() == p).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_missing_from_search_are_found_on_default_branches_and_counted_once() {
    let mut world = World::base();
    let mine = Some((ME, ME_ID, "User"));
    let w = widget();
    let d = dotfiles();
    // sha 40 is on acme/widget's default branch but commit search misses it.
    world.repo_mut("acme", "widget").default_branch.insert(
        0,
        list_commit(&w.rest, &sha(40), mine, "2024-03-08T10:00:00Z", 1),
    );
    // PR B was merged by fast-forward: its sha 21 is on the default branch
    // too, so the pull request and the listing both report it.
    world.repo_mut(ME, "dotfiles").default_branch.insert(
        1,
        list_commit(&d.rest, &sha(21), mine, "2024-03-06T07:50:00Z", 1),
    );
    let server = serve(&world).await;
    let env = Env::new(&server);

    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "complete");
    // Search: 5 items; the listings add sha 40 and sha 21 (sha 1, 2 and 3
    // are found by both and counted once).
    assert_eq!(s["commits_found"], 7);
    let reqs = requests(&server).await;
    assert_eq!(request_count(&reqs, "/user/repos"), 1);
    for repo in ["acme/widget", "me/dotfiles", "acme/secret"] {
        assert_eq!(
            request_count(&reqs, &format!("/repos/{repo}/commits")),
            1,
            "{repo}"
        );
    }

    let o = env.offline_json(&["overview"]).await.json();
    // The base 6 (sha 1, 2, 3, 11, 12, 21) plus sha 40; sha 21 once.
    assert_eq!(o["metrics"]["commits"], 7);
    assert_eq!(o["metrics"]["merge_commits"], 1);
    assert_eq!(coverage_status(&o, "commits"), "complete");
    let scope = o["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "commits")
        .unwrap()["scopes"]
        .to_string();
    assert!(
        scope.contains("default branches of repositories me pushed to or contributed to"),
        "{scope}"
    );
    let daily = env.offline_json(&["daily"]).await.json();
    assert_eq!(bucket(&daily, "2024-03-06")["summary"]["commits"], 1);
    assert_eq!(bucket(&daily, "2024-03-08")["summary"]["commits"], 1);

    // A rerun has nothing to fetch: no repository listing at all.
    let before = requests(&server).await.len();
    env.sync(&[]).await.expect_code(0);
    let reqs = requests(&server).await;
    assert_eq!(request_count(&reqs[before..], "/user/repos"), 0);
    assert_eq!(
        request_count(&reqs[before..], "/repos/acme/widget/commits"),
        0
    );
    let again = env.offline_json(&["overview"]).await.json();
    assert_eq!(again["metrics"], o["metrics"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_and_inaccessible_repositories_do_not_fail_the_commit_sync() {
    // An empty repository (409) is simply without commits.
    let mut world = World::base();
    let mut empty = widget();
    empty.name = "empty".into();
    empty.gql["id"] = json!("R_empty");
    empty.gql["name"] = json!("empty");
    empty.rest = support::rest_repo("R_empty", "acme", "Organization", "empty", false);
    empty.listed = listed_repo(&empty.rest, "push", "2024-03-06T00:00:00Z");
    world.repos.push(empty);
    let server = serve(&world).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/empty/commits"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "message": "Git Repository is empty.",
            "documentation_url": "https://docs.github.com/rest/commits/commits#list-commits",
            "status": "409"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    assert!(
        requests(&server)
            .await
            .iter()
            .any(|q| q.url.path() == "/repos/acme/empty/commits")
    );
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(coverage_status(&o, "commits"), "complete");
    assert_eq!(o["metrics"]["commits"], 6);

    // A repository the token cannot read makes commits partial, with the
    // repository named, but everything else is still synced.
    let world = World::base();
    let server = serve(&world).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/secret/commits"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "message": "Resource protected by organization SAML enforcement.",
            "documentation_url": "https://docs.github.com/rest",
            "status": "403"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(4).assert_clean();
    assert_eq!(r.json()["status"], "partial");
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(coverage_status(&o, "commits"), "partial");
    assert_eq!(coverage_status(&o, "pull_requests"), "complete");
    // JSON hides the private repository's name unless asked to include it.
    let reasons = coverage_reasons(&o, "commits").join(" ");
    assert!(!reasons.contains("acme/secret"), "{reasons}");
    assert!(reasons.contains("[private repository]"), "{reasons}");
    assert!(!o.to_string().contains("acme/secret"), "{o}");
    let named = env
        .offline_json(&["overview", "--include-private"])
        .await
        .json();
    let reasons = coverage_reasons(&named, "commits").join(" ");
    assert!(reasons.contains("acme/secret"), "{reasons}");
    assert_eq!(o["metrics"]["commits"], 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn public_only_sync_does_not_list_private_repositories() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&["--public-only"]).await.expect_code(0);
    let reqs = requests(&server).await;
    assert_eq!(request_count(&reqs, "/repos/acme/secret/commits"), 0);
    assert_eq!(request_count(&reqs, "/repos/acme/widget/commits"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reports_reuse_a_fresh_sync_and_say_how_old_it_is() {
    let world = World::base();
    let server = serve(&world).await;
    let env = Env::new(&server);
    // A period that runs until now.
    env.run(&["sync", "--since", SINCE, "--format", "json"])
        .await
        .expect_code(0);
    let synced = requests(&server).await.len();

    let r = env
        .run(&["overview", "--since", SINCE, "--format", "json"])
        .await;
    r.expect_code(0).assert_clean();
    assert!(
        r.stderr
            .contains("note: showing data synced less than a minute ago"),
        "{}",
        r.stderr
    );
    // Only the identity check; nothing is fetched again.
    let after: Vec<String> = requests(&server).await[synced..]
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert_eq!(after, vec!["/user".to_string()]);
    // The report ends where the sync ended, so it is complete.
    for kind in ["commits", "pull_requests", "issues"] {
        assert_eq!(coverage_status(&r.json(), kind), "complete");
    }
    let end = r.json()["period"]["end"].as_str().unwrap().to_string();
    let end = chrono::DateTime::parse_from_rfc3339(&end).unwrap();
    assert!(chrono::Utc::now() - end.to_utc() < chrono::Duration::minutes(1));

    // --max-age 0 fetches the latest data.
    let r = env
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--format",
            "json",
            "--max-age",
            "0",
        ])
        .await;
    r.expect_code(0);
    assert!(
        !r.stderr.contains("note: showing data synced"),
        "{}",
        r.stderr
    );
    assert!(requests(&server).await.len() > synced);
}

/// Replaces `acme/widget` with `acme/gadget` everywhere in a fixture
/// value, and the bare repository name `widget` with `gadget`.
fn renamed_value(v: &Value) -> Value {
    match v {
        Value::String(s) if s == "widget" => json!("gadget"),
        Value::String(s) => json!(s.replace("acme/widget", "acme/gadget")),
        Value::Array(a) => Value::Array(a.iter().map(renamed_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), renamed_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The base world after acme/widget was renamed to acme/gadget on GitHub.
/// The repository keeps its node ID (R_widget), and so do its pull
/// requests and issues; only names and URLs change.
fn world_after_rename() -> World {
    let mut w = World::base();
    for r in &mut w.repos {
        if r.name == "widget" {
            r.name = "gadget".into();
            r.gql = renamed_value(&r.gql);
            r.rest = renamed_value(&r.rest);
            r.listed = renamed_value(&r.listed);
            r.default_branch = r.default_branch.iter().map(renamed_value).collect();
        }
    }
    w.commits = w.commits.iter().map(renamed_value).collect();
    for p in &mut w.prs {
        if p.name == "widget" {
            p.name = "gadget".into();
            p.gql = renamed_value(&p.gql);
        }
    }
    for i in &mut w.issues {
        if i.name == "widget" {
            i.name = "gadget".into();
            i.gql = renamed_value(&i.gql);
        }
    }
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn renamed_repository_is_one_repository_after_resync() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    reserve(&server, &world_after_rename()).await;
    let r = env.sync(&["--full"]).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["status"], "complete");

    // Same repository ID, so still two repositories with activity: the
    // renamed one (sha 1, 11, 12) and me/dotfiles (sha 2, 3, 21).
    let rp = env.offline_json(&["repos"]).await.json();
    let repos: Vec<(String, String, u64)> = rp["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["repository_id"].as_str().unwrap().to_string(),
                r["full_name"].as_str().unwrap().to_string(),
                r["summary"]["commits"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        repos,
        vec![
            (WIDGET_ID.to_string(), "acme/gadget".to_string(), 3),
            (DOTFILES_ID.to_string(), "me/dotfiles".to_string(), 3)
        ]
    );
    // Nothing is counted twice.
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["commits"], 6);
    assert_eq!(o["metrics"]["prs_created"], 2);
    assert_eq!(o["metrics"]["pr_commits"], 3);
    assert_eq!(o["metrics"]["active_prs"], 3);
    assert_eq!(o["metrics"]["issues_closed"], 2);
    // The new name selects everything of the repository.
    let g = env
        .offline_json(&["overview", "--repo", "acme/gadget"])
        .await
        .json();
    assert_eq!(g["metrics"]["commits"], 3);
    assert_eq!(g["metrics"]["prs_created"], 1);
    // The pull request is found under its new URL.
    let pr = env
        .run(&[
            "pr",
            "https://github.com/acme/gadget/pull/10",
            "--offline",
            "--format",
            "json",
        ])
        .await;
    pr.expect_code(0);
    let pr = pr.json();
    assert_eq!(pr["pr_id"], "PR_A");
    assert_eq!(pr["repository"], "acme/gadget");
    assert_eq!(pr["commits"], 4);

    // The old URL still works online: GitHub resolves it to the renamed
    // repository, and the fetched pull request is shown.
    let renamed = world_after_rename();
    let pr_a = renamed.prs.iter().find(|p| p.number == 10).unwrap();
    pr_detail_mock(
        "acme",
        "widget",
        10,
        ResponseTemplate::new(200).set_body_json(renamed.pr_body(pr_a)),
        1,
    )
    .mount(&server)
    .await;
    let old = env
        .run(&[
            "pr",
            "https://github.com/acme/widget/pull/10",
            "--format",
            "json",
        ])
        .await;
    old.expect_code(0).assert_clean();
    let old = old.json();
    assert_eq!(old["pr_id"], "PR_A");
    assert_eq!(old["repository"], "acme/gadget");
}

/// Mounts, at the highest priority, responses for acme/widget having been
/// deleted on GitHub: its commit list and repository lookups are 404, the
/// repository is gone from `GET /user/repos`, and `PullRequestDetail` for
/// acme/widget#20 says the repository cannot be resolved.
async fn mount_widget_deleted(server: &wiremock::MockServer, world: &World) {
    let not_found = json!({
        "message": "Not Found",
        "documentation_url": "https://docs.github.com/rest/commits/commits#list-commits",
        "status": "404"
    });
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .respond_with(ResponseTemplate::new(404).set_body_json(not_found.clone()))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget"))
        .respond_with(ResponseTemplate::new(404).set_body_json(not_found))
        .with_priority(1)
        .mount(server)
        .await;
    let gone = json!({
        "data": { "repository": null },
        "errors": [{
            "type": "NOT_FOUND",
            "path": ["repository"],
            "locations": [{ "line": 2, "column": 3 }],
            "message": "Could not resolve to a Repository with the name 'acme/widget'."
        }]
    });
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query RepositoryDetail"))
        .and(body_partial_json(
            json!({ "variables": { "owner": "acme", "name": "widget" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(gone.clone()))
        .with_priority(1)
        .mount(server)
        .await;
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200).set_body_json(gone),
        1,
    )
    .mount(server)
    .await;
    let mut listed: Vec<Value> = world
        .repos
        .iter()
        .filter(|r| r.name != "widget")
        .map(|r| r.listed.clone())
        .collect();
    listed.sort_by(|a, b| {
        b["pushed_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["pushed_at"].as_str().unwrap_or(""))
    });
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(listed)))
        .with_priority(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_deleted_after_sync_keeps_cached_data_and_is_partial() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = env.offline_json(&["overview"]).await.json();

    // acme/widget is deleted. Commit search no longer finds sha 1, 4 and 5;
    // issue search no longer finds acme/widget#7. The pull request search
    // index still lists acme/widget#20 (it lags behind), but its detail
    // query fails. me/dotfiles is unchanged.
    let mut world = World::base();
    world
        .commits
        .retain(|c| c["repository"]["full_name"] != "acme/widget");
    world.prs.retain(|p| p.number != 10);
    world.issues.retain(|i| i.name != "widget");
    reserve(&server, &world).await;
    mount_widget_deleted(&server, &world).await;

    let r = env.sync(&["--full"]).await;
    r.expect_code(4).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "partial");
    let failures = s["failures"].to_string();
    assert!(failures.contains("acme/widget#20"), "{failures}");

    // Everything synced before is kept: sha 1, 11, 12 of acme/widget and
    // pull requests #10 and #20 are still counted.
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0).assert_clean();
    let o = o.json();
    assert_eq!(o["metrics"], before["metrics"]);
    assert_eq!(o["metrics"]["commits"], 6);
    assert_eq!(coverage_status(&o, "commits"), "partial");
    assert_eq!(coverage_status(&o, "pull_requests"), "partial");
    // acme/widget is public, so its name may appear in the reason.
    let reasons = coverage_reasons(&o, "commits").join(" ");
    assert!(reasons.contains("acme/widget"), "{reasons}");

    // The repository is still listed with its data, marked inaccessible.
    let rp = env.offline_json(&["repos"]).await.json();
    let find = |id: &str| -> Value {
        rp["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["repository_id"] == id)
            .unwrap_or_else(|| panic!("{id} missing: {rp}"))
            .clone()
    };
    let w = find(WIDGET_ID);
    assert_eq!(w["accessible"], false);
    assert_eq!(w["summary"]["commits"], 3);
    assert_eq!(find(DOTFILES_ID)["accessible"], true);
    let p = env.offline_json(&["prs"]).await.json();
    assert_eq!(pr_numbers(&p), vec![3, 10, 20]);
    env.run(&[
        "pr",
        "https://github.com/acme/widget/pull/20",
        "--offline",
        "--format",
        "json",
    ])
    .await
    .expect_code(0);

    // Incomplete data is detected.
    env.offline_json(&["overview", "--require-complete"])
        .await
        .expect_code(6);
    let t = env
        .run(&["repos", "--since", SINCE, "--until", UNTIL, "--offline"])
        .await;
    t.expect_code(0);
    assert!(t.stdout.contains("acme/widget (no access)"), "{}", t.stdout);
}

/// 250 commits of mine in acme/widget, one every 30 minutes from
/// 2024-03-04T00:00Z, returned by commit search over three pages
/// (100 + 100 + 50) linked with `Link: <...>; rel="next"`.
#[tokio::test(flavor = "multi_thread")]
async fn two_hundred_fifty_commits_over_three_search_pages_are_all_counted() {
    let mut world = World::base();
    world.commits = vec![];
    for r in &mut world.repos {
        r.default_branch.clear();
    }
    world.prs = vec![];
    world.issues = vec![];
    let server = serve(&world).await;
    let w = widget();
    let mine = Some((ME, ME_ID, "User"));
    let start = chrono::DateTime::parse_from_rfc3339("2024-03-04T00:00:00Z").unwrap();
    let all: Vec<Value> = (0..250)
        .map(|i| {
            let t = (start + chrono::Duration::minutes(30 * i))
                .to_utc()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            search_commit(&w.rest, &sha(1000 + i as u64), mine, &t, 1)
        })
        .collect();
    let page = |items: &[Value]| json!({ "total_count": 250, "incomplete_results": false, "items": items });
    let uri = server.uri();
    let next = |n: u32| {
        format!(
            "<{uri}/search/commits?marker=m&per_page=100&page={n}>; rel=\"next\", <{uri}/search/commits?marker=m&per_page=100&page=3>; rel=\"last\""
        )
    };
    Mock::given(method("GET"))
        .and(path("/search/commits"))
        .and(query_param_contains("q", format!("author:{ME}")))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(page(&all[0..100]))
                .insert_header("link", next(2).as_str()),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    for (n, items, link) in [
        (2, &all[100..200], Some(next(3))),
        (3, &all[200..250], None),
    ] {
        let mut resp = ResponseTemplate::new(200).set_body_json(page(items));
        if let Some(l) = link {
            resp = resp.insert_header("link", l.as_str());
        }
        Mock::given(method("GET"))
            .and(path("/search/commits"))
            .and(query_param("marker", "m"))
            .and(query_param("page", n.to_string()))
            .respond_with(resp)
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let env = Env::new(&server);
    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["status"], "complete");
    assert_eq!(s["commits_found"], 250);

    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["commits"], 250);
    assert_eq!(coverage_status(&o, "commits"), "complete");
    // 48 commits a day on 03-04..03-08 (240), then 10 on 03-09 (00:00 to
    // 04:30).
    let d = env.offline_json(&["daily"]).await.json();
    for (date, n) in [
        ("2024-03-04", 48),
        ("2024-03-05", 48),
        ("2024-03-06", 48),
        ("2024-03-07", 48),
        ("2024-03-08", 48),
        ("2024-03-09", 10),
        ("2024-03-10", 0),
    ] {
        assert_eq!(bucket(&d, date)["summary"]["commits"], n, "{date}");
    }
    // Re-syncing the same pages never adds commits.
    env.sync(&["--full"]).await.expect_code(0);
    let again = env.offline_json(&["overview"]).await.json();
    assert_eq!(again["metrics"]["commits"], 250);
}

/// Answers a `CommitLineCounts` query: every requested commit gets
/// additions = 10 and deletions = 1, except SHAs in `missing`, which GitHub
/// does not return.
fn line_count_answer(req: &wiremock::Request, missing: &[String]) -> ResponseTemplate {
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    let q = body["query"].as_str().unwrap();
    let mut data = serde_json::Map::new();
    let mut repo = String::new();
    for line in q.lines().map(str::trim) {
        if let Some((alias, _)) = line.split_once(": repository(") {
            repo = alias.to_string();
            data.insert(repo.clone(), json!({}));
        } else if let Some((alias, rest)) = line.split_once(": object(oid: \"") {
            let sha = &rest[..40];
            let v = if missing.iter().any(|m| m == sha) {
                Value::Null
            } else {
                json!({ "additions": 10, "deletions": 1 })
            };
            data[&repo][alias] = v;
        }
    }
    ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_stats_are_fetched_once_and_unknown_ones_stay_unknown() {
    let world = World::base();
    let server = serve(&world).await;
    // sha 3 is an ordinary commit of mine (sha 2 is a merge commit, whose
    // counts are never asked for).
    let missing = vec![sha(3)];
    let m = missing.clone();
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(move |req: &wiremock::Request| line_count_answer(req, &m))
        .mount(&server)
        .await;
    let env = Env::new(&server);

    // A plain sync asks for no line counts.
    env.sync(&[]).await.expect_code(0).assert_clean();
    let asked = |reqs: &[wiremock::Request]| {
        reqs.iter()
            .filter(|r| String::from_utf8_lossy(&r.body).contains("query CommitLineCounts"))
            .count()
    };
    assert_eq!(asked(&requests(&server).await), 0);
    let o = env.offline_json(&["overview"]).await.json();
    let d = &o["metrics"]["diff_stats"];
    assert_eq!(d["coverage"], "unavailable", "{d}");
    let commits = d["commits"].as_u64().unwrap();
    assert!(commits > 1, "{d}");

    // --diff-stats fetches them; one commit is not returned by GitHub.
    let r = env.sync(&["--diff-stats"]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    assert_eq!(s["diff_stats"]["commits"], commits);
    assert_eq!(s["diff_stats"]["unavailable"], 1, "{s}");
    let o = env.offline_json(&["overview"]).await.json();
    let d = &o["metrics"]["diff_stats"];
    assert_eq!(d["coverage"], "partial", "{d}");
    assert_eq!(d["commits_with_stats"], commits - 1);
    assert_eq!(d["additions"], 10 * (commits - 1));
    assert_eq!(d["deletions"], commits - 1);

    // Running it again only asks for the one still missing.
    let before = asked(&requests(&server).await);
    let s = env.sync(&["--diff-stats"]).await.json();
    assert_eq!(s["diff_stats"]["already_known"], commits - 1);
    assert_eq!(asked(&requests(&server).await), before + 1);
}
