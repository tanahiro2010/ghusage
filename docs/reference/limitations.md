# Known limitations

`ghusage` can only report what GitHub's APIs expose to your token. Where data
is missing or uncertain, it reports the gap (coverage) or leaves the value
unknown rather than guessing. This page lists the limitations you should know
before relying on the numbers.

## Commits

- **Commits are found on default branches.** Commits are discovered in two
  ways, and a commit found by both counts once: GitHub's commit search, and
  the commit list of the default branch of every repository in your
  repository list that was pushed to in the period, every repository
  where search found your commits, and every repository of a synced pull
  request. Commit search alone
  is not enough: its index can silently miss commits, even weeks after they
  were pushed to an indexed repository, so `ghusage` does not rely on it.
- **Commits only on other branches are not counted** unless they are part
  of a pull request that `ghusage` synced (because it involves you). Work on
  a feature branch without a pull request, or on a branch that was deleted
  without merging, is outside the commit scope.
- **Repository discovery depends on who is analyzed.** For the
  authenticated user, the repository list includes repositories you own,
  collaborate on, or can access as an organization member. With `--user`,
  only the other user's own public repositories can be listed; commits in
  other repositories are found only through commit search and pull
  requests. With `--public-only`, private and internal repositories are not
  listed.
- **Forks are not listed.** A fork's default branch usually repeats the
  commits of its upstream repository, so forks are skipped (commit search
  skips them as well). Your commits in a fork are counted only when they are
  part of a synced pull request.
- **Copies of your commits in unrelated repositories are not counted.**
  Commit search also finds your commits in repositories that only copied
  them, for example a project uploaded again as a new repository instead of
  forked. Search results count only in repositories you are related to:
  ones you own, ones in your repository list, and ones with your pull
  requests or issues. Your commits in a repository you are related to only
  in another way (you starred or forked it, or, with `--user`, the other
  user collaborates on it without pull requests or issues there) are
  therefore not counted. When the repository list fails, every search
  result is kept and commits coverage is `partial`.
- **Repositories you cannot read.** When the commit list of a repository is
  refused (for example HTTP 403 because of organization SSO, or 404 after
  access was removed), commits coverage is `partial` and names the
  repository, and the repository is marked as no longer accessible. Empty
  repositories are skipped without affecting coverage.
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
  search and the commit lists are queried by committer date. With `--commit-date author`, commits
  whose committer date lies outside the synced ranges are not in the cache.
- **Force-pushed history.** When a force push removes commits from a pull
  request, the removed commits are kept and marked "no longer in the PR" if
  `ghusage` had seen them before the force push. Commits that were pushed and
  force-pushed away before the first sync cannot be reconstructed. Your
  commits removed this way still count in "Commits"; they no longer count in
  "PR commits".

## Line counts

See [Lines changed](./metrics.md#lines-changed) for the definitions.

- **The rule for commits counted through other commits is provisional.** It
  decides from the synced pull requests only, and can be wrong:
  - a commit pushed directly to the default branch that is also part of a
    pull request merged by squash or rebase is left out, although it reached
    the default branch itself;
  - a pull request merged into a branch other than the default branch is
    treated like any other merged pull request, whether or not its changes
    ever reached the default branch;
  - pull requests synced before `ghusage` recorded the merge method have an
    unknown merge method, so their commits are left out. Run
    `ghusage sync --full` for the period to record it.

  The rule may be refined in a future version.
- **GitHub's numbers are used as they are.** The line counts of a commit are
  what GitHub reports. For a merge commit, they are against its first
  parent, which is why merge commits are left out of the totals. Binary
  files, renames and generated files are counted as GitHub counts them.
- **Line counts of commits outside pull requests are fetched only on
  request** (`ghusage sync --diff-stats`). Until then the totals are
  `partial` or `unavailable`, never zero.
- **Pull request changes are a snapshot.** A pull request's diff is the one
  seen at the last sync and changes with pushes, force pushes and updates of
  the base branch. Its history is not kept.

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
  for earlier commits of the pull request are not fetched; those stored by
  an earlier sync, while the commit was the head, are kept.
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
- Repositories that GitHub refuses during a sync (HTTP 403 or 404, for
  example after access was removed) keep their cached data and are shown
  with `(no access)`; coverage for the affected range is `partial`.
- With `--user`, another user's activity is limited to repositories your
  token can see. Ownership for other users is based on the repository owner
  only.

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
- Waiting for an exhausted primary rate limit has been checked against the
  real API by using up the search limit. Secondary rate limits and resets
  more than 15 minutes away are covered only by automated tests that
  simulate GitHub's responses.
- When GitHub cannot be reached, report commands fall back to cached data
  with a warning, but nothing new is fetched; `ghusage sync` needs a working
  connection.

## Platforms

- `ghusage` is developed on macOS (Apple Silicon), where it is used against
  real GitHub data.
- Continuous integration builds and runs the test suite on Linux (x86_64 and
  ARM64), macOS (ARM64 and Intel) and Windows (x86_64). The npm binaries,
  including Windows ARM64, are built and smoke-tested (installed with npm and
  run) on each of these platforms; the full test suite does not run on
  Windows ARM64.
- Windows (x86_64) has been checked by hand on a real machine: sign-in,
  sync progress, tables, the terminal UI (tabs, detail views, resizing,
  wide characters, restoring the terminal) and opening links. Running two
  syncs at once (the sync lock) has not been checked by hand on Windows.
- Linux has not been checked by hand on a real machine; there it is covered
  by continuous integration only, including a smoke test that drives the
  terminal UI in a pseudo-terminal.
- File permission checks for `--token-file` are done on Unix-like systems
  only.
- GitHub Enterprise Server (`GHUSAGE_API_URL`) has not been tested.
  `ghusage pr` and opening links in the terminal UI only accept
  `github.com` URLs.

## Terminal UI

- Changing the time range with `t` does not sync the new range by itself;
  press `r`.
- With very large caches, the first view of a screen after changing the
  period or filter is computed in the background and can take a noticeable
  moment.
- In narrow terminals the Repositories tab hides its less important
  columns (PR events, active PRs, issues, -Lines, +Lines, merged, reviews,
  then visibility) to keep repository names readable. Very long names can still be shortened;
  widen the terminal or open the repository detail with Enter to see the
  full name.
- On macOS and Linux, `SIGTERM` and `SIGHUP` end the terminal UI like `q`
  and restore the terminal. A process killed with `SIGKILL` (`kill -9`)
  cannot restore it; run `reset` in that case.

## Cache files

- New data directories are created readable only by you (`0700`) and the
  database as `0600`. An existing directory keeps its permissions; on a
  shared machine, check them with `ls -ld` and tighten them with
  `chmod 700` if needed.
