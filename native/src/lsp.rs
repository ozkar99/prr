//! In-process LSP client (replaces the web version's relay + monaco-languageclient).
//! One server per language per open PR: gopls, clangd, pyright, starpls,
//! bash-language-server. Speaks JSON-RPC over the server's stdio.
use crate::backend::exec::which;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

/// Server language keys (like the web version's LspLanguage).
pub const LANGUAGES: &[&str] = &["go", "cpp", "python", "starlark", "shell"];

pub fn server_name(lang: &str) -> &'static str {
    match lang {
        "go" => "gopls",
        "cpp" => "clangd",
        "python" => "pyright",
        "starlark" => "starpls",
        "shell" => "bash-language-server",
        _ => "language server",
    }
}

/// Which server handles a file, by the highlighter's language name.
pub fn server_for_language(language: &str) -> Option<&'static str> {
    Some(match language {
        "go" => "go",
        "cpp" | "c" => "cpp",
        "python" => "python",
        "starlark" => "starlark",
        "bash" => "shell",
        _ => return None,
    })
}

/// LSP `languageId` for didOpen.
fn language_id(language: &str) -> &'static str {
    match language {
        "go" => "go",
        "c" => "c",
        "cpp" => "cpp",
        "python" => "python",
        "starlark" => "starlark",
        "bash" => "shellscript",
        _ => "plaintext",
    }
}

fn install_hint(lang: &str) -> &'static str {
    match lang {
        "go" => "go install golang.org/x/tools/gopls@latest",
        "cpp" => "xcode-select --install  (or: brew install llvm)",
        "python" | "shell" => "runs on Node.js: brew install node",
        "starlark" => "download starpls from https://github.com/withered-magic/starpls/releases and put it on your PATH",
        _ => "",
    }
}

/// Node-based servers ship with the app (development: the repo's node_modules).
fn bundled(rel: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates = [
        exe_dir.join("../Resources").join(rel),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(rel),
    ];
    candidates.into_iter().find(|p| p.exists()).and_then(|p| p.canonicalize().ok())
}

fn command(lang: &str, root: &Path) -> Result<(String, Vec<String>, Vec<(&'static str, &'static str)>), String> {
    Ok(match lang {
        // GOTOOLCHAIN=local stops a PR's go.mod from making us download and run a different Go toolchain.
        "go" => ("gopls".into(), vec!["serve".into()], vec![("GOTOOLCHAIN", "local")]),
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
            let js = bundled("node_modules/pyright/langserver.index.js").ok_or("pyright isn't bundled with this build")?;
            ("node".into(), vec![js.to_string_lossy().into(), "--stdio".into()], vec![])
        }
        "starlark" => ("starpls".into(), vec!["server".into()], vec![]),
        "shell" => {
            let js = bundled("node_modules/bash-language-server/out/cli.js").ok_or("bash-language-server isn't bundled with this build")?;
            ("node".into(), vec![js.to_string_lossy().into(), "start".into()], vec![])
        }
        other => return Err(format!("unsupported language: {other}")),
    })
}

/// clangd: guessed flags for files without compile commands (repo root and usual include dirs).
fn clangd_fallback_flags(root: &Path) -> Vec<String> {
    let mut flags = vec!["-std=c++20".to_string()];
    for dir in ["", "include", "src", "lib", "third_party", "external"] {
        let p = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
        if p.is_dir() {
            flags.push(format!("-I{}", p.display()));
        }
    }
    flags
}

pub fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Byte column in `line` → UTF-16 column (what LSP positions use).
pub fn utf16_column(line: &str, byte_col: usize) -> u32 {
    line.get(..byte_col.min(line.len())).unwrap_or(line).encode_utf16().count() as u32
}

/// A location in a file, 0-based line and UTF-16 column.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: String,
    pub line: u32,
    pub character: u32,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct LspClient {
    pub lang: &'static str,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicI64,
    opened: Mutex<HashSet<String>>,
    child: Mutex<Option<Child>>,
}

async fn write_message(stdin: &Mutex<ChildStdin>, msg: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    let mut w = stdin.lock().await;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await.map_err(|e| e.to_string())?;
    w.write_all(&body).await.map_err(|e| e.to_string())?;
    w.flush().await.map_err(|e| e.to_string())
}

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

impl LspClient {
    /// Starts the server for `lang` in `root` and runs the initialize handshake.
    pub async fn start(lang: &'static str, root: String) -> Result<Arc<Self>, String> {
        let root_path = PathBuf::from(&root);
        let (cmd, args, env) = command(lang, &root_path)?;
        let resolved = which(&cmd).ok_or_else(|| format!("not installed: {cmd}. To install: {}", install_hint(lang)))?;
        let mut child = Command::new(resolved)
            .args(&args)
            .envs(env)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("{cmd}: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        // Reader: route responses to waiting requests; answer the server's own requests.
        let (pending_r, stdin_r) = (pending.clone(), stdin.clone());
        tokio::spawn(async move {
            while let Some(msg) = read_message(&mut stdout).await {
                let id = msg.get("id").cloned();
                if msg.get("method").is_some() {
                    // A request from the server (has an id) needs a reply; notifications don't.
                    if let Some(id) = id {
                        let method = msg["method"].as_str().unwrap_or("");
                        let result = match method {
                            // One null per requested configuration section.
                            "workspace/configuration" => {
                                Value::Array(msg["params"]["items"].as_array().map(|a| a.iter().map(|_| Value::Null).collect()).unwrap_or_default())
                            }
                            _ => Value::Null,
                        };
                        let _ = write_message(&stdin_r, &json!({ "jsonrpc": "2.0", "id": id, "result": result })).await;
                    }
                } else if let Some(id) = id.and_then(|i| i.as_i64()) {
                    if let Some(tx) = pending_r.lock().await.remove(&id) {
                        let result = match msg.get("error") {
                            Some(e) => Err(e["message"].as_str().unwrap_or("request failed").to_string()),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(result);
                    }
                }
            }
            // Server exited: fail anything still waiting.
            for (_, tx) in pending_r.lock().await.drain() {
                let _ = tx.send(Err("language server exited".into()));
            }
        });

        let client = Arc::new(Self {
            lang,
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            opened: Mutex::new(HashSet::new()),
            child: Mutex::new(Some(child)),
        });
        let init_options = match lang {
            "cpp" => json!({ "fallbackFlags": clangd_fallback_flags(&root_path) }),
            "go" => json!({ "semanticTokens": true }),
            _ => Value::Null,
        };
        let uri = file_uri(&root);
        client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": uri,
                    "workspaceFolders": [{ "uri": uri, "name": "pr" }],
                    "initializationOptions": init_options,
                    "capabilities": {
                        "textDocument": {
                            "hover": { "contentFormat": ["markdown", "plaintext"] },
                            "definition": { "linkSupport": true },
                            "references": {},
                            "synchronization": { "didSave": false }
                        },
                        "workspace": { "configuration": true, "workspaceFolders": true }
                    }
                }),
            )
            .await?;
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        write_message(&self.stdin, &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await?;
        rx.await.map_err(|_| "language server exited".to_string())?
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        write_message(&self.stdin, &json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    /// Opens a document on the server once (its text from disk), so requests about it work.
    pub async fn ensure_open(&self, path: &str, language: &str) -> Result<(), String> {
        if !self.opened.lock().await.insert(path.to_string()) {
            return Ok(());
        }
        let text = tokio::fs::read_to_string(path).await.map_err(|e| format!("{path}: {e}"))?;
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": file_uri(path), "languageId": language_id(language), "version": 1, "text": text } }),
        )
        .await
    }

    /// Hover contents as Markdown, if any.
    pub async fn hover(&self, path: &str, language: &str, line: u32, character: u32) -> Result<Option<String>, String> {
        self.ensure_open(path, language).await?;
        let result = self
            .request("textDocument/hover", json!({ "textDocument": { "uri": file_uri(path) }, "position": { "line": line, "character": character } }))
            .await?;
        Ok(hover_markdown(&result["contents"]).filter(|s| !s.trim().is_empty()))
    }

    /// Definition locations (several for overloads).
    pub async fn definition(&self, path: &str, language: &str, line: u32, character: u32) -> Result<Vec<Location>, String> {
        self.ensure_open(path, language).await?;
        let result = self
            .request("textDocument/definition", json!({ "textDocument": { "uri": file_uri(path) }, "position": { "line": line, "character": character } }))
            .await?;
        Ok(locations(&result))
    }

    /// All references, including the declaration.
    pub async fn references(&self, path: &str, language: &str, line: u32, character: u32) -> Result<Vec<Location>, String> {
        self.ensure_open(path, language).await?;
        let result = self
            .request(
                "textDocument/references",
                json!({ "textDocument": { "uri": file_uri(path) }, "position": { "line": line, "character": character }, "context": { "includeDeclaration": true } }),
            )
            .await?;
        Ok(locations(&result))
    }

    pub async fn stop(&self) {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), self.request("shutdown", Value::Null)).await;
        let _ = self.notify("exit", Value::Null).await;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

/// MarkupContent | MarkedString | MarkedString[] → Markdown.
fn hover_markdown(contents: &Value) -> Option<String> {
    match contents {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            if let Some(v) = o.get("value").and_then(|v| v.as_str()) {
                // MarkupContent, or MarkedString { language, value }
                match o.get("language").and_then(|l| l.as_str()) {
                    Some(lang) => Some(format!("```{lang}\n{v}\n```")),
                    None => Some(v.to_string()),
                }
            } else {
                None
            }
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(hover_markdown).collect();
            (!parts.is_empty()).then(|| parts.join("\n\n---\n\n"))
        }
        _ => None,
    }
}

/// Location | Location[] | LocationLink[] → Locations.
fn locations(result: &Value) -> Vec<Location> {
    let one = |v: &Value| -> Option<Location> {
        let uri = v.get("uri").or_else(|| v.get("targetUri"))?.as_str()?;
        let range = v.get("range").or_else(|| v.get("targetSelectionRange")).or_else(|| v.get("targetRange"))?;
        Some(Location {
            path: uri_to_path(uri)?,
            line: range["start"]["line"].as_u64()? as u32,
            character: range["start"]["character"].as_u64()? as u32,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs gopls on PATH: cargo test -- --ignored gopls
    #[tokio::test]
    #[ignore]
    async fn gopls_hover_and_definition() {
        crate::backend::exec::augment_path();
        let dir = std::env::temp_dir().join(format!("rt-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/t\n\ngo 1.21\n").unwrap();
        let src = "package t\n\n// Add sums two ints.\nfunc Add(a, b int) int { return a + b }\n\nvar x = Add(1, 2)\n";
        let file = dir.join("t.go");
        std::fs::write(&file, src).unwrap();
        let root = dir.canonicalize().unwrap().to_string_lossy().to_string();
        let path = format!("{root}/t.go");
        let client = LspClient::start("go", root).await.unwrap();
        // "Add" in `var x = Add(1, 2)`: line 5, column 8.
        let hover = client.hover(&path, "go", 5, 9).await.unwrap().unwrap_or_default();
        assert!(hover.contains("func Add(a int, b int) int"), "{hover}");
        assert!(hover.contains("Add sums two ints."), "{hover}");
        let defs = client.definition(&path, "go", 5, 9).await.unwrap();
        assert_eq!((defs[0].path.as_str(), defs[0].line), (path.as_str(), 3));
        let refs = client.references(&path, "go", 5, 9).await.unwrap();
        assert_eq!(refs.len(), 2, "{refs:?}");
        client.stop().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
