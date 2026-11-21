//! GitHub records every reply to a review thread as a review without a
//! body. Such reviews are replies, not review submissions.

use chrono::{DateTime, Utc};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{self, CommitDateBasis, Dataset, Filter, Options};
use ghusage_core::model::*;
use ghusage_core::time::{Period, Zone};

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
fn me() -> Actor {
    Actor::new(Some("U_me".into()), Some("me".into()), Some("User"))
}
fn ev(id: EventId, ty: EventType, meta: EventMeta, t: &str) -> ActivityEvent {
    ActivityEvent {
        event_id: id,
        event_type: ty,
        actor: me(),
        repository_id: "R".into(),
        entity_id: "x".into(),
        parent_pr_id: Some("PR".into()),
        occurred_at: at(t),
        observed_at: at(t),
        source: Source::Graphql,
        url: None,
        meta,
        coverage: CoverageStatus::Complete,
    }
}
fn review(id: &str, state: ReviewState) -> ActivityEvent {
    let mut e = ev(
        EventId::review(id),
        EventType::ReviewSubmitted,
        EventMeta {
            review_state: Some(state),
            review_id: Some(id.into()),
            ..Default::default()
        },
        "2026-11-20T10:00:00Z",
    );
    e.entity_id = id.into();
    e
}
fn inline(id: &str, review: &str, reply_to: Option<&str>) -> ActivityEvent {
    ev(
        EventId::comment(id),
        EventType::InlineComment,
        EventMeta {
            review_id: Some(review.into()),
            reply_to: reply_to.map(Into::into),
            ..Default::default()
        },
        "2026-11-20T10:00:00Z",
    )
}

fn summarize(events: Vec<ActivityEvent>) -> metrics::Summary {
    let ds = Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![],
        vec![],
        vec![],
        vec![],
        events,
        vec![],
    );
    metrics::summary(
        &ds,
        &Options {
            period: Period::new(at("2026-11-01T00:00:00Z"), at("2026-12-01T00:00:00Z")).unwrap(),
            zone: Zone::utc(),
            filter: Filter::default(),
            commit_basis: CommitDateBasis::Committer,
        },
    )
}

#[test]
fn reply_only_review_is_not_a_submission() {
    let s = summarize(vec![
        review("RV1", ReviewState::Commented),
        inline("C1", "RV1", Some("C0")),
    ]);
    assert_eq!(s.reviews_submitted.total, 0);
    assert_eq!(s.comments_written.inline, 1);
}

#[test]
fn review_with_new_inline_comment_counts() {
    let s = summarize(vec![
        review("RV1", ReviewState::Commented),
        inline("C1", "RV1", Some("C0")),
        inline("C2", "RV1", None),
    ]);
    assert_eq!(s.reviews_submitted.total, 1);
}

#[test]
fn review_with_summary_or_decision_counts() {
    let mut summary = ev(
        EventId::review_summary("RV1"),
        EventType::ReviewSummary,
        EventMeta {
            review_id: Some("RV1".into()),
            ..Default::default()
        },
        "2026-11-20T10:00:00Z",
    );
    summary.entity_id = "RV1".into();
    let s = summarize(vec![
        review("RV1", ReviewState::Commented),
        summary,
        inline("C1", "RV1", Some("C0")),
    ]);
    assert_eq!(s.reviews_submitted.total, 1);
    let s = summarize(vec![
        review("RV2", ReviewState::Approved),
        inline("C1", "RV2", Some("C0")),
    ]);
    assert_eq!(s.reviews_submitted.total, 1);
    // A commented review with no comments at all is still a submission.
    let s = summarize(vec![review("RV3", ReviewState::Commented)]);
    assert_eq!(s.reviews_submitted.total, 1);
}
