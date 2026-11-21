//! Making untrusted GitHub content safe to display and open.

use unicode_width::UnicodeWidthStr;

/// Removes terminal control sequences and control characters from untrusted
/// text so it cannot move the cursor, change colors, set the window title or
/// reorder text. Newlines and tabs become spaces; the result is one line.
pub fn sanitize_line(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            // C1 control introducers (CSI, OSC, DCS, ...) used as single bytes.
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\n' | '\r' | '\t' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c if is_unsafe_char(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Like [`sanitize_line`] for one line of code or diff: tabs become four
/// spaces instead of one, so indentation keeps its shape, and a trailing
/// carriage return is dropped.
pub fn sanitize_code_line(input: &str) -> String {
    let line = input.strip_suffix('\r').unwrap_or(input);
    if line.contains('\t') {
        sanitize_line(&line.replace('\t', "    "))
    } else {
        sanitize_line(line)
    }
}

fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        // Bidirectional overrides/isolates can visually reorder text.
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}')
        // Line/paragraph separators break single-line layout.
        || matches!(c, '\u{2028}' | '\u{2029}')
        || c == '\u{feff}'
}

fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek() {
        Some('[') => {
            chars.next();
            skip_csi(chars);
        }
        Some(']' | 'P' | 'X' | '^' | '_') => {
            chars.next();
            skip_string(chars);
        }
        Some(_) => {
            // Two-character escape such as ESC c or ESC (B.
            let c = chars.next();
            if matches!(c, Some('(' | ')' | '*' | '+' | '#' | '%')) {
                chars.next();
            }
        }
        None => {}
    }
}

fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    // Parameters and intermediates, then one final byte in 0x40..=0x7e.
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            break;
        }
        if !(('\u{20}'..='\u{3f}').contains(&c)) {
            break;
        }
    }
}

fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    // Terminated by BEL, ST (ESC \) or C1 ST.
    while let Some(c) = chars.next() {
        match c {
            '\u{7}' | '\u{9c}' => break,
            '\u{1b}' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                break;
            }
            _ => {}
        }
    }
}

/// Sanitizes and truncates to a display width, appending `…` when cut.
pub fn truncate_width(input: &str, max_width: usize) -> String {
    let s = sanitize_line(input);
    if s.width() <= max_width {
        return s;
    }
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max_width - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Display width of sanitized text.
pub fn display_width(s: &str) -> usize {
    s.width()
}

/// Validates a URL before opening it in a browser: only `https` URLs on
/// GitHub hosts are allowed.
pub fn validate_github_url(url: &str, allowed_hosts: &[&str]) -> Result<String, UrlError> {
    let trimmed = url.trim();
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(UrlError::InvalidCharacters);
    }
    let rest = trimmed
        .strip_prefix("https://")
        .ok_or(UrlError::SchemeNotAllowed)?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];
    if authority.contains('@') {
        return Err(UrlError::HostNotAllowed);
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(UrlError::HostNotAllowed);
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("only https URLs can be opened")]
    SchemeNotAllowed,
    #[error("URL host is not a GitHub host")]
    HostNotAllowed,
    #[error("URL contains invalid characters")]
    InvalidCharacters,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_osc_and_controls() {
        assert_eq!(sanitize_line("a\u{1b}[31mred\u{1b}[0m"), "ared");
        assert_eq!(sanitize_line("x\u{1b}]0;evil title\u{7}y"), "xy");
        assert_eq!(
            sanitize_line("x\u{1b}]8;;http://e\u{1b}\\link\u{1b}]8;;\u{1b}\\"),
            "xlink"
        );
        assert_eq!(sanitize_line("a\u{9b}2Jb"), "ab");
        assert_eq!(sanitize_line("bell\u{7}\u{8}\u{0}"), "bell");
        assert_eq!(sanitize_line("line1\nline2\r\n"), "line1 line2 ");
        assert_eq!(sanitize_line("abc\u{202e}fed"), "abcfed");
        assert_eq!(sanitize_line("\u{1b}c"), "");
    }

    #[test]
    fn code_lines_keep_indentation_and_lose_escapes() {
        assert_eq!(sanitize_code_line("+\tif x {\r"), "+    if x {");
        assert_eq!(sanitize_code_line("-  a\u{1b}[2Jb\u{7}"), "-  ab");
        assert_eq!(sanitize_code_line(" \u{202e}x"), " x");
    }

    #[test]
    fn keeps_unicode_text() {
        assert_eq!(sanitize_line("修正: パーサー 🚀"), "修正: パーサー 🚀");
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_width("日本語テキスト", 7), "日本語…");
        assert_eq!(truncate_width("abc", 3), "abc");
        assert_eq!(truncate_width("abcdef", 4), "abc…");
    }

    #[test]
    fn url_validation() {
        let hosts = ["github.com"];
        assert!(validate_github_url("https://github.com/o/r/pull/1", &hosts).is_ok());
        assert_eq!(
            validate_github_url("http://github.com/o/r", &hosts),
            Err(UrlError::SchemeNotAllowed)
        );
        assert_eq!(
            validate_github_url("javascript:alert(1)", &hosts),
            Err(UrlError::SchemeNotAllowed)
        );
        assert_eq!(
            validate_github_url("file:///etc/passwd", &hosts),
            Err(UrlError::SchemeNotAllowed)
        );
        assert_eq!(
            validate_github_url("https://evil.com/github.com", &hosts),
            Err(UrlError::HostNotAllowed)
        );
        assert_eq!(
            validate_github_url("https://github.com@evil.com/", &hosts),
            Err(UrlError::HostNotAllowed)
        );
        assert_eq!(
            validate_github_url("https://github.com/a b", &hosts),
            Err(UrlError::InvalidCharacters)
        );
    }
}
