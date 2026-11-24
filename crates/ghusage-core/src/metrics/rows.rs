/// One calendar bucket and its summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketSummary {
    /// Calendar interval represented by this row.
    pub bucket: Bucket,
    /// Activity totals for the interval.
    pub summary: Summary,
}

/// Per-day/week/month summaries. Buckets partition the period, so additive
/// counts sum to the whole-period summary.
#[must_use]
pub fn breakdown(ds: &Dataset, opts: &Options, gran: Granularity) -> Vec<BucketSummary> {
    let bs = buckets(opts.period, &opts.zone, gran);
    let mut per: BTreeMap<Date, SummaryAccumulator<'_>> = BTreeMap::new();
    for e in selected(ds, opts) {
        per.entry(bucket_key(
            event_time(e, opts.commit_basis),
            &opts.zone,
            gran,
        ))
        .or_default()
        .observe(ds, e, &opts.zone, opts.commit_basis);
    }
    bs.into_iter()
        .map(|b| {
            let summary = per
                .remove(&b.key)
                .map_or_else(Summary::default, SummaryAccumulator::finish);
            BucketSummary { bucket: b, summary }
        })
        .collect()
}

/// Per-repository activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
    /// GitHub node ID of the repository.
    pub repository_id: String,
    /// Repository owner and name.
    pub full_name: String,
    /// Current repository visibility.
    pub visibility: Visibility,
    /// Whether the subject owns or administers the repository.
    pub owned: bool,
    /// Whether the repository was accessible at the last sync.
    pub accessible: bool,
    /// Activity totals for the repository.
    pub summary: Summary,
}

/// Activity per repository, ordered by subject activity then name.
#[must_use]
pub fn by_repository(ds: &Dataset, opts: &Options) -> Vec<RepoSummary> {
    let mut per: BTreeMap<&str, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in selected(ds, opts) {
        per.entry(e.repository_id.as_str()).or_default().push(e);
    }
    let mut out: Vec<RepoSummary> = per
        .into_iter()
        .map(|(id, events)| {
            let summary =
                summarize_events(ds, events.into_iter(), opts.zone.clone(), opts.commit_basis);
            let repo = ds.repositories.get(id);
            RepoSummary {
                repository_id: id.to_string(),
                full_name: ds.repo_name(id),
                visibility: repo.map_or(Visibility::Unknown, |r| r.visibility),
                owned: repo.is_some_and(|r| r.is_owned_by(&ds.subject, ds.subject_is_viewer)),
                accessible: repo.is_none_or(|r| r.accessible),
                summary,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        activity_weight(&b.summary)
            .cmp(&activity_weight(&a.summary))
            .then_with(|| a.full_name.cmp(&b.full_name))
    });
    out
}

/// Activity measure used to rank repositories and draw activity charts:
/// commits, the subject's pull request events and issue events.
#[must_use]
pub const fn activity_weight(s: &Summary) -> u64 {
    s.commits + s.pr_internal_events + s.issues_opened + s.issues_closed + s.issues_reopened
}

/// Snapshot values derived from current state (not historical events).
/// Current-state counts for pull requests, issues, and review threads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The subject's pull requests currently open.
    pub open_prs: u64,
    /// Currently open issues authored by the subject.
    pub open_issues_authored: u64,
    /// Current thread totals on pull requests active in the period.
    pub threads: ThreadCounts,
}

/// Counts of current review threads by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCounts {
    /// Total number of current threads.
    pub total: u64,
    /// Threads not resolved.
    pub open: u64,
    /// Threads marked resolved.
    pub resolved: u64,
    /// Threads whose comments are outdated.
    pub outdated: u64,
}

impl ThreadCounts {
    const fn add(&mut self, t: &ReviewThread) {
        self.total += 1;
        if t.is_resolved {
            self.resolved += 1;
        } else {
            self.open += 1;
        }
        if t.is_outdated {
            self.outdated += 1;
        }
    }
}

/// Current-state values. Thread counts cover pull requests active in the
/// period (all participants).
#[must_use]
pub fn snapshot(ds: &Dataset, opts: &Options) -> Snapshot {
    let mut s = Snapshot::default();
    for pr in ds.pull_requests.values() {
        if pr.state == PrState::Open
            && pr.author.is(&ds.subject)
            && opts.filter.matches_repo(ds, &pr.repository_id)
        {
            s.open_prs += 1;
        }
    }
    for issue in ds.issues.values() {
        if issue.is_open
            && issue.author.is(&ds.subject)
            && opts.filter.matches_repo(ds, &issue.repository_id)
        {
            s.open_issues_authored += 1;
        }
    }
    let active: BTreeSet<&str> = selected(ds, opts)
        .filter(|e| !e.event_type.is_automation())
        .filter_map(|e| e.parent_pr_id.as_deref())
        .collect();
    for t in ds.threads.values() {
        if active.contains(t.pr_id.as_str()) {
            s.threads.add(t);
        }
    }
    s
}

/// Sort keys for the pull request list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrSort {
    /// Sort by number of activity events.
    #[default]
    Activity,
    /// Sort by comment count.
    Comments,
    /// Sort by submitted review count.
    Reviews,
    /// Sort by creation time.
    Created,
    /// Sort by most recent activity.
    Updated,
}

/// Which pull requests a list covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Participation {
    /// Pull requests where the subject had activity in the period.
    #[default]
    Subject,
    /// Pull requests with any participant's activity in the period.
    All,
}

/// Activity counts for one pull request within the period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRow {
    /// GitHub node ID of the pull request.
    pub pr_id: String,
    /// Repository display name.
    pub repository: String,
    /// Pull request number within the repository.
    pub number: u64,
    /// Current pull request title.
    pub title: String,
    /// GitHub pull request URL.
    pub url: String,
    /// Pull request author's login or display name.
    pub author: String,
    /// Current pull request state.
    pub state: PrState,
    /// Whether the pull request is currently a draft.
    pub is_draft: bool,
    /// Pull request creation time.
    pub created_at: Timestamp,
    /// Pull request merge time, when merged.
    pub merged_at: Option<Timestamp>,
    /// Human events by anyone (automation excluded).
    pub events: u64,
    /// Events performed by the subject.
    pub events_by_subject: u64,
    /// Distinct commits in the pull request during the period.
    pub commits: u64,
    /// Reviews submitted during the period.
    pub reviews: u64,
    /// Comments added during the period.
    pub comments: u64,
    /// Distinct human participants during the period.
    pub participants: u64,
    /// People who wrote a conversation, inline or review summary comment.
    pub discussion_participants: u64,
    /// Review threads with an inline comment in the period.
    pub threads: u64,
    /// Time of the latest event in the period, when available.
    pub last_activity: Option<Timestamp>,
    /// Latest combined CI result observed for the pull request.
    pub latest_ci: Option<CheckOutcome>,
    /// The pull request's base-to-head diff when last fetched: a snapshot of
    /// the pull request, not changes made in the period.
    pub diff: Option<PrDiff>,
}

/// Pull requests active in the period with per-PR counts.
#[must_use]
pub fn pr_rows(
    ds: &Dataset,
    opts: &Options,
    participation: Participation,
    state: Option<PrState>,
    sort: PrSort,
) -> Vec<PrRow> {
    let mut per: BTreeMap<&str, Vec<&ActivityEvent>> = BTreeMap::new();
    for e in selected(ds, opts) {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !e.event_type.is_automation()
        {
            per.entry(pr).or_default().push(e);
        }
    }
    let mut rows: Vec<PrRow> = per
        .into_iter()
        .filter_map(|(pr_id, events)| {
            let pr = ds.pull_requests.get(pr_id)?;
            if state.is_some_and(|s| s != pr.state) {
                return None;
            }
            let by_subject = events.iter().filter(|e| e.actor.is(&ds.subject)).count() as u64;
            if participation == Participation::Subject && by_subject == 0 {
                return None;
            }
            Some(pr_row(ds, pr, &events, by_subject, opts.commit_basis))
        })
        .collect();
    rows.sort_by(|a, b| {
        let key = |r: &PrRow| match sort {
            PrSort::Activity => i128::from(r.events),
            PrSort::Comments => i128::from(r.comments),
            PrSort::Reviews => i128::from(r.reviews),
            PrSort::Created => i128::from(r.created_at.as_second()),
            PrSort::Updated => i128::from(r.last_activity.map_or(i64::MIN, Timestamp::as_second)),
        };
        key(b)
            .cmp(&key(a))
            .then_with(|| a.repository.cmp(&b.repository))
            .then_with(|| a.number.cmp(&b.number))
    });
    rows
}

fn pr_row(
    ds: &Dataset,
    pr: &PullRequest,
    events: &[&ActivityEvent],
    by_subject: u64,
    basis: CommitDateBasis,
) -> PrRow {
    let mut commits: HashSet<&str> = HashSet::new();
    let mut reviews = 0;
    let mut comments = 0;
    let mut participants: HashSet<String> = HashSet::new();
    let mut discussion: HashSet<String> = HashSet::new();
    let mut threads: HashSet<&str> = HashSet::new();
    let mut last = None;
    for e in events {
        if e.event_type.is_discussion()
            && let Some(key) = participant_key(&e.actor)
        {
            discussion.insert(key);
        }
        if e.event_type == EventType::InlineComment
            && let Some(t) = e.meta.thread_id.as_deref()
        {
            threads.insert(t);
        }
        match e.event_type {
            EventType::PrCommit => {
                if let Some(sha) = e.meta.sha.as_deref() {
                    commits.insert(sha);
                }
            }
            EventType::ReviewSubmitted => reviews += 1,
            EventType::ConversationComment
            | EventType::InlineComment
            | EventType::ReviewSummary => comments += 1,
            _ => {}
        }
        if let Some(key) = participant_key(&e.actor) {
            participants.insert(key);
        }
        last = last.max(Some(event_time(e, basis)));
    }
    PrRow {
        pr_id: pr.id.clone(),
        repository: ds.repo_name(&pr.repository_id),
        number: pr.number,
        title: pr.title.clone(),
        url: pr.url.clone(),
        author: pr.author.display().to_string(),
        state: pr.state,
        is_draft: pr.is_draft,
        created_at: pr.created_at,
        merged_at: pr.merged_at,
        events: events.len() as u64,
        events_by_subject: by_subject,
        commits: commits.len() as u64,
        reviews,
        comments,
        participants: participants.len() as u64,
        discussion_participants: discussion.len() as u64,
        threads: threads.len() as u64,
        last_activity: last,
        latest_ci: pr.latest_ci,
        diff: ds.pr_diffs.get(&pr.id).copied(),
    }
}

fn participant_key(a: &Actor) -> Option<String> {
    if a.is_unknown() {
        return None;
    }
    a.id.clone()
        .or_else(|| a.login.as_ref().map(|l| l.to_ascii_lowercase()))
}

/// Descriptive statistics over per-PR values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Number of pull requests included in the statistic.
    pub count: u64,
    /// Smallest observed value.
    pub min: f64,
    /// Median observed value.
    pub median: f64,
    /// Arithmetic mean of observed values.
    pub mean: f64,
    /// Nearest-rank 90th percentile.
    pub p90: f64,
    /// Largest observed value.
    pub max: f64,
}

impl Stats {
    /// Computes descriptive statistics, or `None` for an empty input.
    pub fn from_values(mut v: Vec<f64>) -> Option<Self> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let n = v.len();
        // Nearest-rank 90th percentile: ceil(0.9 * n) == n - floor(n / 10).
        let p90 = v[n - n / 10 - 1];
        let median = if n % 2 == 1 {
            v[n / 2]
        } else {
            f64::midpoint(v[n / 2 - 1], v[n / 2])
        };
        Some(Self {
            count: n as u64,
            min: v[0],
            median,
            mean: v.iter().sum::<f64>() / n as f64,
            p90,
            max: v[n - 1],
        })
    }
}

/// Activity density across pull requests active in the period. These are
/// descriptive statistics, not quality or skill measures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Intensity {
    /// Number of pull requests included.
    pub prs: u64,
    /// Human activity events per pull request.
    pub events_per_pr: Option<Stats>,
    /// Submitted reviews per pull request.
    pub reviews_per_pr: Option<Stats>,
    /// Comments per pull request.
    pub comments_per_pr: Option<Stats>,
    /// Commits per pull request.
    pub commits_per_pr: Option<Stats>,
    /// Distinct participants per pull request.
    pub participants_per_pr: Option<Stats>,
    /// Distinct discussion authors per pull request.
    pub discussion_participants_per_pr: Option<Stats>,
    /// Threads with comments per pull request.
    pub threads_per_pr: Option<Stats>,
    /// Active local dates per pull request.
    pub active_days_per_pr: Option<Stats>,
}

/// Computes per-pull-request activity density for the selected participation set.
#[must_use]
pub fn intensity(ds: &Dataset, opts: &Options, participation: Participation) -> Intensity {
    let rows = pr_rows(ds, opts, participation, None, PrSort::Activity);
    let mut days: HashMap<&str, HashSet<Date>> = HashMap::new();
    for e in selected(ds, opts) {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && !e.event_type.is_automation()
        {
            days.entry(pr)
                .or_default()
                .insert(opts.zone.local_date(event_time(e, opts.commit_basis)));
        }
    }
    let col = |f: &dyn Fn(&PrRow) -> f64| Stats::from_values(rows.iter().map(f).collect());
    Intensity {
        prs: rows.len() as u64,
        events_per_pr: col(&|r| r.events as f64),
        reviews_per_pr: col(&|r| r.reviews as f64),
        comments_per_pr: col(&|r| r.comments as f64),
        commits_per_pr: col(&|r| r.commits as f64),
        participants_per_pr: col(&|r| r.participants as f64),
        discussion_participants_per_pr: col(&|r| r.discussion_participants as f64),
        threads_per_pr: col(&|r| r.threads as f64),
        active_days_per_pr: Stats::from_values(
            rows.iter()
                .map(|r| {
                    days.get(r.pr_id.as_str())
                        .map_or(0, HashSet::len) as f64
                })
                .collect(),
        ),
    }
}

