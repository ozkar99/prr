//! GitHub API: REST for PR data, GraphQL for reviews (the only API with pending reviews and threads).
use crate::backend::gh;
use crate::backend::types::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// A REST call through `gh api` (see `gh`).
async fn rest(path: &str) -> Result<Value, String> {
    gh::api(path, None).await
}

async fn graphql(query: &str, variables: Value) -> Result<Value, String> {
    let body = gh::api("graphql", Some(&json!({ "query": query, "variables": variables }))).await?;
    if let Some(errors) = body["errors"].as_array().filter(|e| !e.is_empty()) {
        let msgs: Vec<&str> = errors.iter().filter_map(|e| e["message"].as_str()).collect();
        return Err(msgs.join("; "));
    }
    Ok(body["data"].clone())
}

/// Whether a PR was merged or closed (false when GitHub can't say).
pub async fn pr_is_done(owner: &str, repo: &str, number: u64) -> bool {
    gh::in_repo(owner, repo, rest(&format!("/repos/{owner}/{repo}/pulls/{number}"))).await.map(|p| p["state"] == "closed").unwrap_or(false)
}

/// The signed-in account's login, for the setup check.
pub async fn viewer() -> Result<String, String> {
    Ok(s(&rest("/user").await?["login"]))
}

/// The open PR whose head is `head_owner:branch` in `owner/repo`, or else one
/// that contains commit `sha`: what `prr` with no argument opens.
pub async fn pr_for_branch(owner: &str, repo: &str, head_owner: Option<&str>, branch: Option<&str>, sha: Option<&str>) -> Result<Option<String>, String> {
    if let (Some(head_owner), Some(branch)) = (head_owner, branch) {
        let prs = rest(&format!("/repos/{owner}/{repo}/pulls?state=open&head={head_owner}:{}", urlencode(branch))).await?;
        if let Some(url) = prs.as_array().and_then(|a| a.first()).and_then(|p| p["html_url"].as_str()) {
            return Ok(Some(url.to_string()));
        }
    }
    if let Some(sha) = sha {
        let prs = rest(&format!("/repos/{owner}/{repo}/commits/{sha}/pulls")).await.unwrap_or(Value::Null);
        if let Some(url) = prs.as_array().into_iter().flatten().find(|p| p["state"] == "open").and_then(|p| p["html_url"].as_str()) {
            return Ok(Some(url.to_string()));
        }
    }
    Ok(None)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Line ranges per side from the unified diff's hunk headers (`@@ -a,b +c,d @@`).
fn commentable_ranges(patch: &str) -> CommentableRanges {
    let re = regex::Regex::new(r"(?m)^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    let mut ranges = CommentableRanges { left: vec![], right: vec![] };
    for c in re.captures_iter(patch) {
        let n = |i: usize, default: u64| c.get(i).and_then(|m| m.as_str().parse().ok()).unwrap_or(default);
        let (left, left_len, right, right_len) = (n(1, 0), n(2, 1), n(3, 0), n(4, 1));
        if left_len > 0 {
            ranges.left.push((left, left + left_len - 1));
        }
        if right_len > 0 {
            ranges.right.push((right, right + right_len - 1));
        }
    }
    ranges
}

pub async fn get_pr(owner: &str, repo: &str, number: u64) -> Result<PRDetails, String> {
    gh::in_repo(owner, repo, get_pr_inner(owner, repo, number)).await
}

async fn get_pr_inner(owner: &str, repo: &str, number: u64) -> Result<PRDetails, String> {
    // The PR and its files at once; all pages of files in one gh call
    // (GitHub caps the listing at 3000 files).
    let (pr_path, files_path) = (format!("/repos/{owner}/{repo}/pulls/{number}"), format!("/repos/{owner}/{repo}/pulls/{number}/files?per_page=100"));
    let (pr, items) = tokio::join!(rest(&pr_path), gh::api_all(&files_path));
    let (pr, items) = (pr?, items?);
    let files = items
        .iter()
        .map(|f| PRFile {
            path: s(&f["filename"]),
            previous_path: f["previous_filename"].as_str().map(String::from),
            status: s(&f["status"]),
            additions: f["additions"].as_u64().unwrap_or_default(),
            deletions: f["deletions"].as_u64().unwrap_or_default(),
            commentable: f["patch"].as_str().map(commentable_ranges),
        })
        .collect();
    Ok(PRDetails {
        owner: owner.into(),
        repo: repo.into(),
        number,
        title: s(&pr["title"]),
        body: s(&pr["body"]),
        author: s(&pr["user"]["login"]),
        url: s(&pr["html_url"]),
        base_ref: s(&pr["base"]["ref"]),
        base_sha: s(&pr["base"]["sha"]),
        head_sha: s(&pr["head"]["sha"]),
        files,
    })
}

// ---- Reviews ----

const CONVERSATION_FIELDS: &str = "id body createdAt url author { login avatarUrl }";

fn to_thread(t: &Value) -> ReviewThread {
    ReviewThread {
        id: s(&t["id"]),
        path: s(&t["path"]),
        side: s(&t["diffSide"]),
        line: t["line"].as_u64(),
        start_line: t["startLine"].as_u64(),
        is_resolved: t["isResolved"].as_bool().unwrap_or(false),
        is_outdated: t["isOutdated"].as_bool().unwrap_or(false),
        viewer_can_resolve: t["viewerCanResolve"].as_bool().unwrap_or(false),
        viewer_can_unresolve: t["viewerCanUnresolve"].as_bool().unwrap_or(false),
        comments: t["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| ReviewComment {
                id: s(&c["id"]),
                author: c["author"]["login"].as_str().unwrap_or("ghost").into(),
                author_avatar: s(&c["author"]["avatarUrl"]),
                body: s(&c["body"]),
                created_at: s(&c["createdAt"]),
                url: s(&c["url"]),
                pending: c["state"].as_str() == Some("PENDING"),
                viewer_can_delete: c["viewerCanDelete"].as_bool().unwrap_or(false),
            })
            .collect(),
    }
}

fn to_conversation_item(kind: &str, c: &Value) -> ConversationItem {
    ConversationItem {
        kind: kind.into(),
        id: s(&c["id"]),
        author: c["author"]["login"].as_str().unwrap_or("ghost").into(),
        author_avatar: s(&c["author"]["avatarUrl"]),
        body: s(&c["body"]),
        created_at: s(&c["createdAt"]),
        url: s(&c["url"]),
        review_state: None,
    }
}

/// One pending review per PR. Holding the lock while creating one keeps
/// concurrent comment adds from each creating their own.
static PENDING_REVIEWS: Mutex<Option<HashMap<String, String>>> = Mutex::const_new(None);

pub async fn review_state(owner: &str, repo: &str, number: u64) -> Result<ReviewState, String> {
    gh::in_repo(owner, repo, review_state_inner(owner, repo, number)).await
}

async fn review_state_inner(owner: &str, repo: &str, number: u64) -> Result<ReviewState, String> {
    let mut threads = Vec::new();
    let mut after: Option<String> = None;
    let mut base: Option<(String, String, Option<String>, Vec<ConversationItem>)> = None;
    loop {
        let query = format!(
            r#"query($owner: String!, $repo: String!, $number: Int!, $after: String, $first: Boolean!) {{
              viewer {{ login }}
              repository(owner: $owner, name: $repo) {{
                pullRequest(number: $number) {{
                  id
                  reviews(states: PENDING, first: 20) {{ nodes {{ id viewerDidAuthor }} }}
                  comments(last: 100) @include(if: $first) {{ nodes {{ {CONVERSATION_FIELDS} }} }}
                  submitted: reviews(states: [APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED], last: 100) @include(if: $first) {{
                    nodes {{ state {CONVERSATION_FIELDS} }}
                  }}
                  reviewThreads(first: 100, after: $after) {{
                    pageInfo {{ hasNextPage endCursor }}
                    nodes {{
                      id path diffSide line startLine isResolved isOutdated viewerCanResolve viewerCanUnresolve
                      comments(first: 100) {{ nodes {{ id body createdAt url state viewerCanDelete author {{ login avatarUrl }} }} }}
                    }}
                  }}
                }}
              }}
            }}"#
        );
        let data = graphql(
            &query,
            json!({ "owner": owner, "repo": repo, "number": number, "after": after, "first": base.is_none() }),
        )
        .await?;
        let pr = &data["repository"]["pullRequest"];
        if base.is_none() {
            let pending = pr["reviews"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["viewerDidAuthor"].as_bool() == Some(true))
                .map(|r| s(&r["id"]));
            let mut conversation: Vec<ConversationItem> =
                pr["comments"]["nodes"].as_array().into_iter().flatten().map(|c| to_conversation_item("comment", c)).collect();
            for r in pr["submitted"]["nodes"].as_array().into_iter().flatten() {
                let state = s(&r["state"]);
                // A review with no summary text only carries inline comments, which show under files.
                if !s(&r["body"]).trim().is_empty() || state == "APPROVED" || state == "CHANGES_REQUESTED" {
                    let mut item = to_conversation_item("review", r);
                    item.review_state = Some(state);
                    conversation.push(item);
                }
            }
            conversation.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            base = Some((s(&pr["id"]), s(&data["viewer"]["login"]), pending, conversation));
        }
        threads.extend(pr["reviewThreads"]["nodes"].as_array().into_iter().flatten().map(to_thread));
        let page = &pr["reviewThreads"]["pageInfo"];
        after = if page["hasNextPage"].as_bool() == Some(true) { page["endCursor"].as_str().map(String::from) } else { None };
        if after.is_none() {
            break;
        }
    }
    let (pr_node_id, viewer, pending_review_id, conversation) = base.ok_or("no pull request data")?;

    // GitHub is the source of truth for whether a pending review exists.
    let mut cache = PENDING_REVIEWS.lock().await;
    let map = cache.get_or_insert_with(HashMap::new);
    match &pending_review_id {
        Some(id) => map.insert(pr_node_id.clone(), id.clone()),
        None => map.remove(&pr_node_id),
    };
    Ok(ReviewState { pr_node_id, viewer, pending_review_id, threads, conversation })
}

async fn ensure_pending_review(h: &ReviewHandle, head_sha: &str) -> Result<String, String> {
    let mut cache = PENDING_REVIEWS.lock().await;
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(id) = map.get(&h.pr_node_id).cloned().or_else(|| h.pending_review_id.clone()) {
        return Ok(id);
    }
    let data = graphql(
        "mutation($pr: ID!, $sha: GitObjectID!) { addPullRequestReview(input: { pullRequestId: $pr, commitOID: $sha }) { pullRequestReview { id } } }",
        json!({ "pr": h.pr_node_id, "sha": head_sha }),
    )
    .await?;
    let id = s(&data["addPullRequestReview"]["pullRequestReview"]["id"]);
    map.insert(h.pr_node_id.clone(), id.clone());
    Ok(id)
}

pub async fn add_thread(h: ReviewHandle, head_sha: &str, t: NewThread) -> Result<(), String> {
    let review = ensure_pending_review(&h, head_sha).await?;
    let multi_line = t.start_line.is_some_and(|s| s < t.line);
    graphql(
        r#"mutation($review: ID!, $path: String!, $body: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide) {
          addPullRequestReviewThread(input: {
            pullRequestReviewId: $review, path: $path, body: $body, line: $line, side: $side,
            startLine: $startLine, startSide: $startSide, subjectType: LINE
          }) { thread { id } }
        }"#,
        json!({
            "review": review, "path": t.path, "body": t.body, "line": t.line, "side": t.side,
            "startLine": if multi_line { t.start_line } else { None },
            "startSide": if multi_line { Some(&t.side) } else { None },
        }),
    )
    .await
    .map(|_| ())
}

pub async fn reply(h: ReviewHandle, head_sha: &str, thread_id: &str, body: &str) -> Result<(), String> {
    let review = ensure_pending_review(&h, head_sha).await?;
    graphql(
        "mutation($review: ID!, $thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: { pullRequestReviewId: $review, pullRequestReviewThreadId: $thread, body: $body }) { comment { id } } }",
        json!({ "review": review, "thread": thread_id, "body": body }),
    )
    .await
    .map(|_| ())
}

pub async fn delete_comment(id: &str) -> Result<(), String> {
    graphql("mutation($id: ID!) { deletePullRequestReviewComment(input: { id: $id }) { clientMutationId } }", json!({ "id": id }))
        .await
        .map(|_| ())
}

pub async fn add_pr_comment(pr_node_id: &str, body: &str) -> Result<(), String> {
    graphql(
        "mutation($id: ID!, $body: String!) { addComment(input: { subjectId: $id, body: $body }) { clientMutationId } }",
        json!({ "id": pr_node_id, "body": body }),
    )
    .await
    .map(|_| ())
}

pub async fn set_resolved(thread_id: &str, resolved: bool) -> Result<(), String> {
    let mutation = if resolved { "resolveReviewThread" } else { "unresolveReviewThread" };
    graphql(&format!("mutation($id: ID!) {{ {mutation}(input: {{ threadId: $id }}) {{ thread {{ id }} }} }}"), json!({ "id": thread_id }))
        .await
        .map(|_| ())
}

pub async fn submit_review(h: ReviewHandle, head_sha: &str, event: &str, body: &str) -> Result<(), String> {
    let body = if body.is_empty() { None } else { Some(body) };
    let pending = {
        let mut cache = PENDING_REVIEWS.lock().await;
        cache.get_or_insert_with(HashMap::new).get(&h.pr_node_id).cloned().or(h.pending_review_id.clone())
    };
    if let Some(review) = pending {
        graphql(
            "mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) { submitPullRequestReview(input: { pullRequestReviewId: $review, event: $event, body: $body }) { pullRequestReview { id } } }",
            json!({ "review": review, "event": event, "body": body }),
        )
        .await?;
    } else {
        // No inline comments: create and submit in one step.
        graphql(
            "mutation($pr: ID!, $sha: GitObjectID!, $event: PullRequestReviewEvent!, $body: String) { addPullRequestReview(input: { pullRequestId: $pr, commitOID: $sha, event: $event, body: $body }) { pullRequestReview { id } } }",
            json!({ "pr": h.pr_node_id, "sha": head_sha, "event": event, "body": body }),
        )
        .await?;
    }
    PENDING_REVIEWS.lock().await.get_or_insert_with(HashMap::new).remove(&h.pr_node_id);
    Ok(())
}
