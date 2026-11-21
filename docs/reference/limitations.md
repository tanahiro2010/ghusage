# Known limitations

`ghusage` can only report what GitHub's APIs expose to your token. Where data
is missing or uncertain, it reports the gap (coverage) or leaves the value
unknown rather than guessing. This page lists the limitations you should know
before relying on the numbers.

## Commits

- **Commit search covers default branches only.** GitHub's commit search
  indexes commits on the default branch of repositories it has indexed.
  Commits that exist only on other branches are found only if they are part
  of a pull request that `ghusage` synced (because it involves you). Commits
  on other branches with no pull request, and commits in repositories GitHub
  has not indexed, are not counted.
- **Commits must be linked to your account.** A commit is attributed to you
  when GitHub links its author to your account, usually by the commit email.
  Commits made with an email that is not associated with your account are
  not counted.
- **Commit times are commit dates, not push times.** GitHub does not record
  when a commit was pushed. `ghusage` uses the committer date (or the author
  date with `--commit-date author`). A commit created last week and pushed
  today counts last week. Rebased or amended commits get a new committer
  date.
- **Author-date mode only sees what was synced by committer date.** Commit
  search is queried by committer date. With `--commit-date author`, commits
  whose committer date lies outside the synced ranges are not in the cache.
- **Force-pushed history.** When a force push removes commits from a pull
  request, the removed commits are kept and marked "no longer in the PR" if
  `ghusage` had seen them before the force push. Commits that were pushed and
  force-pushed away before the first sync cannot be reconstructed. Your
  commits removed this way still count in "Commits"; they no longer count in
  "PR commits".

## Search

- **`updated:` matches the current update time.** GitHub's issue and pull
  request search filters on the time an item was *last* updated. A pull
  request that was active in March and updated again in May does not match a
  search for March. `ghusage` compensates by searching from the start of the
  missing range up to now, so the pull request is found and its March events
  are counted in March. The cost is larger searches for old periods.
- **1000-result cap.** GitHub returns at most 1000 results per search query.
  `ghusage` splits the time range until each part has at most 1000 results.
  If a one-second range still has more, only 1000 are available and coverage
  is `partial` with the reason.
- **Incomplete search results.** When GitHub reports that a search timed out
  and returned incomplete results, coverage is `partial`.
- **Discovery is based on involvement.** Pull requests are found when they
  "involve" you (author, assignee, mentioned, commenter) or were reviewed by
  you; issues when they involve you. Activity in pull requests that GitHub
  search does not associate with you is not found.
- **Search indexing delay.** Very recent activity may not be in GitHub's
  search index yet. Each incremental sync starts 3 days before the missing
  range to catch late-indexed items, and the next sync picks up the rest.

## History before the first sync

`ghusage` stores what it fetches and builds history from that. Things that
the API does not keep cannot be recovered later:

- items deleted before `ghusage` first fetched them;
- the original content of edited comments (only the latest content is
  available);
- commits that were force-pushed away before the first sync;
- push times (never available).

## Edits and deletions

- Edits are not counted as activity and the edit history is not available;
  only the latest content and whether a comment was edited are known.
- Deleted comments, reviews and commits are noticed only when `ghusage`
  refetches the pull request completely. They are then marked removed and
  excluded from counts. The deletion time is not known; only the time the
  removal was observed is recorded.
- A period that is already fully covered is not refetched automatically, so
  deletions and state changes there are only noticed when the pull request
  is refetched (for example after `ghusage sync`, which refreshes pull
  requests updated since the last sync, or `ghusage sync --full`).

## Pull requests

- **Review thread resolution history is not available.** Only the current
  state of each review thread (resolved, outdated, who resolved it) is known,
  not when it was resolved. Thread counts are therefore current state.
- **CI results are only for the head commit.** Check runs and commit
  statuses are fetched for each pull request's current head commit. Results
  for earlier commits of the pull request are not fetched.
- **Comments nested under reviews.** Inline comments are read from review
  threads (all pages). Comments listed under each review are only used as a
  secondary source (first 20 per review) to fill gaps.
- **Merge and close.** Who merged a pull request is sometimes not reported by
  GitHub; it is then shown as unknown and never attributed to anyone. A close
  is treated as part of a merge when it happened within 60 seconds of the
  merge.
- **Draft reviews** (pending, not submitted) are not visible to `ghusage` and
  are never counted.
- **Thread replies as reviews.** GitHub records each reply in a review thread
  as a "commented" review with no body. These are not counted as review
  submissions (see
  [Metric definitions](./metrics.md#thread-reply-only-reviews)).
- **Issue comments are not collected.** Comment metrics cover pull requests.
  Issue metrics cover opening, closing and reopening.
- **Very large pull requests** with thousands of timeline items, commits or
  comments have not been measured systematically; a collection that cannot be
  fetched completely marks the pull request `partial`.

## Accounts and access

- Everything is limited to what the token can see. Private repositories
  without access, organizations that require SAML authorization, and
  repositories outside a fine-grained token's grant are invisible.
- Repositories that become inaccessible keep their cached data; it is not
  removed automatically.
- With `--user`, another user's activity is limited to repositories your
  token can see. Ownership for other users is based on the repository owner
  only.
- Sync coverage is recorded per account, not per `--public-only` mode. After
  switching between `--public-only` and normal mode, run
  `ghusage sync --full` for the affected period.
- Pull requests you look up with `ghusage pr` are stored for your account and
  can appear in lists that include other participants' activity.

## Rate limits and network

- GitHub's REST, search and GraphQL rate limits apply. Search is limited to
  30 requests per minute for authenticated users, so large first syncs
  over long periods take time.
- When the primary rate limit is exhausted, `ghusage` waits for the reset if
  it is at most 15 minutes away; otherwise the sync stops with an API error
  (exit code 4). Everything fetched so far is kept and the next run
  continues.
- Secondary rate limits are honored with waits of up to 60 seconds.
  `Retry-After` is honored when given in seconds.
- Without `--offline`, every report command contacts GitHub to identify your
  account, so it fails without network access. Use `--offline` there.

## Platforms

- `ghusage` is developed on macOS (Apple Silicon), where it is used against
  real GitHub data.
- Continuous integration is set up to build and run the test suite on Linux
  (x86_64 and ARM64), macOS (ARM64 and Intel) and Windows. Linux and Windows have not yet
  been verified by hand on real machines. This includes the sync lock,
  opening links from the terminal UI, and terminal rendering.
- File permission checks for `--token-file` are done on Unix-like systems
  only.
- GitHub Enterprise Server (`GHUSAGE_API_URL`) has not been tested.
  `ghusage pr` and opening links in the terminal UI only accept
  `github.com` URLs.

## Terminal UI

- The terminal UI always uses the committer date for commits;
  `--commit-date author` is not applied.
- Changing the time range with `t` does not sync the new range by itself;
  press `r`.
- With very large caches, the first view of a screen after changing the
  period or filter can take a noticeable moment.
