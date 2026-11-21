//! Shared helpers for end-to-end tests: an isolated `ghusage` process, a
//! mocked GitHub API (REST + GraphQL) and fixture builders whose JSON shapes
//! mirror what GitHub returns.
//!
//! The default fixture ("the world") is described in [`World::base`]; tests
//! derive expectations from that description, not from the implementation.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::{
    body_partial_json, body_string_contains, method, path, query_param, query_param_contains,
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A recognizable fake token; it must never appear in any output.
pub const SENTINEL_TOKEN: &str = "ghp_sentinelTOKENvalue1234567890abcdef";

/// The authenticated user whose activity is analyzed.
pub const ME: &str = "me";
pub const ME_ID: &str = "U_me";

/// The fixed analysis period used by most tests: Monday 2024-03-04 through
/// Sunday 2024-03-10 (both days included), in UTC.
pub const SINCE: &str = "2024-03-04";
pub const UNTIL: &str = "2024-03-10";

pub const WIDGET_ID: &str = "R_widget";
pub const DOTFILES_ID: &str = "R_dotfiles";
pub const SECRET_ID: &str = "R_secret";

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

/// Result of one `ghusage` invocation.
#[derive(Debug, Clone)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: Vec<u8>,
}

impl Run {
    fn from_output(o: Output) -> Self {
        Self {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            stdout_bytes: o.stdout,
        }
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON ({e})\nstdout: {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }

    /// Asserts the exit code and returns `self` for chaining.
    #[track_caller]
    pub fn expect_code(&self, code: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(code),
            "unexpected exit code\nstdout: {}\nstderr: {}",
            self.stdout,
            self.stderr
        );
        self
    }

    pub fn all_output(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// Fails if a panic message or the token shows up anywhere.
    #[track_caller]
    pub fn assert_clean(&self) {
        let all = self.all_output();
        assert!(!all.contains("panicked"), "process panicked: {all}");
        assert!(!all.contains("RUST_BACKTRACE"), "process panicked: {all}");
        assert!(!all.contains(SENTINEL_TOKEN), "token leaked: {all}");
        assert!(!all.contains("sentinelTOKEN"), "token leaked: {all}");
    }
}

/// An isolated data directory plus the API endpoint the binary talks to.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub api_url: String,
}

impl Env {
    pub fn new(server: &MockServer) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            api_url: server.uri(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.path().join("ghusage.db")
    }

    /// A command isolated from the developer's environment: private data
    /// directory, only the sentinel token, no `gh` on PATH, UTC.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(bin());
        self.configure(&mut c);
        c.args(args);
        c
    }

    /// Applies the isolated environment to any command (e.g. a wrapper
    /// shell that then executes `ghusage`).
    pub fn configure(&self, c: &mut Command) {
        c.env_clear();
        keep_system_env(c);
        c.env("PATH", self.dir.path().join("no-bin"))
            .env("GHUSAGE_DATA_DIR", self.dir.path())
            .env("TZ", "UTC")
            .env("GHUSAGE_BACKOFF_MS", "1")
            .env("GH_TOKEN", SENTINEL_TOKEN)
            .env("GHUSAGE_API_URL", &self.api_url)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
    }

    /// Runs an arbitrary prepared command to completion.
    pub async fn run_command(&self, mut c: Command) -> Run {
        tokio::task::spawn_blocking(move || Run::from_output(c.output().unwrap()))
            .await
            .unwrap()
    }

    /// Runs `ghusage` to completion on a blocking thread.
    pub async fn run(&self, args: &[&str]) -> Run {
        let mut c = self.command(args);
        tokio::task::spawn_blocking(move || Run::from_output(c.output().unwrap()))
            .await
            .unwrap()
    }

    /// `sync` over the standard period.
    pub async fn sync(&self, extra: &[&str]) -> Run {
        let mut args = vec![
            "sync", "--since", SINCE, "--until", UNTIL, "--format", "json",
        ];
        args.extend_from_slice(extra);
        self.run(&args).await
    }

    /// An offline JSON report over the standard period.
    pub async fn offline_json(&self, cmd: &[&str]) -> Run {
        let mut args: Vec<&str> = cmd.to_vec();
        args.extend_from_slice(&[
            "--since",
            SINCE,
            "--until",
            UNTIL,
            "--offline",
            "--format",
            "json",
        ]);
        self.run(&args).await
    }
}

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ghusage"))
}

/// Polls `cond` until it is true or `timeout` elapses.
pub async fn wait_until<F, Fut>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if cond().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Request inspection
// ---------------------------------------------------------------------------

pub async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

pub fn body_str(r: &Request) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

/// Number of `PullRequestDetail` queries for pull request `number`.
pub fn pr_detail_requests(reqs: &[Request], number: u64) -> usize {
    reqs.iter()
        .filter(|r| r.url.path() == "/graphql")
        .filter(|r| {
            let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            v["query"]
                .as_str()
                .is_some_and(|q| q.contains("query PullRequestDetail"))
                && v["variables"]["number"].as_u64() == Some(number)
        })
        .count()
}

pub fn all_pr_detail_requests(reqs: &[Request]) -> usize {
    reqs.iter()
        .filter(|r| r.url.path() == "/graphql" && body_str(r).contains("query PullRequestDetail"))
        .count()
}

// ---------------------------------------------------------------------------
// REST fixtures
// ---------------------------------------------------------------------------

pub fn rest_user(login: &str, node_id: &str, kind: &str) -> Value {
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

pub fn rest_repo(node_id: &str, owner: &str, owner_type: &str, name: &str, private: bool) -> Value {
    json!({
        "id": 4242,
        "node_id": node_id,
        "name": name,
        "full_name": format!("{owner}/{name}"),
        "private": private,
        "owner": rest_user(owner, &format!("{owner}_node"), owner_type),
        "html_url": format!("https://github.com/{owner}/{name}"),
        "description": null,
        "fork": false,
        "url": format!("https://api.github.com/repos/{owner}/{name}"),
        "commits_url": format!("https://api.github.com/repos/{owner}/{name}/commits{{/sha}}")
    })
}

/// A 40-character hex SHA derived from `n`.
pub fn sha(n: u64) -> String {
    format!("{n:040x}")
}

/// Author of a commit search result: `(login, node_id, type)` or nobody.
pub type RestAuthor<'a> = Option<(&'a str, &'a str, &'a str)>;

/// One item of `GET /search/commits`.
pub fn search_commit(
    repo: &Value,
    sha: &str,
    author: RestAuthor<'_>,
    committer_date: &str,
    parents: usize,
) -> Value {
    let full = repo["full_name"].as_str().unwrap();
    let parents: Vec<Value> = (0..parents)
        .map(|i| {
            let p = self::sha(9000 + i as u64);
            json!({
                "url": format!("https://api.github.com/repos/{full}/commits/{p}"),
                "html_url": format!("https://github.com/{full}/commit/{p}"),
                "sha": p
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
            "author": { "date": committer_date, "name": "Author", "email": "author@example.com" },
            "committer": { "date": committer_date, "name": "GitHub", "email": "noreply@github.com" },
            "message": "Change things",
            "tree": { "url": "https://api.github.com/repos/x/y/git/trees/abc", "sha": "abc" },
            "comment_count": 0
        },
        "author": author.map(|(l, id, kind)| rest_user(l, id, kind)),
        "committer": rest_user("web-flow", "MDQ6VXNlcjE5ODY0NDQ3", "User"),
        "parents": parents,
        "repository": repo,
        "score": 1.0
    })
}

/// One item of `GET /repos/{owner}/{name}/commits`: the same shape as a
/// commit search result without `repository` and `score`.
pub fn list_commit(
    repo: &Value,
    sha: &str,
    author: RestAuthor<'_>,
    committer_date: &str,
    parents: usize,
) -> Value {
    let mut v = search_commit(repo, sha, author, committer_date, parents);
    let obj = v.as_object_mut().unwrap();
    obj.remove("repository");
    obj.remove("score");
    v
}

/// A repository of `GET /user/repos`: the REST repository object with the
/// viewer's permissions and the time of the last push.
pub fn listed_repo(rest: &Value, permission: &str, pushed_at: &str) -> Value {
    let level = |l: &str| {
        let order = ["pull", "triage", "push", "maintain", "admin"];
        let have = order.iter().position(|x| *x == permission).unwrap();
        order.iter().position(|x| *x == l).unwrap() <= have
    };
    let mut v = rest.clone();
    v["pushed_at"] = json!(pushed_at);
    v["visibility"] = json!(if v["private"] == true {
        "private"
    } else {
        "public"
    });
    v["permissions"] = json!({
        "admin": level("admin"), "maintain": level("maintain"), "push": level("push"),
        "triage": level("triage"), "pull": level("pull")
    });
    v
}

pub fn search_page(items: Vec<Value>) -> Value {
    json!({ "total_count": items.len(), "incomplete_results": false, "items": items })
}

/// One item of `GET /search/issues`.
pub fn search_issue(
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

pub fn check_run(node_id: &str, head: &str, name: &str, conclusion: &str, at: &str) -> Value {
    json!({
        "id": 1, "node_id": node_id, "head_sha": head, "name": name, "status": "completed",
        "conclusion": conclusion, "started_at": at, "completed_at": at,
        "html_url": "https://github.com/acme/widget/actions/runs/1/job/1",
        "check_suite": { "id": 901 },
        "app": { "id": 15368, "slug": "github-actions", "node_id": "MDM6QXBwMTUzNjg=", "name": "GitHub Actions",
                 "owner": rest_user("github", "O_github", "Organization") },
        "output": { "title": null, "summary": null }
    })
}

// ---------------------------------------------------------------------------
// GraphQL fixtures
// ---------------------------------------------------------------------------

pub fn gql_actor(typename: &str, login: &str, id: &str) -> Value {
    json!({ "__typename": typename, "login": login, "id": id })
}

pub fn user(login: &str) -> Value {
    gql_actor("User", login, &format!("U_{login}"))
}

pub fn bot(login: &str) -> Value {
    gql_actor("Bot", login, &format!("BOT_{login}"))
}

/// The `checkSuites` connection of a head commit in the pull request query
/// carrying the same check runs as the REST `check-runs` fixtures: one suite
/// per `check_suite.id` with the suite's app, enum values in upper case and
/// the REST `html_url` as the run's `permalink`.
pub fn gql_check_suites(runs: &[Value]) -> Value {
    let mut suites: Vec<(Value, Value, Vec<Value>)> = Vec::new();
    for r in runs {
        let id = r["check_suite"]["id"].clone();
        let run = json!({
            "id": r["node_id"],
            "name": r["name"],
            "status": r["status"].as_str().map(str::to_ascii_uppercase),
            "conclusion": r["conclusion"].as_str().map(str::to_ascii_uppercase),
            "startedAt": r["started_at"],
            "completedAt": r["completed_at"],
            "permalink": r["html_url"],
        });
        match suites.iter_mut().find(|(s, _, _)| *s == id) {
            Some((_, _, list)) => list.push(run),
            None => {
                let app = if r["app"].is_null() {
                    Value::Null
                } else {
                    json!({ "id": r["app"]["node_id"], "slug": r["app"]["slug"] })
                };
                suites.push((id, app, vec![run]));
            }
        }
    }
    let nodes = suites
        .into_iter()
        .map(|(id, app, runs)| json!({ "databaseId": id, "app": app, "checkRuns": connection(runs) }))
        .collect();
    connection(nodes)
}

pub fn page_info(has_next: bool, cursor: Option<&str>) -> Value {
    json!({ "hasNextPage": has_next, "endCursor": cursor })
}

pub fn connection(nodes: Vec<Value>) -> Value {
    json!({ "pageInfo": page_info(false, Some("end")), "nodes": nodes })
}

pub fn gql_repo(id: &str, owner: Value, name: &str, visibility: &str, permission: &str) -> Value {
    let login = owner["login"].as_str().unwrap().to_string();
    json!({
        "id": id,
        "name": name,
        "url": format!("https://github.com/{login}/{name}"),
        "visibility": visibility,
        "isFork": false,
        "viewerPermission": permission,
        "owner": owner
    })
}

/// Commit author for a pull request commit: `(login, id)` or no linked user.
pub type GitUser<'a> = Option<(&'a str, &'a str)>;

pub fn pr_commit(
    repo: &str,
    number: u64,
    oid: &str,
    author: GitUser<'_>,
    committed: &str,
) -> Value {
    let user = match author {
        Some((login, id)) => json!({ "user": { "id": id, "login": login } }),
        None => json!({ "user": null }),
    };
    json!({
        "url": format!("https://github.com/{repo}/pull/{number}/commits/{oid}"),
        "commit": {
            "oid": oid,
            "committedDate": committed,
            "authoredDate": committed,
            "url": format!("https://github.com/{repo}/commit/{oid}"),
            "parents": { "totalCount": 1 },
            "author": user.clone(),
            "committer": user
        }
    })
}

pub fn review(
    id: &str,
    state: &str,
    body: &str,
    submitted_at: &str,
    author: Value,
    comments: Vec<Value>,
) -> Value {
    json!({
        "id": id,
        "state": state,
        "body": body,
        "submittedAt": submitted_at,
        "url": format!("https://github.com/acme/widget/pull/1#pullrequestreview-{id}"),
        "author": author,
        "comments": { "nodes": comments }
    })
}

pub fn review_comment(
    id: &str,
    author: Value,
    created_at: &str,
    review_id: &str,
    reply_to: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "state": "SUBMITTED",
        "createdAt": created_at,
        "lastEditedAt": null,
        "url": format!("https://github.com/acme/widget/pull/1#discussion_{id}"),
        "author": author,
        "replyTo": reply_to.map(|r| json!({ "id": r })),
        "pullRequestReview": { "id": review_id }
    })
}

pub fn thread(id: &str, resolved: bool, comments: Vec<Value>) -> Value {
    let n = comments.len();
    json!({
        "id": id,
        "isResolved": resolved,
        "isOutdated": false,
        "path": "src/lib.rs",
        "resolvedBy": null,
        "comments": {
            "totalCount": n,
            "pageInfo": page_info(false, Some("end")),
            "nodes": comments
        }
    })
}

pub fn issue_comment(id: &str, author: Value, created_at: &str, edited: Option<&str>) -> Value {
    json!({
        "id": id,
        "createdAt": created_at,
        "lastEditedAt": edited,
        "url": format!("https://github.com/acme/widget/pull/1#issuecomment-{id}"),
        "author": author
    })
}

pub fn tl(typename: &str, id: &str, created_at: &str, actor: Value, extra: Value) -> Value {
    let mut v =
        json!({ "__typename": typename, "id": id, "createdAt": created_at, "actor": actor });
    if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in extra {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

// ---------------------------------------------------------------------------
// The mocked world
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RepoFixture {
    pub owner: String,
    pub name: String,
    /// GraphQL `RepositoryFields`.
    pub gql: Value,
    /// REST repository object embedded in commit search results.
    pub rest: Value,
    /// Item of `GET /user/repos` (with permissions and `pushed_at`).
    pub listed: Value,
    /// My commits on the default branch (`GET /repos/{owner}/{name}/commits`).
    pub default_branch: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct PrFixture {
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub node_id: String,
    /// `updated_at` reported by search.
    pub updated_at: String,
    /// The `pullRequest` GraphQL object.
    pub gql: Value,
    pub check_runs: Vec<Value>,
    /// Found by `reviewed-by:` as well as `involves:`.
    pub reviewed_by_me: bool,
}

impl PrFixture {
    pub fn head(&self) -> String {
        self.gql["headRefOid"].as_str().unwrap().to_string()
    }

    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.owner, self.name, self.number
        )
    }
}

#[derive(Debug, Clone)]
pub struct IssueFixture {
    pub owner: String,
    pub name: String,
    pub number: u64,
    pub node_id: String,
    pub updated_at: String,
    /// The `issueOrPullRequest` GraphQL object.
    pub gql: Value,
}

#[derive(Debug, Clone)]
pub struct World {
    pub repos: Vec<RepoFixture>,
    pub commits: Vec<Value>,
    pub prs: Vec<PrFixture>,
    pub issues: Vec<IssueFixture>,
}

pub fn widget() -> RepoFixture {
    RepoFixture {
        owner: "acme".into(),
        name: "widget".into(),
        gql: gql_repo(
            WIDGET_ID,
            gql_actor("Organization", "acme", "O_acme"),
            "widget",
            "PUBLIC",
            "WRITE",
        ),
        rest: rest_repo(WIDGET_ID, "acme", "Organization", "widget", false),
        listed: listed_repo(
            &rest_repo(WIDGET_ID, "acme", "Organization", "widget", false),
            "push",
            "2024-03-08T09:00:00Z",
        ),
        default_branch: vec![],
    }
}

pub fn dotfiles() -> RepoFixture {
    RepoFixture {
        owner: ME.into(),
        name: "dotfiles".into(),
        gql: gql_repo(DOTFILES_ID, user(ME), "dotfiles", "PUBLIC", "ADMIN"),
        rest: rest_repo(DOTFILES_ID, ME, "User", "dotfiles", false),
        listed: listed_repo(
            &rest_repo(DOTFILES_ID, ME, "User", "dotfiles", false),
            "admin",
            "2024-03-08T00:00:00Z",
        ),
        default_branch: vec![],
    }
}

pub fn secret() -> RepoFixture {
    RepoFixture {
        owner: "acme".into(),
        name: "secret".into(),
        gql: gql_repo(
            SECRET_ID,
            gql_actor("Organization", "acme", "O_acme"),
            "secret",
            "PRIVATE",
            "READ",
        ),
        rest: rest_repo(SECRET_ID, "acme", "Organization", "secret", true),
        listed: listed_repo(
            &rest_repo(SECRET_ID, "acme", "Organization", "secret", true),
            "pull",
            "2024-03-05T00:00:00Z",
        ),
        default_branch: vec![],
    }
}

/// Pull request acme/widget#10 by me (open in the base world).
///
/// - opened 2024-03-04T09:00Z by me; review requested from bob at 09:05
/// - commits: sha 11 (me, 03-04 08:30), sha 12 (me, 03-05 09:30),
///   sha 13 (dependabot[bot], 03-05 10:00), sha 14 (no linked user,
///   03-05 11:00, head)
/// - reviews: bob APPROVED 03-06 10:00 (no body); carol CHANGES_REQUESTED
///   03-05 14:00 with body "Please fix" and inline comment PRRC_A1
///   (03-05 13:59); my COMMENTED review 03-05 15:00 without body that only
///   contains my reply PRRC_A2 in carol's thread
/// - conversation comments: IC_A1 me 03-04 10:30, IC_A2 bob 03-04 11:00,
///   IC_A3 github-actions bot 03-04 11:05
/// - one successful check run on the head commit, completed 03-05 11:10
pub fn pr_a() -> PrFixture {
    let repo = "acme/widget";
    let carol_c = review_comment(
        "PRRC_A1",
        user("carol"),
        "2024-03-05T13:59:00Z",
        "PRR_A2",
        None,
    );
    let my_reply = review_comment(
        "PRRC_A2",
        user(ME),
        "2024-03-05T15:00:00Z",
        "PRR_A3",
        Some("PRRC_A1"),
    );
    PrFixture {
        owner: "acme".into(),
        name: "widget".into(),
        number: 10,
        node_id: "PR_A".into(),
        updated_at: "2024-03-06T12:00:00Z".into(),
        reviewed_by_me: false,
        check_runs: vec![check_run(
            "CR_A1",
            &sha(14),
            "build",
            "success",
            "2024-03-05T11:10:00Z",
        )],
        gql: json!({
            "id": "PR_A",
            "number": 10,
            "title": "Add widget",
            "url": "https://github.com/acme/widget/pull/10",
            "state": "OPEN",
            "isDraft": false,
            "createdAt": "2024-03-04T09:00:00Z",
            "updatedAt": "2024-03-06T12:00:00Z",
            "closedAt": null,
            "mergedAt": null,
            "headRefOid": sha(14),
            "author": user(ME),
            "mergedBy": null,
            "headCommit": { "nodes": [ { "commit": { "oid": sha(14), "statusCheckRollup": { "state": "SUCCESS" } } } ] },
            "commits": connection(vec![
                pr_commit(repo, 10, &sha(11), Some((ME, ME_ID)), "2024-03-04T08:30:00Z"),
                pr_commit(repo, 10, &sha(12), Some((ME, ME_ID)), "2024-03-05T09:30:00Z"),
                pr_commit(repo, 10, &sha(13), Some(("dependabot[bot]", "BOT_dependabot")), "2024-03-05T10:00:00Z"),
                pr_commit(repo, 10, &sha(14), None, "2024-03-05T11:00:00Z"),
            ]),
            "reviews": connection(vec![
                review("PRR_A1", "APPROVED", "", "2024-03-06T10:00:00Z", user("bob"), vec![]),
                review("PRR_A2", "CHANGES_REQUESTED", "Please fix", "2024-03-05T14:00:00Z", user("carol"), vec![carol_c.clone()]),
                review("PRR_A3", "COMMENTED", "", "2024-03-05T15:00:00Z", user(ME), vec![my_reply.clone()]),
            ]),
            "reviewThreads": connection(vec![thread("PRRT_A1", false, vec![carol_c, my_reply])]),
            "comments": connection(vec![
                issue_comment("IC_A1", user(ME), "2024-03-04T10:30:00Z", None),
                issue_comment("IC_A2", user("bob"), "2024-03-04T11:00:00Z", None),
                issue_comment("IC_A3", bot("github-actions"), "2024-03-04T11:05:00Z", None),
            ]),
            "timelineItems": connection(vec![
                tl("ReviewRequestedEvent", "RRE_A1", "2024-03-04T09:05:00Z", user(ME), json!({ "requestedReviewer": { "login": "bob" } })),
            ])
        }),
    }
}

/// Pull request me/dotfiles#3 by me: opened 2024-03-06T08:00Z, one commit
/// by me (sha 21, 03-06 07:50), merged and closed by me at 03-06 09:00.
pub fn pr_b() -> PrFixture {
    PrFixture {
        owner: ME.into(),
        name: "dotfiles".into(),
        number: 3,
        node_id: "PR_B".into(),
        updated_at: "2024-03-06T09:00:00Z".into(),
        reviewed_by_me: false,
        check_runs: vec![],
        gql: json!({
            "id": "PR_B",
            "number": 3,
            "title": "Tidy shell config",
            "url": "https://github.com/me/dotfiles/pull/3",
            "state": "MERGED",
            "isDraft": false,
            "createdAt": "2024-03-06T08:00:00Z",
            "updatedAt": "2024-03-06T09:00:00Z",
            "closedAt": "2024-03-06T09:00:00Z",
            "mergedAt": "2024-03-06T09:00:00Z",
            "headRefOid": sha(21),
            "author": user(ME),
            "mergedBy": user(ME),
            "headCommit": { "nodes": [ { "commit": { "oid": sha(21), "statusCheckRollup": null } } ] },
            "commits": connection(vec![
                pr_commit("me/dotfiles", 3, &sha(21), Some((ME, ME_ID)), "2024-03-06T07:50:00Z"),
            ]),
            "reviews": connection(vec![]),
            "reviewThreads": connection(vec![]),
            "comments": connection(vec![]),
            "timelineItems": connection(vec![
                tl("MergedEvent", "ME_B", "2024-03-06T09:00:00Z", user(ME), json!({ "url": "https://github.com/me/dotfiles/pull/3#event-1" })),
                tl("ClosedEvent", "CE_B", "2024-03-06T09:00:00Z", user(ME), json!({ "url": "https://github.com/me/dotfiles/pull/3#event-2" })),
            ])
        }),
    }
}

/// Pull request acme/widget#20 by bob, reviewed by me: opened
/// 2024-03-05T08:00Z with one commit by bob (sha 31); bob comments at
/// 08:10; my CHANGES_REQUESTED review with body "Needs tests" at
/// 2024-03-07T09:00Z containing inline comment PRRC_C1 (08:59).
pub fn pr_c() -> PrFixture {
    let my_c = review_comment("PRRC_C1", user(ME), "2024-03-07T08:59:00Z", "PRR_C1", None);
    PrFixture {
        owner: "acme".into(),
        name: "widget".into(),
        number: 20,
        node_id: "PR_C".into(),
        updated_at: "2024-03-07T09:00:00Z".into(),
        reviewed_by_me: true,
        check_runs: vec![],
        gql: json!({
            "id": "PR_C",
            "number": 20,
            "title": "Bob's refactor",
            "url": "https://github.com/acme/widget/pull/20",
            "state": "OPEN",
            "isDraft": false,
            "createdAt": "2024-03-05T08:00:00Z",
            "updatedAt": "2024-03-07T09:00:00Z",
            "closedAt": null,
            "mergedAt": null,
            "headRefOid": sha(31),
            "author": user("bob"),
            "mergedBy": null,
            "headCommit": { "nodes": [ { "commit": { "oid": sha(31), "statusCheckRollup": null } } ] },
            "commits": connection(vec![
                pr_commit("acme/widget", 20, &sha(31), Some(("bob", "U_bob")), "2024-03-05T07:00:00Z"),
            ]),
            "reviews": connection(vec![
                review("PRR_C1", "CHANGES_REQUESTED", "Needs tests", "2024-03-07T09:00:00Z", user(ME), vec![my_c.clone()]),
            ]),
            "reviewThreads": connection(vec![thread("PRRT_C1", false, vec![my_c])]),
            "comments": connection(vec![
                issue_comment("IC_C1", user("bob"), "2024-03-05T08:10:00Z", None),
            ]),
            "timelineItems": connection(vec![])
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn gql_issue(
    id: &str,
    number: u64,
    title: &str,
    repo: &str,
    author: Value,
    created: &str,
    closed: Option<&str>,
    timeline: Vec<Value>,
) -> Value {
    json!({
        "__typename": "Issue",
        "id": id,
        "number": number,
        "title": title,
        "url": format!("https://github.com/{repo}/issues/{number}"),
        "state": if closed.is_some() { "CLOSED" } else { "OPEN" },
        "createdAt": created,
        "closedAt": closed,
        "author": author,
        "timelineItems": connection(timeline)
    })
}

/// Issue me/dotfiles#5 opened by me 2024-03-05T07:00Z, closed by me
/// 2024-03-07T07:00Z.
pub fn issue_1() -> IssueFixture {
    IssueFixture {
        owner: ME.into(),
        name: "dotfiles".into(),
        number: 5,
        node_id: "I_1".into(),
        updated_at: "2024-03-07T07:00:00Z".into(),
        gql: gql_issue(
            "I_1",
            5,
            "Shell is slow",
            "me/dotfiles",
            user(ME),
            "2024-03-05T07:00:00Z",
            Some("2024-03-07T07:00:00Z"),
            vec![tl(
                "ClosedEvent",
                "CLE_1",
                "2024-03-07T07:00:00Z",
                user(ME),
                json!({ "url": "https://github.com/me/dotfiles/issues/5#event-1" }),
            )],
        ),
    }
}

/// Issue acme/widget#7 opened by bob 2024-02-20 (before the period),
/// closed by me 2024-03-06T06:00Z and reopened by bob at 07:00.
pub fn issue_2() -> IssueFixture {
    IssueFixture {
        owner: "acme".into(),
        name: "widget".into(),
        number: 7,
        node_id: "I_2".into(),
        updated_at: "2024-03-06T07:00:00Z".into(),
        gql: gql_issue(
            "I_2",
            7,
            "Widget breaks",
            "acme/widget",
            user("bob"),
            "2024-02-20T07:00:00Z",
            None,
            vec![
                tl(
                    "ClosedEvent",
                    "CLE_2",
                    "2024-03-06T06:00:00Z",
                    user(ME),
                    json!({ "url": "https://github.com/acme/widget/issues/7#event-1" }),
                ),
                tl(
                    "ReopenedEvent",
                    "REE_2",
                    "2024-03-06T07:00:00Z",
                    user("bob"),
                    json!({}),
                ),
            ],
        ),
    }
}

impl World {
    /// The base fixture.
    ///
    /// Commit search (`author:me`) returns:
    /// - sha 1 in acme/widget, committed 2024-03-04T10:00Z
    /// - sha 2 in me/dotfiles, 2024-03-05T12:00Z, a merge commit (2 parents)
    /// - sha 3 in me/dotfiles, 2024-03-07T23:30Z
    /// - sha 4 in acme/widget by dependabot[bot] (a bot), 2024-03-05T08:00Z
    /// - sha 5 in acme/widget without a linked GitHub user, 2024-03-06T08:00Z
    ///
    /// The default branches (`GET /repos/{owner}/{name}/commits?author=me`)
    /// hold my commits sha 1 (acme/widget) and sha 2, sha 3 (me/dotfiles),
    /// the same ones commit search finds; acme/secret has none of mine.
    /// `GET /user/repos` lists acme/widget (last push 03-08), me/dotfiles
    /// (03-08) and acme/secret (03-05).
    ///
    /// Pull requests: [`pr_a`], [`pr_b`], [`pr_c`]. Issues: [`issue_1`],
    /// [`issue_2`].
    pub fn base() -> Self {
        let mut w = widget();
        let mut d = dotfiles();
        let mine = Some((ME, ME_ID, "User"));
        w.default_branch = vec![list_commit(
            &w.rest,
            &sha(1),
            mine,
            "2024-03-04T10:00:00Z",
            1,
        )];
        d.default_branch = vec![
            list_commit(&d.rest, &sha(3), mine, "2024-03-07T23:30:00Z", 1),
            list_commit(&d.rest, &sha(2), mine, "2024-03-05T12:00:00Z", 2),
        ];
        let me = Some((ME, ME_ID, "User"));
        let commits = vec![
            search_commit(&w.rest, &sha(1), me, "2024-03-04T10:00:00Z", 1),
            search_commit(&d.rest, &sha(2), me, "2024-03-05T12:00:00Z", 2),
            search_commit(&d.rest, &sha(3), me, "2024-03-07T23:30:00Z", 1),
            search_commit(
                &w.rest,
                &sha(4),
                Some(("dependabot[bot]", "BOT_dependabot", "Bot")),
                "2024-03-05T08:00:00Z",
                1,
            ),
            search_commit(&w.rest, &sha(5), None, "2024-03-06T08:00:00Z", 1),
        ];
        Self {
            repos: vec![w, d, secret()],
            commits,
            prs: vec![pr_a(), pr_b(), pr_c()],
            issues: vec![issue_1(), issue_2()],
        }
    }

    pub fn repo_mut(&mut self, owner: &str, name: &str) -> &mut RepoFixture {
        self.repos
            .iter_mut()
            .find(|r| r.owner == owner && r.name == name)
            .unwrap()
    }

    pub fn pr_mut(&mut self, number: u64) -> &mut PrFixture {
        self.prs.iter_mut().find(|p| p.number == number).unwrap()
    }

    fn repo_gql(&self, owner: &str, name: &str) -> Value {
        self.repos
            .iter()
            .find(|r| r.owner == owner && r.name == name)
            .map(|r| r.gql.clone())
            .unwrap_or_else(|| panic!("no repo fixture {owner}/{name}"))
    }

    /// Mounts every endpoint the sync touches (default priority, so tests
    /// can override single responses with a higher priority).
    pub async fn mount(&self, server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "login": ME, "id": 1, "node_id": ME_ID, "type": "User", "name": "Me"
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rate_limit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resources": {
                    "core": { "limit": 5000, "remaining": 4999, "reset": 4102444800u64 },
                    "search": { "limit": 30, "remaining": 30, "reset": 4102444800u64 },
                    "graphql": { "limit": 5000, "remaining": 5000, "reset": 4102444800u64 }
                }
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search/commits"))
            .and(query_param_contains("q", format!("author:{ME}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(search_page(self.commits.clone())),
            )
            .mount(server)
            .await;
        // Repositories by last push, newest first, as GitHub sorts them.
        let mut listed: Vec<Value> = self.repos.iter().map(|r| r.listed.clone()).collect();
        listed.sort_by(|a, b| {
            b["pushed_at"]
                .as_str()
                .unwrap_or("")
                .cmp(a["pushed_at"].as_str().unwrap_or(""))
        });
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(listed)))
            .mount(server)
            .await;
        for r in &self.repos {
            Mock::given(method("GET"))
                .and(path(format!("/repos/{}/{}/commits", r.owner, r.name)))
                .and(query_param("author", ME))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(Value::Array(r.default_branch.clone())),
                )
                .mount(server)
                .await;
        }
        let pr_items = |only_reviewed: bool| -> Vec<Value> {
            self.prs
                .iter()
                .filter(|p| !only_reviewed || p.reviewed_by_me)
                .map(|p| search_issue(&p.owner, &p.name, p.number, &p.node_id, true, &p.updated_at))
                .collect()
        };
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param_contains("q", format!("involves:{ME} is:pr")))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(pr_items(false))))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param_contains("q", format!("reviewed-by:{ME} is:pr")))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(pr_items(true))))
            .mount(server)
            .await;
        let issue_items: Vec<Value> = self
            .issues
            .iter()
            .map(|i| {
                search_issue(
                    &i.owner,
                    &i.name,
                    i.number,
                    &i.node_id,
                    false,
                    &i.updated_at,
                )
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/search/issues"))
            .and(query_param_contains("q", format!("involves:{ME} is:issue")))
            .respond_with(ResponseTemplate::new(200).set_body_json(search_page(issue_items)))
            .mount(server)
            .await;
        for r in &self.repos {
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_string_contains("query RepositoryDetail"))
                .and(body_partial_json(
                    json!({ "variables": { "owner": r.owner, "name": r.name } }),
                ))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({ "data": { "repository": r.gql } })),
                )
                .mount(server)
                .await;
        }
        for p in &self.prs {
            self.mount_pr(
                server,
                p,
                ResponseTemplate::new(200).set_body_json(self.pr_body(p)),
            )
            .await;
            Mock::given(method("GET"))
                .and(path(format!(
                    "/repos/{}/{}/commits/{}/check-runs",
                    p.owner,
                    p.name,
                    p.head()
                )))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "total_count": p.check_runs.len(),
                    "check_runs": p.check_runs
                })))
                .mount(server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!(
                    "/repos/{}/{}/commits/{}/statuses",
                    p.owner,
                    p.name,
                    p.head()
                )))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .mount(server)
                .await;
        }
        for i in &self.issues {
            let mut repo = self.repo_gql(&i.owner, &i.name);
            repo["issueOrPullRequest"] = i.gql.clone();
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_string_contains("query IssueDetail"))
                .and(body_partial_json(json!({
                    "variables": { "owner": i.owner, "name": i.name, "number": i.number }
                })))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({ "data": { "repository": repo } })),
                )
                .mount(server)
                .await;
        }
    }

    /// The `PullRequestDetail` response body of a pull request.
    /// The `PullRequestDetail` response. The head commit carries the
    /// fixture's check runs and no commit statuses, like the REST mocks.
    pub fn pr_body(&self, p: &PrFixture) -> Value {
        let mut repo = self.repo_gql(&p.owner, &p.name);
        let mut pr = p.gql.clone();
        if let Some(head) = pr["headCommit"]["nodes"][0]["commit"].as_object_mut() {
            head.entry("status").or_insert(Value::Null);
            head.entry("checkSuites")
                .or_insert_with(|| gql_check_suites(&p.check_runs));
        }
        repo["pullRequest"] = pr;
        json!({ "data": { "repository": repo } })
    }

    /// Mounts a `PullRequestDetail` response for one pull request.
    pub async fn mount_pr(&self, server: &MockServer, p: &PrFixture, resp: ResponseTemplate) {
        pr_detail_mock(&p.owner, &p.name, p.number, resp, 5)
            .mount(server)
            .await;
    }
}

/// A `PullRequestDetail` mock for one pull request at `priority` (1 is the
/// highest; the world uses the default 5).
pub fn pr_detail_mock(
    owner: &str,
    name: &str,
    number: u64,
    resp: ResponseTemplate,
    priority: u8,
) -> Mock {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("query PullRequestDetail"))
        .and(body_partial_json(json!({
            "variables": { "owner": owner, "name": name, "number": number }
        })))
        .respond_with(resp)
        .with_priority(priority)
}

/// Starts a server serving `world`.
pub async fn serve(world: &World) -> MockServer {
    let server = MockServer::start().await;
    world.mount(&server).await;
    server
}

/// Replaces everything the server serves (and forgets recorded requests).
pub async fn reserve(server: &MockServer, world: &World) {
    server.reset().await;
    world.mount(server).await;
}

// ---------------------------------------------------------------------------
// JSON report helpers
// ---------------------------------------------------------------------------

/// Status of one data kind in a report's `coverage` list.
pub fn coverage_status(report: &Value, kind: &str) -> String {
    report["coverage"]
        .as_array()
        .expect("coverage list")
        .iter()
        .find(|c| c["kind"] == kind)
        .unwrap_or_else(|| panic!("no coverage entry for {kind}: {report}"))["status"]
        .as_str()
        .unwrap()
        .to_string()
}

/// All segment reasons of one data kind.
pub fn coverage_reasons(report: &Value, kind: &str) -> Vec<String> {
    report["coverage"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == kind)
        .flat_map(|c| c["segments"].as_array().cloned().unwrap_or_default())
        .filter_map(|s| s["reason"].as_str().map(String::from))
        .collect()
}

/// The breakdown bucket whose start date is `date` (YYYY-MM-DD).
pub fn bucket<'a>(report: &'a Value, date: &str) -> &'a Value {
    report["activity_breakdown"]
        .as_array()
        .expect("activity_breakdown")
        .iter()
        .find(|b| b["bucket"]["key"] == date)
        .unwrap_or_else(|| panic!("no bucket for {date}: {}", report["activity_breakdown"]))
}

/// Pull request numbers in a `prs` report.
pub fn pr_numbers(report: &Value) -> Vec<u64> {
    let mut v: Vec<u64> = report["pull_requests"]
        .as_array()
        .expect("pull_requests")
        .iter()
        .map(|p| p["number"].as_u64().unwrap())
        .collect();
    v.sort();
    v
}

/// Environment variables the OS needs to start a process with networking
/// (Windows cannot initialize sockets without them).
fn keep_system_env(c: &mut Command) {
    for k in [
        "SYSTEMROOT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "COMSPEC",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
}
