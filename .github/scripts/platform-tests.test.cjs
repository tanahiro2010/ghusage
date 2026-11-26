const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");

const { parseManifest, plan, commands, listedTests } = require("./platform-tests.cjs");

const targets = [
  { pkg: "core", target: "scenarios" },
  { pkg: "core", target: "properties" },
  { pkg: "storage", target: "properties" },
  { pkg: "storage", target: "store" },
  { pkg: "app", target: "reports" },
];

test("parses targets, named tests, comments and CRLF line endings", () => {
  const m = parseManifest(
    "# comment\r\ncore scenarios\r\ncore scenarios dst_day # why\r\n\r\napp reports\r\n",
  );
  assert.deepEqual(m.errors, []);
  assert.deepEqual(m.targets, [
    { pkg: "core", target: "scenarios", everywhere: ["dst_day"] },
    { pkg: "app", target: "reports", everywhere: [] },
  ]);
});

test("rejects malformed, duplicate and orphaned entries", () => {
  const m = parseManifest("core\ncore a\ncore a\ncore b x\ncore a y\ncore a y\ncore a y z w\n");
  assert.deepEqual(m.errors, [
    'line 1: expected "<package> <test target> [test name]"',
    "line 3: core a is listed twice",
    'line 4: list the target "core b" before its test x',
    "line 6: y is listed twice",
    'line 7: expected "<package> <test target> [test name]"',
  ]);
});

test("unlisted targets run everywhere and named tests of listed ones too", () => {
  const p = plan(parseManifest("core scenarios\ncore scenarios dst_day\napp reports\n"), targets);
  assert.deepEqual(p.errors, []);
  assert.deepEqual(p.everywhere, ["properties", "store"]);
  assert.deepEqual(p.mixedTargets, ["scenarios"]);
  assert.deepEqual(p.mixedTests, ["dst_day"]);
});

test("a new test target runs everywhere until it is listed", () => {
  const p = plan(parseManifest("app reports\n"), [...targets, { pkg: "app", target: "new_one" }]);
  assert.ok(p.everywhere.includes("new_one"));
});

test("a listed target that shares its name with an unlisted one still runs everywhere", () => {
  const p = plan(parseManifest("storage properties\n"), targets);
  assert.ok(p.everywhere.includes("properties"));
});

test("entries for targets that do not exist are errors", () => {
  const p = plan(parseManifest("core gone\n"), targets);
  assert.deepEqual(p.errors, ["core gone is not a test target of the workspace"]);
});

test("builds once, then runs unit tests, unlisted targets and named tests", () => {
  const p = plan(parseManifest("core scenarios\ncore scenarios dst_day\napp reports\n"), targets);
  assert.deepEqual(commands(p), [
    ["test", "--workspace", "--lib", "--bins", "--test", "properties", "--test", "store",
      "--test", "scenarios", "--no-run"],
    ["test", "--workspace", "--lib", "--bins", "--test", "properties", "--test", "store",
      "--no-fail-fast"],
    ["test", "--workspace", "--test", "scenarios", "--no-fail-fast", "--", "--exact", "dst_day"],
  ]);
});

test("without named tests no filtered run is added", () => {
  const p = plan(parseManifest("app reports\n"), targets);
  assert.equal(commands(p).length, 2);
});

test("reads test names from cargo test --list output", () => {
  assert.deepEqual(
    listedTests("a_test: test\r\nmod::b: test\nc: bench\n\n2 tests, 0 benchmarks\n"),
    ["a_test", "mod::b"],
  );
});

test("the repository manifest parses", () => {
  const text = fs.readFileSync(path.join(__dirname, "..", "platform-independent-tests.txt"), "utf8");
  assert.deepEqual(parseManifest(text).errors, []);
});
