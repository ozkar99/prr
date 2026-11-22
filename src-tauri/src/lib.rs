mod agent;
mod cli;
mod deps;
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

/// Starts the coding agent (Claude Code unless ~/.review-tool-agent-command says otherwise) in the PR checkout.
#[tauri::command]
fn agent_start(app: AppHandle, root: String, pr_label: String, merge_base: String, cols: u16, rows: u16, on_data: Channel<InvokeResponseBody>) -> Result<u32, String> {
    let launch = agent::launch(&pr_label, &merge_base, &root)?;
    pty::spawn(app, &launch.program, &launch.args, &launch.env, &root, cols, rows, on_data)
}

#[tauri::command]
async fn check_tools() -> Vec<deps::ToolStatus> {
    deps::check().await
}

#[tauri::command]
fn agent_info() -> agent::AgentInfo {
    agent::info()
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

/// bash-language-server: bundled as its own npm install in the app; in development, from node_modules.
fn find_bash_ls(app: &AppHandle) -> Option<PathBuf> {
    let bundled = app.path().resource_dir().ok().map(|d| d.join("bash-language-server/node_modules/bash-language-server/out/cli.js"));
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../node_modules/bash-language-server/out/cli.js");
    [bundled, Some(dev)].into_iter().flatten().find(|p| p.exists()).and_then(|p| p.canonicalize().ok())
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

/// A terminal invocation (`review-tool [PR]`); returns true if it was handled
/// and the app shouldn't start in this process.
pub fn handle_cli(args: &[String]) -> bool {
    cli::handle(args)
}

/// `review-tool --mcp-stdio <url> <token>` runs the stdio MCP bridge instead of the app.
pub fn run_mcp_stdio_bridge(url: &str, token: &str) {
    agent::run_stdio_bridge(url, token)
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
            let _ = fsaccess::BASH_LS.set(find_bash_ls(&handle));
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
            agent_start,
            agent_info,
            check_tools,
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
