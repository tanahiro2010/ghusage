# CLI reference

```text
ghusage [OPTIONS] [COMMAND]
```

Without a command, `ghusage` runs `overview`. Global options can be placed
before or after the command (`ghusage --since 30d daily` and
`ghusage daily --since 30d` are the same).

## Commands

| Command | Description | Default period |
|---|---|---|
| `overview` | Overview of the period (default command) | last 7 days |
| `daily` | Activity per day | last 7 days |
| `weekly` | Activity per week (weeks start on Monday) | last 8 weeks |
| `monthly` | Activity per calendar month | last 6 months |
| `yearly` | Activity per calendar year | last 3 years |
| `repos` | Activity per repository | last 7 days |
| `prs` | Pull requests with activity in the period | last 7 days |
| `reviews` | Reviews you submitted and reviews you received | last 7 days |
| `comments` | Comments you wrote and comments on your pull requests | last 7 days |
| `issues` | Issues you opened, closed or reopened | last 7 days |
| `pr <URL>` | Detail and timeline of one pull request | (not period based) |
| `tui` | Interactive terminal UI | last 30 days |
| `sync` | Fetch data from GitHub into the local cache | last 30 days |
| `doctor` | Check authentication, cache and connectivity | |
| `auth status` | Show which credentials are used and whether they work | |
| `cache status` | Show cache location, size, last sync and coverage | |
| `cache clear` | Delete all cached GitHub data (credentials are not affected) | |

"Last 7 days" means today and the 6 days before it, starting at local
midnight and ending now. "Last 8 weeks" starts on the Monday 7 weeks before
the current week; "last 6 months" starts on the first day of the month 5
months before the current one. See [Periods and time zones](./time.md).

### Report commands

`overview`, `daily`, `weekly`, `monthly`, `yearly`, `repos`, `prs`, `reviews`,
`comments` and `issues` sync missing ranges of the period (unless
`--offline`), then print the report followed by its coverage. See the
[metric definitions](./metrics.md) for what each number means.

- `overview`: the summary of the period and the top 5 repositories.
- `daily` / `weekly` / `monthly` / `yearly`: one row per calendar bucket and a total
  row. Weeks are labeled with ISO week numbers (`2026-W47`), months as
  `2026-11`.
- `repos`: one row per repository with activity in the period, ordered by
  your activity. `*` after the visibility marks repositories you own;
  `(no access)` marks repositories that GitHub refused (HTTP 403 or 404)
  during a sync. Their cached data is kept.
- `reviews`: counts by state, then the reviews you submitted and the reviews
  others submitted on your pull requests.
- `comments`: counts by type, then each comment you wrote or received on your
  pull requests.
- `issues`: counts, the number of open issues you authored (current state),
  and each issue you opened, closed or reopened in the period.

### `prs`

```text
ghusage prs [--details] [--sort <SORT>] [--state <STATE>] [--all-participants]
```

Lists pull requests with activity in the period.

| Option | Description |
|---|---|
| `--details` | Also show activity statistics per pull request and lifecycle timings of your pull requests |
| `--sort <SORT>` | `activity` (default; events in the period), `comments`, `reviews`, `created` (creation time), `updated` (last activity in the period). Always largest / newest first |
| `--state <STATE>` | Only pull requests currently `open`, `closed` (closed without merge) or `merged` |
| `--all-participants` | Include pull requests where only other people were active in the period |

By default only pull requests in which you had at least one event in the
period are listed. Column meanings are in
[Metric definitions](./metrics.md#pull-request-list).

### `pr`

```text
ghusage pr <URL> [--timeline] [--event-type <TYPES>]...
```

Shows one pull request: state, author, merge, commits (and commits that are
no longer in the pull request), reviews by state, comments by type, review
threads (current state), latest CI result, participants, timings and
coverage.

| Option | Description |
|---|---|
| `<URL>` | `https://github.com/OWNER/REPO/pull/NUMBER`. Trailing paths such as `/files`, a query string or a fragment are accepted |
| `--timeline` | Show every event in order (table output). JSON and CSV always contain the timeline |
| `--event-type <TYPES>` | Only these event types in the timeline. Comma-separated and/or repeated. Counts are not affected |

When online, the pull request is always fetched again before it is shown.
If that fails and the pull request is in the cache, a warning is printed and
the cached data is used. With `--offline`, the pull request must already be
in the cache. Looking at a pull request you are not involved in does not add
it to your reports.

Event types: `commit_authored`, `pr_opened`, `pr_converted_to_draft`,
`pr_ready_for_review`, `pr_closed`, `pr_reopened`, `pr_merged`, `pr_commit`,
`pr_head_force_pushed`, `review_submitted`, `review_dismissed`,
`conversation_comment`, `inline_comment`, `review_summary`,
`review_requested`, `review_request_removed`, `assigned`, `unassigned`,
`labeled`, `unlabeled`, `milestoned`, `demilestoned`, `renamed`,
`issue_opened`, `issue_closed`, `issue_reopened`, `check_run`,
`commit_status`. (The older name `commit` is still accepted for
`commit_authored`.)

```sh
ghusage pr https://github.com/OWNER/REPO/pull/123 --timeline \
  --event-type review_submitted,inline_comment
```

### `sync`

```text
ghusage sync [--full]
```

Fetches the period into the cache. Without `--full`, only ranges that are not
yet complete are fetched; `--full` refetches the whole period. Cannot be
combined with `--offline`. See [Syncing and the cache](../guide/syncing.md).

Filters (`--repo`, `--owner`, `--visibility`, `--ownership`) do not restrict
what `sync` fetches.

### `tui`

Opens the interactive terminal UI. See [Terminal UI](../guide/tui.md).

### `doctor`

Checks that the cache can be opened and passes an integrity check, shows the
last sync, and (unless `--offline`) that credentials are found, accepted by
GitHub, and that rate limit is left. Exits with 1 if any check fails.

```text
$ ghusage doctor --offline
ok   cache opened: /Users/me/Library/Application Support/dev.ghusage.ghusage/ghusage.db
ok   cache integrity: ok
ok   offline: GitHub checks skipped
```

### `auth status`

Shows the login, the credential source, token scopes (when reported) and rate
limits. Fails with exit code 3 if no credentials are found or GitHub rejects
them.

### `cache status` / `cache clear`

`cache status` shows the database path and size, schema version, the number
of repositories, pull requests, issues and events, the last sync, and the
coverage of the last 30 days. `cache clear` deletes all cached data.

```text
$ ghusage cache status
Cache: /Users/me/Library/Application Support/dev.ghusage.ghusage/ghusage.db
  size: 0.2 MiB, schema version 1
  repositories 0, pull requests 0, issues 0, events 0
  last sync: never
```

## Global options

| Option | Description |
|---|---|
| `--since <SINCE>` | Start of the period: `YYYY-MM-DD`, RFC 3339, `today`, `yesterday`, or `Nd` / `Nw` / `Nm` / `Ny` (last N days / weeks / months / years) |
| `--until <UNTIL>` | End of the period (exclusive). A date includes that whole day. Defaults to now. `Nd`/`Nw`/`Nm`/`Ny` are not accepted here |
| `--timezone <TZ>` | IANA time zone for day / week / month boundaries (default: `TZ`, then the system setting, then UTC) |
| `--repo <OWNER/NAME>` | Only this repository (case-insensitive). Repeatable |
| `--owner <LOGIN>` | Only repositories owned by this user or organization. Repeatable |
| `--visibility <public\|private>` | Only public, or only non-public (private and internal) repositories |
| `--ownership <owned\|external>` | Only repositories you own, or only other repositories |
| `--user <USER>` | Analyze another user's activity (limited to what your token can see) |
| `--format <table\|json\|csv>` | Output format (default `table`). `csv` is only available for reports and `pr`; `sync`, `doctor`, `auth`, `cache` and `tui` reject it with exit code 2 |
| `--offline` | Use only cached data; never contact GitHub |
| `--no-cache` | Fetch fresh data into a temporary cache instead of the local cache |
| `--require-complete` | Exit with code 6 if any data for the period is incomplete |
| `--no-color` | Disable colors (also honors `NO_COLOR`) |
| `--include-private` | Include private repository names and titles in JSON / CSV output |
| `--public-only` | Only fetch and show public repositories |
| `--commit-date <committer\|author>` | Which commit date decides a commit's day and whether it is in the period (default `committer`). Also used by the terminal UI |
| `--token-file <PATH>` | Read the GitHub token from this file (must not be readable by others) |
| `--data-dir <PATH>` | Directory for the cache database (default: OS user data directory, or `GHUSAGE_DATA_DIR`) |
| `--concurrency <N>` | Maximum concurrent GitHub requests, 1–16 (default 8) |
| `--max-age <AGE>` | Reports skip syncing when the cache was synced this recently, and show the data as of that sync: `30s`, `5m`, `1h`, or `0` to always fetch the latest (default `5m`, or `GHUSAGE_MAX_AGE`) |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

Filters combine with AND: a repository must match every given filter. Within
`--repo` or `--owner`, repeated values combine with OR. "Owned" means the
repository's owner is you, or you have admin permission on it (when analyzing
yourself). Being a member of the owning organization does not make a
repository owned.

Global options are accepted by every command, but not every command uses
them. The period (`--since`, `--until`) is used by reports, `sync` and
`tui`. The filters (`--repo`, `--owner`, `--visibility`, `--ownership`) and
`--commit-date` are used by reports and `tui` only. When a command is given
one of these options and does not use it, `ghusage` prints
a warning such as `` warning: --repo has no effect on `sync` `` to stderr and
continues.

## Output and terminals

- Reports go to stdout. Errors, warnings, notes and sync progress go to
  stderr.
- Colors are used only when stdout is a terminal, `--no-color` is not given,
  and `NO_COLOR` is not set to a non-empty value. Piped output never contains
  escape sequences.
- Sync progress is shown only when stderr is a terminal.
- Text from GitHub (titles, logins, names) is stripped of control characters
  before it is printed.
- A closed pipe (for example `ghusage daily | head -3`) is not treated as an
  error.
- Table output is meant for people and may change between versions. Use
  `--format json` or `--format csv` for scripts; see
  [JSON and CSV output](./json.md).

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | General error (for example: no cached account in offline mode, pull request not in the cache, a failed `doctor` check, terminal error) |
| `2` | Invalid arguments (unknown option or value, invalid date or time zone, empty period, invalid pull request URL, unknown event type, `sync --offline`, `--format csv` on a command without CSV output) |
| `3` | Authentication error (no credentials, unusable token file, token rejected by GitHub) |
| `4` | GitHub API error (network, rate limit, server errors); also `sync` when some data could not be fetched completely. Report commands do not fail when GitHub is unreachable and an account is cached: they warn and show cached data |
| `5` | Cache error (cannot create or open the database, corrupt or too-new database, another sync is running) |
| `6` | Incomplete data with `--require-complete` (the report is still printed) |

`--help` and `--version` exit with 0.

Examples, as printed by the current version:

```text
$ ghusage --since 0d
error: invalid date or time: 0d (use YYYY-MM-DD, RFC 3339, today, yesterday, or Nd/Nw/Nm/Ny)
$ echo $?
2

$ ghusage --timezone Mars/Phobos
error: unknown time zone: Mars/Phobos

$ ghusage sync --offline
error: sync cannot run with --offline

$ ghusage --offline --no-cache
error: the argument '--offline' cannot be used with '--no-cache'
```

## Environment variables

| Variable | Description |
|---|---|
| `GH_TOKEN` | GitHub token (second in priority after `--token-file`) |
| `GITHUB_TOKEN` | GitHub token (used when `GH_TOKEN` is not set) |
| `GHUSAGE_DATA_DIR` | Data directory for the cache (overridden by `--data-dir`) |
| `GHUSAGE_MAX_AGE` | Default for `--max-age` |
| `GHUSAGE_API_URL` | REST API base URL, for GitHub Enterprise Server (default `https://api.github.com`). Also moves the GraphQL endpoint: `https://HOST/api/graphql` when the URL ends in `/api/v3`, otherwise `<GHUSAGE_API_URL>/graphql` |
| `GHUSAGE_GRAPHQL_URL` | GraphQL endpoint URL (overrides the one derived from `GHUSAGE_API_URL`) |
| `GHUSAGE_LOG` | Log filter for diagnostics on stderr, in `tracing` filter syntax, e.g. `debug` or `ghusage_github=debug` (default `warn`) |
| `TZ` | Default time zone when `--timezone` is not given (an IANA name such as `Europe/Berlin`) |
| `NO_COLOR` | Disable colors when set to a non-empty value |

For GitHub Enterprise Server, see
[Authentication](../guide/authentication.md#github-enterprise-server).
