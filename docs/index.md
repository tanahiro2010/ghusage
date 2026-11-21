---
layout: home

hero:
  name: ghusage
  text: Know what you actually did on GitHub.
  tagline: A CLI and terminal UI that analyzes your GitHub activity, including everything that happens inside pull requests.
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Metric definitions
      link: /reference/metrics
    - theme: alt
      text: View on GitHub
      link: https://github.com/tanahiro2010/ghusage

features:
  - title: Pull requests as a process
    details: Commits, reviews and their states, inline versus conversation comments, review threads, force pushes and the full timeline of each pull request.
  - title: You, others and bots kept apart
    details: Comments written versus received, reviews submitted versus received, your merges versus your pull requests being merged. CI results are never counted as your activity.
  - title: Precise, documented numbers
    details: Every metric has a written definition. Unique counts are labeled, current-state values are marked, and statistics are descriptive rather than scores.
  - title: Honest about gaps
    details: Every report shows coverage. Partial or missing data is reported, and --require-complete makes scripts fail instead of silently using incomplete numbers.
  - title: Local and offline
    details: Data lives in a local SQLite cache and is synced incrementally. Reports run offline. No telemetry.
  - title: Table, JSON, CSV and TUI
    details: Human-readable tables, a versioned JSON schema, flat CSV, and an interactive terminal UI that syncs in the background.
---
