// Benchmarks on request for performance pull requests.
//
// A pull request whose title starts with `perf` gets a comment with a
// checkbox. When the repository owner checks it, the benchmarks of the pull
// request are compared with its base; anyone else who checks it has the
// check removed at once. This script runs from the trusted default branch and
// never executes pull request content.

const MARKER = "<!-- ghusage-perf-bench -->";
const REPORT_MARKER = "<!-- ghusage-perf-bench-report -->";
const BOT = "github-actions[bot]";
const CHECKBOX = "Run the benchmarks of this pull request against its base";

function isPerfTitle(title) {
  return /^perf(\([^)]*\))?!?: /.test(title ?? "");
}

function isChecked(body) {
  return new RegExp(`^- \\[[xX]\\] ${CHECKBOX}`, "m").test(body ?? "");
}

function renderOffer(owner, status) {
  return [
    MARKER,
    "### Benchmarks",
    "",
    `- [ ] ${CHECKBOX}`,
    "",
    `Only @${owner} can start them: checking the box runs \`cargo bench\` (aggregation and latency) for the head of this pull request and its base on the same runner, and posts the comparison below.`,
    ...(status ? ["", status] : []),
  ].join("\n");
}

async function findComment(github, owner, repo, number, marker) {
  const comments = await github.paginate(github.rest.issues.listComments, {
    owner,
    repo,
    issue_number: number,
    per_page: 100,
  });
  return comments.find((c) => c.user?.login === BOT && c.body?.startsWith(marker));
}

// pull_request_target: offer the checkbox on performance pull requests.
async function offer({ github, context }) {
  const pr = context.payload.pull_request;
  if (!isPerfTitle(pr.title)) return "not a performance pull request";
  const { owner, repo } = context.repo;
  const existing = await findComment(github, owner, repo, pr.number, MARKER);
  if (existing) return "offered already";
  await github.rest.issues.createComment({
    owner,
    repo,
    issue_number: pr.number,
    body: renderOffer(owner),
  });
  return "offered";
}

// issue_comment (edited): decide whether a check starts a run. Returns the
// run's inputs, or null. A check by anyone but the owner is undone.
async function gate({ github, context }) {
  const { comment, issue, sender } = context.payload;
  const { owner, repo } = context.repo;
  if (!issue?.pull_request || comment?.user?.login !== BOT) return null;
  if (!comment.body?.startsWith(MARKER) || !isChecked(comment.body)) return null;
  const update = (body) =>
    github.rest.issues.updateComment({ owner, repo, comment_id: comment.id, body });

  if (sender?.login !== owner) {
    await update(
      renderOffer(owner, `_@${sender?.login} checked the box; only @${owner} can start the benchmarks, so it was unchecked._`),
    );
    return null;
  }
  const { data: pr } = await github.rest.pulls.get({ owner, repo, pull_number: issue.number });
  if (pr.state !== "open") {
    await update(renderOffer(owner, "_The pull request is not open; nothing was run._"));
    return null;
  }
  const runUrl = `${context.serverUrl}/${owner}/${repo}/actions/runs/${context.runId}`;
  const what = `\`${pr.head.sha.slice(0, 7)}\` against \`${pr.base.ref}\` \`${pr.base.sha.slice(0, 7)}\``;
  // Acknowledge at once: a reaction on the checkbox comment and a comment
  // saying the run started, before the benchmarks take their minutes.
  await github.rest.reactions.createForIssueComment({
    owner,
    repo,
    comment_id: comment.id,
    content: "eyes",
  });
  await github.rest.issues.createComment({
    owner,
    repo,
    issue_number: pr.number,
    body: `Starting the benchmarks for ${what}, as requested by @${owner}. The comparison is posted here when they finish (usually 10–20 minutes): [workflow run](${runUrl})`,
  });
  await update(renderOffer(owner, `Running for ${what}: [workflow run](${runUrl})`));
  return {
    number: pr.number,
    head_sha: pr.head.sha,
    base_sha: pr.base.sha,
    base_ref: pr.base.ref,
  };
}

// After the benchmark job: post the report and say how the run ended.
async function finish({ github, context, number, headSha, report, succeeded }) {
  const { owner, repo } = context.repo;
  const runUrl = `${context.serverUrl}/${owner}/${repo}/actions/runs/${context.runId}`;
  const short = headSha.slice(0, 7);
  if (report) {
    const body = `${REPORT_MARKER}\n${report}`;
    const existing = await findComment(github, owner, repo, number, REPORT_MARKER);
    if (existing) {
      await github.rest.issues.updateComment({ owner, repo, comment_id: existing.id, body });
    } else {
      await github.rest.issues.createComment({ owner, repo, issue_number: number, body });
    }
  }
  const offerComment = await findComment(github, owner, repo, number, MARKER);
  if (offerComment) {
    const status = succeeded
      ? `Finished for \`${short}\`; the comparison is in the comment below. Check the box again to rerun. ([workflow run](${runUrl}))`
      : `The run for \`${short}\` failed: [workflow run](${runUrl}). Check the box again to retry.`;
    await github.rest.issues.updateComment({
      owner,
      repo,
      comment_id: offerComment.id,
      body: renderOffer(owner, status),
    });
  }
}

module.exports = { CHECKBOX, MARKER, REPORT_MARKER, finish, gate, isChecked, isPerfTitle, offer, renderOffer };
