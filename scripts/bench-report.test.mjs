// Run with `node --test scripts/bench-report.test.mjs`.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  buildReport,
  change,
  criterionTwin,
  formatNs,
  latencyVerdict,
  main,
  parseLatency,
  readCriterion,
  verdict,
} from "./bench-report.mjs";

function writeBaseline(root, dir, baseline, id, medianNs) {
  const d = path.join(root, dir, baseline);
  mkdirSync(d, { recursive: true });
  writeFileSync(
    path.join(d, "benchmark.json"),
    JSON.stringify({ group_id: id, function_id: null, full_id: id, directory_name: dir, title: id }),
  );
  const est = (v) => ({
    confidence_interval: { confidence_level: 0.95, lower_bound: v * 0.98, upper_bound: v * 1.02 },
    point_estimate: v,
    standard_error: v * 0.01,
  });
  writeFileSync(
    path.join(d, "estimates.json"),
    JSON.stringify({ mean: est(medianNs * 1.1), median: est(medianNs), slope: null }),
  );
}

function fixture() {
  const root = mkdtempSync(path.join(os.tmpdir(), "bench-report-"));
  writeBaseline(root, "slow (1 year)", "base", "slow (1 year)", 100e6);
  writeBaseline(root, "slow (1 year)", "head", "slow (1 year)", 125e6);
  writeBaseline(root, "fast", "base", "fast", 2e6);
  writeBaseline(root, "fast", "head", "fast", 1e6);
  writeBaseline(root, "steady", "base", "steady", 500e3);
  writeBaseline(root, "steady", "head", "steady", 510e3);
  writeBaseline(root, "added", "head", "added", 42);
  // Criterion keeps its own `new` results next to saved baselines; they must be ignored.
  writeBaseline(root, "steady", "new", "steady", 1);
  // Grouped benchmarks nest one directory deeper.
  writeBaseline(root, path.join("group", "fn"), "base", "group/fn", 10e3);
  writeBaseline(root, path.join("group", "fn"), "head", "group/fn", 10e3);
  mkdirSync(path.join(root, "report"), { recursive: true });
  return root;
}

const LATENCY_BASE = `dataset: 100000 events, 2000 pull requests, 40 repositories, 50 runs after 5 warm-up
100k event aggregation (overview, 1 year): p50 80.0 ms, p95 90.0 ms, max 95.0 ms (target p95 <= 1000 ms) PASS
cached 7-day aggregation (load + overview): p50 30.0 ms, p95 40.0 ms, max 41.0 ms (target p95 <= 200 ms) PASS
`;
const LATENCY_HEAD = `100k event aggregation (overview, 1 year): p50 81.0 ms, p95 91.0 ms, max 99.0 ms (target p95 <= 1000 ms) PASS
cached 7-day aggregation (load + overview): p50 230.0 ms, p95 250.0 ms, max 260.0 ms (target p95 <= 200 ms) FAIL
`;

test("reads medians for one saved baseline, including nested groups", () => {
  const root = fixture();
  try {
    const base = readCriterion(root, "base");
    assert.deepEqual([...base.keys()].sort(), ["fast", "group/fn", "slow (1 year)", "steady"]);
    assert.equal(base.get("slow (1 year)").ns, 100e6);
    assert.equal(readCriterion(root, "head").get("added").ns, 42);
    assert.equal(readCriterion(path.join(root, "missing"), "base").size, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("parses latency lines and ignores everything else", () => {
  const m = parseLatency(LATENCY_HEAD);
  assert.equal(m.size, 2);
  assert.deepEqual(m.get("cached 7-day aggregation (load + overview)"), {
    p50: 230,
    p95: 250,
    target: 200,
    pass: false,
  });
  assert.equal(parseLatency("latency percentiles are measured with `cargo bench`; skipped").size, 0);
});

test("formats durations and changes", () => {
  assert.equal(formatNs(1.5e9), "1.50 s");
  assert.equal(formatNs(82.26e6), "82.26 ms");
  assert.equal(formatNs(1500), "1.50 µs");
  assert.equal(formatNs(42), "42.0 ns");
  assert.equal(change(100, 110), 10);
  assert.equal(change(0, 1), null);
  assert.equal(change(undefined, 1), null);
  assert.match(verdict(10.1, 10), /slower/);
  assert.match(verdict(-10.1, 10), /faster/);
  assert.equal(verdict(10, 10), "≈");
});

test("builds a report that marks regressions and improvements", () => {
  const root = fixture();
  try {
    const out = path.join(root, "report.md");
    main([
      "--criterion", root,
      "--base-name", "base",
      "--head-name", "head",
      "--base-sha", "0123456789abcdef",
      "--head-sha", "fedcba9876543210",
      "--base-ref", "release",
      "--runner", "Linux test runner",
      "--out", out,
    ]);
    const md = readFileSync(out, "utf8");
    assert.ok(md.startsWith("<!-- ghusage-bench -->\n"));
    assert.match(md, /`0123456789ab` vs\. PR `fedcba987654`/);
    assert.match(md, /Linux test runner/);
    assert.match(md, /\| slow \(1 year\) \| 100\.00 ms \| 125\.00 ms \| \+25\.0% \| :red_circle: slower \|/);
    assert.match(md, /\| fast \| 2\.00 ms \| 1\.00 ms \| -50\.0% \| :green_circle: faster \|/);
    assert.match(md, /\| steady \| 500\.00 µs \| 510\.00 µs \| \+2\.0% \| ≈ \|/);
    assert.match(md, /\| added \| – \| 42\.0 ns \| – \| new \|/);
    assert.match(md, /\*\*1\*\* slower and \*\*1\*\* faster by more than 10%/);
    assert.match(md, /noisy/);
    assert.doesNotMatch(md, /Latency/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("includes latency percentiles and escapes table syntax in names", () => {
  const md = buildReport({
    criterionBase: new Map([["a|b <x>", { ns: 1e6 }]]),
    criterionHead: new Map([["a|b <x>", { ns: 1e6 }]]),
    latencyBase: parseLatency(LATENCY_BASE),
    latencyHead: parseLatency(LATENCY_HEAD),
    threshold: 5,
    runner: "r",
  });
  assert.match(md, /\| a\\\|b \\<x\\> \|/);
  assert.match(md, /\| cached 7-day aggregation \(load \+ overview\) \| 30\.0 ms \| 230\.0 ms \| \+666\.7% \| 40\.0 ms \| 250\.0 ms \| \+525\.0% \| ≤ 200 ms :x: \*\*fail\*\* \| :red_circle: slower \|/);
  assert.match(md, /\| 100k event aggregation \(overview, 1 year\) \| 80\.0 ms \| 81\.0 ms \| \+1\.3% \| 90\.0 ms \| 91\.0 ms \| \+1\.1% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /more than 5%/);
});

test("marks latency scenarios that exist on only one side", () => {
  const md = buildReport({
    latencyBase: parseLatency(LATENCY_BASE),
    latencyHead: parseLatency(LATENCY_HEAD.split("\n")[1]),
    runner: "r",
  });
  assert.match(md, /\| 100k event aggregation \(overview, 1 year\) \| 80\.0 ms \| – \| – \| 90\.0 ms \| – \| – \| – \| removed \|/);
  assert.match(md, /No Criterion results were found/);
});

// Latency lines for the `latency` bench, one per scenario.
function latencyLines(rows) {
  return rows
    .map(([name, p50, p95]) => `${name}: p50 ${p50} ms, p95 ${p95} ms, max ${p95 + 1} ms (target p95 <= 1000 ms) PASS`)
    .join("\n");
}

test("does not mark a p95 change when the median stays put", () => {
  // Measured on a shared runner for a change that did not touch aggregation:
  // p95 moved by up to a third while p50 and the Criterion median stayed within a few percent.
  const md = buildReport({
    criterionBase: new Map([
      ["aggregate 100k events (weekly, 1 year)", { ns: 92.52e6 }],
      ["load pull request detail (PR_0)", { ns: 258.36e3 }],
    ]),
    criterionHead: new Map([
      ["aggregate 100k events (weekly, 1 year)", { ns: 91.32e6 }],
      ["load pull request detail (PR_0)", { ns: 260.36e3 }],
    ]),
    latencyBase: parseLatency(
      latencyLines([
        ["100k event aggregation (weekly, 1 year)", 84.0, 93.3],
        ["cached pull request detail load (PR_0)", 0.3, 0.3],
        ["cached 7-day report aggregation (preloaded)", 1.9, 2.1],
      ]),
    ),
    latencyHead: parseLatency(
      latencyLines([
        ["100k event aggregation (weekly, 1 year)", 91.3, 109.1],
        ["cached pull request detail load (PR_0)", 0.3, 0.2],
        ["cached 7-day report aggregation (preloaded)", 2.0, 2.5],
      ]),
    ),
    runner: "r",
  });
  assert.match(md, /\| 100k event aggregation \(weekly, 1 year\) \| 84\.0 ms \| 91\.3 ms \| \+8\.7% \| 93\.3 ms \| 109\.1 ms \| \+16\.9% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /\| cached pull request detail load \(PR\\_0\) \|.*\| -33\.3% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /\| cached 7-day report aggregation \(preloaded\) \|.*\| \+19\.0% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /\*\*0\*\* slower and \*\*0\*\* faster/);
});

test("does not mark a p95 change when p50 moves but the Criterion median does not", () => {
  const md = buildReport({
    criterionBase: new Map([["cached 7-day aggregation (load + overview)", { ns: 160e6 }]]),
    criterionHead: new Map([["cached 7-day aggregation (load + overview)", { ns: 161e6 }]]),
    latencyBase: parseLatency(latencyLines([["cached 7-day aggregation (load + overview)", 180.0, 200.0]])),
    latencyHead: parseLatency(latencyLines([["cached 7-day aggregation (load + overview)", 150.0, 165.4]])),
    runner: "r",
  });
  assert.match(md, /\| cached 7-day aggregation \(load \+ overview\) \|.*\| -17\.3% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /\*\*0\*\* slower and \*\*0\*\* faster/);
});

test("marks a real regression where the median moves with p95", () => {
  const md = buildReport({
    criterionBase: new Map([
      ["aggregate 100k events (daily, 1 year)", { ns: 90e6 }],
      ["load 100k event dataset (30 days)", { ns: 240e6 }],
    ]),
    criterionHead: new Map([
      ["aggregate 100k events (daily, 1 year)", { ns: 112.5e6 }],
      ["load 100k event dataset (30 days)", { ns: 120e6 }],
    ]),
    latencyBase: parseLatency(
      latencyLines([
        ["100k event aggregation (daily, 1 year)", 90.0, 100.0],
        ["cached 30-day database load", 240.0, 250.0],
        ["cached 30-day report aggregation (preloaded)", 8.0, 10.0],
      ]),
    ),
    latencyHead: parseLatency(
      latencyLines([
        ["100k event aggregation (daily, 1 year)", 112.0, 125.0],
        ["cached 30-day database load", 121.0, 125.0],
        // No Criterion counterpart: p50 alone confirms the change.
        ["cached 30-day report aggregation (preloaded)", 10.4, 13.0],
      ]),
    ),
    runner: "r",
  });
  assert.match(md, /\| 100k event aggregation \(daily, 1 year\) \|.*\| \+25\.0% \| ≤ 1000 ms pass \| :red_circle: slower \|/);
  assert.match(md, /\| cached 30-day database load \|.*\| -50\.0% \| ≤ 1000 ms pass \| :green_circle: faster \|/);
  assert.match(md, /\| cached 30-day report aggregation \(preloaded\) \|.*\| \+30\.0% \| ≤ 1000 ms pass \| :red_circle: slower \|/);
  // Criterion: one slower, one faster; latency: two slower, one faster.
  assert.match(md, /\*\*3\*\* slower and \*\*2\*\* faster/);
});

test("explains how latency changes are marked", () => {
  const md = buildReport({
    latencyBase: parseLatency(LATENCY_BASE),
    latencyHead: parseLatency(LATENCY_HEAD),
    runner: "r",
  });
  assert.match(md, /### Latency percentiles \(p50, p95\)/);
  assert.match(md, /p95 change is marked only when p50.*same direction/);
});

test("pairs latency scenarios with the Criterion benchmark of the same work", () => {
  assert.equal(criterionTwin("100k event aggregation (monthly, 1 year)"), "aggregate 100k events (monthly, 1 year)");
  assert.equal(criterionTwin("cached 30-day database load"), "load 100k event dataset (30 days)");
  assert.equal(criterionTwin("cached pull request detail load (PR_0)"), "load pull request detail (PR_0)");
  assert.equal(criterionTwin("cached 7-day aggregation (load + overview)"), "cached 7-day aggregation (load + overview)");
  assert.equal(criterionTwin("cached 7-day report aggregation (preloaded)"), null);
  assert.equal(latencyVerdict(15, 12, undefined, 10), ":red_circle: slower");
  assert.equal(latencyVerdict(15, 12, 2, 10), "≈");
  assert.equal(latencyVerdict(15, 5, 12, 10), "≈");
  assert.equal(latencyVerdict(-15, -12, -11, 10), ":green_circle: faster");
  assert.equal(latencyVerdict(5, 30, 30, 10), "≈");
  assert.equal(latencyVerdict(null, null, undefined, 10), "");
});

test("does not mark a p95 change when p50 or the median moved the other way", () => {
  assert.equal(latencyVerdict(15, -12, undefined, 10), "≈");
  assert.equal(latencyVerdict(-15, 12, undefined, 10), "≈");
  assert.equal(latencyVerdict(15, 12, -12, 10), "≈");
  assert.equal(latencyVerdict(-15, -12, 12, 10), "≈");
});
