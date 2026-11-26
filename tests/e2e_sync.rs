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

/// With fewer parallel requests than pull requests, several pull requests go
/// in one query; what is stored is the same as fetching them one by one.
#[tokio::test(flavor = "multi_thread")]
async fn pull_requests_fetched_in_one_query_match_one_by_one() {
    let world = World::base();
    let server = serve(&world).await;
    let one_by_one = Env::new(&server);
    one_by_one.sync(&[]).await.expect_code(0).assert_clean();
    let reqs = requests(&server).await;
    assert_eq!(all_pr_detail_requests(&reqs), 3);
    assert_eq!(pr_batch_requests(&reqs), 0);

    reserve(&server, &world).await;
    let batched = Env::new(&server);
    let r = batched.sync(&["--jobs", "1"]).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["status"], "complete");
    assert_eq!(r.json()["pull_requests_fetched"], 3);
    let reqs = requests(&server).await;
    assert_eq!(all_pr_detail_requests(&reqs), 0);
    assert_eq!(pr_batch_requests(&reqs), 1);

    for cmd in REPORTS {
        let a = one_by_one.offline_json(&[cmd]).await;
        let b = batched.offline_json(&[cmd]).await;
        a.expect_code(0);
        b.expect_code(0);
        assert_eq!(without_times(a.json()), without_times(b.json()), "{cmd}");
    }
}

/// `value` without the times at which it was synced or generated.
fn without_times(mut value: Value) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.retain(|k, _| {
                    !matches!(
                        k.as_str(),
                        "generated_at"
                            | "synced_at"
                            | "observed_at"
                            | "started_at"
                            | "finished_at"
                            | "last_sync"
                    )
                });
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut value);
    value
}

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
/// - dave APPROVED on 03-08 09:00; bob commented `IC_A4` on 03-08 12:00
/// - bob's `IC_A2` was edited on 03-08 13:00; my `IC_A1` was deleted
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
    // Merged with a merge commit (two parents), as GitHub reports it.
    g["mergeCommit"] = json!({ "parents": { "totalCount": 2 } });
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
    assert_ne!(types, Vec::<&str>::new());
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
async fn viewing_an_unrelated_pull_request_keeps_its_detail_without_claiming_activity() {
    let mut world = World::base();
    let viewed = world.pr_mut(20);
    viewed.reviewed_by_me = false;
    viewed.gql["reviews"] = connection(vec![]);
    viewed.gql["reviewThreads"] = connection(vec![]);
    let server = serve(&world).await;
    let env = Env::new(&server);
    let url = "https://github.com/acme/widget/pull/20";

    let online = env
        .run(&["pr", url, "--timeline", "--format", "json"])
        .await;
    online.expect_code(0).assert_clean();
    let online_json = online.json();
    let online_timeline = online_json["timeline"].clone();
    assert!(timeline_ids(&online_json).contains(&"pr:PR_C:created".to_string()));

    let offline = env
        .run(&["pr", url, "--timeline", "--format", "json", "--offline"])
        .await;
    offline.expect_code(0).assert_clean();
    assert_eq!(offline.json()["timeline"], online_timeline);

    let sync = env.sync(&[]).await;
    sync.expect_code(0).assert_clean();
    assert_eq!(sync.json()["pull_requests_found"], 2);
    assert_eq!(sync.json()["pull_requests_fetched"], 2);
    let overview = env.offline_json(&["overview"]).await;
    overview.expect_code(0);
    assert_eq!(overview.json()["metrics"]["reviews_submitted"]["total"], 0);
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
    empty.rest = rest_repo("R_empty", "acme", "Organization", "empty", false);
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

/// copier/widget-copy: someone else's copy of acme/widget, uploaded as a new
/// repository rather than forked, holding my `commits` (sha number and
/// committer date) on its default branch. Returns the repository and the
/// commit search results that find those commits there.
fn widget_copy(commits: &[(u64, &str)]) -> (RepoFixture, Vec<Value>) {
    let rest = rest_repo("R_copy", "copier", "User", "widget-copy", false);
    let mine = Some((ME, ME_ID, "User"));
    let search = commits
        .iter()
        .map(|(n, at)| search_commit(&rest, &sha(*n), mine, at, 1))
        .collect();
    let repo = RepoFixture {
        owner: "copier".into(),
        name: "widget-copy".into(),
        gql: gql_repo("R_copy", user("copier"), "widget-copy", "PUBLIC", "READ"),
        listed: listed_repo(&rest, "pull", "2024-03-09T00:00:00Z"),
        default_branch: commits
            .iter()
            .map(|(n, at)| list_commit(&rest, &sha(*n), mine, at, 1))
            .collect(),
        rest,
    };
    (repo, search)
}

/// `GET /user/repos` listing only the base world's repositories.
async fn mount_base_repo_list(server: &wiremock::MockServer) {
    let mut listed: Vec<Value> = World::base()
        .repos
        .iter()
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

fn repository_names(report: &Value) -> Vec<String> {
    report["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_found_by_search_are_skipped_and_other_search_results_kept() {
    let mut world = World::base();
    let mine = Some((ME, ME_ID, "User"));
    // sha 40 is on acme/widget's default branch, but search misses it there.
    let w = widget();
    world.repo_mut("acme", "widget").default_branch.insert(
        0,
        list_commit(&w.rest, &sha(40), mine, "2024-03-08T10:00:00Z", 1),
    );
    // The copy is not in my repository list, and search finds sha 1 and
    // sha 40 there: both are copies of acme/widget's commits.
    let (copy, copy_search) =
        widget_copy(&[(1, "2024-03-04T10:00:00Z"), (40, "2024-03-08T10:00:00Z")]);
    world.commits.extend(copy_search);
    world.repos.push(copy);
    // upstream/tool is not in my repository list either, but I opened
    // upstream/tool#9 (found by this sync): search finds my sha 61 there,
    // and its default branch also holds my sha 70, which search misses.
    let tool_rest = rest_repo("R_tool", "upstream", "User", "tool", false);
    world.commits.push(search_commit(
        &tool_rest,
        &sha(61),
        mine,
        "2024-03-06T10:00:00Z",
        1,
    ));
    world.repos.push(RepoFixture {
        owner: "upstream".into(),
        name: "tool".into(),
        gql: gql_repo("R_tool", user("upstream"), "tool", "PUBLIC", "READ"),
        listed: listed_repo(&tool_rest, "pull", "2024-03-09T00:00:00Z"),
        default_branch: vec![
            list_commit(&tool_rest, &sha(70), mine, "2024-03-07T10:00:00Z", 1),
            list_commit(&tool_rest, &sha(61), mine, "2024-03-06T10:00:00Z", 1),
        ],
        rest: tool_rest,
    });
    let mut issue = issue_1();
    issue.owner = "upstream".into();
    issue.name = "tool".into();
    issue.number = 9;
    issue.node_id = "I_tool".into();
    issue.gql["id"] = json!("I_tool");
    issue.gql["number"] = json!(9);
    issue.gql["url"] = json!("https://github.com/upstream/tool/issues/9");
    issue.gql["state"] = json!("OPEN");
    issue.gql["closedAt"] = Value::Null;
    issue.gql["timelineItems"] = connection(vec![]);
    world.issues.push(issue);
    // someone/patched has no known relation to me, and its sha 62 is not
    // found anywhere else: it counts, from search alone.
    let patched = rest_repo("R_patched", "someone", "User", "patched", false);
    world.commits.push(search_commit(
        &patched,
        &sha(62),
        mine,
        "2024-03-07T12:00:00Z",
        1,
    ));
    let server = serve(&world).await;
    mount_base_repo_list(&server).await;
    let env = Env::new(&server);

    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["status"], "complete");
    // Only repositories known to be related are listed.
    let reqs = requests(&server).await;
    assert_eq!(request_count(&reqs, "/repos/copier/widget-copy/commits"), 0);
    assert_eq!(request_count(&reqs, "/repos/someone/patched/commits"), 0);
    assert_eq!(request_count(&reqs, "/repos/upstream/tool/commits"), 1);
    assert_eq!(request_count(&reqs, "/repos/acme/widget/commits"), 1);

    let o = env.offline_json(&["overview"]).await.json();
    // The base 6, sha 40 of acme/widget once, sha 61 and sha 70 of
    // upstream/tool, and sha 62 of someone/patched; nothing of the copy.
    assert_eq!(o["metrics"]["commits"], 10);
    assert_eq!(coverage_status(&o, "commits"), "complete");
    let names = repository_names(&o);
    assert!(names.contains(&"upstream/tool".to_string()), "{names:?}");
    assert!(names.contains(&"someone/patched".to_string()), "{names:?}");
    assert!(
        !names.contains(&"copier/widget-copy".to_string()),
        "{names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn copies_already_cached_are_removed_by_a_resync() {
    // The copy is in my repository list at first, so its commits count:
    // sha 1 once per repository, and sha 60, which only the copy has.
    let mut world = World::base();
    let (copy, copy_search) =
        widget_copy(&[(1, "2024-03-04T10:00:00Z"), (60, "2024-03-06T09:00:00Z")]);
    world.commits.extend(copy_search);
    world.repos.push(copy);
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0).assert_clean();
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["commits"], 8);

    // Once it is not, a sync of the period takes out the copy of sha 1 but
    // keeps sha 60, which search still finds there.
    mount_base_repo_list(&server).await;
    let r = env.sync(&["--full"]).await;
    r.expect_code(0).assert_clean();
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["commits"], 7);
    assert_eq!(coverage_status(&o, "commits"), "complete");
    let rp = env.offline_json(&["repos"]).await.json();
    let copy = rp["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["repository_id"] == "R_copy")
        .unwrap_or_else(|| panic!("copier/widget-copy missing: {rp}"));
    assert_eq!(copy["summary"]["commits"], 1);
    // 2024-03-04 has sha 1 and sha 11 again, without the copy of sha 1.
    let daily = env.offline_json(&["daily"]).await.json();
    assert_eq!(bucket(&daily, "2024-03-04")["summary"]["commits"], 2);
    // A resync changes nothing.
    env.sync(&["--full"]).await.expect_code(0);
    let again = env.offline_json(&["overview"]).await.json();
    assert_eq!(again["metrics"], o["metrics"]);
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
/// The repository keeps its node ID (`R_widget`), and so do its pull
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
            search_commit(
                &w.rest,
                &sha(1000 + u64::try_from(i).expect("fixture index fits u64")),
                mine,
                &t,
                1,
            )
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
    ResponseTemplate::new(200).set_body_json(json!({ "data": line_count_data(req, missing) }))
}

/// The `data` of [`line_count_answer`].
fn line_count_data(req: &wiremock::Request, missing: &[String]) -> Value {
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
    Value::Object(data)
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

    // A sync with --no-diff-stats asks for no line counts, even the first.
    env.sync(&["--no-diff-stats"])
        .await
        .expect_code(0)
        .assert_clean();
    let asked = |reqs: &[wiremock::Request]| {
        reqs.iter()
            .filter(|r| String::from_utf8_lossy(&r.body).contains("query CommitLineCounts"))
            .count()
    };
    assert_eq!(asked(&requests(&server).await), 0);
    let o = env.offline_json(&["overview"]).await.json();
    let d = &o["metrics"]["diff_stats"];
    assert_eq!(d["coverage"], "unavailable", "{d}");
    // Unknown sums are null, never 0.
    assert!(d["additions"].is_null() && d["deletions"].is_null(), "{d}");
    // Commits whose line counts go into the totals (others are excluded as
    // counted through other commits).
    let commits = d["commits"].as_u64().unwrap() - d["excluded"].as_u64().unwrap();
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

/// The `api` statistics of a sync cover the line count queries too.
#[tokio::test(flavor = "multi_thread")]
async fn sync_statistics_include_the_line_count_queries() {
    let world = World::base();
    let server = serve(&world).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(|req: &wiremock::Request| {
            let mut data = line_count_data(req, &[]);
            data["rateLimit"] = json!({ "cost": 7 });
            ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
        })
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = env.sync(&["--diff-stats"]).await;
    r.expect_code(0).assert_clean();
    let s = r.json();
    let reqs = requests(&server).await;
    let asked = reqs
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("query CommitLineCounts"))
        .count() as u64;
    assert!(asked > 0);
    // Only the line count answers report a cost in this test.
    assert_eq!(s["api"]["graphql_points"], 7 * asked, "{s}");
    assert_eq!(s["api"]["requests"], reqs.len() as u64, "{s}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_line_count_query_keeps_the_sync_and_leaves_counts_unknown() {
    let world = World::base();
    let server = serve(&world).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = env.sync(&["--diff-stats"]).await;
    // The line count query fails as an API error ...
    r.expect_code(4).assert_clean();
    // ... but what the sync fetched before it is kept, and nothing is
    // presented as zero.
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0).assert_clean();
    let o = o.json();
    assert_eq!(o["metrics"]["commits"], 6);
    let d = &o["metrics"]["diff_stats"];
    assert_eq!(d["coverage"], "unavailable", "{d}");
    assert_eq!(d["commits_with_stats"], 0);
    assert!(d["additions"].is_null(), "{d}");
    // A later run fetches them once GitHub answers again.
    server.reset().await;
    let world = World::base();
    reserve(&server, &world).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(|req: &wiremock::Request| line_count_answer(req, &[]))
        .mount(&server)
        .await;
    env.sync(&["--diff-stats"])
        .await
        .expect_code(0)
        .assert_clean();
    let d = env.offline_json(&["overview"]).await.json()["metrics"]["diff_stats"].clone();
    assert_eq!(d["coverage"], "complete", "{d}");
}

/// Records when each request arrives, then answers with `response`.
struct Arrivals {
    at: std::sync::Arc<std::sync::Mutex<Vec<std::time::Instant>>>,
    response: ResponseTemplate,
}

impl wiremock::Respond for Arrivals {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        self.at.lock().unwrap().push(std::time::Instant::now());
        self.response.clone()
    }
}

/// Mounts `response` for `GET {endpoint}` above the fixture and returns the
/// arrival times of its requests.
async fn record_arrivals(
    server: &wiremock::MockServer,
    endpoint: &str,
    response: ResponseTemplate,
) -> std::sync::Arc<std::sync::Mutex<Vec<std::time::Instant>>> {
    let at = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    Mock::given(method("GET"))
        .and(path(endpoint))
        .respond_with(Arrivals {
            at: std::sync::Arc::clone(&at),
            response,
        })
        .with_priority(1)
        .mount(server)
        .await;
    at
}

/// Rows the cache holds for `login`: sync records, coverage records and
/// linked events.
fn account_rows(env: &Env, login: &str) -> (i64, i64, i64) {
    let db = rusqlite::Connection::open_with_flags(
        env.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count = |table: &str| -> i64 {
        db.query_row(
            &format!(
                "SELECT count(*) FROM {table} t JOIN accounts a ON a.id = t.account_id \
                 WHERE a.login = ?1"
            ),
            [login],
            |r| r.get(0),
        )
        .unwrap()
    };
    (
        count("sync_runs"),
        count("coverage_intervals"),
        count("account_events"),
    )
}

fn remembered_viewer(env: &Env) -> String {
    let db = rusqlite::Connection::open_with_flags(
        env.db_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.query_row(
        "SELECT value FROM cache_metadata WHERE key = 'viewer_login'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remembered_account_starts_syncing_without_waiting_for_the_identity_check() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    // The identity check now takes 2 s; commit search must not wait for it.
    let delay = std::time::Duration::from_secs(2);
    let user = record_arrivals(
        &server,
        "/user",
        ResponseTemplate::new(200)
            .set_body_json(json!({
                "login": ME, "id": 1, "node_id": ME_ID, "type": "User", "name": "Me"
            }))
            .set_delay(delay),
    )
    .await;
    let search = record_arrivals(
        &server,
        "/search/commits",
        ResponseTemplate::new(200).set_body_json(search_page(vec![])),
    )
    .await;
    env.sync(&["--full"]).await.expect_code(0).assert_clean();
    let user_at = user.lock().unwrap()[0];
    let first_search = *search.lock().unwrap().first().expect("commit search ran");
    assert!(
        first_search < user_at + delay,
        "commit search waited for the identity check"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_of_another_account_writes_nothing_for_the_remembered_one() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = account_rows(&env, ME);
    assert!(before.0 > 0 && before.1 > 0 && before.2 > 0, "{before:?}");
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "login": "other", "id": 2, "node_id": "U_other", "type": "User", "name": "Other"
                }))
                // Late enough that the sync reaches its first writes before it.
                .set_delay(std::time::Duration::from_millis(1500)),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    // Search finds nothing for the other account.
    for endpoint in ["/search/issues", "/search/commits"] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .and(query_param_contains("q", "other"))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(vec![])))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    // A week before the cached one, so a write for the remembered account
    // would add coverage rather than repeat it.
    let r = env
        .run(&[
            "sync",
            "--since",
            "2024-02-26",
            "--until",
            SINCE,
            "--format",
            "json",
        ])
        .await;
    // Partial: the fixture only lists commits of the remembered account,
    // so listing the other account's commits fails.
    r.expect_code(4).assert_clean();
    let report = r.json();
    assert_eq!(report["status"], "partial");
    // Nothing more for the remembered account; the sync ran for the token's.
    assert_eq!(account_rows(&env, ME), before);
    assert!(account_rows(&env, "other").0 > 0);
    assert_eq!(remembered_viewer(&env), "other");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_token_writes_nothing_for_the_remembered_account() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    let before = account_rows(&env, ME);
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials", "documentation_url": "https://docs.github.com/rest"
        }))
        .set_delay(std::time::Duration::from_millis(1500)))
        .with_priority(1)
        .mount(&server)
        .await;
    env.run(&["sync", "--since", SINCE, "--until", UNTIL, "--full"])
        .await
        .expect_code(3)
        .assert_clean();
    assert_eq!(account_rows(&env, ME), before);
    // Report commands still do not hide the failure behind the cache.
    env.run(&[
        "overview",
        "--since",
        SINCE,
        "--until",
        UNTIL,
        "--max-age",
        "1h",
    ])
    .await
    .expect_code(3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_identity_check_shows_no_cache_of_the_remembered_account() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(422).set_body_json(json!({ "message": "Unprocessable" })),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    // A period that needs a sync: the report fails instead of showing the
    // cache of an account that was not confirmed.
    let r = env
        .run(&["overview", "--since", SINCE, "--format", "json"])
        .await;
    r.expect_code(4);
    assert!(r.stdout.is_empty(), "{}", r.stdout);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_during_another_sync_still_shows_the_cache() {
    let world = World::base();
    let server = serve(&world).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);
    // Keep the first sync, and its lock, busy on one pull request.
    pr_detail_mock(
        "acme",
        "widget",
        20,
        ResponseTemplate::new(200)
            .set_body_json(world.pr_body(&world.prs[2]))
            .set_delay(std::time::Duration::from_secs(4)),
        1,
    )
    .mount(&server)
    .await;
    let searches = |reqs: &[wiremock::Request]| {
        reqs.iter()
            .filter(|r| r.url.path() == "/search/commits")
            .count()
    };
    let before = searches(&requests(&server).await);
    let first = env
        .command(&["sync", "--since", SINCE, "--until", UNTIL, "--full"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        wait_until(std::time::Duration::from_secs(10), || async {
            searches(&requests(&server).await) > before
        })
        .await
    );
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    // A period that needs a sync: the lock is taken, so the report warns and
    // shows the cache of the confirmed account.
    let r = env
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--until",
            "2024-03-12",
            "--format",
            "json",
        ])
        .await;
    r.expect_code(0);
    assert!(r.stderr.contains("already running"), "{}", r.stderr);
    assert!(r.stderr.contains("showing cached data"), "{}", r.stderr);
    assert!(r.json()["metrics"].is_object());
    let mut first = first;
    tokio::task::spawn_blocking(move || first.wait().unwrap())
        .await
        .unwrap();
}

/// Answers every line count query, with all commits known.
async fn mount_line_counts(server: &wiremock::MockServer) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(|req: &wiremock::Request| line_count_answer(req, &[]))
        .mount(server)
        .await;
}

/// Line count queries the server has received.
async fn line_count_queries(server: &wiremock::MockServer) -> usize {
    requests(server)
        .await
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("query CommitLineCounts"))
        .count()
}

/// `sync` over the standard period as JSON, with `CI` set to `ci`.
async fn sync_in(env: &Env, ci: Option<&str>, extra: &[&str]) -> Run {
    let mut args = vec![
        "sync", "--since", SINCE, "--until", UNTIL, "--format", "json",
    ];
    args.extend_from_slice(extra);
    let mut c = env.command(&args);
    if let Some(v) = ci {
        c.env("CI", v);
    }
    env.run_command(c).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_sync_fetches_line_counts_and_later_ones_do_not() {
    let server = serve(&World::base()).await;
    mount_line_counts(&server).await;
    let env = Env::new(&server);
    let first = sync_in(&env, None, &[]).await;
    first.expect_code(0).assert_clean();
    assert!(first.json()["diff_stats"].is_object(), "{}", first.stdout);
    let asked = line_count_queries(&server).await;
    assert!(asked > 0);
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["diff_stats"]["coverage"], "complete");
    // A later interactive sync stays as it was.
    let later = sync_in(&env, None, &["--full"]).await;
    later.expect_code(0).assert_clean();
    assert!(later.json()["diff_stats"].is_null(), "{}", later.stdout);
    assert_eq!(line_count_queries(&server).await, asked);
}

#[tokio::test(flavor = "multi_thread")]
async fn syncs_in_ci_fetch_line_counts_unless_told_not_to() {
    let server = serve(&World::base()).await;
    mount_line_counts(&server).await;
    let env = Env::new(&server);
    sync_in(&env, None, &["--no-diff-stats"])
        .await
        .expect_code(0);
    assert_eq!(line_count_queries(&server).await, 0);
    // CI=false or 0 is not CI.
    for off in ["false", "0", ""] {
        let r = sync_in(&env, Some(off), &[]).await;
        r.expect_code(0);
        assert!(r.json()["diff_stats"].is_null(), "CI={off}: {}", r.stdout);
    }
    assert_eq!(line_count_queries(&server).await, 0);
    let r = sync_in(&env, Some("true"), &["--no-diff-stats"]).await;
    assert!(r.json()["diff_stats"].is_null(), "{}", r.stdout);
    assert_eq!(line_count_queries(&server).await, 0);
    let r = sync_in(&env, Some("true"), &[]).await;
    r.expect_code(0).assert_clean();
    assert!(r.json()["diff_stats"].is_object(), "{}", r.stdout);
    assert!(line_count_queries(&server).await > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sync_of_a_first_report_fetches_line_counts() {
    let server = serve(&World::base()).await;
    mount_line_counts(&server).await;
    let env = Env::new(&server);
    let r = env
        .run(&[
            "overview", "--since", SINCE, "--until", UNTIL, "--format", "json",
        ])
        .await;
    r.expect_code(0).assert_clean();
    assert!(line_count_queries(&server).await > 0);
    assert_eq!(r.json()["metrics"]["diff_stats"]["coverage"], "complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn line_counts_fetched_by_default_do_not_fail_the_sync() {
    let server = serve(&World::base()).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let env = Env::new(&server);
    let r = sync_in(&env, None, &[]).await;
    r.expect_code(0).assert_clean();
    assert!(r.stderr.contains("line counts not fetched"), "{}", r.stderr);
    // Unknown line counts stay unknown, never zero.
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["diff_stats"]["coverage"], "unavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_token_while_fetching_line_counts_by_default_fails() {
    let server = serve(&World::base()).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query CommitLineCounts"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials", "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;
    let env = Env::new(&server);
    // The first sync, and the first report on a second empty cache.
    sync_in(&env, None, &[]).await.expect_code(3).assert_clean();
    let other = Env::new(&server);
    other
        .run(&["overview", "--since", SINCE, "--until", UNTIL])
        .await
        .expect_code(3)
        .assert_clean();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_first_sync_leaves_the_next_one_first_and_a_partial_one_does_not() {
    let server = serve(&World::base()).await;
    mount_line_counts(&server).await;
    let env = Env::new(&server);
    // The first sync fails: pull request search answers 500 while `failing`.
    let failing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let on = std::sync::Arc::clone(&failing);
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(move |_: &wiremock::Request| on.load(std::sync::atomic::Ordering::SeqCst))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&server)
        .await;
    let failed = sync_in(&env, None, &["--no-diff-stats"]).await;
    assert_ne!(failed.code, Some(0), "{}", failed.stderr);
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    // So the next sync is still the first finished one and fetches them.
    let r = sync_in(&env, None, &[]).await;
    r.expect_code(0).assert_clean();
    assert!(r.json()["diff_stats"].is_object(), "{}", r.stdout);

    // A partial first sync counts as finished: the next one is not first.
    let server = serve(&World::base()).await;
    mount_line_counts(&server).await;
    let env = Env::new(&server);
    let partial = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let on = std::sync::Arc::clone(&partial);
    Mock::given(method("GET"))
        .and(path("/repos/acme/widget/commits"))
        .and(move |_: &wiremock::Request| on.load(std::sync::atomic::Ordering::SeqCst))
        .respond_with(ResponseTemplate::new(404))
        .with_priority(1)
        .mount(&server)
        .await;
    sync_in(&env, None, &["--no-diff-stats"])
        .await
        .expect_code(4);
    partial.store(false, std::sync::atomic::Ordering::SeqCst);
    let r = sync_in(&env, None, &["--full"]).await;
    r.expect_code(0).assert_clean();
    assert!(r.json()["diff_stats"].is_null(), "{}", r.stdout);
}

/// GitHub's answer to a request over the secondary rate limit: 403 with
/// requests left in the primary limit and no `Retry-After`.
fn secondary_limit() -> ResponseTemplate {
    ResponseTemplate::new(403)
        .insert_header("x-ratelimit-remaining", "17")
        .set_body_json(json!({
            "message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
            "documentation_url": "https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"
        }))
}

/// Index of the `n`th (from 0) request to `endpoint` whose `q` contains
/// `query`.
fn nth_search(reqs: &[wiremock::Request], endpoint: &str, query: &str, n: usize) -> usize {
    reqs.iter()
        .enumerate()
        .filter(|(_, r)| r.url.path() == endpoint)
        .filter(|(_, r)| {
            r.url
                .query_pairs()
                .any(|(k, v)| k == "q" && v.contains(query))
        })
        .nth(n)
        .map_or_else(
            || panic!("no request {n} to {endpoint} for {query}"),
            |(i, _)| i,
        )
}

/// The sync report without times and request statistics, with coverage in
/// a fixed order (the phases finish in any order).
fn sync_result(r: &Run) -> Value {
    let mut v = without_times(r.json());
    let report = v.as_object_mut().unwrap();
    report.remove("api");
    let coverage = report["coverage"].as_array_mut().unwrap();
    for c in coverage.iter_mut() {
        c.as_object_mut().unwrap().remove("recorded_at");
    }
    coverage.sort_by_key(|c| (c["kind"].to_string(), c["start"].to_string()));
    v
}

/// Serves `world` anew, with the first search to `endpoint` for `query`
/// over the secondary rate limit, and runs a first sync in a new cache.
async fn sync_with_one_search_limited(
    server: &wiremock::MockServer,
    world: &World,
    endpoint: &str,
    query: &str,
) -> (Env, Run) {
    reserve(server, world).await;
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(query_param_contains("q", query))
        .respond_with(secondary_limit())
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    let env = Env::new(server);
    // The wait is 60 times the backoff base: 1.5 s here.
    let mut c = env.command(&[
        "sync", "--since", SINCE, "--until", UNTIL, "--format", "json",
    ]);
    c.env("GHUSAGE_BACKOFF_MS", "25");
    let r = env.run_command(c).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["api"]["rate_limit_waits"], 1, "{query}");
    (env, r)
}

/// Pull requests found by one search are fetched while the next search
/// waits out the secondary rate limit, and the result is the same as
/// without the wait.
#[tokio::test(flavor = "multi_thread")]
async fn pull_requests_found_are_fetched_while_another_search_waits() {
    let world = World::base();
    let server = serve(&world).await;
    let unlimited = Env::new(&server);
    let expected = unlimited.sync(&[]).await;
    expected.expect_code(0).assert_clean();

    let query = format!("reviewed-by:{ME} is:pr");
    let (env, r) = sync_with_one_search_limited(&server, &world, "/search/issues", &query).await;
    let reqs = requests(&server).await;
    let retried = nth_search(&reqs, "/search/issues", &query, 1);
    let first_detail = reqs
        .iter()
        .position(|r| {
            r.url.path() == "/graphql" && {
                let body = body_str(r);
                body.contains("query PullRequestDetail") || body.contains("query PullRequestBatch")
            }
        })
        .expect("pull requests fetched");
    assert!(
        first_detail < retried,
        "fetching pull requests waited for the retried search"
    );
    assert_eq!(r.json()["pull_requests_fetched"], 3);

    assert_eq!(sync_result(&r), sync_result(&expected));
    for cmd in REPORTS {
        let a = unlimited.offline_json(&[cmd]).await;
        let b = env.offline_json(&[cmd]).await;
        a.expect_code(0);
        b.expect_code(0);
        assert_eq!(without_times(a.json()), without_times(b.json()), "{cmd}");
    }
}

/// While one search waits out the secondary rate limit, the default
/// branches of the repositories pushed to are already listed, and the
/// result is the same as without the wait.
#[tokio::test(flavor = "multi_thread")]
async fn a_search_waiting_for_the_rate_limit_does_not_hold_up_listing_commits() {
    let world = World::base();
    let server = serve(&world).await;
    let unlimited = Env::new(&server);
    let expected = unlimited.sync(&[]).await;
    expected.expect_code(0).assert_clean();

    for (endpoint, query) in [
        ("/search/issues", format!("involves:{ME} is:issue")),
        ("/search/issues", format!("involves:{ME} is:pr")),
        ("/search/commits", format!("author:{ME}")),
    ] {
        let (env, r) = sync_with_one_search_limited(&server, &world, endpoint, &query).await;
        let reqs = requests(&server).await;
        let retried = nth_search(&reqs, endpoint, &query, 1);
        let first_listing = reqs
            .iter()
            .position(|r| {
                let p = r.url.path();
                p.starts_with("/repos/") && p.ends_with("/commits")
            })
            .expect("default branches listed");
        assert!(
            first_listing < retried,
            "listing commits waited for the retried search ({query})"
        );

        assert_eq!(sync_result(&r), sync_result(&expected), "{query}");
        for cmd in REPORTS {
            let a = unlimited.offline_json(&[cmd]).await;
            let b = env.offline_json(&[cmd]).await;
            a.expect_code(0);
            b.expect_code(0);
            assert_eq!(
                without_times(a.json()),
                without_times(b.json()),
                "{query}: {cmd}"
            );
        }
    }
}

/// Answers `GET /search/issues` like GitHub does: only the items whose
/// current update time is inside the query's `updated:` range
/// (`lo..hi` or `>=lo`, whole seconds). The items in `moved` are updated
/// once more while the sync runs: from the first search on, their update
/// time is one second after that search arrived.
struct UpdatedDuringSync {
    items: Vec<Value>,
    moved: Vec<&'static str>,
    updated_at: std::sync::Arc<std::sync::Mutex<Option<jiff::Timestamp>>>,
}

impl wiremock::Respond for UpdatedDuringSync {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let moved_to = *self.updated_at.lock().unwrap().get_or_insert_with(|| {
            let next = jiff::Timestamp::now().as_second() + 1;
            jiff::Timestamp::from_second(next).unwrap()
        });
        let q = request
            .url
            .query_pairs()
            .find(|(k, _)| k == "q")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let range = q
            .split_whitespace()
            .find_map(|t| t.strip_prefix("updated:"))
            .unwrap_or_else(|| panic!("search without updated: {q}"));
        let time = |s: &str| s.parse::<jiff::Timestamp>().unwrap();
        let (lo, hi) = if let Some(lo) = range.strip_prefix(">=") {
            (time(lo), None)
        } else {
            let (lo, hi) = range.split_once("..").unwrap();
            (time(lo), Some(time(hi)))
        };
        let items: Vec<Value> = self
            .items
            .iter()
            .cloned()
            .map(|mut item| {
                if self.moved.contains(&item["node_id"].as_str().unwrap()) {
                    item["updated_at"] = json!(moved_to.to_string());
                }
                item
            })
            .filter(|item| {
                let updated = time(item["updated_at"].as_str().unwrap());
                updated >= lo && hi.is_none_or(|hi| updated <= hi)
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(search_page(items))
    }
}

/// A pull request and an issue updated after the sync started, but before
/// they were searched, are still found: GitHub matches `updated:` against
/// the current update time, so a search that stops at the start of the
/// sync would miss them while the range is recorded as complete.
#[tokio::test(flavor = "multi_thread")]
async fn items_updated_during_a_sync_are_still_found() {
    let world = World::base();
    let server = serve(&world).await;
    let updated_at = std::sync::Arc::default();
    let pr =
        |p: &PrFixture| search_issue(&p.owner, &p.name, p.number, &p.node_id, true, &p.updated_at);
    let issue = |i: &IssueFixture| {
        search_issue(
            &i.owner,
            &i.name,
            i.number,
            &i.node_id,
            false,
            &i.updated_at,
        )
    };
    for (q, items) in [
        (
            format!("involves:{ME} is:pr"),
            world.prs.iter().map(pr).collect::<Vec<_>>(),
        ),
        (
            format!("reviewed-by:{ME} is:pr"),
            world
                .prs
                .iter()
                .filter(|p| p.reviewed_by_me)
                .map(pr)
                .collect(),
        ),
        (
            format!("involves:{ME} is:issue"),
            world.issues.iter().map(issue).collect(),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param_contains("q", q))
            .respond_with(UpdatedDuringSync {
                items,
                // Pull request C (reviewed by me) and issue 1 (mine).
                moved: vec!["PR_C", "I_1"],
                updated_at: std::sync::Arc::clone(&updated_at),
            })
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let env = Env::new(&server);

    let r = env.sync(&[]).await;
    r.expect_code(0).assert_clean();
    let first = r.json();
    assert_eq!(first["status"], "complete");

    // Coverage is complete, so every item updated in the period must be
    // there.
    let metrics = reported_items(&env).await;
    assert_eq!(first["pull_requests_found"], 3);
    assert_eq!(first["pull_requests_fetched"], 3);
    assert_eq!(first["issues_fetched"], 2);

    // The same sync again finds the same items and counts nothing twice.
    let r = env.sync(&["--full"]).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["status"], "complete");
    assert_eq!(r.json()["pull_requests_found"], 3);
    assert_eq!(reported_items(&env).await, metrics);
}

/// Checks that every pull request and issue of the period is reported,
/// with complete coverage, and returns the overview metrics.
async fn reported_items(env: &Env) -> Value {
    let p = env.offline_json(&["prs"]).await;
    p.expect_code(0);
    let p = p.json();
    assert_eq!(coverage_status(&p, "pull_requests"), "complete");
    assert_eq!(pr_numbers(&p), vec![3, 10, 20]);
    let i = env.offline_json(&["issues"]).await;
    i.expect_code(0);
    let i = i.json();
    let mut issues: Vec<u64> = i["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    issues.sort_unstable();
    assert_eq!(coverage_status(&i, "issues"), "complete");
    assert_eq!(issues, vec![5, 7]);
    let o = env.offline_json(&["overview"]).await;
    o.expect_code(0);
    o.json()["metrics"].clone()
}
