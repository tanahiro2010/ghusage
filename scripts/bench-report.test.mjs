// Run with `node --test scripts/bench-report.test.mjs`.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { buildReport, change, formatNs, main, parseLatency, readCriterion, verdict } from "./bench-report.mjs";

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
  assert.match(md, /\| cached 7-day aggregation \(load \+ overview\) \| 40\.0 ms \| 250\.0 ms \| \+525\.0% \| ≤ 200 ms :x: \*\*fail\*\* \| :red_circle: slower \|/);
  assert.match(md, /\| 100k event aggregation \(overview, 1 year\) \| 90\.0 ms \| 91\.0 ms \| \+1\.1% \| ≤ 1000 ms pass \| ≈ \|/);
  assert.match(md, /more than 5%/);
});

test("marks latency scenarios that exist on only one side", () => {
  const md = buildReport({
    latencyBase: parseLatency(LATENCY_BASE),
    latencyHead: parseLatency(LATENCY_HEAD.split("\n")[1]),
    runner: "r",
  });
  assert.match(md, /\| 100k event aggregation \(overview, 1 year\) \| 90\.0 ms \| – \| – \| – \| removed \|/);
  assert.match(md, /No Criterion results were found/);
});
