const assert = require("node:assert/strict");
const { test } = require("node:test");

const { isReleasePullRequest, runGuidance } = require("./pr-guidance.cjs");

function makePr({ base = "dev", head = "feature", baseRepoId = 7, headRepoId = 7 } = {}) {
  return {
    number: 123,
    base: { ref: base, repo: { id: baseRepoId } },
    head: { ref: head, repo: { id: headRepoId } },
    additions: 10,
    deletions: 2,
    changed_files: 1,
  };
}

function mockApi({ comments = [], commits = [{ commit: { message: "perf: optimize reports" } }] } = {}) {
  const calls = { create: [], update: [] };
  const github = {
    rest: {
      pulls: { listCommits: Symbol("list commits") },
      issues: {
        listComments: Symbol("list comments"),
        createComment: async (args) => calls.create.push(args),
        updateComment: async (args) => calls.update.push(args),
      },
    },
    paginate: async (method) => {
      if (method === github.rest.pulls.listCommits) return commits;
      if (method === github.rest.issues.listComments) return comments;
      throw new Error("Unexpected paginated endpoint");
    },
  };
  const context = {
    repo: { owner: "octo", repo: "ghusage" },
    payload: { pull_request: makePr() },
  };
  return { calls, github, context };
}

test("posts the guidance comment when none exists", async () => {
  const { calls, github, context } = mockApi();
  const body = await runGuidance({ github, context });

  assert.equal(calls.create.length, 1);
  assert.equal(calls.update.length, 0);
  assert.equal(calls.create[0].issue_number, 123);
  assert.equal(calls.create[0].body, body);
  assert.match(body, /Squash and merge is the default/);
});

test("updates the existing bot guidance comment", async () => {
  const { calls, github, context } = mockApi({
    comments: [
      {
        id: 456,
        user: { login: "github-actions[bot]" },
        body: "<!-- ghusage-pr-guidance -->\nold guidance",
      },
    ],
  });
  const body = await runGuidance({ github, context });

  assert.equal(calls.create.length, 0);
  assert.equal(calls.update.length, 1);
  assert.equal(calls.update[0].comment_id, 456);
  assert.equal(calls.update[0].body, body);
});

test("does not classify a fork's dev branch as the release pull request", () => {
  const pr = makePr({ base: "release", head: "dev", baseRepoId: 7, headRepoId: 8 });

  assert.equal(isReleasePullRequest(pr), false);
});

test("classifies the same-repository dev-to-release pull request", () => {
  const pr = makePr({ base: "release", head: "dev" });

  assert.equal(isReleasePullRequest(pr), true);
});
