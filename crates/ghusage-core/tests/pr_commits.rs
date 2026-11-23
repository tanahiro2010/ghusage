//! The commit list of a pull request.

mod common;

use common::*;
use ghusage_core::metrics::{pr_commit_rows, pr_detail};
use ghusage_core::model::Visibility;

#[test]
fn every_commit_once_oldest_first_with_removed_ones_marked() {
    let ds = dataset(
        vec![repo("R1", "me", "app", Visibility::Public)],
        vec![pull("P1", "R1", 1, me()), pull("P2", "R1", 2, me())],
        vec![],
        vec![
            with_meta(
                pr_commit("R1", "P1", "bbb", me(), "2026-03-01T11:00:00Z"),
                |m| m.message_headline = Some("second".into()),
            ),
            with_meta(
                pr_commit("R1", "P1", "aaa", user("bob"), "2026-03-01T10:00:00Z"),
                |m| {
                    m.message_headline = Some("first".into());
                    m.is_merge_commit = Some(true);
                },
            ),
            // Force-pushed away.
            with_meta(
                pr_commit("R1", "P1", "ccc", me(), "2026-03-01T09:00:00Z"),
                |m| m.removed_observed_at = Some(utc("2026-03-02T00:00:00Z")),
            ),
            // Another pull request's commit is not listed.
            pr_commit("R1", "P2", "ddd", me(), "2026-03-01T08:00:00Z"),
            // Neither is a plain commit of the repository.
            commit("R1", "eee", me(), "2026-03-01T08:00:00Z"),
        ],
    );
    let rows = pr_commit_rows(&ds, "P1");
    let got: Vec<(&str, Option<&str>, &str, bool, bool)> = rows
        .iter()
        .map(|r| {
            (
                r.sha.as_str(),
                r.headline.as_deref(),
                r.author.as_str(),
                r.is_merge_commit,
                r.removed,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("ccc", None, "me", false, true),
            ("aaa", Some("first"), "bob", true, false),
            ("bbb", Some("second"), "me", false, false),
        ]
    );
    // The list agrees with the counts of the pull request.
    let d = pr_detail(&ds, "P1", &[]).unwrap();
    assert_eq!(rows.iter().filter(|r| !r.removed).count() as u64, d.commits);
    assert_eq!(
        rows.iter().filter(|r| r.removed).count() as u64,
        d.removed_commits
    );
    assert_eq!(pr_commit_rows(&ds, "P9").len(), 0);
}
