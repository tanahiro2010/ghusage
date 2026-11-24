/// Lifecycle timings of one pull request, from observed events only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrTiming {
    /// Creation to the first submitted review by someone other than the author.
    pub time_to_first_review_secs: Option<i64>,
    /// Creation to merge.
    pub creation_to_merge_secs: Option<i64>,
    /// First review by another participant to the author's next observed
    /// event of any kind. This is not necessarily a response to the review.
    pub review_to_next_author_activity_secs: Option<i64>,
    /// First observed human event to last observed human event.
    pub first_to_last_activity_secs: Option<i64>,
}

/// Computes lifecycle timings from a pull request's observed events.
#[must_use]
pub fn pr_timing(ds: &Dataset, pr: &PullRequest) -> PrTiming {
    let events: Vec<&ActivityEvent> = ds
        .events
        .iter()
        .filter(|e| e.parent_pr_id.as_deref() == Some(pr.id.as_str()))
        .collect();
    timing_of(pr, &events)
}

/// Timings from one pull request's events (in dataset order).
fn timing_of(pr: &PullRequest, all: &[&ActivityEvent]) -> PrTiming {
    let events: Vec<&&ActivityEvent> = all
        .iter()
        .filter(|e| is_countable(e) && !e.event_type.is_automation())
        .collect();
    let first_review = events.iter().find(|e| {
        e.event_type == EventType::ReviewSubmitted
            && !e.actor.is_unknown()
            && !same_actor(&e.actor, &pr.author)
    });
    let after_review = first_review.and_then(|r| {
        events
            .iter()
            .find(|e| e.occurred_at > r.occurred_at && same_actor(&e.actor, &pr.author))
            .map(|e| e.occurred_at.duration_since(r.occurred_at).as_secs())
    });
    let secs = |d: SignedDuration| d.as_secs();
    PrTiming {
        time_to_first_review_secs: first_review.map(|r| secs(r.occurred_at - pr.created_at)),
        creation_to_merge_secs: pr.merged_at.map(|m| secs(m - pr.created_at)),
        review_to_next_author_activity_secs: after_review,
        first_to_last_activity_secs: match (events.first(), events.last()) {
            (Some(a), Some(b)) => Some(secs(b.occurred_at - a.occurred_at)),
            _ => None,
        },
    }
}

fn same_actor(a: &Actor, b: &Actor) -> bool {
    if a.is_unknown() || b.is_unknown() {
        return false;
    }
    match (&a.id, &b.id) {
        (Some(x), Some(y)) => x == y,
        _ => match (&a.login, &b.login) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
            _ => false,
        },
    }
}

/// Timing statistics (seconds) over the subject's pull requests active in
/// the period.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimingStats {
    /// Distribution of creation-to-first-review times.
    pub time_to_first_review: Option<Stats>,
    /// Distribution of creation-to-merge times.
    pub creation_to_merge: Option<Stats>,
    /// Distribution of first-review-to-next-author-event times.
    pub review_to_next_author_activity: Option<Stats>,
    /// Distribution of first-to-last-activity times.
    pub first_to_last_activity: Option<Stats>,
}

/// Computes timing distributions for subject-authored pull requests in the period.
#[must_use]
pub fn timing_stats(ds: &Dataset, opts: &Options) -> TimingStats {
    let rows = pr_rows(ds, opts, Participation::All, None, PrSort::Activity);
    let wanted: HashSet<&str> = rows
        .iter()
        .filter_map(|r| ds.pull_requests.get(&r.pr_id))
        .filter(|pr| pr.author.is(&ds.subject))
        .map(|pr| pr.id.as_str())
        .collect();
    // Group each wanted pull request's events in one pass.
    let mut per: HashMap<&str, Vec<&ActivityEvent>> = HashMap::new();
    for e in &ds.events {
        if let Some(pr) = e.parent_pr_id.as_deref()
            && wanted.contains(pr)
        {
            per.entry(pr).or_default().push(e);
        }
    }
    let mut ids: Vec<&str> = wanted.into_iter().collect();
    ids.sort_unstable();
    let timings: Vec<PrTiming> = ids
        .into_iter()
        .filter_map(|id| {
            let pr = ds.pull_requests.get(id)?;
            Some(timing_of(pr, per.get(id).map_or(&[][..], |v| v.as_slice())))
        })
        .collect();
    let col = |f: &dyn Fn(&PrTiming) -> Option<i64>| {
        Stats::from_values(timings.iter().filter_map(f).map(|v| v as f64).collect())
    };
    TimingStats {
        time_to_first_review: col(&|t| t.time_to_first_review_secs),
        creation_to_merge: col(&|t| t.creation_to_merge_secs),
        review_to_next_author_activity: col(&|t| t.review_to_next_author_activity_secs),
        first_to_last_activity: col(&|t| t.first_to_last_activity_secs),
    }
}

