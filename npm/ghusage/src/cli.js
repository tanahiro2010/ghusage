#!/usr/bin/env node
// Runs the native ghusage binary from the platform package that npm
// installed as an optional dependency. When that package is missing, the
// launcher fetches it once with `npm pack` (so the user's npm registry,
// proxy, authentication and integrity checks apply) and keeps the binary in
// a per-user cache.
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

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

const PUBLIC_REGISTRY = "https://registry.npmjs.org";
// Upper bounds for a fetched package; the real ones are a few MB.
const MAX_UNPACKED_BYTES = 512 * 1024 * 1024;
const PACK_TIMEOUT_MS = 5 * 60 * 1000;

/** Name of the platform package for this platform, if one exists. */
export function nativePackageName(platform = process.platform, arch = process.arch) {
  return PACKAGES[`${platform}-${arch}`];
}

/** Path of the binary inside a platform package. */
export function nativeBinarySubpath(platform = process.platform) {
  return platform === "win32" ? "bin/ghusage.exe" : "bin/ghusage";
}

/** Explains a missing platform package and how to get it back. */
export function missingPackageMessage({
  pkg,
  target,
  launcherVersion = version,
  fetchError,
  autoInstallDisabled = false,
}) {
  const spec = `${pkg}@${launcherVersion}`;
  const lines = [
    `ghusage: the native binary for ${target} is missing.`,
    "",
    `ghusage@${launcherVersion} runs its binary from the optional package "${spec}",`,
    "but npm did not install it.",
  ];
  if (autoInstallDisabled) {
    lines.push("Fetching it automatically is turned off (GHUSAGE_NO_AUTO_INSTALL).");
  } else if (fetchError) {
    lines.push("Fetching it automatically with `npm pack` failed:");
    for (const l of String(fetchError).trim().split(/\r?\n/).slice(0, 8)) lines.push(`  ${l}`);
  }
  lines.push(
    "",
    "Possible causes:",
    "  - The release was published moments ago and your registry does not serve the",
    "    platform package yet.",
    "  - A registry proxy, mirror or security tool configured as your npm registry",
    "    holds new versions back.",
    "  - Optional dependencies were skipped (--omit=optional, --no-optional).",
    "  - A minimum release age setting (npm --before / --min-release-age,",
    "    mise minimum_release_age) held the platform package back.",
    "",
    "How to fix:",
    "  1. Reinstall the same version so npm installs the platform package:",
    `       npm uninstall -g ghusage && npm install -g ghusage@${launcherVersion}`,
    "  2. Check which registry npm uses:",
    "       npm config get registry",
    `     If it is a proxy or mirror rather than ${PUBLIC_REGISTRY}/, wait until it`,
    `     serves ${spec}, or install the previous version of ghusage meanwhile`,
    "     (list versions with `npm view ghusage versions`):",
    "       npm install -g ghusage@<previous version>",
    "  3. Or install from the public npm registry:",
    `       npm install -g ghusage@${launcherVersion} --registry ${PUBLIC_REGISTRY}`,
    "     If your organization requires a registry proxy, follow that policy instead.",
    "",
    "To run a binary you built yourself, set GHUSAGE_BINARY to its path.",
    "",
  );
  return lines.join("\n");
}

/**
 * Decides what to run from what is installed: the `GHUSAGE_BINARY`
 * override, the installed platform package, or an error. A missing
 * platform package also reports `missing` so the caller can fetch it.
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
      missing: { pkg, target },
      error: missingPackageMessage({ pkg, target }),
    };
  }
}

/** Per-user cache directory of the launcher. `GHUSAGE_CACHE_DIR` overrides it. */
export function cacheRoot({ platform = process.platform, env = process.env, homedir = os.homedir() } = {}) {
  if (env.GHUSAGE_CACHE_DIR) return path.resolve(env.GHUSAGE_CACHE_DIR);
  if (platform === "darwin") return path.join(homedir, "Library", "Caches", "ghusage");
  if (platform === "win32") {
    const local = env.LOCALAPPDATA || path.join(homedir, "AppData", "Local");
    return path.win32.join(local, "ghusage", "cache");
  }
  const xdg = env.XDG_CACHE_HOME;
  // The XDG spec says relative paths are invalid and must be ignored.
  const base = xdg && path.isAbsolute(xdg) ? xdg : path.join(homedir, ".cache");
  return path.join(base, "ghusage");
}

/** Directory that holds the fetched binary for one version and platform. */
export function cachedPackageDir({ root, launcherVersion = version, target }) {
  return path.join(root, "native", launcherVersion, target);
}

function isTruthyEnv(value) {
  return value != null && value !== "" && !/^(0|false|no|off)$/i.test(value);
}

/** Rejects archive paths that are absolute or climb out of the archive. */
function checkArchivePath(name) {
  const p = name.replace(/\\/g, "/");
  if (p.startsWith("/") || /^[A-Za-z]:/.test(p) || p.split("/").includes("..")) {
    throw new Error(`refusing unsafe path in package archive: ${JSON.stringify(name)}`);
  }
  return p.replace(/^\.\//, "");
}

function tarString(buf, start, length) {
  const end = buf.indexOf(0, start);
  return buf.toString("utf8", start, end === -1 || end > start + length ? start + length : end);
}

function tarNumber(buf, start, length) {
  if (buf[start] & 0x80) {
    // Base-256 encoding for large values.
    let n = 0;
    for (let i = start + 1; i < start + length; i++) n = n * 256 + buf[i];
    return n;
  }
  const s = tarString(buf, start, length).trim();
  return s === "" ? 0 : parseInt(s, 8);
}

function parsePax(data) {
  const out = {};
  let i = 0;
  while (i < data.length) {
    const space = data.indexOf(0x20, i);
    if (space === -1) break;
    const len = parseInt(data.toString("utf8", i, space), 10);
    if (!(len > 0)) break;
    const record = data.toString("utf8", space + 1, i + len - 1);
    const eq = record.indexOf("=");
    if (eq !== -1) out[record.slice(0, eq)] = record.slice(eq + 1);
    i += len;
  }
  return out;
}

/**
 * Reads the regular files of a gzip-compressed tar archive (ustar, with
 * pax and GNU long names). Returns a Map from path to contents. Throws on
 * a corrupt archive or an absolute or `..` path.
 */
export function readTarGz(gz) {
  const tar = gunzipSync(gz, { maxOutputLength: MAX_UNPACKED_BYTES });
  const files = new Map();
  let offset = 0;
  let nextName;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    let sum = 0;
    for (let i = 0; i < 512; i++) sum += i >= 148 && i < 156 ? 0x20 : header[i];
    if (sum !== tarNumber(header, 148, 8)) throw new Error("corrupt package archive (bad header checksum)");
    const size = tarNumber(header, 124, 12);
    const type = String.fromCharCode(header[156] || 0x30);
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > tar.length) throw new Error("corrupt package archive (truncated)");
    const data = tar.subarray(dataStart, dataEnd);
    offset = dataStart + Math.ceil(size / 512) * 512;

    if (type === "x") {
      const pax = parsePax(data);
      if (pax.path != null) nextName = pax.path;
      continue;
    }
    if (type === "L") {
      nextName = tarString(data, 0, data.length);
      continue;
    }
    if (type === "g") continue;

    let name = tarString(header, 0, 100);
    const prefix = header.toString("latin1", 257, 262) === "ustar" ? tarString(header, 345, 155) : "";
    if (prefix) name = `${prefix}/${name}`;
    if (nextName != null) name = nextName;
    nextName = undefined;
    const safe = checkArchivePath(name);
    if (type === "0" || type === "7") files.set(safe, Buffer.from(data));
  }
  return files;
}

/**
 * The first lines of npm's error output (code and reason) and its log path,
 * without control characters (the text may quote a registry response).
 */
export function summarizeNpmError(stderr) {
  const lines = String(stderr ?? "")
    .split(/\r?\n/)
    .map((l) => l.replace(/[\u0000-\u001f\u007f-\u009f]/g, "").trimEnd())
    .filter((l) => l.trim() !== "" && !/^npm (error|ERR!)\s*$/.test(l));
  const head = lines.slice(0, 4);
  const log = lines.slice(4).find((l) => /complete log/i.test(l));
  return (log ? [...head, log] : head).join("\n");
}

function npmInvocation(env, platform) {
  const execpath = env.npm_execpath;
  if (execpath && /^npm-cli\.c?js$/.test(path.basename(execpath))) {
    return { command: process.execPath, prefix: [execpath], shell: false };
  }
  if (platform === "win32") return { command: "npm", prefix: [], shell: true };
  return { command: "npm", prefix: [], shell: false };
}

/**
 * Downloads `<pkg>@<version>` into `dest` with `npm pack`, which honours the
 * user's npm configuration, and returns the path of the tarball.
 */
export function npmPack({ pkg, version: v, dest, env = process.env, platform = process.platform }) {
  const npm = npmInvocation(env, platform);
  const args = [...npm.prefix, "pack", `${pkg}@${v}`, "--pack-destination", dest, "--ignore-scripts"];
  // Windows runs npm.cmd through the shell: pass one command line, quoting
  // each argument ourselves (paths may contain spaces).
  const [command, commandArgs] = npm.shell
    ? [[npm.command, ...args.map((a) => `"${a}"`)].join(" "), []]
    : [npm.command, args];
  const r = spawnSync(command, commandArgs, {
    env,
    shell: npm.shell,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout: PACK_TIMEOUT_MS,
    windowsHide: true,
  });
  if (r.error) {
    if (r.error.code === "ENOENT") throw new Error("npm was not found on PATH");
    throw new Error(`could not run npm: ${r.error.message}`);
  }
  if (r.status !== 0) {
    throw new Error(summarizeNpmError(r.stderr) || `npm pack exited with status ${r.status ?? r.signal}`);
  }
  const tgz = readdirSync(dest).filter((f) => f.endsWith(".tgz"));
  if (tgz.length !== 1) throw new Error("npm pack did not produce a package tarball");
  return path.join(dest, tgz[0]);
}

/**
 * Fetches the platform package and places its binary in the cache.
 * Concurrent launchers are safe: each extracts into its own temporary
 * directory and renames it into place; whoever loses the race uses the
 * winner's copy. Returns the path of the binary.
 */
export function installFromRegistry({
  pkg,
  target,
  platform = process.platform,
  launcherVersion = version,
  root,
  env = process.env,
  pack = npmPack,
}) {
  const subpath = nativeBinarySubpath(platform);
  const finalDir = cachedPackageDir({ root, launcherVersion, target });
  const versionDir = path.dirname(finalDir);
  mkdirSync(versionDir, { recursive: true, mode: 0o700 });
  const work = mkdtempSync(path.join(versionDir, `.fetch-${process.pid}-`));
  try {
    const downloads = path.join(work, "download");
    mkdirSync(downloads);
    const tgzPath = pack({ pkg, version: launcherVersion, dest: downloads, env, platform });
    const files = readTarGz(readFileSync(tgzPath));

    const manifest = files.get("package/package.json");
    if (manifest == null) throw new Error("the package has no package.json");
    const meta = JSON.parse(manifest.toString("utf8"));
    if (meta.name !== pkg || meta.version !== launcherVersion) {
      throw new Error(`expected ${pkg}@${launcherVersion} but got ${meta.name}@${meta.version}`);
    }
    const binary = files.get(`package/${subpath}`);
    if (binary == null) throw new Error(`the package has no ${subpath}`);

    const staged = path.join(work, "pkg");
    mkdirSync(path.join(staged, path.dirname(subpath)), { recursive: true, mode: 0o700 });
    const stagedBinary = path.join(staged, subpath);
    writeFileSync(stagedBinary, binary, { mode: 0o700 });
    if (platform !== "win32") chmodSync(stagedBinary, 0o755);
    writeFileSync(path.join(staged, "package.json"), manifest);

    try {
      renameSync(staged, finalDir);
    } catch (error) {
      // Another launcher finished first; use its copy if it is complete.
      if (!existsSync(path.join(finalDir, subpath))) throw error;
    }
    return path.join(finalDir, subpath);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

/**
 * Decides what to run, fetching the platform package into the cache when
 * npm did not install it. Resolves with `{ command }` or `{ error }`.
 */
export async function prepareRuntime({
  platform = process.platform,
  arch = process.arch,
  env = process.env,
  resolvePath,
  homedir = os.homedir(),
  launcherVersion = version,
  pack = npmPack,
  stderr = process.stderr,
} = {}) {
  const runtime = resolveRuntime({ platform, arch, env, resolvePath });
  if (!runtime.missing) return runtime;
  const { pkg, target } = runtime.missing;
  const root = cacheRoot({ platform, env, homedir });
  const cached = path.join(cachedPackageDir({ root, launcherVersion, target }), nativeBinarySubpath(platform));
  if (existsSync(cached)) return { command: cached };
  if (isTruthyEnv(env.GHUSAGE_NO_AUTO_INSTALL)) {
    return { error: missingPackageMessage({ pkg, target, launcherVersion, autoInstallDisabled: true }) };
  }
  stderr.write(`ghusage: fetching ${pkg}@${launcherVersion} ...\n`);
  try {
    return { command: installFromRegistry({ pkg, target, platform, launcherVersion, root, env, pack }) };
  } catch (error) {
    const fetchError = error instanceof Error ? error.message : String(error);
    return { error: missingPackageMessage({ pkg, target, launcherVersion, fetchError }) };
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

/** Runs ghusage. `options` are passed to `prepareRuntime` (for tests). */
export async function main(argv, options = {}) {
  const runtime = await prepareRuntime(options);
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
