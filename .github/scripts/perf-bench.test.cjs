const assert = require("node:assert/strict");
const { test } = require("node:test");

const {
  MARKER,
  REPORT_MARKER,
  finish,
  gate,
  isChecked,
  isPerfTitle,
  offer,
  renderOffer,
} = require("./perf-bench.cjs");

const OWNER = "octo";

function mockApi({ comments = [], pr = {} } = {}) {
  const calls = { create: [], update: [], get: [], react: [] };
  const github = {
    rest: {
      pulls: {
        get: async (args) => {
          calls.get.push(args);
          return {
            data: {
              number: 7,
              state: "open",
              head: { sha: "a".repeat(40) },
              base: { sha: "b".repeat(40), ref: "dev" },
              ...pr,
            },
          };
        },
      },
      reactions: {
        createForIssueComment: async (args) => calls.react.push(args),
      },
      issues: {
        listComments: Symbol("list comments"),
        createComment: async (args) => calls.create.push(args),
        updateComment: async (args) => calls.update.push(args),
      },
    },
    paginate: async (method) => {
      if (method === github.rest.issues.listComments) return comments;
      throw new Error("Unexpected paginated endpoint");
    },
  };
  return { calls, github };
}

function context(payload) {
  return { repo: { owner: OWNER, repo: "ghusage" }, serverUrl: "https://github.com", runId: 99, payload };
}

function checkedComment(id = 1) {
  return {
    id,
    user: { login: "github-actions[bot]" },
    body: renderOffer(OWNER).replace("- [ ] ", "- [x] "),
  };
}

test("only perf titles count", () => {
  assert.ok(isPerfTitle("perf: faster sync"));
  assert.ok(isPerfTitle("perf(github): fetch in batches"));
  assert.ok(isPerfTitle("perf!: drop a slow path"));
  assert.ok(!isPerfTitle("fix(perf): typo"));
  assert.ok(!isPerfTitle("performance: something"));
  assert.ok(!isPerfTitle("feat: perf mode"));
  assert.ok(!isPerfTitle(undefined));
});

test("the offer starts unchecked", () => {
  const body = renderOffer(OWNER);
  assert.ok(body.startsWith(MARKER));
  assert.ok(!isChecked(body));
  assert.ok(isChecked(body.replace("- [ ] ", "- [x] ")));
  assert.ok(isChecked(body.replace("- [ ] ", "- [X] ")));
});

test("a perf pull request gets one offer", async () => {
  const { calls, github } = mockApi();
  const payload = { pull_request: { number: 7, title: "perf: faster" } };
  assert.equal(await offer({ github, context: context(payload) }), "offered");
  assert.equal(calls.create.length, 1);
  assert.ok(!isChecked(calls.create[0].body));

  const again = mockApi({ comments: [{ id: 1, user: { login: "github-actions[bot]" }, body: renderOffer(OWNER) }] });
  assert.equal(await offer({ github: again.github, context: context(payload) }), "offered already");
  assert.equal(again.calls.create.length, 0);
});

test("other pull requests get no offer", async () => {
  const { calls, github } = mockApi();
  const payload = { pull_request: { number: 7, title: "feat: thing" } };
  await offer({ github, context: context(payload) });
  assert.equal(calls.create.length, 0);
});

test("the owner's check starts a run and clears the box", async () => {
  const { calls, github } = mockApi();
  const run = await gate({
    github,
    context: context({ comment: checkedComment(), issue: { number: 7, pull_request: {} }, sender: { login: OWNER } }),
  });
  assert.deepEqual(run, { number: 7, head_sha: "a".repeat(40), base_sha: "b".repeat(40), base_ref: "dev" });
  assert.equal(calls.update.length, 1);
  assert.ok(!isChecked(calls.update[0].body));
  assert.match(calls.update[0].body, /Running for `aaaaaaa`/);
  // Acknowledged at once with a reaction and a comment.
  assert.deepEqual(calls.react, [{ owner: OWNER, repo: "ghusage", comment_id: 1, content: "eyes" }]);
  assert.equal(calls.create.length, 1);
  assert.equal(calls.create[0].issue_number, 7);
  assert.match(calls.create[0].body, /^Starting the benchmarks for `aaaaaaa` against `dev` `bbbbbbb`, as requested by @octo/);
  assert.match(calls.create[0].body, /actions\/runs\/99/);
});

test("anyone else's check is undone and starts nothing", async () => {
  const { calls, github } = mockApi();
  const run = await gate({
    github,
    context: context({ comment: checkedComment(), issue: { number: 7, pull_request: {} }, sender: { login: "mallory" } }),
  });
  assert.equal(run, null);
  assert.equal(calls.update.length, 1);
  assert.ok(!isChecked(calls.update[0].body));
  assert.match(calls.update[0].body, /@mallory checked the box/);
  assert.equal(calls.get.length, 0);
  assert.equal(calls.react.length, 0);
  assert.equal(calls.create.length, 0);
});

test("unchecking, other comments and issues start nothing", async () => {
  const cases = [
    { comment: { id: 1, user: { login: "github-actions[bot]" }, body: renderOffer(OWNER) }, issue: { number: 7, pull_request: {} } },
    { comment: { id: 1, user: { login: OWNER }, body: renderOffer(OWNER).replace("- [ ] ", "- [x] ") }, issue: { number: 7, pull_request: {} } },
    { comment: checkedComment(), issue: { number: 7 } },
    { comment: { id: 1, user: { login: "github-actions[bot]" }, body: "- [x] something else" }, issue: { number: 7, pull_request: {} } },
  ];
  for (const payload of cases) {
    const { calls, github } = mockApi();
    assert.equal(await gate({ github, context: context({ ...payload, sender: { login: OWNER } }) }), null);
    assert.equal(calls.update.length, 0);
  }
});

test("a closed pull request is not benchmarked", async () => {
  const { calls, github } = mockApi({ pr: { state: "closed" } });
  const run = await gate({
    github,
    context: context({ comment: checkedComment(), issue: { number: 7, pull_request: {} }, sender: { login: OWNER } }),
  });
  assert.equal(run, null);
  assert.ok(!isChecked(calls.update[0].body));
});

test("finish posts the report once and reports the outcome", async () => {
  const offerComment = { id: 1, user: { login: "github-actions[bot]" }, body: renderOffer(OWNER) };
  const { calls, github } = mockApi({ comments: [offerComment] });
  await finish({ github, context: context({}), number: 7, headSha: "c".repeat(40), report: "| table |", succeeded: true });
  assert.equal(calls.create.length, 1);
  assert.ok(calls.create[0].body.startsWith(REPORT_MARKER));
  assert.match(calls.update[0].body, /Finished for `ccccccc`/);
  assert.ok(!isChecked(calls.update[0].body));

  const report = { id: 2, user: { login: "github-actions[bot]" }, body: `${REPORT_MARKER}\nold` };
  const again = mockApi({ comments: [offerComment, report] });
  await finish({ github: again.github, context: context({}), number: 7, headSha: "c".repeat(40), report: "new", succeeded: false });
  assert.equal(again.calls.create.length, 0);
  assert.equal(again.calls.update[0].comment_id, 2);
  assert.match(again.calls.update[1].body, /failed/);
});
