//! Core data model and aggregation for ghusage.

pub mod commit;
pub mod coverage;
pub mod metrics;
pub mod model;
pub mod sanitize;
pub mod time;

// Temporary migration seam for the chrono/chrono-tz replacement. This module
// is intentionally separate until the public model can move to Jiff types.
#[cfg(test)]
mod jiff_time;
