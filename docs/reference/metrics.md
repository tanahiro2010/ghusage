# Metric definitions

This page defines every number `ghusage` reports. Table output, JSON, CSV and
the terminal UI are all computed by the same code, so the definitions apply
to all of them. JSON field names are given in `code`.

## Basic rules

### Events

Everything `ghusage` counts is an **event**: a commit, a pull request being
opened, a review being submitted, a comment, a label being added, a check run
finishing, and so on. Each event has a stable identity, so the same event
fetched twice or from two different GitHub APIs is counted once, and the
order in which data was fetched never changes a result.

### You

"You" is the account being analyzed: the authenticated user, or the account
given with `--user`. An event is yours when its actor is that account. When
both account IDs are known, only the IDs are compared, so a renamed or reused
login cannot be mistaken for you.

- **Bots** are accounts whose GitHub type is `Bot` or whose login ends in
  `[bot]`. Their activity is never yours; it is included in counts of what
  you received and of everyone's activity.
- **Unknown actors** (for example deleted accounts) are never counted as you.

### Which events count

An event counts toward a period when:

1. its time is inside the period, which is half-open: start ≤ time < end
   (see [Periods and time zones](./time.md));
2. its repository matches the filters (`--repo`, `--owner`, `--visibility`,
   `--ownership`, `--public-only`);
3. it has not been observed to be removed (see
   [edits and deletions](#edits-and-deletions));
4. it is not a pending (draft, unsubmitted) review, and not a
   [thread-reply-only review](#thread-reply-only-reviews).

An event's time is when it happened on GitHub (`created_at`, `submitted_at`,
the time of a timeline event). For commits it is the **committer date** by
default; with `--commit-date author` it is the author date. Commit times are
when the commit was made, not when it was pushed; GitHub does not provide
push times.

### Activity, not CI

Check runs and commit statuses (CI) are **automation**. They are never
counted as anyone's activity: they are excluded from active days, active
pull requests, pull request internal events, per-PR event counts,
participants and timings. CI results are reported separately.

### Period counts versus current state

Most numbers count events in the period. A few are **snapshot** values that
describe the current state as of the last sync, regardless of the period:
open pull requests, open issues you authored, and review thread resolution.
They are labeled "(current)" or "(now)" in table output and live under
`snapshot` in JSON.

### Totals and breakdowns

`daily`, `weekly` and `monthly` split the period into calendar buckets that
cover it exactly, without gaps or overlaps. Event counts in the buckets add up
to the total of the period. Unique counts do not add up: a pull request
reviewed on two days counts once on each of those days but once in the
total. The unique counts are `reviewed_prs`, `active_prs` and
`active_prs_all_participants`.

## Summary metrics

These appear in `overview` (and in every JSON report under `metrics`, per
bucket under `activity_breakdown[].summary`, and per repository under
`repositories[].summary`).

### Commits

**Commits** (`commits`): the number of distinct commits you authored in the
period, identified by repository and SHA.

- Sources: GitHub commit search for commits whose author is your account,
  plus your commits found inside pull requests `ghusage` has synced. A commit
  found both ways counts once.
- A commit is yours when GitHub links its author to your account (usually by
  the commit email). Commits with an author email that is not linked to your
  account are not attributed to you.
- The same SHA in two repositories (for example a fork and its upstream)
  counts as two commits.
- Commits that were later removed from a pull request by a force push still
  count: you made them.
- Placed in time by committer date (default) or author date
  (`--commit-date author`).

**Merge commits** (`merge_commits`): how many of those commits have more than
one parent. Shown as `(N merge)`.

### Pull requests

**PRs created** (`prs_created`): pull requests you opened in the period
(counted at their creation time).

**PRs merged (yours)** (`prs_merged`): your pull requests that were merged in
the period, by anyone, counted at the merge time. A pull request created
before the period and merged in it counts; one created in the period and
merged later does not.

**Merges you performed** (`merges_performed`): merges in the period where you
were the one who merged, whoever authored the pull request. Merging your own
pull request counts here and in "PRs merged (yours)".

**Merged by unknown** (`merges_unknown_actor`): how many of your merged pull
requests have no known merger (GitHub did not report one). Shown in table
output only when non-zero.

**PRs closed without merge** (`prs_closed_without_merge`): close events in the
period on your pull requests that are known not to be part of a merge, by
anyone. A close counts as part of a merge when the pull request is merged and
the close happened within 60 seconds of the merge. A pull request that was
closed, reopened and closed again counts twice.

**PRs reopened** (`prs_reopened`, JSON/TUI only): reopen events on your pull
requests in the period, by anyone.

**Open PRs (current)** (`snapshot.open_prs`): your pull requests in the cache
that are open as of their last sync. This is current state, not a count for
the period. Repository filters apply.

### Reviews

GitHub reviews have a state: approved, changes requested, or commented.

**Reviews submitted** (`reviews_submitted`): review submissions you made in
the period, on anyone's pull request.

- `total`: all counted submissions. Submitting three reviews on one pull
  request counts three.
- `approved`, `changes_requested`, `commented`: by state.
- `dismissed_unknown_state`: dismissed reviews whose original state could not
  be determined. When GitHub reports the state a review had before it was
  dismissed, the review counts under that state.
- Pending (unsubmitted) reviews are never counted.

**Reviewed PRs** (`reviewed_prs`): the number of distinct pull requests on
which you submitted at least one counted review in the period. This is a
unique count. Shown as `on N PRs`.

**Reviews received** (`reviews_received`): review submissions by anyone other
than you (people, bots and unknown actors) on your pull requests in the
period, with the same breakdown by state.

**From bots** (`reviews_received_from_bots`): how many of the reviews
received were submitted by bots.

**Review dismissals performed** (`review_dismissals_performed`, JSON only):
reviews you dismissed in the period.

#### Thread-reply-only reviews

When you reply to an existing review thread, GitHub records the reply as a
review with state "commented" and no body. Counting those would inflate
review numbers, so a commented review is **not** counted as a review
submission when it has no summary text and every inline comment in it is a
reply to an existing thread. The replies themselves still count as inline
comments. In `ghusage pr --timeline` such reviews are shown with the detail
"thread reply only, not counted as a review".

#### Review summaries

The text body of a submitted review is a **review summary**. It counts as a
comment (type `review_summary`), never as an additional review. A review with
an empty or whitespace-only body has no summary.

### Comments

There are three comment types:

| Type | JSON key | What it is |
|---|---|---|
| Conversation | `conversation` | A comment in the pull request's conversation (not attached to code) |
| Inline | `inline` | A review comment attached to code, in a review thread, including replies |
| Review summary | `review_summary` | The text body of a submitted review |

**Comments written** (`comments_written`): comments you wrote in the period on
any pull request, by type. The table total is the sum of the three types.

**Comments received** (`comments_received`): comments written by anyone other
than you (including bots) on your pull requests in the period, by type.

Comments on issues are not collected; comment metrics cover pull requests
only. Editing a comment is not counted as activity.

**Review thread replies** (`review_responses`): your inline replies in review
threads on your own pull requests where the thread was started by someone
else (a known account). These are your answers to review feedback. Additional
commits pushed after a review are not counted here.

### Pull request development

**PR commits** (`pr_commits`): distinct commits you authored that are
currently part of a pull request, by commit time in the period. A commit
that is part of two pull requests counts once for each. Commits removed from
the pull request by a force push are excluded.

**Force pushes** (`force_pushes`): force pushes you made to a pull request's
branch in the period, as recorded in the pull request timeline.

**Coordination actions** (`coordination_actions`, JSON only): review
requests, removed review requests, assignments and unassignments you made.

**Metadata actions** (`metadata_actions`, JSON only): labels added or removed,
milestones set or removed, and title changes you made.

### Issues

**Issues** (`issues_opened`, `issues_closed`, `issues_reopened`): issues you
opened, close events you performed, and reopen events you performed in the
period. Closing the same issue twice counts twice.

**Open issues you authored (current)** (`snapshot.open_issues_authored`,
shown by `issues`): issues you opened that are currently open.

### Activity

**Active PRs** (`active_prs`): the number of distinct pull requests in which
you had at least one event in the period (any type except CI: a commit, a
review, a comment, a label, ...). Unique count.

**With anyone's activity** (`active_prs_all_participants`): the number of
distinct synced pull requests in which anyone (you, others or bots) had at
least one non-CI event in the period. Unique count. Only pull requests that
were synced because they involve you are known to `ghusage`.

**PR internal events** (`pr_internal_events`): the number of your events
inside pull requests in the period: pull request commits, lifecycle events
(open, close, reopen, merge, draft changes), force pushes, counted reviews,
dismissals, comments of all three types, coordination and metadata actions.
CI is excluded. Commits found by commit search and issue events are not pull
request events.

**Active days** (`active_days`): the number of distinct local calendar dates
(in the report's time zone) on which you had at least one counted non-CI
event: a commit, a pull request event or an issue event.

**Last activity** (`last_activity`, JSON/CSV): the time of your latest
counted non-CI event in the period.

### CI results

**CI results (not counted as activity)** (`ci`): check runs and commit
statuses of the synced pull requests with a time in the period, by outcome,
regardless of who triggered them:

| Field | Outcomes |
|---|---|
| `passed` | success |
| `failed` | failure, timed out, startup failure, action required, error |
| `pending` | queued, in progress, waiting, requested, pending |
| `other` | neutral, skipped, stale, cancelled, unknown |

Only CI results of each pull request's current head commit are available.

### Review threads (current)

`snapshot.threads` counts the review threads of the pull requests that had
anyone's non-CI activity in the period, by their current state: `total`,
`open` (not resolved), `resolved`, and `outdated` (the code they refer to has
changed; overlaps with open and resolved). When a thread was resolved is not
known.

## Table labels

| `overview` label | JSON field |
|---|---|
| Commits | `commits` (`merge_commits`) |
| PRs created | `prs_created` |
| PRs merged (yours) | `prs_merged` |
| Merges you performed | `merges_performed` (`merges_unknown_actor`) |
| PRs closed without merge | `prs_closed_without_merge` |
| Open PRs (current) | `snapshot.open_prs` |
| Reviews submitted | `reviews_submitted`, `reviewed_prs` |
| Reviews received | `reviews_received`, `reviews_received_from_bots` |
| Comments written | `comments_written` |
| Comments received | `comments_received` |
| Review thread replies | `review_responses` |
| PR commits | `pr_commits` |
| Force pushes | `force_pushes` |
| Issues | `issues_opened`, `issues_closed`, `issues_reopened` |
| Active PRs | `active_prs`, `active_prs_all_participants` |
| PR internal events | `pr_internal_events` |
| CI results (not counted as activity) | `ci` |
| Active days | `active_days` |

In `daily` / `weekly` / `monthly`, the columns are: Commits (`commits`), PRs
(`prs_created`), Merged (`prs_merged`), Reviews (`reviews_submitted.total`),
Received (`reviews_received.total`), Comments (comments written, all types),
PR commits (`pr_commits`), Issues (`issues_opened`), Active PRs
(`active_prs`).

In `repos`, the columns are: Commits, PRs (created), Merged (yours), Reviews
(submitted), Comments (written), Issues (opened), Active PRs, PR events
(`pr_internal_events`) and Last activity, all for that repository.
Repositories are ordered by your activity there (commits + PR internal
events + issue events), then by name.

## Pull request list

`ghusage prs` lists pull requests with at least one non-CI event in the
period: by default those where at least one event is yours; with
`--all-participants` also those where only others were active. All columns
count events in the period only.

| Column | JSON field | Definition |
|---|---|---|
| State | `state`, `is_draft` | Current state: `open`, `draft`, `merged`, `closed` (closed without merge) |
| Author | `author` | Login of the pull request author |
| Events | `events` | Counted non-CI events by anyone (including bots) |
| Yours | `events_by_subject` | Those of the events that are yours |
| Commits | `commits` | Distinct commits in the pull request (by anyone) with a commit time in the period |
| Reviews | `reviews` | Counted review submissions by anyone |
| Comments | `comments` | Comments of all three types by anyone |
| People | `participants` | Distinct known accounts with at least one event |
| Last activity | `last_activity` | Time of the latest of those events |
| CI | `latest_ci` | Current combined CI state of the head commit (snapshot) |

### `--details`: activity per pull request

Descriptive statistics over the listed pull requests (same participation
rule): events, reviews, comments, commits, participants and active days per
pull request, where active days are the distinct local dates with a non-CI
event in the period. For each, `ghusage` reports the median, mean, 90th
percentile (nearest rank), maximum and the number of pull requests (`n`);
JSON also includes the minimum.

### `--details`: lifecycle timing (your pull requests)

Computed for **your** pull requests that had anyone's activity in the period,
using everything known about each pull request (not only events in the
period). Reported in hours in the table and in seconds in JSON.

| Timing | Definition |
|---|---|
| Time to first review | From creation to the first counted review submitted by someone other than the author (bots included) |
| Creation to merge | From creation to merge, for merged pull requests |
| Review to your next activity | From that first review to the author's next observed event of any kind. This is **not necessarily a response** to the review; it is just the next thing the author did in that pull request |
| First to last activity | From the first to the last observed non-CI event in the pull request |

::: warning Not quality scores
Intensity and timing values are descriptive statistics about how activity is
distributed. They are not measures of quality, skill or productivity, and
should not be used to compare people.
:::

## Pull request detail

`ghusage pr <URL>` covers the whole life of one pull request, not a period.

| Field | Definition |
|---|---|
| State (current) | `open`, `open (draft)`, `merged`, `closed` |
| Merged | Merge time and merger; "merger unknown" when GitHub did not report one |
| Commits | Distinct commits currently in the pull request. "N no longer in the PR" counts commits seen earlier that a force push removed |
| Reviews | Counted review submissions by anyone, by state |
| Comments | Comments by anyone, by type |
| Review threads | Current state: total, open, resolved, outdated |
| Latest CI | Current combined CI state of the head commit |
| Participants | Known accounts with at least one counted non-CI event |
| Time to first review, Creation to merge | As defined above |

The timeline (`--timeline`, JSON, CSV) lists every stored event, including
events that are not counted (pending reviews are never stored; removed items
are marked "no longer present"; thread-reply-only reviews are marked). PR
commits are labeled "Commit (committed)" because their time is the commit
time, not the time they were added to the pull request.

## Edits and deletions

- Editing a comment or review is not counted as activity. Only the latest
  content is known; comments that were edited are marked `edited`.
- When a sync completely refetches a pull request and finds that a comment,
  review, commit or timeline event that was seen before is gone, it is marked
  as removed with the time the removal was observed, and it is excluded from
  counts. When it was actually deleted is not known. Review threads that no
  longer exist are dropped from the thread counts.
- Items deleted before `ghusage` first saw them are not known at all.
