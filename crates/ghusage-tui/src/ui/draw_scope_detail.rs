fn draw_scope_detail(f: &mut Frame<'_>, app: &App, area: Rect) {
    let name = match app.screen {
        Screen::DayDetail => app
            .detail_day
            .as_ref().map_or_else(|| "Day".into(), |d| format!("Day {}", d.label)),
        _ => app
            .detail_repo
            .as_ref().map_or_else(|| "Repository".into(), |r| format!("Repository {}", clean(&r.full_name))),
    };
    let Some(r) = app.detail_report() else {
        return loading(f, area, app.screen.title());
    };
    let s = &r.metrics;
    let summary = [
        format!(
            "Commits {} · PRs created {} · merged {} · Reviews submitted {} · received {}",
            s.commits,
            s.prs_created,
            s.prs_merged,
            s.reviews_submitted.total,
            s.reviews_received.total
        ),
        format!(
            "Comments written {} · received {} · Issues opened {} · closed {} · Active PRs {} · coverage: {}",
            s.comments_written.total(),
            s.comments_received.total(),
            s.issues_opened,
            s.issues_closed,
            s.active_prs,
            coverage_text(r.overall_coverage())
        ),
    ];
    let show_summary = area.height >= 9;
    let constraints = if show_summary {
        vec![
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Min(1),
        ]
    } else {
        vec![Constraint::Length(1), Constraint::Min(1)]
    };
    let areas = Layout::vertical(constraints).split(area);
    let (tabs_area, list_area) = if show_summary {
        let inner = area.width.saturating_sub(2) as usize;
        let lines: Vec<Line<'_>> = summary.iter().map(|l| Line::raw(fit(l, inner))).collect();
        f.render_widget(
            Paragraph::new(lines).block(block(fit(&name, inner.saturating_sub(4)), false)),
            areas[0],
        );
        (areas[1], areas[2])
    } else {
        (areas[0], areas[1])
    };

    let full: usize = app
        .detail_tabs()
        .iter()
        .map(|t| display_width(t.label()) + 8)
        .sum::<usize>()
        + 16;
    let short = full > tabs_area.width as usize;
    let mut spans = vec![Span::raw(if short { " " } else { " section (Tab): " })];
    for t in app.detail_tabs() {
        let name = if short { t.short_label() } else { t.label() };
        let label = format!("{name} ({})", detail_count(r, *t));
        if *t == app.detail_tab {
            spans.push(Span::styled(format!("[{label}]"), highlight()));
        } else {
            spans.push(Span::raw(format!(" {label} ")));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    let list = app.detail_tab.list();
    let vis = app.visible(list);
    let title = format!(
        "{} · {} · {} shown",
        fit(&name, 50),
        app.detail_tab.label(),
        vis.len()
    );
    if detail_count(r, app.detail_tab) == 0 {
        let msg = format!(
            "No {} here in this period. Press Tab for another section.",
            app.detail_tab.label().to_lowercase()
        );
        return empty(f, list_area, title, &[&msg]);
    }
    if vis.is_empty() {
        return empty(f, list_area, title, &[&no_match(app, list)]);
    }
    let z = &app.zone;
    let t = match app.detail_tab {
        DetailTab::Activity => detail_activity_table(r, &vis, list_area, z),
        DetailTab::Commits => detail_commits_table(r, &vis, list_area, z),
        DetailTab::PullRequests => detail_pullrequests_table(r, &vis, list_area, z),
        DetailTab::Reviews => detail_reviews_table(app, &vis, list_area, z),
        DetailTab::Comments => detail_comments_table(r, &vis, list_area, z),
        DetailTab::Issues => detail_issues_table(r, &vis, list_area),
    }
    .block(block(title, true));
    stateful_table(f, app, list_area, list, t, true);
}

fn detail_activity_table<'a>(r: &'a Report, vis: &[usize], list_area: Rect, z: &Zone) -> Table<'a> {
let rows_all = r.activity.as_deref().unwrap_or(&[]);
            let flex = flex_width(list_area, &[11, 20, 18]);
            let where_w = (flex * 2 / 5).max(8);
            let title_w = flex.saturating_sub(where_w + 1).max(8);
            let rows: Vec<Row<'_>> = vis
                .iter()
                .map(|&i| {
                    let a = &rows_all[i];
                    let place = match a.number {
                        Some(n) => format!("{}#{n}", a.repository),
                        None => a.repository.clone(),
                    };
                    Row::new(vec![
                        z.format(a.occurred_at, "%m-%d %H:%M"),
                        fit(&actor_text(&a.actor, a.by_subject), 20),
                        a.event_type.label().to_string(),
                        fit(&place, where_w),
                        fit(a.title.as_deref().unwrap_or("-"), title_w),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(11),
                    Constraint::Length(20),
                    Constraint::Length(18),
                    Constraint::Length(to_u16(where_w)),
                    Constraint::Min(8),
                ],
                &["Time", "Actor", "Event", "Where", "Title"],
            )
}

fn detail_commits_table<'a>(r: &'a Report, vis: &[usize], list_area: Rect, z: &Zone) -> Table<'a> {
let rows_all = r.commits.as_deref().unwrap_or(&[]);
            let repo_w = flex_width(list_area, &[16, 12, 5]);
            let rows: Vec<Row<'_>> = vis
                .iter()
                .map(|&i| {
                    let c = &rows_all[i];
                    let sha: String = c.sha.as_deref().unwrap_or("-").chars().take(10).collect();
                    Row::new(vec![
                        z.format(c.committed_at, "%Y-%m-%d %H:%M"),
                        fit(&sha, 12),
                        fit(&c.repository, repo_w),
                        if c.is_merge_commit { "merge" } else { "" }.to_string(),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(16),
                    Constraint::Length(12),
                    Constraint::Min(8),
                    Constraint::Length(5),
                ],
                &["Committed", "SHA", "Repository", "Merge"],
            )
}

fn detail_pullrequests_table<'a>(r: &'a Report, vis: &[usize], list_area: Rect, z: &Zone) -> Table<'a> {
let rows_all = r.pull_requests.as_deref().unwrap_or(&[]);
            let fixed = [9, 6, 5, 7, 8, 11];
            let flex = flex_width(list_area, &fixed);
            let pr_w = (flex * 2 / 5).max(8);
            let title_w = flex.saturating_sub(pr_w + 1).max(8);
            let rows: Vec<Row<'_>> = vis
                .iter()
                .map(|&i| {
                    let p = &rows_all[i];
                    Row::new(vec![
                        Cell::from(Span::styled(
                            pr_state_text(p.state, p.is_draft),
                            pr_state_style(p.state),
                        )),
                        Cell::from(fit(&format!("{}#{}", p.repository, p.number), pr_w)),
                        Cell::from(fit(&p.title, title_w)),
                        Cell::from(p.events.to_string()),
                        Cell::from(p.events_by_subject.to_string()),
                        Cell::from(p.reviews.to_string()),
                        Cell::from(p.comments.to_string()),
                        Cell::from(
                            p.last_activity.map_or_else(|| "-".into(), |t| z.format(t, "%m-%d %H:%M")),
                        ),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(9),
                    Constraint::Length(to_u16(pr_w)),
                    Constraint::Min(8),
                    Constraint::Length(6),
                    Constraint::Length(5),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(11),
                ],
                &[
                    "State", "PR", "Title", "Events", "Yours", "Reviews", "Comments", "Last",
                ],
            )
}

fn detail_reviews_table<'a>(app: &App, vis: &[usize], list_area: Rect, z: &Zone) -> Table<'a> {
let rows_all = app.detail_reviews();
            let flex = flex_width(list_area, &[9, 11, 18, 18]);
            let pr_w = (flex * 2 / 5).max(8);
            let rows: Vec<Row<'_>> = vis
                .iter()
                .map(|&i| {
                    let (dir, x) = rows_all[i];
                    let state = x
                        .state.map_or_else(|| "-".into(), |s| s.as_str().to_ascii_lowercase().replace('_', " "));
                    let pr = match x.pr_number {
                        Some(n) => format!("{}#{n}", x.repository),
                        None => x.repository.clone(),
                    };
                    Row::new(vec![
                        match dir {
                            ReviewDirection::Submitted => "given",
                            ReviewDirection::Received => "received",
                        }
                        .to_string(),
                        z.format(x.submitted_at, "%m-%d %H:%M"),
                        fit(&x.reviewer, 18),
                        state,
                        fit(&pr, pr_w),
                        fit(
                            x.pr_title.as_deref().unwrap_or("-"),
                            flex.saturating_sub(pr_w + 1).max(8),
                        ),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(18),
                    Constraint::Length(18),
                    Constraint::Length(to_u16(pr_w)),
                    Constraint::Min(8),
                ],
                &["Direction", "Submitted", "Reviewer", "State", "PR", "Title"],
            )
}

fn detail_comments_table<'a>(r: &'a Report, vis: &[usize], list_area: Rect, z: &Zone) -> Table<'a> {
let rows_all = r.comments.as_deref().unwrap_or(&[]);
            let pr_w = flex_width(list_area, &[11, 20, 15, 9]);
            let rows: Vec<Row<'_>> = vis
                .iter()
                .map(|&i| {
                    let c = &rows_all[i];
                    let pr = match c.pr_number {
                        Some(n) => format!("{}#{n}", c.repository),
                        None => c.repository.clone(),
                    };
                    let mut kind = c.kind.label().to_string();
                    if c.edited {
                        kind.push_str(" ✎");
                    }
                    Row::new(vec![
                        z.format(c.created_at, "%m-%d %H:%M"),
                        fit(&c.author, 20),
                        kind,
                        if c.written_by_subject {
                            "written"
                        } else {
                            "received"
                        }
                        .to_string(),
                        fit(&pr, pr_w),
                    ])
                })
                .collect();
            styled_table(
                rows,
                vec![
                    Constraint::Length(11),
                    Constraint::Length(20),
                    Constraint::Length(15),
                    Constraint::Length(9),
                    Constraint::Min(8),
                ],
                &["Time", "Author", "Kind", "Direction", "PR"],
            )
}

fn detail_issues_table<'a>(r: &'a Report, vis: &[usize], list_area: Rect) -> Table<'a> {
let rows_all = r.issues.as_deref().unwrap_or(&[]);
            issue_table(rows_all, vis, list_area)
}

// ----- System ----------------------------------------------------------------

fn draw_system(f: &mut Frame<'_>, app: &App, area: Rect) {
    let z = &app.zone;
    let s = &app.system;
    let w = area.width.saturating_sub(26) as usize;
    let mut lines = vec![
        kv(
            "Account",
            format!(
                "{}{}",
                clean(&s.login),
                if s.subject_is_viewer {
                    " (authenticated user)"
                } else {
                    ""
                }
            ),
        ),
        kv(
            "Mode",
            if app.background_sync {
                "online · reading the cache, syncing in the background".into()
            } else {
                "offline · cache only, GitHub is not contacted".into()
            },
        ),
        kv(
            "Authentication",
            match (&app.sync.auth, app.background_sync) {
                (Some(a), _) => format!("token from {}", clean(a)),
                (None, true) if app.sync.running => "connecting…".into(),
                (None, true) if app.sync.last_error.is_some() => {
                    "sync could not start (see errors below)".into()
                }
                (None, true) => "not checked yet (press r to sync)".into(),
                (None, false) => "not used (offline)".into(),
            },
        ),
        kv(
            "Rate limit",
            app.sync
                .rate_limit
                .as_deref().map_or_else(|| "unknown".into(), clean),
        ),
        kv(
            "Last sync",
            match &s.last_sync {
                Some(l) => fit(
                    &format!(
                        "{} · {}{}",
                        ts(z, l.finished_at.or(Some(l.started_at))),
                        l.status,
                        l.message
                            .as_deref()
                            .map(|m| format!(" · {m}"))
                            .unwrap_or_default()
                    ),
                    w,
                ),
                None => "never".into(),
            },
        ),
        kv(
            "Sync now",
            if app.sync.running {
                format!(
                    "running · {}",
                    clean(app.sync.progress.as_deref().unwrap_or(""))
                )
            } else if let Some(sum) = &app.sync.last_summary {
                fit(&format!("last run this session: {sum}"), w)
            } else {
                "idle".into()
            },
        ),
        kv(
            "Cache",
            match s.cache_size_bytes {
                Some(b) => format!(
                    "{} · {} events, {} pull requests, {} repositories",
                    bytes(b),
                    s.cached_events.unwrap_or(0),
                    s.cached_pull_requests.unwrap_or(0),
                    s.cached_repositories.unwrap_or(0)
                ),
                None => "unknown".into(),
            },
        ),
        kv(
            "Data directory",
            s.data_dir
                .as_ref().map_or_else(|| "in memory".into(), |d| fit(&d.display().to_string(), w)),
        ),
        Line::raw(""),
        Line::styled("Coverage for the selected period", bold()),
    ];
    match app.base_report() {
        Some(r) => {
            for c in &r.coverage {
                let gaps = c.gaps().count();
                lines.push(kv(
                    &format!("  {}", c.kind.as_str()),
                    format!(
                        "{}{}",
                        coverage_text(c.status),
                        if gaps > 0 {
                            format!(" · {gaps} gap(s)")
                        } else {
                            String::new()
                        }
                    ),
                ));
            }
        }
        None => lines.push(Line::raw("  computing…")),
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Warnings and errors", bold()));
    let any = s.error.is_some()
        || app.sync.last_error.is_some()
        || !app.sync.warnings.is_empty();
    if let Some(e) = &s.error {
        lines.push(Line::raw(format!("  ! cache: {}", fit(e, w))));
    }
    if let Some(e) = &app.sync.last_error {
        lines.push(Line::raw(format!("  ! sync failed: {}", fit(e, w))));
    }
    for warn in app.sync.warnings.iter().take(20) {
        lines.push(Line::raw(format!("  ! {}", fit(warn, w))));
    }
    if app.sync.warnings.len() > 20 {
        lines.push(Line::raw(format!(
            "  … and {} more",
            app.sync.warnings.len() - 20
        )));
    }
    if !any {
        lines.push(Line::raw("  none"));
    }
    f.render_widget(
        Paragraph::new(lines).block(block("System".into(), true)),
        area,
    );
}

// ----- Help --------------------------------------------------------------------

/// Key bindings shown in the help overlay.
pub const HELP: &[(&str, &str)] = &[
    (
        "1-8",
        "Dashboard / Overview / Daily / Repositories / Pull Requests / Reviews / Issues / System",
    ),
    (
        "Enter",
        "Open detail: day, repository, PR (then timeline), issue, commit; review → its PR",
    ),
    ("Esc", "Back to the previous screen / clear search"),
    ("j / Down", "Next item (Commit: scroll down)"),
    ("k / Up", "Previous item (Commit: scroll up)"),
    ("PgDn PgUp g G", "Page down / up, first, last"),
    ("] / [", "Commit: next / previous file of the diff"),
    (
        "Tab / Shift+Tab",
        "Next / previous pane or section (Reviews: submitted/received; PR: commits)",
    ),
    ("/", "Search the current list"),
    (
        "r",
        "Reload from cache (and sync in the background when online)",
    ),
    ("t", "Cycle time range"),
    (
        "f",
        "Cycle repository filter: all/public/private/owned/external",
    ),
    ("o", "Open the selected item on GitHub"),
    ("?", "Toggle this help"),
    ("q", "Quit"),
];
