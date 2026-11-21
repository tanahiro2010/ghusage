# Authentication

`ghusage` needs a GitHub token to call the GitHub API. It only reads data; it
never writes to GitHub.

## Where the token comes from

Credentials are resolved in this order. The first one that is present wins.

| Order | Source | Notes |
|---|---|---|
| 1 | `--token-file <PATH>` | Explicit file. Never silently skipped (see below). |
| 2 | `GH_TOKEN` | Environment variable. Empty values are ignored. |
| 3 | `GITHUB_TOKEN` | Environment variable. Empty values are ignored. |
| 4 | `gh auth token` | Run directly (not through a shell) if the GitHub CLI is installed and logged in. |

If none is available, commands that need GitHub fail with exit code 3:

```text
error: no GitHub credentials found: set GH_TOKEN or GITHUB_TOKEN, log in with `gh auth login`, or pass --token-file
```

### Token files

`--token-file` reads a file that contains a single token (surrounding
whitespace and a trailing newline are fine). When you pass a token file,
`ghusage` uses it or fails; it does not fall back to other sources. The file
is rejected (exit code 3) when:

- it does not exist, is not a regular file, or cannot be read;
- it is empty, larger than 64 KiB, or contains more than one word;
- on Unix, it is readable or writable by the group or by other users.

```sh
umask 077
printf '%s\n' "$TOKEN" > ~/.config/ghusage-token
chmod 600 ~/.config/ghusage-token
ghusage --token-file ~/.config/ghusage-token auth status
```

```text
$ chmod 644 token.txt
$ ghusage --token-file token.txt auth status
error: token file token.txt must not be readable by other users (use chmod 600)
```

The permission check is done on Unix-like systems only.

## Checking credentials

```sh
ghusage auth status
```

shows the login, where the token came from, the token's OAuth scopes (when
GitHub reports them) and the current rate limits. `ghusage doctor` runs the
same checks together with cache checks.

If GitHub rejects the token (HTTP 401), commands fail with exit code 3:

```text
error: authentication failed: GitHub rejected the credentials (HTTP 401)
```

## Which token to use

`ghusage` can only see what the token can see.

- **Public data only**: any valid token works. Use `--public-only` to make
  sure private repositories are neither fetched nor shown.
- **Private repositories**: a classic personal access token with the `repo`
  scope, or the token of a logged-in GitHub CLI, gives access to the private
  repositories you can read.
- **Organizations with SAML single sign-on**: the token must be authorized
  for the organization, otherwise its repositories are not returned or are
  reported as inaccessible.
- **Fine-grained tokens** only see the repositories they were granted. Search
  results and activity in other repositories are not visible to them.

## Analyzing another user

```sh
ghusage overview --user octocat
```

`--user` analyzes another account's activity, limited to what your token can
see. Ownership (`--ownership owned`) for another user is based on repository
owner only, because your token's admin permissions do not say anything about
theirs.

## Public-only mode

```sh
ghusage sync --public-only
ghusage overview --public-only
```

With `--public-only`, searches are restricted to public repositories and
anything from non-public repositories is dropped from reports.

::: warning Switching modes
Sync coverage is recorded per account, not per mode. If you sync a period
with `--public-only` and later run without it (or the other way round),
`ghusage` considers the period already synced. Run
`ghusage sync --full --since <start>` after switching modes to refetch it.
:::

## GitHub Enterprise Server

Set `GHUSAGE_API_URL` to your server's REST API base URL. The GraphQL
endpoint defaults to `<GHUSAGE_API_URL>/graphql`; override it with
`GHUSAGE_GRAPHQL_URL` if needed.

```sh
export GHUSAGE_API_URL=https://github.example.com/api/v3
export GHUSAGE_GRAPHQL_URL=https://github.example.com/api/graphql
export GH_TOKEN=...   # a token for that server
ghusage sync
```

GitHub Enterprise Server support has not been tested. Note that `gh auth
token` returns the token of the GitHub CLI's default host, so set `GH_TOKEN`
or use `--token-file` explicitly. `ghusage pr` and opening links from the
terminal UI currently only accept `github.com` URLs.

## Token safety

- Tokens are never written to the cache database, logs or output.
- The token is only sent to the configured API endpoint. Pagination links
  pointing to another host are rejected.
- `ghusage cache clear` does not touch credentials.
