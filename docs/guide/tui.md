# Terminal UI

```sh
ghusage tui                 # last 30 days, syncing in the background
ghusage tui --since 90d
ghusage tui --offline       # cache only
ghusage tui --owner my-org  # start with a repository filter
```

The terminal UI shows the same numbers as the CLI; both are computed by the
same code from the local cache. The UI reads only the cache and never waits
for the network. When started without `--offline`, it starts a sync of the
initial period in the background and reloads automatically when the sync
finishes. Progress, errors and the last sync time are shown in the status
bar.

On the very first run with an empty cache, `ghusage tui` contacts GitHub once
to find out who you are before the UI opens. With `--offline` and an empty
cache it fails unless `--user` is given.

The global options `--since`, `--until`, `--timezone`, `--repo`, `--owner`,
`--visibility`, `--ownership`, `--user`, `--public-only`, `--data-dir` and
`--concurrency` apply. Unlike JSON and CSV output, the UI shows private
repository names, like table output. Commit counts always use the committer
date; `--commit-date author` is not applied in the UI.

## Screens

| Key | Screen | Contents |
|---|---|---|
| `1` | Overview | Key numbers (commits, PRs created and merged, reviews submitted and received, comments written, active PRs, open PRs now, issues opened, active days), an activity-per-day chart, and the top repositories |
| `2` | Daily | One row per day (newest first) with commits, PRs, merged, reviews, received, comments, PR commits, issues and active PRs |
| `3` | Repositories | Activity per repository, with visibility and owned/external |
| `4` | Pull Requests | Pull requests you were active in during the period: state, title, events, your events, commits, reviews, comments, people, last activity |
| | PR Detail | Opened with `Enter` on a pull request: state, author, merge, commits, reviews, comments, review threads, latest CI, participants, timings, coverage, URL |
| | PR Timeline | Opened with `Enter` on PR Detail: every event of the pull request in order, filterable by event category |
| `5` | Reviews | Reviews you submitted and reviews on your pull requests |
| `6` | System | Account, online/offline, credential source, rate limit, last sync, result of this session's syncs, cache location, size and counts, coverage and gaps per data kind, warnings and errors |

The status bar shows whether the data comes from the cache, the coverage of
the data on screen, the last sync time, background sync progress or errors,
and messages from the last key press.

Activity in the Overview chart is the number of your commits, your events
inside pull requests, and your issue events per day.

## Keys

### Navigation

| Key | Action |
|---|---|
| `1`–`6` | Switch to Overview, Daily, Repositories, Pull Requests, Reviews, System |
| `j` / `Down` | Next row |
| `k` / `Up` | Previous row |
| `PageDown` / `PageUp` | Move 10 rows |
| `g` / `Home` | First row |
| `G` / `End` | Last row |
| `Enter` | Pull Requests: open PR Detail. PR Detail: open PR Timeline. Repositories (and the repository pane of Overview): show the pull requests of the selected repository |
| `Esc` | PR Timeline: back to PR Detail. PR Detail: back to Pull Requests. Elsewhere: clear the current search |
| `Tab` / `Shift+Tab` | Overview: switch between the summary and the repository pane. Reviews: switch between submitted and received. PR Timeline: next / previous event category filter |

Selection stops at the first and last row; it does not wrap around.

### Search

| Key | Action |
|---|---|
| `/` | Start searching the current list (Daily, Repositories, Pull Requests, Reviews, PR Timeline) |
| any character | Add to the search text; the list is filtered as you type (case-insensitive) |
| `Backspace` | Delete the last character |
| `Up` / `Down` | Move the selection while searching |
| `Enter` | Keep the search and leave search mode |
| `Esc` | Clear the search and leave search mode |

What a search matches:

- Daily: the date;
- Repositories: the repository name;
- Pull Requests: title, repository, `#number`, author;
- Reviews: reviewer, repository, pull request title;
- PR Timeline: actor, event name, detail.

### Data and display

| Key | Action |
|---|---|
| `r` | Reload from the cache. When online, also start a background sync of the current period (only one sync runs at a time) |
| `t` | Cycle the time range: as started → today → last 7 days → last 30 days → last 90 days → this month |
| `f` | Cycle the repository filter: all → public → private → owned → external (applied on top of the command-line filters) |
| `o` | Open the selected item (repository, pull request, review, timeline event) in the browser |
| `?` | Show or hide help |
| `q` / `Ctrl+C` | Quit |

While the help is shown, `?` or `Esc` closes it, `q` and `Ctrl+C` quit, and
other keys are ignored.

`t` changes the period of the reports immediately, using the cache only. If
the new range has not been synced, the coverage in the status bar says so;
press `r` to sync it.

PR Timeline filters (cycled with `Tab`): All, Commits, Reviews, Comments,
Lifecycle, Coordination, Metadata, CI.

### Opening links

`o` only opens `https://github.com` URLs that pass validation. It uses `open`
on macOS, `xdg-open` on Linux and `cmd /C start` on Windows, without going
through a shell command string. On Windows, URLs containing shell
metacharacters are refused.

## Notes

- Quitting while a background sync runs stops the sync. Everything written
  so far is kept, and the next sync continues from there.
- Very large caches may take a moment to compute a screen the first time it
  is opened after changing the period or filter.
- Terminals smaller than 20 columns by 6 rows show "terminal too small".
- The terminal UI has been exercised on macOS. Linux and Windows terminals
  (including opening links) have not yet been verified by hand.
