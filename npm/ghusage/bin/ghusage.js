#!/usr/bin/env node
// Launches the native ghusage binary installed for this platform.
"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");

const PLATFORMS = {
  "darwin-arm64": "ghusage-darwin-arm64",
  "darwin-x64": "ghusage-darwin-x64",
  "linux-x64": "ghusage-linux-x64",
  "linux-arm64": "ghusage-linux-arm64",
  "win32-x64": "ghusage-win32-x64",
};

function binaryPath() {
  if (process.env.GHUSAGE_BINARY) {
    return process.env.GHUSAGE_BINARY;
  }
  const key = `${process.platform}-${process.arch}`;
  const pkg = PLATFORMS[key];
  if (!pkg) {
    fail(
      `ghusage does not provide a prebuilt binary for ${key}.\n` +
        "Supported: " + Object.keys(PLATFORMS).join(", ") + ".\n" +
        "You can build from source: cargo install --git https://github.com/tanahiro2010/ghusage"
    );
  }
  const exe = process.platform === "win32" ? "ghusage.exe" : "ghusage";
  try {
    return require.resolve(`${pkg}/bin/${exe}`);
  } catch {
    fail(
      `The ghusage binary package "${pkg}" is not installed.\n` +
        "It is an optional dependency; reinstall without --no-optional / --omit=optional,\n" +
        "and make sure your package manager installs optional dependencies for this platform."
    );
  }
}

function fail(message) {
  process.stderr.write(`ghusage: ${message}\n`);
  process.exit(1);
}

const bin = binaryPath();
if (!fs.existsSync(bin)) {
  fail(`binary not found at ${path.normalize(bin)}`);
}
const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  fail(`failed to start ${bin}: ${result.error.message}`);
}
if (result.signal) {
  // Re-raise the signal so the parent sees the same termination.
  process.kill(process.pid, result.signal);
} else {
  process.exit(result.status ?? 1);
}
