//! Pull request fetching and normalization. Everything comes from GraphQL
//! except commit statuses (and check runs that do not fit in the main
//! query), which come from REST.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{
    ActivityEvent, Actor, CheckOutcome, Collection, EventId, EventMeta, EventType, PrState,
    PullRequest, ReviewState, ReviewThread, Source,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::gql::{self, Connection, GqlActor, GqlRepository, IdOnly, actor_of, time};
use crate::http::{GraphqlError, Transport, parse_time};
use crate::queries::{self, PrConnection};
use crate::{ApiError, PrBundle};

/// A close within this many seconds of the merge time is the close that
/// GitHub records as part of merging.
const MERGE_CLOSE_WINDOW_SECS: i64 = 60;

/// GraphQL collections of a pull request whose errors make only themselves
/// partial (checks have their own REST fallback).
const GRAPHQL_COLLECTIONS: [Collection; 5] = [
    Collection::Commits,
    Collection::Reviews,
    Collection::Comments,
    Collection::ReviewComments,
    Collection::Timeline,
];

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlPullRequest {
    id: Option<String>,
    number: Option<u64>,
    title: Option<String>,
    url: Option<String>,
    state: Option<String>,
    is_draft: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
    merged_at: Option<String>,
    head_ref_oid: Option<String>,
    author: Option<GqlActor>,
    merged_by: Option<GqlActor>,
    head_commit: Option<Connection>,
    commits: Option<Connection>,
    reviews: Option<Connection>,
    review_threads: Option<Connection>,
    comments: Option<Connection>,
    timeline_items: Option<Connection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct HeadCommitNode {
    commit: Option<HeadCommit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct HeadCommit {
    oid: Option<String>,
    status_check_rollup: Option<Rollup>,
    /// `null` when the commit has no commit statuses at all.
    status: Option<Value>,
    check_suites: Option<Connection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlCheckSuite {
    database_id: Option<u64>,
    app: Option<GqlApp>,
    check_runs: Option<Connection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GqlApp {
    id: Option<String>,
    slug: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlCheckRun {
    id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    permalink: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rollup {
    state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CommitNode {
    url: Option<String>,
    commit: Option<GitCommit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GitCommit {
    oid: Option<String>,
    message_headline: Option<String>,
    committed_date: Option<String>,
    authored_date: Option<String>,
    url: Option<String>,
    parents: Option<Count>,
    author: Option<GitActor>,
    committer: Option<GitActor>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Count {
    total_count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GitActor {
    user: Option<GqlActor>,
}

impl GitActor {
    /// The GitHub user linked to a git identity (`None` when unlinked).
    fn user_actor(a: &Option<GitActor>) -> Option<Actor> {
        let user = a.as_ref()?.user.as_ref()?;
        let actor = Actor::new(
            user.id.clone(),
            user.login.clone(),
            Some(user.typename.as_deref().unwrap_or("User")),
        );
        (!actor.is_unknown()).then_some(actor)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ReviewNode {
    id: Option<String>,
    state: Option<String>,
    body: Option<String>,
    submitted_at: Option<String>,
    url: Option<String>,
    author: Option<GqlActor>,
    comments: Option<Connection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CommentNode {
    pub id: Option<String>,
    pub state: Option<String>,
    pub created_at: Option<String>,
    pub last_edited_at: Option<String>,
    pub url: Option<String>,
    pub author: Option<GqlActor>,
    pub reply_to: Option<IdOnly>,
    pub pull_request_review: Option<IdOnly>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThreadNode {
    id: Option<String>,
    is_resolved: Option<bool>,
    is_outdated: Option<bool>,
    path: Option<String>,
    resolved_by: Option<GqlActor>,
    comments: Option<Connection>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Named {
    login: Option<String>,
    slug: Option<String>,
    name: Option<String>,
}

impl Named {
    fn label(n: &Option<Named>) -> Option<String> {
        let n = n.as_ref()?;
        n.login
            .clone()
            .or_else(|| n.slug.clone())
            .or_else(|| n.name.clone())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Oid {
    oid: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TimelineNode {
    #[serde(rename = "__typename")]
    pub typename: Option<String>,
    pub id: Option<String>,
    pub created_at: Option<String>,
    pub url: Option<String>,
    pub actor: Option<GqlActor>,
    before_commit: Option<Oid>,
    after_commit: Option<Oid>,
    requested_reviewer: Option<Named>,
    assignee: Option<Named>,
    label: Option<Named>,
    milestone_title: Option<String>,
    previous_title: Option<String>,
    current_title: Option<String>,
    review: Option<IdOnly>,
    previous_review_state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestCheckRun {
    node_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    html_url: Option<String>,
    check_suite: Option<RestId>,
    app: Option<RestApp>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestId {
    id: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestApp {
    node_id: Option<String>,
    slug: Option<String>,
}

impl RestCheckRun {
    fn into_record(self) -> CheckRunRecord {
        CheckRunRecord {
            node_id: self.node_id,
            name: self.name,
            status: self.status,
            conclusion: self.conclusion,
            started_at: self.started_at,
            completed_at: self.completed_at,
            url: self.html_url,
            suite_id: self.check_suite.and_then(|s| s.id),
            app: self.app.map(|a| (a.node_id, a.slug)),
            source: Source::Rest,
        }
    }
}

/// A check run of the head commit, from either REST or GraphQL. The GraphQL
/// fields map one to one: `id` is the REST `node_id`, `permalink` the REST
/// `html_url`, the suite's `databaseId` the REST `check_suite.id` and the
/// suite's app the run's app.
#[derive(Debug)]
struct CheckRunRecord {
    node_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    url: Option<String>,
    suite_id: Option<u64>,
    /// Node id and slug of the app.
    app: Option<(Option<String>, Option<String>)>,
    source: Source,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RestStatus {
    node_id: Option<String>,
    state: Option<String>,
    context: Option<String>,
    created_at: Option<String>,
    creator: Option<RestUser>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RestUser {
    pub login: Option<String>,
    pub node_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

impl RestUser {
    pub fn actor(&self) -> Actor {
        Actor::new(
            self.node_id.clone(),
            self.login.clone(),
            self.kind.as_deref(),
        )
    }
}

/// Which collections are incomplete, and why.
#[derive(Debug, Default)]
struct Tracker {
    partial: BTreeSet<Collection>,
    notes: Vec<String>,
}

impl Tracker {
    fn partial(&mut self, c: Collection, note: impl Into<String>) {
        self.partial.insert(c);
        let note = note.into();
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }

    fn skipped(&mut self, c: Collection, what: &str, n: usize) {
        if n > 0 {
            self.partial(
                c,
                format!("{n} {what} could not be read (null or missing fields) and were skipped"),
            );
        }
    }
}

/// Errors that make further requests pointless.
fn is_fatal(e: &ApiError) -> bool {
    matches!(e, ApiError::Unauthorized | ApiError::RateLimited { .. })
}

/// Maps GraphQL errors of the main query to the collections they affect.
fn mark_errors(errors: &[GraphqlError], tracker: &mut Tracker) {
    for e in errors {
        let path = e.field_path();
        let pos = path.iter().position(|p| *p == "pullRequest");
        let field = pos.and_then(|i| path.get(i + 1)).copied();
        let note = format!("GraphQL error: {}", e.message());
        let affected: Vec<Collection> = match field {
            Some("commits") => vec![Collection::Commits],
            Some("reviews") => {
                if path.contains(&"comments") {
                    vec![Collection::Reviews, Collection::ReviewComments]
                } else {
                    vec![Collection::Reviews]
                }
            }
            Some("reviewThreads") => vec![Collection::ReviewComments],
            Some("comments") => vec![Collection::Comments],
            Some("timelineItems") => vec![Collection::Timeline],
            // Errors in the head commit's checks or statuses only make them
            // fall back to REST (see `graphql_checks`).
            Some("headCommit") if path.contains(&"checkSuites") || path.contains(&"status") => {
                vec![]
            }
            Some("headCommit") => {
                tracker.note(format!("latest CI state unavailable: {}", e.message()));
                vec![]
            }
            // Scalar fields of the pull request or errors without a path:
            // nothing can be trusted to be complete.
            _ => GRAPHQL_COLLECTIONS.to_vec(),
        };
        for c in affected {
            tracker.partial(c, note.clone());
        }
    }
}

fn collection_of(conn: PrConnection) -> Collection {
    match conn {
        PrConnection::Commits => Collection::Commits,
        PrConnection::Reviews => Collection::Reviews,
        PrConnection::Threads => Collection::ReviewComments,
        PrConnection::Comments => Collection::Comments,
        PrConnection::Timeline => Collection::Timeline,
    }
}

/// Fetches all remaining pages of one pull request connection.
async fn collect_connection(
    t: &Transport,
    pr_id: &str,
    kind: PrConnection,
    first: Option<Connection>,
    tracker: &mut Tracker,
) -> Result<Vec<Value>, ApiError> {
    let c = collection_of(kind);
    let field = kind.field();
    let Some(mut conn) = first else {
        tracker.partial(c, format!("{field} were not returned"));
        return Ok(Vec::new());
    };
    let mut nodes = Vec::new();
    let mut cursors = HashSet::new();
    loop {
        nodes.extend(conn.nodes.take().unwrap_or_default());
        let cursor = match conn.next_cursor() {
            Ok(None) => break,
            Ok(Some(c)) => c,
            Err(()) => {
                tracker.partial(c, format!("{field}: pagination info missing"));
                break;
            }
        };
        if !cursors.insert(cursor.clone()) {
            tracker.partial(c, format!("{field}: pagination did not advance"));
            break;
        }
        let resp = match t
            .graphql(&kind.page_query(), json!({ "id": pr_id, "after": cursor }))
            .await
        {
            Ok(r) => r,
            Err(e) if is_fatal(&e) => return Err(e),
            Err(e) => {
                tracker.partial(
                    c,
                    format!("{field}: a later page could not be fetched ({e})"),
                );
                break;
            }
        };
        if !resp.errors.is_empty() {
            tracker.partial(
                c,
                format!("{field}: GraphQL error: {}", gql::describe(&resp.errors)),
            );
        }
        let next = resp
            .data
            .as_ref()
            .and_then(|d| d.get("node"))
            .and_then(|n| n.get(field));
        match next.filter(|v| !v.is_null()) {
            Some(v) => conn = gql::from_value(Some(v)),
            None => {
                tracker.partial(c, format!("{field}: a later page returned no data"));
                break;
            }
        }
    }
    Ok(nodes)
}

/// Fetches the remaining comment pages of one review thread.
async fn collect_thread_comments(
    t: &Transport,
    thread_id: &str,
    first: Option<Connection>,
    tracker: &mut Tracker,
) -> Result<(Vec<Value>, Option<u64>), ApiError> {
    let c = Collection::ReviewComments;
    let Some(mut conn) = first else {
        tracker.partial(c, "review thread comments were not returned");
        return Ok((Vec::new(), None));
    };
    let total = conn.total_count;
    let mut nodes = Vec::new();
    let mut cursors = HashSet::new();
    loop {
        nodes.extend(conn.nodes.take().unwrap_or_default());
        let cursor = match conn.next_cursor() {
            Ok(None) => break,
            Ok(Some(c)) => c,
            Err(()) => {
                tracker.partial(c, "review thread comments: pagination info missing");
                break;
            }
        };
        if !cursors.insert(cursor.clone()) {
            tracker.partial(c, "review thread comments: pagination did not advance");
            break;
        }
        let resp = match t
            .graphql(
                &queries::thread_comments_page(),
                json!({ "id": thread_id, "after": cursor }),
            )
            .await
        {
            Ok(r) => r,
            Err(e) if is_fatal(&e) => return Err(e),
            Err(e) => {
                tracker.partial(
                    c,
                    format!("review thread comments: a later page could not be fetched ({e})"),
                );
                break;
            }
        };
        if !resp.errors.is_empty() {
            tracker.partial(
                c,
                format!(
                    "review thread comments: GraphQL error: {}",
                    gql::describe(&resp.errors)
                ),
            );
        }
        let next = resp
            .data
            .as_ref()
            .and_then(|d| d.get("node"))
            .and_then(|n| n.get("comments"));
        match next.filter(|v| !v.is_null()) {
            Some(v) => conn = gql::from_value(Some(v)),
            None => {
                tracker.partial(c, "review thread comments: a later page returned no data");
                break;
            }
        }
    }
    Ok((nodes, total))
}

#[derive(Default)]
struct ChecksResult {
    runs: Vec<CheckRunRecord>,
    /// Check runs that could not be read.
    bad_runs: usize,
    statuses: Vec<Value>,
}

/// What the pull request query returned about the head commit's checks.
struct GraphqlChecks {
    runs: Vec<CheckRunRecord>,
    /// Whether the commit has commit statuses (which only REST lists in
    /// full: GraphQL keeps just the latest status of each context).
    has_statuses: bool,
}

/// The head commit's check runs from the pull request query, or `None` when
/// they are not known completely there (more than one page, GraphQL errors,
/// unexpected shapes) and must be fetched over REST instead.
fn graphql_checks(head: &HeadCommit, errors: &[GraphqlError]) -> Option<GraphqlChecks> {
    let affects_checks = errors.iter().any(|e| {
        let path = e.field_path();
        let pos = path.iter().position(|p| *p == "pullRequest");
        let field = pos.and_then(|i| path.get(i + 1)).copied();
        !matches!(
            field,
            Some("commits" | "reviews" | "reviewThreads" | "comments" | "timelineItems")
        )
    });
    if affects_checks {
        return None;
    }
    let suites = head.check_suites.as_ref()?;
    if !matches!(suites.next_cursor(), Ok(None)) {
        return None;
    }
    let mut runs = Vec::new();
    for suite in suites.nodes.as_ref()? {
        let suite: GqlCheckSuite = serde_json::from_value(suite.clone()).ok()?;
        let conn = suite.check_runs.as_ref()?;
        if !matches!(conn.next_cursor(), Ok(None)) {
            return None;
        }
        for run in conn.nodes.as_ref()? {
            let run: GqlCheckRun = serde_json::from_value(run.clone()).ok()?;
            runs.push(CheckRunRecord {
                node_id: run.id,
                name: run.name,
                status: run.status,
                conclusion: run.conclusion,
                started_at: run.started_at,
                completed_at: run.completed_at,
                url: run.permalink,
                suite_id: suite.database_id,
                app: suite.app.as_ref().map(|a| (a.id.clone(), a.slug.clone())),
                source: Source::Graphql,
            });
        }
    }
    Some(GraphqlChecks {
        runs,
        has_statuses: head.status.as_ref().is_some_and(|v| !v.is_null()),
    })
}

/// Fetches the head commit's check runs over REST unless the pull request
/// query already returned them all, and its commit statuses unless it has
/// none.
async fn fetch_checks(
    t: &Transport,
    owner: &str,
    name: &str,
    sha: &str,
    from_graphql: Option<GraphqlChecks>,
    tracker: &mut Tracker,
) -> Result<ChecksResult, ApiError> {
    let mut out = ChecksResult::default();
    let fetch_statuses = match from_graphql {
        Some(g) => {
            out.runs = g.runs;
            g.has_statuses
        }
        None => {
            let runs_url = t.rest_url(
                &["repos", owner, name, "commits", sha, "check-runs"],
                // Every attempt, not only the latest run of each name (the
                // endpoint's default): a failure that was re-run happened.
                &[("per_page", "100"), ("filter", "all")],
            );
            match t
                .get_paginated(runs_url, |v| v.get("check_runs")?.as_array().cloned())
                .await
            {
                Ok(v) => {
                    let (runs, bad) = parse_values::<RestCheckRun>(v);
                    out.runs = runs.into_iter().map(RestCheckRun::into_record).collect();
                    out.bad_runs = bad;
                }
                Err(e) if is_fatal(&e) => return Err(e),
                Err(e) => {
                    tracker.partial(Collection::Checks, format!("check runs not fetched: {e}"))
                }
            }
            true
        }
    };
    if !fetch_statuses {
        return Ok(out);
    }
    let statuses_url = t.rest_url(
        &["repos", owner, name, "commits", sha, "statuses"],
        &[("per_page", "100")],
    );
    match t
        .get_paginated(statuses_url, |v| v.as_array().cloned())
        .await
    {
        Ok(v) => out.statuses = v,
        Err(e) if is_fatal(&e) => return Err(e),
        Err(e) => tracker.partial(
            Collection::Checks,
            format!("commit statuses not fetched: {e}"),
        ),
    }
    Ok(out)
}

pub(crate) async fn fetch(
    t: &Transport,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<PrBundle, ApiError> {
    let what = format!("pull request {owner}/{name}#{number}");
    let resp = t
        .graphql(
            &queries::pull_request(),
            json!({ "owner": owner, "name": name, "number": number }),
        )
        .await?;
    let repo_value = resp
        .data
        .as_ref()
        .and_then(|d| d.get("repository"))
        .filter(|v| !v.is_null());
    let Some(repo_value) = repo_value else {
        return Err(gql::failure(
            &resp.errors,
            &format!("repository {owner}/{name}"),
        ));
    };
    let pr_value = repo_value.get("pullRequest").filter(|v| !v.is_null());
    let Some(pr_value) = pr_value else {
        return Err(gql::failure(&resp.errors, &what));
    };
    let repo: GqlRepository = gql::from_value(Some(repo_value));
    let repository = repo.to_repository()?;
    let mut pr: GqlPullRequest = serde_json::from_value(pr_value.clone())
        .map_err(|e| ApiError::InvalidResponse(format!("{what}: {e}")))?;
    let missing = |f: &str| ApiError::InvalidResponse(format!("{what} is missing `{f}`"));
    let pr_id = pr.id.clone().ok_or_else(|| missing("id"))?;
    let created_at = time(&pr.created_at).ok_or_else(|| missing("createdAt"))?;
    let pr_url = pr.url.clone().ok_or_else(|| missing("url"))?;

    let mut tracker = Tracker::default();
    mark_errors(&resp.errors, &mut tracker);

    // Remaining GraphQL pages and REST checks run concurrently.
    let mut checks_tracker = Tracker::default();
    let head_sha = pr.head_ref_oid.clone();
    let head_commit = pr
        .head_commit
        .as_ref()
        .map(|c| c.parse_nodes::<HeadCommitNode>().0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| n.commit)
        .find(|c| head_sha.is_none() || c.oid == head_sha);
    let checks_from_graphql = head_commit
        .as_ref()
        .filter(|_| head_sha.is_some())
        .and_then(|c| graphql_checks(c, &resp.errors));
    let graphql_pages = async {
        let commits = collect_connection(
            t,
            &pr_id,
            PrConnection::Commits,
            pr.commits.take(),
            &mut tracker,
        )
        .await?;
        let reviews = collect_connection(
            t,
            &pr_id,
            PrConnection::Reviews,
            pr.reviews.take(),
            &mut tracker,
        )
        .await?;
        let thread_values = collect_connection(
            t,
            &pr_id,
            PrConnection::Threads,
            pr.review_threads.take(),
            &mut tracker,
        )
        .await?;
        let mut threads = Vec::new();
        let mut bad_threads = 0;
        for v in thread_values {
            match serde_json::from_value::<ThreadNode>(v) {
                Ok(mut th) if th.id.is_some() => {
                    let id = th.id.clone().unwrap_or_default();
                    let (comments, total) =
                        collect_thread_comments(t, &id, th.comments.take(), &mut tracker).await?;
                    threads.push((th, comments, total));
                }
                _ => bad_threads += 1,
            }
        }
        tracker.skipped(Collection::ReviewComments, "review threads", bad_threads);
        let comments = collect_connection(
            t,
            &pr_id,
            PrConnection::Comments,
            pr.comments.take(),
            &mut tracker,
        )
        .await?;
        let timeline = collect_connection(
            t,
            &pr_id,
            PrConnection::Timeline,
            pr.timeline_items.take(),
            &mut tracker,
        )
        .await?;
        Ok::<_, ApiError>((commits, reviews, threads, comments, timeline))
    };
    let checks = async {
        match head_sha.as_deref() {
            Some(sha) => {
                fetch_checks(
                    t,
                    owner,
                    name,
                    sha,
                    checks_from_graphql,
                    &mut checks_tracker,
                )
                .await
            }
            None => {
                checks_tracker.partial(
                    Collection::Checks,
                    "head commit unknown; checks not fetched",
                );
                Ok(ChecksResult::default())
            }
        }
    };
    let (pages, checks) = tokio::join!(graphql_pages, checks);
    let (commits, reviews, threads, comments, timeline) = pages?;
    let checks = checks?;
    tracker.partial.extend(checks_tracker.partial);
    for n in checks_tracker.notes {
        tracker.note(n);
    }

    let now = Utc::now();
    let merged_at = time(&pr.merged_at);
    let mut b = Builder {
        repo_id: repository.id.clone(),
        pr_id: pr_id.clone(),
        now,
        events: Vec::new(),
    };

    // Opened.
    b.push(
        EventType::PrOpened,
        EventId::pr_created(&pr_id),
        actor_of(&pr.author),
        pr_id.clone(),
        created_at,
        Some(pr_url.clone()),
        EventMeta::default(),
        Source::Graphql,
    );

    // Commits.
    let (commit_nodes, bad) = parse_values::<CommitNode>(commits);
    tracker.skipped(Collection::Commits, "commits", bad);
    let mut current_commit_shas = Vec::new();
    let mut bad = 0;
    for node in commit_nodes {
        let Some(c) = node.commit else {
            bad += 1;
            continue;
        };
        let (Some(oid), Some(at)) = (c.oid.clone(), time(&c.committed_date)) else {
            bad += 1;
            continue;
        };
        current_commit_shas.push(oid.clone());
        let meta = EventMeta {
            sha: Some(oid.clone()),
            author_date: time(&c.authored_date),
            is_merge_commit: c
                .parents
                .as_ref()
                .and_then(|p| p.total_count)
                .map(|n| n > 1),
            committer: GitActor::user_actor(&c.committer),
            time_basis: Some("commit_date".into()),
            message_headline: c
                .message_headline
                .clone()
                .map(|h| crate::http::truncate(&h, 300)),
            ..EventMeta::default()
        };
        b.push(
            EventType::PrCommit,
            EventId::pr_commit(&pr_id, &oid),
            GitActor::user_actor(&c.author).unwrap_or_else(Actor::unknown),
            oid,
            at,
            node.url.or(c.url),
            meta,
            Source::Graphql,
        );
    }
    tracker.skipped(Collection::Commits, "commits", bad);

    // Timeline.
    let (items, bad) = parse_values::<TimelineNode>(timeline);
    tracker.skipped(Collection::Timeline, "timeline items", bad);
    let mut bad = 0;
    let mut saw_merged = false;
    let mut unsupported: HashMap<String, usize> = HashMap::new();
    let mut dismissed_previous: HashMap<String, String> = HashMap::new();
    for item in items {
        let (Some(typename), Some(id), Some(at)) = (
            item.typename.clone(),
            item.id.clone(),
            time(&item.created_at),
        ) else {
            bad += 1;
            continue;
        };
        let actor = actor_of(&item.actor);
        let mut meta = EventMeta::default();
        let ty = match typename.as_str() {
            "MergedEvent" => {
                saw_merged = true;
                b.push(
                    EventType::PrMerged,
                    EventId::pr_merged(&pr_id),
                    actor,
                    id,
                    at,
                    item.url.clone(),
                    meta,
                    Source::Graphql,
                );
                continue;
            }
            "ClosedEvent" => {
                meta.via_merge = Some(
                    merged_at
                        .is_some_and(|m| (at - m).num_seconds().abs() <= MERGE_CLOSE_WINDOW_SECS),
                );
                EventType::PrClosed
            }
            "ReopenedEvent" => EventType::PrReopened,
            "ConvertToDraftEvent" => EventType::PrConvertedToDraft,
            "ReadyForReviewEvent" => EventType::PrReadyForReview,
            "HeadRefForcePushedEvent" => {
                let oid = |o: &Option<Oid>| {
                    o.as_ref()
                        .and_then(|o| o.oid.clone())
                        .unwrap_or_else(|| "unknown".into())
                };
                meta.target = Some(format!(
                    "{}->{}",
                    oid(&item.before_commit),
                    oid(&item.after_commit)
                ));
                EventType::PrHeadForcePushed
            }
            "ReviewRequestedEvent" => {
                meta.target = Named::label(&item.requested_reviewer);
                EventType::ReviewRequested
            }
            "ReviewRequestRemovedEvent" => {
                meta.target = Named::label(&item.requested_reviewer);
                EventType::ReviewRequestRemoved
            }
            "AssignedEvent" => {
                meta.target = Named::label(&item.assignee);
                EventType::Assigned
            }
            "UnassignedEvent" => {
                meta.target = Named::label(&item.assignee);
                EventType::Unassigned
            }
            "LabeledEvent" => {
                meta.target = Named::label(&item.label);
                EventType::Labeled
            }
            "UnlabeledEvent" => {
                meta.target = Named::label(&item.label);
                EventType::Unlabeled
            }
            "MilestonedEvent" => {
                meta.target = item.milestone_title.clone();
                EventType::Milestoned
            }
            "DemilestonedEvent" => {
                meta.target = item.milestone_title.clone();
                EventType::Demilestoned
            }
            "RenamedTitleEvent" => {
                meta.previous_title = item.previous_title.clone();
                meta.target = item.current_title.clone();
                EventType::Renamed
            }
            "ReviewDismissedEvent" => {
                let review_id = item.review.as_ref().and_then(|r| r.id.clone());
                if let (Some(rid), Some(prev)) = (&review_id, &item.previous_review_state) {
                    dismissed_previous.insert(rid.clone(), prev.clone());
                }
                meta.review_id = review_id;
                meta.review_state = item
                    .previous_review_state
                    .as_deref()
                    .and_then(ReviewState::parse);
                EventType::ReviewDismissed
            }
            other => {
                *unsupported.entry(other.to_string()).or_default() += 1;
                continue;
            }
        };
        b.push(
            ty,
            EventId::timeline(&id),
            actor,
            id,
            at,
            item.url.clone(),
            meta,
            Source::Graphql,
        );
    }
    tracker.skipped(Collection::Timeline, "timeline items", bad);
    let mut unsupported: Vec<_> = unsupported.into_iter().collect();
    unsupported.sort();
    for (ty, n) in unsupported {
        tracker.note(format!(
            "skipped {n} timeline item(s) of unsupported type {ty}"
        ));
    }
    if !saw_merged && let Some(m) = merged_at {
        b.push(
            EventType::PrMerged,
            EventId::pr_merged(&pr_id),
            pr.merged_by
                .as_ref()
                .map(GqlActor::actor)
                .unwrap_or_else(Actor::unknown),
            pr_id.clone(),
            m,
            Some(pr_url.clone()),
            EventMeta::default(),
            Source::Graphql,
        );
        tracker.note("merge recorded from the pull request's merge time (no merge timeline item)");
    }

    // Reviews.
    let (review_nodes, bad) = parse_values::<ReviewNode>(reviews);
    tracker.skipped(Collection::Reviews, "reviews", bad);
    let mut bad = 0;
    let mut pending = 0;
    let mut review_comments: Vec<(String, Value)> = Vec::new();
    for r in review_nodes {
        let Some(id) = r.id.clone() else {
            bad += 1;
            continue;
        };
        let state_str = r.state.clone().unwrap_or_default();
        if state_str == "PENDING" {
            pending += 1;
            continue;
        }
        let Some(at) = time(&r.submitted_at) else {
            // Not submitted: a draft review.
            pending += 1;
            continue;
        };
        let mut state = ReviewState::parse(&state_str);
        if state == Some(ReviewState::Dismissed)
            && let Some(prev) = dismissed_previous
                .get(&id)
                .and_then(|p| ReviewState::parse(p))
                .filter(|p| !matches!(p, ReviewState::Dismissed | ReviewState::Pending))
        {
            state = Some(prev);
        }
        let actor = actor_of(&r.author);
        b.push(
            EventType::ReviewSubmitted,
            EventId::review(&id),
            actor.clone(),
            id.clone(),
            at,
            r.url.clone(),
            EventMeta {
                review_state: state,
                review_id: Some(id.clone()),
                ..EventMeta::default()
            },
            Source::Graphql,
        );
        if r.body.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            b.push(
                EventType::ReviewSummary,
                EventId::review_summary(&id),
                actor,
                id.clone(),
                at,
                r.url.clone(),
                EventMeta {
                    review_state: state,
                    review_id: Some(id.clone()),
                    ..EventMeta::default()
                },
                Source::Graphql,
            );
        }
        if let Some(c) = &r.comments {
            for v in c.nodes.iter().flatten() {
                review_comments.push((id.clone(), v.clone()));
            }
        }
    }
    tracker.skipped(Collection::Reviews, "reviews", bad);
    if pending > 0 {
        tracker.note(format!("{pending} pending (unsubmitted) review(s) ignored"));
    }

    // Review threads and inline comments (thread comments first so that
    // the copies listed under reviews are deduplicated against them).
    let mut out_threads: Vec<ReviewThread> = Vec::new();
    let mut bad = 0;
    for (th, comment_values, total) in threads {
        let total: Option<u64> = total;
        let thread_id = th.id.clone().unwrap_or_default();
        let (comment_nodes, bad_c) = parse_values::<CommentNode>(comment_values);
        bad += bad_c;
        let mut first_author = None;
        let mut fetched = 0u32;
        let mut pending = 0u32;
        for c in comment_nodes {
            fetched += 1;
            match inline_comment(&mut b, &c, Some(&thread_id), None) {
                CommentOutcome::Added => {
                    if first_author.is_none() {
                        first_author = Some(actor_of(&c.author));
                    }
                }
                // Unsubmitted comments are visible only to their author's
                // token and are not part of the thread yet.
                CommentOutcome::Pending => pending += 1,
                CommentOutcome::Bad => bad += 1,
            }
        }
        let fetched_all = total.is_none_or(|n| n <= u64::from(fetched));
        if fetched_all && pending == fetched {
            // A thread made only of unsubmitted comments does not exist for
            // anyone else.
            continue;
        }
        out_threads.push(ReviewThread {
            id: thread_id,
            pr_id: pr_id.clone(),
            is_resolved: th.is_resolved.unwrap_or(false),
            is_outdated: th.is_outdated.unwrap_or(false),
            resolved_by: th.resolved_by.as_ref().map(GqlActor::actor),
            path: th.path.clone(),
            comment_count: total
                .map(|n| n as u32)
                .unwrap_or(fetched)
                .saturating_sub(pending),
            first_comment_author: first_author,
        });
    }
    for (review_id, v) in review_comments {
        match serde_json::from_value::<CommentNode>(v) {
            Ok(c) => {
                if let CommentOutcome::Bad = inline_comment(&mut b, &c, None, Some(&review_id)) {
                    bad += 1;
                }
            }
            Err(_) => bad += 1,
        }
    }
    tracker.skipped(Collection::ReviewComments, "inline comments", bad);

    // Conversation comments.
    let (comment_nodes, bad) = parse_values::<CommentNode>(comments);
    tracker.skipped(Collection::Comments, "comments", bad);
    let mut bad = 0;
    for c in comment_nodes {
        let (Some(id), Some(at)) = (c.id.clone(), time(&c.created_at)) else {
            bad += 1;
            continue;
        };
        b.push(
            EventType::ConversationComment,
            EventId::comment(&id),
            actor_of(&c.author),
            id,
            at,
            c.url.clone(),
            EventMeta {
                edited_at: time(&c.last_edited_at),
                ..EventMeta::default()
            },
            Source::Graphql,
        );
    }
    tracker.skipped(Collection::Comments, "comments", bad);

    // Checks.
    tracker.skipped(Collection::Checks, "check runs", checks.bad_runs);
    let mut bad = 0;
    for r in checks.runs {
        let Some(node_id) = r.node_id.clone() else {
            bad += 1;
            continue;
        };
        let at = r
            .completed_at
            .as_deref()
            .and_then(parse_time)
            .or_else(|| r.started_at.as_deref().and_then(parse_time));
        // Queued runs have no time yet; they are not events.
        let Some(at) = at else { continue };
        let actor = r
            .app
            .map(|(id, slug)| Actor::new(id, slug, Some("Bot")))
            .unwrap_or_else(Actor::unknown);
        b.push(
            EventType::CheckRun,
            EventId::check_run(&node_id),
            actor,
            node_id,
            at,
            r.url,
            EventMeta {
                sha: head_sha.clone(),
                check_name: r.name,
                check_outcome: Some(CheckOutcome::from_github(
                    r.status.as_deref(),
                    r.conclusion.as_deref(),
                )),
                check_suite_id: r.suite_id.map(|id| id.to_string()),
                ..EventMeta::default()
            },
            r.source,
        );
    }
    let (statuses, bad_s) = parse_values::<RestStatus>(checks.statuses);
    bad += bad_s;
    for s in statuses {
        let (Some(node_id), Some(at)) = (
            s.node_id.clone(),
            s.created_at.as_deref().and_then(parse_time),
        ) else {
            bad += 1;
            continue;
        };
        b.push(
            EventType::CommitStatus,
            EventId::commit_status(&node_id),
            s.creator
                .as_ref()
                .map(RestUser::actor)
                .unwrap_or_else(Actor::unknown),
            node_id,
            at,
            None,
            EventMeta {
                sha: head_sha.clone(),
                check_name: s.context.clone(),
                check_outcome: Some(CheckOutcome::from_github(None, s.state.as_deref())),
                ..EventMeta::default()
            },
            Source::Rest,
        );
    }
    tracker.skipped(Collection::Checks, "checks", bad);

    // Latest CI state of the head commit.
    let latest_ci = head_commit
        .and_then(|c| c.status_check_rollup)
        .and_then(|r| r.state)
        .map(|s| CheckOutcome::from_github(None, Some(&s)));

    // Finalize coverage.
    let complete_collections: Vec<Collection> = Collection::ALL
        .iter()
        .copied()
        .filter(|c| !tracker.partial.contains(c))
        .collect();
    let status = if complete_collections.len() == Collection::ALL.len() {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    let mut events = dedupe(b.events);
    for e in &mut events {
        e.coverage = match Collection::of(e.event_type) {
            Some(c) if tracker.partial.contains(&c) => CoverageStatus::Partial,
            _ => CoverageStatus::Complete,
        };
    }
    events.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });

    let state = pr
        .state
        .as_deref()
        .and_then(PrState::parse)
        .unwrap_or(if merged_at.is_some() {
            PrState::Merged
        } else if pr.closed_at.is_some() {
            PrState::Closed
        } else {
            PrState::Open
        });
    let pull_request = PullRequest {
        id: pr_id.clone(),
        repository_id: repository.id.clone(),
        number: pr.number.unwrap_or(number),
        title: pr.title.clone().unwrap_or_default(),
        url: pr_url,
        author: actor_of(&pr.author),
        state,
        is_draft: pr.is_draft.unwrap_or(false),
        created_at,
        updated_at: time(&pr.updated_at),
        closed_at: time(&pr.closed_at),
        merged_at,
        merged_by: pr.merged_by.as_ref().map(GqlActor::actor),
        head_sha: head_sha.clone(),
        latest_ci,
        coverage: status,
        synced_at: Some(now),
    };
    Ok(PrBundle {
        repository,
        pull_request,
        events,
        threads: out_threads,
        current_commit_shas,
        complete_collections,
        status,
        notes: tracker.notes,
    })
}

enum CommentOutcome {
    Added,
    Pending,
    Bad,
}

fn inline_comment(
    b: &mut Builder,
    c: &CommentNode,
    thread_id: Option<&str>,
    review_id: Option<&str>,
) -> CommentOutcome {
    if c.state.as_deref() == Some("PENDING") {
        return CommentOutcome::Pending;
    }
    let (Some(id), Some(at)) = (c.id.clone(), time(&c.created_at)) else {
        return CommentOutcome::Bad;
    };
    let review_id = c
        .pull_request_review
        .as_ref()
        .and_then(|r| r.id.clone())
        .or_else(|| review_id.map(str::to_string));
    b.push(
        EventType::InlineComment,
        EventId::comment(&id),
        actor_of(&c.author),
        id,
        at,
        c.url.clone(),
        EventMeta {
            thread_id: thread_id.map(str::to_string),
            review_id,
            reply_to: c.reply_to.as_ref().and_then(|r| r.id.clone()),
            edited_at: time(&c.last_edited_at),
            ..EventMeta::default()
        },
        Source::Graphql,
    );
    CommentOutcome::Added
}

fn parse_values<T: serde::de::DeserializeOwned>(values: Vec<Value>) -> (Vec<T>, usize) {
    Connection {
        page_info: None,
        nodes: Some(values),
        total_count: None,
    }
    .parse_nodes()
}

/// Keeps the first event of each id.
fn dedupe(events: Vec<ActivityEvent>) -> Vec<ActivityEvent> {
    let mut seen = HashSet::new();
    events
        .into_iter()
        .filter(|e| seen.insert(e.event_id.clone()))
        .collect()
}

struct Builder {
    repo_id: String,
    pr_id: String,
    now: DateTime<Utc>,
    events: Vec<ActivityEvent>,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        event_type: EventType,
        event_id: EventId,
        actor: Actor,
        entity_id: String,
        occurred_at: DateTime<Utc>,
        url: Option<String>,
        meta: EventMeta,
        source: Source,
    ) {
        self.events.push(ActivityEvent {
            event_id,
            event_type,
            actor,
            repository_id: self.repo_id.clone(),
            entity_id,
            parent_pr_id: Some(self.pr_id.clone()),
            occurred_at,
            observed_at: self.now,
            source,
            url,
            meta,
            coverage: CoverageStatus::Complete,
        });
    }
}
