#!/usr/bin/env node
// Runs the native ghusage binary from the platform package that npm
// installed as an optional dependency.
import { spawn } from "node:child_process";
import { chmodSync, realpathSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import process from "node:process";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { version } = require("../package.json");

const PACKAGES = {
  "darwin-arm64": "@ghusage/ghusage-darwin-arm64",
  "darwin-x64": "@ghusage/ghusage-darwin-x64",
  "linux-arm64": "@ghusage/ghusage-linux-arm64",
  "linux-x64": "@ghusage/ghusage-linux-x64",
  "win32-arm64": "@ghusage/ghusage-win32-arm64",
  "win32-x64": "@ghusage/ghusage-win32-x64",
};

/** Name of the platform package for this platform, if one exists. */
export function nativePackageName(platform = process.platform, arch = process.arch) {
  return PACKAGES[`${platform}-${arch}`];
}

/** Path of the binary inside a platform package. */
export function nativeBinarySubpath(platform = process.platform) {
  return platform === "win32" ? "bin/ghusage.exe" : "bin/ghusage";
}

/**
 * Decides what to run: the native binary, or an error message.
 * `GHUSAGE_BINARY` overrides the binary (e.g. one built from source).
 */
export function resolveRuntime({
  platform = process.platform,
  arch = process.arch,
  env = process.env,
  resolvePath = (id) => require.resolve(id),
} = {}) {
  if (env.GHUSAGE_BINARY) {
    return { command: env.GHUSAGE_BINARY };
  }
  const target = `${platform}-${arch}`;
  const pkg = nativePackageName(platform, arch);
  if (pkg == null) {
    return {
      error:
        `ghusage does not provide a native binary for ${target}.\n` +
        "Build it from source: cargo install --git https://github.com/tanahiro2010/ghusage\n",
    };
  }
  try {
    return { command: resolvePath(`${pkg}/${nativeBinarySubpath(platform)}`) };
  } catch {
    return {
      error:
        `Missing optional native package "${pkg}@${version}" for ${target}. ` +
        "It may be absent because of --omit=optional, --no-optional, or a minimum release age " +
        "setting (npm --before / --min-release-age, mise minimum_release_age). " +
        "Reinstall ghusage with optional dependencies enabled.\n",
    };
  }
}

/** Restores the executable bit if a package manager dropped it. */
export function ensureExecutable(path, platform = process.platform) {
  if (platform === "win32") return undefined;
  try {
    if ((statSync(path).mode & 0o111) === 0) chmodSync(path, 0o755);
    return undefined;
  } catch (error) {
    if (error && error.code === "ENOENT") {
      return `ghusage native binary not found: ${path}\n`;
    }
    return `ghusage native binary is not executable: ${error instanceof Error ? error.message : error}\n`;
  }
}

export function forwardedSignals(platform = process.platform) {
  return platform === "win32" ? ["SIGINT", "SIGBREAK", "SIGHUP"] : ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"];
}

/** Runs the binary, forwarding signals, and resolves with how it ended. */
export function runNative(command, args) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { stdio: "inherit" });
    const handlers = forwardedSignals().map((signal) => {
      const handler = () => child.kill(signal);
      try {
        process.on(signal, handler);
      } catch {
        // Some signals cannot be listened to on some platforms.
      }
      return [signal, handler];
    });
    const cleanup = () => handlers.forEach(([s, h]) => process.off(s, h));
    child.on("error", (error) => {
      cleanup();
      resolve({ error, status: null, signal: null });
    });
    child.on("exit", (status, signal) => {
      cleanup();
      resolve({ status, signal });
    });
  });
}

export async function main(argv) {
  const runtime = resolveRuntime();
  if ("error" in runtime) {
    process.stderr.write(runtime.error);
    return 1;
  }
  const notExecutable = ensureExecutable(runtime.command);
  if (notExecutable) {
    process.stderr.write(notExecutable);
    return 1;
  }
  const result = await runNative(runtime.command, argv);
  if (result.error) {
    process.stderr.write(`ghusage: failed to start ${runtime.command}: ${result.error.message}\n`);
    return 1;
  }
  if (result.signal) {
    // End the same way the binary did.
    process.kill(process.pid, result.signal);
    return 1;
  }
  return result.status ?? 1;
}

function isMainModule() {
  const entry = process.argv[1];
  if (!entry) return false;
  const self = fileURLToPath(import.meta.url);
  try {
    return realpathSync(self) === realpathSync(entry);
  } catch {
    return self === entry;
  }
}

if (isMainModule()) {
  process.exitCode = await main(process.argv.slice(2));
}
