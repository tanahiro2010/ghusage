#!/usr/bin/env node
// Assembles the npm packages for a release.
//
// Usage: node npm/scripts/build-packages.mjs --version 0.1.0 \
//          --binaries <dir> --out <dir> [--only <package>...]
//
// <dir> contains one directory per Rust target with the built binary:
//   <binaries>/<target>/ghusage(.exe)
// Packages whose binary is missing are skipped unless they are listed in
// --only, in which case a missing binary is an error.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const npmDir = path.resolve(here, "..");
const repoRoot = path.resolve(npmDir, "..");

function args() {
  const a = process.argv.slice(2);
  const out = { only: [] };
  for (let i = 0; i < a.length; i++) {
    const k = a[i];
    const v = a[i + 1];
    if (k === "--version") (out.version = v), i++;
    else if (k === "--binaries") (out.binaries = v), i++;
    else if (k === "--out") (out.out = v), i++;
    else if (k === "--only") (out.only.push(v), i++);
    else throw new Error(`unknown argument ${k}`);
  }
  if (!out.version || !out.binaries || !out.out) {
    throw new Error("--version, --binaries and --out are required");
  }
  return out;
}

function cargoVersion() {
  const toml = fs.readFileSync(path.join(repoRoot, "Cargo.toml"), "utf8");
  const m = toml.match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/ms);
  if (!m) throw new Error("cannot read workspace version from Cargo.toml");
  return m[1];
}

const opts = args();
const version = opts.version.replace(/^v/, "");
if (version !== cargoVersion()) {
  throw new Error(`version ${version} does not match Cargo.toml version ${cargoVersion()}`);
}
const platforms = JSON.parse(fs.readFileSync(path.join(npmDir, "platforms.json"), "utf8"));
const main = JSON.parse(fs.readFileSync(path.join(npmDir, "ghusage", "package.json"), "utf8"));
const license = main.license;

fs.rmSync(opts.out, { recursive: true, force: true });
fs.mkdirSync(opts.out, { recursive: true });

const built = [];
for (const p of platforms) {
  const src = path.join(opts.binaries, p.target, p.binary);
  if (!fs.existsSync(src)) {
    if (opts.only.includes(p.package)) throw new Error(`missing binary ${src}`);
    console.error(`skip ${p.package}: no binary at ${src}`);
    continue;
  }
  if (opts.only.length && !opts.only.includes(p.package)) continue;
  const dir = path.join(opts.out, p.package);
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  const dest = path.join(dir, "bin", p.binary);
  fs.copyFileSync(src, dest);
  fs.chmodSync(dest, 0o755);
  const pkg = {
    name: p.package,
    version,
    description: `The ${p.os} ${p.cpu} binary for ghusage.`,
    repository: main.repository,
    homepage: main.homepage,
    license,
    os: [p.os],
    cpu: [p.cpu],
    files: [`bin/${p.binary}`],
    preferUnplugged: true,
  };
  fs.writeFileSync(path.join(dir, "package.json"), JSON.stringify(pkg, null, 2) + "\n");
  fs.writeFileSync(
    path.join(dir, "README.md"),
    `# ${p.package}\n\nThe ${p.os}-${p.cpu} binary for [ghusage](https://www.npmjs.com/package/ghusage). Install \`ghusage\` instead of this package.\n`
  );
  built.push(p.package);
}

// The main package lists every platform package as an optional dependency.
const mainDir = path.join(opts.out, "ghusage");
fs.cpSync(path.join(npmDir, "ghusage"), mainDir, { recursive: true });
const mainPkg = { ...main, version };
mainPkg.optionalDependencies = Object.fromEntries(platforms.map((p) => [p.package, version]));
fs.writeFileSync(path.join(mainDir, "package.json"), JSON.stringify(mainPkg, null, 2) + "\n");
fs.copyFileSync(path.join(repoRoot, "README.md"), path.join(mainDir, "README.md"));
for (const f of ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"]) {
  const src = path.join(repoRoot, f);
  if (fs.existsSync(src)) {
    fs.copyFileSync(src, path.join(mainDir, f));
    for (const name of built) fs.copyFileSync(src, path.join(opts.out, name, f));
  }
}
console.log(JSON.stringify({ version, packages: ["ghusage", ...built] }));
