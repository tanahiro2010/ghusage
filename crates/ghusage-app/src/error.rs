//! Application errors and their process exit codes.

use ghusage_github::{ApiError, AuthError};
use ghusage_storage::StorageError;

/// Process exit codes.
pub mod exit {
    /// Command completed successfully.
    pub const SUCCESS: i32 = 0;
    /// Unclassified runtime error.
    pub const GENERAL: i32 = 1;
    /// Invalid command line arguments.
    pub const INVALID_ARGS: i32 = 2;
    /// Authentication failed.
    pub const AUTH: i32 = 3;
    /// GitHub API request failed.
    pub const API: i32 = 4;
    /// Cache operation failed.
    pub const CACHE: i32 = 5;
    /// Requested data is incomplete.
    pub const INCOMPLETE: i32 = 6;
}

/// Application failures mapped to user-facing messages and exit codes.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Command arguments are invalid.
    #[error("{0}")]
    InvalidArgs(String),
    /// Credentials could not be resolved.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// GitHub rejected the supplied credentials.
    #[error("authentication failed: {0}")]
    AuthRejected(ApiError),
    /// GitHub API operation failed.
    #[error("GitHub API error: {0}")]
    Api(ApiError),
    /// Local cache operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Required data is incomplete for the requested report.
    #[error("data is incomplete for the requested period: {0}")]
    Incomplete(String),
    /// Another application error occurred.
    #[error("{0}")]
    Other(String),
}

impl From<ApiError> for AppError {
    fn from(e: ApiError) -> Self {
        if e.is_auth() {
            Self::AuthRejected(e)
        } else {
            Self::Api(e)
        }
    }
}

impl From<ghusage_core::time::TimeError> for AppError {
    fn from(e: ghusage_core::time::TimeError) -> Self {
        Self::InvalidArgs(e.to_string())
    }
}

impl AppError {
    /// Returns the process exit code associated with this error.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidArgs(_) => exit::INVALID_ARGS,
            Self::Auth(_) | Self::AuthRejected(_) => exit::AUTH,
            Self::Api(_) => exit::API,
            Self::Storage(_) => exit::CACHE,
            Self::Incomplete(_) => exit::INCOMPLETE,
            Self::Other(_) => exit::GENERAL,
        }
    }
}

/// Result type for application operations.
pub type Result<T> = std::result::Result<T, AppError>;
