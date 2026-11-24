//! Deep application interfaces for commands that operate on credentials or
//! the local cache. Presentation crates receive owned results only.

mod auth;
mod cache;
mod doctor;
mod open_cache;

pub use auth::{AuthStatus, RateLimit, auth_status};
pub use cache::{CacheStatus, LastSync, cache_status, clear_cache};
pub use doctor::{DoctorCheck, DoctorReport, DoctorState, doctor};
