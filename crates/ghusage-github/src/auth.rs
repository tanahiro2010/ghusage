//! Credential resolution.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A token that never appears in Debug/Display output.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// The raw value. Only for building the Authorization header.
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
    TokenFile(PathBuf),
    GhTokenEnv,
    GithubTokenEnv,
    GhCli,
}

impl TokenSource {
    pub fn describe(&self) -> String {
        match self {
            TokenSource::TokenFile(p) => format!("token file {}", p.display()),
            TokenSource::GhTokenEnv => "GH_TOKEN".into(),
            TokenSource::GithubTokenEnv => "GITHUB_TOKEN".into(),
            TokenSource::GhCli => "gh auth token".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub token: Secret,
    pub source: TokenSource,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(
        "no GitHub credentials found: set GH_TOKEN or GITHUB_TOKEN, log in with `gh auth login`, or pass --token-file"
    )]
    NotFound,
    #[error("cannot read token file {path}: {message}")]
    TokenFile { path: PathBuf, message: String },
    #[error("token file {0} must not be readable by other users (use chmod 600)")]
    InsecureTokenFile(PathBuf),
}

/// Resolves credentials in order: `token_file`, `GH_TOKEN`, `GITHUB_TOKEN`,
/// `gh auth token`. `env` reads environment variables and `gh_token` runs
/// `gh auth token`; both are injectable for tests. Empty values are skipped.
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
