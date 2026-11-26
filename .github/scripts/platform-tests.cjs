// Selects the tests that must run on every OS, from
// .github/platform-independent-tests.txt.
//
//   node .github/scripts/platform-tests.cjs run    run the platform-dependent tests
//   node .github/scripts/platform-tests.cjs check  fail on stale or unknown entries

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const MANIFEST = path.join(__dirname, "..", "platform-independent-tests.txt");

function parseManifest(text) {
  const targets = new Map();
  const errors = [];
  text.split(/\r?\n/).forEach((raw, index) => {
    const line = raw.replace(/#.*/, "").trim();
    if (!line) return;
    const where = `line ${index + 1}`;
    const fields = line.split(/\s+/);
    if (fields.length < 2 || fields.length > 3) {
      errors.push(`${where}: expected "<package> <test target> [test name]"`);
      return;
    }
    const [pkg, target, name] = fields;
    const key = `${pkg} ${target}`;
    if (name === undefined) {
      if (targets.has(key)) errors.push(`${where}: ${key} is listed twice`);
      else targets.set(key, { pkg, target, everywhere: [] });
      return;
    }
    const entry = targets.get(key);
    if (!entry) {
      errors.push(`${where}: list the target "${key}" before its test ${name}`);
    } else if (entry.everywhere.includes(name)) {
      errors.push(`${where}: ${name} is listed twice`);
    } else {
      entry.everywhere.push(name);
    }
  });
  return { targets: [...targets.values()], errors };
}

// `testTargets` holds every `{ pkg, target }` of kind `test` in the workspace.
function plan(manifest, testTargets) {
  const known = new Set(testTargets.map((t) => `${t.pkg} ${t.target}`));
  const errors = manifest.targets
    .filter((t) => !known.has(`${t.pkg} ${t.target}`))
    .map((t) => `${t.pkg} ${t.target} is not a test target of the workspace`);
  const listed = new Set(manifest.targets.map((t) => `${t.pkg} ${t.target}`));
  const unique = (names) => [...new Set(names)].sort();
  // Targets are selected by name across the workspace, so a target that
  // shares its name with an unlisted one also runs everywhere: the safe side.
  const everywhere = unique(
    testTargets.filter((t) => !listed.has(`${t.pkg} ${t.target}`)).map((t) => t.target),
  );
  const mixed = manifest.targets.filter((t) => t.everywhere.length > 0);
  return {
    errors,
    everywhere,
    mixedTargets: unique(mixed.map((t) => t.target)),
    mixedTests: unique(mixed.flatMap((t) => t.everywhere)),
  };
}

function testFlags(targets) {
  return targets.flatMap((t) => ["--test", t]);
}

// The cargo invocations that run the platform-dependent tests: one build of
// every target needed, the unit tests and unlisted targets in full, and the
// named tests of listed targets.
function commands(p) {
  const all = ["--lib", "--bins", ...testFlags(p.everywhere), ...testFlags(p.mixedTargets)];
  const cmds = [["test", "--workspace", ...all, "--no-run"]];
  cmds.push(["test", "--workspace", "--lib", "--bins", ...testFlags(p.everywhere), "--no-fail-fast"]);
  if (p.mixedTests.length > 0) {
    cmds.push([
      "test",
      "--workspace",
      ...testFlags(p.mixedTargets),
      "--no-fail-fast",
      "--",
      "--exact",
      ...p.mixedTests,
    ]);
  }
  return cmds;
}

// Names printed by `cargo test -- --list` (`<name>: test`).
function listedTests(output) {
  return output
    .split(/\r?\n/)
    .map((line) => /^(\S+): test$/.exec(line))
    .filter(Boolean)
    .map((m) => m[1]);
}

function cargo(args, options = {}) {
  console.log(`$ cargo ${args.join(" ")}`);
  return spawnSync("cargo", args, { stdio: "inherit", ...options });
}

function workspaceTestTargets() {
  const r = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (r.status !== 0) throw new Error(`cargo metadata failed: ${r.stderr}`);
  return JSON.parse(r.stdout).packages.flatMap((p) =>
    p.targets.filter((t) => t.kind.includes("test")).map((t) => ({ pkg: p.name, target: t.name })),
  );
}

function load() {
  const manifest = parseManifest(fs.readFileSync(MANIFEST, "utf8"));
  const p = plan(manifest, workspaceTestTargets());
  const errors = [...manifest.errors, ...p.errors];
  return { manifest, p, errors };
}

function fail(errors) {
  for (const e of errors) console.error(`platform-independent-tests.txt: ${e}`);
  process.exit(1);
}

function main(mode) {
  const { manifest, p, errors } = load();
  if (errors.length > 0) fail(errors);
  if (mode === "run") {
    console.log(`Test targets run on every OS: ${p.everywhere.join(", ")}`);
    console.log(`Tests of platform-independent targets run on every OS: ${p.mixedTests.join(", ")}`);
    let failed = false;
    for (const args of commands(p)) {
      const r = cargo(args);
      if (r.status !== 0) {
        failed = true;
        // Nothing can run without a build.
        if (args.includes("--no-run")) break;
      }
    }
    process.exit(failed ? 1 : 0);
  }
  if (mode === "check") {
    // Every named test must exist, or it would silently stop running.
    const missing = [];
    for (const t of manifest.targets.filter((x) => x.everywhere.length > 0)) {
      const args = ["test", "--workspace", "--test", t.target, "--", "--list"];
      console.log(`$ cargo ${args.join(" ")}`);
      const r = spawnSync("cargo", args, {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "inherit"],
        maxBuffer: 64 * 1024 * 1024,
      });
      if (r.status !== 0) fail([`cargo ${args.join(" ")} failed`]);
      const names = new Set(listedTests(r.stdout));
      for (const name of t.everywhere) {
        if (!names.has(name)) missing.push(`${t.pkg} ${t.target} has no test named ${name}`);
      }
    }
    if (missing.length > 0) fail(missing);
    console.log("Every entry names an existing test target or test.");
    return;
  }
  console.error("usage: platform-tests.cjs run|check");
  process.exit(2);
}

if (require.main === module) main(process.argv[2]);

module.exports = { parseManifest, plan, commands, listedTests };
