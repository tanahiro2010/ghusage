//! Lines added and deleted by commits: sums over the counted commits only,
//! with unknown statistics never taken as zero.

use std::collections::{BTreeMap, HashMap};

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{
    self, CommitDateBasis, Dataset, DiffCoverage, Filter, LineCounts, Options,
};
use ghusage_core::model::*;
use ghusage_core::time::{Granularity, Period, Zone};
use jiff::Timestamp;

fn at(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn me() -> Actor {
    Actor::new(Some("U_me".into()), Some("me".into()), Some("User"))
}

fn other() -> Actor {
    Actor::new(Some("U_x".into()), Some("someone".into()), Some("User"))
}

fn commit(repo: &str, sha: &str, t: &str, actor: Actor, merge: bool) -> ActivityEvent {
    ActivityEvent {
        event_id: EventId::commit(repo, sha),
        event_type: EventType::CommitAuthored,
        actor,
        repository_id: repo.into(),
        entity_id: sha.into(),
        parent_pr_id: None,
        occurred_at: at(t),
        observed_at: at(t),
        source: Source::Search,
        url: None,
        meta: EventMeta {
            sha: Some(sha.into()),
            is_merge_commit: Some(merge),
            ..Default::default()
        },
        coverage: CoverageStatus::Complete,
    }
}

const fn lines(additions: u64, deletions: u64) -> LineCounts {
    LineCounts {
        additions,
        deletions,
    }
}

fn repo(id: &str, name: &str) -> Repository {
    Repository {
        id: id.into(),
        owner: "me".into(),
        name: name.into(),
        owner_kind: ActorKind::User,
        visibility: Visibility::Public,
        is_fork: false,
        viewer_permission: None,
        accessible: true,
        url: None,
    }
}

fn dataset(events: Vec<ActivityEvent>, stats: &[(&str, &str, LineCounts)]) -> Dataset {
    let stats: HashMap<String, LineCounts> = stats
        .iter()
        .map(|(r, sha, l)| (EventId::commit(r, sha).0, *l))
        .collect();
    Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![repo("R1", "one"), repo("R2", "two")],
        vec![],
        vec![],
        vec![],
        events,
        vec![],
    )
    .with_diff_stats(stats, BTreeMap::new())
}

fn opts() -> Options {
    Options {
        period: Period::new(at("2026-11-01T00:00:00Z"), at("2026-11-04T00:00:00Z")).unwrap(),
        zone: Zone::utc(),
        filter: Filter::default(),
        commit_basis: CommitDateBasis::Committer,
    }
}

#[test]
fn totals_cover_known_commits_and_say_how_many() {
    let ds = dataset(
        vec![
            commit("R1", "a1", "2026-11-01T10:00:00Z", me(), false),
            commit("R1", "a2", "2026-11-02T10:00:00Z", me(), false),
            commit("R2", "b1", "2026-11-02T11:00:00Z", me(), false),
            // Not the subject's: never counted.
            commit("R1", "x1", "2026-11-02T12:00:00Z", other(), false),
        ],
        &[
            ("R1", "a1", lines(10, 2)),
            ("R2", "b1", lines(5, 5)),
            ("R1", "x1", lines(1000, 1000)),
        ],
    );
    let d = metrics::summary(&ds, &opts()).diff_stats;
    assert_eq!(d.commits, 3);
    assert_eq!(d.commits_with_stats, 2);
    assert_eq!((d.additions, d.deletions), (15, 7));
    // a2 is unknown: the totals are partial, not complete with a zero.
    assert_eq!(d.coverage, DiffCoverage::Partial);
}

#[test]
fn no_statistics_is_unavailable_and_all_is_complete() {
    let events = vec![commit("R1", "a1", "2026-11-01T10:00:00Z", me(), false)];
    let none = metrics::summary(&dataset(events.clone(), &[]), &opts()).diff_stats;
    assert_eq!(none.coverage, DiffCoverage::Unavailable);
    assert_eq!((none.commits, none.commits_with_stats), (1, 0));
    let all = metrics::summary(&dataset(events, &[("R1", "a1", lines(3, 1))]), &opts()).diff_stats;
    assert_eq!(all.coverage, DiffCoverage::Complete);
    let empty = metrics::summary(&dataset(vec![], &[]), &opts()).diff_stats;
    assert_eq!(empty.coverage, DiffCoverage::NoCommits);
}

#[test]
fn merge_commits_are_left_out() {
    let ds = dataset(
        vec![
            commit("R1", "a1", "2026-11-01T10:00:00Z", me(), false),
            commit("R1", "m1", "2026-11-01T11:00:00Z", me(), true),
        ],
        &[("R1", "a1", lines(4, 0)), ("R1", "m1", lines(400, 300))],
    );
    let s = metrics::summary(&ds, &opts());
    assert_eq!(s.commits, 2);
    assert_eq!(s.merge_commits, 1);
    assert_eq!(s.diff_stats.commits, 1);
    assert_eq!((s.diff_stats.additions, s.diff_stats.deletions), (4, 0));
    assert_eq!(s.diff_stats.coverage, DiffCoverage::Complete);
}

#[test]
fn a_commit_seen_twice_is_counted_once() {
    let first = commit("R1", "a1", "2026-11-01T10:00:00Z", me(), false);
    let mut again = first.clone();
    again.source = Source::Rest;
    let ds = dataset(vec![first, again], &[("R1", "a1", lines(7, 7))]);
    let d = metrics::summary(&ds, &opts()).diff_stats;
    assert_eq!((d.commits, d.additions, d.deletions), (1, 7, 7));
}

#[test]
fn days_and_repositories_add_up_to_the_period() {
    let ds = dataset(
        vec![
            commit("R1", "a1", "2026-11-01T10:00:00Z", me(), false),
            commit("R1", "a2", "2026-11-02T10:00:00Z", me(), false),
            commit("R2", "b1", "2026-11-03T11:00:00Z", me(), false),
        ],
        &[
            ("R1", "a1", lines(10, 1)),
            ("R1", "a2", lines(20, 2)),
            ("R2", "b1", lines(30, 3)),
        ],
    );
    let total = metrics::summary(&ds, &opts()).diff_stats;
    let days = metrics::breakdown(&ds, &opts(), Granularity::Day);
    let sum = |f: &dyn Fn(&metrics::DiffTotals) -> u64, v: Vec<metrics::DiffTotals>| {
        v.iter().map(f).sum::<u64>()
    };
    let per_day: Vec<_> = days.iter().map(|b| b.summary.diff_stats).collect();
    assert_eq!(sum(&|d| d.additions, per_day.clone()), total.additions);
    assert_eq!(sum(&|d| d.deletions, per_day), total.deletions);
    let per_repo: Vec<_> = metrics::by_repository(&ds, &opts())
        .iter()
        .map(|r| r.summary.diff_stats)
        .collect();
    assert_eq!(sum(&|d| d.additions, per_repo.clone()), total.additions);
    assert_eq!(sum(&|d| d.commits, per_repo), total.commits);
    // A repository filter keeps only that repository's commits.
    let mut only_r2 = opts();
    only_r2.filter.repositories = vec!["me/two".into()];
    let r2 = metrics::summary(&ds, &only_r2).diff_stats;
    assert_eq!((r2.commits, r2.additions), (1, 30));
}

fn pr(id: &str, state: PrState) -> PullRequest {
    PullRequest {
        id: id.into(),
        repository_id: "R1".into(),
        number: 1,
        title: "t".into(),
        url: "https://github.com/me/one/pull/1".into(),
        author: me(),
        state,
        is_draft: false,
        created_at: at("2026-11-01T09:00:00Z"),
        updated_at: None,
        closed_at: None,
        merged_at: None,
        merged_by: None,
        head_sha: None,
        latest_ci: None,
        coverage: CoverageStatus::Complete,
        synced_at: None,
    }
}

/// A commit of mine found inside pull request `pr` (both the pull request
/// commit and, as sync records it, the commit itself).
fn in_pr(pr: &str, sha: &str, removed: bool) -> [ActivityEvent; 2] {
    let mut c = commit("R1", sha, "2026-11-01T10:00:00Z", me(), false);
    // Sync records a commit found in a pull request from GraphQL.
    c.source = Source::Graphql;
    let mut p = c.clone();
    p.event_id = EventId::pr_commit(pr, sha);
    p.event_type = EventType::PrCommit;
    p.parent_pr_id = Some(pr.into());
    if removed {
        p.meta.removed_observed_at = Some(at("2026-11-02T00:00:00Z"));
    }
    [c, p]
}

#[test]
fn changes_reaching_the_default_branch_as_other_commits_are_not_counted_twice() {
    use std::collections::HashSet;
    let mut events = Vec::new();
    // Open pull request: counted.
    events.extend(in_pr("PR_open", "o1", false));
    // Merged with a merge commit: its commits are on the default branch.
    events.extend(in_pr("PR_merge", "m1", false));
    // Squash-merged: the squash commit sq1 on the default branch (in no
    // pull request) carries the change, the branch commit s1 does not count.
    events.extend(in_pr("PR_squash", "s1", false));
    events.push(commit("R1", "sq1", "2026-11-01T12:00:00Z", me(), false));
    // Merged by an unknown method: left out like squash.
    events.extend(in_pr("PR_unknown", "u1", false));
    // Removed from an open pull request by a force push.
    events.extend(in_pr("PR_open", "f1", true));
    let ds = Dataset::new(
        Subject {
            id: Some("U_me".into()),
            login: "me".into(),
        },
        true,
        vec![repo("R1", "one")],
        vec![
            pr("PR_open", PrState::Open),
            pr("PR_merge", PrState::Merged),
            pr("PR_squash", PrState::Merged),
            pr("PR_unknown", PrState::Merged),
        ],
        vec![],
        vec![],
        events,
        vec![],
    )
    .with_diff_stats(
        ["o1", "m1", "s1", "sq1", "u1", "f1"]
            .iter()
            .map(|s| (EventId::commit("R1", s).0, lines(10, 1)))
            .collect(),
        BTreeMap::new(),
    )
    .with_line_count_rule(&HashSet::from(["PR_merge".to_string()]));
    let s = metrics::summary(&ds, &opts());
    // Every commit still counts as a commit.
    assert_eq!(s.commits, 6);
    let d = s.diff_stats;
    assert_eq!(d.commits, 6);
    // s1, u1 and f1 are left out of the line totals.
    assert_eq!(d.excluded, 3);
    assert_eq!(d.commits_with_stats, 3);
    assert_eq!((d.additions, d.deletions), (30, 3));
    assert_eq!(d.coverage, DiffCoverage::Complete);
}
