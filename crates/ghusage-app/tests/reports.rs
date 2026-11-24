//! Report, redaction, JSON and CSV behavior on synthetic data.

use ghusage_app::csv_out;
use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::sync::{SyncOptions, applicable_coverage, ranges_to_fetch};
use ghusage_core::coverage::{
    CoverageGeneration, CoverageRecord, CoverageScope, CoverageStatus, CoverageVisibility, DataKind,
};
use ghusage_core::metrics::{CommitDateBasis, Dataset, Filter, Participation, PrSort};
use ghusage_core::model::*;
use ghusage_core::time::{Granularity, Period, Zone};
use jiff::{SignedDuration, Timestamp};

fn t(d: u32, h: u32) -> Timestamp {
    format!("2026-11-{d:02}T{h:02}:00:00Z").parse().unwrap()
}

fn me() -> Actor {
    Actor::new(Some("U_me".into()), Some("me".into()), Some("User"))
}
fn other() -> Actor {
    Actor::new(Some("U_o".into()), Some("other".into()), Some("User"))
}

fn repo(id: &str, owner: &str, name: &str, vis: Visibility) -> Repository {
    Repository {
        id: id.into(),
        owner: owner.into(),
        name: name.into(),
        owner_kind: ActorKind::User,
        visibility: vis,
        is_fork: false,
        viewer_permission: None,
        accessible: true,
        url: Some(format!("https://github.com/{owner}/{name}")),
    }
}

fn ev(
    id: &str,
    ty: EventType,
    actor: Actor,
    repo: &str,
    pr: Option<&str>,
    at: Timestamp,
) -> ActivityEvent {
    ActivityEvent {
        event_id: EventId(id.into()),
        event_type: ty,
        actor,
        repository_id: repo.into(),
        entity_id: id.into(),
        parent_pr_id: pr.map(Into::into),
        occurred_at: at,
        observed_at: t(21, 0),
        source: Source::Graphql,
        url: Some(format!("https://github.com/x/{id}")),
        meta: EventMeta::default(),
        coverage: CoverageStatus::Complete,
    }
}

fn pr(id: &str, repo: &str, title: &str) -> PullRequest {
    PullRequest {
        id: id.into(),
        repository_id: repo.into(),
        number: 7,
        title: title.into(),
        url: format!("https://github.com/o/r/pull/7?{id}"),
        author: me(),
        state: PrState::Merged,
        is_draft: false,
        created_at: t(1, 9),
        updated_at: Some(t(2, 9)),
        closed_at: Some(t(2, 9)),
        merged_at: Some(t(2, 9)),
        merged_by: Some(other()),
        head_sha: None,
        latest_ci: Some(CheckOutcome::Success),
        coverage: CoverageStatus::Complete,
        synced_at: Some(t(21, 0)),
    }
}

fn dataset() -> Dataset {
    let mut commit = ev(
        "c1",
        EventType::CommitAuthored,
        me(),
        "R_pub",
        None,
        t(1, 10),
    );
    commit.meta.sha = Some("abc".into());
    let mut review = ev(
        "rv1",
        EventType::ReviewSubmitted,
        other(),
        "R_priv",
        Some("PR_2"),
        t(2, 3),
    );
    review.meta.review_state = Some(ReviewState::Approved);
    Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![
            repo("R_pub", "me", "public-tool", Visibility::Public),
            repo("R_priv", "acme", "secret-plans", Visibility::Private),
        ],
        vec![
            pr("PR_1", "R_pub", "Public change"),
            pr("PR_2", "R_priv", "Secret launch"),
        ],
        vec![],
        vec![],
        vec![
            commit,
            ev(
                "o1",
                EventType::PrOpened,
                me(),
                "R_pub",
                Some("PR_1"),
                t(1, 9),
            ),
            ev(
                "o2",
                EventType::PrOpened,
                me(),
                "R_priv",
                Some("PR_2"),
                t(1, 11),
            ),
            review,
            ev(
                "m2",
                EventType::PrMerged,
                other(),
                "R_priv",
                Some("PR_2"),
                t(2, 9),
            ),
            ev(
                "cm",
                EventType::ConversationComment,
                me(),
                "R_priv",
                Some("PR_2"),
                t(2, 4),
            ),
        ],
        vec![CoverageRecord {
            kind: DataKind::Commits,
            start: t(1, 0),
            end: t(10, 0),
            status: CoverageStatus::Complete,
            scope: "commits".into(),
            description: "commits".into(),
            reason: None,
            recorded_at: t(21, 0),
        }],
    )
}

fn ctx(redact: bool) -> QueryContext {
    QueryContext {
        period: Period::new(t(1, 0), t(8, 0)).unwrap(),
        zone: Zone::utc(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
        redact_private: redact,
        synced_at: Some(t(21, 0)),
    }
}

#[test]
fn redaction_hides_private_names_but_keeps_numbers() {
    let ds = dataset();
    let plain = report::build(&ds, &ctx(false), ReportKind::Overview);
    let red = report::build(&ds, &ctx(true), ReportKind::Overview);
    assert_eq!(plain.metrics, red.metrics);
    assert_eq!(plain.activity_breakdown.len(), red.activity_breakdown.len());
    let json = serde_json::to_string(&red).unwrap();
    assert!(!json.contains("secret-plans"));
    assert!(!json.contains("acme"));
    assert!(json.contains("public-tool"));
    let prs = report::build(
        &ds,
        &ctx(true),
        ReportKind::PullRequests {
            participation: Participation::Subject,
            state: None,
            sort: PrSort::Activity,
        },
    );
    let json = serde_json::to_string(&prs).unwrap();
    assert!(!json.contains("Secret launch"));
    assert!(json.contains("Public change"));
}

#[test]
fn filter_by_private_repository_name_still_works_when_redacted() {
    let ds = dataset();
    let mut c = ctx(true);
    c.filter.repositories = vec!["acme/secret-plans".into()];
    let r = report::build(&ds, &c, ReportKind::Overview);
    assert_eq!(r.metrics.prs_created, 1);
    assert_eq!(r.metrics.commits, 0);
    c.filter.repositories = vec!["nobody/nothing".into()];
    let r = report::build(&ds, &c, ReportKind::Overview);
    assert_eq!(r.metrics.prs_created, 0);
}

#[test]
fn json_has_required_root_fields_and_iso_timestamps() {
    let r = report::build(
        &dataset(),
        &ctx(false),
        ReportKind::Breakdown(Granularity::Day),
    );
    let v: serde_json::Value = serde_json::to_value(&r).unwrap();
    for k in [
        "schema_version",
        "account",
        "period",
        "timezone",
        "metrics",
        "repositories",
        "activity_breakdown",
        "coverage",
        "synced_at",
    ] {
        assert!(v.get(k).is_some(), "missing root field {k}");
    }
    assert_eq!(v["schema_version"], 1);
    let synced = v["synced_at"].as_str().unwrap();
    assert!(synced.parse::<Timestamp>().is_ok());
    // Unknown values are null, zero values are numbers.
    assert!(v["metrics"]["issues_opened"].is_number());
    assert_eq!(v["activity_breakdown"].as_array().unwrap().len(), 7);
}

#[test]
fn csv_round_trips_and_neutralizes_formulas() {
    let mut ds = dataset();
    ds.pull_requests.get_mut("PR_1").unwrap().title = "=HYPERLINK(\"http://evil\")".into();
    let kind = ReportKind::PullRequests {
        participation: Participation::All,
        state: None,
        sort: PrSort::Activity,
    };
    let r = report::build(&ds, &ctx(false), kind);
    let csv = csv_out::report_csv(&r, kind);
    let mut rd = csv::Reader::from_reader(csv.as_bytes());
    let headers = rd.headers().unwrap().clone();
    let title_idx = headers.iter().position(|h| h == "title").unwrap();
    let rows: Vec<csv::StringRecord> = rd.records().map(|r| r.unwrap()).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| !r[title_idx].starts_with('=')));

    let daily = report::build(&ds, &ctx(false), ReportKind::Breakdown(Granularity::Day));
    let csv = csv_out::report_csv(&daily, ReportKind::Breakdown(Granularity::Day));
    let mut rd = csv::Reader::from_reader(csv.as_bytes());
    let commits_idx = rd
        .headers()
        .unwrap()
        .iter()
        .position(|h| h == "commits")
        .unwrap();
    let total: u64 = rd
        .records()
        .map(|r| r.unwrap()[commits_idx].parse::<u64>().unwrap())
        .sum();
    assert_eq!(total, daily.metrics.commits);
}

#[test]
fn only_missing_ranges_are_fetched() {
    let period = Period::new(t(1, 0), t(20, 0)).unwrap();
    let mut opts = SyncOptions::new(period, t(20, 0));
    opts.overlap = SignedDuration::from_secs(24 * 60 * 60);
    let recs = vec![CoverageRecord {
        kind: DataKind::Commits,
        start: t(1, 0),
        end: t(10, 0),
        status: CoverageStatus::Complete,
        scope: "".into(),
        description: String::new(),
        reason: None,
        recorded_at: t(10, 0),
    }];
    let r = ranges_to_fetch(&recs, DataKind::Commits, &opts);
    assert_eq!(r, vec![Period::new(t(9, 0), t(20, 0)).unwrap()]);
    // Nothing synced yet for another kind: the whole period.
    assert_eq!(
        ranges_to_fetch(&recs, DataKind::Issues, &opts),
        vec![period]
    );
    opts.full = true;
    assert_eq!(
        ranges_to_fetch(&recs, DataKind::Commits, &opts),
        vec![period]
    );
}

#[test]
fn commit_coverage_from_search_alone_is_synced_again() {
    let rec = |kind: DataKind, scope: CoverageScope| CoverageRecord {
        kind,
        start: t(1, 0),
        end: t(10, 0),
        status: CoverageStatus::Complete,
        scope,
        description: "coverage test".into(),
        reason: None,
        recorded_at: t(10, 0),
    };
    let scope = |generation, visibility| CoverageScope {
        generation,
        visibility,
    };
    let search_only = rec(
        DataKind::Commits,
        scope(CoverageGeneration::LegacyUnusable, CoverageVisibility::All),
    );
    let listed = rec(
        DataKind::Commits,
        scope(
            CoverageGeneration::CommitsListingSearchV1,
            CoverageVisibility::All,
        ),
    );
    let prs = rec(
        DataKind::PullRequests,
        scope(CoverageGeneration::PullRequestsV1, CoverageVisibility::All),
    );
    let public = rec(
        DataKind::Commits,
        scope(
            CoverageGeneration::CommitsListingSearchV1,
            CoverageVisibility::PublicOnly,
        ),
    );
    let all = vec![search_only, listed.clone(), prs.clone(), public.clone()];
    assert_eq!(
        applicable_coverage(all.clone(), false),
        vec![listed.clone(), prs.clone()]
    );
    assert_eq!(applicable_coverage(all, true), vec![listed, prs, public]);
    // Without applicable commit coverage, the whole period is fetched again.
    let opts = SyncOptions::new(Period::new(t(1, 0), t(10, 0)).unwrap(), t(10, 0));
    let old = applicable_coverage(
        vec![rec(
            DataKind::Commits,
            scope(CoverageGeneration::LegacyUnusable, CoverageVisibility::All),
        )],
        false,
    );
    assert_eq!(
        ranges_to_fetch(&old, DataKind::Commits, &opts),
        vec![opts.period]
    );
    // Listing records written while search results from unrelated
    // repositories were still taken are synced again too.
    let unfiltered = rec(
        DataKind::Commits,
        scope(CoverageGeneration::LegacyUnusable, CoverageVisibility::All),
    );
    assert_eq!(applicable_coverage(vec![unfiltered], false), vec![]);
}

#[test]
fn pr_report_lists_timeline_in_order_and_filters() {
    let ds = dataset();
    let r = report::pr_report(&ds, "PR_2", &[], Zone::utc(), false).unwrap();
    let types: Vec<EventType> = r.timeline.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![
            EventType::PrOpened,
            EventType::ReviewSubmitted,
            EventType::ConversationComment,
            EventType::PrMerged
        ]
    );
    let r = report::pr_report(&ds, "PR_2", &[EventType::PrMerged], Zone::utc(), true).unwrap();
    assert_eq!(r.timeline.len(), 1);
    assert_eq!(r.title, "[private]");
    assert_eq!(
        r.reviews.total, 1,
        "counts cover all events regardless of the filter"
    );
}

#[test]
fn own_pull_request_commits_join_the_global_commit_set_once() {
    use ghusage_github::PrBundle;
    let subject = Subject {
        id: Some("U_me".into()),
        login: "me".into(),
    };
    let mut search_hit = ev(
        "x",
        EventType::CommitAuthored,
        me(),
        "R_pub",
        None,
        t(1, 10),
    );
    search_hit.event_id = EventId::commit("R_pub", "abc");
    search_hit.meta.sha = Some("abc".into());
    let pc = |sha: &str, who: Actor| {
        let mut e = ev(
            "y",
            EventType::PrCommit,
            who,
            "R_pub",
            Some("PR_1"),
            t(1, 11),
        );
        e.event_id = EventId::pr_commit("PR_1", sha);
        e.meta.sha = Some(sha.into());
        e
    };
    let bundle = PrBundle {
        repository: repo("R_pub", "me", "public-tool", Visibility::Public),
        pull_request: pr("PR_1", "R_pub", "Public change"),
        events: vec![pc("abc", me()), pc("def", me()), pc("fff", other())],
        threads: vec![],
        current_commit_shas: vec!["abc".into(), "def".into(), "fff".into()],
        commit_stats: vec![],
        diff: None,
        merge_commit_parents: None,
        complete_collections: Collection::ALL.to_vec(),
        status: CoverageStatus::Complete,
        notes: vec![],
    };
    let batch = ghusage_app::sync::pr_batch(&subject, bundle);
    let mut events = batch.events.clone();
    events.push(search_hit);
    let ds = Dataset::new(
        subject,
        true,
        batch.repositories.clone(),
        batch.pull_requests,
        vec![],
        vec![],
        events,
        vec![],
    );
    let r = report::build(&ds, &ctx(false), ReportKind::Overview);
    // {abc} from search ∪ {abc, def} from the pull request; fff is not ours.
    assert_eq!(r.metrics.commits, 2);
    assert_eq!(r.metrics.pr_commits, 2);
}

fn day(d: u32) -> QueryContext {
    let mut c = ctx(false);
    c.period = Period::new(t(d, 0), t(d + 1, 0)).unwrap();
    c
}

#[test]
fn day_detail_lists_that_days_activity_with_the_same_numbers() {
    let ds = dataset();
    let r = report::build_detail(&ds, &day(1));
    // Day 1: PR_1 opened 09:00, commit abc 10:00, PR_2 opened 11:00.
    assert_eq!(r.metrics.commits, 1);
    assert_eq!(r.metrics.prs_created, 2);
    let commits = r.commits.as_ref().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].sha.as_deref(), Some("abc"));
    let types: Vec<EventType> = r
        .activity
        .as_ref()
        .unwrap()
        .iter()
        .map(|a| a.event_type)
        .collect();
    assert_eq!(
        types,
        vec![
            EventType::PrOpened,
            EventType::CommitAuthored,
            EventType::PrOpened
        ]
    );
    // One event each; ties are ordered by repository name.
    let prs: Vec<&str> = r
        .pull_requests
        .as_ref()
        .unwrap()
        .iter()
        .map(|p| p.pr_id.as_str())
        .collect();
    assert_eq!(prs, vec!["PR_2", "PR_1"]);
    assert_eq!(r.reviews_received.as_ref().unwrap().len(), 0);

    // Day 2: the review received on PR_2 and my comment.
    let r2 = report::build_detail(&ds, &day(2));
    let received = r2.reviews_received.as_ref().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].pr_id.as_deref(), Some("PR_2"));
    assert_eq!(r2.comments.as_ref().unwrap().len(), 1);

    // The day's numbers are the daily breakdown row of the whole period.
    let whole = report::build(&ds, &ctx(false), ReportKind::Overview);
    assert_eq!(whole.activity_breakdown[0].summary, r.metrics);
    assert_eq!(whole.activity_breakdown[1].summary, r2.metrics);
    assert_eq!(
        report::build(&ds, &day(1), ReportKind::Overview).metrics,
        r.metrics
    );
}

#[test]
fn repository_detail_matches_the_repository_row() {
    let ds = dataset();
    let mut c = ctx(false);
    c.filter.repositories = vec!["R_priv".into()];
    let r = report::build_detail(&ds, &c);
    let whole = report::build(&ds, &ctx(false), ReportKind::Overview);
    let row = whole
        .repositories
        .iter()
        .find(|x| x.repository_id == "R_priv")
        .unwrap();
    assert_eq!(row.summary, r.metrics);
    assert_eq!(r.commits.as_ref().unwrap(), &Vec::new());
    // PR_2 opened, the review received, my comment. The merge by someone
    // else is not my activity.
    let ids: Vec<&str> = r
        .activity
        .as_ref()
        .unwrap()
        .iter()
        .map(|a| a.event_id.as_str())
        .collect();
    assert_eq!(ids, vec!["o2", "rv1", "cm"]);
    assert_eq!(r.pull_requests.as_ref().unwrap().len(), 1);

    // Redaction applies to the listings too.
    c.redact_private = true;
    let json = serde_json::to_string(&report::build_detail(&ds, &c)).unwrap();
    assert!(!json.contains("Secret launch"), "{json}");
    assert!(!json.contains("secret-plans"), "{json}");
}

#[test]
fn dashboard_lists_pull_requests_most_recently_active_first() {
    let ds = dataset();
    let r = report::build_dashboard(&ds, &ctx(false));
    assert_eq!(
        r.metrics,
        report::build(&ds, &ctx(false), ReportKind::Overview).metrics
    );
    // PR_1's last event is day 1 09:00; PR_2's is the merge on day 2 09:00.
    let prs: Vec<&str> = r
        .pull_requests
        .as_ref()
        .unwrap()
        .iter()
        .map(|p| p.pr_id.as_str())
        .collect();
    assert_eq!(prs, vec!["PR_2", "PR_1"]);
}

#[test]
fn issue_report_shows_the_issue_and_its_events_in_the_period() {
    let mut ds = dataset();
    ds.issues.insert(
        "I_1".into(),
        Issue {
            id: "I_1".into(),
            repository_id: "R_pub".into(),
            number: 12,
            title: "Crash on start".into(),
            url: "https://github.com/me/public-tool/issues/12".into(),
            author: me(),
            is_open: false,
            created_at: t(3, 1),
            closed_at: Some(t(9, 1)),
        },
    );
    for (id, ty, at) in [
        ("io", EventType::IssueOpened, t(3, 1)),
        ("ic", EventType::IssueClosed, t(9, 1)),
    ] {
        let mut e = ev(id, ty, me(), "R_pub", None, at);
        e.entity_id = "I_1".into();
        ds.events.push(e);
    }
    ghusage_core::metrics::sort_events(&mut ds.events);
    // The period ends on day 8, so the close on day 9 is outside it.
    let r = report::issue_report(&ds, &ctx(false), "I_1").unwrap();
    assert_eq!(r.number, 12);
    assert_eq!(r.repository, "me/public-tool");
    assert!(!r.is_open);
    assert_eq!(r.events.len(), 1);
    assert_eq!(r.events[0].event_type, EventType::IssueOpened);
    assert!(report::issue_report(&ds, &ctx(false), "I_missing").is_none());
}

/// Every count of a summary object, recursively (`ci`, review and comment
/// counts are nested objects).
fn assert_all_counts_zero(v: &serde_json::Value, path: &str) {
    for (k, x) in v.as_object().unwrap() {
        let p = format!("{path}.{k}");
        match x {
            serde_json::Value::Object(_) => assert_all_counts_zero(x, &p),
            serde_json::Value::Number(n) => assert_eq!(n.as_u64(), Some(0), "{p}"),
            serde_json::Value::Null => {}
            // Without commits there is nothing to describe.
            serde_json::Value::String(c) if p.ends_with(".diff_stats.coverage") => {
                assert_eq!(c, "no_commits", "{p}");
            }
            other => panic!("{p}: unexpected {other}"),
        }
    }
}

#[test]
fn missing_values_are_null_and_zero_counts_are_numbers() {
    // An account with no activity and nothing synced for the period.
    let nobody = Subject {
        id: None,
        login: "nobody".into(),
    };
    let ds = Dataset::new(
        nobody,
        false,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    let prs_kind = ReportKind::PullRequests {
        participation: Participation::Subject,
        state: None,
        sort: PrSort::Activity,
    };
    let r = report::build(&ds, &ctx(false), prs_kind);
    let v = serde_json::to_value(&r).unwrap();
    // Unknown account ID: null.
    assert!(v["account"]["id"].is_null());
    // Nothing happened: every count is the number 0, never null.
    assert_all_counts_zero(&v["metrics"], "metrics");
    assert_eq!(v["metrics"]["commits"], 0);
    assert_eq!(v["metrics"]["ci"]["passed"], 0);
    // No last activity: null, not 0 or an empty string.
    assert!(v["metrics"]["last_activity"].is_null());
    // Every daily bucket has zeros too.
    for b in v["activity_breakdown"].as_array().unwrap() {
        assert_all_counts_zero(&b["summary"], "bucket");
        assert!(b["summary"]["last_activity"].is_null());
    }
    // Never synced: the zeros come with coverage that says so.
    for c in v["coverage"].as_array().unwrap() {
        assert_eq!(c["status"], "not_yet_synced", "{c}");
    }
    // Statistics over zero pull requests are null; the count is 0.
    assert_eq!(v["pull_requests"], serde_json::json!([]));
    assert_eq!(v["intensity"]["prs"], 0);
    for (k, s) in v["intensity"].as_object().unwrap() {
        if k != "prs" {
            assert!(s.is_null(), "intensity.{k} = {s}");
        }
    }
    for (k, s) in v["timing"].as_object().unwrap() {
        assert!(s.is_null(), "timing.{k} = {s}");
    }
    // A synced period without activity: complete, still zeros.
    let synced = Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        DataKind::ALL
            .iter()
            .map(|k| CoverageRecord {
                kind: *k,
                start: t(1, 0),
                end: t(10, 0),
                status: CoverageStatus::Complete,
                scope: "everything".into(),
                description: "everything".into(),
                reason: None,
                recorded_at: t(21, 0),
            })
            .collect(),
    );
    let v =
        serde_json::to_value(report::build(&synced, &ctx(false), ReportKind::Overview)).unwrap();
    assert_all_counts_zero(&v["metrics"], "metrics");
    assert!(v["metrics"]["last_activity"].is_null());
    for c in v["coverage"].as_array().unwrap() {
        assert_eq!(c["status"], "complete", "{c}");
    }

    // A merged pull request whose merger GitHub did not report, without
    // CI, reviews or a close time.
    let mut merged = pr("PR_1", "R_pub", "Public change");
    merged.merged_by = None;
    merged.latest_ci = None;
    merged.closed_at = None;
    let ds = Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![repo("R_pub", "me", "public-tool", Visibility::Public)],
        vec![merged],
        vec![],
        vec![],
        vec![ev(
            "o1",
            EventType::PrOpened,
            me(),
            "R_pub",
            Some("PR_1"),
            t(1, 9),
        )],
        vec![],
    );
    let d = report::pr_report(&ds, "PR_1", &[], Zone::utc(), false).unwrap();
    let v = serde_json::to_value(&d).unwrap();
    assert!(v["merged_by"].is_null());
    assert!(v["latest_ci"].is_null());
    assert!(v["closed_at"].is_null());
    // Created 11-01 09:00, merged 11-02 09:00: 86400 s. No review: null.
    assert_eq!(v["timing"]["creation_to_merge_secs"], 86400);
    assert!(v["timing"]["time_to_first_review_secs"].is_null());
    assert!(v["timing"]["review_to_next_author_activity_secs"].is_null());
    // One event: first to last is 0 seconds, a number.
    assert_eq!(v["timing"]["first_to_last_activity_secs"], 0);
    assert_eq!(v["reviews"]["total"], 0);
    assert_eq!(v["removed_commits"], 0);
    assert_eq!(v["commit_authors"], serde_json::json!([]));
}

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {v}"))
        .keys()
        .cloned()
        .collect();
    k.sort();
    k
}

#[track_caller]
fn assert_keys(v: &serde_json::Value, expected: &[&str]) {
    let mut e: Vec<String> = expected
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    e.sort();
    assert_eq!(keys(v), e, "{v}");
}

/// The field lists of docs/reference/json.md, checked against real output.
#[test]
fn json_output_matches_documented_schema() {
    let base = dataset();
    let mut events = base.events.clone();
    // Add an issue I opened, and an inline comment by someone else on my
    // private pull request in a review thread, so every row type exists.
    let mut opened = ev("io", EventType::IssueOpened, me(), "R_pub", None, t(3, 9));
    opened.entity_id = "I_1".into();
    events.push(opened);
    let mut ic = ev(
        "ic",
        EventType::InlineComment,
        other(),
        "R_priv",
        Some("PR_2"),
        t(2, 5),
    );
    ic.meta.thread_id = Some("TH1".into());
    events.push(ic);
    let ds = Dataset::new(
        base.subject.clone(),
        true,
        base.repositories.values().cloned(),
        base.pull_requests.values().cloned(),
        vec![Issue {
            id: "I_1".into(),
            repository_id: "R_pub".into(),
            number: 5,
            title: "Broken".into(),
            url: "https://github.com/me/public-tool/issues/5".into(),
            author: me(),
            is_open: true,
            created_at: t(3, 9),
            closed_at: None,
        }],
        vec![ReviewThread {
            id: "TH1".into(),
            pr_id: "PR_2".into(),
            is_resolved: false,
            is_outdated: false,
            resolved_by: None,
            path: Some("src/lib.rs".into()),
            comment_count: 1,
            first_comment_author: Some(other()),
        }],
        events,
        base.coverage.clone(),
    );

    let summary = [
        "commits",
        "merge_commits",
        "prs_created",
        "prs_merged",
        "merges_performed",
        "merges_unknown_actor",
        "prs_closed_without_merge",
        "prs_reopened",
        "reviews_submitted",
        "reviewed_prs",
        "reviews_received",
        "reviews_received_from_bots",
        "review_dismissals_performed",
        "comments_written",
        "comments_received",
        "review_responses",
        "pr_commits",
        "force_pushes",
        "coordination_actions",
        "metadata_actions",
        "issues_opened",
        "issues_closed",
        "issues_reopened",
        "active_prs",
        "active_prs_all_participants",
        "pr_internal_events",
        "ci",
        "active_days",
        "last_activity",
        "diff_stats",
    ];
    let diff_stats = [
        "commits",
        "excluded",
        "commits_with_stats",
        "additions",
        "deletions",
        "coverage",
    ];
    let review_counts = [
        "total",
        "approved",
        "changes_requested",
        "commented",
        "dismissed_unknown_state",
    ];
    let comment_counts = ["total", "conversation", "inline", "review_summary"];
    let stats = ["count", "min", "median", "mean", "p90", "max"];
    let root = [
        "schema_version",
        "account",
        "period",
        "timezone",
        "metrics",
        "snapshot",
        "repositories",
        "activity_breakdown",
        "coverage",
        "synced_at",
        "private_repositories_redacted",
    ];
    let check_summary = |s: &serde_json::Value| {
        assert_keys(s, &summary);
        assert_keys(&s["diff_stats"], &diff_stats);
        assert_keys(&s["reviews_submitted"], &review_counts);
        assert_keys(&s["reviews_received"], &review_counts);
        assert_keys(&s["comments_written"], &comment_counts);
        assert_keys(&s["comments_received"], &comment_counts);
        assert_keys(&s["ci"], &["passed", "failed", "pending", "other"]);
    };

    let kinds: [(ReportKind, &[&str]); 9] = [
        (ReportKind::Overview, &[]),
        (ReportKind::Breakdown(Granularity::Day), &["granularity"]),
        (ReportKind::Breakdown(Granularity::Week), &["granularity"]),
        (ReportKind::Breakdown(Granularity::Month), &["granularity"]),
        (ReportKind::Repositories, &[]),
        (
            ReportKind::PullRequests {
                participation: Participation::All,
                state: None,
                sort: PrSort::Activity,
            },
            &["pull_requests", "intensity", "timing"],
        ),
        (
            ReportKind::Reviews,
            &["reviews_submitted", "reviews_received"],
        ),
        (ReportKind::Comments, &["comments"]),
        (ReportKind::Issues, &["issues"]),
    ];
    for (kind, extra) in kinds {
        let v = serde_json::to_value(report::build(&ds, &ctx(false), kind)).unwrap();
        let mut expected: Vec<&str> = root.to_vec();
        expected.extend_from_slice(extra);
        assert_keys(&v, &expected);
        assert_keys(&v["account"], &["login", "id", "is_authenticated_user"]);
        assert_keys(&v["period"], &["start", "end"]);
        check_summary(&v["metrics"]);
        assert_keys(
            &v["snapshot"],
            &["open_prs", "open_issues_authored", "threads"],
        );
        assert_keys(
            &v["snapshot"]["threads"],
            &["total", "open", "resolved", "outdated"],
        );
        for r in v["repositories"].as_array().unwrap() {
            assert_keys(
                r,
                &[
                    "repository_id",
                    "full_name",
                    "visibility",
                    "owned",
                    "accessible",
                    "summary",
                ],
            );
            check_summary(&r["summary"]);
        }
        assert_ne!(
            v["activity_breakdown"].as_array().unwrap(),
            &Vec::<serde_json::Value>::new()
        );
        for b in v["activity_breakdown"].as_array().unwrap() {
            assert_keys(b, &["bucket", "summary"]);
            assert_keys(&b["bucket"], &["key", "label", "start", "end"]);
            check_summary(&b["summary"]);
        }
        let cov = v["coverage"].as_array().unwrap();
        assert_eq!(cov.len(), 3);
        for c in cov {
            assert_keys(c, &["kind", "status", "scopes", "segments"]);
            for s in c["segments"].as_array().unwrap() {
                assert_keys(s, &["start", "end", "status", "reason"]);
            }
        }
    }

    let prs = serde_json::to_value(report::build(
        &ds,
        &ctx(false),
        ReportKind::PullRequests {
            participation: Participation::All,
            state: None,
            sort: PrSort::Activity,
        },
    ))
    .unwrap();
    let rows = prs["pull_requests"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for r in rows {
        assert_keys(
            r,
            &[
                "pr_id",
                "repository",
                "number",
                "title",
                "url",
                "author",
                "state",
                "is_draft",
                "created_at",
                "merged_at",
                "events",
                "events_by_subject",
                "commits",
                "reviews",
                "comments",
                "participants",
                "discussion_participants",
                "threads",
                "last_activity",
                "latest_ci",
                "diff",
            ],
        );
    }
    assert_keys(
        &prs["intensity"],
        &[
            "prs",
            "events_per_pr",
            "reviews_per_pr",
            "comments_per_pr",
            "commits_per_pr",
            "participants_per_pr",
            "discussion_participants_per_pr",
            "threads_per_pr",
            "active_days_per_pr",
        ],
    );
    assert_keys(&prs["intensity"]["events_per_pr"], &stats);
    assert_keys(
        &prs["timing"],
        &[
            "time_to_first_review",
            "creation_to_merge",
            "review_to_next_author_activity",
            "first_to_last_activity",
        ],
    );
    assert_keys(&prs["timing"]["creation_to_merge"], &stats);

    let rv = serde_json::to_value(report::build(&ds, &ctx(false), ReportKind::Reviews)).unwrap();
    let received = rv["reviews_received"].as_array().unwrap();
    assert_eq!(received.len(), 1);
    assert_keys(
        &received[0],
        &[
            "event_id",
            "submitted_at",
            "reviewer",
            "state",
            "repository",
            "pr_id",
            "pr_number",
            "pr_title",
            "url",
        ],
    );
    let cm = serde_json::to_value(report::build(&ds, &ctx(false), ReportKind::Comments)).unwrap();
    let comments = cm["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    for c in comments {
        assert_keys(
            c,
            &[
                "event_id",
                "created_at",
                "author",
                "kind",
                "written_by_subject",
                "repository",
                "pr_id",
                "pr_number",
                "edited",
                "url",
            ],
        );
    }
    let is = serde_json::to_value(report::build(&ds, &ctx(false), ReportKind::Issues)).unwrap();
    let issues = is["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_keys(
        &issues[0],
        &[
            "issue_id",
            "repository",
            "number",
            "title",
            "url",
            "is_open",
            "opened_in_period",
            "closed_in_period",
            "reopened_in_period",
        ],
    );

    let d = serde_json::to_value(report::pr_report(&ds, "PR_2", &[], Zone::utc(), false).unwrap())
        .unwrap();
    assert_keys(
        &d,
        &[
            "schema_version",
            "account",
            "timezone",
            "pr_id",
            "repository",
            "number",
            "title",
            "url",
            "author",
            "state",
            "is_draft",
            "created_at",
            "merged_at",
            "closed_at",
            "merged_by",
            "latest_ci",
            "commits",
            "removed_commits",
            "reviews",
            "comments",
            "review_threads",
            "participants",
            "discussion_participants",
            "commit_authors",
            "timing",
            "diff",
            "timeline",
            "coverage",
            "synced_at",
            "private_repositories_redacted",
        ],
    );
    assert_keys(&d["reviews"], &review_counts);
    assert_keys(&d["comments"], &comment_counts);
    assert_keys(
        &d["review_threads"],
        &["total", "open", "resolved", "outdated"],
    );
    assert_keys(
        &d["timing"],
        &[
            "time_to_first_review_secs",
            "creation_to_merge_secs",
            "review_to_next_author_activity_secs",
            "first_to_last_activity_secs",
        ],
    );
    let timeline = d["timeline"].as_array().unwrap();
    assert_ne!(timeline, &Vec::<serde_json::Value>::new());
    for e in timeline {
        assert_keys(
            e,
            &[
                "event_id",
                "occurred_at",
                "observed_at",
                "actor",
                "actor_kind",
                "event_type",
                "label",
                "detail",
                "url",
                "removed_observed_at",
            ],
        );
    }
}
