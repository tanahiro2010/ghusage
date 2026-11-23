//! Issue fetching and normalization.

use std::collections::HashSet;

use ghusage_core::coverage::CoverageStatus;
use ghusage_core::model::{ActivityEvent, EventId, EventMeta, EventType, Issue, Source};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::gql::{self, Connection, GqlActor, GqlRepository, actor_of, time};
use crate::http::Transport;
use crate::pull_request::TimelineNode;
use crate::queries;
use crate::{ApiError, IssueBundle};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct GqlIssue {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    id: Option<String>,
    number: Option<u64>,
    title: Option<String>,
    url: Option<String>,
    state: Option<String>,
    created_at: Option<String>,
    closed_at: Option<String>,
    author: Option<GqlActor>,
    timeline_items: Option<Connection>,
}

pub(crate) async fn fetch(
    t: &Transport,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<IssueBundle, ApiError> {
    let what = format!("issue {owner}/{name}#{number}");
    let resp = t
        .graphql(
            &queries::issue(),
            json!({ "owner": owner, "name": name, "number": number }),
        )
        .await?;
    let Some(repo_value) = resp
        .data
        .as_ref()
        .and_then(|d| d.get("repository"))
        .filter(|v| !v.is_null())
    else {
        return Err(gql::failure(
            &resp.errors,
            &format!("repository {owner}/{name}"),
        ));
    };
    let Some(issue_value) = repo_value
        .get("issueOrPullRequest")
        .filter(|v| !v.is_null())
    else {
        return Err(gql::failure(&resp.errors, &what));
    };
    let repository = gql::from_value::<GqlRepository>(Some(repo_value)).to_repository()?;
    let mut issue: GqlIssue = serde_json::from_value(issue_value.clone())
        .map_err(|e| ApiError::InvalidResponse(format!("{what}: {e}")))?;
    match issue.typename.as_deref() {
        Some("Issue") => {}
        Some("PullRequest") => {
            return Err(ApiError::InvalidResponse(format!(
                "{owner}/{name}#{number} is a pull request, not an issue"
            )));
        }
        _ => {
            return Err(ApiError::InvalidResponse(format!(
                "{what}: unexpected object type"
            )));
        }
    }
    let missing = |f: &str| ApiError::InvalidResponse(format!("{what} is missing `{f}`"));
    let id = issue.id.clone().ok_or_else(|| missing("id"))?;
    let created_at = time(&issue.created_at).ok_or_else(|| missing("createdAt"))?;
    let url = issue.url.clone().ok_or_else(|| missing("url"))?;

    let mut complete = resp.errors.is_empty();
    // Timeline pages.
    let mut nodes = Vec::new();
    match issue.timeline_items.take() {
        None => complete = false,
        Some(mut conn) => {
            let mut cursors = HashSet::new();
            loop {
                nodes.extend(conn.nodes.take().unwrap_or_default());
                let cursor = match conn.next_cursor() {
                    Ok(None) => break,
                    Ok(Some(c)) if cursors.insert(c.clone()) => c,
                    _ => {
                        complete = false;
                        break;
                    }
                };
                let resp = match t
                    .graphql(
                        &queries::issue_timeline_page(),
                        json!({ "id": id, "after": cursor }),
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(e @ (ApiError::Unauthorized | ApiError::RateLimited { .. })) => {
                        return Err(e);
                    }
                    Err(_) => {
                        complete = false;
                        break;
                    }
                };
                if !resp.errors.is_empty() {
                    complete = false;
                }
                match resp
                    .data
                    .as_ref()
                    .and_then(|d| d.get("node"))
                    .and_then(|n| n.get("timelineItems"))
                    .filter(|v| !v.is_null())
                {
                    Some(v) => conn = gql::from_value(Some(v)),
                    None => {
                        complete = false;
                        break;
                    }
                }
            }
        }
    }

    let now = Timestamp::now();
    let mut events = vec![ActivityEvent {
        event_id: EventId::issue_created(&id),
        event_type: EventType::IssueOpened,
        actor: actor_of(&issue.author),
        repository_id: repository.id.clone(),
        entity_id: id.clone(),
        parent_pr_id: None,
        occurred_at: created_at,
        observed_at: now,
        source: Source::Graphql,
        url: Some(url.clone()),
        meta: EventMeta::default(),
        coverage: CoverageStatus::Complete,
    }];
    let parsed = Connection {
        page_info: None,
        nodes: Some(nodes),
        total_count: None,
    }
    .parse_nodes::<TimelineNode>();
    if parsed.1 > 0 {
        complete = false;
    }
    for item in parsed.0 {
        let (Some(ty), Some(node_id), Some(at)) = (
            item.typename.as_deref(),
            item.id.clone(),
            time(&item.created_at),
        ) else {
            complete = false;
            continue;
        };
        let event_type = match ty {
            "ClosedEvent" => EventType::IssueClosed,
            "ReopenedEvent" => EventType::IssueReopened,
            _ => continue,
        };
        events.push(ActivityEvent {
            event_id: EventId::timeline(&node_id),
            event_type,
            actor: actor_of(&item.actor),
            repository_id: repository.id.clone(),
            // The entity is the issue; the timeline node id is in event_id.
            entity_id: id.clone(),
            parent_pr_id: None,
            occurred_at: at,
            observed_at: now,
            source: Source::Graphql,
            url: item.url.clone().or_else(|| Some(url.clone())),
            meta: EventMeta::default(),
            coverage: CoverageStatus::Complete,
        });
    }
    let status = if complete {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    let mut seen = HashSet::new();
    events.retain(|e| seen.insert(e.event_id.clone()));
    for e in &mut events {
        e.coverage = if e.event_type == EventType::IssueOpened {
            CoverageStatus::Complete
        } else {
            status
        };
    }
    events.sort_by(|a, b| {
        (a.occurred_at, a.event_type.sort_rank(), &a.event_id).cmp(&(
            b.occurred_at,
            b.event_type.sort_rank(),
            &b.event_id,
        ))
    });
    let closed_at = time(&issue.closed_at);
    let is_open = match issue.state.as_deref() {
        Some("OPEN") => true,
        Some("CLOSED") => false,
        _ => closed_at.is_none(),
    };
    let repository_id = repository.id.clone();
    Ok(IssueBundle {
        repository,
        issue: Issue {
            id: id.clone(),
            repository_id,
            number: issue.number.unwrap_or(number),
            title: issue.title.clone().unwrap_or_default(),
            url,
            author: actor_of(&issue.author),
            is_open,
            created_at,
            closed_at,
        },
        events,
        status,
    })
}
