//! Credential resolution.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A token that never appears in Debug/Display output.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wraps a token so formatting never reveals its value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// The raw value. Only for building the Authorization header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// Where the token came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Token was read from an explicit token file.
    TokenFile(PathBuf),
    /// Token came from the `GH_TOKEN` environment variable.
    GhTokenEnv,
    /// Token came from the `GITHUB_TOKEN` environment variable.
    GithubTokenEnv,
    /// Token came from `gh auth token`.
    GhCli,
}

impl TokenSource {
    /// Returns a safe description of the credential source.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::TokenFile(p) => format!("token file {}", p.display()),
            Self::GhTokenEnv => "GH_TOKEN".into(),
            Self::GithubTokenEnv => "GITHUB_TOKEN".into(),
            Self::GhCli => "gh auth token".into(),
        }
    }
}

/// A GitHub token and the source from which it was resolved.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Secret token used for authorization.
    pub token: Secret,
    /// Source selected during credential resolution.
    pub source: TokenSource,
}

/// Failures encountered while locating or reading GitHub credentials.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No supported source supplied a token.
    #[error(
        "no GitHub credentials found: set GH_TOKEN or GITHUB_TOKEN, log in with `gh auth login`, or pass --token-file"
    )]
    NotFound,
    #[error("cannot read token file {path}: {message}")]
    /// An explicitly selected token file could not be read.
    TokenFile {
        /// Path to the selected token file.
        path: PathBuf,
        /// Underlying read error.
        message: String,
    },
    /// Token file permissions allow access by other users.
    #[error("token file {0} must not be readable by other users (use chmod 600)")]
    InsecureTokenFile(PathBuf),
}

/// Resolves credentials from an explicit token file or a supported source.
///
/// Sources are checked in order: `token_file`, `GH_TOKEN`, `GITHUB_TOKEN`,
/// then `gh auth token`. The environment and command are injectable for tests.
///
/// An explicitly given token file is never skipped: if it cannot be read,
/// is accessible by group or other users (Unix), or is empty, an error is
/// returned instead of silently falling back to another source.
pub fn resolve_credentials(
    token_file: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    gh_token: &dyn Fn() -> Option<String>,
) -> Result<Credentials, AuthError> {
    if let Some(path) = token_file {
        let token = read_token_file(path)?;
        return Ok(Credentials {
            token: Secret::new(token),
            source: TokenSource::TokenFile(path.to_path_buf()),
        });
    }
    for (var, source) in [
        ("GH_TOKEN", TokenSource::GhTokenEnv),
        ("GITHUB_TOKEN", TokenSource::GithubTokenEnv),
    ] {
        if let Some(v) = env(var).map(|v| v.trim().to_string())
            && !v.is_empty()
        {
            return Ok(Credentials {
                token: Secret::new(v),
                source,
            });
        }
    }
    if let Some(v) = gh_token().map(|v| v.trim().to_string())
        && !v.is_empty()
    {
        return Ok(Credentials {
            token: Secret::new(v),
            source: TokenSource::GhCli,
        });
    }
    Err(AuthError::NotFound)
}

fn read_token_file(path: &Path) -> Result<String, AuthError> {
    let file_err = |message: String| AuthError::TokenFile {
        path: path.to_path_buf(),
        message,
    };
    let meta = std::fs::metadata(path).map_err(|e| file_err(io_reason(&e)))?;
    if !meta.is_file() {
        return Err(file_err("not a regular file".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(AuthError::InsecureTokenFile(path.to_path_buf()));
        }
    }
    // Token files are tiny; refuse anything unreasonably large.
    if meta.len() > 64 * 1024 {
        return Err(file_err("file is too large to be a token".into()));
    }
    let content = std::fs::read_to_string(path).map_err(|e| file_err(io_reason(&e)))?;
    let token = content.trim();
    if token.is_empty() {
        return Err(file_err("file is empty".into()));
    }
    if token.contains(char::is_whitespace) {
        return Err(file_err("file must contain a single token".into()));
    }
    Ok(token.to_string())
}

/// Describes an I/O error without echoing file contents.
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "file not found".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        std::io::ErrorKind::InvalidData => "file is not valid UTF-8".into(),
        _ => "I/O error".into(),
    }
}

/// Runs `gh auth token` (without a shell). Returns `None` if gh is missing
/// or not logged in.
pub fn gh_cli_token() -> Option<String> {
    let output = Command::new("gh")
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let token = token.trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    Some(token.to_string())
}
