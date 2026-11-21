# Getting started

`ghusage` analyzes a developer's GitHub activity: commits, pull requests,
reviews, comments, issues, and the activity inside pull requests. It fetches
data from the GitHub API into a local SQLite cache and builds reports from
that cache.

## Install

### npm

```sh
npm install -g ghusage
ghusage --version
```

The `ghusage` npm package is a small Node.js launcher. A prebuilt native
binary for your platform is installed as an optional dependency, and the
launcher runs it with your arguments. Node.js 18 or newer is required.

| Platform | npm package |
|---|---|
| macOS, Apple Silicon | `ghusage-darwin-arm64` |
| macOS, Intel | `ghusage-darwin-x64` |
| Linux x64 (statically linked) | `ghusage-linux-x64` |
| Linux arm64 (statically linked) | `ghusage-linux-arm64` |
| Windows x64 | `ghusage-win32-x64` |

- If optional dependencies are skipped (for example `npm install
  --omit=optional` or `--no-optional`), the binary is not installed and
  `ghusage` exits with a message explaining how to reinstall.
- On other platforms the launcher exits with a message; build from source
  instead.
- Set `GHUSAGE_BINARY` to the path of a `ghusage` binary to make the
  launcher run that binary instead (for example one you built yourself).

### From source

You need a Rust toolchain (Rust 1.89 or newer).

Install directly from the repository:

```sh
cargo install --git https://github.com/tanahiro2010/ghusage
```

Or build a local checkout:

```sh
git clone https://github.com/tanahiro2010/ghusage
cd ghusage
cargo build --release
./target/release/ghusage --version
```

::: info Platforms
`ghusage` is developed and used on macOS (Apple Silicon). The code is
written to be portable, and continuous integration is set up to build and
test it on Linux (x86_64 and ARM64), macOS (ARM64 and Intel) and Windows, but
Linux and Windows have not yet been verified by hand on real machines. Please report
problems you find.
:::

## Authenticate

If you already use the [GitHub CLI](https://cli.github.com/) and are logged
in, there is nothing to do: `ghusage` uses `gh auth token`. Otherwise set
`GH_TOKEN` (or `GITHUB_TOKEN`) or pass `--token-file`. See
[Authentication](./authentication.md).

```sh
ghusage auth status
```

```text
Logged in as octocat (credentials from gh auth token)
  token scopes: gist, read:org, repo
  rate limit: core 4998/5000 (resets 2026-11-21T11:02:13+00:00), search 30/30, graphql 4990/5000
```

## First sync

```sh
ghusage sync
```

By default `sync` fetches the last 30 days (today and the 29 days before it).
Use `--since` for a longer range:

```sh
ghusage sync --since 2026-01-01
```

The first sync of a busy account can take a while; later syncs only fetch
what is missing or changed. See [Syncing and the cache](./syncing.md).

You do not have to run `sync` first: report commands fetch missing ranges
automatically. Running it explicitly lets you see progress and the result.

## Reports

```sh
ghusage                       # overview of the last 7 days (default command)
ghusage overview --since 30d
ghusage daily --since 14d
ghusage weekly                # last 8 weeks
ghusage monthly               # last 6 calendar months
ghusage repos
ghusage prs --details
ghusage reviews
ghusage comments
ghusage issues
ghusage pr https://github.com/OWNER/REPO/pull/123 --timeline
```

Every report ends with a coverage line. `Coverage: complete` means every data
kind was fully fetched for the whole period. Anything else lists the ranges
that are missing or partial:

```text
Coverage: not_yet_synced  (last sync: -)
  commits: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
  pull_requests: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
  issues: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
```

## Choosing a period

```sh
ghusage overview --since 2026-11-01 --until 2026-11-30   # whole of November
ghusage daily --since 7d                                 # today and the 6 days before
ghusage weekly --since 4w                                # this week and the 3 before
ghusage overview --since yesterday --until yesterday     # just yesterday
ghusage daily --timezone America/New_York
```

A date given to `--until` includes that whole day. See
[Periods and time zones](../reference/time.md).

## Filtering

```sh
ghusage repos --owner my-org
ghusage prs --repo my-org/api --repo my-org/web
ghusage overview --visibility public
ghusage overview --ownership external     # repositories you do not own
```

Filters only affect reports; `sync` always fetches everything for the period.

## Scripts and exports

```sh
ghusage weekly --format json > weekly.json
ghusage prs --format csv > prs.csv
ghusage overview --format json --require-complete || echo "data incomplete"
```

JSON and CSV hide private repository names and titles unless you pass
`--include-private`. See [JSON and CSV output](../reference/json.md).

## Offline use

```sh
ghusage overview --offline
```

`--offline` never contacts GitHub. Ranges that were never synced are shown as
`not_yet_synced`, and a note is printed to stderr.

## Next steps

- Read the [metric definitions](../reference/metrics.md) to know exactly
  what each number means.
- Read the [known limitations](../reference/limitations.md) before relying on
  the numbers.
- Try the [terminal UI](./tui.md): `ghusage tui`.
