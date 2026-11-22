//! Language servers spawned per PR. Rust only frames LSP messages: stdout ->
//! Channel to the page, and `lsp_send` from the page -> stdin.
use crate::exec::which;
use crate::fsaccess::PYRIGHT_DIR;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

struct Server {
    stdin: Arc<Mutex<ChildStdin>>,
    kill: oneshot::Sender<()>,
    pid: Option<u32>,
}

static SERVERS: Mutex<Option<HashMap<u32, Server>>> = Mutex::const_new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn install_hint(lang: &str) -> &'static str {
    match lang {
        "go" => "go install golang.org/x/tools/gopls@latest",
        "cpp" => "xcode-select --install  (or: brew install llvm)",
        "python" => "pyright runs on Node.js: brew install node",
        "starlark" => "download starpls from https://github.com/withered-magic/starpls/releases and put it on your PATH",
        _ => "",
    }
}

fn command(lang: &str, root: &Path) -> Result<(String, Vec<String>, Vec<(&'static str, &'static str)>), String> {
    Ok(match lang {
        "go" => {
            // GOTOOLCHAIN=local stops a PR's go.mod from making us download and run a different Go toolchain.
            ("gopls".into(), vec!["serve".into()], vec![("GOTOOLCHAIN", "local")])
        }
        "cpp" => {
            let brew = "/opt/homebrew/opt/llvm/bin/clangd";
            let cmd = if Path::new(brew).exists() { brew } else { "clangd" };
            let mut args: Vec<String> =
                ["--background-index", "--header-insertion=never", "--clang-tidy=false", "--pch-storage=memory"].map(String::from).into();
            if let Some(dir) =
                ["build", "out", "cmake-build-debug", "builddir"].iter().map(|d| root.join(d)).find(|d| d.join("compile_commands.json").exists())
            {
                args.push(format!("--compile-commands-dir={}", dir.display()));
            }
            (cmd.into(), args, vec![])
        }
        "python" => {
            let dir = PYRIGHT_DIR.get().cloned().flatten().ok_or("pyright isn't bundled with this build")?;
            ("node".into(), vec![dir.join("langserver.index.js").to_string_lossy().into(), "--stdio".into()], vec![])
        }
        "starlark" => ("starpls".into(), vec!["server".into()], vec![]),
        other => return Err(format!("unsupported language: {other}")),
    })
}

/// Reads one `Content-Length`-framed JSON-RPC message.
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; len?];
    r.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

pub async fn start(app: AppHandle, lang: String, root: String, on_message: Channel<Value>) -> Result<u32, String> {
    let (cmd, args, env) = command(&lang, Path::new(&root))?;
    let resolved = which(&cmd).ok_or_else(|| format!("not installed: {cmd}. To install: {}", install_hint(&lang)))?;
    let mut child = Command::new(resolved)
        .args(&args)
        .envs(env)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let pid = child.id();
    let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let (kill_tx, kill_rx) = oneshot::channel();
    SERVERS.lock().await.get_or_insert_with(HashMap::new).insert(id, Server { stdin, kill: kill_tx, pid });

    tokio::spawn(async move {
        while let Some(msg) = read_message(&mut stdout).await {
            if on_message.send(msg).is_err() {
                break;
            }
        }
    });
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let tail = stderr_tail.clone();
    tokio::spawn(async move {
        let mut line = String::new();
        while stderr.read_line(&mut line).await.unwrap_or(0) > 0 {
            let mut t = tail.lock().await;
            t.push_str(&line);
            if t.len() > 4000 {
                let cut = t.len() - 4000;
                *t = t[t.ceil_char_boundary(cut)..].to_string();
            }
            line.clear();
        }
    });
    tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status.ok(),
            _ = kill_rx => { let _ = child.kill().await; None }
        };
        // Only report exits we didn't ask for.
        let unexpected = SERVERS.lock().await.get_or_insert_with(HashMap::new).remove(&id).is_some();
        if unexpected {
            let code = status.and_then(|s| s.code());
            let tail = stderr_tail.lock().await.clone();
            let _ = app.emit("lsp:exit", serde_json::json!({ "id": id, "lang": lang, "code": code, "stderrTail": tail }));
        }
    });
    Ok(id)
}

pub async fn send(id: u32, message: Value) -> Result<(), String> {
    let stdin = {
        let servers = SERVERS.lock().await;
        servers.as_ref().and_then(|m| m.get(&id)).map(|s| s.stdin.clone()).ok_or("server not running")?
    };
    let body = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
    let mut w = stdin.lock().await;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await.map_err(|e| e.to_string())?;
    w.write_all(&body).await.map_err(|e| e.to_string())?;
    w.flush().await.map_err(|e| e.to_string())
}

pub async fn stop(id: u32) {
    if let Some(server) = SERVERS.lock().await.get_or_insert_with(HashMap::new).remove(&id) {
        let _ = server.kill.send(());
    }
}

/// Stops every server. Signals the processes directly, since on quit the
/// tasks that would kill them may never run again.
pub async fn stop_all() {
    if let Some(map) = SERVERS.lock().await.as_mut() {
        for (_, server) in map.drain() {
            if let Some(pid) = server.pid {
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            }
            let _ = server.kill.send(());
        }
    }
}
