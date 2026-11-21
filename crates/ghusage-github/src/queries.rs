//! GraphQL documents. Each document contains exactly the fragments it uses.

/// Page size of pull request commits, reviews, conversation comments and
/// timeline items.
pub(crate) const PAGE_SIZE: u32 = 100;
/// Page size of review threads.
pub(crate) const THREAD_PAGE_SIZE: u32 = 50;
/// Page size of comments inside a review thread.
pub(crate) const THREAD_COMMENT_PAGE_SIZE: u32 = 50;

const ACTOR: &str = "fragment ActorFields on Actor { __typename login ... on Node { id } }";

const REPOSITORY: &str = "fragment RepositoryFields on Repository { id name url visibility isFork viewerPermission owner { __typename login ... on Node { id } } }";

const PR_COMMIT: &str = "fragment PrCommitFields on PullRequestCommit { url commit { oid committedDate authoredDate url parents { totalCount } author { user { id login } } committer { user { id login } } } }";

const REVIEW_COMMENT: &str = "fragment ReviewCommentFields on PullRequestReviewComment { id state createdAt lastEditedAt url author { ...ActorFields } replyTo { id } pullRequestReview { id } }";

/// Comments listed under a review are a secondary source of inline comments
/// (the thread comments are authoritative and fully paginated); they are
/// deduplicated by node id.
const REVIEW: &str = "fragment ReviewFields on PullRequestReview { id state body submittedAt url author { ...ActorFields } comments(first: 20) { nodes { ...ReviewCommentFields } } }";

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

pub(crate) fn pull_request() -> String {
    doc(
        format!(
            "query PullRequestDetail($owner: String!, $name: String!, $number: Int!) {{
  repository(owner: $owner, name: $name) {{
    ...RepositoryFields
    pullRequest(number: $number) {{
      id number title url state isDraft createdAt updatedAt closedAt mergedAt headRefOid
      author {{ ...ActorFields }}
      mergedBy {{ ...ActorFields }}
      headCommit: commits(last: 1) {{ nodes {{ commit {{ oid statusCheckRollup {{ state }} }} }} }}
      commits(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...PrCommitFields }} }}
      reviews(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...ReviewFields }} }}
      reviewThreads(first: {THREAD_PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...ReviewThreadFields }} }}
      comments(first: {PAGE_SIZE}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...IssueCommentFields }} }}
      timelineItems(first: {PAGE_SIZE}, itemTypes: {PR_TIMELINE_TYPES}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...PrTimelineFields }} }}
    }}
  }}
}}"
        ),
        &[
            ACTOR,
            REPOSITORY,
            PR_COMMIT,
            REVIEW_COMMENT,
            REVIEW,
            THREAD,
            ISSUE_COMMENT,
            TIMELINE,
        ],
    )
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
    pub fn field(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::Reviews => "reviews",
            Self::Threads => "reviewThreads",
            Self::Comments => "comments",
            Self::Timeline => "timelineItems",
        }
    }

    pub fn page_query(self) -> String {
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
                vec![ACTOR, REVIEW_COMMENT, REVIEW],
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
        "query RepositoryDetail($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) { ...RepositoryFields }
}"
        .to_string(),
        &[REPOSITORY],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes every document to `$GHUSAGE_DUMP_QUERIES` so they can be
    /// validated against the live schema by hand.
    #[test]
    #[ignore]
    fn dump_queries() {
        let Ok(dir) = std::env::var("GHUSAGE_DUMP_QUERIES") else {
            return;
        };
        let mut all = vec![
            ("pull_request", pull_request()),
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
        let mut docs = vec![
            pull_request(),
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
        for d in docs {
            for def in d.split("fragment ").skip(1) {
                let name = def.split_whitespace().next().unwrap();
                assert!(
                    d.contains(&format!("...{name}")),
                    "unused fragment {name} in {d}"
                );
            }
        }
    }
}
