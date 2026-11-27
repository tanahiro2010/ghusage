#!/usr/bin/env node
// Builds a Markdown comparison of two benchmark runs made on the same machine.
//
// Criterion results are read from its machine-readable output: every
// benchmark directory under the Criterion output directory holds one
// sub-directory per saved baseline (`--save-baseline <name>`), each with a
// `benchmark.json` (the benchmark id) and an `estimates.json` (timings in ns).
// The latency percentiles come from the `latency` bench's stdout.
//
// Usage:
//   node scripts/bench-report.mjs --criterion target/criterion \
//     --base-name base --head-name head \
//     [--latency-base base.txt --latency-head head.txt] \
//     [--base-sha SHA --head-sha SHA --base-ref release] \
//     [--threshold 10] [--marker '<!-- ghusage-bench -->'] [--out report.md]

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const DEFAULT_MARKER = "<!-- ghusage-bench -->";

/** Reads `{ id -> median ns }` for one saved baseline. */
export function readCriterion(root, baseline) {
  const out = new Map();
  if (!root || !existsSync(root)) return out;
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const sub = path.join(dir, entry);
      if (!statSync(sub).isDirectory()) continue;
      const est = path.join(sub, baseline, "estimates.json");
      const meta = path.join(sub, baseline, "benchmark.json");
      if (existsSync(est) && existsSync(meta)) {
        const id = JSON.parse(readFileSync(meta, "utf8")).full_id;
        const e = JSON.parse(readFileSync(est, "utf8"));
        const median = e.median ?? e.mean;
        if (id && median && Number.isFinite(median.point_estimate)) {
          out.set(id, {
            ns: median.point_estimate,
            lo: median.confidence_interval?.lower_bound,
            hi: median.confidence_interval?.upper_bound,
          });
        }
      } else if (entry !== "report" && !["new", "base", "change"].includes(entry)) {
        // Benchmark groups nest one level deeper (`group/function/...`).
        walk(sub);
      }
    }
  };
  walk(root);
  return out;
}

const LATENCY_LINE =
  /^(.+): p50 ([\d.]+) ms, p95 ([\d.]+) ms, max ([\d.]+) ms \(target p95 <= (\d+) ms\) (PASS|FAIL)\s*$/;

/** Parses the `latency` bench's stdout into `{ name -> { p50, p95, target, pass } }` (ms). */
export function parseLatency(text) {
  const out = new Map();
  for (const line of (text ?? "").split(/\r?\n/)) {
    const m = LATENCY_LINE.exec(line);
    if (m) {
      out.set(m[1], { p50: +m[2], p95: +m[3], target: +m[5], pass: m[6] === "PASS" });
    }
  }
  return out;
}

export function formatNs(ns) {
  if (!Number.isFinite(ns)) return "–";
  const units = [
    [1e9, "s"],
    [1e6, "ms"],
    [1e3, "µs"],
  ];
  for (const [scale, unit] of units) {
    if (ns >= scale) return `${(ns / scale).toFixed(2)} ${unit}`;
  }
  return `${ns.toFixed(1)} ns`;
}

/** Percentage change from base to head, or null if not comparable. */
export function change(base, head) {
  if (!Number.isFinite(base) || !Number.isFinite(head) || base <= 0) return null;
  return ((head - base) / base) * 100;
}

/**
 * Whether `value` is beyond `limit`. Percentages computed from values printed
 * in fixed steps land a rounding error off the limit (1.2 -> 1.8 ms is
 * 50.000000000000014%, 2.0 -> 3.0 ms is exactly 50%), so the margin keeps
 * changes of exactly the limit on the same side.
 */
function beyond(value, limit) {
  return value > limit + 1e-9;
}

export function verdict(pct, threshold) {
  if (pct === null) return "";
  if (beyond(pct, threshold)) return ":red_circle: slower";
  if (beyond(-pct, threshold)) return ":green_circle: faster";
  return "≈";
}

/** p95 changes beyond this (in %) are shown even when p50 and the median stay put. */
export const P95_ONLY_THRESHOLD = 50;

/**
 * The latency bench prints times in steps of 0.1 ms, so scenarios that take
 * about a millisecond or less move by tens of percent from a step or two.
 * Between runs of unchanged code on shared runners, p95 of the scenarios
 * under 3 ms moved by at most 0.4 ms; p95 changes smaller than this (in ms)
 * are not marked.
 */
export const MIN_LATENCY_CHANGE_MS = 0.5;

/** The Criterion benchmark that measures the same work as a latency scenario, if any. */
export function criterionTwin(scenario) {
  let m;
  if ((m = /^100k event aggregation \((.+), 1 year\)$/.exec(scenario))) return `aggregate 100k events (${m[1]}, 1 year)`;
  if ((m = /^cached (\d+)-day database load$/.exec(scenario))) return `load 100k event dataset (${m[1]} days)`;
  if ((m = /^cached pull request detail load \((.+)\)$/.exec(scenario))) return `load pull request detail (${m[1]})`;
  if (/^cached \d+-day aggregation \(load \+ overview\)$/.test(scenario)) return scenario;
  return null;
}

/**
 * Marks a latency scenario only when p95, p50 and the Criterion median (when
 * the scenario has one) all moved past the threshold in the same direction.
 * The latency bench repeats each scenario a few dozen times, so its p95 rests
 * on the slowest few runs and, on shared runners, swings by up to about 30%
 * between runs of unchanged code; p50 and Criterion's median are steadier.
 * A scenario without a Criterion counterpart is confirmed by p50 alone.
 * An unconfirmed p95 change beyond `P95_ONLY_THRESHOLD` is still shown, but
 * not counted as slower or faster. A p95 change smaller than
 * `MIN_LATENCY_CHANGE_MS` (`p95Ms`, the change in ms, when known) is not
 * marked at all.
 */
export function latencyVerdict(p95, p50, median, threshold, p95Ms) {
  const mark = verdict(p95, threshold);
  if (mark === "" || mark === "≈") return mark;
  // 0.7 - 0.2 is 0.49999999999999994, so the same margin applies here.
  if (Number.isFinite(p95Ms) && Math.abs(p95Ms) < MIN_LATENCY_CHANGE_MS - 1e-9) return "≈";
  const agrees = (pct) => verdict(pct, threshold) === mark;
  if (agrees(p50) && (median == null || agrees(median))) return mark;
  return beyond(Math.abs(p95), P95_ONLY_THRESHOLD) ? ":warning: p95 only" : "≈";
}

function formatPct(pct) {
  if (pct === null) return "–";
  return `${pct > 0 ? "+" : ""}${pct.toFixed(1)}%`;
}

/** Keeps benchmark names from breaking the table or injecting markup. */
function cell(s) {
  return String(s).replace(/[|\\`*_<>[\]]/g, (c) => `\\${c}`).replace(/\r?\n/g, " ");
}

function runnerInfo() {
  const cpus = os.cpus();
  const model = cpus[0]?.model?.trim() || "unknown CPU";
  return `${os.type()} ${os.release()} (${os.arch()}), ${model}, ${cpus.length} logical CPUs`;
}

export function buildReport(opts) {
  const threshold = opts.threshold ?? 10;
  const marker = opts.marker ?? DEFAULT_MARKER;
  const lines = [marker, "## Benchmark comparison", ""];
  const short = (sha) => (sha ? `\`${String(sha).slice(0, 12)}\`` : "unknown");
  lines.push(
    `Base ${opts.baseRef ? `(\`${cell(opts.baseRef)}\`) ` : ""}${short(opts.baseSha)} vs. PR ${short(opts.headSha)}, both measured on the same runner: ${opts.runner ?? runnerInfo()}.`,
    "",
  );

  const base = opts.criterionBase ?? new Map();
  const head = opts.criterionHead ?? new Map();
  const ids = [...new Set([...base.keys(), ...head.keys()])].sort();
  let regressions = 0;
  let improvements = 0;
  if (ids.length) {
    lines.push("### Criterion (median time)", "");
    lines.push("| Benchmark | Base | PR | Change | |", "|---|---:|---:|---:|---|");
    for (const id of ids) {
      const b = base.get(id);
      const h = head.get(id);
      const pct = change(b?.ns, h?.ns);
      let mark = verdict(pct, threshold);
      if (!b) mark = "new";
      if (!h) mark = "removed";
      if (mark.includes("slower")) regressions++;
      if (mark.includes("faster")) improvements++;
      lines.push(`| ${cell(id)} | ${formatNs(b?.ns)} | ${formatNs(h?.ns)} | ${formatPct(pct)} | ${mark} |`);
    }
    lines.push("");
  } else {
    lines.push("No Criterion results were found.", "");
  }

  const lb = opts.latencyBase ?? new Map();
  const lh = opts.latencyHead ?? new Map();
  const names = [...new Set([...lb.keys(), ...lh.keys()])];
  if (names.length) {
    lines.push("### Latency percentiles (p50, p95)", "");
    lines.push(
      "| Scenario | Base p50 | PR p50 | Change | Base p95 | PR p95 | Change | PR target | |",
      "|---|---:|---:|---:|---:|---:|---:|---|---|",
    );
    for (const name of names) {
      const b = lb.get(name);
      const h = lh.get(name);
      const p50 = change(b?.p50, h?.p50);
      const p95 = change(b?.p95, h?.p95);
      const twin = criterionTwin(name);
      const median = twin && base.has(twin) && head.has(twin) ? change(base.get(twin).ns, head.get(twin).ns) : undefined;
      let mark = latencyVerdict(p95, p50, median, threshold, h?.p95 - b?.p95);
      if (!b) mark = "new";
      if (!h) mark = "removed";
      if (mark.includes("slower")) regressions++;
      if (mark.includes("faster")) improvements++;
      const ms = (v) => (Number.isFinite(v) ? `${v.toFixed(1)} ms` : "–");
      const target = h ? `≤ ${h.target} ms ${h.pass ? "pass" : ":x: **fail**"}` : "–";
      lines.push(
        `| ${cell(name)} | ${ms(b?.p50)} | ${ms(h?.p50)} | ${formatPct(p50)} | ${ms(b?.p95)} | ${ms(h?.p95)} | ${formatPct(p95)} | ${target} | ${mark} |`,
      );
    }
    lines.push("");
  }

  lines.push(
    `**${regressions}** slower and **${improvements}** faster by more than ${threshold}%.`,
    "",
    `> [!NOTE]`,
    `> Shared CI runners are noisy: differences of a few percent, and occasionally more, can be measurement noise. ` +
      `The base is measured first and the PR right after on the same machine; re-run the workflow or benchmark locally before acting on a single result.`,
    `>`,
    `> A latency p95 change is marked only when p50 and the scenario's Criterion median (if it has one) also moved by more than ${threshold}% in the same direction. ` +
      `On its own, p95 can move by up to about 30% between runs of unchanged code. ` +
      `When p95 alone moves by more than ${P95_ONLY_THRESHOLD}%, the row says "p95 only" and is not counted above; check it before relying on the result. ` +
      `The latency bench reports times in steps of 0.1 ms, so p95 changes smaller than ${MIN_LATENCY_CHANGE_MS} ms are not marked.`,
    "",
  );
  return lines.join("\n");
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) throw new Error(`unexpected argument: ${a}`);
    const v = argv[i + 1];
    if (v === undefined) throw new Error(`missing value for ${a}`);
    args[a.slice(2)] = v;
    i++;
  }
  return args;
}

function readOptional(file) {
  return file && existsSync(file) ? readFileSync(file, "utf8") : "";
}

export function main(argv) {
  const a = parseArgs(argv);
  const report = buildReport({
    criterionBase: readCriterion(a.criterion, a["base-name"] ?? "base"),
    criterionHead: readCriterion(a.criterion, a["head-name"] ?? "head"),
    latencyBase: parseLatency(readOptional(a["latency-base"])),
    latencyHead: parseLatency(readOptional(a["latency-head"])),
    baseSha: a["base-sha"],
    headSha: a["head-sha"],
    baseRef: a["base-ref"],
    threshold: a.threshold === undefined ? 10 : Number(a.threshold),
    marker: a.marker,
    runner: a.runner,
  });
  if (a.out) writeFileSync(a.out, report);
  else process.stdout.write(report);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
