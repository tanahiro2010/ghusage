//! Lines added and deleted by many commits at once, through one GraphQL
//! query per batch instead of one REST request per commit.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use ghusage_core::metrics::LineCounts;
use serde_json::Value;

use crate::ApiError;
use crate::http::Transport;

/// Commits per query. Each is one object lookup; GitHub's query limits
/// allow far more, but a smaller batch keeps one slow commit from failing
/// many others.
pub(crate) const BATCH: usize = 100;

/// A commit to look up.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitRef {
    /// Repository owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Full commit SHA.
    pub sha: String,
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The query for `commits` (all valid names and full SHAs), grouped by
/// repository: `r<i>` per repository, `c<j>` per commit.
fn query(groups: &[(&str, &str, Vec<&str>)]) -> String {
    let mut q = String::from("query CommitLineCounts {\n");
    for (i, (owner, name, shas)) in groups.iter().enumerate() {
        let _ = writeln!(
            q,
            "  r{i}: repository(owner: \"{owner}\", name: \"{name}\") {{"
        );
        for (j, sha) in shas.iter().enumerate() {
            let _ = writeln!(
                q,
                "    c{j}: object(oid: \"{sha}\") {{ ... on Commit {{ additions deletions }} }}"
            );
        }
        q.push_str("  }\n");
    }
    q.push('}');
    q
}

/// Line counts of `commits`, in one query per [`BATCH`] commits. A commit
/// GitHub does not return (missing, inaccessible repository, an error for
/// that commit) maps to `None`; so does an invalid name or SHA, which is
/// never sent. Transport failures fail the whole call.
pub(crate) async fn fetch(
    t: &Transport,
    commits: &[CommitRef],
) -> Result<Vec<(CommitRef, Option<LineCounts>)>, ApiError> {
    let mut out: Vec<(CommitRef, Option<LineCounts>)> = Vec::new();
    let valid: Vec<&CommitRef> = commits
        .iter()
        .filter(|c| {
            let ok = is_name(&c.owner) && is_name(&c.name) && is_sha(&c.sha);
            if !ok {
                out.push(((*c).clone(), None));
            }
            ok
        })
        .collect();
    // Batches run at once; the transport limits how many are in flight.
    let batches =
        futures::future::try_join_all(valid.chunks(BATCH).map(|b| fetch_batch(t, b))).await?;
    out.extend(batches.into_iter().flatten());
    Ok(out)
}

/// One query for up to [`BATCH`] commits.
async fn fetch_batch(
    t: &Transport,
    batch: &[&CommitRef],
) -> Result<Vec<(CommitRef, Option<LineCounts>)>, ApiError> {
    let mut groups: BTreeMap<(String, String), Vec<&CommitRef>> = BTreeMap::new();
    for c in batch {
        groups
            .entry((c.owner.to_ascii_lowercase(), c.name.to_ascii_lowercase()))
            .or_default()
            .push(c);
    }
    let groups: Vec<Vec<&CommitRef>> = groups.into_values().collect();
    let shaped: Vec<(&str, &str, Vec<&str>)> = groups
        .iter()
        .map(|cs| {
            (
                cs[0].owner.as_str(),
                cs[0].name.as_str(),
                cs.iter().map(|c| c.sha.as_str()).collect(),
            )
        })
        .collect();
    let resp = t.graphql(&query(&shaped), serde_json::json!({})).await?;
    let data = resp.data.unwrap_or(Value::Null);
    let mut out = Vec::new();
    for (i, cs) in groups.iter().enumerate() {
        for (j, c) in cs.iter().enumerate() {
            let node = &data[format!("r{i}")][format!("c{j}")];
            let counts = match (node["additions"].as_u64(), node["deletions"].as_u64()) {
                (Some(additions), Some(deletions)) => Some(LineCounts {
                    additions,
                    deletions,
                }),
                _ => None,
            };
            out.push(((*c).clone(), counts));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_groups_commits_by_repository() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let q = query(&[("octo", "app", vec![&a, &b])]);
        assert!(q.starts_with("query CommitLineCounts {"));
        assert!(q.contains("r0: repository(owner: \"octo\", name: \"app\")"));
        assert!(q.contains(&format!("c1: object(oid: \"{b}\")")));
    }

    #[test]
    fn only_plain_names_and_full_shas_are_sent() {
        assert!(is_name("my.repo-name_2"));
        assert!(!is_name("evil\") { x }"));
        assert!(!is_name(""));
        assert!(is_sha(&"0123456789abcdef".repeat(3)[..40]));
        assert!(!is_sha("abc"));
        assert!(!is_sha(&"g".repeat(40)));
    }
}
