//! Lenient typed views of GraphQL responses. Every field is optional so
//! that nulls and missing fields never abort parsing; required values are
//! checked where they are used.

use ghusage_core::model::{Actor, Repository, Visibility};
use jiff::Timestamp;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::ApiError;
use crate::http::{GraphqlError, parse_time};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct GqlActor {
    #[serde(rename = "__typename")]
    pub typename: Option<String>,
    pub login: Option<String>,
    pub id: Option<String>,
}

impl GqlActor {
    pub fn actor(&self) -> Actor {
        Actor::new(
            self.id.clone(),
            self.login.clone(),
            self.typename.as_deref(),
        )
    }
}

pub(crate) fn actor_of(a: &Option<GqlActor>) -> Actor {
    a.as_ref().map_or_else(Actor::unknown, GqlActor::actor)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// A connection whose nodes are parsed individually by the caller.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Connection {
    pub page_info: Option<PageInfo>,
    pub nodes: Option<Vec<Value>>,
    pub total_count: Option<u64>,
}

impl Connection {
    /// Parses each non-null node; returns the parsed nodes and the number of
    /// nodes that could not be parsed.
    pub fn parse_nodes<T: DeserializeOwned>(&self) -> (Vec<T>, usize) {
        let mut out = Vec::new();
        let mut bad = 0;
        for n in self.nodes.iter().flatten() {
            if n.is_null() {
                bad += 1;
                continue;
            }
            match serde_json::from_value::<T>(n.clone()) {
                Ok(v) => out.push(v),
                Err(_) => bad += 1,
            }
        }
        (out, bad)
    }

    /// Cursor of the next page, `Ok(None)` when this was the last page and
    /// `Err(())` when more pages exist but no cursor was given.
    pub fn next_cursor(&self) -> Result<Option<String>, ()> {
        match &self.page_info {
            Some(p) if p.has_next_page => p.end_cursor.clone().map(Some).ok_or(()),
            Some(_) => Ok(None),
            None => Err(()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct IdOnly {
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct GqlRepository {
    pub id: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub visibility: Option<String>,
    pub is_fork: Option<bool>,
    pub viewer_permission: Option<String>,
    pub owner: Option<GqlActor>,
}

impl GqlRepository {
    pub fn to_repository(&self) -> Result<Repository, ApiError> {
        let missing = |f: &str| ApiError::InvalidResponse(format!("repository is missing `{f}`"));
        let id = self.id.clone().ok_or_else(|| missing("id"))?;
        let name = self.name.clone().ok_or_else(|| missing("name"))?;
        let owner = self.owner.clone().unwrap_or_default();
        let owner_login = owner.login.clone().ok_or_else(|| missing("owner"))?;
        Ok(Repository {
            id,
            owner: owner_login,
            name,
            owner_kind: owner.actor().kind,
            visibility: self
                .visibility
                .as_deref()
                .map_or(Visibility::Unknown, Visibility::parse),
            is_fork: self.is_fork.unwrap_or(false),
            viewer_permission: self.viewer_permission.clone(),
            accessible: true,
            url: self.url.clone(),
        })
    }
}

/// Parses an optional GraphQL timestamp.
pub(crate) fn time(s: &Option<String>) -> Option<Timestamp> {
    s.as_deref().and_then(parse_time)
}

/// Maps the errors of a GraphQL response that returned no usable data.
pub(crate) fn failure(errors: &[GraphqlError], what: &str) -> ApiError {
    for e in errors {
        match e.kind.as_deref() {
            Some("NOT_FOUND") => return ApiError::NotFound(what.to_string()),
            Some("FORBIDDEN") => return ApiError::Forbidden(format!("{what}: {}", e.message())),
            Some("RATE_LIMITED") => return ApiError::RateLimited { reset_at: None },
            _ => {}
        }
    }
    errors.first().map_or_else(
        || ApiError::InvalidResponse(format!("GraphQL response for {what} has no data")),
        |e| ApiError::Graphql(e.message()),
    )
}

/// Short description of GraphQL errors for coverage notes.
pub(crate) fn describe(errors: &[GraphqlError]) -> String {
    let mut parts: Vec<String> = errors
        .iter()
        .take(3)
        .map(super::http::GraphqlError::message)
        .collect();
    if errors.len() > 3 {
        parts.push(format!("and {} more", errors.len() - 3));
    }
    parts.join("; ")
}

/// Deserializes a value leniently into `T`.
pub(crate) fn from_value<T: DeserializeOwned + Default>(v: Option<&Value>) -> T {
    v.and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}
