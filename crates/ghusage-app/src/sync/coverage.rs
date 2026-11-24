/// Filters coverage records for the current sync mode.
///
/// Records from a
/// public-only sync do not cover private data, so they are ignored unless
/// the current mode is public-only too. Commit records without
/// older generations are ignored and fetched again.
#[must_use]
pub fn applicable_coverage(records: Vec<CoverageRecord>, public_only: bool) -> Vec<CoverageRecord> {
    records
        .into_iter()
        .filter(|r| public_only || r.scope.visibility == CoverageVisibility::All)
        .filter(|r| match r.kind {
            DataKind::Commits => r.scope.generation == CoverageGeneration::CommitsListingSearchV1,
            DataKind::PullRequests => r.scope.generation == CoverageGeneration::PullRequestsV1,
            DataKind::Issues => r.scope.generation == CoverageGeneration::IssuesV1,
        })
        .collect()
}

/// Ranges of `kind` that need fetching for `period`.
#[must_use]
pub fn ranges_to_fetch(
    records: &[CoverageRecord],
    kind: DataKind,
    opts: &SyncOptions,
) -> Vec<Period> {
    if opts.full {
        return vec![opts.period];
    }
    let summary = coverage::summarize(records, kind, opts.period.start, opts.period.end);
    let mut ranges: Vec<(Timestamp, Timestamp)> = summary
        .gaps()
        .map(|g| ((g.start - opts.overlap).max(opts.period.start), g.end))
        .collect();
    ranges.sort();
    let mut merged: Vec<(Timestamp, Timestamp)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
        .into_iter()
        .filter_map(|(s, e)| Period::new(s, e).ok())
        .collect()
}
