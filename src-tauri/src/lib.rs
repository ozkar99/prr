mod devtools;
mod exec;
mod fsaccess;
mod git;
mod github;
mod lsp;
mod mcp;
mod pty;
mod types;

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, TitleBarStyle, Url, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use types::*;

/// A PR the app was launched with (`review-tool <pr>`), consumed once by the page.
struct LaunchPR(Mutex<Option<PRRef>>);

fn pr_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<PRRef> {
    args.into_iter().find_map(|a| a.strip_prefix("--open-pr=").and_then(parse_pr_ref))
}

// ---- Commands (same surface as the Electron preload's window.api) ----

#[tauri::command]
async fn review_queue() -> Result<Vec<PRSummary>, String> {
    github::review_queue().await
}

#[tauri::command]
async fn get_pr(owner: String, repo: String, number: u64) -> Result<PRDetails, String> {
    github::get_pr(&owner, &repo, number).await
}

#[tauri::command]
async fn checkout(app: AppHandle, pr: PRDetails) -> Result<Checkout, String> {
    git::checkout(&pr, |msg| {
        let _ = app.emit("progress", msg);
    })
    .await
}

#[tauri::command]
async fn read_base(owner: String, repo: String, sha: String, path: String) -> Result<Option<String>, String> {
    git::read_at_commit(&owner, &repo, &sha, &path).await
}

#[tauri::command]
async fn fs_stat(path: String) -> Option<FileStat> {
    fsaccess::stat(&path).await
}

/// Raw bytes, not a JSON number array.
#[tauri::command]
async fn fs_read(path: String) -> Result<Response, String> {
    fsaccess::read(&path).await.map(Response::new)
}

#[tauri::command]
async fn fs_readdir(path: String) -> Result<Vec<(String, &'static str)>, String> {
    fsaccess::readdir(&path).await
}

#[tauri::command]
async fn lsp_start(app: AppHandle, lang: String, root: String, on_message: Channel<Value>) -> Result<u32, String> {
    lsp::start(app, lang, root, on_message).await
}

#[tauri::command]
async fn lsp_send(id: u32, message: Value) -> Result<(), String> {
    lsp::send(id, message).await
}

#[tauri::command]
async fn lsp_stop(id: u32) {
    lsp::stop(id).await
}

#[tauri::command]
async fn review_state(owner: String, repo: String, number: u64) -> Result<ReviewState, String> {
    github::review_state(&owner, &repo, number).await
}

#[tauri::command]
async fn add_thread(state: ReviewHandle, head_sha: String, thread: NewThread) -> Result<(), String> {
    github::add_thread(state, &head_sha, thread).await
}

#[tauri::command]
async fn reply(state: ReviewHandle, head_sha: String, thread_id: String, body: String) -> Result<(), String> {
    github::reply(state, &head_sha, &thread_id, &body).await
}

#[tauri::command]
async fn delete_comment(id: String) -> Result<(), String> {
    github::delete_comment(&id).await
}

#[tauri::command]
async fn add_pr_comment(pr_node_id: String, body: String) -> Result<(), String> {
    github::add_pr_comment(&pr_node_id, &body).await
}

#[tauri::command]
async fn set_resolved(thread_id: String, resolved: bool) -> Result<(), String> {
    github::set_resolved(&thread_id, resolved).await
}

#[tauri::command]
async fn submit_review(state: ReviewHandle, head_sha: String, event: String, body: String) -> Result<(), String> {
    github::submit_review(state, &head_sha, &event, &body).await
}

/// Opens http(s) links (PR page, links in comments and hovers) in the default browser.
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("refusing to open {url}"));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ---- Claude Code panel ----

/// The page reports the open PR (and its checkout) and the editor selection;
/// the MCP server hands them to Claude Code.
#[tauri::command]
fn set_review_context(review: Value) {
    mcp::set_review(review)
}

#[tauri::command]
fn set_selection(selection: Value) {
    mcp::set_selection(selection)
}

/// Starts Claude Code in the PR checkout, wired to the review-tool MCP server.
#[tauri::command]
fn claude_start(app: AppHandle, root: String, pr_label: String, merge_base: String, cols: u16, rows: u16, on_data: Channel<InvokeResponseBody>) -> Result<u32, String> {
    let claude = exec::which("claude")
        .ok_or("Claude Code isn't installed: see https://docs.claude.com/en/docs/claude-code (curl -fsSL https://claude.ai/install.sh | bash)")?;
    let (port, token) = mcp::ensure_started()?;
    let mcp_config = serde_json::json!({
        "mcpServers": { "review-tool": {
            "type": "http",
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "headers": { "Authorization": format!("Bearer {token}") }
        }}
    });
    let system = format!(
        "You are running inside Review Tool, a desktop app for reviewing GitHub pull requests. \
         The working directory is a checkout of {pr_label} at the PR's head commit; the PR's changes are the diff against merge-base {merge_base}. \
         The user is reviewing this PR, not editing it: answer questions about the code and the change, and don't modify files. \
         When the user refers to code they're looking at (\"this\", \"here\", \"the selection\", \"this function\"), call the review-tool get_selection tool first. \
         Use get_pr for the PR description and changed files, and get_file_diff to see what changed in a file."
    );
    let args: Vec<String> = vec![
        "--mcp-config".into(),
        mcp_config.to_string(),
        // PR code is untrusted: ignore the repo's own .mcp.json and .claude/settings.json (hooks, permissions).
        "--strict-mcp-config".into(),
        "--setting-sources".into(),
        "user".into(),
        "--append-system-prompt".into(),
        system,
        "--allowedTools".into(),
        "mcp__review-tool".into(),
        "Bash(git diff:*)".into(),
        "Bash(git log:*)".into(),
        "Bash(git show:*)".into(),
        "Bash(git blame:*)".into(),
        "--disallowedTools".into(),
        "Edit".into(),
        "Write".into(),
        "NotebookEdit".into(),
    ];
    pty::spawn(app, &claude.to_string_lossy(), &args, &root, cols, rows, on_data)
}

#[tauri::command]
fn pty_write(id: u32, data: String) -> Result<(), String> {
    pty::write(id, &data)
}

#[tauri::command]
fn pty_resize(id: u32, cols: u16, rows: u16) -> Result<(), String> {
    pty::resize(id, cols, rows)
}

#[tauri::command]
fn pty_kill(id: u32) {
    pty::kill(id)
}

#[tauri::command]
fn take_launch_pr(launch: State<LaunchPR>) -> Option<PRRef> {
    launch.0.lock().unwrap().take()
}

// ---- App ----

/// Pyright ships inside the app bundle; in development it comes from node_modules.
fn find_pyright(app: &AppHandle) -> Option<PathBuf> {
    let bundled = app.path().resource_dir().ok().map(|d| d.join("pyright"));
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../node_modules/pyright");
    [bundled, Some(dev)].into_iter().flatten().find(|d| d.join("langserver.index.js").exists()).and_then(|d| d.canonicalize().ok())
}

/// The window may only show the app itself; links go to the browser.
fn allow_navigation(app: &AppHandle, url: &Url) -> bool {
    let is_app = url.scheme() == "tauri"
        || (url.scheme() == "http" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    if !is_app && matches!(url.scheme(), "http" | "https") {
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    }
    is_app
}

pub fn run() {
    exec::augment_path();
    let launch = pr_from_args(std::env::args());

    let app = tauri::Builder::default()
        // Must be first: a second launch hands its PR to this instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
            if let Some(pr) = pr_from_args(argv) {
                let _ = app.emit_to("main", "app:openPR", pr);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .manage(LaunchPR(Mutex::new(launch)))
        .on_page_load(|_webview, payload| {
            // A reload orphans the page's language clients; stop their servers.
            if payload.event() == PageLoadEvent::Started {
                tauri::async_runtime::spawn(lsp::stop_all());
                pty::kill_all();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let _ = fsaccess::PYRIGHT_DIR.set(find_pyright(&handle));
            let nav = handle.clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Review Tool")
                .inner_size(1500.0, 950.0)
                .title_bar_style(TitleBarStyle::Overlay)
                .hidden_title(true)
                .on_navigation(move |url| allow_navigation(&nav, url))
                .build()?;
            devtools::start(&handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            review_queue,
            get_pr,
            checkout,
            read_base,
            fs_stat,
            fs_read,
            fs_readdir,
            lsp_start,
            lsp_send,
            lsp_stop,
            review_state,
            add_thread,
            reply,
            delete_comment,
            add_pr_comment,
            set_resolved,
            submit_review,
            open_url,
            take_launch_pr,
            set_review_context,
            set_selection,
            claude_start,
            pty_write,
            pty_resize,
            pty_kill,
            devtools::debug_result
        ])
        .build(tauri::generate_context!())
        .expect("error while building review-tool");

    app.run(|_app, event| {
        if let RunEvent::Exit = event {
            pty::kill_all();
            tauri::async_runtime::block_on(lsp::stop_all());
        }
    });
}
