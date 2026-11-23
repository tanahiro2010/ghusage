//! Pull request fetching and normalization. Everything comes from GraphQL
//! except commit statuses (and check runs that do not fit in the main
//! query), which come from REST.

use std::collections::{BTreeSet, HashMap, HashSet};

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::metrics::{LineCounts, PrDiff};
use ghusage_core::model::{
    ActivityEvent, Actor, CheckOutcome, Collection, EventId, EventMeta, EventType, PrState,
    PullRequest, ReviewState, ReviewThread, Source,
};
use jiff::Timestamp;
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
    additions: Option<u64>,
    deletions: Option<u64>,
    changed_files: Option<u64>,
    merge_commit: Option<MergeCommit>,
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
struct MergeCommit {
    parents: Option<Count>,
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
    additions: Option<u64>,
    deletions: Option<u64>,
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
    fn user_actor(a: &Option<Self>) -> Option<Actor> {
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
    fn label(n: &Option<Self>) -> Option<String> {
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
    pub(crate) fn actor(&self) -> Actor {
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

#[derive(Debug, Default)]
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
                    tracker.partial(Collection::Checks, format!("check runs not fetched: {e}"));
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

include!("pull_request/fetch.rs");

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
    now: Timestamp,
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
        occurred_at: Timestamp,
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
