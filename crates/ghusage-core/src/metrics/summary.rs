
/// Comment counts by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentCounts {
    pub total: u64,
    pub conversation: u64,
    pub inline: u64,
    pub review_summary: u64,
}

impl CommentCounts {
    pub fn total(&self) -> u64 {
        self.total
    }
    fn add(&mut self, t: EventType) {
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
    pub total: u64,
    pub approved: u64,
    pub changes_requested: u64,
    pub commented: u64,
    /// Dismissed reviews whose original state could not be observed.
    pub dismissed_unknown_state: u64,
}

impl ReviewCounts {
    fn add(&mut self, state: Option<ReviewState>) {
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
    pub passed: u64,
    pub failed: u64,
    pub pending: u64,
    pub other: u64,
}

/// Additive activity counts for a period. Unique counts are labeled as such.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Distinct commits authored by the subject.
    pub commits: u64,
    pub merge_commits: u64,
    pub prs_created: u64,
    /// The subject's pull requests merged in the period (by anyone).
    pub prs_merged: u64,
    /// Merges the subject performed (any author).
    pub merges_performed: u64,
    /// Merges of the subject's pull requests whose merger is unknown.
    pub merges_unknown_actor: u64,
    pub prs_closed_without_merge: u64,
    pub prs_reopened: u64,
    pub reviews_submitted: ReviewCounts,
    /// Unique pull requests the subject reviewed.
    pub reviewed_prs: u64,
    pub reviews_received: ReviewCounts,
    pub reviews_received_from_bots: u64,
    pub review_dismissals_performed: u64,
    pub comments_written: CommentCounts,
    pub comments_received: CommentCounts,
    /// Subject's replies in review threads started by others on the
    /// subject's pull requests.
    pub review_responses: u64,
    /// Distinct pull request commits authored by the subject.
    pub pr_commits: u64,
    pub force_pushes: u64,
    pub coordination_actions: u64,
    pub metadata_actions: u64,
    pub issues_opened: u64,
    pub issues_closed: u64,
    pub issues_reopened: u64,
    /// Unique pull requests with at least one event by the subject.
    pub active_prs: u64,
    /// Unique pull requests with at least one event by anyone.
    pub active_prs_all_participants: u64,
    /// The subject's events inside pull requests (automation excluded).
    pub pr_internal_events: u64,
    pub ci: CiCounts,
    /// Distinct local dates with activity by the subject.
    pub active_days: u64,
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
    pub deletions: u64,
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
    fn finish(&mut self) {
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
fn summarize_events<'a>(
    ds: &Dataset,
    events: impl Iterator<Item = &'a ActivityEvent>,
    zone: Zone,
    basis: CommitDateBasis,
) -> Summary {
    let mut s = Summary::default();
    let subject = &ds.subject;
    let mut commits: HashSet<&str> = HashSet::new();
    let mut pr_commits: HashSet<(&str, &str)> = HashSet::new();
    let mut reviewed: HashSet<&str> = HashSet::new();
    let mut active_self: HashSet<&str> = HashSet::new();
    let mut active_all: HashSet<&str> = HashSet::new();
    let mut days: HashSet<Date> = HashSet::new();

    for e in events {
        let t = e.event_type;
        let by_subject = e.actor.is(subject);
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !t.is_automation()
        {
            active_all.insert(pr);
            if by_subject {
                active_self.insert(pr);
            }
        }
        if by_subject && !t.is_automation() {
            days.insert(zone.local_date(event_time(e, basis)));
            s.last_activity = s.last_activity.max(Some(event_time(e, basis)));
            if t.is_pr_event() {
                s.pr_internal_events += 1;
            }
        }
        let on_subject_pr = ds.pr_author_is_subject(e.parent_pr_id.as_deref());
        match t {
            EventType::CommitAuthored if by_subject => {
                if commits.insert(e.event_id.as_str()) {
                    if e.meta.is_merge_commit == Some(true) {
                        s.merge_commits += 1;
                    } else {
                        s.diff_stats.commits += 1;
                        if !ds.counts_lines(e.event_id.as_str()) {
                            s.diff_stats.excluded += 1;
                        } else if let Some(d) = ds.commit_stats.get(e.event_id.as_str()) {
                            s.diff_stats.commits_with_stats += 1;
                            s.diff_stats.additions += d.additions;
                            s.diff_stats.deletions += d.deletions;
                        }
                    }
                }
            }
            EventType::PrCommit if by_subject => {
                if let (Some(pr), Some(sha)) = (e.parent_pr_id.as_deref(), e.meta.sha.as_deref()) {
                    pr_commits.insert((pr, sha));
                }
            }
            EventType::PrOpened if by_subject => s.prs_created += 1,
            EventType::PrMerged => {
                if on_subject_pr {
                    s.prs_merged += 1;
                    if e.actor.is_unknown() {
                        s.merges_unknown_actor += 1;
                    }
                }
                if by_subject {
                    s.merges_performed += 1;
                }
            }
            // Only closures known not to be part of a merge.
            EventType::PrClosed if on_subject_pr && e.meta.via_merge == Some(false) => {
                s.prs_closed_without_merge += 1;
            }
            EventType::PrReopened if on_subject_pr => s.prs_reopened += 1,
            EventType::PrHeadForcePushed if by_subject => s.force_pushes += 1,
            EventType::ReviewSubmitted => {
                if by_subject {
                    s.reviews_submitted.add(e.meta.review_state);
                    if let Some(pr) = e.parent_pr_id.as_deref() {
                        reviewed.insert(pr);
                    }
                } else if on_subject_pr {
                    s.reviews_received.add(e.meta.review_state);
                    if e.actor.is_bot() {
                        s.reviews_received_from_bots += 1;
                    }
                }
            }
            EventType::ReviewDismissed if by_subject => s.review_dismissals_performed += 1,
            EventType::ConversationComment
            | EventType::InlineComment
            | EventType::ReviewSummary => {
                if by_subject {
                    s.comments_written.add(t);
                    if t == EventType::InlineComment
                        && on_subject_pr
                        && e.meta.reply_to.is_some()
                        && thread_started_by_other(ds, e)
                    {
                        s.review_responses += 1;
                    }
                } else if on_subject_pr {
                    s.comments_received.add(t);
                }
            }
            t if by_subject && t.category() == EventCategory::PrCoordination => {
                s.coordination_actions += 1;
            }
            t if by_subject && t.category() == EventCategory::PrMetadata => {
                s.metadata_actions += 1;
            }
            EventType::IssueOpened if by_subject => s.issues_opened += 1,
            EventType::IssueClosed if by_subject => s.issues_closed += 1,
            EventType::IssueReopened if by_subject => s.issues_reopened += 1,
            EventType::CheckRun | EventType::CommitStatus => {
                match e.meta.check_outcome.unwrap_or(CheckOutcome::Unknown) {
                    CheckOutcome::Success => s.ci.passed += 1,
                    CheckOutcome::Failure => s.ci.failed += 1,
                    CheckOutcome::Pending => s.ci.pending += 1,
                    _ => s.ci.other += 1,
                }
            }
            _ => {}
        }
    }
    s.commits = commits.len() as u64;
    s.pr_commits = pr_commits.len() as u64;
    s.reviewed_prs = reviewed.len() as u64;
    s.active_prs = active_self.len() as u64;
    s.active_prs_all_participants = active_all.len() as u64;
    s.active_days = days.len() as u64;
    s.diff_stats.finish();
    s
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
pub fn summary(ds: &Dataset, opts: &Options) -> Summary {
    summarize_events(ds, selected(ds, opts), opts.zone.clone(), opts.commit_basis)
}

