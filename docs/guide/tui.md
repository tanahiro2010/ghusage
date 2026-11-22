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
`--visibility`, `--ownership`, `--user`, `--public-only`, `--commit-date`,
`--data-dir` and `--concurrency` apply. Unlike JSON and CSV output, the UI
shows private repository names, like table output.

## Screens

| Key | Screen | Contents |
|---|---|---|
| `1` | Dashboard | Everything at a glance: commits, PRs created and merged, reviews submitted and received, comments written, issues opened and active days; an activity-per-day chart; the top repositories; the pull requests you were most recently active in; and the coverage of the period |
| `2` | Overview | Key numbers (commits, PRs created and merged, reviews submitted and received, comments written, active PRs, open PRs now, issues opened, active days, lines added and lines deleted), an activity-per-day chart, and the top repositories |
| `3` | Daily | One row per day (newest first) with commits, PRs, merged, reviews, received, comments, PR commits, issues, active PRs, and lines added and deleted (+Lines, -Lines) |
| | Day Detail | Opened with `Enter` on a day: that day's numbers and what happened, in sections: activity, commits, pull requests, reviews (given and received), comments (written and received), issues |
| `4` | Repositories | Activity per repository, with visibility and owned/external, including lines added and deleted (+Lines, -Lines) |
| | Repository Detail | Opened with `Enter` on a repository: its numbers for the period and, in sections, its pull requests, commits, issues, reviews, comments and activity |
| `5` | Pull Requests | Pull requests you were active in during the period: state, title, events, your events, commits, reviews, comments, people, last activity |
| | PR Detail | Opened with `Enter` on a pull request: state, author, merge, commits, current changes, reviews, comments, review threads, latest CI, participants, timings, coverage, URL, and the list of the pull request's commits |
| | PR Timeline | Opened with `Enter` on PR Detail: every event of the pull request in order, filterable by event category |
| | Commit | Opened with `Enter` on a commit (PR Detail commit list, or the Commits section of a day or repository detail): the commit's facts, message, changed files and diff |
| `6` | Reviews | Reviews you submitted and reviews on your pull requests. `Enter` opens the reviewed pull request |
| `7` | Issues | Issues you opened, closed or reopened during the period |
| | Issue Detail | Opened with `Enter` on an issue: title, repository, author, current state, created and closed times, and the issue's events in the period |
| `8` | System | Account, online/offline, credential source, rate limit, last sync, result of this session's syncs, cache location, size and counts, coverage and gaps per data kind, warnings and errors |

The second line shows the numbered tabs and, on a detail screen, the path
that led to it (for example `› Day Detail › PR Detail`). On narrow terminals
the tab titles are shortened, and below about 75 columns only the current
tab keeps its title.

The status bar shows whether the data comes from the cache, the coverage of
the data on screen, the last sync time, background sync progress or errors,
and messages from the last key press. On wide terminals it also shows the
main keys of the current screen.

Activity in the Dashboard and Overview charts is the number of your commits,
your events inside pull requests, and your issue events per day.

### Line counts

Lines added and deleted use the same
[definition](../reference/metrics.md#lines-changed) as the CLI:

- Overview shows **Lines added** and **Lines deleted** for the period.
- Daily and Repositories have **+Lines** and **-Lines** columns. As in table
  output, `~N` is a sum over only some of the commits and `?` means that no
  commit has known line counts; run `ghusage sync --diff-stats` to fetch the
  missing ones. On narrow terminals, Repositories hides these columns early
  (see [Known limitations](../reference/limitations.md#terminal-ui)).
- PR Detail shows **Changes (now)**: the pull request's base-to-head diff as
  of the last sync (`+A -D in F files (base to head as of <time>)`), a
  snapshot rather than changes in the period.
- The commit list of PR Detail has a **Lines** column (`+A -D`, `?` when not
  known) on terminals at least 70 columns wide.

Line counts are easily dominated by generated files, formatting changes,
dependency updates and moved files, and are not a measure of how much or
how well someone worked.

### Dashboard

The Dashboard opens first. It adapts to the terminal size: at 80×24 it shows
the figures, the coverage line, the chart and both lists side by side; on
narrower terminals the lists are stacked, and when there is not enough room
the chart and then the lists are left out (only the list in focus is shown
when both do not fit). The coverage line starts with `✓` when every data
kind is complete for the period and `!` otherwise, and names the kinds that
are partial or not yet synced.

`Tab` moves the focus between the figures, the repository list and the pull
request list. `Enter` on a repository opens its Repository Detail; on a pull
request it opens its PR Detail.

### Day and repository details

The numbers at the top of a day or repository detail are the same numbers as
the Daily row or Repositories row they were opened from. `Tab` and
`Shift+Tab` switch between the sections; the number of rows of each section
is shown next to its name. In each section:

- Activity lists your own events (commits, pull request events, reviews,
  comments, issue events) and the reviews and comments others left on your
  pull requests, newest first. CI results are not listed. Rows marked
  `(you)` are your own.
- Commits lists your commits, counted once each, newest first, with the date
  chosen by `--commit-date`.
- Pull requests, Reviews, Comments and Issues use the same rows as the CLI's
  `prs`, `reviews`, `comments` and `issues` commands for that day or
  repository.

`Enter` opens the pull request, issue or commit behind the selected row.
Changing the time range with `t`
or the repository filter with `f` leaves a day detail, because the day may
not be part of the new period; repository and issue details stay open and
follow the new period.

The Issue Detail lists the issue's events within the selected period only.

### PR Detail commits

Below the pull request's facts, PR Detail lists its commits, oldest first:
short SHA, the first line of the message, author, commit time and lines
added and deleted. Commits
that were part of the pull request earlier but no longer are (for example
after a force push) are still listed, dimmed and marked `removed`; merge
commits are marked `merge`. The title shows how many commits there are and
how many were removed. Commit messages are recorded by `sync`; for pull
requests synced with an older version the message column shows `-` until
the pull request is synced again.

`Tab` (or `Shift+Tab`) moves the focus between the facts and the commit
list. With the facts in focus, `Enter` opens the PR Timeline and `o` the
pull request, as before. With the commit list in focus, `j`/`k` select a
commit, `/` searches the list (SHA, message, author), `Enter` opens the
Commit screen and `o` opens the commit on GitHub.

### Commit

The Commit screen shows, in one scrollable page:

- the repository, full SHA, author and committer (git name and GitHub
  login), author date and committer date, the parent commits (marked
  `merge commit` when there is more than one) and the number of changed
  files and lines;
- the full commit message;
- the changed files with their status (`added`, `modified`, `removed`,
  `renamed` with the old path, ...), lines added and removed, and a total;
- the diff of every file, each starting with a header line
  `── [n/total] status path (+added −removed)`. Added lines are green,
  removed lines red, hunk headers (`@@ ... @@`) cyan and file headers
  yellow; the `+`, `-` and `@@` prefixes stay, so nothing depends on color
  alone.

The commit is loaded when the screen opens: from the cache if it was opened
before, otherwise from GitHub (`GET /repos/{owner}/{repo}/commits/{sha}`,
every page of files), on a separate thread so the UI keeps responding. While
it loads, the screen shows what the cache already knows (SHA, message
headline, author, date, whether it was removed from the pull request) and
"Loading the commit"; the status bar shows `⧗ loading commit`. A fetched
commit is stored in the cache and can be shown again offline.

With `--offline`, or when GitHub cannot be reached, a commit that is not in
the cache cannot be shown: the screen says "Not available offline". If
loading fails (for example the commit was deleted after a force push, or
access is denied), the screen says why; press `r` to try again or `o` to open
it on GitHub. With `--public-only`, commits of repositories that are not
public are not loaded.

GitHub does not return a diff for binary files or very large diffs; such
files say "no diff available from GitHub". To keep the screen fast, at most
3000 diff lines per file and 20000 per commit are shown; a note says how
many lines were left out (`o` opens the full diff on GitHub). Only the rows
on screen are drawn, so scrolling stays fast on large commits.

| Key | Action |
|---|---|
| `j` / `Down`, `k` / `Up` | Scroll one line |
| `PageDown` / `Space`, `PageUp` | Scroll one screen |
| `g` / `Home`, `G` / `End` | Top, bottom |
| `]` | Next file's diff |
| `[` | Previous file's diff (from the first file: back to the top) |
| `o` | Open the commit on GitHub |
| `r` | Load again after a failure (otherwise: reload and sync, as elsewhere) |
| `Esc` | Back to the screen it was opened from |

The block title shows the visible lines and which file's diff is at the top
(`file 2/5`).

## Keys

### Navigation

| Key | Action |
|---|---|
| `1`–`8` | Switch to Dashboard, Overview, Daily, Repositories, Pull Requests, Reviews, Issues, System |
| `j` / `Down` | Next row |
| `k` / `Up` | Previous row |
| `PageDown` / `PageUp` | Move 10 rows |
| `g` / `Home` | First row |
| `G` / `End` | Last row |
| `Enter` | Open the detail of the selected row: Daily → Day Detail; Repositories (and the repository panes of Dashboard and Overview) → Repository Detail; Pull Requests (and the pull request pane of Dashboard) → PR Detail → PR Timeline (or, with the commit list in focus, Commit); Reviews → PR Detail of the reviewed pull request; Issues → Issue Detail; a row of a day or repository detail → its PR Detail, Issue Detail or Commit |
| `Esc` | On a detail screen: back to the screen it was opened from. Elsewhere: clear the current search |
| `Tab` / `Shift+Tab` | Dashboard: move the focus between figures, repositories and pull requests. Overview: switch between the summary and the repository pane. Reviews: switch between submitted and received. Day and repository details: next / previous section. PR Detail: switch between the facts and the commit list. PR Timeline: next / previous event category filter |

Selection stops at the first and last row; it does not wrap around.

### Search

| Key | Action |
|---|---|
| `/` | Start searching the current list (Daily, Repositories, Pull Requests, Reviews, Issues, the sections of day and repository details, Issue Detail, PR Timeline, the commit list of PR Detail) |
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
- Issues: title, repository, `#number`;
- Day and repository details: activity by actor, event name, repository
  and title; commits by SHA and repository; comments by author, repository
  and kind; pull requests, reviews and issues as in their own lists;
- Issue Detail: actor and event name;
- PR Timeline: actor, event name, detail;
- PR Detail commit list: SHA, message, author.

### Data and display

| Key | Action |
|---|---|
| `r` | Reload from the cache. When online, also start a background sync of the current period (only one sync runs at a time) |
| `t` | Cycle the time range: as started → today → last 7 days → last 30 days → last 90 days → this month |
| `f` | Cycle the repository filter: all → public → private → owned → external (applied on top of the command-line filters) |
| `o` | Open the selected item (repository, pull request, review, issue, commit, comment, timeline event) in the browser |
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
- Reports are computed in the background. With a very large cache, a screen
  opened for the first time after changing the period or filter shows
  "Computing…" (and the status bar shows `⧗ computing`) for a moment; other
  screens and keys keep working meanwhile.
- Terminals smaller than 20 columns by 6 rows show "terminal too small".
- The terminal UI has been exercised on macOS. Linux and Windows terminals
  (including opening links) have not yet been verified by hand.
