# JSON and CSV output

Use `--format json` or `--format csv` for scripts. Table output is meant for
people and may change; the JSON schema is versioned.

```sh
ghusage weekly --format json > weekly.json
ghusage prs --format csv > prs.csv
```

Unless `--include-private` is given, JSON and CSV hide private repository
names, titles and URLs. See [Private repositories](#private-repositories).

## Conventions

- **Schema version.** Every report has `"schema_version": 1`. Fields may be
  added in a compatible way without changing the version; removing or
  changing the meaning of a field changes it.
- **Timestamps** are RFC 3339 / ISO 8601 strings. `period.start` and
  `period.end` use the report's time zone offset
  (`2026-11-02T00:00:00+09:00`); all other timestamps are UTC
  (`2026-11-01T15:00:00Z`).
- **Durations** are whole seconds (fields ending in `_secs`, and timing
  statistics).
- **`null` versus `0`.** A count is `0` when nothing happened. `null` means
  unknown or not applicable: no last activity, no merge, no CI result,
  statistics over zero pull requests, an unknown account ID.
- **Field order** is stable but should not be relied on. Arrays have a
  defined order, described below.
- Command-specific sections are **omitted** (not `null`) when the command
  does not produce them.

## Report object

All period reports (`overview`, `daily`, `weekly`, `monthly`, `repos`, `prs`,
`reviews`, `comments`, `issues`) print one JSON object with these root fields:

| Field | Type | Description |
|---|---|---|
| `schema_version` | integer | `1` |
| `account` | object | Who was analyzed: `login`, `id` (GitHub node ID or `null`), `is_authenticated_user` |
| `period` | object | `start` (inclusive) and `end` (exclusive), RFC 3339 in the report time zone |
| `timezone` | string | IANA time zone used for calendar boundaries |
| `metrics` | [Summary](#summary) | Summary of the whole period |
| `snapshot` | [Snapshot](#snapshot) | Current-state values |
| `repositories` | array of [RepoSummary](#reposummary) | Per-repository summaries, ordered by your activity then name |
| `activity_breakdown` | array of [BucketSummary](#bucketsummary) | Per-bucket summaries in time order. Daily buckets unless the command is `weekly` or `monthly` |
| `granularity` | string | `day`, `week` or `month`. Present only for `daily`, `weekly`, `monthly` |
| `coverage` | array of [CoverageSummary](#coveragesummary) | One entry per data kind |
| `synced_at` | timestamp or `null` | When the last sync of this account finished |
| `private_repositories_redacted` | boolean | `true` when private names were hidden (JSON/CSV without `--include-private`) |

Command-specific fields:

| Command | Fields |
|---|---|
| `prs` | `pull_requests` (array of [PrRow](#prrow)), `intensity` ([Intensity](#intensity)), `timing` ([TimingStats](#timingstats)) |
| `reviews` | `reviews_submitted`, `reviews_received` (arrays of [ReviewRow](#reviewrow)) |
| `comments` | `comments` (array of [CommentRow](#commentrow)) |
| `issues` | `issues` (array of [IssueRow](#issuerow)) |

Definitions of every count are in [Metric definitions](./metrics.md).

### Summary

| Field | Type |
|---|---|
| `commits`, `merge_commits` | integer |
| `prs_created`, `prs_merged`, `merges_performed`, `merges_unknown_actor` | integer |
| `prs_closed_without_merge`, `prs_reopened` | integer |
| `reviews_submitted` | [ReviewCounts](#reviewcounts) |
| `reviewed_prs` | integer (unique count) |
| `reviews_received` | [ReviewCounts](#reviewcounts) |
| `reviews_received_from_bots`, `review_dismissals_performed` | integer |
| `comments_written`, `comments_received` | [CommentCounts](#commentcounts) |
| `review_responses`, `pr_commits`, `force_pushes` | integer |
| `coordination_actions`, `metadata_actions` | integer |
| `issues_opened`, `issues_closed`, `issues_reopened` | integer |
| `active_prs`, `active_prs_all_participants` | integer (unique counts) |
| `pr_internal_events` | integer |
| `ci` | object: `passed`, `failed`, `pending`, `other` (integers). Never part of activity |
| `active_days` | integer |
| `last_activity` | timestamp or `null` |

#### ReviewCounts

`total`, `approved`, `changes_requested`, `commented`,
`dismissed_unknown_state` (integers).

#### CommentCounts

`conversation`, `inline`, `review_summary` (integers). There is no total
field; the total is the sum of the three.

### Snapshot

| Field | Description |
|---|---|
| `open_prs` | Your pull requests that are open as of their last sync |
| `open_issues_authored` | Issues you opened that are currently open |
| `threads` | Review threads of pull requests active in the period: `total`, `open`, `resolved`, `outdated` |

### RepoSummary

| Field | Description |
|---|---|
| `repository_id` | GitHub node ID of the repository |
| `full_name` | `owner/name` (redacted for private repositories) |
| `visibility` | `public`, `private`, `internal` or `unknown` |
| `owned` | Whether you own it (see [CLI reference](./cli.md#global-options)) |
| `accessible` | `false` if the repository is known to be inaccessible |
| `summary` | [Summary](#summary) for this repository |

### BucketSummary

| Field | Description |
|---|---|
| `bucket.key` | First local date of the calendar bucket (`YYYY-MM-DD`) |
| `bucket.label` | `2026-11-21` (day), `2026-W47` (ISO week), `2026-11` (month) |
| `bucket.start`, `bucket.end` | Exact UTC range of the bucket, clipped to the period |
| `summary` | [Summary](#summary) for the bucket |

### CoverageSummary

| Field | Description |
|---|---|
| `kind` | `commits`, `pull_requests` or `issues` |
| `status` | `complete`, `partial`, `not_available`, `not_yet_synced` or `unknown` for the whole period |
| `scopes` | What was fetched, as human-readable descriptions |
| `segments` | Array of `{start, end, status, reason}` covering the period in order; `reason` explains non-complete segments or is `null` |

The report's overall coverage is `complete` only if every entry is
`complete`; `--require-complete` uses that.

### PrRow

| Field | Description |
|---|---|
| `pr_id` | GitHub node ID of the pull request |
| `repository` | `owner/name` |
| `number`, `title`, `url` | Pull request number, title and URL |
| `author` | Author login (`unknown` if not known) |
| `state` | Current state: `open`, `closed`, `merged` |
| `is_draft` | Current draft flag |
| `created_at`, `merged_at` | Timestamps (`merged_at` is `null` if not merged) |
| `events`, `events_by_subject` | Counted non-CI events in the period by anyone / by you |
| `commits`, `reviews`, `comments`, `participants` | Counts in the period |
| `last_activity` | Latest event time in the period |
| `latest_ci` | Current CI state of the head commit: `success`, `failure`, `pending`, `neutral`, `skipped`, `cancelled`, `unknown`, or `null` |

Ordered by `--sort` (largest or newest first), then repository and number.

### Intensity

`prs` (number of pull requests), and `events_per_pr`, `reviews_per_pr`,
`comments_per_pr`, `commits_per_pr`, `participants_per_pr`,
`active_days_per_pr`, each a [Stats](#stats) object or `null` when there are
no pull requests.

### TimingStats

`time_to_first_review`, `creation_to_merge`,
`review_to_next_author_activity`, `first_to_last_activity`: each a
[Stats](#stats) object in seconds, or `null` when no pull request has that
value. See [lifecycle timing](./metrics.md#details-lifecycle-timing-your-pull-requests).

#### Stats

`count`, `min`, `median`, `mean`, `p90` (nearest-rank 90th percentile),
`max`. Numbers may be fractional.

### ReviewRow

| Field | Description |
|---|---|
| `event_id` | Stable event identifier |
| `submitted_at` | Submission time |
| `reviewer` | Reviewer login |
| `state` | `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED` (original state unknown), or `null` |
| `repository` | `owner/name` |
| `pr_number`, `pr_title` | Pull request number and title (or `null`) |
| `url` | Link to the review, or `null` |

`reviews_submitted` lists your counted reviews; `reviews_received` lists
reviews by others on your pull requests. Both in time order.

### CommentRow

| Field | Description |
|---|---|
| `event_id` | Stable event identifier |
| `created_at` | Creation time |
| `author` | Author login |
| `kind` | `conversation_comment`, `inline_comment` or `review_summary` |
| `written_by_subject` | `true` for comments you wrote |
| `repository`, `pr_number` | Where the comment is |
| `edited` | Whether the comment was edited |
| `url` | Link, or `null` |

Lists comments you wrote and comments others wrote on your pull requests, in
time order.

### IssueRow

| Field | Description |
|---|---|
| `issue_id` | GitHub node ID |
| `repository`, `number`, `title`, `url` | The issue |
| `is_open` | Current state |
| `opened_in_period` | Whether you opened it in the period |
| `closed_in_period`, `reopened_in_period` | Close / reopen events by you in the period |

Ordered by repository and number.

## Pull request detail (`ghusage pr`)

| Field | Description |
|---|---|
| `schema_version`, `account`, `timezone` | As in reports |
| `pr_id`, `repository`, `number`, `title`, `url` | The pull request |
| `author` | Author login |
| `state`, `is_draft` | Current state |
| `created_at`, `merged_at`, `closed_at` | Timestamps (`null` when not applicable) |
| `merged_by` | Merger login, or `null` when not merged or unknown |
| `latest_ci` | Current CI state of the head commit (string) or `null` |
| `commits`, `removed_commits` | Commits currently in the pull request / commits removed by force pushes |
| `reviews` | [ReviewCounts](#reviewcounts) |
| `comments` | [CommentCounts](#commentcounts) |
| `review_threads` | `total`, `open`, `resolved`, `outdated` (current state) |
| `participants` | Logins of known accounts with counted non-CI events, sorted |
| `timing` | `time_to_first_review_secs`, `creation_to_merge_secs`, `review_to_next_author_activity_secs`, `first_to_last_activity_secs` (integers or `null`) |
| `timeline` | Array of [TimelineEntry](#timelineentry) in event order (filtered by `--event-type`) |
| `coverage` | Coverage status of this pull request's data |
| `synced_at` | When the pull request was last fetched |
| `private_repositories_redacted` | As in reports |

### TimelineEntry

| Field | Description |
|---|---|
| `event_id` | Stable event identifier |
| `occurred_at` | When it happened (for commits: the commit time, not the push time) |
| `observed_at` | When `ghusage` fetched it |
| `actor`, `actor_kind` | Login (`unknown` if not known) and `user`, `bot`, `organization`, `mannequin` or `unknown` |
| `event_type` | One of the event types listed in the [CLI reference](./cli.md#pr) |
| `label` | Human-readable event name |
| `detail` | Extra information (review state, short SHA, label, previous title, CI check and outcome, notes such as "thread reply only, not counted as a review", "edited", "via merge", "no longer present"), or `null` |
| `url` | Link, or `null` |
| `removed_observed_at` | When the item was found to be gone, or `null` |

Events at the same instant are ordered by the natural order of a pull
request's life (opened, commits, force pushes, ..., merged, closed, reopened).

## Other commands

### sync

`ghusage sync --format json`:

| Field | Description |
|---|---|
| `status` | `complete` or `partial` |
| `commits_found` | Distinct commits found by commit search and the default-branch commit lists (commits found only inside pull requests are not included) |
| `pull_requests_found`, `pull_requests_fetched`, `pull_requests_unchanged` | Pull requests found by search / fetched / skipped as unchanged |
| `issues_found`, `issues_fetched` | Issues found / fetched |
| `failures` | Descriptions of pull requests or issues that failed or were partially fetched |
| `coverage` | Coverage records written: `kind`, `start`, `end`, `status`, `scope`, `reason`, `recorded_at` |
| `api` | `requests`, `retries`, `rate_limit_waits`, `total_api_time_ms`, `bytes_received` |

### doctor

`{"ok": true, "checks": [{"ok": true, "message": "..."}]}`

### auth status

`authenticated`, `source`, `login`, `id`, `scopes` (array or `null`),
`rate_limit` (`core_remaining`, `core_limit`, `core_reset`,
`search_remaining`, `search_limit`, `graphql_remaining`, `graphql_limit`, or
`null`).

### cache status

`path`, `file_size_bytes`, `schema_version`, `account`, `repositories`,
`pull_requests`, `issues`, `events`, `last_sync` (`started_at`,
`finished_at`, `status`, `message`, or `null`), `coverage_last_30_days`
(array of [CoverageSummary](#coveragesummary)).

## CSV

CSV output is one flat table per command, with a header row. Unknown values
are empty cells; zero is written as `0`. Timestamps are RFC 3339 in UTC.
Booleans are `true` / `false`. Free text (titles, details) has control
characters removed, and a leading `=`, `+`, `-` or `@` is prefixed with `'`
so spreadsheets do not evaluate it as a formula.

### overview, daily, weekly, monthly

One row per calendar bucket (`overview` produces daily rows):

```text
period,start,end,commits,merge_commits,prs_created,prs_merged,merges_performed,prs_closed_without_merge,reviews_submitted,reviewed_prs,reviews_received,comments_written,comments_received,review_responses,pr_commits,issues_opened,issues_closed,active_prs,pr_internal_events,ci_passed,ci_failed,coverage
```

`period` is the bucket label, `reviews_submitted` / `reviews_received` are
totals, `comments_written` / `comments_received` are totals of all three
types, and `coverage` is the combined coverage of that bucket.

### repos

```text
repository,visibility,owned,accessible,commits,prs_created,prs_merged,reviews_submitted,comments_written,issues_opened,active_prs,pr_internal_events,last_activity,period_coverage
```

### prs

```text
repository,number,title,state,draft,author,created_at,merged_at,events,events_by_you,commits,reviews,comments,participants,last_activity,latest_ci,url,period_coverage
```

### reviews

```text
direction,submitted_at,reviewer,state,repository,pr_number,url,period_coverage
```

`direction` is `submitted` or `received`.

### comments

```text
created_at,author,kind,written_by_you,repository,pr_number,edited,url,period_coverage
```

### issues

```text
repository,number,title,open,opened_in_period,closed_in_period,reopened_in_period,url,period_coverage
```

### pr

The timeline of the pull request, one row per event:

```text
occurred_at,actor,actor_kind,event_type,detail,removed_observed_at,url
```

`period_coverage` is the overall coverage of the whole period. `sync`,
`doctor`, `auth status` and `cache status` have no CSV form; with
`--format csv` they print their table output.

## Private repositories

Without `--include-private`, JSON and CSV output hide information about
non-public (private and internal) repositories. Numbers are identical either
way.

| Item | Replaced with |
|---|---|
| Repository name | `<owner>/repo-<8 hex characters>`, where the owner is your login for repositories you own and `private` otherwise. The suffix is derived from the repository ID, so it is stable across runs |
| Pull request and issue titles | `[private]` |
| Pull request and issue URLs | empty string |
| Review, comment and event URLs, repository URLs | `null` |
| Review thread file paths, label and milestone names, previous titles, CI check names | removed |

Logins of people (yours and others') and GitHub node IDs are not hidden.
Table output and the terminal UI always show real names, because they are
shown only to you.
