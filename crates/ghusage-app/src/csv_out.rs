//! CSV output. Each report kind maps to one flat table. Unknown values are
//! empty cells; zero is written as `0`.

use ghusage_core::coverage::{self, CoverageStatus};
use jiff::Timestamp;

use crate::report::{PrReport, Report, ReportKind};

fn ts(t: Option<Timestamp>) -> String {
    t.map(|t| t.to_string()).unwrap_or_default()
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
/// Line count columns: additions, deletions, commits whose counts go into
/// the sums, of those with known counts, commits left out as counted
/// through other commits, coverage. The sums are empty when no commit has
/// known counts (unknown, not zero).
fn line_cols(d: &ghusage_core::metrics::DiffTotals) -> [String; 6] {
    use ghusage_core::metrics::DiffCoverage;
    let known = !matches!(d.coverage, DiffCoverage::Unavailable);
    let coverage = match d.coverage {
        DiffCoverage::NoCommits => "no_commits",
        DiffCoverage::Complete => "complete",
        DiffCoverage::Partial => "partial",
        DiffCoverage::Unavailable => "unavailable",
    };
    [
        if known {
            d.additions.to_string()
        } else {
            String::new()
        },
        if known {
            d.deletions.to_string()
        } else {
            String::new()
        },
        (d.commits - d.excluded).to_string(),
        d.commits_with_stats.to_string(),
        d.excluded.to_string(),
        coverage.to_string(),
    ]
}

include!("csv_out/report_csv.rs");

fn bucket_coverage(r: &Report, start: Timestamp, end: Timestamp) -> CoverageStatus {
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
                scope: "".into(),
                description: String::new(),
                reason: None,
                recorded_at: start,
            })
            .collect();
        coverage::summarize(&records, c.kind, start, end).status
    }))
}

/// Renders a pull request timeline as CSV.
#[must_use]
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
            e.occurred_at.to_string(),
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
