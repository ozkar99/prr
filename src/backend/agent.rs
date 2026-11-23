//! The coding agent in the bottom panel. Defaults to Claude Code; any other
//! terminal agent (Codex, Gemini CLI, …) can be used by putting its command in
//! ~/.review-tool-agent-command. Every agent gets the review context through
//! environment variables, an MCP server (HTTP, or stdio via a bridge script),
//! and a JSON file with the current selection.
use crate::backend::exec::{data_dir, which};
use crate::backend::mcp;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    /// Short name for the UI ("Claude Code", "codex", …).
    pub name: String,
    /// The configured command, or None for the built-in Claude Code default.
    pub command: Option<String>,
    pub config_path: String,
}

pub fn config_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".review-tool-agent-command")
}

/// The custom command: from Settings, else the legacy file's non-blank,
/// non-`#` lines joined with spaces.
fn configured_command() -> Option<String> {
    let from_settings = crate::settings::load().agent_command;
    if !from_settings.trim().is_empty() {
        return Some(from_settings.trim().to_string());
    }
    let text = std::fs::read_to_string(config_path()).ok()?;
    let command = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    (!command.is_empty()).then_some(command)
}

pub fn info() -> AgentInfo {
    let command = configured_command();
    let name = match &command {
        None => "Claude Code".into(),
        Some(c) => c
            .split_whitespace()
            .find(|w| !w.contains('=')) // skip leading VAR=value
            .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
            .unwrap_or_else(|| "agent".into()),
    };
    AgentInfo { name, command, config_path: config_path().to_string_lossy().into() }
}

fn instructions(pr_label: &str, merge_base: &str) -> String {
    format!(
        "You are running inside prr, a desktop app for reviewing GitHub pull requests. \
         The working directory is a checkout of {pr_label} at the PR's head commit; the PR's changes are the diff against merge-base {merge_base}. \
         The user is reviewing this PR, not editing it: you have read-only access (reading and searching files, and the review-tool tools); you can't run commands or change files. \
         When the user refers to code they're looking at (\"this\", \"here\", \"the selection\", \"this function\"), call the review-tool get_selection tool first \
         (or read the JSON file in $REVIEW_TOOL_SELECTION_FILE if you have no MCP tools). \
         Use get_pr for the PR description and changed files, get_file_diff to see what changed in a file, and git_log, git_blame and git_show for history. \
         When the user asks you to comment, reply, resolve a thread, or approve / request changes on / submit the review, use the review-tool tools for it \
         (get_review_threads for thread ids); the app shows each action to the user and only does it if they confirm, so call the tool rather than asking in chat."
    )
}

/// Writes a launcher for the stdio MCP bridge with this launch's URL and token
/// baked in, for agents that only speak stdio MCP or start MCP servers with a
/// cleared environment. Readable only by the user (it contains the token).
fn write_stdio_bridge(url: &str, token: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // One per window (session), named by the start of its token.
    let path = data_dir().join(format!("mcp-stdio-{}.sh", &token[..12.min(token.len())]));
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let script = format!("#!/bin/sh\nexec {} --mcp-stdio {} {}\n", quote(&exe.to_string_lossy()), quote(url), quote(token));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o700)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(path)
}


/// The agent command for one PR window, wired to that window's MCP session.
pub fn launch(pr_label: &str, merge_base: &str, root: &str, session: &mcp::Session) -> Result<Launch, String> {
    let (url, token) = (session.url(), session.token.clone());
    let mcp_config = serde_json::json!({
        "mcpServers": { "review-tool": {
            "type": "http",
            "url": url,
            "headers": { "Authorization": format!("Bearer {token}") }
        }}
    })
    .to_string();
    let instructions = instructions(pr_label, merge_base);
    let bridge = write_stdio_bridge(&url, &token)?;

    let env: Vec<(String, String)> = vec![
        ("REVIEW_TOOL_ROOT".into(), root.into()),
        ("REVIEW_TOOL_PR".into(), pr_label.into()),
        ("REVIEW_TOOL_MERGE_BASE".into(), merge_base.into()),
        ("REVIEW_TOOL_INSTRUCTIONS".into(), instructions.clone()),
        ("REVIEW_TOOL_MCP_URL".into(), url),
        ("REVIEW_TOOL_MCP_TOKEN".into(), token),
        // Claude Code-style --mcp-config JSON (also the shape several other agents use).
        ("REVIEW_TOOL_MCP_CONFIG".into(), mcp_config.clone()),
        ("REVIEW_TOOL_MCP_STDIO".into(), bridge.to_string_lossy().into()),
        ("REVIEW_TOOL_SELECTION_FILE".into(), session.selection_file().to_string_lossy().into()),
    ];

    if let Some(command) = configured_command() {
        // Run through the user's login shell so tools installed via npm -g, ~/.local/bin, etc. are on PATH.
        let shell = std::env::var("SHELL").ok().filter(|s| which(s).is_some()).unwrap_or_else(|| "/bin/sh".into());
        return Ok(Launch { program: shell, args: vec!["-l".into(), "-c".into(), command], env });
    }

    // Skip Claude Code's "trust this folder?" for PR checkouts (Settings; on by default).
    if crate::settings::load().trust_claude_checkouts
        && let Err(e) = trust_in_claude_code(Path::new(root))
    {
        eprintln!("prr: couldn't mark the checkout as trusted in Claude Code: {e}");
    }

    let claude = which("claude").ok_or(
        "Claude Code isn't installed (curl -fsSL https://claude.ai/install.sh | bash), \
         or put another agent's command in ~/.review-tool-agent-command",
    )?;
    let args: Vec<String> = vec![
        "--mcp-config".into(),
        mcp_config,
        // PR code is untrusted: ignore the repo's own .mcp.json and .claude/settings.json (hooks, permissions).
        "--strict-mcp-config".into(),
        "--setting-sources".into(),
        "user".into(),
        "--append-system-prompt".into(),
        instructions,
        // Read-only: only these built-in tools exist in the session (no shell, no
        // editing, no web), so no permission mode can bring the others back. Git
        // history comes from the app's MCP tools (git_log, git_blame, git_show).
        "--tools".into(),
        "Read,Grep,Glob".into(),
        // Not the user's default mode (e.g. auto): nothing runs unasked here. The
        // review-tool MCP tools are allowed below; their actions are confirmed in the app.
        "--permission-mode".into(),
        "manual".into(),
        "--allowedTools".into(),
        "mcp__review-tool".into(),
    ];
    Ok(Launch { program: claude.to_string_lossy().into(), args, env })
}

/// Marks the repository behind `root` as trusted in Claude Code's own config
/// (`~/.claude.json`, `projects["<repo>"].hasTrustDialogAccepted`), so its
/// "trust this folder?" prompt doesn't come up for PR checkouts. Claude Code
/// keys trust by the repository's git folder (minus a trailing `/.git`), which
/// covers every PR checkout and worktree of that repository. Nothing is written
/// when it's already trusted; otherwise only that flag is set, and the file is
/// replaced atomically with everything else kept as it was.
///
/// The panel's Claude Code ignores the repository's own settings, hooks and
/// MCP servers and is read-only, which is what the prompt guards against.
pub fn trust_in_claude_code(root: &Path) -> Result<(), String> {
    let path = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude.json");
    trust_in_claude_config(&path, root)
}

fn trust_in_claude_config(path: &Path, root: &Path) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("not a git checkout".into());
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let key = common.strip_suffix("/.git").unwrap_or(&common).to_string();

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        // No Claude Code config yet: let Claude Code create it (and ask once).
        Err(_) => return Ok(()),
    };
    let mut config: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("~/.claude.json: {e}"))?;
    let projects = config
        .as_object_mut()
        .ok_or("~/.claude.json isn't an object")?
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}));
    let project = projects.as_object_mut().ok_or("projects isn't an object")?.entry(key).or_insert_with(|| serde_json::json!({}));
    if project["hasTrustDialogAccepted"] == serde_json::Value::Bool(true) {
        return Ok(());
    }
    project.as_object_mut().ok_or("project entry isn't an object")?.insert("hasTrustDialogAccepted".into(), true.into());
    let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("json.prr-{}", std::process::id()));
    std::fs::write(&tmp, body + "\n").map_err(|e| e.to_string())?;
    // Keep the file's permissions (it can hold credentials).
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusts_the_repository_once_and_keeps_everything_else() {
        let dir = std::env::temp_dir().join(format!("prr-trust-{}", std::process::id()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
        let repo = repo.canonicalize().unwrap();
        let config = dir.join("claude.json");
        std::fs::write(&config, r#"{"numStartups": 3, "projects": {"/other": {"hasTrustDialogAccepted": false, "x": 1}}, "zLast": true}"#).unwrap();

        trust_in_claude_config(&config, &repo).unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let key = repo.to_string_lossy().to_string();
        assert_eq!(v["projects"][&key]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"]["/other"]["hasTrustDialogAccepted"], false, "other entries untouched");
        assert_eq!((v["numStartups"].clone(), v["zLast"].clone()), (3.into(), true.into()));
        assert!(text.find("numStartups").unwrap() < text.find("zLast").unwrap(), "key order kept");

        // Already trusted: the file isn't rewritten.
        let before = std::fs::metadata(&config).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        trust_in_claude_config(&config, &repo).unwrap();
        assert_eq!(std::fs::metadata(&config).unwrap().modified().unwrap(), before);

        // A subfolder of the repository maps to the same entry.
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        std::fs::write(&config, "{}").unwrap();
        trust_in_claude_config(&config, &repo.join("sub")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(v["projects"][&key]["hasTrustDialogAccepted"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `prr --mcp-stdio <url> <token>`: a stdio MCP server that forwards
/// each newline-delimited JSON-RPC message to the app's HTTP MCP endpoint.
pub fn run_stdio_bridge(url: &str, token: &str) {
    use std::io::{BufRead, Read};
    use std::net::TcpStream;
    let Some(rest) = url.strip_prefix("http://") else { return };
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut stream) = TcpStream::connect(host) else {
            eprintln!("prr: can't reach the app at {url} (is it still running?)");
            break;
        };
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{line}",
            line.len()
        );
        if stream.write_all(request.as_bytes()).is_err() {
            break;
        }
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        // Notifications get 202 with no body; requests get one JSON-RPC message.
        if let Some((_, body)) = response.split_once("\r\n\r\n") {
            let body = body.trim();
            if !body.is_empty() {
                let _ = writeln!(stdout, "{body}");
                let _ = stdout.flush();
            }
        }
    }
}
