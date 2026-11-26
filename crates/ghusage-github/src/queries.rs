//! GraphQL documents. Each document contains exactly the fragments it uses.

use std::fmt::Write as _;

/// Page size of pull request commits, reviews, conversation comments and
/// timeline items.
pub(crate) const PAGE_SIZE: u32 = 100;
/// Page size of review threads.
pub(crate) const THREAD_PAGE_SIZE: u32 = 50;
/// Page size of comments inside a review thread.
pub(crate) const THREAD_COMMENT_PAGE_SIZE: u32 = 50;
/// Asks GitHub for the rate-limit points the query used; summed in
/// [`crate::ApiStats::graphql_points`]. Reading it costs nothing.
pub(crate) const COST: &str = "rateLimit { cost }";

const ACTOR: &str = "fragment ActorFields on Actor { __typename login ... on Node { id } }";

const REPOSITORY: &str = "fragment RepositoryFields on Repository { id name url visibility isFork viewerPermission owner { __typename login ... on Node { id } } }";

const PR_COMMIT: &str = "fragment PrCommitFields on PullRequestCommit { url commit { oid messageHeadline committedDate authoredDate url additions deletions parents { totalCount } author { user { id login } } committer { user { id login } } } }";

/// The head commit's latest CI rollup, whether it has any commit statuses,
/// and its check runs: up to 40 suites with up to 100 runs each (a head
/// commit with more falls back to the REST checks endpoint). GitHub computes
/// a query's cost from the requested page sizes, and the suites multiply the
/// runs asked for in each, so the suite page is kept small. `ALL` returns
/// every attempt, so a failed run that was re-run is kept alongside the
/// re-run (the default, `LATEST`, keeps only the most recent run of each
/// name); the REST fallback asks for the same with `filter=all`. Commit
/// statuses themselves are not read here: GraphQL only exposes the latest
/// status of each context, while every status posted is an event.
const HEAD_COMMIT: &str = "fragment HeadCommitFields on Commit { oid statusCheckRollup { state } status { state } checkSuites(first: 40) { pageInfo { hasNextPage endCursor } nodes { databaseId app { id slug } checkRuns(first: 100, filterBy: { checkType: ALL }) { pageInfo { hasNextPage endCursor } nodes { id name status conclusion startedAt completedAt permalink } } } } }";

const REVIEW_COMMENT: &str = "fragment ReviewCommentFields on PullRequestReviewComment { id state createdAt lastEditedAt url author { ...ActorFields } replyTo { id } pullRequestReview { id } }";

/// A review's inline comments are not listed here: they are read from the
/// review threads, which are paginated in full and carry each comment's
/// review id. Listing them again would only raise the query's cost.
const REVIEW: &str = "fragment ReviewFields on PullRequestReview { id state body submittedAt url author { ...ActorFields } }";

const THREAD: &str = "fragment ReviewThreadFields on PullRequestReviewThread { id isResolved isOutdated path resolvedBy { __typename login id } comments(first: 50) { totalCount pageInfo { hasNextPage endCursor } nodes { ...ReviewCommentFields } } }";

const ISSUE_COMMENT: &str = "fragment IssueCommentFields on IssueComment { id createdAt lastEditedAt url author { ...ActorFields } }";

const TIMELINE: &str = "fragment PrTimelineFields on PullRequestTimelineItems {
  __typename
  ... on Node { id }
  ... on MergedEvent { createdAt url actor { ...ActorFields } }
  ... on ClosedEvent { createdAt url actor { ...ActorFields } }
  ... on ReopenedEvent { createdAt actor { ...ActorFields } }
  ... on ConvertToDraftEvent { createdAt url actor { ...ActorFields } }
  ... on ReadyForReviewEvent { createdAt url actor { ...ActorFields } }
  ... on HeadRefForcePushedEvent { createdAt actor { ...ActorFields } beforeCommit { oid } afterCommit { oid } }
  ... on ReviewRequestedEvent { createdAt actor { ...ActorFields } requestedReviewer { ... on Actor { login } ... on Team { slug } } }
  ... on ReviewRequestRemovedEvent { createdAt actor { ...ActorFields } requestedReviewer { ... on Actor { login } ... on Team { slug } } }
  ... on AssignedEvent { createdAt actor { ...ActorFields } assignee { ... on Actor { login } } }
  ... on UnassignedEvent { createdAt actor { ...ActorFields } assignee { ... on Actor { login } } }
  ... on LabeledEvent { createdAt actor { ...ActorFields } label { name } }
  ... on UnlabeledEvent { createdAt actor { ...ActorFields } label { name } }
  ... on MilestonedEvent { createdAt actor { ...ActorFields } milestoneTitle }
  ... on DemilestonedEvent { createdAt actor { ...ActorFields } milestoneTitle }
  ... on RenamedTitleEvent { createdAt actor { ...ActorFields } previousTitle currentTitle }
  ... on ReviewDismissedEvent { createdAt url actor { ...ActorFields } review { id } previousReviewState }
}";

const PR_TIMELINE_TYPES: &str = "[MERGED_EVENT, CLOSED_EVENT, REOPENED_EVENT, CONVERT_TO_DRAFT_EVENT, READY_FOR_REVIEW_EVENT, HEAD_REF_FORCE_PUSHED_EVENT, REVIEW_REQUESTED_EVENT, REVIEW_REQUEST_REMOVED_EVENT, ASSIGNED_EVENT, UNASSIGNED_EVENT, LABELED_EVENT, UNLABELED_EVENT, MILESTONED_EVENT, DEMILESTONED_EVENT, RENAMED_TITLE_EVENT, REVIEW_DISMISSED_EVENT]";

fn doc(body: String, fragments: &[&str]) -> String {
    let mut s = body;
    for f in fragments {
        s.push('\n');
        s.push_str(f);
    }
    s
}

/// The fields of a pull request read by the pull request queries.
fn pull_request_fields() -> String {
    format!(
        "id number title url state isDraft createdAt updatedAt closedAt mergedAt headRefOid
      additions deletions changedFiles mergeCommit {{ parents {{ totalCount }} }}
      author {{ ...ActorFields }}
      mergedBy {{ ...ActorFields }}
      headCommit: commits(last: 1) {{ nodes {{ commit {{ ...HeadCommitFields }} }} }}
      commits(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...PrCommitFields }} }}
      reviews(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...ReviewFields }} }}
      reviewThreads(first: {THREAD_PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...ReviewThreadFields }} }}
      comments(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...IssueCommentFields }} }}
      timelineItems(first: {PAGE_SIZE}, itemTypes: {PR_TIMELINE_TYPES}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...PrTimelineFields }} }}"
    )
}

const PULL_REQUEST_FRAGMENTS: [&str; 9] = [
    ACTOR,
    REPOSITORY,
    HEAD_COMMIT,
    PR_COMMIT,
    REVIEW_COMMENT,
    REVIEW,
    THREAD,
    ISSUE_COMMENT,
    TIMELINE,
];

pub(crate) fn pull_request() -> String {
    let fields = pull_request_fields();
    doc(
        format!(
            "query PullRequestDetail($owner: String!, $name: String!, $number: Int!) {{
  {COST}
  repository(owner: $owner, name: $name) {{
    ...RepositoryFields
    pullRequest(number: $number) {{
      {fields}
    }}
  }}
}}"
        ),
        &PULL_REQUEST_FRAGMENTS,
    )
}

/// `count` pull requests in one query, with the same fields as
/// [`pull_request`]: alias `r<i>` is the repository with owner `$o<i>` and
/// name `$n<i>`, and its pull request number `$k<i>`.
pub(crate) fn pull_requests(count: usize) -> String {
    let fields = pull_request_fields();
    let params: Vec<String> = (0..count)
        .map(|i| format!("$o{i}: String!, $n{i}: String!, $k{i}: Int!"))
        .collect();
    let mut body = format!(
        "query PullRequestBatch({}) {{\n  {COST}\n",
        params.join(", ")
    );
    for i in 0..count {
        let _ = writeln!(
            body,
            "  r{i}: repository(owner: $o{i}, name: $n{i}) {{
    ...RepositoryFields
    pullRequest(number: $k{i}) {{
      {fields}
    }}
  }}"
        );
    }
    body.push('}');
    doc(body, &PULL_REQUEST_FRAGMENTS)
}

/// The follow-up page queries of a pull request's connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrConnection {
    Commits,
    Reviews,
    Threads,
    Comments,
    Timeline,
}

impl PrConnection {
    pub(crate) const fn field(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::Reviews => "reviews",
            Self::Threads => "reviewThreads",
            Self::Comments => "comments",
            Self::Timeline => "timelineItems",
        }
    }

    pub(crate) fn page_query(self) -> String {
        let (name, args, nodes, fragments): (&str, String, &str, Vec<&str>) = match self {
            Self::Commits => (
                "PullRequestCommitsPage",
                format!("first: {PAGE_SIZE}, after: $after"),
                "...PrCommitFields",
                vec![PR_COMMIT],
            ),
            Self::Reviews => (
                "PullRequestReviewsPage",
                format!("first: {PAGE_SIZE}, after: $after"),
                "...ReviewFields",
                vec![ACTOR, REVIEW],
            ),
            Self::Threads => (
                "PullRequestReviewThreadsPage",
                format!("first: {THREAD_PAGE_SIZE}, after: $after"),
                "...ReviewThreadFields",
                vec![ACTOR, REVIEW_COMMENT, THREAD],
            ),
            Self::Comments => (
                "PullRequestCommentsPage",
                format!("first: {PAGE_SIZE}, after: $after"),
                "...IssueCommentFields",
                vec![ACTOR, ISSUE_COMMENT],
            ),
            Self::Timeline => (
                "PullRequestTimelinePage",
                format!("first: {PAGE_SIZE}, after: $after, itemTypes: {PR_TIMELINE_TYPES}"),
                "...PrTimelineFields",
                vec![ACTOR, TIMELINE],
            ),
        };
        let field = self.field();
        doc(
            format!(
                "query {name}($id: ID!, $after: String) {{
  {COST}
  node(id: $id) {{
    ... on PullRequest {{
      {field}({args}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ {nodes} }} }}
    }}
  }}
}}"
            ),
            &fragments,
        )
    }
}

pub(crate) fn thread_comments_page() -> String {
    doc(
        format!(
            "query ReviewThreadCommentsPage($id: ID!, $after: String) {{
  {COST}
  node(id: $id) {{
    ... on PullRequestReviewThread {{
      comments(first: {THREAD_COMMENT_PAGE_SIZE}, after: $after) {{ totalCount pageInfo {{ hasNextPage endCursor }} nodes {{ ...ReviewCommentFields }} }}
    }}
  }}
}}"
        ),
        &[ACTOR, REVIEW_COMMENT],
    )
}

const ISSUE_TIMELINE: &str = "fragment IssueTimelineFields on IssueTimelineItems {
  __typename
  ... on Node { id }
  ... on ClosedEvent { createdAt url actor { ...ActorFields } }
  ... on ReopenedEvent { createdAt actor { ...ActorFields } }
}";

pub(crate) fn issue() -> String {
    doc(
        format!(
            "query IssueDetail($owner: String!, $name: String!, $number: Int!) {{
  {COST}
  repository(owner: $owner, name: $name) {{
    ...RepositoryFields
    issueOrPullRequest(number: $number) {{
      __typename
      ... on Issue {{
        id number title url state createdAt closedAt
        author {{ ...ActorFields }}
        timelineItems(first: {PAGE_SIZE}, itemTypes: [CLOSED_EVENT, REOPENED_EVENT]) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...IssueTimelineFields }} }}
      }}
    }}
  }}
}}"
        ),
        &[ACTOR, REPOSITORY, ISSUE_TIMELINE],
    )
}

pub(crate) fn issue_timeline_page() -> String {
    doc(
        format!(
            "query IssueTimelinePage($id: ID!, $after: String) {{
  {COST}
  node(id: $id) {{
    ... on Issue {{
      timelineItems(first: {PAGE_SIZE}, after: $after, itemTypes: [CLOSED_EVENT, REOPENED_EVENT]) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...IssueTimelineFields }} }}
    }}
  }}
}}"
        ),
        &[ACTOR, ISSUE_TIMELINE],
    )
}

pub(crate) fn repository() -> String {
    doc(
        format!(
            "query RepositoryDetail($owner: String!, $name: String!) {{
  {COST}
  repository(owner: $owner, name: $name) {{ ...RepositoryFields }}
}}"
        ),
        &[REPOSITORY],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes every document to `$GHUSAGE_DUMP_QUERIES` so they can be
    /// validated against the live schema by hand.
    #[test]
    #[ignore = "manual helper for dumping query documents against the live schema"]
    fn dump_queries() {
        let Ok(dir) = std::env::var("GHUSAGE_DUMP_QUERIES") else {
            return;
        };
        let mut all = vec![
            ("pull_request", pull_request()),
            ("pull_requests", pull_requests(3)),
            ("thread_comments", thread_comments_page()),
            ("issue", issue()),
            ("issue_timeline", issue_timeline_page()),
            ("repository", repository()),
        ];
        for c in [
            PrConnection::Commits,
            PrConnection::Reviews,
            PrConnection::Threads,
            PrConnection::Comments,
            PrConnection::Timeline,
        ] {
            all.push((c.field(), c.page_query()));
        }
        for (name, q) in all {
            std::fs::write(format!("{dir}/{name}.graphql"), q).unwrap();
        }
    }

    #[test]
    fn documents_only_contain_used_fragments() {
        for d in all_documents() {
            for def in d.split("fragment ").skip(1) {
                let name = def.split_whitespace().next().unwrap();
                assert!(
                    d.contains(&format!("...{name}")),
                    "unused fragment {name} in {d}"
                );
            }
        }
    }

    fn all_documents() -> Vec<String> {
        let mut docs = vec![
            pull_request(),
            pull_requests(1),
            pull_requests(5),
            thread_comments_page(),
            issue(),
            issue_timeline_page(),
            repository(),
        ];
        for c in [
            PrConnection::Commits,
            PrConnection::Reviews,
            PrConnection::Threads,
            PrConnection::Comments,
            PrConnection::Timeline,
        ] {
            docs.push(c.page_query());
        }
        docs
    }

    #[test]
    fn every_document_reports_its_rate_limit_cost() {
        for d in all_documents() {
            let top = d.split("\nfragment ").next().unwrap();
            let first = top.find('{').unwrap();
            assert!(
                top[first..].starts_with("{\n  rateLimit { cost }\n"),
                "no top-level rateLimit {{ cost }} in {d}"
            );
        }
    }

    /// Inline comments come from the review threads, which are paginated in
    /// full; listing them again under each review only adds query cost.
    #[test]
    fn reviews_do_not_list_their_comments() {
        let review = REVIEW;
        assert!(!review.contains("comments"), "{review}");
        let page = PrConnection::Reviews.page_query();
        assert!(!page.contains("ReviewCommentFields"), "{page}");
    }

    /// The check suites of the head commit are asked for 40 at a time, the
    /// size the query cost is computed from; a commit with more falls back
    /// to the REST checks endpoint.
    #[test]
    fn head_commit_check_suites_page_size() {
        assert!(
            HEAD_COMMIT.contains("checkSuites(first: 40)"),
            "{HEAD_COMMIT}"
        );
    }

    #[test]
    fn batched_query_reads_the_same_pull_request_fields() {
        let single = pull_request();
        let batch = pull_requests(2);
        let selection = |q: &str, from: &str| {
            let start = q.find(from).unwrap() + from.len();
            let end = start + q[start..].find("\n    }").unwrap();
            q[start..end].to_string()
        };
        let fields = selection(&single, "pullRequest(number: $number) {");
        assert_eq!(selection(&batch, "pullRequest(number: $k0) {"), fields);
        assert_eq!(selection(&batch, "pullRequest(number: $k1) {"), fields);
        assert!(batch.starts_with(
            "query PullRequestBatch($o0: String!, $n0: String!, $k0: Int!, $o1: String!, $n1: String!, $k1: Int!) {"
        ));
        assert!(batch.contains("r1: repository(owner: $o1, name: $n1) {\n    ...RepositoryFields"));
        assert_eq!(
            single.split("\nfragment ").count(),
            batch.split("\nfragment ").count()
        );
    }
}
