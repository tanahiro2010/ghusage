//! Application errors and their process exit codes.

use ghusage_github::{ApiError, AuthError};
use ghusage_storage::StorageError;

/// Process exit codes.
pub mod exit {
    pub const SUCCESS: i32 = 0;
    pub const GENERAL: i32 = 1;
    pub const INVALID_ARGS: i32 = 2;
    pub const AUTH: i32 = 3;
    pub const API: i32 = 4;
    pub const CACHE: i32 = 5;
    pub const INCOMPLETE: i32 = 6;
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidArgs(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("authentication failed: {0}")]
    AuthRejected(ApiError),
    #[error("GitHub API error: {0}")]
    Api(ApiError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("data is incomplete for the requested period: {0}")]
    Incomplete(String),
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
    pub fn exit_code(&self) -> i32 {
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

pub type Result<T> = std::result::Result<T, AppError>;
