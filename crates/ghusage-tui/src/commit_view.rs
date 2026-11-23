//! The commit detail screen's content: a commit turned into rows of
//! sanitized text, built once when the commit arrives (off the UI thread)
//! so drawing only touches the rows on screen.

use std::sync::Arc;

use ghusage_app::commit::CommitSource;
use ghusage_core::commit::{CommitDetail, CommitPerson, DiffLineKind, diff_lines, short_sha};
use ghusage_core::sanitize::{sanitize_code_line, sanitize_line};
use ghusage_core::time::Zone;
use jiff::Timestamp;

/// Most diff lines kept for one file.
pub const MAX_FILE_LINES: usize = 3_000;
/// Most diff lines kept for the whole commit.
pub const MAX_DIFF_LINES: usize = 20_000;
/// Longest line kept, in characters; the screen shows less anyway.
const MAX_LINE_CHARS: usize = 1_000;

/// Identifies a commit: `owner/name` and a full or abbreviated SHA.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitKey {
    pub repository: String,
    pub sha: String,
}

impl CommitKey {
    /// The commit's page on GitHub.
    pub fn github_url(&self) -> String {
        format!("https://github.com/{}/commit/{}", self.repository, self.sha)
    }
}

/// What is already known about a commit from the cached activity, shown
/// before (or instead of) the fetched detail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSeed {
    pub headline: Option<String>,
    pub author: Option<String>,
    pub date: Option<Timestamp>,
    pub url: Option<String>,
    /// No longer part of the pull request it was listed in.
    pub removed: bool,
    pub is_merge: bool,
}

/// Why a commit could not be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFailure {
    /// Not cached, and GitHub is not used or cannot be reached.
    pub offline: bool,
    pub message: String,
}

/// The result of loading a commit.
#[derive(Debug, Clone)]
pub struct CommitMsg {
    pub key: CommitKey,
    pub result: Result<Arc<CommitView>, CommitFailure>,
}

/// How a row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Title,
    Heading,
    /// `key` in bold, then the text.
    Field,
    Message,
    FileEntry,
    Total,
    Warning,
    /// The start of one file's diff.
    FileHeader,
    Hunk,
    Added,
    Removed,
    Context,
    /// Explanations such as a missing diff or cut lines.
    Note,
    Blank,
}

impl RowKind {
    fn of(k: DiffLineKind) -> Self {
        match k {
            DiffLineKind::Hunk => RowKind::Hunk,
            DiffLineKind::Added => RowKind::Added,
            DiffLineKind::Removed => RowKind::Removed,
            DiffLineKind::Context => RowKind::Context,
            DiffLineKind::Note => RowKind::Note,
        }
    }
}

/// One row of the commit screen. `text` is sanitized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub key: &'static str,
    pub text: String,
}

/// A commit ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitView {
    pub repository: String,
    pub sha: String,
    pub url: Option<String>,
    pub rows: Vec<Row>,
    /// Row index of each file's diff header, in file order.
    pub file_rows: Vec<usize>,
    /// Diff lines left out because of the limits.
    pub hidden_lines: usize,
}

fn clip(s: String) -> String {
    if s.chars().count() <= MAX_LINE_CHARS {
        s
    } else {
        let mut out: String = s.chars().take(MAX_LINE_CHARS).collect();
        out.push('…');
        out
    }
}

fn who(p: &CommitPerson) -> String {
    let s = match (&p.name, &p.login) {
        (Some(n), Some(l)) if n != l => format!("{n} ({l})"),
        (_, Some(l)) => l.clone(),
        (Some(n), None) => n.clone(),
        (None, None) => "unknown".into(),
    };
    sanitize_line(&s)
}

fn when(zone: &Zone, t: Option<Timestamp>) -> String {
    t.map(|t| zone.format(t, "%Y-%m-%d %H:%M:%S %Z"))
        .unwrap_or_else(|| "-".into())
}

impl CommitView {
    /// Turns a commit into rows: facts, message, files, then each file's
    /// diff. Diffs longer than [`MAX_FILE_LINES`] per file or
    /// [`MAX_DIFF_LINES`] in total are cut, with a note.
    pub fn build(d: &CommitDetail, source: CommitSource, zone: Zone) -> Self {
        let mut rows: Vec<Row> = Vec::new();
        fn push(rows: &mut Vec<Row>, kind: RowKind, key: &'static str, text: String) {
            rows.push(Row {
                kind,
                key,
                text: clip(text),
            });
        }
        let repo = sanitize_line(&d.repository);
        push(
            &mut rows,
            RowKind::Title,
            "",
            format!(
                "{repo}@{} — {}",
                short_sha(&d.sha),
                sanitize_line(d.headline())
            ),
        );
        push(&mut rows, RowKind::Blank, "", String::new());
        let parents = if d.parents.is_empty() {
            "none (root commit)".to_string()
        } else {
            let list: Vec<&str> = d.parents.iter().map(|p| short_sha(p)).collect();
            format!(
                "{}{}",
                sanitize_line(&list.join(", ")),
                if d.is_merge() { " (merge commit)" } else { "" }
            )
        };
        let totals = d.file_totals();
        let fields: Vec<(&'static str, String)> = vec![
            ("Repository", repo.clone()),
            ("SHA", sanitize_line(&d.sha)),
            ("Author", who(&d.author)),
            ("Author date", when(&zone, d.author.date)),
            ("Committer", who(&d.committer)),
            ("Committer date", when(&zone, d.committer.date)),
            ("Parents", parents),
            (
                "Changes",
                format!(
                    "{} file(s), +{} −{}",
                    d.files.len(),
                    d.stats.additions,
                    d.stats.deletions
                ),
            ),
            (
                "Loaded",
                format!(
                    "{} · {}",
                    match source {
                        CommitSource::Cache => "from the cache",
                        CommitSource::Github => "from GitHub",
                    },
                    when(&zone, Some(d.fetched_at))
                ),
            ),
        ];
        for (k, v) in fields {
            push(&mut rows, RowKind::Field, k, v);
        }
        push(&mut rows, RowKind::Blank, "", String::new());
        push(&mut rows, RowKind::Heading, "", "Message".into());
        if d.message.trim().is_empty() {
            push(&mut rows, RowKind::Note, "", "  (empty message)".into());
        }
        for line in d.message.trim_end().lines() {
            push(
                &mut rows,
                RowKind::Message,
                "",
                format!("  {}", sanitize_code_line(line)),
            );
        }
        push(&mut rows, RowKind::Blank, "", String::new());
        push(
            &mut rows,
            RowKind::Heading,
            "",
            format!("Files ({})", d.files.len()),
        );
        let name = |f: &ghusage_core::commit::CommitFile| match &f.previous_filename {
            Some(old) if old != &f.filename => sanitize_line(&format!("{old} → {}", f.filename)),
            _ => sanitize_line(&f.filename),
        };
        for f in &d.files {
            push(
                &mut rows,
                RowKind::FileEntry,
                "",
                format!(
                    "  {:<9} {:>7} {:>7}  {}",
                    f.status.as_str(),
                    format!("+{}", f.additions),
                    format!("−{}", f.deletions),
                    name(f)
                ),
            );
        }
        push(
            &mut rows,
            RowKind::Total,
            "",
            format!(
                "  {:<9} {:>7} {:>7}  {} file(s)",
                "total",
                format!("+{}", totals.additions),
                format!("−{}", totals.deletions),
                d.files.len()
            ),
        );
        if d.files_truncated {
            push(
                &mut rows,
                RowKind::Warning,
                "",
                "  ! Not every changed file is listed (GitHub lists at most 3000 files)".into(),
            );
        }
        push(&mut rows, RowKind::Blank, "", String::new());
        push(&mut rows, RowKind::Heading, "", "Diff".into());
        if d.files.is_empty() {
            push(&mut rows, RowKind::Note, "", "  (no changed files)".into());
        }
        let mut file_rows = Vec::with_capacity(d.files.len());
        let mut shown_total = 0usize;
        let mut hidden = 0usize;
        let n = d.files.len();
        for (i, f) in d.files.iter().enumerate() {
            file_rows.push(rows.len());
            push(
                &mut rows,
                RowKind::FileHeader,
                "",
                format!(
                    "── [{}/{n}] {} {} (+{} −{})",
                    i + 1,
                    f.status.as_str(),
                    name(f),
                    f.additions,
                    f.deletions
                ),
            );
            match (&f.patch, f.missing_patch_reason()) {
                (Some(patch), None) => {
                    let mut shown = 0usize;
                    let mut cut = 0usize;
                    for (kind, line) in diff_lines(patch) {
                        if shown >= MAX_FILE_LINES || shown_total >= MAX_DIFF_LINES {
                            cut += 1;
                            continue;
                        }
                        shown += 1;
                        shown_total += 1;
                        push(&mut rows, RowKind::of(kind), "", sanitize_code_line(line));
                    }
                    if cut > 0 {
                        hidden += cut;
                        push(
                            &mut rows,
                            RowKind::Note,
                            "",
                            format!(
                                "  … {cut} more line(s) not shown; press o to see the whole diff on GitHub"
                            ),
                        );
                    }
                }
                (_, reason) => push(
                    &mut rows,
                    RowKind::Note,
                    "",
                    format!("  ({})", reason.unwrap_or("no diff available from GitHub")),
                ),
            }
            push(&mut rows, RowKind::Blank, "", String::new());
        }
        Self {
            repository: d.repository.clone(),
            sha: d.sha.clone(),
            url: d.url.clone(),
            rows,
            file_rows,
            hidden_lines: hidden,
        }
    }

    /// The file whose diff contains row `row` (0-based), if any.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        match self.file_rows.binary_search(&row) {
            Ok(i) => Some(i),
            Err(0) => None,
            Err(i) => Some(i - 1),
        }
    }

    /// First row of the next file after `row`.
    pub fn next_file(&self, row: usize) -> Option<usize> {
        self.file_rows.iter().copied().find(|r| *r > row)
    }

    /// First row of the file before the one at `row`.
    pub fn prev_file(&self, row: usize) -> Option<usize> {
        self.file_rows.iter().copied().rev().find(|r| *r < row)
    }
}
