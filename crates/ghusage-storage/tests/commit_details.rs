//! Commits fetched on demand, kept in the cache for offline use.

use ghusage_core::commit::{CommitDetail, CommitFile, CommitPerson, DiffStats, FileStatus};
use ghusage_core::model::{ActorKind, Repository, Visibility};
use ghusage_storage::{Store, WriteBatch};
use jiff::Timestamp;

fn t(day: u32, hour: u32) -> Timestamp {
    format!("2026-03-{day:02}T{hour:02}:00:00Z")
        .parse()
        .unwrap()
}

fn detail(repository: &str, sha: &str, message: &str) -> CommitDetail {
    CommitDetail {
        repository: repository.into(),
        sha: sha.into(),
        url: Some(format!("https://github.com/{repository}/commit/{sha}")),
        message: message.into(),
        author: CommitPerson {
            name: Some("Alice".into()),
            login: Some("alice".into()),
            date: Some(t(2, 3)),
        },
        committer: CommitPerson::default(),
        parents: vec!["p1".into()],
        stats: DiffStats {
            additions: 1,
            deletions: 1,
            total: 2,
        },
        files: vec![CommitFile {
            filename: "src/lib.rs".into(),
            previous_filename: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 1,
            changes: 2,
            patch: Some("@@ -1 +1 @@\n-a\n+b".into()),
        }],
        files_truncated: false,
        visibility: Visibility::Public,
        fetched_at: t(3, 4),
    }
}

#[test]
fn commit_details_round_trip_by_full_or_abbreviated_sha() {
    let mut s = Store::open_in_memory().unwrap();
    let full = "abcdef0123456789abcdef0123456789abcdef01";
    let d = detail("acme/widget", full, "fix: one\n\nbody");
    s.put_commit_detail(&d).unwrap();
    assert_eq!(
        s.commit_detail("acme/widget", full).unwrap(),
        Some(d.clone())
    );
    assert_eq!(
        s.commit_detail("ACME/Widget", "ABCDEF0").unwrap(),
        Some(d.clone())
    );
    assert_eq!(s.commit_detail("acme/other", full).unwrap(), None);
    assert_eq!(s.commit_detail("acme/widget", "abcdef1").unwrap(), None);
    // Storing the same commit again replaces it: there is one copy.
    let newer = detail("acme/widget", full, "fix: one (edited)");
    s.put_commit_detail(&newer).unwrap();
    s.put_commit_detail(&newer).unwrap();
    assert_eq!(
        s.commit_detail("acme/widget", "abcdef0").unwrap(),
        Some(newer)
    );
}

#[test]
fn ambiguous_abbreviated_sha_finds_nothing() {
    let mut s = Store::open_in_memory().unwrap();
    s.put_commit_detail(&detail("acme/widget", "abcdef0111", "a"))
        .unwrap();
    s.put_commit_detail(&detail("acme/widget", "abcdef0222", "b"))
        .unwrap();
    assert_eq!(s.commit_detail("acme/widget", "abcdef0").unwrap(), None);
    assert_eq!(
        s.commit_detail("acme/widget", "abcdef01")
            .unwrap()
            .unwrap()
            .message,
        "a"
    );
}

#[test]
fn commit_details_survive_reopening_and_are_removed_by_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ghusage.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.put_commit_detail(&detail("acme/widget", "abcdef0111", "a"))
            .unwrap();
    }
    let mut s = Store::open(&path).unwrap();
    assert!(
        s.commit_detail("acme/widget", "abcdef0111")
            .unwrap()
            .is_some()
    );
    s.clear().unwrap();
    assert_eq!(s.commit_detail("acme/widget", "abcdef0111").unwrap(), None);
    assert!(s.integrity_check().unwrap().is_empty());
}

#[test]
fn repositories_are_found_by_name_in_any_case() {
    let mut s = Store::open_in_memory().unwrap();
    let r = Repository {
        id: "R1".into(),
        owner: "Acme".into(),
        name: "Widget".into(),
        owner_kind: ActorKind::Organization,
        visibility: Visibility::Private,
        is_fork: false,
        viewer_permission: None,
        accessible: true,
        url: None,
    };
    s.apply(&WriteBatch {
        account: "alice".into(),
        repositories: vec![r.clone()],
        ..WriteBatch::default()
    })
    .unwrap();
    assert_eq!(s.repository_by_name("acme", "WIDGET").unwrap(), Some(r));
    assert_eq!(s.repository_by_name("nobody", "nothing").unwrap(), None);
}
