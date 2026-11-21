//! Application services shared by the ghusage CLI and TUI: synchronization,
//! reports and export formats.

pub mod commit;
pub mod csv_out;
pub mod error;
pub mod paths;
pub mod report;
pub mod session;
pub mod sync;

pub use error::{AppError, Result, exit};
