//! Pull request URL parsing.

/// Parses `https://github.com/{owner}/{repo}/pull/{number}`, optionally
/// followed by more path segments (e.g. `/files`), a query or a fragment.
///
/// Only the `https` scheme and the `github.com` host (case-insensitive) are
/// accepted. URLs with credentials, a port, another host, a non-numeric or
/// zero number, or a path other than `/pull/` (e.g. `/issues/`, `/pulls/`)
/// are rejected.
pub fn parse_pr_url(url: &str) -> Option<(String, String, u64)> {
    let url = url.trim();
    let scheme_end = url.find("://")?;
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return None;
    }
    let rest = &url[scheme_end + 3..];
    let rest = rest.split(['#', '?']).next()?;
    let i = rest.find('/')?;
    let (host, path) = (&rest[..i], &rest[i + 1..]);
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next()? != "pull" {
        return None;
    }
    let number = segments.next()?;
    if !valid_owner(owner) || !valid_repo(repo) {
        return None;
    }
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u64 = number.parse().ok().filter(|n| *n > 0)?;
    Some((owner.to_string(), repo.to_string(), number))
}

/// Parses a commit reference: `owner/repo@sha`,
/// `https://github.com/{owner}/{repo}/commit/{sha}` or
/// `https://github.com/{owner}/{repo}/pull/{n}/commits/{sha}` (optionally
/// followed by a query or a fragment). The SHA must be 7 to 40 hexadecimal
/// digits and is returned in lower case.
pub fn parse_commit_ref(input: &str) -> Option<(String, String, String)> {
    let input = input.trim();
    let (owner, repo, sha) = if let Some(scheme_end) = input.find("://") {
        if !input[..scheme_end].eq_ignore_ascii_case("https") {
            return None;
        }
        let rest = &input[scheme_end + 3..];
        let rest = rest.split(['#', '?']).next()?;
        let i = rest.find('/')?;
        let (host, path) = (&rest[..i], &rest[i + 1..]);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        match segments.as_slice() {
            [owner, repo, "commit", sha] => (*owner, *repo, *sha),
            [owner, repo, "pull", number, "commits", sha]
                if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
            {
                (*owner, *repo, *sha)
            }
            _ => return None,
        }
    } else {
        let (name, sha) = input.split_once('@')?;
        let (owner, repo) = name.split_once('/')?;
        (owner, repo, sha)
    };
    if !valid_owner(owner) || !valid_repo(repo) || !valid_sha(sha) {
        return None;
    }
    Some((
        owner.to_string(),
        repo.to_string(),
        sha.to_ascii_lowercase(),
    ))
}

fn valid_sha(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_owner(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_repo(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(owner: &str, repo: &str, n: u64) -> Option<(String, String, u64)> {
        Some((owner.into(), repo.into(), n))
    }

    #[test]
    fn accepts_pull_request_urls() {
        assert_eq!(
            parse_pr_url("https://github.com/o/r/pull/123"),
            ok("o", "r", 123)
        );
        assert_eq!(
            parse_pr_url("https://github.com/o/r/pull/123/"),
            ok("o", "r", 123)
        );
        assert_eq!(
            parse_pr_url("https://github.com/o/r.rs/pull/123/files"),
            ok("o", "r.rs", 123)
        );
        assert_eq!(
            parse_pr_url("https://github.com/o/r/pull/7?diff=split"),
            ok("o", "r", 7)
        );
        assert_eq!(
            parse_pr_url("https://GitHub.com/o-x/r_y/pull/7#discussion_r1"),
            ok("o-x", "r_y", 7)
        );
        assert_eq!(
            parse_pr_url("  https://github.com/o/r/pull/1  "),
            ok("o", "r", 1)
        );
    }

    fn commit(owner: &str, repo: &str, sha: &str) -> Option<(String, String, String)> {
        Some((owner.into(), repo.into(), sha.into()))
    }

    #[test]
    fn accepts_commit_references() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            parse_commit_ref(&format!("https://github.com/o/r/commit/{full}")),
            commit("o", "r", full)
        );
        assert_eq!(
            parse_commit_ref(&format!(
                "https://github.com/o/r.rs/commit/{full}/?diff=split#x"
            )),
            commit("o", "r.rs", full)
        );
        assert_eq!(
            parse_commit_ref("https://github.com/o/r/pull/12/commits/ABCDEF0"),
            commit("o", "r", "abcdef0")
        );
        assert_eq!(
            parse_commit_ref("  o-x/r_y@abcdef01  "),
            commit("o-x", "r_y", "abcdef01")
        );
    }

    #[test]
    fn rejects_other_commit_references() {
        for bad in [
            "o/r@abc",
            "o/r@xyz1234",
            "o/r@0123456789abcdef0123456789abcdef012345678",
            "o@abcdef0",
            "/r@abcdef0",
            "o/../r@abcdef0",
            "http://github.com/o/r/commit/abcdef0",
            "https://evil.com/o/r/commit/abcdef0",
            "https://github.com:443/o/r/commit/abcdef0",
            "https://github.com/o/r/commits/abcdef0",
            "https://github.com/o/r/pull/x/commits/abcdef0",
            "https://github.com/o/r/commit/abcdef0/extra",
            "https://github.com/o/r/commit/",
            "abcdef0",
            "",
        ] {
            assert_eq!(parse_commit_ref(bad), None, "{bad}");
        }
    }

    #[test]
    fn rejects_other_urls() {
        for bad in [
            "http://github.com/o/r/pull/1",
            "https://gitlab.com/o/r/pull/1",
            "https://github.com.evil.com/o/r/pull/1",
            "https://user@github.com/o/r/pull/1",
            "https://github.com:443/o/r/pull/1",
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/pulls/1",
            "https://github.com/o/r/pull/abc",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pull/0",
            "https://github.com/o/r/pull/-1",
            "https://github.com/o/r/pull/1e3",
            "https://github.com/o/../pull/1",
            "https://github.com/o/r",
            "github.com/o/r/pull/1",
            "javascript://github.com/o/r/pull/1",
            "",
        ] {
            assert_eq!(parse_pr_url(bad), None, "{bad}");
        }
    }
}
