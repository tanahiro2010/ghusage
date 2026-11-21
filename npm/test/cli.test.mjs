// Tests for the npm launcher. Run with: node --test npm/test
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  ensureExecutable,
  nativeBinarySubpath,
  nativePackageName,
  resolveRuntime,
} from "../ghusage/src/cli.js";

const cli = fileURLToPath(new URL("../ghusage/src/cli.js", import.meta.url));

test("maps every supported platform to its package", () => {
  assert.equal(nativePackageName("darwin", "arm64"), "@ghusage/ghusage-darwin-arm64");
  assert.equal(nativePackageName("darwin", "x64"), "@ghusage/ghusage-darwin-x64");
  assert.equal(nativePackageName("linux", "arm64"), "@ghusage/ghusage-linux-arm64");
  assert.equal(nativePackageName("linux", "x64"), "@ghusage/ghusage-linux-x64");
  assert.equal(nativePackageName("win32", "arm64"), "@ghusage/ghusage-win32-arm64");
  assert.equal(nativePackageName("win32", "x64"), "@ghusage/ghusage-win32-x64");
  assert.equal(nativePackageName("freebsd", "x64"), undefined);
  assert.equal(nativeBinarySubpath("win32"), "bin/ghusage.exe");
  assert.equal(nativeBinarySubpath("linux"), "bin/ghusage");
});

test("platform list matches the packages that are built", () => {
  const platforms = JSON.parse(
    fs.readFileSync(new URL("../platforms.json", import.meta.url), "utf8"),
  );
  for (const p of platforms) {
    assert.equal(nativePackageName(p.os, p.cpu), p.package);
    assert.equal(`bin/${p.binary}`, nativeBinarySubpath(p.os));
  }
});

test("unsupported platforms get an explanation", () => {
  const r = resolveRuntime({ platform: "aix", arch: "ppc64", env: {} });
  assert.match(r.error, /does not provide a native binary for aix-ppc64/);
});

test("a missing optional package explains why and how to fix it", () => {
  const r = resolveRuntime({
    platform: "linux",
    arch: "x64",
    env: {},
    resolvePath: () => {
      throw new Error("not found");
    },
  });
  assert.match(r.error, /Missing optional native package "@ghusage\/ghusage-linux-x64@/);
  assert.match(r.error, /--omit=optional/);
});

test("the installed platform package is resolved", () => {
  const r = resolveRuntime({
    platform: "darwin",
    arch: "arm64",
    env: {},
    resolvePath: (id) => `/modules/${id}`,
  });
  assert.equal(r.command, "/modules/@ghusage/ghusage-darwin-arm64/bin/ghusage");
});

test("GHUSAGE_BINARY overrides the binary", () => {
  const r = resolveRuntime({ platform: "aix", arch: "ppc64", env: { GHUSAGE_BINARY: "/x/ghusage" } });
  assert.equal(r.command, "/x/ghusage");
});

test("the executable bit is restored", { skip: process.platform === "win32" }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ghusage-npm-"));
  const f = path.join(dir, "ghusage");
  fs.writeFileSync(f, "#!/bin/sh\n");
  fs.chmodSync(f, 0o644);
  assert.equal(ensureExecutable(f), undefined);
  assert.notEqual(fs.statSync(f).mode & 0o111, 0);
});

test("arguments, output and exit code pass through", { skip: process.platform === "win32" }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ghusage-npm-"));
  const fake = path.join(dir, "fake");
  fs.writeFileSync(fake, '#!/bin/sh\necho "args:$*"\nexit 7\n', { mode: 0o755 });
  const r = spawnSync(process.execPath, [cli, "daily", "--format", "json"], {
    env: { ...process.env, GHUSAGE_BINARY: fake },
    encoding: "utf8",
  });
  assert.equal(r.stdout.trim(), "args:daily --format json");
  assert.equal(r.status, 7);
});

test("termination by a signal is reproduced", { skip: process.platform === "win32" }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ghusage-npm-"));
  const fake = path.join(dir, "fake");
  fs.writeFileSync(fake, "#!/bin/sh\nkill -TERM $$\n", { mode: 0o755 });
  const r = spawnSync(process.execPath, [cli], {
    env: { ...process.env, GHUSAGE_BINARY: fake },
  });
  assert.equal(r.signal, "SIGTERM");
});

test("a missing binary is reported", () => {
  const r = spawnSync(process.execPath, [cli, "--version"], {
    env: { ...process.env, GHUSAGE_BINARY: path.join(os.tmpdir(), "does-not-exist-ghusage") },
    encoding: "utf8",
  });
  assert.equal(r.status, 1);
  assert.match(r.stderr, process.platform === "win32" ? /failed to start/ : /native binary not found/);
});
