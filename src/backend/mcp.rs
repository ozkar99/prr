//! A small MCP server (streamable HTTP, JSON responses) that gives each PR
//! window's agent what the reviewer is looking at in that window: the current
//! selection, the PR, and per-file diffs. Listens on 127.0.0.1 only. Every PR
//! window has its own session, identified by its bearer token, so agents in
//! different windows only see their own PR.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// What a PR window reports: its PR and its current editor selection.
#[derive(Default)]
struct Context {
    review: Value,
    selection: Value,
}

/// Sessions by bearer token.
static SESSIONS: Mutex<Option<HashMap<String, Context>>> = Mutex::new(None);

/// Something the agent asks the PR window to do on GitHub (or read from it).
/// Every change is confirmed by the user in the window first.
#[derive(Clone, Debug)]
pub enum ReviewAction {
    /// The PR's review threads (read-only; no confirmation).
    Threads,
    Comment { path: String, side: String, line: u64, start_line: Option<u64>, body: String },
    Reply { thread_id: String, body: String },
    Resolve { thread_id: String, resolved: bool },
    PrComment { body: String },
    /// event: APPROVE, REQUEST_CHANGES or COMMENT.
    Submit { event: &'static str, body: String },
}

pub struct ActionRequest {
    pub action: ReviewAction,
    pub reply: std::sync::mpsc::Sender<Result<String, String>>,
}

type ActionSender = futures::channel::mpsc::UnboundedSender<ActionRequest>;
static ACTIONS: Mutex<Option<HashMap<String, ActionSender>>> = Mutex::new(None);

/// Hands an action to the session's window and waits for the outcome (the user
/// may take a while to confirm).
fn run_action(token: &str, action: ReviewAction) -> Result<String, String> {
    let sender = ACTIONS.lock().unwrap().as_ref().and_then(|m| m.get(token).cloned()).ok_or("The review window isn't available.")?;
    let (tx, rx) = std::sync::mpsc::channel();
    sender.unbounded_send(ActionRequest { action, reply: tx }).map_err(|_| "The review window was closed.".to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(30 * 60)).map_err(|_| "No answer from the user in the app.".to_string())?
}
static PORT: OnceLock<u16> = OnceLock::new();

/// One PR window's connection to the MCP server.
#[derive(Clone, Debug)]
pub struct Session {
    pub port: u16,
    pub token: String,
}

impl Session {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// $REVIEW_TOOL_SELECTION_FILE for this window's agent.
    pub fn selection_file(&self) -> PathBuf {
        selection_file(&self.token)
    }

    pub fn set_review(&self, review: Value) {
        with(&self.token, |ctx| {
            ctx.review = review;
            ctx.selection = Value::Null;
        });
        write_selection_file(&self.token);
    }

    pub fn set_selection(&self, selection: Value) {
        with(&self.token, |ctx| ctx.selection = selection);
        write_selection_file(&self.token);
    }

    /// Requests from this session's agent to act on the review; the window handles them.
    pub fn actions(&self) -> futures::channel::mpsc::UnboundedReceiver<ActionRequest> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        ACTIONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(self.token.clone(), tx);
        rx
    }

    /// The window closed: forget its context and its selection file.
    pub fn end(&self) {
        SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&self.token);
        ACTIONS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&self.token);
        let _ = std::fs::remove_file(self.selection_file());
    }
}

/// Starts the server if needed and opens a session for one PR window.
pub fn new_session() -> Result<Session, String> {
    let port = ensure_started()?;
    let token = random_token();
    SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).insert(token.clone(), Context::default());
    Ok(Session { port, token })
}

fn with(token: &str, f: impl FnOnce(&mut Context)) {
    if let Some(ctx) = SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).get_mut(token) {
        f(ctx);
    }
}

fn get(token: &str, f: impl FnOnce(&Context) -> Value) -> Value {
    SESSIONS.lock().unwrap().as_ref().and_then(|m| m.get(token)).map(f).unwrap_or(Value::Null)
}

fn selection_file(token: &str) -> PathBuf {
    crate::backend::exec::data_dir().join("selections").join(format!("{}.json", &token[..12.min(token.len())]))
}

/// Mirrors the selection to $REVIEW_TOOL_SELECTION_FILE for agents without MCP.
fn write_selection_file(token: &str) {
    let review = get(token, |c| c.review.clone());
    let value = json!({
        "pr": if review.is_null() { Value::Null } else { json!({ "repo": review["pr"]["repo"], "number": review["pr"]["number"], "title": review["pr"]["title"] }) },
        "selection": get(token, |c| c.selection.clone()),
    });
    let path = selection_file(token);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&value).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Starts the server once; returns its port.
fn ensure_started() -> Result<u16, String> {
    if let Some(port) = PORT.get() {
        return Ok(*port);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = handle(stream);
            });
        }
    });
    let _ = PORT.set(port);
    Ok(port)
}

fn random_token() -> String {
    let mut bytes = [0u8; 24];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn respond(stream: &mut TcpStream, status: &str, body: Option<&str>) -> std::io::Result<()> {
    let body = body.unwrap_or("");
    let content_type = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    write!(stream, "HTTP/1.1 {status}\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut len = 0usize;
    let mut token = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap_or(("", ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => len = value.trim().parse().unwrap_or(0),
            "authorization" => token = value.trim().strip_prefix("Bearer ").unwrap_or("").to_string(),
            _ => {}
        }
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if path != "/mcp" {
        return respond(&mut stream, "404 Not Found", None);
    }
    let known = !token.is_empty() && SESSIONS.lock().unwrap().as_ref().is_some_and(|m| m.contains_key(&token));
    if !known {
        return respond(&mut stream, "401 Unauthorized", None);
    }
    if method != "POST" {
        // No server-initiated stream; the spec allows 405 here.
        return respond(&mut stream, "405 Method Not Allowed", None);
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return respond(&mut stream, "400 Bad Request", None);
    };
    match rpc(&msg, &token) {
        Some(reply) => respond(&mut stream, "200 OK", Some(&reply.to_string())),
        None => respond(&mut stream, "202 Accepted", None), // notification
    }
}

fn rpc(msg: &Value, token: &str) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "review-tool", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools for the Review Tool app the user is reviewing this pull request in. Call get_selection whenever the user refers to code they're looking at (\"this\", \"here\", \"the selection\", \"this function\"). You can also act on the review for the user (comment, reply, resolve threads, submit the review); each action is shown to the user in the app and only happens if they confirm it there, so call the tool when asked instead of asking for confirmation in chat."
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let (text, is_error) = match call_tool(name, &msg["params"]["arguments"], token) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
        }
        _ => {
            return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {method}") } }))
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn tools() -> Value {
    let read_only = json!({ "readOnlyHint": true });
    let write = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": true });
    json!([
        {
            "name": "get_selection",
            "description": "What the user is looking at in the review app right now: the file, which side of the diff (new = PR head, old = before the PR), the selected line range, and the selected text (or the cursor line when nothing is selected).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only
        },
        {
            "name": "get_pr",
            "description": "The pull request under review: title, URL, author, base branch, description, and the list of changed files with line counts.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only
        },
        {
            "name": "git_log",
            "description": "Commit history (read-only `git log`): the PR's own commits by default (merge-base..HEAD), or the recent history of a file or directory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Repo-relative file or directory. Omit for the whole repo." },
                    "pr_only": { "type": "boolean", "description": "Only the PR's commits (default true). False for history before the PR too." },
                    "max_count": { "type": "integer", "description": "At most this many commits (default 30, max 200)." }
                },
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "git_blame",
            "description": "Who last changed each line of a file, and in which commit (read-only `git blame` at the PR head).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Repo-relative file." },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "git_show",
            "description": "One commit's message and changes (read-only `git show`), optionally limited to one path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commit": { "type": "string", "description": "A commit hash (4-40 hex characters)." },
                    "path": { "type": "string", "description": "Repo-relative path to limit the diff to." }
                },
                "required": ["commit"],
                "additionalProperties": false
            },
            "annotations": read_only
        },
        {
            "name": "get_review_threads",
            "description": "The PR's review comment threads: id (for reply_to_thread / resolve_thread), file, line, side, resolved/outdated, and the comments (author, body, whether pending in the user's review).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only
        },
        {
            "name": "add_review_comment",
            "description": "Adds a comment on a line (or range) of a changed file to the user's pending review; it's published when the review is submitted. The user confirms it in the app first. Only lines inside the PR's diff can be commented on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Repo-relative path of a file the PR changes." },
                    "line": { "type": "integer", "description": "The (last) line to comment on." },
                    "start_line": { "type": "integer", "description": "First line, for a multi-line comment." },
                    "side": { "type": "string", "enum": ["new", "old"], "description": "new = the PR's version (default), old = before the PR." },
                    "body": { "type": "string", "description": "The comment (Markdown)." }
                },
                "required": ["path", "line", "body"],
                "additionalProperties": false
            },
            "annotations": write
        },
        {
            "name": "reply_to_thread",
            "description": "Replies to a review thread (in the user's pending review). The user confirms it in the app first.",
            "inputSchema": {
                "type": "object",
                "properties": { "thread_id": { "type": "string" }, "body": { "type": "string" } },
                "required": ["thread_id", "body"],
                "additionalProperties": false
            },
            "annotations": write
        },
        {
            "name": "resolve_thread",
            "description": "Resolves (or unresolves) a review thread. The user confirms it in the app first.",
            "inputSchema": {
                "type": "object",
                "properties": { "thread_id": { "type": "string" }, "resolved": { "type": "boolean", "description": "Default true." } },
                "required": ["thread_id"],
                "additionalProperties": false
            },
            "annotations": write
        },
        {
            "name": "add_pr_comment",
            "description": "Posts a general comment on the PR's conversation right away (not part of the review). The user confirms it in the app first.",
            "inputSchema": {
                "type": "object",
                "properties": { "body": { "type": "string" } },
                "required": ["body"],
                "additionalProperties": false
            },
            "annotations": write
        },
        {
            "name": "submit_review",
            "description": "Submits the user's review: approve, request changes (GitHub has no 'reject'; use this), or comment, with an optional summary. Publishes the pending comments too. The user confirms it in the app first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "event": { "type": "string", "enum": ["approve", "request_changes", "comment"] },
                    "body": { "type": "string", "description": "Review summary (Markdown). Needed to request changes when there are no pending comments." }
                },
                "required": ["event"],
                "additionalProperties": false
            },
            "annotations": write
        },
        {
            "name": "get_file_diff",
            "description": "The PR's unified diff for one file (against the merge-base, like GitHub shows it). Defaults to the file the user has open.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Repo-relative path. Omit for the file currently open." } },
                "additionalProperties": false
            },
            "annotations": read_only
        }
    ])
}

fn call_tool(name: &str, args: &Value, token: &str) -> Result<String, String> {
    let selection = || get(token, |c| c.selection.clone());
    let review = || get(token, |c| c.review.clone());
    match name {
        "get_selection" => {
            let sel = selection();
            if sel.is_null() {
                return Ok("Nothing is open in the editor yet.".into());
            }
            Ok(serde_json::to_string_pretty(&sel).unwrap())
        }
        "get_pr" => {
            let r = review();
            if r.is_null() {
                return Err("No pull request is open.".into());
            }
            Ok(serde_json::to_string_pretty(&r["pr"]).unwrap())
        }
        "get_file_diff" => {
            let r = review();
            let root = r["root"].as_str().ok_or("No pull request is open.")?;
            let base = r["mergeBase"].as_str().ok_or("No pull request is open.")?;
            let path = args["path"]
                .as_str()
                .map(String::from)
                .or_else(|| selection()["path"].as_str().map(String::from))
                .ok_or("No file given and none is open.")?;
            let out = std::process::Command::new("git")
                .args(["diff", "--no-color", base, "--", &path])
                .current_dir(root)
                .output()
                .map_err(|e| e.to_string())?;
            let diff = String::from_utf8_lossy(&out.stdout).into_owned();
            Ok(if diff.trim().is_empty() { format!("{path} has no changes in this PR.") } else { diff })
        }
        "get_review_threads" => run_action(token, ReviewAction::Threads),
        "add_review_comment" => {
            let path = safe_path(args["path"].as_str().unwrap_or(""))?;
            let line = args["line"].as_u64().ok_or("line is required.")?;
            let start_line = args["start_line"].as_u64().filter(|s| *s < line);
            let side = if args["side"].as_str() == Some("old") { "LEFT" } else { "RIGHT" }.to_string();
            let body = text_arg(args, "body")?;
            run_action(token, ReviewAction::Comment { path, side, line, start_line, body })
        }
        "reply_to_thread" => run_action(token, ReviewAction::Reply { thread_id: text_arg(args, "thread_id")?, body: text_arg(args, "body")? }),
        "resolve_thread" => run_action(
            token,
            ReviewAction::Resolve { thread_id: text_arg(args, "thread_id")?, resolved: args["resolved"].as_bool().unwrap_or(true) },
        ),
        "add_pr_comment" => run_action(token, ReviewAction::PrComment { body: text_arg(args, "body")? }),
        "submit_review" => {
            let event = match args["event"].as_str().unwrap_or("") {
                "approve" => "APPROVE",
                "request_changes" | "reject" => "REQUEST_CHANGES",
                "comment" => "COMMENT",
                other => return Err(format!("event must be approve, request_changes or comment, not {other:?}")),
            };
            let body = args["body"].as_str().unwrap_or("").trim().to_string();
            run_action(token, ReviewAction::Submit { event, body })
        }
        "git_log" | "git_blame" | "git_show" => {
            let r = review();
            let root = r["root"].as_str().ok_or("No pull request is open.")?.to_string();
            let base = r["mergeBase"].as_str().unwrap_or("").to_string();
            let path = match args["path"].as_str() {
                Some(p) => Some(safe_path(p)?),
                None => None,
            };
            let mut cmd: Vec<String> = vec!["--no-pager".into()];
            match name {
                "git_log" => {
                    let n = args["max_count"].as_u64().unwrap_or(30).clamp(1, 200);
                    cmd.extend(["log".into(), "--no-color".into(), format!("--max-count={n}"), "--format=%h %ad %an%n    %s".into(), "--date=short".into()]);
                    if args["pr_only"].as_bool().unwrap_or(true) && !base.is_empty() {
                        cmd.push(format!("{base}..HEAD"));
                    }
                }
                "git_blame" => {
                    if path.is_none() {
                        return Err("git_blame needs a path.".into());
                    }
                    cmd.extend(["blame".into(), "--date=short".into()]);
                    if let Some(start) = args["start_line"].as_u64() {
                        let end = args["end_line"].as_u64().unwrap_or(start).max(start);
                        cmd.push(format!("-L{start},{end}"));
                    }
                    cmd.push("HEAD".into());
                }
                _ => {
                    let commit = args["commit"].as_str().unwrap_or("");
                    if !(4..=40).contains(&commit.len()) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err("commit must be a hash (4-40 hex characters).".into());
                    }
                    cmd.extend(["show".into(), "--no-color".into(), "--stat".into(), "--patch".into(), commit.into()]);
                }
            }
            cmd.push("--".into());
            cmd.extend(path);
            let out = std::process::Command::new("git").args(&cmd).current_dir(&root).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            if text.len() > 60_000 {
                let mut cut = 60_000;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                text.push_str("\n… (truncated)");
            }
            Ok(if text.trim().is_empty() { "No results.".into() } else { text })
        }
        other => Err(format!("Unknown tool: {other}")),
    }
}

/// A required, non-empty string argument.
fn text_arg(args: &Value, name: &str) -> Result<String, String> {
    args[name].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from).ok_or_else(|| format!("{name} is required."))
}

/// A repo-relative path an agent passed: no options, no absolute paths, no `..`.
fn safe_path(p: &str) -> Result<String, String> {
    let p = p.trim();
    if p.is_empty() || p.starts_with('-') || p.starts_with('/') || p.split('/').any(|c| c == "..") {
        return Err(format!("Not a repo-relative path: {p}"));
    }
    Ok(p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_tools_are_read_only_and_reject_unsafe_arguments() {
        let session = new_session().unwrap();
        let root = env!("CARGO_MANIFEST_DIR");
        let head = std::process::Command::new("git").args(["rev-parse", "HEAD~3"]).current_dir(root).output().unwrap();
        let base = String::from_utf8_lossy(&head.stdout).trim().to_string();
        session.set_review(json!({ "root": root, "mergeBase": base, "pr": {} }));
        let call = |name: &str, args: Value| call_tool(name, &args, &session.token);

        let log = call("git_log", json!({})).unwrap();
        assert_eq!(log.lines().filter(|l| !l.starts_with(' ')).count(), 3, "{log}");
        assert!(call("git_log", json!({ "pr_only": false, "max_count": 2, "path": "README.md" })).unwrap().lines().count() <= 4);
        let blame = call("git_blame", json!({ "path": "README.md", "start_line": 1, "end_line": 2 })).unwrap();
        assert_eq!(blame.lines().count(), 2, "{blame}");
        // The last commit that changed the README, limited to that file.
        let last = std::process::Command::new("git").args(["log", "-1", "--format=%h", "--", "README.md"]).current_dir(root).output().unwrap();
        let last = String::from_utf8_lossy(&last.stdout).trim().to_string();
        let show = call("git_show", json!({ "commit": last, "path": "README.md" })).unwrap();
        assert!(show.starts_with("commit "), "{show}");

        for bad in ["../etc/passwd", "/etc/passwd", "--output=/tmp/x", "-p"] {
            assert!(call("git_blame", json!({ "path": bad })).is_err(), "{bad}");
        }
        assert!(call("git_show", json!({ "commit": "HEAD; rm -rf /" })).is_err());
        assert!(call("git_show", json!({ "commit": "--output=/tmp/x" })).is_err());
        session.end();
    }
}
