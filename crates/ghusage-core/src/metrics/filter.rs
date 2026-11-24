
/// Which date counts as a commit's date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitDateBasis {
    /// Use the commit time reported by the committer.
    #[default]
    Committer,
    /// Use the original author time when available.
    Author,
}

/// Whether a repository is owned by the subject or another account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// Repositories owned or administered by the subject.
    Owned,
    /// Repositories not owned or administered by the subject.
    External,
}

/// Repository filters. Empty lists mean "no restriction".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// `owner/name` (case-insensitive) or repository id.
    pub repositories: Vec<String>,
    /// Owner or organization login.
    pub owners: Vec<String>,
    /// Optional public or non-public repository selection.
    pub visibility: Option<VisibilityFilter>,
    /// Optional ownership selection.
    pub ownership: Option<Ownership>,
}

/// Repository visibility selection for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityFilter {
    /// Include public repositories only.
    Public,
    /// Include private and internal repositories.
    Private,
}

impl Filter {
    /// Whether no repository, owner, visibility, or ownership restriction is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.repositories.is_empty()
            && self.owners.is_empty()
            && self.visibility.is_none()
            && self.ownership.is_none()
    }

    fn matches_repo(&self, ds: &Dataset, repository_id: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        let Some(repo) = ds.repositories.get(repository_id) else {
            return false;
        };
        if !self.repositories.is_empty() {
            let full = repo.full_name();
            if !self
                .repositories
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&full) || r == &repo.id)
            {
                return false;
            }
        }
        if !self.owners.is_empty()
            && !self
                .owners
                .iter()
                .any(|o| o.eq_ignore_ascii_case(&repo.owner))
        {
            return false;
        }
        match self.visibility {
            Some(VisibilityFilter::Public) if repo.visibility != Visibility::Public => {
                return false;
            }
            Some(VisibilityFilter::Private) if repo.visibility == Visibility::Public => {
                return false;
            }
            _ => {}
        }
        match self.ownership {
            Some(Ownership::Owned) if !repo.is_owned_by(&ds.subject, ds.subject_is_viewer) => {
                return false;
            }
            Some(Ownership::External) if repo.is_owned_by(&ds.subject, ds.subject_is_viewer) => {
                return false;
            }
            _ => {}
        }
        true
    }
}

/// Aggregation options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Half-open date and time range being summarized.
    pub period: Period,
    /// Time zone used for dates and calendar buckets.
    pub zone: Zone,
    /// Repository and owner selection criteria.
    pub filter: Filter,
    /// Date source used for commit activity.
    pub commit_basis: CommitDateBasis,
}

/// The instant an event is attributed to.
#[must_use]
pub fn event_time(e: &ActivityEvent, basis: CommitDateBasis) -> Timestamp {
    match (e.event_type, basis) {
        (EventType::CommitAuthored | EventType::PrCommit, CommitDateBasis::Author) => {
            e.meta.author_date.unwrap_or(e.occurred_at)
        }
        _ => e.occurred_at,
    }
}

/// Events counted by default: not removed, not pending reviews.
fn is_countable(e: &ActivityEvent) -> bool {
    let not_a_submission = e.event_type == EventType::ReviewSubmitted
        && (e.meta.review_state == Some(ReviewState::Pending)
            || e.meta.thread_reply_only == Some(true));
    !e.is_removed() && !not_a_submission
}

fn selected<'a>(ds: &'a Dataset, opts: &'a Options) -> impl Iterator<Item = &'a ActivityEvent> {
    ds.events.iter().filter(move |e| {
        is_countable(e)
            && opts.period.contains(event_time(e, opts.commit_basis))
            && opts.filter.matches_repo(ds, &e.repository_id)
    })
}
