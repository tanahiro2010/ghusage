//! Data coverage: how completely a kind of data was fetched for a time range.
//!
//! Incomplete fetches are never reported as complete. A `Complete` record
//! always carries a scope describing exactly what was fetched.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Completeness state of a fetched data range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// All records in the requested range were fetched.
    Complete,
    /// Some, but not all, of the requested range was fetched.
    Partial,
    /// The source does not provide this kind of data.
    NotAvailable,
    /// No sync has fetched this range yet.
    NotYetSynced,
    /// The stored status is not recognized.
    Unknown,
}

impl CoverageStatus {
    /// Returns the stable serialized status name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::NotAvailable => "not_available",
            Self::NotYetSynced => "not_yet_synced",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a serialized status name, returning `Unknown` for other values.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "complete" => Self::Complete,
            "partial" => Self::Partial,
            "not_available" => Self::NotAvailable,
            "not_yet_synced" => Self::NotYetSynced,
            _ => Self::Unknown,
        }
    }

    /// Whether every part of the range is known to be covered.
    #[must_use]
    pub fn is_complete(self) -> bool {
        self == Self::Complete
    }

    /// Combines statuses of parts into the status of the whole. The whole is
    /// complete only when every part is complete.
    pub fn combine<I: IntoIterator<Item = Self>>(parts: I) -> Self {
        let mut iter = parts.into_iter();
        let Some(first) = iter.next() else {
            return Self::NotYetSynced;
        };
        let mut all_same = true;
        for s in iter {
            if s != first {
                all_same = false;
            }
        }
        if all_same { first } else { Self::Partial }
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

/// Visibility scope of a synchronization generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageVisibility {
    /// Data fetched from all repositories available to the account.
    All,
    /// Data fetched from public repositories only.
    PublicOnly,
}

/// Fetch implementation used to produce a coverage record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGeneration {
    /// A legacy scope whose completeness cannot be established.
    LegacyUnusable,
    /// Commit data fetched using the listing and search strategy.
    CommitsListingSearchV1,
    /// Pull request data fetched using the current sync strategy.
    PullRequestsV1,
    /// Issue data fetched using the current sync strategy.
    IssuesV1,
}

/// Repository visibility and fetch generation associated with coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CoverageScope {
    /// Whether the sync included private repositories.
    pub visibility: CoverageVisibility,
    /// The versioned fetch strategy used by the sync.
    pub generation: CoverageGeneration,
}

impl CoverageScope {
    /// Returns the stable scope label stored in the cache.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match (self.visibility, self.generation) {
            (CoverageVisibility::All, CoverageGeneration::LegacyUnusable) => "legacy_unusable",
            (CoverageVisibility::PublicOnly, CoverageGeneration::LegacyUnusable) => {
                "legacy_unusable_public_only"
            }
            (CoverageVisibility::All, CoverageGeneration::CommitsListingSearchV1) => "commits_v1",
            (CoverageVisibility::PublicOnly, CoverageGeneration::CommitsListingSearchV1) => {
                "commits_v1_public_only"
            }
            (CoverageVisibility::All, CoverageGeneration::PullRequestsV1) => "pull_requests_v1",
            (CoverageVisibility::PublicOnly, CoverageGeneration::PullRequestsV1) => {
                "pull_requests_v1_public_only"
            }
            (CoverageVisibility::All, CoverageGeneration::IssuesV1) => "issues_v1",
            (CoverageVisibility::PublicOnly, CoverageGeneration::IssuesV1) => {
                "issues_v1_public_only"
            }
        }
    }
}

impl From<&str> for CoverageScope {
    fn from(_: &str) -> Self {
        Self {
            visibility: CoverageVisibility::All,
            generation: CoverageGeneration::LegacyUnusable,
        }
    }
}

impl DataKind {
    /// All coverage kinds in stable declaration order.
    pub const ALL: [Self; 3] = [Self::Commits, Self::PullRequests, Self::Issues];

    /// Returns the stable serialized data kind name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::PullRequests => "pull_requests",
            Self::Issues => "issues",
        }
    }

    /// Parses a serialized data kind name.
    #[must_use]
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
    /// Kind of data covered by this record.
    pub kind: DataKind,
    /// Inclusive beginning of the covered range.
    pub start: Timestamp,
    /// Exclusive end of the covered range.
    pub end: Timestamp,
    /// Result of fetching the range.
    pub status: CoverageStatus,
    /// Repository visibility and fetch generation for this record.
    pub scope: CoverageScope,
    /// Human-readable explanation retained for reports and legacy cache rows.
    #[serde(default, skip_serializing)]
    pub description: String,
    /// Why the status is not complete, when applicable.
    /// Explanation for an incomplete or unavailable result, when known.
    pub reason: Option<String>,
    /// Time at which this coverage result was recorded.
    pub recorded_at: Timestamp,
}

/// A contiguous part of a requested range with a single status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSegment {
    /// Inclusive beginning of this segment.
    pub start: Timestamp,
    /// Exclusive end of this segment.
    pub end: Timestamp,
    /// Coverage state throughout this segment.
    pub status: CoverageStatus,
    /// Explanation for this segment's status, when known.
    pub reason: Option<String>,
}

/// Coverage of one data kind for a requested range.
/// Coverage for one data kind across a requested time range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSummary {
    /// Kind of data summarized.
    pub kind: DataKind,
    /// Combined status over the requested range.
    pub status: CoverageStatus,
    /// Distinct scopes represented in the range.
    pub scopes: Vec<String>,
    /// Ordered segments partitioning the requested range.
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
#[must_use]
pub fn summarize(
    records: &[CoverageRecord],
    kind: DataKind,
    start: Timestamp,
    end: Timestamp,
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
        // The most recent records covering this segment decide its status.
        // Records recorded at the same instant that disagree are combined
        // (never complete), so the result does not depend on record order.
        let covering: Vec<&&CoverageRecord> = relevant
            .iter()
            .filter(|r| r.start <= a && r.end >= b)
            .collect();
        let (status, reason) = covering.iter().map(|r| r.recorded_at).max().map_or(
            (CoverageStatus::NotYetSynced, None),
            |latest_at| {
                let latest: Vec<&&CoverageRecord> = covering
                    .into_iter()
                    .filter(|r| r.recorded_at == latest_at)
                    .collect();
                let status = CoverageStatus::combine(latest.iter().map(|r| r.status));
                let mut reasons: Vec<String> =
                    latest.iter().filter_map(|r| r.reason.clone()).collect();
                reasons.sort();
                reasons.dedup();
                (status, (!reasons.is_empty()).then(|| reasons.join("; ")))
            },
        );
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
    let mut scopes: Vec<String> = relevant.iter().map(|r| r.description.clone()).collect();
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
    fn t(d: u32) -> Timestamp {
        format!("2026-01-{d:02}T00:00:00Z").parse().unwrap()
    }

    fn rec(s: u32, e: u32, status: CoverageStatus, at: u32) -> CoverageRecord {
        CoverageRecord {
            kind: DataKind::Commits,
            start: t(s),
            end: t(e),
            status,
            scope: "commits".into(),
            description: "commits".into(),
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
