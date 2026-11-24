//! Shared helpers for HTTP-level tests. Response bodies mirror the shapes
//! returned by the GitHub REST and GraphQL APIs (including fields the
//! client ignores).

#![expect(
    dead_code,
    reason = "shared fixtures are only used by some integration test targets"
)]

use std::time::Duration;

use ghusage_core::time::Period;
use ghusage_github::{ClientConfig, Credentials, GitHubClient, Secret, TokenSource};
use jiff::{Timestamp, civil::Date};
use serde_json::{Value, json};
use wiremock::MockServer;

/// A recognizable fake token; it must never show up in any error output.
pub(crate) const TOKEN: &str = "ghp_SENTINELtoken0123456789abcdefNEVERPRINT";

pub(crate) fn config(server: &MockServer) -> ClientConfig {
    ClientConfig {
        api_base: server.uri(),
        graphql_url: format!("{}/graphql", server.uri()),
        concurrency: 4,
        max_attempts: 3,
        request_timeout: Duration::from_secs(5),
        max_rate_limit_wait: Duration::from_secs(5),
        backoff_base: Duration::from_millis(1),
        max_response_bytes: 4 * 1024 * 1024,
        user_agent: "ghusage-test".into(),
    }
}

pub(crate) fn credentials() -> Credentials {
    Credentials {
        token: Secret::new(TOKEN),
        source: TokenSource::GhTokenEnv,
    }
}

pub(crate) fn client(server: &MockServer) -> GitHubClient {
    GitHubClient::new(Some(credentials()), config(server)).unwrap()
}

pub(crate) fn client_with(server: &MockServer, f: impl FnOnce(&mut ClientConfig)) -> GitHubClient {
    let mut c = config(server);
    f(&mut c);
    GitHubClient::new(Some(credentials()), c).unwrap()
}

pub(crate) fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

pub(crate) fn period(start: &str, end: &str) -> Period {
    Period::new(ts(start), ts(end)).unwrap()
}

pub(crate) fn day(y: i32, m: u8, d: u8) -> Timestamp {
    Date::new(
        i16::try_from(y).expect("test year fits i16"),
        i8::try_from(m).expect("test month fits i8"),
        i8::try_from(d).expect("test day fits i8"),
    )
    .unwrap()
    .at(0, 0, 0, 0)
    .to_zoned_datetime(jiff::tz::TimeZone::UTC)
    .unwrap()
    .timestamp()
}

/// Asserts that a rendered error contains no trace of the token.
pub(crate) fn assert_no_token(e: &impl std::fmt::Debug, display: &str) {
    let debug = format!("{e:?}");
    for s in [&debug, display] {
        assert!(!s.contains(TOKEN), "token leaked: {s}");
        assert!(!s.contains("SENTINEL"), "token leaked: {s}");
        assert!(
            !s.to_ascii_lowercase().contains("bearer"),
            "header leaked: {s}"
        );
    }
}

// ---------------------------------------------------------------------------
// REST fixtures
// ---------------------------------------------------------------------------

pub(crate) fn rest_user(login: &str, node_id: &str, kind: &str) -> Value {
    json!({
        "login": login,
        "id": 1000,
        "node_id": node_id,
        "avatar_url": "https://avatars.githubusercontent.com/u/1000?v=4",
        "gravatar_id": "",
        "url": format!("https://api.github.com/users/{login}"),
        "html_url": format!("https://github.com/{login}"),
        "type": kind,
        "user_view_type": "public",
        "site_admin": false
    })
}

pub(crate) fn rest_repo(
    node_id: &str,
    owner: &str,
    owner_type: &str,
    name: &str,
    private: bool,
    fork: bool,
) -> Value {
    json!({
        "id": 4242,
        "node_id": node_id,
        "name": name,
        "full_name": format!("{owner}/{name}"),
        "private": private,
        "owner": rest_user(owner, &format!("{owner}_node"), owner_type),
        "html_url": format!("https://github.com/{owner}/{name}"),
        "description": null,
        "fork": fork,
        "url": format!("https://api.github.com/repos/{owner}/{name}"),
        "commits_url": format!("https://api.github.com/repos/{owner}/{name}/commits{{/sha}}")
    })
}

/// A 40-character hex SHA derived from `n`.
pub(crate) fn sha(n: u64) -> String {
    format!("{n:040x}")
}

/// One item of `GET /search/commits`.
pub(crate) fn search_commit(
    repo: &Value,
    sha: &str,
    author: Option<&str>,
    author_date: &str,
    committer_date: &str,
    parents: usize,
) -> Value {
    let full = repo["full_name"].as_str().unwrap();
    let parents: Vec<Value> = (0..parents)
        .map(|i| {
            json!({
                "url": format!("https://api.github.com/repos/{full}/commits/{}", self::sha(9000 + i as u64)),
                "html_url": format!("https://github.com/{full}/commit/{}", self::sha(9000 + i as u64)),
                "sha": self::sha(9000 + i as u64)
            })
        })
        .collect();
    json!({
        "url": format!("https://api.github.com/repos/{full}/commits/{sha}"),
        "sha": sha,
        "node_id": format!("C_{sha}"),
        "html_url": format!("https://github.com/{full}/commit/{sha}"),
        "comments_url": format!("https://api.github.com/repos/{full}/commits/{sha}/comments"),
        "commit": {
            "url": format!("https://api.github.com/repos/{full}/git/commits/{sha}"),
            "author": { "date": author_date, "name": "Author", "email": "author@example.com" },
            "committer": { "date": committer_date, "name": "GitHub", "email": "noreply@github.com" },
            "message": "Change things",
            "tree": { "url": "https://api.github.com/repos/x/y/git/trees/abc", "sha": "abc" },
            "comment_count": 0
        },
        "author": author.map(|a| rest_user(a, &format!("U_{a}"), "User")),
        "committer": rest_user("web-flow", "MDQ6VXNlcjE5ODY0NDQ3", "User"),
        "parents": parents,
        "repository": repo,
        "score": 1.0
    })
}

pub(crate) fn search_page(total: u64, incomplete: bool, items: Vec<Value>) -> Value {
    json!({ "total_count": total, "incomplete_results": incomplete, "items": items })
}

/// One item of `GET /search/issues`.
pub(crate) fn search_issue(
    owner: &str,
    name: &str,
    number: u64,
    node_id: &str,
    is_pr: bool,
    updated_at: &str,
) -> Value {
    let mut v = json!({
        "url": format!("https://api.github.com/repos/{owner}/{name}/issues/{number}"),
        "repository_url": format!("https://api.github.com/repos/{owner}/{name}"),
        "html_url": format!("https://github.com/{owner}/{name}/issues/{number}"),
        "id": number * 10,
        "node_id": node_id,
        "number": number,
        "title": "Something",
        "user": rest_user("alice", "U_alice", "User"),
        "labels": [],
        "state": "open",
        "locked": false,
        "comments": 0,
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": updated_at,
        "closed_at": null,
        "author_association": "OWNER",
        "draft": false,
        "body": null,
        "score": 1.0
    });
    if is_pr {
        v["pull_request"] = json!({
            "url": format!("https://api.github.com/repos/{owner}/{name}/pulls/{number}"),
            "html_url": format!("https://github.com/{owner}/{name}/pull/{number}"),
            "diff_url": format!("https://github.com/{owner}/{name}/pull/{number}.diff"),
            "patch_url": format!("https://github.com/{owner}/{name}/pull/{number}.patch"),
            "merged_at": null
        });
        v["html_url"] = json!(format!("https://github.com/{owner}/{name}/pull/{number}"));
    }
    v
}

/// A `Link` header with only a `next` relation.
pub(crate) fn next_link(server: &MockServer, path_and_query: &str) -> String {
    format!("<{}{}>; rel=\"next\"", server.uri(), path_and_query)
}

// ---------------------------------------------------------------------------
// GraphQL fixtures
// ---------------------------------------------------------------------------

pub(crate) fn gql_actor(typename: &str, login: &str, id: &str) -> Value {
    json!({ "__typename": typename, "login": login, "id": id })
}

pub(crate) fn user(login: &str) -> Value {
    gql_actor("User", login, &format!("U_{login}"))
}

pub(crate) fn page_info(has_next: bool, cursor: Option<&str>) -> Value {
    json!({ "hasNextPage": has_next, "endCursor": cursor })
}

pub(crate) fn connection(nodes: Vec<Value>, has_next: bool, cursor: Option<&str>) -> Value {
    json!({ "pageInfo": page_info(has_next, cursor), "nodes": nodes })
}

pub(crate) fn gql_repo(
    id: &str,
    owner: Value,
    name: &str,
    visibility: &str,
    is_fork: bool,
) -> Value {
    let login = owner["login"].as_str().unwrap().to_string();
    json!({
        "id": id,
        "name": name,
        "url": format!("https://github.com/{login}/{name}"),
        "visibility": visibility,
        "isFork": is_fork,
        "viewerPermission": "WRITE",
        "owner": owner
    })
}

pub(crate) fn pr_commit(
    owner: &str,
    name: &str,
    number: u64,
    oid: &str,
    author: Option<&str>,
    authored: &str,
    committed: &str,
    parents: u64,
) -> Value {
    let user = |l: Option<&str>| {
        l.map_or_else(
            || json!({ "user": null }),
            |l| json!({ "user": { "id": format!("U_{l}"), "login": l } }),
        )
    };
    json!({
        "url": format!("https://github.com/{owner}/{name}/pull/{number}/commits/{oid}"),
        "commit": {
            "oid": oid,
            "committedDate": committed,
            "authoredDate": authored,
            "url": format!("https://github.com/{owner}/{name}/commit/{oid}"),
            "parents": { "totalCount": parents },
            "author": user(author),
            "committer": user(author)
        }
    })
}

pub(crate) fn review(
    id: &str,
    state: &str,
    body: &str,
    submitted_at: Option<&str>,
    author: Value,
    comments: Vec<Value>,
) -> Value {
    json!({
        "id": id,
        "state": state,
        "body": body,
        "submittedAt": submitted_at,
        "url": format!("https://github.com/acme/widget/pull/42#pullrequestreview-{id}"),
        "author": author,
        "comments": { "nodes": comments }
    })
}

pub(crate) fn review_comment(
    id: &str,
    author: Value,
    created_at: &str,
    review_id: &str,
    reply_to: Option<&str>,
    edited: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "state": "SUBMITTED",
        "createdAt": created_at,
        "lastEditedAt": edited,
        "url": format!("https://github.com/acme/widget/pull/42#discussion_{id}"),
        "author": author,
        "replyTo": reply_to.map(|r| json!({ "id": r })),
        "pullRequestReview": { "id": review_id }
    })
}

pub(crate) fn thread(
    id: &str,
    resolved: bool,
    outdated: bool,
    resolved_by: Option<Value>,
    path: &str,
    comments: Vec<Value>,
) -> Value {
    let n = comments.len();
    json!({
        "id": id,
        "isResolved": resolved,
        "isOutdated": outdated,
        "path": path,
        "resolvedBy": resolved_by,
        "comments": {
            "totalCount": n,
            "pageInfo": page_info(false, Some("end")),
            "nodes": comments
        }
    })
}

pub(crate) fn issue_comment(
    id: &str,
    author: Value,
    created_at: &str,
    edited: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "createdAt": created_at,
        "lastEditedAt": edited,
        "url": format!("https://github.com/acme/widget/pull/42#issuecomment-{id}"),
        "author": author
    })
}

pub(crate) fn tl(typename: &str, id: &str, created_at: &str, actor: Value, extra: Value) -> Value {
    let mut v =
        json!({ "__typename": typename, "id": id, "createdAt": created_at, "actor": actor });
    if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in extra {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

pub(crate) fn gql_data(data: Value) -> Value {
    json!({ "data": data })
}
