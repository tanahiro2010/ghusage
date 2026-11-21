//! Data coverage: how completely a kind of data was fetched for a time range.
//!
//! Incomplete fetches are never reported as complete. A `Complete` record
//! always carries a scope describing exactly what was fetched.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Partial,
    NotAvailable,
    NotYetSynced,
    Unknown,
}

impl CoverageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::NotAvailable => "not_available",
            Self::NotYetSynced => "not_yet_synced",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "complete" => Self::Complete,
            "partial" => Self::Partial,
            "not_available" => Self::NotAvailable,
            "not_yet_synced" => Self::NotYetSynced,
            _ => Self::Unknown,
        }
    }

    pub fn is_complete(self) -> bool {
        self == Self::Complete
    }

    /// Combines statuses of parts into the status of the whole. The whole is
    /// complete only when every part is complete.
    pub fn combine<I: IntoIterator<Item = CoverageStatus>>(parts: I) -> CoverageStatus {
        let mut iter = parts.into_iter();
        let Some(first) = iter.next() else {
            return CoverageStatus::NotYetSynced;
        };
        let mut all_same = true;
        for s in iter {
            if s != first {
                all_same = false;
            }
        }
        if all_same {
            first
        } else {
            CoverageStatus::Partial
        }
    }
}

/// A kind of data whose coverage is tracked per time range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    /// Commits authored by the subject.
    Commits,
    /// Pull requests involving the subject and their internal activity.
    PullRequests,
    /// Issues involving the subject.
    Issues,
}

impl DataKind {
    pub const ALL: [DataKind; 3] = [DataKind::Commits, DataKind::PullRequests, DataKind::Issues];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::PullRequests => "pull_requests",
            Self::Issues => "issues",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "commits" => Some(Self::Commits),
            "pull_requests" => Some(Self::PullRequests),
            "issues" => Some(Self::Issues),
            _ => None,
        }
    }
}

/// Coverage for one data kind over `[start, end)`, recorded by a sync run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageRecord {
    pub kind: DataKind,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub status: CoverageStatus,
    /// What was fetched (event types and source range).
    pub scope: String,
    /// Why the status is not complete, when applicable.
    pub reason: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

/// A contiguous part of a requested range with a single status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSegment {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub status: CoverageStatus,
    pub reason: Option<String>,
}

/// Coverage of one data kind for a requested range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub kind: DataKind,
    pub status: CoverageStatus,
    pub scopes: Vec<String>,
    pub segments: Vec<CoverageSegment>,
}

impl CoverageSummary {
    /// Segments that are not complete.
    pub fn gaps(&self) -> impl Iterator<Item = &CoverageSegment> {
        self.segments.iter().filter(|s| !s.status.is_complete())
    }
}

/// Computes coverage of `kind` over `[start, end)`. For every instant the
/// most recently recorded record wins; instants with no record are
/// `NotYetSynced`.
pub fn summarize(
    records: &[CoverageRecord],
    kind: DataKind,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> CoverageSummary {
    let mut relevant: Vec<&CoverageRecord> = records
        .iter()
        .filter(|r| r.kind == kind && r.start < end && r.end > start && r.start < r.end)
        .collect();
    relevant.sort_by_key(|r| r.recorded_at);

    let mut points = vec![start, end];
    for r in &relevant {
        points.push(r.start.max(start));
        points.push(r.end.min(end));
    }
    points.sort();
    points.dedup();

    let mut segments: Vec<CoverageSegment> = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let latest = relevant.iter().rev().find(|r| r.start <= a && r.end >= b);
        let (status, reason) = match latest {
            Some(r) => (r.status, r.reason.clone()),
            None => (CoverageStatus::NotYetSynced, None),
        };
        match segments.last_mut() {
            Some(last) if last.status == status && last.reason == reason && last.end == a => {
                last.end = b;
            }
            _ => segments.push(CoverageSegment {
                start: a,
                end: b,
                status,
                reason,
            }),
        }
    }

    let status = if start >= end {
        CoverageStatus::Complete
    } else {
        CoverageStatus::combine(segments.iter().map(|s| s.status))
    };
    let mut scopes: Vec<String> = relevant.iter().map(|r| r.scope.clone()).collect();
    scopes.sort();
    scopes.dedup();
    CoverageSummary {
        kind,
        status,
        scopes,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, d, 0, 0, 0).unwrap()
    }

    fn rec(s: u32, e: u32, status: CoverageStatus, at: u32) -> CoverageRecord {
        CoverageRecord {
            kind: DataKind::Commits,
            start: t(s),
            end: t(e),
            status,
            scope: "commits".into(),
            reason: None,
            recorded_at: t(at),
        }
    }

    #[test]
    fn empty_is_not_yet_synced() {
        let s = summarize(&[], DataKind::Commits, t(1), t(5));
        assert_eq!(s.status, CoverageStatus::NotYetSynced);
        assert_eq!(s.segments.len(), 1);
    }

    #[test]
    fn full_complete_cover_is_complete() {
        let r = [rec(1, 10, CoverageStatus::Complete, 10)];
        assert_eq!(
            summarize(&r, DataKind::Commits, t(2), t(5)).status,
            CoverageStatus::Complete
        );
    }

    #[test]
    fn gap_makes_partial() {
        let r = [
            rec(1, 3, CoverageStatus::Complete, 10),
            rec(4, 10, CoverageStatus::Complete, 10),
        ];
        let s = summarize(&r, DataKind::Commits, t(1), t(10));
        assert_eq!(s.status, CoverageStatus::Partial);
        let gaps: Vec<_> = s.gaps().collect();
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].start, gaps[0].end), (t(3), t(4)));
        assert_eq!(gaps[0].status, CoverageStatus::NotYetSynced);
    }

    #[test]
    fn latest_record_wins() {
        let r = [
            rec(1, 10, CoverageStatus::Partial, 5),
            rec(1, 10, CoverageStatus::Complete, 6),
        ];
        assert_eq!(
            summarize(&r, DataKind::Commits, t(1), t(10)).status,
            CoverageStatus::Complete
        );
        let r = [
            rec(1, 10, CoverageStatus::Complete, 5),
            rec(3, 4, CoverageStatus::Partial, 6),
        ];
        assert_eq!(
            summarize(&r, DataKind::Commits, t(1), t(10)).status,
            CoverageStatus::Partial
        );
    }

    #[test]
    fn other_kinds_ignored() {
        let mut r = rec(1, 10, CoverageStatus::Complete, 5);
        r.kind = DataKind::Issues;
        assert_eq!(
            summarize(&[r], DataKind::Commits, t(1), t(10)).status,
            CoverageStatus::NotYetSynced
        );
    }

    #[test]
    fn combine_rules() {
        use CoverageStatus::*;
        assert_eq!(CoverageStatus::combine([Complete, Complete]), Complete);
        assert_eq!(CoverageStatus::combine([Complete, NotYetSynced]), Partial);
        assert_eq!(CoverageStatus::combine([NotAvailable]), NotAvailable);
        assert_eq!(CoverageStatus::combine([]), NotYetSynced);
    }
}
