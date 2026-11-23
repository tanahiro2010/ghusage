//! Conversions between model values and SQLite column values.
//!
//! Timestamps are stored as UTC epoch seconds (`INTEGER`) plus a companion
//! `*_ns` column holding the sub-second nanoseconds, so values round-trip
//! exactly while the seconds column stays directly comparable and indexable.

use ghusage_core::model::{Actor, ActorKind};
use jiff::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Result, StorageError};

pub(crate) fn secs(dt: Timestamp) -> i64 {
    dt.as_second()
}

pub(crate) fn nanos(dt: Timestamp) -> i64 {
    i64::from(dt.subsec_nanosecond())
}

pub(crate) fn opt_secs(dt: Option<Timestamp>) -> Option<i64> {
    dt.map(secs)
}

pub(crate) fn opt_nanos(dt: Option<Timestamp>) -> Option<i64> {
    dt.map(nanos)
}

pub(crate) fn time(s: i64, ns: i64) -> Result<Timestamp> {
    let nanos = i128::from(s) * 1_000_000_000 + i128::from(ns);
    Timestamp::from_nanosecond(nanos)
        .map_err(|_| StorageError::Data(format!("invalid stored timestamp {s}.{ns}")))
}

pub(crate) fn opt_time(s: Option<i64>, ns: Option<i64>) -> Result<Option<Timestamp>> {
    s.map(|s| time(s, ns.unwrap_or(0))).transpose()
}

/// String form of a unit enum that serializes as a JSON string.
pub(crate) fn enum_str<T: Serialize>(v: &T) -> Result<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        Ok(other) => Err(StorageError::Data(format!(
            "expected a string value, got {other}"
        ))),
        Err(e) => Err(StorageError::Data(e.to_string())),
    }
}

pub(crate) fn parse_enum<T: DeserializeOwned>(what: &str, s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|_| StorageError::Data(format!("unknown stored {what} value {s:?}")))
}

pub(crate) fn to_json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| StorageError::Data(e.to_string()))
}

pub(crate) fn from_json<T: DeserializeOwned>(what: &str, s: &str) -> Result<T> {
    serde_json::from_str(s).map_err(|e| StorageError::Data(format!("invalid stored {what}: {e}")))
}

pub(crate) fn opt_to_json<T: Serialize>(v: Option<&T>) -> Result<Option<String>> {
    v.map(to_json).transpose()
}

pub(crate) fn opt_from_json<T: DeserializeOwned>(
    what: &str,
    s: Option<String>,
) -> Result<Option<T>> {
    s.map(|s| from_json(what, &s)).transpose()
}

pub(crate) fn actor_kind_str(k: ActorKind) -> Result<String> {
    enum_str(&k)
}

pub(crate) fn actor(id: Option<String>, login: Option<String>, kind: &str) -> Result<Actor> {
    Ok(Actor {
        id,
        login,
        kind: parse_enum("actor kind", kind)?,
    })
}
