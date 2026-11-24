/// Lifetime detail of one pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDetail {
    /// Current pull request snapshot.
    pub pr: PullRequest,
    /// Repository display name.
    pub repository: String,
    /// Commits currently in the pull request.
    pub commits: u64,
    /// Commits observed earlier but no longer in the pull request.
    pub removed_commits: u64,
    /// Review submission totals.
    pub reviews: ReviewCounts,
    /// Comment totals by type.
    pub comments: CommentCounts,
    /// Current review thread totals.
    pub threads: ThreadCounts,
    /// Distinct actors in countable pull request events.
    pub participants: Vec<String>,
    /// People who wrote a conversation, inline or review summary comment.
    pub discussion_participants: Vec<String>,
    /// Current commits by commit author, most commits first.
    /// Current commit totals grouped by author.
    pub commit_authors: Vec<AuthorCount>,
    /// Lifecycle timings derived from observed events.
    pub timing: PrTiming,
    /// Pull request event timeline.
    pub timeline: Vec<ActivityEvent>,
    /// Base-to-head diff when last fetched (a snapshot).
    pub diff: Option<PrDiff>,
}

/// Number of commits by one author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorCount {
    /// GitHub login or display name.
    pub author: String,
    /// Number of commits attributed to this author.
    pub commits: u64,
}

/// Full detail and timeline of a pull request. `types` filters the
/// timeline only; counts always cover every event.
#[must_use]
pub fn pr_detail(ds: &Dataset, pr_id: &str, types: &[EventType]) -> Option<PrDetail> {
    let pr = ds.pull_requests.get(pr_id)?;
    let mut commits = HashSet::new();
    let mut removed_commits = HashSet::new();
    let mut reviews = ReviewCounts::default();
    let mut comments = CommentCounts::default();
    let mut participants = BTreeSet::new();
    let mut discussion = BTreeSet::new();
    let mut authors: HashMap<String, String> = HashMap::new();
    let mut timeline = Vec::new();
    for e in ds
        .events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr_id))
    {
        if e.event_type == EventType::PrCommit {
            let sha = e.meta.sha.clone().unwrap_or_default();
            if e.is_removed() {
                removed_commits.insert(sha);
            } else {
                authors.insert(sha.to_ascii_lowercase(), e.actor.display().to_string());
                commits.insert(sha);
            }
        }
        if is_countable(e) {
            match e.event_type {
                EventType::ReviewSubmitted => reviews.add(e.meta.review_state),
                t @ (EventType::ConversationComment
                | EventType::InlineComment
                | EventType::ReviewSummary) => comments.add(t),
                _ => {}
            }
            if !e.event_type.is_automation() && !e.actor.is_unknown() {
                participants.insert(e.actor.display().to_string());
                if e.event_type.is_discussion() {
                    discussion.insert(e.actor.display().to_string());
                }
            }
        }
        if types.is_empty() || types.contains(&e.event_type) {
            timeline.push(e.clone());
        }
    }
    let mut by_author: BTreeMap<String, u64> = BTreeMap::new();
    for author in authors.into_values() {
        *by_author.entry(author).or_default() += 1;
    }
    let mut commit_authors: Vec<AuthorCount> = by_author
        .into_iter()
        .map(|(author, commits)| AuthorCount { author, commits })
        .collect();
    commit_authors.sort_by(|a, b| b.commits.cmp(&a.commits).then(a.author.cmp(&b.author)));
    let mut threads = ThreadCounts::default();
    for t in ds.threads.values().filter(|t| t.pr_id == pr_id) {
        threads.add(t);
    }
    Some(PrDetail {
        pr: pr.clone(),
        repository: ds.repo_name(&pr.repository_id),
        commits: commits.len() as u64,
        removed_commits: removed_commits.len() as u64,
        reviews,
        comments,
        threads,
        participants: participants.into_iter().collect(),
        discussion_participants: discussion.into_iter().collect(),
        commit_authors,
        timing: pr_timing(ds, pr),
        timeline,
        diff: ds.pr_diffs.get(&pr.id).copied(),
    })
}

/// One commit of a pull request, for listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCommitRow {
    /// Commit SHA.
    pub sha: String,
    /// First line of the commit message, when it was fetched.
    pub headline: Option<String>,
    /// The commit author's GitHub login, or `unknown`.
    pub author: String,
    /// Commit author date, when available.
    pub authored_at: Option<Timestamp>,
    /// Committer date.
    pub committed_at: Timestamp,
    /// Whether the commit has multiple parents.
    pub is_merge_commit: bool,
    /// GitHub commit URL, when available.
    pub url: Option<String>,
    /// The commit was in the pull request earlier but no longer is (for
    /// example after a force push).
    pub removed: bool,
    /// Lines added and deleted by the commit, when known.
    pub lines: Option<LineCounts>,
}

/// The commits of a pull request, current and removed, one row per SHA,
/// oldest first. Rows are not limited to a period: like [`pr_detail`] they
/// describe the pull request as a whole.
#[must_use]
pub fn pr_commit_rows(ds: &Dataset, pr_id: &str) -> Vec<PrCommitRow> {
    let mut by_sha: BTreeMap<String, PrCommitRow> = BTreeMap::new();
    for e in ds
        .events
        .iter()
        .filter(|e| e.event_type == EventType::PrCommit && e.parent_pr_id.as_deref() == Some(pr_id))
    {
        let sha = e.meta.sha.clone().unwrap_or_else(|| e.entity_id.clone());
        let row = PrCommitRow {
            sha: sha.clone(),
            headline: e.meta.message_headline.clone(),
            author: e.actor.display().to_string(),
            authored_at: e.meta.author_date,
            committed_at: e.occurred_at,
            is_merge_commit: e.meta.is_merge_commit == Some(true),
            url: e.url.clone(),
            removed: e.is_removed(),
            lines: ds
                .commit_stats
                .get(crate::model::EventId::commit(&e.repository_id, &sha).as_str())
                .copied(),
        };
        match by_sha.get_mut(&sha.to_ascii_lowercase()) {
            // A commit still in the pull request wins over a removed copy.
            Some(existing) if existing.removed && !row.removed => *existing = row,
            Some(_) => {}
            None => {
                by_sha.insert(sha.to_ascii_lowercase(), row);
            }
        }
    }
    let mut rows: Vec<PrCommitRow> = by_sha.into_values().collect();
    rows.sort_by(|a, b| (a.committed_at, &a.sha).cmp(&(b.committed_at, &b.sha)));
    rows
}

/// A review row for the reviews view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRow {
    /// Stable review event identifier.
    pub event_id: String,
    /// Review submission time.
    pub submitted_at: Timestamp,
    /// Reviewer login or display name.
    pub reviewer: String,
    /// Review decision, when known.
    pub state: Option<ReviewState>,
    /// Repository display name.
    pub repository: String,
    /// The reviewed pull request, when known.
    pub pr_id: Option<String>,
    /// Pull request number, when available.
    pub pr_number: Option<u64>,
    /// Pull request title, when available.
    pub pr_title: Option<String>,
    /// GitHub review URL, when available.
    pub url: Option<String>,
}

/// Whether review rows include submitted reviews or reviews received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDirection {
    /// Reviews submitted by the subject.
    Submitted,
    /// Reviews received on pull requests authored by the subject.
    Received,
}

/// Reviews submitted by the subject, or received on the subject's PRs.
#[must_use]
pub fn review_rows(ds: &Dataset, opts: &Options, dir: ReviewDirection) -> Vec<ReviewRow> {
    selected(ds, opts)
        .filter(|e| e.event_type == EventType::ReviewSubmitted)
        .filter(|e| match dir {
            ReviewDirection::Submitted => e.actor.is(&ds.subject),
            ReviewDirection::Received => {
                !e.actor.is(&ds.subject) && ds.pr_author_is_subject(e.parent_pr_id.as_deref())
            }
        })
        .map(|e| {
            let pr = e
                .parent_pr_id
                .as_deref()
                .and_then(|id| ds.pull_requests.get(id));
            ReviewRow {
                event_id: e.event_id.0.clone(),
                submitted_at: e.occurred_at,
                reviewer: e.actor.display().to_string(),
                state: e.meta.review_state,
                repository: ds.repo_name(&e.repository_id),
                pr_id: e.parent_pr_id.clone(),
                pr_number: pr.map(|p| p.number),
                pr_title: pr.map(|p| p.title.clone()),
                url: e.url.clone(),
            }
        })
        .collect()
}

/// Issue activity row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRow {
    /// GitHub node ID of the issue.
    pub issue_id: String,
    /// Repository display name.
    pub repository: String,
    /// Issue number within the repository.
    pub number: u64,
    /// Current issue title.
    pub title: String,
    /// GitHub issue URL.
    pub url: String,
    /// Whether the issue is currently open.
    pub is_open: bool,
    /// Whether the issue was opened by the subject during the period.
    pub opened_in_period: bool,
    /// Number of subject close events during the period.
    pub closed_in_period: u64,
    /// Number of subject reopen events during the period.
    pub reopened_in_period: u64,
}

/// Issues with subject activity in the period. Pull requests never appear.
#[must_use]
pub fn issue_rows(ds: &Dataset, opts: &Options) -> Vec<IssueRow> {
    let mut per: BTreeMap<&str, IssueRow> = BTreeMap::new();
    for e in selected(ds, opts) {
        if e.event_type.category() != EventCategory::Issue || !e.actor.is(&ds.subject) {
            continue;
        }
        let Some(issue) = ds.issues.get(&e.entity_id) else {
            continue;
        };
        let row = per.entry(issue.id.as_str()).or_insert_with(|| IssueRow {
            issue_id: issue.id.clone(),
            repository: ds.repo_name(&issue.repository_id),
            number: issue.number,
            title: issue.title.clone(),
            url: issue.url.clone(),
            is_open: issue.is_open,
            opened_in_period: false,
            closed_in_period: 0,
            reopened_in_period: 0,
        });
        match e.event_type {
            EventType::IssueOpened => row.opened_in_period = true,
            EventType::IssueClosed => row.closed_in_period += 1,
            EventType::IssueReopened => row.reopened_in_period += 1,
            _ => {}
        }
    }
    let mut rows: Vec<IssueRow> = per.into_values().collect();
    rows.sort_by(|a, b| {
        a.repository
            .cmp(&b.repository)
            .then(a.number.cmp(&b.number))
    });
    rows
}

/// A comment row for the comments view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRow {
    /// Stable comment event identifier.
    pub event_id: String,
    /// Comment creation time.
    pub created_at: Timestamp,
    /// Comment author login or display name.
    pub author: String,
    /// Comment event kind.
    pub kind: EventType,
    /// Whether the subject wrote this comment.
    pub written_by_subject: bool,
    /// Repository display name.
    pub repository: String,
    /// The pull request the comment belongs to, when known.
    pub pr_id: Option<String>,
    /// Pull request number, when available.
    pub pr_number: Option<u64>,
    /// Whether the comment was edited.
    pub edited: bool,
    /// GitHub comment URL, when available.
    pub url: Option<String>,
}

/// Comments written by the subject or received on the subject's PRs.
#[must_use]
pub fn comment_rows(ds: &Dataset, opts: &Options) -> Vec<CommentRow> {
    selected(ds, opts)
        .filter(|e| {
            matches!(
                e.event_type,
                EventType::ConversationComment
                    | EventType::InlineComment
                    | EventType::ReviewSummary
            )
        })
        .filter(|e| e.actor.is(&ds.subject) || ds.pr_author_is_subject(e.parent_pr_id.as_deref()))
        .map(|e| CommentRow {
            event_id: e.event_id.0.clone(),
            created_at: e.occurred_at,
            author: e.actor.display().to_string(),
            kind: e.event_type,
            written_by_subject: e.actor.is(&ds.subject),
            repository: ds.repo_name(&e.repository_id),
            pr_id: e.parent_pr_id.clone(),
            pr_number: e
                .parent_pr_id
                .as_deref()
                .and_then(|id| ds.pull_requests.get(id))
                .map(|p| p.number),
            edited: e.meta.edited_at.is_some(),
            url: e.url.clone(),
        })
        .collect()
}

/// A commit authored by the subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRow {
    /// Stable commit event identifier.
    pub event_id: String,
    /// Commit SHA, when available.
    pub sha: Option<String>,
    /// The date the commit is attributed to (see [`CommitDateBasis`]).
    pub committed_at: Timestamp,
    /// Repository display name.
    pub repository: String,
    /// Whether the commit has multiple parents.
    pub is_merge_commit: bool,
    /// GitHub commit URL, when available.
    pub url: Option<String>,
    /// First line of the commit message, when it was fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_headline: Option<String>,
}

/// The subject's distinct commits in the period, oldest first. There is
/// exactly one row per commit counted in [`Summary::commits`].
#[must_use]
pub fn commit_rows(ds: &Dataset, opts: &Options) -> Vec<CommitRow> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut rows: Vec<CommitRow> = selected(ds, opts)
        .filter(|e| e.event_type == EventType::CommitAuthored && e.actor.is(&ds.subject))
        .filter(|e| seen.insert(e.event_id.as_str()))
        .map(|e| CommitRow {
            event_id: e.event_id.0.clone(),
            sha: e.meta.sha.clone(),
            committed_at: event_time(e, opts.commit_basis),
            repository: ds.repo_name(&e.repository_id),
            is_merge_commit: e.meta.is_merge_commit == Some(true),
            url: e.url.clone(),
            message_headline: e.meta.message_headline.clone(),
        })
        .collect();
    rows.sort_by(|a, b| (a.committed_at, &a.event_id).cmp(&(b.committed_at, &b.event_id)));
    rows
}

/// One event in an activity listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRow {
    /// Stable event identifier.
    pub event_id: String,
    /// The instant the event is attributed to (see [`event_time`]).
    pub occurred_at: Timestamp,
    /// Event kind.
    pub event_type: EventType,
    /// Actor login or display name.
    pub actor: String,
    /// Whether the subject performed the event.
    pub by_subject: bool,
    /// Repository display name.
    pub repository: String,
    /// The pull request the event belongs to, when known.
    pub pr_id: Option<String>,
    /// The issue the event belongs to, when known.
    /// Issue identifier when the event belongs to an issue.
    pub issue_id: Option<String>,
    /// Number of the pull request or issue.
    pub number: Option<u64>,
    /// Title of the pull request or issue.
    pub title: Option<String>,
    /// GitHub URL for the event, when available.
    pub url: Option<String>,
}

fn activity_row(ds: &Dataset, e: &ActivityEvent, basis: CommitDateBasis) -> ActivityRow {
    let pr = e
        .parent_pr_id
        .as_deref()
        .and_then(|id| ds.pull_requests.get(id));
    let issue = (e.event_type.category() == EventCategory::Issue)
        .then(|| ds.issues.get(&e.entity_id))
        .flatten();
    ActivityRow {
        event_id: e.event_id.0.clone(),
        occurred_at: event_time(e, basis),
        event_type: e.event_type,
        actor: e.actor.display().to_string(),
        by_subject: e.actor.is(&ds.subject),
        repository: ds.repo_name(&e.repository_id),
        pr_id: e.parent_pr_id.clone(),
        issue_id: issue.map(|i| i.id.clone()),
        number: pr.map(|p| p.number).or_else(|| issue.map(|i| i.number)),
        title: pr
            .map(|p| p.title.clone())
            .or_else(|| issue.map(|i| i.title.clone())),
        url: e.url.clone(),
    }
}

fn by_time(rows: &mut [ActivityRow]) {
    rows.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
}

/// Activity in the period, oldest first: every human event by the subject,
/// plus reviews and comments others left on the subject's pull requests.
/// Automation (CI) is never listed.
#[must_use]
pub fn activity_rows(ds: &Dataset, opts: &Options) -> Vec<ActivityRow> {
    let mut rows: Vec<ActivityRow> = selected(ds, opts)
        .filter(|e| !e.event_type.is_automation())
        .filter(|e| {
            e.actor.is(&ds.subject)
                || (matches!(
                    e.event_type,
                    EventType::ReviewSubmitted
                        | EventType::ConversationComment
                        | EventType::InlineComment
                        | EventType::ReviewSummary
                ) && ds.pr_author_is_subject(e.parent_pr_id.as_deref()))
        })
        .map(|e| activity_row(ds, e, opts.commit_basis))
        .collect();
    by_time(&mut rows);
    rows
}

/// One issue and its events in the period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    /// Current issue snapshot.
    pub issue: Issue,
    /// Repository display name.
    pub repository: String,
    /// Events of the issue in the period, by anyone, oldest first.
    pub events: Vec<ActivityRow>,
}

/// Detail of one issue: its current state and its events in the period.
#[must_use]
pub fn issue_detail(ds: &Dataset, opts: &Options, issue_id: &str) -> Option<IssueDetail> {
    let issue = ds.issues.get(issue_id)?;
    let mut events: Vec<ActivityRow> = selected(ds, opts)
        .filter(|e| e.event_type.category() == EventCategory::Issue && e.entity_id == issue_id)
        .map(|e| activity_row(ds, e, opts.commit_basis))
        .collect();
    by_time(&mut events);
    Some(IssueDetail {
        issue: issue.clone(),
        repository: ds.repo_name(&issue.repository_id),
        events,
    })
}

/// Events of one pull request in deterministic order, optionally filtered.
#[must_use]
pub fn timeline<'a>(ds: &'a Dataset, pr_id: &str, types: &[EventType]) -> Vec<&'a ActivityEvent> {
    ds.events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr_id))
        .filter(|e| types.is_empty() || types.contains(&e.event_type))
        .collect()
}
