//! Aggregation engine shared by the CLI, the TUI and every export format.

use jiff::{SignedDuration, Timestamp, civil::Date};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::coverage::CoverageRecord;
use crate::model::{
    ActivityEvent, Actor, CheckOutcome, EventCategory, EventType, Issue, PrState, PullRequest,
    Repository, ReviewState, ReviewThread, Subject, Visibility,
};
use crate::time::{Bucket, Granularity, Period, Zone, bucket_key, buckets};

include!("metrics/dataset.rs");
include!("metrics/filter.rs");
include!("metrics/summary.rs");
include!("metrics/rows.rs");
include!("metrics/timing.rs");
include!("metrics/details.rs");
