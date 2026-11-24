//! Synchronization of GitHub data into the local cache.
//!
//! Each pull request and issue is written in its own transaction, so an
//! interrupted sync keeps everything written so far and a rerun continues
//! with what is missing. Coverage records describe exactly which ranges
//! were fetched and how completely.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use futures::StreamExt;
use ghusage_core::coverage::{
    self, CoverageGeneration, CoverageRecord, CoverageScope, CoverageStatus, CoverageVisibility,
    DataKind,
};
use ghusage_core::model::{
    ActivityEvent, Collection, EventId, EventType, PrState, Repository, RepositoryRef, Source,
    Subject,
};
use ghusage_core::time::Period;
use ghusage_github::{
    ApiError, ApiStats, CommitRecord, CommitRef, GitHubClient, IssueKind, IssueRef, PrBundle,
    PushedRepository,
};
use ghusage_storage::{CommitStats, PrCommitLink, PrSnapshot, Store, WriteBatch};
use jiff::{SignedDuration, Timestamp};

use crate::error::{AppError, Result};

/// Metadata keys remembering the authenticated user for offline use.
pub const META_VIEWER_LOGIN: &str = "viewer_login";
/// Key for the authenticated viewer's GitHub node ID.
pub const META_VIEWER_ID: &str = "viewer_id";

/// Controls which ranges and repositories a synchronization fetches.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Requested time range.
    pub period: Period,
    /// Restrict to public repositories.
    pub public_only: bool,
    /// Refetch the whole period instead of only missing ranges.
    pub full: bool,
    /// Current time used for freshness and coverage decisions.
    pub now: Timestamp,
    /// How far before a missing range to start refetching, to catch data
    /// GitHub indexed late.
    pub overlap: SignedDuration,
    /// Open pull requests not refreshed for this long are refetched.
    pub recheck_open_after: SignedDuration,
    /// Number of pull requests/issues fetched concurrently.
    pub concurrency: usize,
    /// Whether the subject is the authenticated user. Repository discovery
    /// for commits then also sees collaborator and organization
    /// repositories; for other users only their own repositories are
    /// visible.
    pub subject_is_viewer: bool,
}

impl SyncOptions {
    /// Creates default sync settings for the requested period.
    #[must_use]
    pub const fn new(period: Period, now: Timestamp) -> Self {
        Self {
            period,
            public_only: false,
            full: false,
            now,
            overlap: SignedDuration::from_secs(3 * 24 * 60 * 60),
            recheck_open_after: SignedDuration::from_secs(7 * 24 * 60 * 60),
            concurrency: 8,
            subject_is_viewer: true,
        }
    }
}

/// Progress notifications for interactive display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncProgress {
    /// Searching GitHub for the selected data kind.
    Searching(DataKind),
    /// Fetching the discovered records of a data kind.
    Fetching {
        /// Data kind being fetched.
        kind: DataKind,
        /// Number of records completed so far.
        done: usize,
        /// Total number of records discovered.
        total: usize,
    },
    /// The latest step, e.g. a repository or pull request just fetched.
    Detail(String),
    /// Synchronization completed.
    Finished,
}

/// What a sync did.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SyncReport {
    /// Overall result, such as `success`, `partial`, or `failed`.
    pub status: String,
    /// Commits discovered during the run.
    pub commits_found: u64,
    /// Pull requests discovered during the run.
    pub pull_requests_found: u64,
    /// Pull requests fetched during the run.
    pub pull_requests_fetched: u64,
    /// Pull requests skipped because cached data was current.
    pub pull_requests_unchanged: u64,
    /// Issues discovered during the run.
    pub issues_found: u64,
    /// Issues fetched during the run.
    pub issues_fetched: u64,
    /// Issues skipped because cached data was current.
    pub issues_unchanged: u64,
    /// Errors encountered while fetching individual resources.
    pub failures: Vec<String>,
    /// Coverage records written by the run.
    pub coverage: Vec<CoverageRecord>,
    /// API request statistics for the run.
    pub api: ApiStats,
    /// Line counts fetched with `--diff-stats`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_stats: Option<LineCountSync>,
}

/// What fetching commit line counts did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LineCountSync {
    /// The subject's non-merge commits in the period whose line counts go
    /// into the totals.
    pub commits: u64,
    /// Of those, already known before this run (not fetched again).
    pub already_known: u64,
    /// Fetched now.
    pub fetched: u64,
    /// Asked for but not returned by GitHub (left unknown).
    pub unavailable: u64,
}

impl SyncReport {
    /// Whether the run completed without failures or coverage gaps.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && self.coverage.iter().all(|c| c.status.is_complete())
    }
}

include!("sync/subject.rs");
include!("sync/coverage.rs");
include!("sync/batch.rs");
include!("sync/line_counts.rs");
include!("sync/prs.rs");
include!("sync/issues.rs");
include!("sync/commits.rs");
