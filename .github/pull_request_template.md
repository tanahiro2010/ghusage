<!--
Use a Conventional Commits title, for example:
  feat(github): collect pull request reviews
  fix(metrics): deduplicate pull request events
Open pull requests against `dev`. `release` only receives merges from `dev`.
Do not paste tokens or private repository names, titles or URLs.
-->

## Summary

<!-- What changes for users, and why. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Change to how a number is counted
- [ ] Performance
- [ ] Refactoring (no behavior change)
- [ ] Documentation
- [ ] Build, CI or packaging
- [ ] Breaking change (CLI options, JSON/CSV schema, exit codes, cache)

## Related issues

<!-- For example: Fixes #123 -->

## Checks

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] `cargo build --workspace --release`
- [ ] Added or updated tests (including time zone and date-boundary cases where relevant)

## Numbers

<!--
If this changes any number: which metrics change, and how you verified the
new values (for example `ghusage pr <URL> --timeline`, `--format json`
before/after, or the value from GitHub / `gh api`). Confirm that re-syncing
the same data does not change counts. Delete this section otherwise.
-->

## Documentation

- [ ] Not needed
- [ ] Updated the English documentation (`README.md`, `docs/`)
- [ ] Updated the Japanese documentation (`docs/ja/`)

## Screenshots

<!-- For changes to table output or the terminal UI: before/after screenshots or text output. Delete otherwise. -->
