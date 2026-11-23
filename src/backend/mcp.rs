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

    /// The window closed: forget its context and its selection file.
    pub fn end(&self) {
        SESSIONS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&self.token);
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
            "instructions": "Tools for the Review Tool app the user is reviewing this pull request in. Call get_selection whenever the user refers to code they're looking at (\"this\", \"here\", \"the selection\", \"this function\")."
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
        other => Err(format!("Unknown tool: {other}")),
    }
}
