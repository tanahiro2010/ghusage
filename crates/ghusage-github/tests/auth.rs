//! Credential resolution order, empty values and token file safety.

use std::collections::HashMap;
use std::io::Write;

use ghusage_github::{AuthError, Credentials, Secret, TokenSource, resolve_credentials};

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

fn resolve(
    file: Option<&std::path::Path>,
    env: &[(&str, &str)],
    gh: Option<&str>,
) -> Result<Credentials, AuthError> {
    let env = env_of(env);
    let gh = gh.map(str::to_string);
    resolve_credentials(file, &env, &move || gh.clone())
}

fn token_file(content: &str, mode: u32) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
    f
}

#[test]
fn gh_token_env_is_used() {
    let c = resolve(None, &[("GH_TOKEN", "from-gh-token")], Some("from-cli")).unwrap();
    assert_eq!(c.token.expose(), "from-gh-token");
    assert_eq!(c.source, TokenSource::GhTokenEnv);
}

#[test]
fn github_token_env_is_used_when_gh_token_is_absent() {
    let c = resolve(
        None,
        &[("GITHUB_TOKEN", "from-github-token")],
        Some("from-cli"),
    )
    .unwrap();
    assert_eq!(c.token.expose(), "from-github-token");
    assert_eq!(c.source, TokenSource::GithubTokenEnv);
}

#[test]
fn gh_token_takes_precedence_over_github_token() {
    let c = resolve(None, &[("GH_TOKEN", "a"), ("GITHUB_TOKEN", "b")], Some("c")).unwrap();
    assert_eq!(c.token.expose(), "a");
}

#[test]
fn gh_cli_token_is_the_last_resort() {
    let c = resolve(None, &[], Some("  from-cli\n")).unwrap();
    assert_eq!(c.token.expose(), "from-cli");
    assert_eq!(c.source, TokenSource::GhCli);
}

#[test]
fn empty_values_are_skipped() {
    let c = resolve(
        None,
        &[("GH_TOKEN", ""), ("GITHUB_TOKEN", "  \n")],
        Some("from-cli"),
    )
    .unwrap();
    assert_eq!(c.source, TokenSource::GhCli);
    let err = resolve(None, &[("GH_TOKEN", "")], Some("   ")).unwrap_err();
    assert!(matches!(err, AuthError::NotFound));
}

#[test]
fn no_credentials_is_reported() {
    let err = resolve(None, &[], None).unwrap_err();
    assert!(matches!(err, AuthError::NotFound));
    assert!(err.to_string().contains("GH_TOKEN"));
}

#[test]
fn token_file_takes_precedence_over_everything() {
    let f = token_file("file-token\n", 0o600);
    let c = resolve(Some(f.path()), &[("GH_TOKEN", "env")], Some("cli")).unwrap();
    assert_eq!(c.token.expose(), "file-token");
    assert_eq!(c.source, TokenSource::TokenFile(f.path().to_path_buf()));
}

#[cfg(unix)]
#[test]
fn token_file_readable_by_others_is_rejected() {
    for mode in [0o644, 0o640, 0o604, 0o660] {
        let f = token_file("file-token", mode);
        let err = resolve(Some(f.path()), &[("GH_TOKEN", "env")], None).unwrap_err();
        assert!(
            matches!(err, AuthError::InsecureTokenFile(_)),
            "mode {mode:o}: {err:?}"
        );
        assert!(!err.to_string().contains("file-token"));
    }
    let f = token_file("file-token", 0o400);
    assert!(resolve(Some(f.path()), &[], None).is_ok());
}

#[test]
fn missing_or_empty_token_file_is_an_error_not_a_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let err = resolve(Some(&missing), &[("GH_TOKEN", "env")], None).unwrap_err();
    assert!(matches!(err, AuthError::TokenFile { .. }), "{err:?}");

    let f = token_file("\n  \n", 0o600);
    let err = resolve(Some(f.path()), &[("GH_TOKEN", "env")], None).unwrap_err();
    assert!(matches!(err, AuthError::TokenFile { .. }), "{err:?}");
}

#[test]
fn secret_is_never_rendered() {
    let s = Secret::new("ghp_supersecret");
    assert_eq!(format!("{s}"), "***");
    assert_eq!(format!("{s:?}"), "Secret(***)");
    let c = Credentials {
        token: s,
        source: TokenSource::GhCli,
    };
    assert!(!format!("{c:?}").contains("supersecret"));
}
