# Syncing and the local cache

`ghusage` never computes reports directly from API responses. It first stores
GitHub data in a local SQLite database (the cache) and then builds every
report, in every format and in the terminal UI, from that cache.

## What a sync fetches

For the requested period, a sync fetches three kinds of data:

| Data kind | How it is found | What is stored |
|---|---|---|
| `commits` | Commits whose author is your account, by committer date: GitHub commit search, plus the default-branch commit list of each of your repositories pushed to in the period and each repository with your commits or pull requests | Commits, plus your commits found inside synced pull requests |
| `pull_requests` | Issue search: pull requests that `involves:` you or are `reviewed-by:` you, updated in the period | Each pull request with all of its commits, reviews, review threads and their comments, conversation comments, timeline events, and the CI results of its head commit |
| `issues` | Issue search: issues that `involves:` you, updated in the period | Each issue with its open/close/reopen history |

Each pull request and issue is written in its own transaction, so an
interrupted sync keeps everything written so far and the next run continues
with what is missing.

### How commits are found

GitHub's commit search index can silently miss commits, even in repositories
it has indexed. `ghusage` therefore also lists commits directly:

1. It lists your repositories that were pushed to since the start of the
   missing range (`GET /user/repos`, newest push first, stopping at the first older
   repository). With `--user`, only the other user's own repositories are
   visible (`GET /users/{login}/repos`).
2. It adds repositories in which commit search found your commits and
   repositories of the pull requests found by the sync.
3. For each of these repositories (forks excluded, and private ones
   excluded with `--public-only`) it lists your commits on the default
   branch in the range (`GET /repos/{owner}/{repo}/commits?author=...`).

A commit found by search, by a listing and inside a pull request is stored
once. This costs at least one request per repository, so the number of API
requests grows with the number of repositories you were active in.
Commits coverage is `complete` only when commit search and every listing
succeeded. Commits coverage recorded by older versions, which relied on
commit search alone, is not reused: the next sync fetches those ranges
again.

## Running a sync

```sh
ghusage sync                       # last 30 days
ghusage sync --since 2026-01-01    # from a date until now
ghusage sync --since 90d --full    # refetch everything in the period
```

```text
Synced octocat for 2026-10-23 00:00 → 2026-11-21 19:30
  commits found: 690
  pull requests found: 83 (fetched 83, unchanged 0)
  issues found: 19 (fetched 19)
  API requests: 279 (retries 0, 40812 ms)
  status: complete
```

Progress is printed to stderr when stderr is a terminal. `--format json`
prints the sync result as JSON (see [JSON output](../reference/json.md#sync)).

Exit codes of `sync`:

- `0`: everything was fetched completely;
- `4`: the sync finished but some data could not be fetched completely
  (failed or partially fetched pull requests or issues, repositories whose
  commits could not be listed, search caps);
  the `incomplete:` lines explain what is missing;
- `6` instead of `4` when `--require-complete` is given;
- `3`: authentication failed; `5`: cache error (including another sync
  running).

## Automatic sync in report commands

Report commands (`overview`, `daily`, `prs`, ...) check the coverage of the
requested period first. If any data kind has a gap, they sync the missing
ranges and then show the report. If that sync fails for a reason other than
authentication, they print a warning and show what is in the cache:

```text
warning: sync failed, showing cached data: ...
```

Without `--offline`, report commands always resolve your account through the
API first, even when the period is fully cached. On a machine without network
access they therefore fail (exit code 4); use `--offline` there.

When the whole period is already covered, report commands do not fetch any
activity data. To pick up changes in a period that is already covered (for
example a pull request that was merged since), run `ghusage sync` or
`ghusage sync --full` for that period.

## Incremental sync

The cache remembers, per account and data kind, which time ranges were
fetched and how completely. This is the coverage of the cache.

- A normal sync only fetches ranges that are not yet `complete`. Each missing
  range is extended 3 days backwards to pick up items GitHub indexed late.
- Pull requests whose `updated_at` has not changed since they were last
  fetched completely are skipped (`unchanged` in the sync output).
- Open pull requests that have not been refreshed for 7 days are refetched
  even if search does not report them as updated.
- `--full` ignores existing coverage and refetches the whole period,
  including unchanged pull requests.

### How search periods are handled

GitHub's `updated:` search qualifier matches a pull request's or issue's
*current* update time. A pull request that was active last month and updated
again today would not be found by a search for last month. To compensate,
`ghusage` searches pull requests and issues from the start of the missing
range up to now, and then counts each event by the time it actually
happened.

GitHub returns at most 1000 results per search query. When a query has more,
`ghusage` splits the time range in halves until each part fits. If even a
one-second range has more than 1000 results, only the first 1000 are
available and the range is marked `partial`.

## Coverage

Every report shows coverage per data kind for the requested period:

| Status | Meaning |
|---|---|
| `complete` | Everything in scope was fetched for the whole range. |
| `partial` | Something is missing: a search hit a cap, GitHub reported incomplete results, a repository's commits could not be listed, a pull request or issue failed or was only partially fetched. The reason is shown. |
| `not_yet_synced` | The range has never been synced for this account. |
| `not_available` | The data cannot be obtained. |
| `unknown` | The status could not be determined. |

The overall status of a report is `complete` only if every data kind is
`complete` for every instant of the period. When a range was synced several
times, the most recent sync decides its status.

"Complete" always refers to a stated scope. For commits, for example, the
scope is "commits authored by you on the default branches of repositories you
pushed to or contributed to (repository listing + commit search), plus
commits of synced pull requests". The
scope strings are included in JSON output. See
[Known limitations](../reference/limitations.md) for what lies outside these
scopes.

### `--require-complete`

With `--require-complete`, any report whose overall coverage is not
`complete` exits with code 6 after printing the report:

```text
error: data for the requested period is not_yet_synced (--require-complete)
```

For `ghusage pr`, the coverage of that pull request is used.

## `--offline`

`--offline` uses only the cache and never contacts GitHub (no credentials are
needed). It cannot be combined with `sync` or `--no-cache`.

- Ranges that were never synced show as `not_yet_synced`; for table output a
  note is printed to stderr:
  `note: offline mode; some of the period has not been synced`.
- Offline mode needs to know who "you" are. It uses the account of the last
  online run, or `--user`. With an empty cache and no `--user`, it fails:
  `error: no cached data for the authenticated user; run `ghusage sync` online first or pass --user`.
- `ghusage pr --offline` only works for pull requests already in the cache.

## `--no-cache`

`--no-cache` fetches the requested period into a temporary in-memory database
that is discarded when the command exits. The local cache is neither read
nor written. Reports are computed exactly the same way. Because nothing is
reused, every run fetches the whole period again.

## Data directory

The cache is a single SQLite file, `ghusage.db`, in the data directory. The
data directory is chosen in this order:

1. `--data-dir <PATH>`
2. the `GHUSAGE_DATA_DIR` environment variable (if not empty)
3. the operating system's user data directory:

| OS | Default data directory |
|---|---|
| macOS | `~/Library/Application Support/dev.ghusage.ghusage` |
| Linux | `$XDG_DATA_HOME/ghusage`, or `~/.local/share/ghusage` |
| Windows | `%APPDATA%\ghusage\ghusage\data` |

The directory is created if it does not exist. SQLite also keeps `-wal` and
`-shm` files next to the database while it is open.

```sh
ghusage cache status          # location, size, counts, last sync, 30-day coverage
ghusage cache status --format json
ghusage cache clear           # delete all cached GitHub data
```

`cache clear` removes all cached data (pull requests, events, coverage, sync
history) but keeps the database file and its schema. Credentials are not
stored in the cache and are not affected.

When the database schema changes in a newer version, `ghusage` migrates the
cache automatically and keeps a backup copy next to it
(`ghusage.db.bak-v<old version>`). A cache created by a newer version is
refused rather than modified. A damaged database is reported as corrupt
(exit code 5); move or delete the file to start over.

## Sync lock

Only one sync can run per data directory at a time. A sync takes an exclusive
lock on `sync.lock` in the data directory. A second sync fails immediately
with exit code 5:

```text
error: another ghusage sync is already running (lock: /path/to/sync.lock)
```

Report commands that need to sync while another sync is running print a
warning and show the cached data instead. Reading the cache does not need the
lock, so reports and the terminal UI work while a sync runs.

## Multiple accounts

Coverage, pull requests, issues and events are associated with the account
they were fetched for (your account, or the `--user` you analyzed), so
syncing another user does not mark your own periods as synced. Pull requests
are shared between accounts when both were involved.

A pull request you look up with `ghusage pr <URL>` is stored in the cache for
your account even if you were not involved in it, so it can appear in lists
that include other participants' activity, such as `prs --all-participants`.

## Network behavior

- Up to `--concurrency` requests run in parallel (default 4, maximum 16).
- Each request times out after 30 seconds and is retried up to 5 attempts in
  total with exponential backoff for timeouts, network errors and HTTP
  500/502/503/504.
- When the primary rate limit is exhausted, `ghusage` waits for the reset if
  it is at most 15 minutes away; otherwise it stops with a rate-limit error.
  Secondary rate limits and `Retry-After` are honored.
- `GHUSAGE_LOG=debug` (or any `tracing` filter such as
  `ghusage_github=debug`) prints diagnostic logs to stderr.
