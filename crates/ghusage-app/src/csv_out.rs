//! CSV output. Each report kind maps to one flat table. Unknown values are
//! empty cells; zero is written as `0`.

use chrono::{DateTime, Utc};
use ghusage_core::coverage::{self, CoverageStatus};

use crate::report::{PrReport, Report, ReportKind};

fn ts(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| t.to_rfc3339()).unwrap_or_default()
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// Free text from GitHub: control characters removed, and a leading
/// formula character neutralized so spreadsheets do not evaluate it.
fn text(s: &str) -> String {
    let clean = ghusage_core::sanitize::sanitize_line(s);
    if clean.starts_with(['=', '+', '-', '@']) {
        format!("'{clean}")
    } else {
        clean
    }
}

fn finish(w: csv::Writer<Vec<u8>>) -> String {
    String::from_utf8(w.into_inner().expect("in-memory writer")).expect("csv is utf-8")
}

/// Renders a period report as CSV for the given report kind.
pub fn report_csv(r: &Report, kind: ReportKind) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    let overall = r.overall_coverage().as_str();
    match kind {
        ReportKind::Overview | ReportKind::Breakdown(_) => {
            w.write_record([
                "period",
                "start",
                "end",
                "commits",
                "merge_commits",
                "prs_created",
                "prs_merged",
                "merges_performed",
                "prs_closed_without_merge",
                "reviews_submitted",
                "reviewed_prs",
                "reviews_received",
                "comments_written",
                "comments_received",
                "review_responses",
                "pr_commits",
                "issues_opened",
                "issues_closed",
                "active_prs",
                "pr_internal_events",
                "ci_passed",
                "ci_failed",
                "coverage",
            ])
            .ok();
            for b in &r.activity_breakdown {
                let s = &b.summary;
                let cov = bucket_coverage(r, b.bucket.start, b.bucket.end);
                w.write_record([
                    b.bucket.label.clone(),
                    b.bucket.start.to_rfc3339(),
                    b.bucket.end.to_rfc3339(),
                    s.commits.to_string(),
                    s.merge_commits.to_string(),
                    s.prs_created.to_string(),
                    s.prs_merged.to_string(),
                    s.merges_performed.to_string(),
                    s.prs_closed_without_merge.to_string(),
                    s.reviews_submitted.total.to_string(),
                    s.reviewed_prs.to_string(),
                    s.reviews_received.total.to_string(),
                    s.comments_written.total().to_string(),
                    s.comments_received.total().to_string(),
                    s.review_responses.to_string(),
                    s.pr_commits.to_string(),
                    s.issues_opened.to_string(),
                    s.issues_closed.to_string(),
                    s.active_prs.to_string(),
                    s.pr_internal_events.to_string(),
                    s.ci.passed.to_string(),
                    s.ci.failed.to_string(),
                    cov.as_str().to_string(),
                ])
                .ok();
            }
        }
        ReportKind::Repositories => {
            w.write_record([
                "repository",
                "visibility",
                "owned",
                "accessible",
                "commits",
                "prs_created",
                "prs_merged",
                "reviews_submitted",
                "comments_written",
                "issues_opened",
                "active_prs",
                "pr_internal_events",
                "last_activity",
                "period_coverage",
            ])
            .ok();
            for x in &r.repositories {
                let s = &x.summary;
                w.write_record([
                    x.full_name.clone(),
                    x.visibility.as_str().to_string(),
                    x.owned.to_string(),
                    x.accessible.to_string(),
                    s.commits.to_string(),
                    s.prs_created.to_string(),
                    s.prs_merged.to_string(),
                    s.reviews_submitted.total.to_string(),
                    s.comments_written.total().to_string(),
                    s.issues_opened.to_string(),
                    s.active_prs.to_string(),
                    s.pr_internal_events.to_string(),
                    ts(s.last_activity),
                    overall.to_string(),
                ])
                .ok();
            }
        }
        ReportKind::PullRequests { .. } => {
            w.write_record([
                "repository",
                "number",
                "title",
                "state",
                "draft",
                "author",
                "created_at",
                "merged_at",
                "events",
                "events_by_you",
                "commits",
                "reviews",
                "comments",
                "participants",
                "discussion_participants",
                "threads",
                "last_activity",
                "latest_ci",
                "url",
                "period_coverage",
            ])
            .ok();
            for p in r.pull_requests.iter().flatten() {
                w.write_record([
                    p.repository.clone(),
                    p.number.to_string(),
                    text(&p.title),
                    p.state.as_str().to_string(),
                    p.is_draft.to_string(),
                    text(&p.author),
                    p.created_at.to_rfc3339(),
                    ts(p.merged_at),
                    p.events.to_string(),
                    p.events_by_subject.to_string(),
                    p.commits.to_string(),
                    p.reviews.to_string(),
                    p.comments.to_string(),
                    p.participants.to_string(),
                    p.discussion_participants.to_string(),
                    p.threads.to_string(),
                    ts(p.last_activity),
                    opt(p.latest_ci.map(|c| c.as_str())),
                    p.url.clone(),
                    overall.to_string(),
                ])
                .ok();
            }
        }
        ReportKind::Reviews => {
            w.write_record([
                "direction",
                "submitted_at",
                "reviewer",
                "state",
                "repository",
                "pr_number",
                "url",
                "period_coverage",
            ])
            .ok();
            for (dir, rows) in [
                ("submitted", r.reviews_submitted.as_deref().unwrap_or(&[])),
                ("received", r.reviews_received.as_deref().unwrap_or(&[])),
            ] {
                for x in rows {
                    w.write_record([
                        dir.to_string(),
                        x.submitted_at.to_rfc3339(),
                        text(&x.reviewer),
                        opt(x.state.map(|s| s.as_str())),
                        x.repository.clone(),
                        opt(x.pr_number),
                        opt(x.url.clone()),
                        overall.to_string(),
                    ])
                    .ok();
                }
            }
        }
        ReportKind::Comments => {
            w.write_record([
                "created_at",
                "author",
                "kind",
                "written_by_you",
                "repository",
                "pr_number",
                "edited",
                "url",
                "period_coverage",
            ])
            .ok();
            for c in r.comments.iter().flatten() {
                w.write_record([
                    c.created_at.to_rfc3339(),
                    text(&c.author),
                    c.kind.as_str().to_string(),
                    c.written_by_subject.to_string(),
                    c.repository.clone(),
                    opt(c.pr_number),
                    c.edited.to_string(),
                    opt(c.url.clone()),
                    overall.to_string(),
                ])
                .ok();
            }
        }
        ReportKind::Issues => {
            w.write_record([
                "repository",
                "number",
                "title",
                "open",
                "opened_in_period",
                "closed_in_period",
                "reopened_in_period",
                "url",
                "period_coverage",
            ])
            .ok();
            for i in r.issues.iter().flatten() {
                w.write_record([
                    i.repository.clone(),
                    i.number.to_string(),
                    text(&i.title),
                    i.is_open.to_string(),
                    i.opened_in_period.to_string(),
                    i.closed_in_period.to_string(),
                    i.reopened_in_period.to_string(),
                    i.url.clone(),
                    overall.to_string(),
                ])
                .ok();
            }
        }
    }
    finish(w)
}

fn bucket_coverage(r: &Report, start: DateTime<Utc>, end: DateTime<Utc>) -> CoverageStatus {
    // Coverage of a bucket is derived from the period segments.
    CoverageStatus::combine(r.coverage.iter().map(|c| {
        let records: Vec<coverage::CoverageRecord> = c
            .segments
            .iter()
            .map(|s| coverage::CoverageRecord {
                kind: c.kind,
                start: s.start,
                end: s.end,
                status: s.status,
                scope: String::new(),
                reason: None,
                recorded_at: start,
            })
            .collect();
        coverage::summarize(&records, c.kind, start, end).status
    }))
}

/// Renders a pull request timeline as CSV.
pub fn pr_csv(r: &PrReport) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    w.write_record([
        "occurred_at",
        "actor",
        "actor_kind",
        "event_type",
        "detail",
        "removed_observed_at",
        "url",
    ])
    .ok();
    for e in &r.timeline {
        w.write_record([
            e.occurred_at.to_rfc3339(),
            text(&e.actor),
            format!("{:?}", e.actor_kind).to_ascii_lowercase(),
            e.event_type.as_str().to_string(),
            text(&e.detail.clone().unwrap_or_default()),
            ts(e.removed_observed_at),
            e.url.clone().unwrap_or_default(),
        ])
        .ok();
    }
    finish(w)
}

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn names_get_the_same_treatment_as_titles() {
        // Logins and display names come from GitHub too.
        // The whole escape sequence is removed, not just ESC.
        assert_eq!(text("evil\u{1b}[2Jbot"), "evilbot");
        assert_eq!(text("=HYPERLINK(1)"), "'=HYPERLINK(1)");
        assert_eq!(text("octocat"), "octocat");
    }
}
