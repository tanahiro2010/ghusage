/// Suffix of coverage scopes recorded by `--public-only` syncs.
pub const PUBLIC_ONLY_SCOPE: &str = " (public repositories only)";

/// Marker in the scope of commit coverage recorded by syncs that list
/// repository commits in addition to commit search, and leave out copies of
/// commits found in related repositories.
pub const COMMIT_LISTING_SCOPE: &str = "(repository listing + commit search; other repositories by commit search alone, copies excluded)";

/// Coverage records that count for the current mode. Records from a
/// public-only sync do not cover private data, so they are ignored unless
/// the current mode is public-only too. Commit records without
/// [`COMMIT_LISTING_SCOPE`] are ignored and the range is synced again:
/// records written before repository listing was added relied on commit
/// search alone, which misses commits, and records written before copies
/// were told apart may count copies of the subject's commits in other
/// repositories.
pub fn applicable_coverage(records: Vec<CoverageRecord>, public_only: bool) -> Vec<CoverageRecord> {
    records
        .into_iter()
        .filter(|r| public_only || !r.scope.ends_with(PUBLIC_ONLY_SCOPE))
        .filter(|r| r.kind != DataKind::Commits || r.scope.contains(COMMIT_LISTING_SCOPE))
        .collect()
}

/// Ranges of `kind` that need fetching for `period`.
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

fn scope(kind: DataKind, login: &str, opts: &SyncOptions) -> String {
    let vis = if opts.public_only {
        PUBLIC_ONLY_SCOPE
    } else {
        ""
    };
    match kind {
        DataKind::Commits => {
            let repos = if opts.subject_is_viewer {
                "pushed to or contributed to"
            } else {
                "owns or contributed to"
            };
            format!(
                "commits authored by {login} on the default branches of repositories {login} {repos} {COMMIT_LISTING_SCOPE}, plus commits of synced pull requests{vis}"
            )
        }
        DataKind::PullRequests => format!(
            "pull requests involving or reviewed by {login} updated in range, with commits, reviews, comments, review threads, timeline events and head-commit checks{vis}"
        ),
        DataKind::Issues => format!(
            "issues involving {login} updated in range, with open/close/reopen history{vis}"
        ),
    }
}

