//! Report, redaction, JSON and CSV behavior on synthetic data.

use chrono::{DateTime, TimeZone, Utc};
use ghusage_app::csv_out;
use ghusage_app::report::{self, QueryContext, ReportKind};
use ghusage_app::sync::{SyncOptions, ranges_to_fetch};
use ghusage_core::coverage::{CoverageRecord, CoverageStatus, DataKind};
use ghusage_core::metrics::{CommitDateBasis, Dataset, Filter, Participation, PrSort};
use ghusage_core::model::*;
use ghusage_core::time::{Granularity, Period, Zone};

fn t(d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap()
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

fn ev(id: &str, ty: EventType, actor: Actor, repo: &str, pr: Option<&str>, at: DateTime<Utc>) -> ActivityEvent {
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
    let mut commit = ev("c1", EventType::CommitAuthored, me(), "R_pub", None, t(1, 10));
    commit.meta.sha = Some("abc".into());
    let mut review = ev("rv1", EventType::ReviewSubmitted, other(), "R_priv", Some("PR_2"), t(2, 3));
    review.meta.review_state = Some(ReviewState::Approved);
    Dataset::new(
        Subject { id: Some("U_me".into()), login: "me".into() },
        true,
        vec![
            repo("R_pub", "me", "public-tool", Visibility::Public),
            repo("R_priv", "acme", "secret-plans", Visibility::Private),
        ],
        vec![pr("PR_1", "R_pub", "Public change"), pr("PR_2", "R_priv", "Secret launch")],
        vec![],
        vec![],
        vec![
            commit,
            ev("o1", EventType::PrOpened, me(), "R_pub", Some("PR_1"), t(1, 9)),
            ev("o2", EventType::PrOpened, me(), "R_priv", Some("PR_2"), t(1, 11)),
            review,
            ev("m2", EventType::PrMerged, other(), "R_priv", Some("PR_2"), t(2, 9)),
            ev("cm", EventType::ConversationComment, me(), "R_priv", Some("PR_2"), t(2, 4)),
        ],
        vec![CoverageRecord {
            kind: DataKind::Commits,
            start: t(1, 0),
            end: t(10, 0),
            status: CoverageStatus::Complete,
            scope: "commits".into(),
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
        ReportKind::PullRequests { participation: Participation::Subject, state: None, sort: PrSort::Activity },
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
    let r = report::build(&dataset(), &ctx(false), ReportKind::Breakdown(Granularity::Day));
    let v: serde_json::Value = serde_json::to_value(&r).unwrap();
    for k in [
        "schema_version", "account", "period", "timezone", "metrics", "repositories",
        "activity_breakdown", "coverage", "synced_at",
    ] {
        assert!(v.get(k).is_some(), "missing root field {k}");
    }
    assert_eq!(v["schema_version"], 1);
    let synced = v["synced_at"].as_str().unwrap();
    assert!(DateTime::parse_from_rfc3339(synced).is_ok());
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
    let commits_idx = rd.headers().unwrap().iter().position(|h| h == "commits").unwrap();
    let total: u64 = rd.records().map(|r| r.unwrap()[commits_idx].parse::<u64>().unwrap()).sum();
    assert_eq!(total, daily.metrics.commits);
}

#[test]
fn only_missing_ranges_are_fetched() {
    let period = Period::new(t(1, 0), t(20, 0)).unwrap();
    let mut opts = SyncOptions::new(period, t(20, 0));
    opts.overlap = chrono::Duration::days(1);
    let recs = vec![CoverageRecord {
        kind: DataKind::Commits,
        start: t(1, 0),
        end: t(10, 0),
        status: CoverageStatus::Complete,
        scope: String::new(),
        reason: None,
        recorded_at: t(10, 0),
    }];
    let r = ranges_to_fetch(&recs, DataKind::Commits, &opts);
    assert_eq!(r, vec![Period::new(t(9, 0), t(20, 0)).unwrap()]);
    // Nothing synced yet for another kind: the whole period.
    assert_eq!(ranges_to_fetch(&recs, DataKind::Issues, &opts), vec![period]);
    opts.full = true;
    assert_eq!(ranges_to_fetch(&recs, DataKind::Commits, &opts), vec![period]);
}

#[test]
fn pr_report_lists_timeline_in_order_and_filters() {
    let ds = dataset();
    let r = report::pr_report(&ds, "PR_2", &[], Zone::utc(), false).unwrap();
    let types: Vec<EventType> = r.timeline.iter().map(|e| e.event_type).collect();
    assert_eq!(
        types,
        vec![EventType::PrOpened, EventType::ReviewSubmitted, EventType::ConversationComment, EventType::PrMerged]
    );
    let r = report::pr_report(&ds, "PR_2", &[EventType::PrMerged], Zone::utc(), true).unwrap();
    assert_eq!(r.timeline.len(), 1);
    assert_eq!(r.title, "[private]");
    assert_eq!(r.reviews.total, 1, "counts cover all events regardless of the filter");
}
