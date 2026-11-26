
/// Comment counts by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentCounts {
    /// Count across conversation, inline, and review summary comments.
    pub total: u64,
    /// Conversation comments on pull requests.
    pub conversation: u64,
    /// Comments attached to diff lines.
    pub inline: u64,
    /// Review body comments.
    pub review_summary: u64,
}

impl CommentCounts {
    /// Returns the total across all comment categories.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }
    const fn add(&mut self, t: EventType) {
        if matches!(
            t,
            EventType::ConversationComment | EventType::InlineComment | EventType::ReviewSummary
        ) {
            self.total += 1;
        }
        match t {
            EventType::ConversationComment => self.conversation += 1,
            EventType::InlineComment => self.inline += 1,
            EventType::ReviewSummary => self.review_summary += 1,
            _ => {}
        }
    }
}

/// Review submission counts by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCounts {
    /// Count of submitted reviews with a known or unknown state.
    pub total: u64,
    /// Reviews that approved the pull request.
    pub approved: u64,
    /// Reviews that requested changes.
    pub changes_requested: u64,
    /// Reviews that left comments without a decision.
    pub commented: u64,
    /// Dismissed reviews whose original state could not be observed.
    pub dismissed_unknown_state: u64,
}

impl ReviewCounts {
    const fn add(&mut self, state: Option<ReviewState>) {
        self.total += 1;
        match state {
            Some(ReviewState::Approved) => self.approved += 1,
            Some(ReviewState::ChangesRequested) => self.changes_requested += 1,
            Some(ReviewState::Commented) => self.commented += 1,
            Some(ReviewState::Dismissed) | None => self.dismissed_unknown_state += 1,
            Some(ReviewState::Pending) => {}
        }
    }
}

/// CI results observed in the period. Never part of human activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCounts {
    /// Check runs and commit statuses that succeeded.
    pub passed: u64,
    /// Check runs and commit statuses that failed.
    pub failed: u64,
    /// Checks that are queued or running.
    pub pending: u64,
    /// Checks with neutral, skipped, cancelled, or unknown outcomes.
    pub other: u64,
}

/// Additive activity counts for a period. Unique counts are labeled as such.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Distinct commits authored by the subject.
    pub commits: u64,
    /// Subject-authored commits with multiple parents.
    pub merge_commits: u64,
    /// Pull requests opened by the subject.
    pub prs_created: u64,
    /// The subject's pull requests merged in the period (by anyone).
    pub prs_merged: u64,
    /// Merges the subject performed (any author).
    pub merges_performed: u64,
    /// Merges of the subject's pull requests whose merger is unknown.
    pub merges_unknown_actor: u64,
    /// Subject pull requests closed without merging.
    pub prs_closed_without_merge: u64,
    /// Subject pull requests reopened.
    pub prs_reopened: u64,
    /// Reviews the subject submitted.
    pub reviews_submitted: ReviewCounts,
    /// Unique pull requests the subject reviewed.
    pub reviewed_prs: u64,
    /// Reviews submitted by others on the subject's pull requests.
    pub reviews_received: ReviewCounts,
    /// Received reviews submitted by bots.
    pub reviews_received_from_bots: u64,
    /// Reviews dismissed by the subject.
    pub review_dismissals_performed: u64,
    /// Comments written by the subject.
    pub comments_written: CommentCounts,
    /// Comments written by others on the subject's pull requests.
    pub comments_received: CommentCounts,
    /// Subject's replies in review threads started by others on the
    /// subject's pull requests.
    pub review_responses: u64,
    /// Distinct pull request commits authored by the subject.
    pub pr_commits: u64,
    /// Force pushes performed by the subject.
    pub force_pushes: u64,
    /// Reviewer request and assignment actions by the subject.
    pub coordination_actions: u64,
    /// Label, milestone, and title changes by the subject.
    pub metadata_actions: u64,
    /// Issues opened by the subject.
    pub issues_opened: u64,
    /// Issues closed by the subject.
    pub issues_closed: u64,
    /// Issues reopened by the subject.
    pub issues_reopened: u64,
    /// Unique pull requests with at least one event by the subject.
    pub active_prs: u64,
    /// Unique pull requests with at least one event by anyone.
    pub active_prs_all_participants: u64,
    /// The subject's events inside pull requests (automation excluded).
    pub pr_internal_events: u64,
    /// CI outcomes observed during the period.
    pub ci: CiCounts,
    /// Distinct local dates with activity by the subject.
    pub active_days: u64,
    /// Most recent activity time in the period, when any exists.
    pub last_activity: Option<Timestamp>,
    /// Lines added and deleted by the counted commits.
    pub diff_stats: DiffTotals,
}

/// How much of a set of commits has known diff statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCoverage {
    /// No commits to describe (none, or all excluded).
    #[default]
    NoCommits,
    /// Every commit has statistics.
    Complete,
    /// Some commits have statistics; the totals cover only those.
    Partial,
    /// No commit has statistics; the totals are unknown, not zero.
    Unavailable,
}

/// Lines added and deleted by the counted non-merge commits. Merge commits
/// are left out: GitHub reports their diff against the first parent, which
/// repeats the changes of the merged branch.
///
/// In JSON, `additions` and `deletions` are `null` when `coverage` is
/// `unavailable`: no commit has known counts, so the sums are unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "DiffTotalsJson", from = "DiffTotalsJson")]
pub struct DiffTotals {
    /// Non-merge commits counted in `commits`.
    pub commits: u64,
    /// Of those, commits left out because their changes are counted through
    /// other commits: they only exist in merged pull requests (squash or
    /// rebase merges) or were removed from a pull request by a force push.
    pub excluded: u64,
    /// Of the rest (`commits - excluded`), the commits with known
    /// statistics.
    pub commits_with_stats: u64,
    /// Sums over `commits_with_stats` commits only.
    pub additions: u64,
    /// Removed lines summed over commits with known statistics.
    pub deletions: u64,
    /// Completeness of the line statistics.
    pub coverage: DiffCoverage,
}

/// The JSON form of [`DiffTotals`].
#[derive(Serialize, Deserialize)]
struct DiffTotalsJson {
    commits: u64,
    excluded: u64,
    commits_with_stats: u64,
    additions: Option<u64>,
    deletions: Option<u64>,
    coverage: DiffCoverage,
}

impl From<DiffTotals> for DiffTotalsJson {
    fn from(d: DiffTotals) -> Self {
        let known = d.coverage != DiffCoverage::Unavailable;
        Self {
            commits: d.commits,
            excluded: d.excluded,
            commits_with_stats: d.commits_with_stats,
            additions: known.then_some(d.additions),
            deletions: known.then_some(d.deletions),
            coverage: d.coverage,
        }
    }
}

impl From<DiffTotalsJson> for DiffTotals {
    fn from(d: DiffTotalsJson) -> Self {
        Self {
            commits: d.commits,
            excluded: d.excluded,
            commits_with_stats: d.commits_with_stats,
            additions: d.additions.unwrap_or(0),
            deletions: d.deletions.unwrap_or(0),
            coverage: d.coverage,
        }
    }
}

impl DiffTotals {
    const fn finish(&mut self) {
        let counted = self.commits - self.excluded;
        self.coverage = if counted == 0 {
            DiffCoverage::NoCommits
        } else if self.commits_with_stats == counted {
            DiffCoverage::Complete
        } else if self.commits_with_stats == 0 {
            DiffCoverage::Unavailable
        } else {
            DiffCoverage::Partial
        };
    }
}

/// Computes a summary over `events` (already filtered to one scope).
fn summarize_events<'a, I>(ds: &Dataset, events: I, zone: Zone, basis: CommitDateBasis) -> Summary
where
    I: Iterator<Item = &'a ActivityEvent>,
{
    let mut acc = SummaryAccumulator::default();
    for event in events {
        acc.observe(ds, event, &zone, basis);
    }
    acc.finish()
}

#[derive(Default)]
struct SummaryAccumulator<'a> {
    summary: Summary,
    commits: HashSet<&'a str>,
    pr_commits: HashSet<(&'a str, &'a str)>,
    reviewed: HashSet<&'a str>,
    active_self: HashSet<&'a str>,
    active_all: HashSet<&'a str>,
    days: HashSet<Date>,
}

impl<'a> SummaryAccumulator<'a> {
    fn observe(
        &mut self,
        ds: &Dataset,
        event: &'a ActivityEvent,
        zone: &Zone,
        basis: CommitDateBasis,
    ) {
        let event_type = event.event_type;
        let by_subject = event.actor.is(&ds.subject);
        if let Some(pr) = event.parent_pr_id.as_deref() && !event_type.is_automation() {
            self.active_all.insert(pr);
            if by_subject {
                self.active_self.insert(pr);
            }
        }
        if by_subject && !event_type.is_automation() {
            self.days.insert(zone.local_date(event_time(event, basis)));
            self.summary.last_activity = self
                .summary
                .last_activity
                .max(Some(event_time(event, basis)));
            if event_type.is_pr_event() {
                self.summary.pr_internal_events += 1;
            }
        }
        let on_subject_pr = ds.pr_author_is_subject(event.parent_pr_id.as_deref());
        summarize_event(self, ds, event, by_subject, on_subject_pr);
    }

    fn finish(mut self) -> Summary {
        self.summary.commits = self.commits.len() as u64;
        self.summary.pr_commits = self.pr_commits.len() as u64;
        self.summary.reviewed_prs = self.reviewed.len() as u64;
        self.summary.active_prs = self.active_self.len() as u64;
        self.summary.active_prs_all_participants = self.active_all.len() as u64;
        self.summary.active_days = self.days.len() as u64;
        self.summary.diff_stats.finish();
        self.summary
    }
}

fn summarize_event<'a>(
    acc: &mut SummaryAccumulator<'a>,
    ds: &Dataset,
    event: &'a ActivityEvent,
    by_subject: bool,
    on_subject_pr: bool,
) {
    let event_type = event.event_type;
    match event_type {
        EventType::CommitAuthored if by_subject => {
            if acc.commits.insert(event.event_id.as_str()) {
                if event.meta.is_merge_commit == Some(true) {
                    acc.summary.merge_commits += 1;
                } else {
                    acc.summary.diff_stats.commits += 1;
                    if !ds.counts_lines(event.event_id.as_str()) {
                        acc.summary.diff_stats.excluded += 1;
                    } else if let Some(d) = ds.commit_stats.get(event.event_id.as_str()) {
                        acc.summary.diff_stats.commits_with_stats += 1;
                        acc.summary.diff_stats.additions += d.additions;
                        acc.summary.diff_stats.deletions += d.deletions;
                    }
                }
            }
        }
        EventType::PrCommit if by_subject => {
            if let (Some(pr), Some(sha)) = (
                event.parent_pr_id.as_deref(),
                event.meta.sha.as_deref(),
            ) {
                acc.pr_commits.insert((pr, sha));
            }
        }
        EventType::PrOpened if by_subject => acc.summary.prs_created += 1,
        EventType::PrMerged => {
            if on_subject_pr {
                acc.summary.prs_merged += 1;
                if event.actor.is_unknown() {
                    acc.summary.merges_unknown_actor += 1;
                }
            }
            if by_subject {
                acc.summary.merges_performed += 1;
            }
        }
        // Only closures known not to be part of a merge.
        EventType::PrClosed if on_subject_pr && event.meta.via_merge == Some(false) => {
            acc.summary.prs_closed_without_merge += 1;
        }
        EventType::PrReopened if on_subject_pr => acc.summary.prs_reopened += 1,
        EventType::PrHeadForcePushed if by_subject => acc.summary.force_pushes += 1,
        EventType::ReviewSubmitted => {
            if by_subject {
                acc.summary.reviews_submitted.add(event.meta.review_state);
                if let Some(pr) = event.parent_pr_id.as_deref() {
                    acc.reviewed.insert(pr);
                }
            } else if on_subject_pr {
                acc.summary.reviews_received.add(event.meta.review_state);
                if event.actor.is_bot() {
                    acc.summary.reviews_received_from_bots += 1;
                }
            }
        }
        EventType::ReviewDismissed if by_subject => {
            acc.summary.review_dismissals_performed += 1;
        }
        EventType::ConversationComment | EventType::InlineComment | EventType::ReviewSummary => {
            if by_subject {
                acc.summary.comments_written.add(event_type);
                if event_type == EventType::InlineComment
                    && on_subject_pr
                    && event.meta.reply_to.is_some()
                    && thread_started_by_other(ds, event)
                {
                    acc.summary.review_responses += 1;
                }
            } else if on_subject_pr {
                acc.summary.comments_received.add(event_type);
            }
        }
        t if by_subject && t.category() == EventCategory::PrCoordination => {
            acc.summary.coordination_actions += 1;
        }
        t if by_subject && t.category() == EventCategory::PrMetadata => {
            acc.summary.metadata_actions += 1;
        }
        EventType::IssueOpened if by_subject => acc.summary.issues_opened += 1,
        EventType::IssueClosed if by_subject => acc.summary.issues_closed += 1,
        EventType::IssueReopened if by_subject => acc.summary.issues_reopened += 1,
        EventType::CheckRun | EventType::CommitStatus => {
            match event.meta.check_outcome.unwrap_or(CheckOutcome::Unknown) {
                CheckOutcome::Success => acc.summary.ci.passed += 1,
                CheckOutcome::Failure => acc.summary.ci.failed += 1,
                CheckOutcome::Pending => acc.summary.ci.pending += 1,
                _ => acc.summary.ci.other += 1,
            }
        }
        _ => {}
    }
}

fn thread_started_by_other(ds: &Dataset, e: &ActivityEvent) -> bool {
    e.meta
        .thread_id
        .as_deref()
        .and_then(|id| ds.threads.get(id))
        .and_then(|t| t.first_comment_author.as_ref())
        .is_some_and(|a| !a.is(&ds.subject) && !a.is_unknown())
}

/// Summary for the whole period.
#[must_use]
pub fn summary(ds: &Dataset, opts: &Options) -> Summary {
    summarize_events(ds, selected(ds, opts), opts.zone.clone(), opts.commit_basis)
}

