//! Human-readable table output. All text from GitHub is sanitized before
//! it reaches the terminal.

use chrono::{DateTime, Utc};
use ghusage_app::report::{PrReport, Report, ReportKind};
use ghusage_core::coverage::{CoverageStatus, CoverageSummary};
use ghusage_core::metrics::{Stats, Summary};
use ghusage_core::sanitize::{display_width, sanitize_line, truncate_width};
use ghusage_core::time::Zone;

/// Output styling. Colors are only used on a terminal without NO_COLOR.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{s}\u{1b}[0m")
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn warn(&self, s: &str) -> String {
        self.paint("33", s)
    }
}

/// A simple aligned table. Cells are sanitized and width-aware.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    right: Vec<bool>,
    max_width: Vec<Option<usize>>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: Vec::new(),
            right: vec![false; headers.len()],
            max_width: vec![None; headers.len()],
        }
    }
    /// Right-align numeric columns.
    pub fn numeric(mut self, cols: &[usize]) -> Self {
        for &c in cols {
            self.right[c] = true;
        }
        self
    }
    pub fn limit(mut self, col: usize, width: usize) -> Self {
        self.max_width[col] = Some(width);
        self
    }
    pub fn row(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn render(&self, style: Style) -> String {
        let cells: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|r| {
                r.iter()
                    .enumerate()
                    .map(|(i, c)| match self.max_width.get(i).copied().flatten() {
                        Some(w) => truncate_width(c, w),
                        None => sanitize_line(c),
                    })
                    .collect()
            })
            .collect();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| display_width(h)).collect();
        for r in &cells {
            for (i, c) in r.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(display_width(c));
                }
            }
        }
        let fmt_row = |r: &[String]| -> String {
            let mut line = String::new();
            for (i, c) in r.iter().enumerate() {
                let pad = widths[i].saturating_sub(display_width(c));
                if i > 0 {
                    line.push_str("  ");
                }
                if self.right[i] {
                    line.push_str(&" ".repeat(pad));
                    line.push_str(c);
                } else {
                    line.push_str(c);
                    if i + 1 < r.len() {
                        line.push_str(&" ".repeat(pad));
                    }
                }
            }
            line.trim_end().to_string()
        };
        let mut out = style.bold(&fmt_row(&self.headers));
        out.push('\n');
        for r in &cells {
            out.push_str(&fmt_row(r));
            out.push('\n');
        }
        out
    }
}

fn t(zone: Zone, ts: Option<DateTime<Utc>>) -> String {
    ts.map(|t| zone.format(t, "%Y-%m-%d %H:%M"))
        .unwrap_or_else(|| "-".into())
}

fn header(r: &Report, zone: Zone, title: &str, style: Style) -> String {
    let start = r
        .period
        .start
        .get(..16)
        .unwrap_or(&r.period.start)
        .replace('T', " ");
    let end = r
        .period
        .end
        .get(..16)
        .unwrap_or(&r.period.end)
        .replace('T', " ");
    let who = sanitize_line(&r.account.login);
    let mut s = format!(
        "{}  {}  {} → {} ({})\n",
        style.bold(title),
        who,
        start,
        end,
        zone.name()
    );
    if r.private_repositories_redacted {
        s.push_str(&style.dim("private repository names are hidden\n"));
    }
    s
}

/// Coverage footer: always shown so incomplete data is never silent.
pub fn coverage_lines(
    cov: &[CoverageSummary],
    synced: Option<DateTime<Utc>>,
    zone: Zone,
    style: Style,
) -> String {
    let mut out = String::new();
    let overall = CoverageStatus::combine(cov.iter().map(|c| c.status));
    let label = format!("Coverage: {}", overall.as_str());
    out.push_str(&if overall.is_complete() {
        style.dim(&label)
    } else {
        style.warn(&label)
    });
    out.push_str(&style.dim(&format!("  (last sync: {})", t(zone, synced))));
    out.push('\n');
    for c in cov {
        if c.status.is_complete() {
            continue;
        }
        let gaps: Vec<String> = c
            .gaps()
            .take(3)
            .map(|g| {
                let reason = g
                    .reason
                    .as_deref()
                    .map(|r| format!(": {}", sanitize_line(r)))
                    .unwrap_or_default();
                format!(
                    "{} {}–{}{}",
                    g.status.as_str(),
                    zone.format(g.start, "%Y-%m-%d %H:%M"),
                    zone.format(g.end, "%Y-%m-%d %H:%M"),
                    reason
                )
            })
            .collect();
        out.push_str(&style.warn(&format!(
            "  {}: {} — {}\n",
            c.kind.as_str(),
            c.status.as_str(),
            gaps.join("; ")
        )));
    }
    out
}

fn summary_lines(s: &Summary, open_prs: u64) -> Vec<(&'static str, String)> {
    vec![
        (
            "Commits",
            format!("{} ({} merge)", s.commits, s.merge_commits),
        ),
        ("PRs created", s.prs_created.to_string()),
        ("PRs merged (yours)", s.prs_merged.to_string()),
        (
            "Merges you performed",
            format!(
                "{}{}",
                s.merges_performed,
                if s.merges_unknown_actor > 0 {
                    format!(
                        " ({} of your PRs merged by unknown)",
                        s.merges_unknown_actor
                    )
                } else {
                    String::new()
                }
            ),
        ),
        (
            "PRs closed without merge",
            s.prs_closed_without_merge.to_string(),
        ),
        ("Open PRs (current)", open_prs.to_string()),
        (
            "Reviews submitted",
            format!(
                "{} (approved {}, changes requested {}, commented {}) on {} PRs",
                s.reviews_submitted.total,
                s.reviews_submitted.approved,
                s.reviews_submitted.changes_requested,
                s.reviews_submitted.commented,
                s.reviewed_prs
            ),
        ),
        (
            "Reviews received",
            format!(
                "{} ({} from bots)",
                s.reviews_received.total, s.reviews_received_from_bots
            ),
        ),
        (
            "Comments written",
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                s.comments_written.total(),
                s.comments_written.conversation,
                s.comments_written.inline,
                s.comments_written.review_summary
            ),
        ),
        (
            "Comments received",
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                s.comments_received.total(),
                s.comments_received.conversation,
                s.comments_received.inline,
                s.comments_received.review_summary
            ),
        ),
        ("Review thread replies", s.review_responses.to_string()),
        ("PR commits", s.pr_commits.to_string()),
        ("Force pushes", s.force_pushes.to_string()),
        (
            "Issues",
            format!(
                "opened {}, closed {}, reopened {}",
                s.issues_opened, s.issues_closed, s.issues_reopened
            ),
        ),
        (
            "Active PRs",
            format!(
                "{} (with anyone's activity: {})",
                s.active_prs, s.active_prs_all_participants
            ),
        ),
        ("PR internal events", s.pr_internal_events.to_string()),
        (
            "CI results (not counted as activity)",
            format!(
                "passed {}, failed {}, pending {}",
                s.ci.passed, s.ci.failed, s.ci.pending
            ),
        ),
        ("Active days", s.active_days.to_string()),
    ]
}

fn kv(lines: &[(&str, String)]) -> String {
    let w = lines
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0);
    lines
        .iter()
        .map(|(k, v)| format!("  {k}{}  {}\n", " ".repeat(w - display_width(k)), v))
        .collect()
}

fn breakdown_table(r: &Report, total: bool) -> Table {
    let mut t = Table::new(&[
        "Period",
        "Commits",
        "PRs",
        "Merged",
        "Reviews",
        "Received",
        "Comments",
        "PR commits",
        "Issues",
        "Active PRs",
    ])
    .numeric(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    let row = |label: String, s: &Summary| {
        vec![
            label,
            s.commits.to_string(),
            s.prs_created.to_string(),
            s.prs_merged.to_string(),
            s.reviews_submitted.total.to_string(),
            s.reviews_received.total.to_string(),
            s.comments_written.total().to_string(),
            s.pr_commits.to_string(),
            s.issues_opened.to_string(),
            s.active_prs.to_string(),
        ]
    };
    for b in &r.activity_breakdown {
        t.row(row(b.bucket.label.clone(), &b.summary));
    }
    if total {
        t.row(row("Total".into(), &r.metrics));
    }
    t
}

fn repo_table(r: &Report, limit: Option<usize>) -> Table {
    let mut t = Table::new(&[
        "Repository",
        "Vis",
        "Commits",
        "PRs",
        "Merged",
        "Reviews",
        "Comments",
        "Issues",
        "Active PRs",
        "PR events",
        "Last activity",
    ])
    .numeric(&[2, 3, 4, 5, 6, 7, 8, 9])
    .limit(0, 48);
    let zone = Zone::parse(&r.timezone).unwrap_or(Zone::utc());
    for x in r.repositories.iter().take(limit.unwrap_or(usize::MAX)) {
        let s = &x.summary;
        let mut name = x.full_name.clone();
        if !x.accessible {
            name.push_str(" (no access)");
        }
        t.row(vec![
            name,
            format!(
                "{}{}",
                x.visibility.as_str(),
                if x.owned { "*" } else { "" }
            ),
            s.commits.to_string(),
            s.prs_created.to_string(),
            s.prs_merged.to_string(),
            s.reviews_submitted.total.to_string(),
            s.comments_written.total().to_string(),
            s.issues_opened.to_string(),
            s.active_prs.to_string(),
            s.pr_internal_events.to_string(),
            self::t(zone, s.last_activity),
        ]);
    }
    t
}

fn stats_line(name: &str, s: &Option<Stats>, unit: f64, suffix: &str) -> (String, String) {
    let v = match s {
        Some(s) => format!(
            "median {:.1}{suffix}, mean {:.1}{suffix}, p90 {:.1}{suffix}, max {:.1}{suffix} (n={})",
            s.median / unit,
            s.mean / unit,
            s.p90 / unit,
            s.max / unit,
            s.count
        ),
        None => "no data".into(),
    };
    (name.to_string(), v)
}

/// Renders a period report.
pub fn report(r: &Report, kind: ReportKind, details: bool, style: Style) -> String {
    let zone = Zone::parse(&r.timezone).unwrap_or(Zone::utc());
    let mut out = String::new();
    match kind {
        ReportKind::Overview => {
            out.push_str(&header(r, zone, "Overview", style));
            out.push_str(&kv(&summary_lines(&r.metrics, r.snapshot.open_prs)));
            if !r.repositories.is_empty() {
                out.push('\n');
                out.push_str(&style.bold("Top repositories\n"));
                out.push_str(&repo_table(r, Some(5)).render(style));
            }
        }
        ReportKind::Breakdown(g) => {
            let title = match g {
                ghusage_core::time::Granularity::Day => "Daily",
                ghusage_core::time::Granularity::Week => "Weekly",
                ghusage_core::time::Granularity::Month => "Monthly",
                ghusage_core::time::Granularity::Year => "Yearly",
            };
            out.push_str(&header(r, zone, title, style));
            out.push_str(&breakdown_table(r, true).render(style));
        }
        ReportKind::Repositories => {
            out.push_str(&header(r, zone, "Repositories", style));
            let t = repo_table(r, None);
            if t.is_empty() {
                out.push_str("No repository activity in this period.\n");
            } else {
                out.push_str(&t.render(style));
                out.push_str(&style.dim("* owned by you\n"));
            }
        }
        ReportKind::PullRequests { .. } => {
            out.push_str(&header(r, zone, "Pull requests", style));
            let mut t = Table::new(&[
                "Repository",
                "#",
                "Title",
                "State",
                "Author",
                "Events",
                "Yours",
                "Commits",
                "Reviews",
                "Comments",
                "People",
                "CI",
            ])
            .numeric(&[1, 5, 6, 7, 8, 9, 10])
            .limit(0, 32)
            .limit(2, 40)
            .limit(4, 16);
            for p in r.pull_requests.iter().flatten() {
                let state = if p.is_draft && p.state == ghusage_core::model::PrState::Open {
                    "draft".to_string()
                } else {
                    p.state.as_str().to_string()
                };
                t.row(vec![
                    p.repository.clone(),
                    p.number.to_string(),
                    p.title.clone(),
                    state,
                    p.author.clone(),
                    p.events.to_string(),
                    p.events_by_subject.to_string(),
                    p.commits.to_string(),
                    p.reviews.to_string(),
                    p.comments.to_string(),
                    p.participants.to_string(),
                    p.latest_ci.map(|c| c.as_str()).unwrap_or("-").to_string(),
                ]);
            }
            if t.is_empty() {
                out.push_str("No pull request activity in this period.\n");
            } else {
                out.push_str(&t.render(style));
            }
            if details {
                out.push('\n');
                out.push_str(&style.bold("Activity per pull request\n"));
                if let Some(i) = &r.intensity {
                    let lines = [
                        stats_line("Events", &i.events_per_pr, 1.0, ""),
                        stats_line("Reviews", &i.reviews_per_pr, 1.0, ""),
                        stats_line("Comments", &i.comments_per_pr, 1.0, ""),
                        stats_line("Commits", &i.commits_per_pr, 1.0, ""),
                        stats_line("Participants", &i.participants_per_pr, 1.0, ""),
                        stats_line(
                            "Discussion participants",
                            &i.discussion_participants_per_pr,
                            1.0,
                            "",
                        ),
                        stats_line("Review threads", &i.threads_per_pr, 1.0, ""),
                        stats_line("Active days", &i.active_days_per_pr, 1.0, ""),
                    ];
                    let refs: Vec<(&str, String)> =
                        lines.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                    out.push_str(&kv(&refs));
                }
                out.push_str(&style.bold("Lifecycle timing (your pull requests)\n"));
                if let Some(tm) = &r.timing {
                    let lines = [
                        stats_line(
                            "Time to first review",
                            &tm.time_to_first_review,
                            3600.0,
                            "h",
                        ),
                        stats_line("Creation to merge", &tm.creation_to_merge, 3600.0, "h"),
                        stats_line(
                            "Review to your next activity",
                            &tm.review_to_next_author_activity,
                            3600.0,
                            "h",
                        ),
                        stats_line(
                            "First to last activity",
                            &tm.first_to_last_activity,
                            3600.0,
                            "h",
                        ),
                    ];
                    let refs: Vec<(&str, String)> =
                        lines.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                    out.push_str(&kv(&refs));
                    out.push_str(&style.dim(
                        "  \"Review to your next activity\" is the next observed event, not necessarily a response.\n",
                    ));
                }
                out.push_str(&style.dim(
                    "  These are descriptive statistics, not measures of quality or productivity.\n",
                ));
            }
        }
        ReportKind::Reviews => {
            out.push_str(&header(r, zone, "Reviews", style));
            let s = &r.metrics;
            out.push_str(&kv(&[
                (
                    "Submitted",
                    format!(
                        "{} on {} PRs (approved {}, changes requested {}, commented {}, dismissed {})",
                        s.reviews_submitted.total,
                        s.reviewed_prs,
                        s.reviews_submitted.approved,
                        s.reviews_submitted.changes_requested,
                        s.reviews_submitted.commented,
                        s.reviews_submitted.dismissed_unknown_state
                    ),
                ),
                (
                    "Received",
                    format!(
                        "{} (approved {}, changes requested {}, commented {}; {} from bots)",
                        s.reviews_received.total,
                        s.reviews_received.approved,
                        s.reviews_received.changes_requested,
                        s.reviews_received.commented,
                        s.reviews_received_from_bots
                    ),
                ),
            ]));
            for (title, rows) in [
                ("Submitted", r.reviews_submitted.as_deref().unwrap_or(&[])),
                ("Received", r.reviews_received.as_deref().unwrap_or(&[])),
            ] {
                if rows.is_empty() {
                    continue;
                }
                out.push('\n');
                out.push_str(&style.bold(&format!("{title}\n")));
                let mut tb = Table::new(&["Time", "Reviewer", "State", "Repository", "#", "Title"])
                    .numeric(&[4])
                    .limit(3, 32)
                    .limit(5, 40);
                for x in rows {
                    tb.row(vec![
                        zone.format(x.submitted_at, "%Y-%m-%d %H:%M"),
                        x.reviewer.clone(),
                        x.state
                            .map(|s| s.as_str().to_ascii_lowercase())
                            .unwrap_or_default(),
                        x.repository.clone(),
                        x.pr_number.map(|n| n.to_string()).unwrap_or_default(),
                        x.pr_title.clone().unwrap_or_default(),
                    ]);
                }
                out.push_str(&tb.render(style));
            }
        }
        ReportKind::Comments => {
            out.push_str(&header(r, zone, "Comments", style));
            out.push_str(&kv(&summary_lines(&r.metrics, r.snapshot.open_prs)[8..11]));
            let rows = r.comments.as_deref().unwrap_or(&[]);
            if !rows.is_empty() {
                out.push('\n');
                let mut tb = Table::new(&[
                    "Time",
                    "Author",
                    "Type",
                    "Yours",
                    "Repository",
                    "#",
                    "Edited",
                ])
                .numeric(&[5])
                .limit(4, 32);
                for c in rows {
                    tb.row(vec![
                        zone.format(c.created_at, "%Y-%m-%d %H:%M"),
                        c.author.clone(),
                        c.kind.label().to_string(),
                        if c.written_by_subject { "yes" } else { "" }.into(),
                        c.repository.clone(),
                        c.pr_number.map(|n| n.to_string()).unwrap_or_default(),
                        if c.edited { "edited" } else { "" }.into(),
                    ]);
                }
                out.push_str(&tb.render(style));
            }
        }
        ReportKind::Issues => {
            out.push_str(&header(r, zone, "Issues", style));
            let s = &r.metrics;
            out.push_str(&kv(&[
                (
                    "Issues",
                    format!(
                        "opened {}, closed {}, reopened {}",
                        s.issues_opened, s.issues_closed, s.issues_reopened
                    ),
                ),
                (
                    "Open issues you authored (current)",
                    r.snapshot.open_issues_authored.to_string(),
                ),
            ]));
            let rows = r.issues.as_deref().unwrap_or(&[]);
            if !rows.is_empty() {
                out.push('\n');
                let mut tb = Table::new(&[
                    "Repository",
                    "#",
                    "Title",
                    "State",
                    "Opened",
                    "Closed",
                    "Reopened",
                ])
                .numeric(&[1, 5, 6])
                .limit(0, 32)
                .limit(2, 48);
                for i in rows {
                    tb.row(vec![
                        i.repository.clone(),
                        i.number.to_string(),
                        i.title.clone(),
                        if i.is_open { "open" } else { "closed" }.into(),
                        if i.opened_in_period { "yes" } else { "" }.into(),
                        i.closed_in_period.to_string(),
                        i.reopened_in_period.to_string(),
                    ]);
                }
                out.push_str(&tb.render(style));
            }
        }
    }
    out.push('\n');
    out.push_str(&coverage_lines(&r.coverage, r.synced_at, zone, style));
    out
}

fn dur(secs: Option<i64>) -> String {
    match secs {
        None => "-".into(),
        Some(s) if s < 3600 => format!("{}m", s / 60),
        Some(s) if s < 86400 => format!("{:.1}h", s as f64 / 3600.0),
        Some(s) => format!("{:.1}d", s as f64 / 86400.0),
    }
}

/// Renders a pull request detail report.
pub fn pr(r: &PrReport, timeline: bool, style: Style) -> String {
    let zone = Zone::parse(&r.timezone).unwrap_or(Zone::utc());
    let mut out = String::new();
    out.push_str(&style.bold(&format!(
        "{}#{} — {}\n",
        sanitize_line(&r.repository),
        r.number,
        sanitize_line(&r.title)
    )));
    let state = if r.is_draft && r.state == ghusage_core::model::PrState::Open {
        "open (draft)".to_string()
    } else {
        r.state.as_str().to_string()
    };
    out.push_str(&kv(&[
        ("Author", sanitize_line(&r.author)),
        ("State (current)", state),
        ("Created", t(zone, Some(r.created_at))),
        (
            "Merged",
            match (&r.merged_at, &r.merged_by) {
                (Some(m), Some(b)) => format!("{} by {}", t(zone, Some(*m)), sanitize_line(b)),
                (Some(m), None) => format!("{} (merger unknown)", t(zone, Some(*m))),
                _ => "-".into(),
            },
        ),
        (
            "Commits",
            format!(
                "{}{}",
                r.commits,
                if r.removed_commits > 0 {
                    format!(" ({} no longer in the PR)", r.removed_commits)
                } else {
                    String::new()
                }
            ),
        ),
        (
            "Reviews",
            format!(
                "{} (approved {}, changes requested {}, commented {})",
                r.reviews.total,
                r.reviews.approved,
                r.reviews.changes_requested,
                r.reviews.commented
            ),
        ),
        (
            "Comments",
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                r.comments.total(),
                r.comments.conversation,
                r.comments.inline,
                r.comments.review_summary
            ),
        ),
        (
            "Review threads",
            format!(
                "{} (open {}, resolved {}, outdated {})",
                r.review_threads.total,
                r.review_threads.open,
                r.review_threads.resolved,
                r.review_threads.outdated
            ),
        ),
        (
            "Latest CI",
            r.latest_ci.clone().unwrap_or_else(|| "-".into()),
        ),
        ("Participants", sanitize_line(&r.participants.join(", "))),
        (
            "Discussion",
            sanitize_line(&r.discussion_participants.join(", ")),
        ),
        (
            "Commit authors",
            sanitize_line(
                &r.commit_authors
                    .iter()
                    .map(|a| format!("{} {}", a.author, a.commits))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ),
        (
            "Time to first review",
            dur(r.timing.time_to_first_review_secs),
        ),
        ("Creation to merge", dur(r.timing.creation_to_merge_secs)),
        ("URL", sanitize_line(&r.url)),
    ]));
    if timeline {
        out.push('\n');
        let mut tb = Table::new(&["Time", "Actor", "Event", "Detail"])
            .limit(1, 24)
            .limit(3, 60);
        for e in &r.timeline {
            let mut actor = e.actor.clone();
            if e.actor_kind == ghusage_core::model::ActorKind::Bot && !actor.ends_with("[bot]") {
                actor.push_str(" (bot)");
            }
            tb.row(vec![
                zone.format(e.occurred_at, "%Y-%m-%d %H:%M:%S"),
                actor,
                e.label.clone(),
                e.detail.clone().unwrap_or_default(),
            ]);
        }
        if tb.is_empty() {
            out.push_str("No events.\n");
        } else {
            out.push_str(&tb.render(style));
        }
        out.push_str(&style.dim(
            "Commit times are when commits were made, not when they were pushed to the PR.\n",
        ));
    }
    out.push('\n');
    out.push_str(&style.dim(&format!(
        "Coverage: {}  (synced: {})\n",
        r.coverage.as_str(),
        t(zone, r.synced_at)
    )));
    out
}
