# Syncing and the local cache

`ghusage` never computes reports directly from API responses. It first stores
GitHub data in a local SQLite database (the cache) and then builds every
report, in every format and in the terminal UI, from that cache.

## What a sync fetches

For the requested period, a sync fetches three kinds of data:

| Data kind | How it is found | What is stored |
|---|---|---|
| `commits` | Commits whose author is your account, by committer date: GitHub commit search, plus the default-branch commit list of each of your repositories pushed to in the period and each repository you are related to with your commits or pull requests. Copies of your commits are left out | Commits, plus your commits found inside synced pull requests |
| `pull_requests` | Issue search: pull requests that `involves:` you or are `reviewed-by:` you, updated in the period | Each pull request with all of its commits (including their line counts), reviews, review threads and their comments, conversation comments, timeline events, the CI results of its head commit, its current diff and how it was merged |
| `issues` | Issue search: issues that `involves:` you, updated in the period | Each issue with its open/close/reopen history |

Each pull request and issue is written in its own transaction, so an
interrupted sync keeps everything written so far and the next run continues
with what is missing.

Pull requests are fetched up to five per GraphQL query, as long as enough
queries remain to run `--jobs` of them at once. Further pages of a pull
request's commits, reviews, comments and timeline, and its head commit's
checks when they come from REST, are fetched per pull request. If GitHub's
answer to such a query cannot be attributed to single pull requests (for
example the request fails), those pull requests are fetched again one by
one.

### How commits are found

GitHub's commit search index can silently miss commits, even in repositories
it has indexed. `ghusage` therefore also lists commits directly:

1. It lists your repositories that were pushed to since the start of the
   missing range (`GET /user/repos`, newest push first, stopping at the first older
   repository). With `--user`, only the other user's own repositories are
   visible (`GET /users/{login}/repos`).
2. It adds repositories of the pull requests found by the sync, and
   repositories in which commit search found your commits if you are
   related to them: repositories you own, repositories in the list of
   step 1, and repositories with your pull requests or issues.
3. For each of these repositories (forks excluded, and private ones
   excluded with `--public-only`) it lists your commits on the default
   branch in the range (`GET /repos/{owner}/{repo}/commits?author=...`).

Commit search also finds your commits in repositories you are not known to
be related to. Some of them are legitimate: with `--user`, for example, a
repository where the other user pushes directly as a collaborator is not in
their repository list. Others only contain a copy of your commits, such as
someone else's project uploaded again as a new repository instead of forked
(GitHub does not mark those as forks). The two cannot always be told apart,
so search results in these repositories are kept, from search alone (their
default branches are not listed), except copies: a commit whose SHA is also
found in a repository you are related to is skipped.

A commit found by search, by a listing and inside a pull request is stored
once. This costs at least one request per repository, so the number of API
requests grows with the number of repositories you were active in.
Commits coverage is `complete` only when commit search and every listing
succeeded. Commits coverage recorded by older versions, which relied on
commit search alone or counted copies of your commits, is not reused: the
next sync fetches those ranges again and removes the copies stored for
them.

### Line counts

[Lines changed](../reference/metrics.md#lines-changed) needs the number of
lines each of your commits added and deleted.

- **Commits inside pull requests** and each **pull request's diff** come
  with the pull request query of every sync. They cost no extra requests.
- **Other commits** (found only on default branches) need
  `ghusage sync --diff-stats`. The first sync of an account, and every sync
  in CI (when `CI` is set, as GitHub Actions does), fetch them without the
  flag; this includes the sync a report runs on its own. `--no-diff-stats`
  turns that off. Fetched this way, a failure only prints a warning and the
  sync or report still succeeds. Syncs of the [terminal UI](./tui.md) do
  not fetch line counts this way; a report with `--no-cache` starts from an
  empty cache every time, so each such report is a first sync and fetches
  them. After the normal sync, it looks for your
  non-merge commits in the period (by committer or author date) whose line
  counts are not in the cache yet, and fetches them through GraphQL, 100
  commits per query, with several queries in parallel (up to
  `--jobs`). Commits whose changes are
  [counted through other commits](../reference/metrics.md#commits-counted-through-other-commits)
  are skipped, because they do not go into the totals; so are commits of
  non-public repositories with `--public-only`.
- Line counts already in the cache are reused and never fetched again, so
  running `--diff-stats` again only asks for what is still missing. As a
  rough guide, about 1,100 commits took about 12 requests.
- Commits that GitHub does not return (for example because the repository
  is no longer accessible) stay unknown (`unavailable` in the output); the
  next `--diff-stats` run asks for them again.

```text
$ ghusage sync --since 1y --diff-stats
...
  status: complete
  line counts: 1084 commits (already known 212, fetched 872, unavailable 0)
```

Line counts that are not known are never shown as zero; reports say how
many commits the sums cover. Pull requests synced by a version that did not
record line counts, diffs or merge methods get them when they are fetched
again; pull requests that have not changed are skipped by a normal sync, so
use `ghusage sync --full` for the period.

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
  issues found: 19 (fetched 19, unchanged 0)
  API requests: 279 (retries 0, 40812 ms)
  status: complete
```

Progress is shown on stderr when stderr is a terminal (see [Progress](#progress)). `--format json`
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

### Recently synced data

When the requested period was fully synced a moment ago and only the last
few minutes are missing, report commands do not sync again. They show the
data as of the last sync and say so on stderr:

```text
note: showing data synced 2 min ago (up to 2026-11-21 23:41); use --max-age 0 to fetch the latest
```

The period then ends at that sync. `--max-age` (default `5m`, or the
`GHUSAGE_MAX_AGE` environment variable) sets how old the data may be;
`--max-age 0` always fetches the latest data.

### Progress

While a sync runs and stderr is a terminal, `ghusage` shows a spinner with
how many requests run in parallel (out of `--jobs`), how many have
finished, and the state of pull requests, commits and issues. Below it are
the requests in flight with how long each has been running (up to four, the
rest counted), and the latest step (the repository or pull request just
fetched). A query for several pull requests shows the first one and how many
more, for example `octo/app#16 +4`. All of it is erased when the sync ends,
so the report output stays clean.

```text
⠹ Syncing with GitHub 0:04 · 6/8 in parallel · 31 requests done · pull requests 1/2 · commits 9/22
  ▸ search commits p4 0.8s
  ▸ octo/app#16 2.3s
  ▸ octo/app commits p2 0.4s
  ▸ octo/tool commits 0.3s
  … 2 more
  ↳ commits in octo/site: 12 found
```

### When GitHub is not reachable

Without `--offline`, report commands first identify your account through the
API. If GitHub cannot be reached at that point (network error, timeout, or
HTTP 5xx after retries), they print a warning and continue as if `--offline`
had been given, using the account of the last online run (or `--user`):

```text
warning: GitHub is not reachable (network error: could not connect to GitHub); showing cached data
```

The report then shows what is in the cache, with coverage telling you which
ranges are missing. This fallback needs a cached account; with an empty cache
and no `--user` the command fails with exit code 4. If GitHub rejects the
token (HTTP 401), the command fails with exit code 3 instead of falling back.

`ghusage sync` cannot fall back: it needs GitHub, so it exits with code 4.
`ghusage pr` warns and shows the cached copy of the pull request when GitHub
is unreachable, and fails if the pull request was never cached.

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
pushed to or contributed to (repository listing + commit search; other
repositories by commit search alone, copies excluded), plus commits of
synced pull requests". The
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

A pull request you look up with `ghusage pr <URL>` is cached so that it can be
shown again (also with `--offline`). If you are not involved in it (not its
author, no activity of yours in it, not requested as reviewer, not assigned),
it is stored separately and never becomes part of your reports or activity.
Viewing a PR does not replace shared PR, event, repository, or commit data and
does not apply sync snapshots that could mark previously stored activity as
removed. Missing detail is cached for offline viewing; a normal sync updates
the account's activity.

Schema v5 repairs v4 viewed-only relationships when the cache contains reliable
evidence of participation, such as authorship, an account-linked PR event,
review/comment authorship, or a PR commit authored by the account. If the cache
does not prove participation, the relationship stays viewed-only rather than
being guessed. To rebuild ambiguous history, sync the required date range with
`--full` into a separate data directory, inspect the resulting reports, and
use that cache only if it contains all the accounts and periods you need. Keep
the original cache until the replacement has been checked.
Schema v4 did not record whether a repository association existed before a PR
was viewed, so migration retains those ambiguous repository links to avoid
deleting a repository that was already in the account's cache.

## Network behavior

- Up to `--jobs` requests run in parallel (default 8, maximum 16).
- Each request times out after 30 seconds and is retried up to 5 attempts in
  total with exponential backoff for timeouts, network errors and HTTP
  500/502/503/504.
- When the primary rate limit is exhausted, `ghusage` waits for the reset if
  it is at most 15 minutes away; otherwise it stops with a rate-limit error.
  Secondary rate limits and `Retry-After` are honored.
- `GHUSAGE_LOG=debug` (or any `tracing` filter such as
  `ghusage_github=debug`) prints diagnostic logs to stderr.
