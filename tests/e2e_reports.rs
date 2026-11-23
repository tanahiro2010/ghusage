//! End-to-end acceptance tests of the report commands and their options,
//! run on the built binary against the mocked GitHub API.
//!
//! The fixture is described in `support::World::base` (and the builders it
//! uses). Every expected value below is worked out by hand from that
//! description and the metric definitions in `docs/reference/metrics.md`,
//! not from the implementation.

mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use support::*;
use wiremock::matchers::{method, path, query_param, query_param_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Every period report command.
const ALL_REPORTS: [&str; 10] = [
    "overview", "daily", "weekly", "monthly", "yearly", "repos", "prs", "reviews", "comments",
    "issues",
];

fn period_args<'a>(cmd: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = cmd.to_vec();
    v.extend_from_slice(&["--since", SINCE, "--until", UNTIL, "--offline"]);
    v.extend_from_slice(extra);
    v
}

fn approx(v: &Value, expected: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - expected).abs() < 1e-9)
}

#[track_caller]
fn assert_stats(s: &Value, count: u64, min: f64, median: f64, mean: f64, p90: f64, max: f64) {
    assert_eq!(s["count"], count, "{s}");
    for (k, want) in [
        ("min", min),
        ("median", median),
        ("mean", mean),
        ("p90", p90),
        ("max", max),
    ] {
        assert!(approx(&s[k], want), "{k}: want {want}, got {s}");
    }
}

/// Numbers of the `pull_requests` array in report order (not sorted).
fn ordered_numbers(report: &Value) -> Vec<u64> {
    report["pull_requests"]
        .as_array()
        .expect("pull_requests")
        .iter()
        .map(|p| p["number"].as_u64().unwrap())
        .collect()
}

// ---------------------------------------------------------------------------
// Default command, weekly and monthly
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn bare_command_is_the_overview() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    let bare = env
        .run(&[
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--offline",
            "--format",
            "json",
        ])
        .await;
    bare.expect_code(0).assert_clean();
    let overview = env.offline_json(&["overview"]).await;
    overview.expect_code(0);
    assert_eq!(bare.json(), overview.json());
    // The base fixture has 6 commits of mine (see the initial sync test).
    assert_eq!(bare.json()["metrics"]["commits"], 6);

    let bare_table = env
        .run(&["--since", SINCE, "--until", UNTIL, "--offline"])
        .await;
    bare_table.expect_code(0).assert_clean();
    let table = env.run(&period_args(&["overview"], &[])).await;
    assert_eq!(bare_table.stdout, table.stdout);
    assert!(
        bare_table.stdout.starts_with("Overview"),
        "{}",
        bare_table.stdout
    );
}

/// The base world plus my commit sha 6 in me/dotfiles on the leap day
/// 2024-02-29T12:00Z, synced from Monday 2024-02-26.
#[tokio::test(flavor = "multi_thread")]
async fn weekly_and_monthly_reports_bucket_the_fixture() {
    let mut world = World::base();
    let d = dotfiles();
    world.commits.push(search_commit(
        &d.rest,
        &sha(6),
        Some((ME, ME_ID, "User")),
        "2024-02-29T12:00:00Z",
        1,
    ));
    let server = serve(&world).await;
    let env = Env::new(&server);
    let since = "2024-02-26";
    env.run(&[
        "sync", "--since", since, "--until", UNTIL, "--format", "json",
    ])
    .await
    .expect_code(0)
    .assert_clean();

    let report = |cmd: &'static str| {
        let env = &env;
        async move {
            let r = env
                .run(&[
                    cmd,
                    "--since",
                    since,
                    "--until",
                    UNTIL,
                    "--offline",
                    "--format",
                    "json",
                ])
                .await;
            r.expect_code(0).assert_clean();
            r.json()
        }
    };
    // Commits by UTC day: 02-29 sha 6; 03-04 sha 1, 11; 03-05 sha 2, 12;
    // 03-06 sha 21; 03-07 sha 3. Total 7.
    // Weeks start on Monday: 2024-W09 is 02-26..03-04, 2024-W10 is
    // 03-04..03-11 (the period ends at the end of Sunday 03-10).
    let w = report("weekly").await;
    assert_eq!(w["granularity"], "week");
    let buckets: Vec<(String, String, String, u64)> = w["activity_breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["bucket"]["label"].as_str().unwrap().to_string(),
                b["bucket"]["start"].as_str().unwrap().to_string(),
                b["bucket"]["end"].as_str().unwrap().to_string(),
                b["summary"]["commits"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        buckets,
        vec![
            (
                "2024-W09".to_string(),
                "2024-02-26T00:00:00Z".to_string(),
                "2024-03-04T00:00:00Z".to_string(),
                1
            ),
            (
                "2024-W10".to_string(),
                "2024-03-04T00:00:00Z".to_string(),
                "2024-03-11T00:00:00Z".to_string(),
                6
            ),
        ]
    );
    assert_eq!(w["metrics"]["commits"], 7);
    // PR B merged on 03-06, in week 10.
    assert_eq!(bucket(&w, "2024-03-04")["summary"]["prs_merged"], 1);

    // Calendar months: February is clipped to start on 02-26, March runs
    // from 03-01 to the end of the period.
    let m = report("monthly").await;
    assert_eq!(m["granularity"], "month");
    let buckets: Vec<(String, String, String, u64)> = m["activity_breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["bucket"]["label"].as_str().unwrap().to_string(),
                b["bucket"]["start"].as_str().unwrap().to_string(),
                b["bucket"]["end"].as_str().unwrap().to_string(),
                b["summary"]["commits"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        buckets,
        vec![
            (
                "2024-02".to_string(),
                "2024-02-26T00:00:00Z".to_string(),
                "2024-03-01T00:00:00Z".to_string(),
                1
            ),
            (
                "2024-03".to_string(),
                "2024-03-01T00:00:00Z".to_string(),
                "2024-03-11T00:00:00Z".to_string(),
                6
            ),
        ]
    );
    assert_eq!(m["metrics"]["commits"], 7);

    // One calendar year, clipped to the period: all 7 commits in 2024.
    let y = report("yearly").await;
    assert_eq!(y["granularity"], "year");
    let years: Vec<(String, u64)> = y["activity_breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["bucket"]["label"].as_str().unwrap().to_string(),
                b["summary"]["commits"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(years, vec![("2024".to_string(), 7)]);

    // The table forms run and show the week and month labels.
    for (cmd, labels) in [
        ("weekly", ["2024-W09", "2024-W10"]),
        ("monthly", ["2024-02", "2024-03"]),
        ("yearly", ["2024", "Yearly"]),
    ] {
        let t = env
            .run(&[cmd, "--since", since, "--until", UNTIL, "--offline"])
            .await;
        t.expect_code(0).assert_clean();
        for l in labels {
            assert!(t.stdout.contains(l), "{cmd} table lacks {l}:\n{}", t.stdout);
        }
    }
}

// ---------------------------------------------------------------------------
// --user, --repo, --timezone and repository filters
// ---------------------------------------------------------------------------

/// Mounts what a sync for `--user bob` needs on top of the base world:
/// bob's profile, his (empty) list of owned repositories, his commit sha 50
/// in acme/widget (2024-03-05T10:00Z), the pull requests that involve him
/// (acme/widget#10 where he commented and approved, and his own #20; #10 is
/// also the one he reviewed) and the issue that involves him (acme/widget#7).
async fn mount_bob(server: &MockServer) {
    let world = World::base();
    Mock::given(method("GET"))
        .and(path("/users/bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "bob", "id": 2, "node_id": "U_bob", "type": "User", "name": "Bob"
        })))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/bob/repos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(1)
        .mount(server)
        .await;
    let w = widget();
    let bob = Some(("bob", "U_bob", "User"));
    Mock::given(method("GET"))
        .and(path("/search/commits"))
        .and(query_param_contains("q", "author:bob"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(search_page(vec![search_commit(
                &w.rest,
                &sha(50),
                bob,
                "2024-03-05T10:00:00Z",
                1,
            )])),
        )
        .with_priority(1)
        .mount(server)
        .await;
    // Default branches: only acme/widget has a commit of bob's.
    for r in &world.repos {
        let items = if r.name == "widget" {
            vec![list_commit(
                &w.rest,
                &sha(50),
                bob,
                "2024-03-05T10:00:00Z",
                1,
            )]
        } else {
            vec![]
        };
        Mock::given(method("GET"))
            .and(path(format!("/repos/{}/{}/commits", r.owner, r.name)))
            .and(query_param("author", "bob"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(items)))
            .with_priority(1)
            .mount(server)
            .await;
    }
    let pr_item = |n: u64| {
        let p = world.prs.iter().find(|p| p.number == n).unwrap();
        search_issue(&p.owner, &p.name, p.number, &p.node_id, true, &p.updated_at)
    };
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param_contains("q", "involves:bob is:pr"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(search_page(vec![pr_item(10), pr_item(20)])),
        )
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param_contains("q", "reviewed-by:bob is:pr"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_page(vec![pr_item(10)])))
        .with_priority(1)
        .mount(server)
        .await;
    let i = issue_2();
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param_contains("q", "involves:bob is:issue"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(search_page(vec![search_issue(
                &i.owner,
                &i.name,
                i.number,
                &i.node_id,
                false,
                &i.updated_at,
            )])),
        )
        .with_priority(1)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn user_option_analyzes_another_account_only() {
    let server = serve(&World::base()).await;
    mount_bob(&server).await;
    let env = Env::new(&server);

    let r = env.sync(&["--user", "bob"]).await;
    r.expect_code(0).assert_clean();
    assert_eq!(r.json()["status"], "complete", "{}", r.stdout);

    let bob = env.offline_json(&["overview", "--user", "bob"]).await;
    bob.expect_code(0).assert_clean();
    let bob = bob.json();
    assert_eq!(bob["account"]["login"], "bob");
    assert_eq!(bob["account"]["is_authenticated_user"], false);
    let m = &bob["metrics"];
    // sha 50 (commit search / default branch) and sha 31 inside his PR #20.
    assert_eq!(m["commits"], 2);
    assert_eq!(m["merge_commits"], 0);
    // #20 opened 03-05.
    assert_eq!(m["prs_created"], 1);
    assert_eq!(m["prs_merged"], 0);
    // His approval of #10 on 03-06 (no body).
    assert_eq!(
        m["reviews_submitted"],
        json!({"total": 1, "approved": 1, "changes_requested": 0, "commented": 0, "dismissed_unknown_state": 0})
    );
    assert_eq!(m["reviewed_prs"], 1);
    // My CHANGES_REQUESTED review on his #20.
    assert_eq!(
        m["reviews_received"],
        json!({"total": 1, "approved": 0, "changes_requested": 1, "commented": 0, "dismissed_unknown_state": 0})
    );
    // IC_A2 on #10 and IC_C1 on #20.
    assert_eq!(
        m["comments_written"],
        json!({"total": 2, "conversation": 2, "inline": 0, "review_summary": 0})
    );
    // On #20: my inline PRRC_C1 and the summary "Needs tests".
    assert_eq!(
        m["comments_received"],
        json!({"total": 2, "conversation": 0, "inline": 1, "review_summary": 1})
    );
    assert_eq!(m["pr_commits"], 1);
    // acme/widget#7: opened 02-20 (before the period); I closed it, bob
    // reopened it on 03-06.
    assert_eq!(m["issues_opened"], 0);
    assert_eq!(m["issues_closed"], 0);
    assert_eq!(m["issues_reopened"], 1);
    assert_eq!(m["active_prs"], 2);
    // 03-04 (IC_A2), 03-05 (#20, sha 31, sha 50), 03-06 (approval, reopen).
    assert_eq!(m["active_days"], 3);
    assert_eq!(m["last_activity"], "2024-03-06T10:00:00Z");
    // Only bob's repositories with activity: acme/widget.
    let names: Vec<&str> = bob["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["acme/widget"]);

    // Syncing my own account into the same cache changes nothing for bob,
    // and my numbers are the base world's.
    env.sync(&[]).await.expect_code(0);
    let again = env
        .offline_json(&["overview", "--user", "bob"])
        .await
        .json();
    assert_eq!(again["metrics"], bob["metrics"]);
    let me = env.offline_json(&["overview"]).await.json();
    assert_eq!(me["account"]["login"], ME);
    assert_eq!(me["metrics"]["commits"], 6);
    assert_eq!(me["metrics"]["prs_created"], 2);
    assert_eq!(me["metrics"]["reviews_submitted"]["total"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn user_option_counts_direct_collaborator_commits_found_by_search() {
    let server = serve(&World::base()).await;

    // Bob is a collaborator on acme/collab and pushes directly to its
    // default branch. GitHub does not include that repository in
    // GET /users/bob/repos, and Bob has no pull request or issue there, but
    // commit search still finds his commit.
    let collab = support::rest_repo("R_collab", "acme", "Organization", "collab", false);
    let bob = Some(("bob", "U_bob", "User"));
    Mock::given(method("GET"))
        .and(path("/search/commits"))
        .and(query_param_contains("q", "author:bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_page(vec![
            search_commit(
                &support::rest_repo("R_widget", "acme", "Organization", "widget", false),
                &sha(50),
                bob,
                "2024-03-05T10:00:00Z",
                1,
            ),
            search_commit(&collab, &sha(51), bob, "2024-03-06T10:00:00Z", 1),
        ])))
        // Mounted before `mount_bob` at its priority, so this response wins
        // over its commit search (the first mock mounted wins a tie, and a
        // lower number is a higher priority).
        .with_priority(1)
        .mount(&server)
        .await;
    mount_bob(&server).await;

    let env = Env::new(&server);
    let r = env.sync(&["--user", "bob"]).await;
    r.expect_code(0).assert_clean();

    let bob_report = env.offline_json(&["overview", "--user", "bob"]).await;
    bob_report.expect_code(0).assert_clean();
    let bob_report = bob_report.json();
    assert_eq!(bob_report["metrics"]["commits"], 3);
    assert!(
        bob_report["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["full_name"] == "acme/collab"),
        "{}",
        bob_report["repositories"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn repo_and_timezone_options_change_the_report() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    // --repo acme/widget: sha 1, 11, 12; PR #10 created; my review on #20;
    // my close of #7. Nothing from me/dotfiles.
    let r = env
        .offline_json(&["overview", "--repo", "acme/widget"])
        .await;
    r.expect_code(0).assert_clean();
    let r = r.json();
    let m = &r["metrics"];
    assert_eq!(m["commits"], 3);
    assert_eq!(m["prs_created"], 1);
    assert_eq!(m["prs_merged"], 0);
    assert_eq!(m["reviews_submitted"]["total"], 1);
    assert_eq!(m["issues_opened"], 0);
    assert_eq!(m["issues_closed"], 1);
    let names: Vec<&str> = r["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["acme/widget"]);

    // Ownership and owner filters: me/dotfiles is mine (sha 2, 3, 21),
    // acme/widget is not (sha 1, 11, 12); acme/secret is the only private
    // repository and has no activity of mine.
    for (args, commits) in [
        (vec!["--ownership", "owned"], 3),
        (vec!["--ownership", "external"], 3),
        (vec!["--owner", "acme"], 3),
        (vec!["--owner", ME], 3),
        (vec!["--visibility", "private"], 0),
        (vec!["--visibility", "public"], 6),
    ] {
        let mut cmd = vec!["overview"];
        cmd.extend(args.iter().copied());
        let o = env.offline_json(&cmd).await;
        o.expect_code(0);
        assert_eq!(o.json()["metrics"]["commits"], commits, "{args:?}");
    }
    let owned = env
        .offline_json(&["repos", "--ownership", "owned"])
        .await
        .json();
    assert_eq!(owned["repositories"][0]["full_name"], "me/dotfiles");
    assert_eq!(owned["repositories"].as_array().unwrap().len(), 1);

    // --timezone Asia/Tokyo (UTC+9): the period becomes
    // 03-03T15:00Z..03-10T15:00Z and every commit moves 9 hours later:
    // sha 1 (03-04 10:00Z) and sha 11 (08:30Z) stay on 03-04; sha 2 and 12
    // stay on 03-05; sha 21 (03-06 07:50Z) on 03-06; sha 3 (03-07 23:30Z)
    // moves to 03-08 08:30 local time.
    let tokyo = env
        .offline_json(&["daily", "--timezone", "Asia/Tokyo"])
        .await;
    tokyo.expect_code(0).assert_clean();
    let tokyo = tokyo.json();
    assert_eq!(tokyo["timezone"], "Asia/Tokyo");
    assert_eq!(tokyo["period"]["start"], "2024-03-04T00:00:00+09:00");
    assert_eq!(tokyo["period"]["end"], "2024-03-11T00:00:00+09:00");
    let utc = env.offline_json(&["daily"]).await.json();
    assert_eq!(utc["timezone"], "UTC");
    for (date, in_tokyo, in_utc) in [
        ("2024-03-04", 2, 2),
        ("2024-03-05", 2, 2),
        ("2024-03-06", 1, 1),
        ("2024-03-07", 0, 1),
        ("2024-03-08", 1, 0),
    ] {
        assert_eq!(
            bucket(&tokyo, date)["summary"]["commits"],
            in_tokyo,
            "Tokyo {date}"
        );
        assert_eq!(
            bucket(&utc, date)["summary"]["commits"],
            in_utc,
            "UTC {date}"
        );
    }
    assert_eq!(tokyo["metrics"]["commits"], 6);
    // Tokyo buckets start at local midnight.
    assert_eq!(
        bucket(&tokyo, "2024-03-08")["bucket"]["start"],
        "2024-03-07T15:00:00Z"
    );
}

// ---------------------------------------------------------------------------
// --no-cache
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn no_cache_reports_match_cached_reports_and_leave_no_database() {
    let server = serve(&World::base()).await;
    let cached = Env::new(&server);
    cached.sync(&[]).await.expect_code(0);
    let fresh = Env::new(&server);

    for cmd in ["overview", "daily", "prs", "repos"] {
        let a = cached.offline_json(&[cmd]).await;
        a.expect_code(0);
        let b = fresh
            .run(&[
                cmd,
                "--since",
                SINCE,
                "--until",
                UNTIL,
                "--no-cache",
                "--format",
                "json",
            ])
            .await;
        b.expect_code(0).assert_clean();
        let (mut a, mut b) = (a.json(), b.json());
        // The time of the last sync is the only thing allowed to differ.
        a.as_object_mut().unwrap().remove("synced_at");
        b.as_object_mut().unwrap().remove("synced_at");
        assert_eq!(a, b, "{cmd}: --no-cache differs from the cached report");
    }
    // The fresh run fetched the whole base world.
    let o = fresh
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--no-cache",
            "--format",
            "json",
        ])
        .await
        .json();
    assert_eq!(o["metrics"]["commits"], 6);
    for kind in ["commits", "pull_requests", "issues"] {
        assert_eq!(coverage_status(&o, kind), "complete", "{kind}");
    }
    // No cache database was created in the data directory.
    let files: Vec<String> = std::fs::read_dir(fresh.data_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !files.iter().any(|f| f.starts_with("ghusage.db")),
        "--no-cache wrote a cache: {files:?}"
    );

    // --no-cache never touches an existing cache: with new data on GitHub
    // (my commit sha 7 in me/dotfiles on 03-08), the fresh report sees 7
    // commits while the local cache still has 6.
    let before = cached.offline_json(&["overview"]).await.json();
    let mut newer = World::base();
    let d = dotfiles();
    newer.commits.push(search_commit(
        &d.rest,
        &sha(7),
        Some((ME, ME_ID, "User")),
        "2024-03-08T10:00:00Z",
        1,
    ));
    reserve(&server, &newer).await;
    let now = cached
        .run(&[
            "overview",
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--no-cache",
            "--format",
            "json",
        ])
        .await;
    now.expect_code(0);
    assert_eq!(now.json()["metrics"]["commits"], 7);
    let after = cached.offline_json(&["overview"]).await.json();
    assert_eq!(after, before);
    assert_eq!(after["metrics"]["commits"], 6);

    // --offline and --no-cache contradict each other.
    cached
        .run(&["overview", "--offline", "--no-cache"])
        .await
        .expect_code(2);
}

// ---------------------------------------------------------------------------
// Pull request lists: sorting, details, all participants
// ---------------------------------------------------------------------------

/// The base world with:
/// - five more conversation comments by bob on #20 (IC_C2..IC_C6,
///   2024-03-05T12:00Z..12:04Z);
/// - two approvals without a body on my #3: dave at 2024-03-06T08:30Z and
///   erin at 08:40Z;
/// - acme/widget#30 by alice, opened 2024-02-01T09:00Z (before the period)
///   with a review request to me at 09:01Z; in the period only alice's
///   commit sha 41 (03-06 10:00Z) and bob's comment IC_D1 (03-06 11:00Z).
///
/// Per pull request in the period (events by anyone, CI excluded):
/// - #10: opened, review request, commits 11/12/13/14, reviews by bob and
///   carol, carol's summary, inline PRRC_A1/A2, comments IC_A1/A2/A3 = 14
///   events; 2 reviews; 6 comments; 4 commits; participants me, bob, carol,
///   dependabot, github-actions = 5 (sha 14 has no known author); 1 thread;
///   active on 03-04, 03-05, 03-06.
/// - #20: opened, sha 31, IC_C1..IC_C6, my review, its summary, PRRC_C1 =
///   11 events; 1 review; 8 comments; 1 commit; participants bob, me; 1
///   thread; active on 03-05 and 03-07.
/// - #3: opened, sha 21, two approvals, merged, closed = 6 events; 2
///   reviews; 0 comments; 1 commit; participants me, dave, erin; active on
///   03-06.
/// - #30: sha 41 and IC_D1 = 2 events, none of them mine.
fn world_for_pr_lists() -> World {
    let mut w = World::base();
    {
        let c = w.pr_mut(20);
        let nodes = c.gql["comments"]["nodes"].as_array_mut().unwrap();
        for i in 2..=6 {
            nodes.push(issue_comment(
                &format!("IC_C{i}"),
                user("bob"),
                &format!("2024-03-05T12:0{}:00Z", i - 2),
                None,
            ));
        }
    }
    {
        let b = w.pr_mut(3);
        b.gql["reviews"] = connection(vec![
            review(
                "PRR_B1",
                "APPROVED",
                "",
                "2024-03-06T08:30:00Z",
                user("dave"),
                vec![],
            ),
            review(
                "PRR_B2",
                "APPROVED",
                "",
                "2024-03-06T08:40:00Z",
                user("erin"),
                vec![],
            ),
        ]);
    }
    w.prs.push(PrFixture {
        owner: "acme".into(),
        name: "widget".into(),
        number: 30,
        node_id: "PR_D".into(),
        updated_at: "2024-03-06T11:00:00Z".into(),
        reviewed_by_me: false,
        check_runs: vec![],
        gql: json!({
            "id": "PR_D",
            "number": 30,
            "title": "Alice's cleanup",
            "url": "https://github.com/acme/widget/pull/30",
            "state": "OPEN",
            "isDraft": false,
            "createdAt": "2024-02-01T09:00:00Z",
            "updatedAt": "2024-03-06T11:00:00Z",
            "closedAt": null,
            "mergedAt": null,
            "headRefOid": sha(41),
            "author": user("alice"),
            "mergedBy": null,
            "headCommit": { "nodes": [ { "commit": { "oid": sha(41), "statusCheckRollup": null } } ] },
            "commits": connection(vec![
                pr_commit("acme/widget", 30, &sha(41), Some(("alice", "U_alice")), "2024-03-06T10:00:00Z"),
            ]),
            "reviews": connection(vec![]),
            "reviewThreads": connection(vec![]),
            "comments": connection(vec![
                issue_comment("IC_D1", user("bob"), "2024-03-06T11:00:00Z", None),
            ]),
            "timelineItems": connection(vec![
                tl("ReviewRequestedEvent", "RRE_D1", "2024-02-01T09:01:00Z", user("alice"), json!({ "requestedReviewer": { "login": ME } })),
            ])
        }),
    });
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn prs_sort_orders_and_details_sections() {
    let server = serve(&world_for_pr_lists()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0).assert_clean();

    // Largest first; ties by repository then number.
    // activity: #10 14, #20 11, #3 6.
    // comments: #20 8, #10 6, #3 0.
    // reviews: #10 2 and #3 2 (acme/widget before me/dotfiles), #20 1.
    // created: #3 03-06, #20 03-05, #10 03-04.
    for (sort, expected) in [
        (None, vec![10, 20, 3]),
        (Some("activity"), vec![10, 20, 3]),
        (Some("comments"), vec![20, 10, 3]),
        (Some("reviews"), vec![10, 3, 20]),
        (Some("created"), vec![3, 20, 10]),
    ] {
        let mut cmd = vec!["prs"];
        if let Some(s) = sort {
            cmd.extend(["--sort", s]);
        }
        let r = env.offline_json(&cmd).await;
        r.expect_code(0);
        assert_eq!(ordered_numbers(&r.json()), expected, "--sort {sort:?}");
    }

    let r = env.offline_json(&["prs"]).await.json();
    let row = |n: u64| -> Value {
        r["pull_requests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["number"] == n)
            .unwrap()
            .clone()
    };
    for (n, events, reviews, comments, commits, participants, threads) in [
        (10, 14, 2, 6, 4, 5, 1),
        (20, 11, 1, 8, 1, 2, 1),
        (3, 6, 2, 0, 1, 3, 0),
    ] {
        let p = row(n);
        assert_eq!(p["events"], events, "#{n} events");
        assert_eq!(p["reviews"], reviews, "#{n} reviews");
        assert_eq!(p["comments"], comments, "#{n} comments");
        assert_eq!(p["commits"], commits, "#{n} commits");
        assert_eq!(p["participants"], participants, "#{n} participants");
        assert_eq!(p["threads"], threads, "#{n} threads");
    }

    // Intensity over #10, #20, #3 (nearest-rank p90 of three values is the
    // largest one).
    let i = &r["intensity"];
    assert_eq!(i["prs"], 3);
    // events 6, 11, 14
    assert_stats(&i["events_per_pr"], 3, 6.0, 11.0, 31.0 / 3.0, 14.0, 14.0);
    // reviews 1, 2, 2
    assert_stats(&i["reviews_per_pr"], 3, 1.0, 2.0, 5.0 / 3.0, 2.0, 2.0);
    // comments 0, 6, 8
    assert_stats(&i["comments_per_pr"], 3, 0.0, 6.0, 14.0 / 3.0, 8.0, 8.0);
    // commits 1, 1, 4
    assert_stats(&i["commits_per_pr"], 3, 1.0, 1.0, 2.0, 4.0, 4.0);
    // participants 2, 3, 5
    assert_stats(&i["participants_per_pr"], 3, 2.0, 3.0, 10.0 / 3.0, 5.0, 5.0);
    // threads 0, 1, 1
    assert_stats(&i["threads_per_pr"], 3, 0.0, 1.0, 2.0 / 3.0, 1.0, 1.0);
    // active days 1, 2, 3
    assert_stats(&i["active_days_per_pr"], 3, 1.0, 2.0, 2.0, 3.0, 3.0);

    // Timing over my pull requests active in the period, #10 and #3:
    // - first review by someone else: #10 carol 03-05 14:00 - 03-04 09:00 =
    //   29 h = 104400 s; #3 dave 08:30 - 08:00 = 1800 s
    // - creation to merge: only #3, 09:00 - 08:00 = 3600 s
    // - first review to my next event: #10 my inline reply 15:00 = 3600 s;
    //   #3 my merge 09:00 = 1800 s
    // - first to last event: #10 sha 11 03-04 08:30 to bob 03-06 10:00 =
    //   49.5 h = 178200 s; #3 sha 21 07:50 to the merge 09:00 = 4200 s
    let t = &r["timing"];
    assert_stats(
        &t["time_to_first_review"],
        2,
        1800.0,
        53100.0,
        53100.0,
        104400.0,
        104400.0,
    );
    assert_stats(
        &t["creation_to_merge"],
        1,
        3600.0,
        3600.0,
        3600.0,
        3600.0,
        3600.0,
    );
    assert_stats(
        &t["review_to_next_author_activity"],
        2,
        1800.0,
        2700.0,
        2700.0,
        3600.0,
        3600.0,
    );
    assert_stats(
        &t["first_to_last_activity"],
        2,
        4200.0,
        91200.0,
        91200.0,
        178200.0,
        178200.0,
    );

    // The table shows the extra sections only with --details.
    let plain = env.run(&period_args(&["prs"], &[])).await;
    plain.expect_code(0).assert_clean();
    assert!(!plain.stdout.contains("Activity per pull request"));
    let details = env.run(&period_args(&["prs", "--details"], &[])).await;
    details.expect_code(0).assert_clean();
    for section in [
        "Activity per pull request",
        "Lifecycle timing (your pull requests)",
        "Time to first review",
        "Creation to merge",
    ] {
        assert!(
            details.stdout.contains(section),
            "missing {section}:\n{}",
            details.stdout
        );
    }
    // Events: median 11.0, mean 10.3, p90 14.0, max 14.0 over 3 PRs.
    assert!(
        details
            .stdout
            .contains("median 11.0, mean 10.3, p90 14.0, max 14.0 (n=3)"),
        "{}",
        details.stdout
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prs_all_participants_adds_pull_requests_only_others_touched() {
    let server = serve(&world_for_pr_lists()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0).assert_clean();

    let mine = env.offline_json(&["prs"]).await.json();
    assert_eq!(pr_numbers(&mine), vec![3, 10, 20]);
    let all = env
        .offline_json(&["prs", "--all-participants"])
        .await
        .json();
    assert_eq!(pr_numbers(&all), vec![3, 10, 20, 30]);
    let d = all["pull_requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["number"] == 30)
        .unwrap();
    assert_eq!(d["events"], 2);
    assert_eq!(d["events_by_subject"], 0);
    assert_eq!(d["author"], "alice");
    // Activity order with #30 last (2 events).
    assert_eq!(ordered_numbers(&all), vec![10, 20, 3, 30]);
    let table = env
        .run(&period_args(&["prs", "--all-participants"], &[]))
        .await;
    table.expect_code(0);
    assert!(table.stdout.contains("Alice's cleanup"), "{}", table.stdout);
    let default_table = env.run(&period_args(&["prs"], &[])).await;
    assert!(!default_table.stdout.contains("Alice's cleanup"));

    // Overview: 3 pull requests with my activity, 4 with anyone's.
    let o = env.offline_json(&["overview"]).await.json();
    assert_eq!(o["metrics"]["active_prs"], 3);
    assert_eq!(o["metrics"]["active_prs_all_participants"], 4);
}

// ---------------------------------------------------------------------------
// Pull request detail JSON
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn pr_json_has_all_detail_fields() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    let pr = |n: &str, repo: &str| {
        let env = &env;
        let url = format!("https://github.com/{repo}/pull/{n}");
        async move {
            let r = env
                .run(&["pr", &url, "--offline", "--format", "json"])
                .await;
            r.expect_code(0).assert_clean();
            r.json()
        }
    };
    let a = pr("10", "acme/widget").await;
    for k in [
        "pr_id",
        "repository",
        "author",
        "state",
        "created_at",
        "merged_at",
        "commits",
        "reviews",
        "comments",
        "review_threads",
        "timeline",
        "coverage",
    ] {
        assert!(a.get(k).is_some(), "missing {k}: {a}");
    }
    assert_eq!(a["schema_version"], 1);
    assert_eq!(a["pr_id"], "PR_A");
    assert_eq!(a["repository"], "acme/widget");
    assert_eq!(a["number"], 10);
    assert_eq!(a["author"], ME);
    assert_eq!(a["state"], "open");
    assert_eq!(a["created_at"], "2024-03-04T09:00:00Z");
    assert!(chrono::DateTime::parse_from_rfc3339(a["created_at"].as_str().unwrap()).is_ok());
    // Not merged: null, not a placeholder.
    assert!(a["merged_at"].is_null(), "{}", a["merged_at"]);
    assert!(a["merged_by"].is_null());
    assert!(a["closed_at"].is_null());
    // Commits 11, 12, 13 and 14.
    assert_eq!(a["commits"], 4);
    assert_eq!(a["removed_commits"], 0);
    // bob APPROVED, carol CHANGES_REQUESTED; my thread-reply-only review is
    // not a review.
    assert_eq!(
        a["reviews"],
        json!({"total": 2, "approved": 1, "changes_requested": 1, "commented": 0, "dismissed_unknown_state": 0})
    );
    // IC_A1..A3; inline PRRC_A1, PRRC_A2; carol's summary.
    assert_eq!(
        a["comments"],
        json!({"total": 6, "conversation": 3, "inline": 2, "review_summary": 1})
    );
    assert_eq!(
        a["review_threads"],
        json!({"total": 1, "open": 1, "resolved": 0, "outdated": 0})
    );
    assert_eq!(a["coverage"], "complete");
    // Timings: see prs_sort_orders_and_details_sections.
    assert_eq!(
        a["timing"],
        json!({
            "time_to_first_review_secs": 104400,
            "creation_to_merge_secs": null,
            "review_to_next_author_activity_secs": 3600,
            "first_to_last_activity_secs": 178200
        })
    );
    assert!(a["timeline"].as_array().is_some_and(|t| !t.is_empty()));

    // A merged pull request has ISO 8601 merge and close times.
    let b = pr("3", "me/dotfiles").await;
    assert_eq!(b["state"], "merged");
    assert_eq!(b["merged_at"], "2024-03-06T09:00:00Z");
    assert_eq!(b["closed_at"], "2024-03-06T09:00:00Z");
    assert_eq!(b["merged_by"], ME);
    assert_eq!(b["timing"]["creation_to_merge_secs"], 3600);
    // Nobody else reviewed it.
    assert!(b["timing"]["time_to_first_review_secs"].is_null());
}

// ---------------------------------------------------------------------------
// CSV and table output agree with JSON
// ---------------------------------------------------------------------------

/// Parses CSV output with the `csv` crate into header -> column values.
fn parse_csv(s: &str) -> (Vec<String>, Vec<BTreeMap<String, String>>) {
    let mut rd = csv::Reader::from_reader(s.as_bytes());
    let headers: Vec<String> = rd.headers().unwrap().iter().map(String::from).collect();
    let rows = rd
        .records()
        .map(|r| {
            let r = r.unwrap_or_else(|e| panic!("bad CSV record ({e}):\n{s}"));
            assert_eq!(r.len(), headers.len(), "ragged CSV:\n{s}");
            headers
                .iter()
                .cloned()
                .zip(r.iter().map(String::from))
                .collect()
        })
        .collect();
    (headers, rows)
}

fn col_sum(rows: &[BTreeMap<String, String>], col: &str) -> u64 {
    rows.iter()
        .map(|r| {
            r[col]
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("{col}: {r:?}"))
        })
        .sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn csv_output_of_every_report_parses_and_matches_json() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    let mut parsed = BTreeMap::new();
    for cmd in ALL_REPORTS {
        let c = env.run(&period_args(&[cmd], &["--format", "csv"])).await;
        c.expect_code(0).assert_clean();
        let j = env.offline_json(&[cmd]).await;
        j.expect_code(0);
        let (headers, rows) = parse_csv(&c.stdout);
        assert!(
            headers
                .iter()
                .any(|h| h == "period_coverage" || h == "coverage"),
            "{cmd}: {headers:?}"
        );
        for r in &rows {
            let cov = r.get("period_coverage").or_else(|| r.get("coverage"));
            assert_eq!(cov.map(String::as_str), Some("complete"), "{cmd}: {r:?}");
        }
        parsed.insert(cmd, (rows, j.json()));
    }

    // Calendar reports: one row per bucket, adding up to the period totals
    // (commits 6, PRs created 2, PR commits 3).
    for (cmd, buckets) in [("overview", 7), ("daily", 7), ("weekly", 1), ("monthly", 1)] {
        let (rows, j) = &parsed[cmd];
        assert_eq!(rows.len(), buckets, "{cmd}");
        assert_eq!(
            rows.len(),
            j["activity_breakdown"].as_array().unwrap().len(),
            "{cmd}"
        );
        assert_eq!(col_sum(rows, "commits"), 6, "{cmd}");
        assert_eq!(col_sum(rows, "prs_created"), 2, "{cmd}");
        assert_eq!(col_sum(rows, "pr_commits"), 3, "{cmd}");
        assert_eq!(j["metrics"]["commits"], 6, "{cmd}");
    }
    let (weekly, _) = &parsed["weekly"];
    assert_eq!(weekly[0]["period"], "2024-W10");
    let (monthly, _) = &parsed["monthly"];
    assert_eq!(monthly[0]["period"], "2024-03");
    let (daily, dj) = &parsed["daily"];
    for (row, b) in daily
        .iter()
        .zip(dj["activity_breakdown"].as_array().unwrap())
    {
        assert_eq!(row["period"], b["bucket"]["label"].as_str().unwrap());
        assert_eq!(
            row["commits"].parse::<u64>().unwrap(),
            b["summary"]["commits"].as_u64().unwrap()
        );
    }

    // repos: acme/widget 3 commits (sha 1, 11, 12), me/dotfiles 3 (sha 2,
    // 3, 21).
    let (rows, j) = &parsed["repos"];
    let repos: BTreeMap<&str, (&str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r["repository"].as_str(),
                (r["commits"].as_str(), r["owned"].as_str()),
            )
        })
        .collect();
    assert_eq!(
        repos,
        BTreeMap::from([
            ("acme/widget", ("3", "false")),
            ("me/dotfiles", ("3", "true"))
        ])
    );
    assert_eq!(rows.len(), j["repositories"].as_array().unwrap().len());

    // prs: the same rows as JSON.
    let (rows, j) = &parsed["prs"];
    let mut numbers: Vec<u64> = rows.iter().map(|r| r["number"].parse().unwrap()).collect();
    numbers.sort();
    assert_eq!(numbers, vec![3, 10, 20]);
    for p in j["pull_requests"].as_array().unwrap() {
        let r = rows
            .iter()
            .find(|r| r["number"].parse::<u64>().ok() == p["number"].as_u64())
            .unwrap();
        for k in ["events", "commits", "reviews", "comments", "participants"] {
            assert_eq!(r[k], p[k].to_string(), "#{} {k}", p["number"]);
        }
        assert_eq!(r["state"], p["state"].as_str().unwrap());
    }
    let merged = rows.iter().find(|r| r["number"] == "3").unwrap();
    assert!(merged["merged_at"].starts_with("2024-03-06T09:00:00"));
    // Unknown values are empty cells: #10 is not merged.
    let open = rows.iter().find(|r| r["number"] == "10").unwrap();
    assert_eq!(open["merged_at"], "");

    // reviews: PRR_C1 submitted; PRR_A2 and PRR_A1 received.
    let (rows, _) = &parsed["reviews"];
    let dirs: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r["direction"].as_str(), r["reviewer"].as_str()))
        .collect();
    assert_eq!(
        dirs,
        vec![
            ("submitted", ME),
            ("received", "carol"),
            ("received", "bob")
        ]
    );

    // comments: 4 written by me, 4 received (see the initial sync test).
    let (rows, j) = &parsed["comments"];
    assert_eq!(rows.len(), 8);
    assert_eq!(rows.len(), j["comments"].as_array().unwrap().len());
    assert_eq!(
        rows.iter()
            .filter(|r| r["written_by_you"] == "true")
            .count(),
        4
    );

    // issues: acme/widget#7 (closed by me) and me/dotfiles#5 (opened and
    // closed by me).
    let (rows, _) = &parsed["issues"];
    let issues: Vec<(&str, &str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r["repository"].as_str(),
                r["number"].as_str(),
                r["opened_in_period"].as_str(),
                r["closed_in_period"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        issues,
        vec![
            ("acme/widget", "7", "false", "1"),
            ("me/dotfiles", "5", "true", "1")
        ]
    );

    // pr: one row per timeline event, same as JSON.
    let url = "https://github.com/acme/widget/pull/10";
    let c = env.run(&["pr", url, "--offline", "--format", "csv"]).await;
    c.expect_code(0).assert_clean();
    let (headers, rows) = parse_csv(&c.stdout);
    assert_eq!(
        headers,
        vec![
            "occurred_at",
            "actor",
            "actor_kind",
            "event_type",
            "detail",
            "removed_observed_at",
            "url"
        ]
    );
    let j = env
        .run(&["pr", url, "--offline", "--format", "json"])
        .await
        .json();
    let timeline = j["timeline"].as_array().unwrap();
    assert_eq!(rows.len(), timeline.len());
    for (r, e) in rows.iter().zip(timeline) {
        assert_eq!(r["event_type"], e["event_type"].as_str().unwrap());
        assert_eq!(r["actor"], e["actor"].as_str().unwrap());
        let t = chrono::DateTime::parse_from_rfc3339(&r["occurred_at"]).unwrap();
        let u = chrono::DateTime::parse_from_rfc3339(e["occurred_at"].as_str().unwrap()).unwrap();
        assert_eq!(t, u);
    }
}

/// The value of a `key  value` line of the overview table.
fn table_value<'a>(table: &'a str, key: &str) -> &'a str {
    table
        .lines()
        .find_map(|l| {
            let l = l.trim_start();
            l.strip_prefix(key)
                .filter(|rest| rest.starts_with("  "))
                .map(str::trim)
        })
        .unwrap_or_else(|| panic!("no {key:?} line in:\n{table}"))
}

fn first_number(s: &str) -> u64 {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|p| !p.is_empty())
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn table_numbers_match_json_metrics() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    let t = env.run(&period_args(&["overview"], &[])).await;
    t.expect_code(0).assert_clean();
    let t = t.stdout;
    let j = env.offline_json(&["overview"]).await.json();
    let m = &j["metrics"];
    let n = |v: &Value| v.as_u64().unwrap();
    assert_eq!(
        table_value(&t, "Commits"),
        format!("{} ({} merge)", n(&m["commits"]), n(&m["merge_commits"]))
    );
    for (key, field) in [
        ("PRs created", "prs_created"),
        ("PRs merged (yours)", "prs_merged"),
        ("Merges you performed", "merges_performed"),
        ("PRs closed without merge", "prs_closed_without_merge"),
        ("Review thread replies", "review_responses"),
        ("PR commits", "pr_commits"),
        ("Force pushes", "force_pushes"),
        ("PR internal events", "pr_internal_events"),
        ("Active days", "active_days"),
    ] {
        assert_eq!(first_number(table_value(&t, key)), n(&m[field]), "{key}");
    }
    assert_eq!(
        table_value(&t, "Reviews submitted"),
        format!(
            "{} (approved {}, changes requested {}, commented {}) on {} PRs",
            n(&m["reviews_submitted"]["total"]),
            n(&m["reviews_submitted"]["approved"]),
            n(&m["reviews_submitted"]["changes_requested"]),
            n(&m["reviews_submitted"]["commented"]),
            n(&m["reviewed_prs"])
        )
    );
    assert_eq!(
        first_number(table_value(&t, "Reviews received")),
        n(&m["reviews_received"]["total"])
    );
    for (key, field) in [
        ("Comments written", "comments_written"),
        ("Comments received", "comments_received"),
    ] {
        let c = &m[field];
        assert_eq!(
            table_value(&t, key),
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                n(&c["total"]),
                n(&c["conversation"]),
                n(&c["inline"]),
                n(&c["review_summary"])
            ),
            "{key}"
        );
    }
    assert_eq!(
        table_value(&t, "Issues"),
        format!(
            "opened {}, closed {}, reopened {}",
            n(&m["issues_opened"]),
            n(&m["issues_closed"]),
            n(&m["issues_reopened"])
        )
    );
    assert_eq!(
        table_value(&t, "Active PRs"),
        format!(
            "{} (with anyone's activity: {})",
            n(&m["active_prs"]),
            n(&m["active_prs_all_participants"])
        )
    );
    // And the hand-derived base values, so both cannot be wrong together.
    assert_eq!(table_value(&t, "Commits"), "6 (1 merge)");
    assert_eq!(table_value(&t, "PRs created"), "2");

    // Daily table rows: Period, Commits, PRs, Merged, ... per bucket and a
    // Total row equal to the JSON buckets and metrics.
    let d = env.run(&period_args(&["daily"], &[])).await;
    d.expect_code(0);
    let dj = env.offline_json(&["daily"]).await.json();
    let mut rows: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in d.stdout.lines() {
        let cells: Vec<&str> = l.split_whitespace().collect();
        if cells.len() == 12
            && (cells[0].starts_with("2024-") || cells[0] == "Total")
            && cells[1..10].iter().all(|c| c.parse::<u64>().is_ok())
        {
            rows.insert(
                cells[0].to_string(),
                cells[1..10].iter().map(|c| c.parse().unwrap()).collect(),
            );
            lines.insert(
                cells[0].to_string(),
                cells[10..].iter().map(|c| c.to_string()).collect(),
            );
        }
    }
    // +Lines / -Lines: a number, `~` and a number for a partial sum, or `?`
    // when no commit has known line counts.
    let line_cells = |s: &Value| -> Vec<String> {
        let d = &s["diff_stats"];
        ["additions", "deletions"]
            .iter()
            .map(|k| match d["coverage"].as_str().unwrap() {
                "unavailable" => "?".to_string(),
                "partial" => format!("~{}", n(&d[*k])),
                _ => n(&d[*k]).to_string(),
            })
            .collect()
    };
    assert_eq!(rows.len(), 8, "{}", d.stdout);
    let summary_cells = |s: &Value| -> Vec<u64> {
        vec![
            n(&s["commits"]),
            n(&s["prs_created"]),
            n(&s["prs_merged"]),
            n(&s["reviews_submitted"]["total"]),
            n(&s["reviews_received"]["total"]),
            n(&s["comments_written"]["total"]),
            n(&s["pr_commits"]),
            n(&s["issues_opened"]),
            n(&s["active_prs"]),
        ]
    };
    for b in dj["activity_breakdown"].as_array().unwrap() {
        let label = b["bucket"]["label"].as_str().unwrap();
        assert_eq!(rows[label], summary_cells(&b["summary"]), "{label}");
        assert_eq!(lines[label], line_cells(&b["summary"]), "{label}");
    }
    assert_eq!(rows["Total"], summary_cells(&dj["metrics"]));
    assert_eq!(lines["Total"], line_cells(&dj["metrics"]));
}

// ---------------------------------------------------------------------------
// Timeline details
// ---------------------------------------------------------------------------

/// The base world's acme/widget#10 timeline: actors, actor kinds, event ids
/// and the difference between when something happened and when it was
/// fetched.
#[tokio::test(flavor = "multi_thread")]
async fn pr_timeline_names_actors_and_keeps_occurrence_and_observation_apart() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    let started = chrono::Utc::now();
    let r = env
        .run(&[
            "pr",
            "https://github.com/acme/widget/pull/10",
            "--timeline",
            "--format",
            "json",
        ])
        .await;
    r.expect_code(0).assert_clean();
    let j = r.json();
    let timeline = j["timeline"].as_array().unwrap();

    // Event ids are unique: an inline comment that GitHub returns both in
    // its review and in its thread is listed once.
    let mut ids: Vec<&str> = timeline
        .iter()
        .map(|e| e["event_id"].as_str().unwrap())
        .collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate event ids: {timeline:?}");
    assert_eq!(
        timeline
            .iter()
            .filter(|e| e["event_id"] == "comment:PRRC_A1")
            .count(),
        1
    );

    let find = |id: &str| -> &Value {
        timeline
            .iter()
            .find(|e| e["event_id"] == id)
            .unwrap_or_else(|| panic!("no {id} in {timeline:?}"))
    };
    for (id, actor, kind) in [
        ("review:PRR_A1", "bob", "user"),
        ("review:PRR_A2", "carol", "user"),
        ("comment:IC_A3", "github-actions", "bot"),
        ("comment:IC_A1", ME, "user"),
    ] {
        let e = find(id);
        assert_eq!(e["actor"], actor, "{id}");
        assert_eq!(e["actor_kind"], kind, "{id}");
    }
    // Commits: dependabot is a bot, sha 14 has no linked account.
    let commit_actor = |sha_: &str| -> (String, String) {
        let e = find(&format!("pr-commit:PR_A:{sha_}"));
        assert_eq!(e["event_type"], "pr_commit");
        (
            e["actor"].as_str().unwrap().to_string(),
            e["actor_kind"].as_str().unwrap().to_string(),
        )
    };
    assert_eq!(
        commit_actor(&sha(13)),
        ("dependabot[bot]".to_string(), "bot".to_string())
    );
    assert_eq!(
        commit_actor(&sha(14)),
        ("unknown".to_string(), "unknown".to_string())
    );
    // Links come from GitHub.
    assert_eq!(
        find("comment:IC_A2")["url"],
        "https://github.com/acme/widget/pull/1#issuecomment-IC_A2"
    );

    // Occurrence times are the fixture's (2024); observation times are when
    // this test fetched them.
    let ic_a2 = find("comment:IC_A2");
    assert_eq!(ic_a2["occurred_at"], "2024-03-04T11:00:00Z");
    for e in timeline {
        let occurred =
            chrono::DateTime::parse_from_rfc3339(e["occurred_at"].as_str().unwrap()).unwrap();
        let observed =
            chrono::DateTime::parse_from_rfc3339(e["observed_at"].as_str().unwrap()).unwrap();
        assert_ne!(occurred, observed, "{e}");
        assert!(
            observed.to_utc() >= started - chrono::Duration::seconds(1),
            "observed_at {observed} is before the fetch started ({started}): {e}"
        );
        assert!(occurred.to_utc() < started, "{e}");
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// Cells of the Markdown table row whose first cell is `first`.
fn md_row(md: &str, first: &str) -> Vec<String> {
    md.lines()
        .find(|l| l.starts_with(&format!("| {first} |")))
        .unwrap_or_else(|| panic!("no row {first:?} in:\n{md}"))
        .trim_matches('|')
        .split(" | ")
        .map(|c| c.trim().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_output_matches_json_and_hides_private_names() {
    let server = serve(&World::base()).await;
    let env = Env::new(&server);
    env.sync(&[]).await.expect_code(0);

    let json = env
        .run(&period_args(&["overview"], &["--format", "json"]))
        .await;
    json.expect_code(0).assert_clean();
    let j = json.json();
    let md = env
        .run(&period_args(&["overview"], &["--format", "markdown"]))
        .await;
    md.expect_code(0).assert_clean();
    let md = md.stdout;
    assert!(md.starts_with("### GitHub activity of "), "{md}");
    assert!(md.contains("| --- | --- |"), "{md}");
    let commits = md_row(&md, "Commits");
    assert!(
        commits[1].starts_with(&format!("{} (", j["metrics"]["commits"])),
        "{commits:?}"
    );
    let prs = md_row(&md, "PRs created");
    assert_eq!(prs[1], j["metrics"]["prs_created"].to_string());
    assert!(md.contains("#### Top repositories"), "{md}");
    assert!(md.contains("Coverage: complete"), "{md}");
    // Like JSON and CSV, Markdown is meant to be shared, so the same
    // redaction of private repositories applies unless asked otherwise.
    // The base world's only private repository has no activity of mine, so
    // no hidden name is shown and no note is needed.
    assert_eq!(j["private_repositories_redacted"], true);
    assert!(!md.contains("private repository names are hidden"), "{md}");

    // Breakdowns are Markdown tables with a total row.
    let weekly = env
        .run(&period_args(&["weekly"], &["--format", "markdown"]))
        .await;
    weekly.expect_code(0).assert_clean();
    let total = md_row(&weekly.stdout, "Total");
    assert_eq!(total[1], j["metrics"]["commits"].to_string());

    // Reports without a Markdown form are refused with a usage error.
    let prs = env
        .run(&period_args(&["prs"], &["--format", "markdown"]))
        .await;
    prs.expect_code(2);
    assert!(
        prs.stderr.contains("--format markdown is available"),
        "{}",
        prs.stderr
    );
}
