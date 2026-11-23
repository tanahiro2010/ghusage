//! Synthetic data shared by the benchmarks.
#![allow(dead_code)]

use ghusage_app::report::QueryContext;
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{CommitDateBasis, Filter};
use ghusage_core::model::*;
use ghusage_core::time::{Period, Zone};
use jiff::{SignedDuration, Timestamp};

pub const EVENTS: usize = 100_000;

pub fn actor(i: usize) -> Actor {
    if i.is_multiple_of(3) {
        Actor::new(Some("U_me".into()), Some("me".into()), Some("User"))
    } else {
        Actor::new(
            Some(format!("U_{}", i % 50)),
            Some(format!("user{}", i % 50)),
            Some("User"),
        )
    }
}

/// 100k events over one year across 40 repositories and 2,000 pull requests.
pub fn data() -> (Vec<Repository>, Vec<PullRequest>, Vec<ActivityEvent>) {
    let start: Timestamp = "2025-11-21T00:00:00Z".parse().unwrap();
    let repos: Vec<Repository> = (0..40)
        .map(|i: u32| Repository {
            id: format!("R_{i}"),
            owner: if i.is_multiple_of(2) {
                "me".into()
            } else {
                format!("org{i}")
            },
            name: format!("repo{i}"),
            owner_kind: ActorKind::User,
            visibility: if i.is_multiple_of(5) {
                Visibility::Private
            } else {
                Visibility::Public
            },
            is_fork: false,
            viewer_permission: None,
            accessible: true,
            url: None,
        })
        .collect();
    let prs: Vec<PullRequest> = (0..2000)
        .map(|i| PullRequest {
            id: format!("PR_{i}"),
            repository_id: format!("R_{}", i % 40),
            number: i as u64,
            title: format!("Change {i}"),
            url: format!("https://github.com/o/r/pull/{i}"),
            author: actor(i),
            state: PrState::Merged,
            is_draft: false,
            created_at: start + SignedDuration::from_secs(i as i64 * 4 * 60 * 60),
            updated_at: None,
            closed_at: None,
            merged_at: Some(start + SignedDuration::from_secs((i as i64 * 4 + 20) * 60 * 60)),
            merged_by: None,
            head_sha: None,
            latest_ci: None,
            coverage: CoverageStatus::Complete,
            synced_at: None,
        })
        .collect();
    let types = EventType::ALL;
    let events = (0..EVENTS)
        .map(|i| {
            let t = types[i % types.len()];
            let pr = (i % 2000).to_string();
            let at = start + SignedDuration::from_secs((i as i64) * 315);
            let mut meta = EventMeta::default();
            if matches!(t, EventType::CommitAuthored | EventType::PrCommit) {
                meta.sha = Some(format!("{i:040x}"));
            }
            if t == EventType::ReviewSubmitted {
                meta.review_state = Some(ReviewState::Approved);
            }
            ActivityEvent {
                event_id: EventId(format!("e{i}")),
                event_type: t,
                actor: actor(i),
                repository_id: format!("R_{}", (i % 2000) % 40),
                entity_id: format!("x{i}"),
                parent_pr_id: t.is_pr_event().then(|| format!("PR_{pr}")),
                occurred_at: at,
                observed_at: at,
                source: Source::Graphql,
                url: None,
                meta,
                coverage: CoverageStatus::Complete,
            }
        })
        .collect();
    (repos, prs, events)
}

pub fn subject() -> Subject {
    Subject {
        id: Some("U_me".into()),
        login: "me".into(),
    }
}

pub fn ctx(days: i64) -> QueryContext {
    let end: Timestamp = "2026-11-21T00:00:00Z".parse().unwrap();
    QueryContext {
        period: Period::new(end - SignedDuration::from_secs(days * 24 * 60 * 60), end).unwrap(),
        zone: Zone::parse("Asia/Tokyo").unwrap(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
        redact_private: false,
        synced_at: None,
    }
}
