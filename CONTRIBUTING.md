# Contributing

## Pull requests

- Open pull requests against `dev`. The only exception is a release pull
  request from `dev` to `release`.
- Stacked pull requests are not used in this repository. Do not set a pull
  request's base to another unmerged feature branch. If work depends on an
  open pull request, wait for it to merge into `dev`, then update your branch
  from `dev` and open the dependent pull request against `dev`.
- Keep each pull request focused on one cohesive change. Split unrelated work
  into separate pull requests.

## Choosing a merge method

Use **Squash and merge by default**. It records the pull request as one commit
and keeps temporary implementation and review-fix commits out of `dev`.

Use **Create a merge commit** only when preserving the pull request's
individual commits is useful for understanding or reverting the history. Each
commit to be preserved must:

- represent a complete, independently meaningful change;
- use a [Conventional Commit](https://www.conventionalcommits.org/) message;
- pass the relevant build and tests on its own.

For the release pull request from `dev` to `release`, use **Create a merge
commit** to preserve the `dev` history.

Commit count is a prompt to review and curate history, not an automatic merge
method selector:

- **1–2 commits:** Squash and merge is the normal choice.
- **3–5 commits:** review whether each commit is independently useful. Use
  interactive rebase to combine temporary or overlapping commits before
  choosing a merge method.
- **6 or more commits:** curate the series with interactive rebase before
  merge. Use a merge commit only if the resulting commits all meet the criteria
  above; otherwise squash.

Do not use GitHub's **Rebase and merge**. It rewrites commit SHAs, making
references from reviews and pull request descriptions harder to follow.

Large diffs are a reason to consider splitting a pull request, not a reason by
themselves to preserve its commits. The automated pull request comment flags
20 or more changed files or 1,000 or more changed lines as a prompt to review
the scope. These are advisory thresholds and include generated and lock files.

The automated comment checks the base branch, commit count, commit message
format, obvious temporary commit markers, and diff size. It cannot determine
whether a commit is independently meaningful or whether its tests pass alone;
contributors and maintainers make the final choice.
