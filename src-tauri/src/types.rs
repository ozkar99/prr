//! Mirrors src/shared/types.ts (camelCase on the wire).
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PRRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

/// Parses a PR URL or owner/repo#123.
pub fn parse_pr_ref(input: &str) -> Option<PRRef> {
    let url = regex::Regex::new(r"github\.com/([^/]+)/([^/]+)/pull/(\d+)").unwrap();
    let short = regex::Regex::new(r"^([\w.-]+)/([\w.-]+)#(\d+)$").unwrap();
    let s = input.trim();
    let caps = url.captures(s).or_else(|| short.captures(s))?;
    Some(PRRef { owner: caps[1].into(), repo: caps[2].into(), number: caps[3].parse().ok()? })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PRSummary {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub author: String,
    pub updated_at: String,
    pub draft: bool,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CommentableRanges {
    #[serde(rename = "LEFT")]
    pub left: Vec<(u64, u64)>,
    #[serde(rename = "RIGHT")]
    pub right: Vec<(u64, u64)>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PRFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commentable: Option<CommentableRanges>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PRDetails {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub url: String,
    pub base_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<PRFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkout {
    pub root: String,
    pub merge_base: String,
    pub head_sha: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: String,
    pub author: String,
    pub author_avatar: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub pending: bool,
    pub viewer_can_delete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    pub side: String,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationItem {
    pub kind: String,
    pub id: String,
    pub author: String,
    pub author_avatar: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_state: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewState {
    pub pr_node_id: String,
    pub viewer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_review_id: Option<String>,
    pub threads: Vec<ReviewThread>,
    pub conversation: Vec<ConversationItem>,
}

/// The part of ReviewState the mutations need.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewHandle {
    pub pr_node_id: String,
    pub pending_review_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewThread {
    pub path: String,
    pub side: String,
    pub line: u64,
    pub start_line: Option<u64>,
    pub body: String,
}

#[derive(Serialize)]
pub struct FileStat {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub size: u64,
    pub mtime: f64,
    pub ctime: f64,
}
