# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

ghusage is a Rust CLI/TUI that collects, stores, analyzes, and visualizes a developer's GitHub activity: commits, PRs, reviews, issues, and the events inside a PR. This repository is public OSS. Commit messages follow Conventional Commits and describe the product change (e.g. `feat(github): collect pull request reviews`).

## Commands

Workspace crates: `ghusage-core`, `ghusage-github`, `ghusage-storage`, `ghusage-app`, `ghusage-tui`; the root package builds the `ghusage` binary. The required CI gates (run all four before considering work done):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --release
```

Run a single test with `cargo test -p <crate> <test_name>`. Never skip, delete, or relax a failing or flaky test to make CI pass.

## Architecture

Stack: clap (CLI), ratatui + crossterm (TUI), Tokio, reqwest (GitHub GraphQL + REST), serde, SQLite via rusqlite, chrono/chrono-tz, tracing, thiserror/anyhow, Criterion (benches).

Layers: **Core** (data model and aggregation) → **GitHub** (API and auth) / **Storage** (SQLite cache, migrations) → **Application** (sync, query, analysis) → **CLI** and **TUI**. The CLI and TUI must share the same Core and Application services so they always produce identical numbers.

Core design rules that span the codebase:

- **Everything is an event.** All activity is normalized into a common event model: `event_id`, `event_type`, `actor_id`, `repository_id`, `entity_id`, `parent_pr_id`, `occurred_at`, `observed_at`, `source`, `url`, `metadata`, `coverage`. A PR is a process made of many events (opened, commits pushed, review requested/submitted, comment, thread resolved, check run, merged), not a single activity. Actor identity separates the user's own activity from other people's and bots'.
- **Idempotent, deterministic aggregation.** Re-fetching the same data must never increase counts, and uniqueness is enforced by DB unique constraints too. Daily totals must sum to the period total. Unique PR count is never inflated by multiple events on one PR. A commit SHA seen both globally and inside a PR is counted once globally.
- **Keep similar-looking activity separate.** A review submission is different from a review comment. A conversation comment is different from an inline comment. Reviews submitted are different from reviews received.
- **Current state vs. historical events.** Current state (PR status, latest CI result, thread resolution) must never be used in place of history. For example, a PR merged later is not counted as merged on its creation date.
- **Coverage is tracked.** Sync records cursors and coverage intervals (`sync_runs`, `sync_cursors`, `coverage_intervals`). Never present partially fetched data as complete. Sync must handle pagination, rate limits, timeouts, partial responses, edits/deletes, and force-pushes.
- Timezone and date-boundary handling is a first-class correctness concern and needs tests.
- GitHub content (titles, bodies, comments) is untrusted. Sanitize it before rendering in the terminal.
