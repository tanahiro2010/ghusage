/// Normalized activity and current GitHub state used by report calculations.
#[derive(Debug, Clone)]
pub struct Dataset {
    /// Account whose activity is summarized.
    pub subject: Subject,
    /// Whether the subject is the authenticated viewer (affects ownership).
    pub subject_is_viewer: bool,
    /// Repository snapshots indexed by GitHub node ID.
    pub repositories: BTreeMap<String, Repository>,
    /// Pull request snapshots indexed by GitHub node ID.
    pub pull_requests: BTreeMap<String, PullRequest>,
    /// Issue snapshots indexed by GitHub node ID.
    pub issues: BTreeMap<String, Issue>,
    /// Current review threads indexed by GitHub node ID.
    pub threads: BTreeMap<String, ReviewThread>,
    /// Events, deduplicated by event id and sorted by (time, rank, id).
    pub events: Vec<ActivityEvent>,
    /// Coverage records from synchronization runs.
    pub coverage: Vec<CoverageRecord>,
    /// Lines added and deleted per commit, keyed by the commit's event id
    /// ([`EventId::commit`](crate::model::EventId::commit)). Commits without
    /// an entry have no known statistics (not zero).
    pub commit_stats: HashMap<String, LineCounts>,
    /// Latest observed diff of each pull request (a snapshot, not history).
    pub pr_diffs: BTreeMap<String, PrDiff>,
    /// Commits (event ids) whose line counts stay out of the totals (see
    /// [`Dataset::with_line_count_rule`]).
    pub lines_excluded: HashSet<String>,
}

/// Lines added and deleted by one commit, as reported by GitHub (for a
/// merge commit: against its first parent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCounts {
    /// Lines added by the commit.
    pub additions: u64,
    /// Lines deleted by the commit.
    pub deletions: u64,
}

/// A pull request's diff between its base and head when it was observed.
/// It changes with pushes, force pushes and base updates, so it describes
/// the pull request at `observed_at`, not activity in a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrDiff {
    /// Lines added in the observed pull request diff.
    pub additions: u64,
    /// Lines deleted in the observed pull request diff.
    pub deletions: u64,
    /// Number of changed files in the observed diff.
    pub changed_files: u64,
    /// Time the diff snapshot was fetched.
    pub observed_at: Timestamp,
}

impl Dataset {
    /// Builds a dataset. Duplicate events (same id) are collapsed; the most
    /// recently observed copy is kept so a resync's state wins.
        pub fn new(
        subject: Subject,
        subject_is_viewer: bool,
        repositories: impl IntoIterator<Item = Repository>,
        pull_requests: impl IntoIterator<Item = PullRequest>,
        issues: impl IntoIterator<Item = Issue>,
        threads: impl IntoIterator<Item = ReviewThread>,
        events: impl IntoIterator<Item = ActivityEvent>,
        coverage: Vec<CoverageRecord>,
    ) -> Self {
        let mut by_id: HashMap<String, ActivityEvent> = HashMap::new();
        for e in events {
            match by_id.get(e.event_id.as_str()) {
                Some(existing)
                    if (existing.observed_at, existing.source.as_str())
                        >= (e.observed_at, e.source.as_str()) => {}
                _ => {
                    by_id.insert(e.event_id.0.clone(), e);
                }
            }
        }
        let mut events: Vec<ActivityEvent> = by_id.into_values().collect();
        mark_thread_reply_reviews(&mut events);
        sort_events(&mut events);
        Self {
            subject,
            subject_is_viewer,
            repositories: repositories
                .into_iter()
                .map(|r| (r.id.clone(), r))
                .collect(),
            pull_requests: pull_requests
                .into_iter()
                .map(|p| (p.id.clone(), p))
                .collect(),
            issues: issues.into_iter().map(|i| (i.id.clone(), i)).collect(),
            threads: threads.into_iter().map(|t| (t.id.clone(), t)).collect(),
            events,
            coverage,
            commit_stats: HashMap::new(),
            pr_diffs: BTreeMap::new(),
            lines_excluded: HashSet::new(),
        }
        .with_line_count_rule(&HashSet::new())
    }

    /// Adds commit statistics and pull request diffs.
    #[must_use]
    pub fn with_diff_stats(
        mut self,
        commit_stats: HashMap<String, LineCounts>,
        pr_diffs: BTreeMap<String, PrDiff>,
    ) -> Self {
        self.commit_stats = commit_stats;
        self.pr_diffs = pr_diffs;
        self
    }

    /// Sets which commits' line counts stay out of the totals: commits that
    /// appear in pull requests of the dataset (by anyone), where none of those
    /// appearances is current in a pull request that is open, closed
    /// without merging, merged with a merge commit
    /// (`merged_by_merge_commit`, pull request ids), or authored by someone
    /// other than the subject. Such commits only exist in the subject's
    /// pull requests merged by squash or rebase (or by an unknown method),
    /// or were removed from a pull request by a force push: their changes
    /// reach the default branch as other commits of the subject, so
    /// counting them would count the same change twice. A squash commit is
    /// authored by the pull request's author, so in someone else's pull
    /// request the branch commits are the only place the subject's changes
    /// count. Commits that never appear in a pull request count. The rule
    /// depends only on pull requests, not on how a commit was found, so
    /// fetching the same data again never changes it.
    #[must_use]
    pub fn with_line_count_rule(mut self, merged_by_merge_commit: &HashSet<String>) -> Self {
        // Commit event id -> whether any appearance keeps it counted.
        let mut appearances: HashMap<String, bool> = HashMap::new();
        for e in &self.events {
            if e.event_type != EventType::PrCommit {
                continue;
            }
            let (Some(pr_id), Some(sha)) = (e.parent_pr_id.as_deref(), e.meta.sha.as_deref())
            else {
                continue;
            };
            let merged = self
                .pull_requests
                .get(pr_id)
                .is_some_and(|pr| pr.state == PrState::Merged);
            let keeps = !e.is_removed()
                && (!merged
                    || merged_by_merge_commit.contains(pr_id)
                    || !self.pr_author_is_subject(Some(pr_id)));
            *appearances
                .entry(crate::model::EventId::commit(&e.repository_id, sha).0)
                .or_insert(false) |= keeps;
        }
        self.lines_excluded = appearances
            .into_iter()
            .filter(|(_, keeps)| !keeps)
            .map(|(id, _)| id)
            .collect();
        self
    }

    /// Whether a commit's line counts go into the totals.
    #[must_use]
    pub fn counts_lines(&self, commit_event_id: &str) -> bool {
        !self.lines_excluded.contains(commit_event_id)
    }

    /// Returns the repository's display name, using an ID placeholder if unknown.
    pub fn repo_name(&self, repository_id: &str) -> String {
        self.repositories.get(repository_id).map_or_else(
            || format!("unknown:{repository_id}"),
            Repository::full_name,
        )
    }

    fn pr_author_is_subject(&self, pr_id: Option<&str>) -> bool {
        pr_id
            .and_then(|id| self.pull_requests.get(id))
            .is_some_and(|pr| pr.author.is(&self.subject))
    }
}

fn review_key(e: &ActivityEvent) -> &str {
    e.meta.review_id.as_deref().unwrap_or(&e.entity_id)
}

/// Marks reviews that only contain replies to existing threads: commented,
/// without a summary, and every inline comment of the review is a reply.
fn mark_thread_reply_reviews(events: &mut [ActivityEvent]) {
    let mut with_summary: HashSet<String> = HashSet::new();
    // review id -> (has inline comments, all of them are replies)
    let mut inline: HashMap<String, (bool, bool)> = HashMap::new();
    for e in events.iter() {
        match e.event_type {
            EventType::ReviewSummary => {
                with_summary.insert(review_key(e).to_string());
            }
            EventType::InlineComment => {
                if let Some(rid) = e.meta.review_id.as_deref() {
                    let entry = inline.entry(rid.to_string()).or_insert((false, true));
                    entry.0 = true;
                    entry.1 &= e.meta.reply_to.is_some();
                }
            }
            _ => {}
        }
    }
    for e in events.iter_mut() {
        if e.event_type == EventType::ReviewSubmitted
            && e.meta.review_state == Some(ReviewState::Commented)
        {
            let key = review_key(e).to_string();
            let reply_only = !with_summary.contains(&key)
                && inline.get(&key).is_some_and(|(has, all)| *has && *all);
            e.meta.thread_reply_only = reply_only.then_some(true);
        }
    }
}

/// Deterministic event order: time, then lifecycle rank, then id.
pub fn sort_events(events: &mut [ActivityEvent]) {
    events.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
}
