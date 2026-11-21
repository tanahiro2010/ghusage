// Tests for fetching a missing platform package. Nothing here touches the
// network: `npm pack` is replaced by a function or a fake npm-cli.js that
// hands back a tarball built in the test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import {
  cacheRoot,
  cachedPackageDir,
  main,
  nativeBinarySubpath,
  nativePackageName,
  prepareRuntime,
  readTarGz,
  summarizeNpmError,
} from "../ghusage/src/cli.js";

const cli = fileURLToPath(new URL("../ghusage/src/cli.js", import.meta.url));
const launcherVersion = JSON.parse(
  fs.readFileSync(new URL("../ghusage/package.json", import.meta.url), "utf8"),
).version;

function tmpdir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "ghusage-npm-auto-"));
}

/** One ustar header block. */
function tarHeader(name, size, type = "0", mode = 0o644) {
  const h = Buffer.alloc(512);
  h.write(name, 0, 100, "utf8");
  h.write(mode.toString(8).padStart(7, "0") + "\0", 100, "latin1");
  h.write("0000000\0", 108, "latin1");
  h.write("0000000\0", 116, "latin1");
  h.write(size.toString(8).padStart(11, "0") + "\0", 124, "latin1");
  h.write("00000000000\0", 136, "latin1");
  h.write(type, 156, "latin1");
  h.write("ustar\0", 257, "latin1");
  h.write("00", 263, "latin1");
  h.fill(0x20, 148, 156);
  let sum = 0;
  for (const b of h) sum += b;
  h.write(sum.toString(8).padStart(6, "0") + "\0 ", 148, "latin1");
  return h;
}

/** A gzip-compressed tar archive of `[name, contents, mode?]` entries. */
function tgz(entries) {
  const parts = [];
  for (const [name, contents, mode] of entries) {
    const data = Buffer.from(contents);
    parts.push(tarHeader(name, data.length, "0", mode), data);
    const pad = (512 - (data.length % 512)) % 512;
    parts.push(Buffer.alloc(pad));
  }
  parts.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(parts));
}

const SCRIPT = '#!/bin/sh\necho "native:$*"\nexit 3\n';

function platformPackage(pkg, v, platform, extra = []) {
  return tgz([
    ["package/package.json", JSON.stringify({ name: pkg, version: v })],
    [`package/${nativeBinarySubpath(platform)}`, SCRIPT, 0o755],
    ["package/README.md", "readme"],
    ...extra,
  ]);
}

function missing() {
  throw new Error("not installed");
}

function collect() {
  const lines = [];
  return { lines, write: (s) => lines.push(s) };
}

const PKG = "@ghusage/ghusage-linux-x64";
const V = "9.9.9";

test("reads files from a ustar archive", () => {
  const files = readTarGz(tgz([["package/a.txt", "hello"], ["package/bin/b", "x".repeat(700)]]));
  assert.equal(files.get("package/a.txt").toString(), "hello");
  assert.equal(files.get("package/bin/b").length, 700);
});

test("archives with .. or absolute paths are refused", () => {
  assert.throws(() => readTarGz(tgz([["package/../../evil", "x"]])), /unsafe path/);
  assert.throws(() => readTarGz(tgz([["/etc/evil", "x"]])), /unsafe path/);
  assert.throws(() => readTarGz(tgz([["C:\\evil", "x"]])), /unsafe path/);
});

test("a corrupt archive is refused", () => {
  const good = tarHeader("package/a", 0);
  good[0] ^= 1; // the checksum no longer matches
  assert.throws(() => readTarGz(gzipSync(Buffer.concat([good, Buffer.alloc(1024)]))), /checksum/);
});

// Expected paths use path.join so they also hold on a Windows host.
test("npm errors are shortened to the code, the reason and the log path", () => {
  const stderr = [
    "npm error code ETARGET",
    "npm error notarget No matching version found for @ghusage/ghusage-linux-x64@9.9.9.",
    "npm error notarget In most cases you or one of your dependencies are requesting",
    "npm error notarget a package version that doesn't exist.",
    "npm error notarget \u001b[31mextra\u001b[0m",
    "npm error",
    "npm error A complete log of this run can be found in: /tmp/x.log",
  ].join("\n");
  assert.equal(
    summarizeNpmError(stderr),
    [
      "npm error code ETARGET",
      "npm error notarget No matching version found for @ghusage/ghusage-linux-x64@9.9.9.",
      "npm error notarget In most cases you or one of your dependencies are requesting",
      "npm error notarget a package version that doesn't exist.",
      "npm error A complete log of this run can be found in: /tmp/x.log",
    ].join("\n"),
  );
  assert.equal(summarizeNpmError("npm error \u001b]0;title\u0007boom"), "npm error ]0;titleboom");
});

test("the cache lives in the per-user cache directory of each OS", () => {
  const home = "/home/u";
  assert.equal(cacheRoot({ platform: "darwin", env: {}, homedir: "/Users/u" }), path.join("/Users/u", "Library", "Caches", "ghusage"));
  assert.equal(cacheRoot({ platform: "linux", env: {}, homedir: home }), path.join(home, ".cache", "ghusage"));
  assert.equal(cacheRoot({ platform: "linux", env: { XDG_CACHE_HOME: "/xdg" }, homedir: home }), path.join("/xdg", "ghusage"));
  assert.equal(
    cacheRoot({ platform: "linux", env: { XDG_CACHE_HOME: "relative" }, homedir: home }),
    path.join(home, ".cache", "ghusage"),
  );
  assert.equal(
    cacheRoot({ platform: "win32", env: { LOCALAPPDATA: "C:\\Users\\u\\AppData\\Local" }, homedir: home }),
    "C:\\Users\\u\\AppData\\Local\\ghusage\\cache",
  );
  const custom = path.resolve("custom-cache");
  assert.equal(cacheRoot({ platform: "linux", env: { GHUSAGE_CACHE_DIR: custom }, homedir: home }), custom);
  assert.equal(
    cachedPackageDir({ root: "/c", launcherVersion: "1.2.3", target: "linux-x64" }),
    path.join("/c", "native", "1.2.3", "linux-x64"),
  );
});

test("a missing platform package is fetched into the cache and reused", async () => {
  const root = tmpdir();
  const env = { GHUSAGE_CACHE_DIR: root };
  const calls = [];
  const pack = ({ pkg, version, dest }) => {
    calls.push(`${pkg}@${version}`);
    const f = path.join(dest, "ghusage-ghusage-linux-x64-9.9.9.tgz");
    fs.writeFileSync(f, platformPackage(pkg, version, "linux"));
    return f;
  };
  const stderr = collect();
  const opts = { platform: "linux", arch: "x64", env, resolvePath: missing, launcherVersion: V, pack, stderr };

  const first = await prepareRuntime(opts);
  const expected = path.join(root, "native", V, "linux-x64", "bin", "ghusage");
  assert.equal(first.command, expected);
  assert.equal(fs.readFileSync(expected, "utf8"), SCRIPT);
  if (process.platform !== "win32") assert.notEqual(fs.statSync(expected).mode & 0o111, 0);
  assert.deepEqual(calls, [`${PKG}@${V}`]);
  assert.deepEqual(stderr.lines, [`ghusage: fetching ${PKG}@${V} ...\n`]);
  // Only the binary and the manifest are kept; no temporary directories remain.
  assert.deepEqual(fs.readdirSync(path.join(root, "native", V)), ["linux-x64"]);
  assert.deepEqual(fs.readdirSync(path.join(root, "native", V, "linux-x64")).sort(), ["bin", "package.json"]);

  const second = await prepareRuntime({ ...opts, pack: () => assert.fail("must use the cache") });
  assert.equal(second.command, expected);
  assert.equal(stderr.lines.length, 1);
});

test("a launcher that loses a concurrent fetch uses the winner's copy", async () => {
  const root = tmpdir();
  const finalDir = path.join(root, "native", V, "linux-x64");
  const r = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: root },
    resolvePath: missing,
    launcherVersion: V,
    pack: ({ pkg, version, dest }) => {
      // Another launcher places its copy while this one is downloading.
      fs.mkdirSync(path.join(finalDir, "bin"), { recursive: true });
      fs.writeFileSync(path.join(finalDir, "bin", "ghusage"), "winner", { mode: 0o755 });
      const f = path.join(dest, "pkg.tgz");
      fs.writeFileSync(f, platformPackage(pkg, version, "linux"));
      return f;
    },
    stderr: collect(),
  });
  assert.equal(r.command, path.join(finalDir, "bin", "ghusage"));
  assert.equal(fs.readFileSync(r.command, "utf8"), "winner");
  assert.deepEqual(fs.readdirSync(path.join(root, "native", V)), ["linux-x64"]);
});

test("an installed platform package and GHUSAGE_BINARY win over fetching", async () => {
  const pack = () => assert.fail("must not fetch");
  const root = tmpdir();
  const installed = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: root },
    resolvePath: (id) => `/modules/${id}`,
    pack,
  });
  assert.equal(installed.command, `/modules/${PKG}/bin/ghusage`);
  const override = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: root, GHUSAGE_BINARY: "/x/ghusage" },
    resolvePath: missing,
    pack,
  });
  assert.equal(override.command, "/x/ghusage");
});

test("GHUSAGE_NO_AUTO_INSTALL turns fetching off", async () => {
  const stderr = collect();
  const r = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: tmpdir(), GHUSAGE_NO_AUTO_INSTALL: "1" },
    resolvePath: missing,
    launcherVersion: V,
    pack: () => assert.fail("must not fetch"),
    stderr,
  });
  assert.match(r.error, /turned off \(GHUSAGE_NO_AUTO_INSTALL\)/);
  assert.match(r.error, /npm install -g ghusage@9\.9\.9/);
  assert.deepEqual(stderr.lines, []);
});

test("a package archive with .. is refused and nothing is cached", async () => {
  const root = tmpdir();
  const r = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: root },
    resolvePath: missing,
    launcherVersion: V,
    pack: ({ pkg, version, dest }) => {
      const f = path.join(dest, "evil.tgz");
      fs.writeFileSync(f, platformPackage(pkg, version, "linux", [["package/../../../evil", "x"]]));
      return f;
    },
    stderr: collect(),
  });
  assert.match(r.error, /unsafe path in package archive/);
  assert.deepEqual(fs.readdirSync(path.join(root, "native", V)), []);
});

test("a package with the wrong name or version is refused", async () => {
  const r = await prepareRuntime({
    platform: "linux",
    arch: "x64",
    env: { GHUSAGE_CACHE_DIR: tmpdir() },
    resolvePath: missing,
    launcherVersion: V,
    pack: ({ dest }) => {
      const f = path.join(dest, "other.tgz");
      fs.writeFileSync(f, platformPackage(PKG, "0.0.1", "linux"));
      return f;
    },
    stderr: collect(),
  });
  assert.match(r.error, /expected @ghusage\/ghusage-linux-x64@9\.9\.9 but got @ghusage\/ghusage-linux-x64@0\.0\.1/);
});

test("a failed fetch explains the cause and exits with 1", async () => {
  const written = [];
  const original = process.stderr.write;
  process.stderr.write = (s) => written.push(String(s));
  let code;
  try {
    code = await main(["--version"], {
      platform: "linux",
      arch: "x64",
      env: { GHUSAGE_CACHE_DIR: tmpdir() },
      resolvePath: missing,
      launcherVersion: V,
      pack: () => {
        throw new Error("npm error code ETARGET\nnpm error notarget No matching version found");
      },
      stderr: { write: (s) => written.push(s) },
    });
  } finally {
    process.stderr.write = original;
  }
  assert.equal(code, 1);
  const out = written.join("");
  assert.match(out, /^ghusage: fetching @ghusage\/ghusage-linux-x64@9\.9\.9 \.\.\.\n/);
  assert.match(out, /Fetching it automatically with `npm pack` failed:\n {2}npm error code ETARGET/);
  assert.match(out, /How to fix:/);
  assert.match(out, /npm config get registry/);
});

// End to end through the real launcher and the real `npm pack` call, with
// npm replaced by a script that copies a prepared tarball.
function fakeNpm(dir, { tarball, fail }) {
  const script = path.join(dir, "npm-cli.js");
  fs.writeFileSync(
    script,
    `const fs = require("fs"), path = require("path");
fs.appendFileSync(${JSON.stringify(path.join(dir, "calls.log"))}, process.argv.slice(2).join(" ") + "\\n");
${fail ? 'process.stderr.write("npm error code E404\\\\n"); process.exit(1);' : ""}
const a = process.argv.slice(2);
const dest = a[a.indexOf("--pack-destination") + 1];
fs.copyFileSync(${JSON.stringify(tarball ?? "")}, path.join(dest, "pkg.tgz"));
console.log("pkg.tgz");
`,
  );
  return script;
}

const hostPkg = nativePackageName();
const hostTarget = `${process.platform}-${process.arch}`;

test(
  "the launcher fetches, caches and runs the binary without writing to stdout",
  { skip: process.platform === "win32" || hostPkg == null },
  () => {
    const dir = tmpdir();
    const tarball = path.join(dir, "host.tgz");
    fs.writeFileSync(tarball, platformPackage(hostPkg, launcherVersion, process.platform));
    const npm = fakeNpm(dir, { tarball });
    const cache = path.join(dir, "cache dir");
    const env = { ...process.env, GHUSAGE_CACHE_DIR: cache, npm_execpath: npm };
    delete env.GHUSAGE_BINARY;
    delete env.GHUSAGE_NO_AUTO_INSTALL;

    const first = spawnSync(process.execPath, [cli, "daily", "--json"], { env, encoding: "utf8" });
    assert.equal(first.stdout, "native:daily --json\n");
    assert.equal(first.stderr, `ghusage: fetching ${hostPkg}@${launcherVersion} ...\n`);
    assert.equal(first.status, 3);
    const calls = fs.readFileSync(path.join(dir, "calls.log"), "utf8").trim().split("\n");
    assert.equal(calls.length, 1);
    assert.match(calls[0], new RegExp(`^pack ${hostPkg}@${launcherVersion} --pack-destination `));
    const bin = path.join(cache, "native", launcherVersion, hostTarget, "bin", "ghusage");
    assert.notEqual(fs.statSync(bin).mode & 0o111, 0);

    const second = spawnSync(process.execPath, [cli, "x"], { env, encoding: "utf8" });
    assert.equal(second.stdout, "native:x\n");
    assert.equal(second.stderr, "");
    assert.equal(fs.readFileSync(path.join(dir, "calls.log"), "utf8").trim().split("\n").length, 1);
  },
);

test("the launcher exits with 1 when npm cannot fetch the package", { skip: hostPkg == null }, () => {
  const dir = tmpdir();
  const npm = fakeNpm(dir, { fail: true });
  const env = { ...process.env, GHUSAGE_CACHE_DIR: path.join(dir, "cache"), npm_execpath: npm };
  delete env.GHUSAGE_BINARY;
  delete env.GHUSAGE_NO_AUTO_INSTALL;
  const r = spawnSync(process.execPath, [cli, "--version"], { env, encoding: "utf8" });
  assert.equal(r.status, 1);
  assert.equal(r.stdout, "");
  assert.match(r.stderr, /ghusage: fetching /);
  assert.match(r.stderr, /npm error code E404/);
  assert.match(r.stderr, /How to fix:/);
});

test("the launcher does not fetch when GHUSAGE_NO_AUTO_INSTALL is set", { skip: hostPkg == null }, () => {
  const dir = tmpdir();
  const npm = fakeNpm(dir, { fail: true });
  const env = {
    ...process.env,
    GHUSAGE_CACHE_DIR: path.join(dir, "cache"),
    GHUSAGE_NO_AUTO_INSTALL: "1",
    npm_execpath: npm,
  };
  delete env.GHUSAGE_BINARY;
  const r = spawnSync(process.execPath, [cli, "--version"], { env, encoding: "utf8" });
  assert.equal(r.status, 1);
  assert.doesNotMatch(r.stderr, /fetching/);
  assert.match(r.stderr, /GHUSAGE_NO_AUTO_INSTALL/);
  assert.equal(fs.existsSync(path.join(dir, "calls.log")), false);
});
