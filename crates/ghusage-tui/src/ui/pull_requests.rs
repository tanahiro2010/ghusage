fn draw_pull_requests(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_list_report() else {
        return loading(f, area, "Pull Requests");
    };
    let rows_all = r.pull_requests.as_deref().unwrap_or(&[]);
    let list = ListId::PullRequests;
    let vis = app.visible(list);
    let title = format!(
        "Pull Requests you were active in · {} shown of {} · Enter for detail",
        vis.len(),
        rows_all.len()
    );
    if rows_all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No pull request activity in this period. Press t to change the time range."],
        );
    }
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let fixed = [9, 6, 5, 7, 7, 8, 6, 16];
    let flex = flex_width(area, &fixed);
    let repo_w = (flex * 2 / 5).max(8);
    let title_w = flex.saturating_sub(repo_w + 1).max(8);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let p = &rows_all[i];
            Row::new(vec![
                Cell::from(Span::styled(
                    pr_state_text(p.state, p.is_draft),
                    pr_state_style(p.state),
                )),
                Cell::from(fit(&format!("{}#{}", p.repository, p.number), repo_w)),
                Cell::from(fit(&p.title, title_w)),
                Cell::from(p.events.to_string()),
                Cell::from(p.events_by_subject.to_string()),
                Cell::from(p.commits.to_string()),
                Cell::from(p.reviews.to_string()),
                Cell::from(p.comments.to_string()),
                Cell::from(p.participants.to_string()),
                Cell::from(ts(&app.zone, p.last_activity)),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(9),
            Constraint::Length(repo_w as u16),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(16),
        ],
        &[
            "State",
            "PR",
            "Title",
            "Events",
            "Yours",
            "Commits",
            "Reviews",
            "Comments",
            "People",
            "Last activity",
        ],
    )
    .block(block(title, true));
    stateful_table(f, app, area, list, t, true);
}

fn draw_pr_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_report() else {
        if app.pr_lookup_done() {
            return empty(
                f,
                area,
                "PR Detail".into(),
                &["This pull request is not in the cache. Press r to refresh or Esc to go back."],
            );
        }
        return loading(f, area, "PR Detail");
    };
    let z = &app.zone;
    let w = area.width.saturating_sub(26) as usize;
    let state = match (r.state, r.is_draft) {
        (PrState::Open, true) => "open (draft)".to_string(),
        (s, _) => s.as_str().to_string(),
    };
    let merged = match (&r.merged_at, &r.merged_by) {
        (Some(m), Some(b)) => format!("{} by {}", ts(z, Some(*m)), clean(b)),
        (Some(m), None) => format!("{} (merger unknown)", ts(z, Some(*m))),
        _ => "-".into(),
    };
    let lines = vec![
        Line::styled(
            fit(
                &format!("{}#{} {}", r.repository, r.number, r.title),
                area.width.saturating_sub(2) as usize,
            ),
            bold(),
        ),
        Line::raw(""),
        kv("Title", fit(&r.title, w)),
        kv("Repository", fit(&r.repository, w)),
        kv("Author", fit(&r.author, w)),
        kv("State (current)", state),
        kv("Created", ts(z, Some(r.created_at))),
        kv("Merged", fit(&merged, w)),
        kv(
            "Commits",
            if r.removed_commits > 0 {
                format!("{} ({} no longer in the PR)", r.commits, r.removed_commits)
            } else {
                r.commits.to_string()
            },
        ),
        kv(
            "Changes (now)",
            fit(
                &match &r.diff {
                    Some(d) => format!(
                        "+{} -{} in {} files (base to head as of {})",
                        d.additions,
                        d.deletions,
                        d.changed_files,
                        ts(z, Some(d.observed_at))
                    ),
                    None => "unknown (sync again to fetch it)".into(),
                },
                w,
            ),
        ),
        kv(
            "Reviews",
            format!(
                "{} (approved {}, changes requested {}, commented {})",
                r.reviews.total,
                r.reviews.approved,
                r.reviews.changes_requested,
                r.reviews.commented
            ),
        ),
        kv(
            "Comments",
            format!(
                "{} (conversation {}, inline {}, review summary {})",
                r.comments.total(),
                r.comments.conversation,
                r.comments.inline,
                r.comments.review_summary
            ),
        ),
        kv(
            "Review threads",
            format!(
                "{} (open {}, resolved {}, outdated {})",
                r.review_threads.total,
                r.review_threads.open,
                r.review_threads.resolved,
                r.review_threads.outdated
            ),
        ),
        kv(
            "Latest CI",
            r.latest_ci
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "-".into()),
        ),
        kv("Participants", fit(&r.participants.join(", "), w)),
        kv(
            "Time to first review",
            dur(r.timing.time_to_first_review_secs),
        ),
        kv("Creation to merge", dur(r.timing.creation_to_merge_secs)),
        kv(
            "Review → next author activity",
            dur(r.timing.review_to_next_author_activity_secs),
        ),
        kv(
            "First → last activity",
            dur(r.timing.first_to_last_activity_secs),
        ),
        kv("Timeline events", r.timeline.len().to_string()),
        kv(
            "Coverage",
            format!(
                "{} (synced {})",
                coverage_text(r.coverage),
                ts(z, r.synced_at)
            ),
        ),
        kv("URL", fit(&r.url, w)),
        Line::raw(""),
        Line::styled(
            match app.pr_pane {
                PrPane::Summary => "Enter: timeline · Tab: commits · o: open on GitHub · Esc: back",
                PrPane::Commits => {
                    "Enter: open commit · Tab: summary · o: open commit on GitHub · Esc: back"
                }
            },
            dim(),
        ),
    ];
    let focus_commits = app.pr_pane == PrPane::Commits;
    let h = area.height;
    let wanted = (r.commit_list.len() as u16).saturating_add(3).max(4);
    let list_h = if h >= 12 {
        let cap = if focus_commits { h - 4 } else { h / 2 };
        wanted.min(cap).max(4)
    } else if focus_commits {
        h
    } else {
        0
    };
    let [info_area, list_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(list_h)]).areas(area);
    if info_area.height > 0 {
        f.render_widget(
            Paragraph::new(lines).block(block("PR Detail".into(), !focus_commits)),
            info_area,
        );
    }
    if list_area.height > 0 {
        draw_pr_commits(f, app, r, list_area, focus_commits);
    }
}

fn draw_pr_commits(
    f: &mut Frame,
    app: &App,
    r: &ghusage_app::report::PrReport,
    area: Rect,
    focused: bool,
) {
    let list = ListId::PrCommits;
    let all = &r.commit_list;
    let removed = all.iter().filter(|c| c.removed).count();
    let mut title = format!("Commits ({}", all.len());
    if removed > 0 {
        title.push_str(&format!(", {removed} removed"));
    }
    title.push(')');
    title.push_str(if focused {
        " · Enter to open · o on GitHub"
    } else {
        " · Tab to select"
    });
    if all.is_empty() {
        return empty(
            f,
            area,
            title,
            &["No commits recorded for this pull request."],
        );
    }
    let vis = app.visible(list);
    if vis.is_empty() {
        return empty(f, area, title, &[&no_match(app, list)]);
    }
    let wide = area.width >= 70;
    let fixed: &[u16] = if wide { &[8, 7, 11, 14, 13] } else { &[8, 7] };
    let msg_w = flex_width(area, fixed);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let c = &all[i];
            let mark = if c.removed {
                "removed"
            } else if c.is_merge_commit {
                "merge"
            } else {
                ""
            };
            let mut cells = vec![
                Cell::from(ghusage_core::commit::short_sha(&c.sha).to_string()),
                Cell::from(mark),
            ];
            if wide {
                cells.push(Cell::from(app.zone.format(c.committed_at, "%m-%d %H:%M")));
                cells.push(Cell::from(fit(&c.author, 14)));
                cells.push(Cell::from(match c.lines {
                    Some(l) => format!("+{} -{}", l.additions, l.deletions),
                    None => "?".into(),
                }));
            }
            cells.push(Cell::from(fit(c.headline.as_deref().unwrap_or("-"), msg_w)));
            let row = Row::new(cells);
            if c.removed { row.style(dim()) } else { row }
        })
        .collect();
    let mut widths: Vec<Constraint> = fixed.iter().map(|w| Constraint::Length(*w)).collect();
    widths.push(Constraint::Min(8));
    let header: &[&str] = if wide {
        &["SHA", "", "Committed", "Author", "Lines", "Message"]
    } else {
        &["SHA", "", "Message"]
    };
    let t = styled_table(rows, widths, header).block(block(title, focused));
    stateful_table(f, app, area, list, t, focused);
}

fn draw_commit_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(key) = &app.detail_commit else {
        return empty(f, area, "Commit".into(), &["No commit selected."]);
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    app.commit_view_height.set(inner_h.max(1));
    let short = ghusage_core::commit::short_sha(&key.sha).to_string();
    if let Some(CommitState::Ready(v)) = app.commit_state() {
        let len = v.rows.len();
        let top = app.commit_scroll.min(len.saturating_sub(inner_h));
        let end = (top + inner_h).min(len);
        // Only the rows on screen are turned into lines.
        let lines: Vec<Line> = v.rows[top..end]
            .iter()
            .map(|r| doc_line(r, inner_w))
            .collect();
        let mut title = format!(
            "Commit {} · lines {}–{} of {len}",
            clean(&short),
            top + 1,
            end
        );
        match v.file_at(top) {
            Some(i) => title.push_str(&format!(" · file {}/{}", i + 1, v.file_rows.len())),
            None => title.push_str(&format!(" · {} file(s)", v.file_rows.len())),
        }
        f.render_widget(
            Paragraph::new(lines).block(block(fit(&title, inner_w.saturating_sub(4)), true)),
            area,
        );
        return;
    }
    // Not loaded (yet): show what the cached activity already tells.
    let seed = &app.commit_seed;
    let w = inner_w.saturating_sub(22);
    let mut lines = vec![
        Line::styled(
            fit(
                &format!(
                    "{}@{} — {}",
                    key.repository,
                    short,
                    seed.headline.as_deref().unwrap_or("")
                ),
                inner_w,
            ),
            bold(),
        ),
        Line::raw(""),
        kv("Repository", fit(&key.repository, w)),
        kv("SHA", fit(&key.sha, w)),
        kv("Author", fit(seed.author.as_deref().unwrap_or("-"), w)),
        kv("Date", ts(&app.zone, seed.date)),
    ];
    if seed.is_merge {
        lines.push(kv("Parents", "merge commit".into()));
    }
    if seed.removed {
        lines.push(kv(
            "In the pull request",
            "removed (no longer part of the pull request)".into(),
        ));
    }
    lines.push(Line::raw(""));
    match app.commit_state() {
        Some(CommitState::Failed(e)) if e.offline => {
            lines.push(Line::styled(
                "! Not available offline: this commit's files and diff are not in the cache.",
                bold(),
            ));
            lines.push(Line::raw(
                "Run ghusage tui without --offline (with GitHub reachable) and open it again, or press r to retry.",
            ));
            lines.push(Line::styled(clean(&e.message), dim()));
        }
        Some(CommitState::Failed(e)) => {
            lines.push(Line::styled("! Could not load this commit.", bold()));
            lines.push(Line::raw(clean(&e.message)));
            lines.push(Line::raw("Press r to try again or o to open it on GitHub."));
        }
        _ => {
            lines.push(Line::raw(
                "⧗ Loading the commit (from the cache, or from GitHub when online)…",
            ));
            lines.push(Line::styled(
                "Other screens and keys keep working meanwhile; Esc goes back.",
                dim(),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block(format!("Commit {}", clean(&short)), true)),
        area,
    );
}

fn draw_pr_timeline(f: &mut Frame, app: &App, area: Rect) {
    let Some(r) = app.pr_report() else {
        if !app.pr_lookup_done() {
            return loading(f, area, "PR Timeline");
        }
        return empty(
            f,
            area,
            "PR Timeline".into(),
            &["This pull request is not in the cache. Press Esc to go back."],
        );
    };
    let [tabs_area, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let mut spans = vec![Span::raw(" type (Tab): ")];
    for t in TimelineFilter::CYCLE {
        if t == app.timeline_filter {
            spans.push(Span::styled(format!("[{}]", t.label()), highlight()));
        } else {
            spans.push(Span::raw(format!(" {} ", t.label())));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    let list = ListId::Timeline;
    let vis = app.visible(list);
    let title = format!(
        "{}#{} timeline · {} of {} events",
        fit(&r.repository, 40),
        r.number,
        vis.len(),
        r.timeline.len()
    );
    if r.timeline.is_empty() {
        return empty(
            f,
            table_area,
            title,
            &["No events recorded for this pull request."],
        );
    }
    if vis.is_empty() {
        let msg = if app.search_query(list).is_empty() {
            "No events of this type. Press Tab to change the type.".to_string()
        } else {
            no_match(app, list)
        };
        return empty(f, table_area, title, &[&msg]);
    }
    let detail_w = flex_width(table_area, &[19, 24, 22]);
    let rows: Vec<Row> = vis
        .iter()
        .map(|&i| {
            let e = &r.timeline[i];
            let mut actor = e.actor.clone();
            if e.actor_kind == ActorKind::Bot && !actor.ends_with("[bot]") {
                actor.push_str(" (bot)");
            }
            let mut detail = e.detail.clone().unwrap_or_default();
            if e.removed_observed_at.is_some() && !detail.contains("no longer present") {
                detail.push_str(" (removed)");
            }
            Row::new(vec![
                app.zone.format(e.occurred_at, "%Y-%m-%d %H:%M:%S"),
                fit(&actor, 24),
                e.label.clone(),
                fit(&detail, detail_w),
            ])
        })
        .collect();
    let t = styled_table(
        rows,
        vec![
            Constraint::Length(19),
            Constraint::Length(24),
            Constraint::Length(22),
            Constraint::Min(8),
        ],
        &["Time", "Actor", "Event", "Detail"],
    )
    .block(block(title, true));
    stateful_table(f, app, table_area, list, t, true);
}
