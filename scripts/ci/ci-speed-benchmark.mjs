#!/usr/bin/env node

import { appendFileSync, readFileSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import { performance } from "node:perf_hooks";

const argv = process.argv.slice(2);

function option(name, fallback) {
  const index = argv.indexOf(name);
  return index === -1 ? fallback : argv[index + 1];
}

const scenario = option("--scenario", "baseline");
const output = resolve(option("--output", "ci-speed-benchmark.json"));
const packageName = option("--package", "");
const scope = packageName
  ? ["--package", packageName]
  : ["--workspace"];
const testSelection = ["--lib", "--bins", "--tests", "--no-fail-fast"];
const records = [];

function commandLine(command, args) {
  return [command, ...args].map((part) => JSON.stringify(part)).join(" ");
}

function run(label, command, args, extraEnv = {}) {
  return new Promise((resolveRun) => {
    const startedAt = new Date().toISOString();
    const started = performance.now();
    const child = spawn(command, args, {
      cwd: process.cwd(),
      env: { ...process.env, ...extraEnv },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let captured = "";
    const capture = (chunk) => {
      process.stdout.write(chunk);
      if (captured.length < 8192) {
        captured += chunk.toString().slice(0, 8192 - captured.length);
      }
    };
    child.stdout.on("data", capture);
    child.stderr.on("data", capture);
    child.on("error", (error) => {
      const record = {
        label,
        command: commandLine(command, args),
        startedAt,
        durationMs: Math.round(performance.now() - started),
        exitCode: null,
        error: error.message,
        output: captured,
      };
      records.push(record);
      resolveRun(record);
    });
    child.on("close", (exitCode, signal) => {
      const record = {
        label,
        command: commandLine(command, args),
        startedAt,
        durationMs: Math.round(performance.now() - started),
        exitCode,
        signal,
        output: captured,
      };
      records.push(record);
      resolveRun(record);
    });
  });
}

async function required(label, command, args, env = {}) {
  const record = await run(label, command, args, env);
  if (record.exitCode !== 0) {
    throw new Error(`${label} failed with exit code ${record.exitCode}`);
  }
  return record;
}

async function cargoPass(prefix, env = {}) {
  await required(`${prefix}-compile`, "cargo", [
    "test",
    ...scope,
    ...testSelection,
    "--no-run",
  ], env);
  await required(`${prefix}-tests`, "cargo", [
    "test",
    ...scope,
    ...testSelection,
  ], env);
}

function sourcePathForIncrementalProbe() {
  return resolve("crates/ghusage-core/src/lib.rs");
}

function touchWithoutChangingContent(file) {
  const stat = statSync(file);
  const future = new Date(Math.max(Date.now() + 2000, stat.mtimeMs + 2000));
  utimesSync(file, new Date(stat.atimeMs), future);
  return () => utimesSync(file, new Date(stat.atimeMs), new Date(stat.mtimeMs));
}

async function main() {
  const benchmarkEnv = {
    CARGO_TERM_COLOR: "never",
    RUSTFLAGS: process.env.RUSTFLAGS || "",
  };

  if (scenario === "baseline") {
    await cargoPass("baseline", benchmarkEnv);
  } else if (scenario === "nextest") {
    await cargoPass("nextest-compile", benchmarkEnv);
    await required("nextest-tests", "cargo", [
      "nextest",
      "run",
      ...scope,
      "--no-fail-fast",
    ], benchmarkEnv);
  } else if (scenario === "incremental") {
    const probe = sourcePathForIncrementalProbe();
    const restoreMtime = await cargoPass("incremental-first", {
      ...benchmarkEnv,
      CARGO_INCREMENTAL: "1",
    }).then(() => touchWithoutChangingContent(probe));
    try {
      await cargoPass("incremental-after-touch", {
        ...benchmarkEnv,
        CARGO_INCREMENTAL: "1",
      });
    } finally {
      restoreMtime();
    }
  } else if (scenario === "sccache") {
    const sccache = process.env.SCCACHE_PATH || "sccache";
    await required("sccache-clean-first", "cargo", ["clean"]);
    await cargoPass("sccache-first", benchmarkEnv);
    await required("sccache-clean-second", "cargo", ["clean"]);
    await cargoPass("sccache-after-clean", benchmarkEnv);
    await required("sccache-stats", sccache, ["--show-stats"]);
  } else if (scenario === "split") {
    await cargoPass("split", benchmarkEnv);
  } else {
    throw new Error(`unknown scenario: ${scenario}`);
  }

  const result = {
    schemaVersion: 1,
    scenario,
    package: packageName || null,
    runnerOs: process.env.RUNNER_OS || process.platform,
    runnerName: process.env.RUNNER_NAME || null,
    cacheMode: process.env.CI_SPEED_CACHE_MODE || null,
    rustCacheHit: process.env.CI_SPEED_RUST_CACHE_HIT || "not-run",
    cargoIncremental: process.env.CARGO_INCREMENTAL || "0",
    startedAt: records[0]?.startedAt || new Date().toISOString(),
    totalDurationMs: records.reduce((total, record) => total + record.durationMs, 0),
    records: records.map(({ output, ...record }) => record),
  };
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, `${JSON.stringify(result, null, 2)}\n`);

  const lines = [
    `### CI speed benchmark: ${scenario}${packageName ? ` (${packageName})` : ""}`,
    "",
    `- runner: \`${result.runnerOs}\`${result.runnerName ? ` / \`${result.runnerName}\`` : ""}`,
    `- cache mode: \`${result.cacheMode || "unknown"}\` (rust-cache hit: \`${result.rustCacheHit}\`)`,
    `- total measured command time: **${(result.totalDurationMs / 1000).toFixed(1)}s**`,
    "",
    "| phase | seconds | exit |",
    "| --- | ---: | ---: |",
    ...result.records.map((record) => `| ${record.label} | ${(record.durationMs / 1000).toFixed(1)} | ${record.exitCode ?? "error"} |`),
    "",
    "The benchmark keeps the production CI workflow unchanged. Compare runs with the same runner, cache mode, and Rust inputs.",
  ];
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${lines.join("\n")}\n`);
  }
  console.log(lines.join("\n"));
}

main().catch((error) => {
  const failure = {
    schemaVersion: 1,
    scenario,
    package: packageName || null,
    runnerOs: process.env.RUNNER_OS || process.platform,
    cacheMode: process.env.CI_SPEED_CACHE_MODE || null,
    rustCacheHit: process.env.CI_SPEED_RUST_CACHE_HIT || "not-run",
    failed: true,
    error: error.message,
    records: records.map(({ output, ...record }) => record),
  };
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, `${JSON.stringify(failure, null, 2)}\n`);
  console.error(error.stack || error);
  process.exitCode = 1;
});
