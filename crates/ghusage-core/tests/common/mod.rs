//! Shared builders for the aggregation test suites.
#![allow(dead_code)]

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{CommitDateBasis, Dataset, Filter, Options};
use ghusage_core::model::{
    ActivityEvent, Actor, ActorKind, EventId, EventMeta, EventType, Issue, PrState, PullRequest,
    Repository, ReviewThread, Source, Subject, Visibility,
};
use ghusage_core::time::{Period, Zone};
use jiff::{Timestamp, civil::Date};

pub(crate) fn utc(s: &str) -> Timestamp {
    s.parse()
        .unwrap_or_else(|e| panic!("bad timestamp {s}: {e}"))
}

pub(crate) fn date(s: &str) -> Date {
    s.parse().unwrap()
}

pub(crate) fn zone(name: &str) -> Zone {
    Zone::parse(name).unwrap()
}

/// The analyzed user.
pub(crate) fn subject() -> Subject {
    Subject {
        id: Some("U_me".into()),
        login: "me".into(),
    }
}

pub(crate) fn me() -> Actor {
    Actor::new(Some("U_me".into()), Some("me".into()), Some("User"))
}

pub(crate) fn user(login: &str) -> Actor {
    Actor::new(Some(format!("U_{login}")), Some(login.into()), Some("User"))
}

pub(crate) fn bot(login: &str) -> Actor {
    Actor::new(Some(format!("B_{login}")), Some(login.into()), Some("Bot"))
}

pub(crate) fn repo(id: &str, owner: &str, name: &str, visibility: Visibility) -> Repository {
    Repository {
        id: id.into(),
        owner: owner.into(),
        name: name.into(),
        owner_kind: if owner == "me" {
            ActorKind::User
        } else {
            ActorKind::Organization
        },
        visibility,
        is_fork: false,
        viewer_permission: None,
        accessible: true,
        url: None,
    }
}

pub(crate) fn pull(id: &str, repository_id: &str, number: u64, author: Actor) -> PullRequest {
    PullRequest {
        id: id.into(),
        repository_id: repository_id.into(),
        number,
        title: format!("Pull request {number}"),
        url: format!("https://github.com/example/{repository_id}/pull/{number}"),
        author,
        state: PrState::Open,
        is_draft: false,
        created_at: utc("2026-01-01T00:00:00Z"),
        updated_at: None,
        closed_at: None,
        merged_at: None,
        merged_by: None,
        head_sha: None,
        latest_ci: None,
        coverage: CoverageStatus::Complete,
        synced_at: None,
    }
}

pub(crate) fn thread(id: &str, pr_id: &str, started_by: Option<Actor>) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        pr_id: pr_id.into(),
        is_resolved: false,
        is_outdated: false,
        resolved_by: None,
        path: Some("src/lib.rs".into()),
        comment_count: 1,
        first_comment_author: started_by,
    }
}

pub(crate) fn issue(id: &str, repository_id: &str, number: u64, author: Actor) -> Issue {
    Issue {
        id: id.into(),
        repository_id: repository_id.into(),
        number,
        title: format!("Issue {number}"),
        url: format!("https://github.com/example/{repository_id}/issues/{number}"),
        author,
        is_open: true,
        created_at: utc("2026-01-01T00:00:00Z"),
        closed_at: None,
    }
}

pub(crate) const OBSERVED: &str = "2027-06-01T00:00:00Z";

/// A bare event; `entity_id` mirrors the event id.
pub(crate) fn event(
    id: EventId,
    event_type: EventType,
    actor: Actor,
    repository_id: &str,
    pr: Option<&str>,
    at: Timestamp,
) -> ActivityEvent {
    ActivityEvent {
        entity_id: id.0.clone(),
        event_id: id,
        event_type,
        actor,
        repository_id: repository_id.into(),
        parent_pr_id: pr.map(str::to_string),
        occurred_at: at,
        observed_at: utc(OBSERVED),
        source: Source::Graphql,
        url: None,
        meta: EventMeta::default(),
        coverage: CoverageStatus::Complete,
    }
}

pub(crate) fn with_meta(mut e: ActivityEvent, f: impl FnOnce(&mut EventMeta)) -> ActivityEvent {
    f(&mut e.meta);
    e
}

pub(crate) fn commit(repository_id: &str, sha: &str, actor: Actor, at: &str) -> ActivityEvent {
    with_meta(
        event(
            EventId::commit(repository_id, sha),
            EventType::CommitAuthored,
            actor,
            repository_id,
            None,
            utc(at),
        ),
        |m| m.sha = Some(sha.to_ascii_lowercase()),
    )
}

pub(crate) fn pr_commit(
    repository_id: &str,
    pr_id: &str,
    sha: &str,
    actor: Actor,
    at: &str,
) -> ActivityEvent {
    with_meta(
        event(
            EventId::pr_commit(pr_id, sha),
            EventType::PrCommit,
            actor,
            repository_id,
            Some(pr_id),
            utc(at),
        ),
        |m| {
            m.sha = Some(sha.to_ascii_lowercase());
            m.time_basis = Some("commit_date".into());
        },
    )
}

pub(crate) fn dataset(
    repositories: Vec<Repository>,
    pull_requests: Vec<PullRequest>,
    threads: Vec<ReviewThread>,
    events: Vec<ActivityEvent>,
) -> Dataset {
    Dataset::new(
        subject(),
        true,
        repositories,
        pull_requests,
        Vec::<Issue>::new(),
        threads,
        events,
        Vec::new(),
    )
}

pub(crate) fn options(start: &str, end: &str, zone: Zone) -> Options {
    Options {
        period: Period::new(utc(start), utc(end)).unwrap(),
        zone,
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
    }
}

/// Period covering whole local days `[first, last_exclusive)` in `zone`.
pub(crate) fn local_days(first: &str, last_exclusive: &str, zone: Zone) -> Options {
    Options {
        period: Period::new(
            zone.start_of_day(date(first)),
            zone.start_of_day(date(last_exclusive)),
        )
        .unwrap(),
        zone,
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
    }
}
