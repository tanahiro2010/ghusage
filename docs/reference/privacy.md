# Privacy

## What leaves your machine

- `ghusage` talks only to the GitHub API: `https://api.github.com` by
  default, or the endpoints set with `GHUSAGE_API_URL` /
  `GHUSAGE_GRAPHQL_URL`. Every request is a read; `ghusage` never changes
  anything on GitHub.
- There is **no telemetry**, no analytics and no update check.
- Your token is sent only to the configured API endpoint, in the
  `Authorization` header. Pagination links pointing to another host are
  rejected so the token cannot be forwarded elsewhere.
- Opening a link from the terminal UI (`o`) hands a `https://github.com` URL
  to your browser.
- With `--offline`, nothing is sent at all.

## What is stored locally

Fetched data is stored in a SQLite database, `ghusage.db`, in the data
directory (see [Syncing and the cache](../guide/syncing.md#data-directory)):
repositories, pull requests, issues, commits, reviews, comments, review
threads, timeline events, CI results, coverage and sync history, for the
accounts you analyzed. It may contain private repository names, titles and
metadata that your token can read.

- Tokens are **never** written to the cache, to logs or to output. Error
  messages do not include them.
- The cache is a normal file protected by your user account's file
  permissions. Treat it like other files that contain your work data.
- `ghusage cache clear` deletes all cached GitHub data (the empty database
  file remains). Deleting the data directory removes everything.
- `--no-cache` keeps fetched data in memory only for the duration of one
  command.

## Exports

JSON and CSV output are often shared or committed somewhere, so by default
they hide information about private and internal repositories: repository
names become `<owner>/repo-<hash>` (with `private` as owner for repositories
you do not own), pull request and issue titles become `[private]`, and URLs,
file paths, label names, previous titles and CI check names are removed. The
numbers do not change.

Pass `--include-private` to include real names, titles and URLs.

The following are **not** hidden: logins of people (including other
participants in private repositories), GitHub node IDs, timestamps and
counts. Review an export before publishing it.

Table output and the terminal UI show real names, because they are displayed
only to you.

To exclude private repositories entirely, use `--public-only`: private
repositories are then neither fetched nor shown.

## Terminal safety

Text from GitHub (titles, logins, repository names, label names, error
messages) is stripped of control characters and escape sequences before it
is printed, so content on GitHub cannot inject terminal escape codes. CSV
cells that start with `=`, `+`, `-` or `@` are prefixed with `'` to prevent
spreadsheet formula injection.
