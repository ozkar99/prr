//! Everything prr asks GitHub goes through the GitHub CLI, run exactly as you
//! would in a terminal: through your login shell (so a company wrapper
//! function or alias around gh applies), after `cd`-ing into a clone of the
//! PR's repo (so whatever sets gh's environment there, such as direnv, mise
//! or per-repo company settings, applies too). prr holds no token itself.
use crate::backend::exec::{data_dir, which};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub const HOW_TO_FIX: &str = "brew install gh && gh auth login";

tokio::task_local! {
    /// The repo ("owner/repo") the current GitHub call is about.
    static REPO: String;
}

/// Runs `fut` with its GitHub calls made from a clone of `owner/repo`.
pub async fn in_repo<F: Future>(owner: &str, repo: &str, fut: F) -> F::Output {
    REPO.scope(format!("{owner}/{repo}").to_lowercase(), fut).await
}

// ---- Which clone gh runs in ----

fn clones_path() -> std::path::PathBuf {
    data_dir().join("clones.json")
}

/// Clones prr has reviewed PRs in, by "owner/repo", plus "" for the latest.
fn clones() -> BTreeMap<String, String> {
    std::fs::read(clones_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Remembers that `path` is a clone of `owner/repo` (prr was run inside it).
pub fn remember_clone(owner: &str, repo: &str, path: &str) {
    let mut all = clones();
    all.insert(format!("{owner}/{repo}").to_lowercase(), path.to_string());
    all.insert(String::new(), path.to_string());
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(clones_path(), serde_json::to_vec_pretty(&all).unwrap_or_default());
}

/// The clone prr remembers for `owner/repo` (prr was run inside it), if it still exists.
pub fn clone_for(owner: &str, repo: &str) -> Option<String> {
    clones().get(&format!("{owner}/{repo}").to_lowercase()).cloned().filter(|p| std::path::Path::new(p).join(".git").exists())
}

/// The GitHub repo prr remembers for a clone (from a PR opened in it).
pub fn repo_of_clone(path: &str) -> Option<(String, String)> {
    clones().into_iter().find(|(k, v)| !k.is_empty() && v == path).and_then(|(k, _)| k.split_once('/').map(|(o, r)| (o.to_string(), r.to_string())))
}

/// Set by the `prr` command: gh runs in the clone it was started in.
static OVERRIDE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn run_in(folder: &str) {
    *OVERRIDE.lock().unwrap() = Some(folder.to_string());
}

/// The folder gh runs in: the clone of the repo the call is about, else the
/// latest clone prr used, else none (home).
fn folder() -> Option<String> {
    if let Some(f) = OVERRIDE.lock().unwrap().clone() {
        return Some(f);
    }
    let all = clones();
    let repo = REPO.try_with(|r| r.clone()).ok();
    repo.and_then(|r| all.get(&r).cloned())
        .or_else(|| all.get("").cloned())
        .filter(|f| std::path::Path::new(f).is_dir())
}

// ---- Running gh ----

fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| which(s).is_some()).unwrap_or_else(|| "/bin/zsh".into())
}

/// `gh <args>` as typed in a terminal: an interactive login shell that first
/// `cd`s into $PRR_GH_DIR (running its cd hooks, as a terminal would).
fn invocation(args: &[&str]) -> (String, Vec<String>) {
    let shell = user_shell();
    let fish = shell.ends_with("fish");
    let script = if fish {
        "test -n \"$PRR_GH_DIR\"; and cd $PRR_GH_DIR; gh $argv".to_string()
    } else {
        "[ -n \"$PRR_GH_DIR\" ] && cd -- \"$PRR_GH_DIR\"; gh \"$@\"".to_string()
    };
    let mut full = vec!["-l".into(), "-i".into(), "-c".into(), script];
    if !fish {
        full.push("gh".into()); // $0
    }
    full.extend(args.iter().map(|a| a.to_string()));
    (shell, full)
}

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs gh with `args` (and `input` on stdin) from the repo's clone.
pub async fn run(args: &[&str], input: Option<&[u8]>) -> Result<Output, String> {
    let folder = folder();
    let home = std::env::var("HOME").unwrap_or("/".into());
    // Fast path: gh as a program, with the environment your shell sets up in
    // this folder (captured once, see `session`).
    let (program, args_vec, cwd, env): (String, Vec<String>, String, Option<Vec<(String, String)>>) = match session(folder.as_deref()).await {
        Some(sess) => (sess.gh.clone(), args.iter().map(|a| a.to_string()).collect(), folder.clone().unwrap_or(home.clone()), Some(sess.env.clone())),
        None => {
            let (shell, full) = invocation(args);
            (shell, full, home.clone(), None)
        }
    };
    let mut command = tokio::process::Command::new(&program);
    command.args(&args_vec).current_dir(&cwd);
    match &env {
        Some(env) => {
            command.env_clear().envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        // The shell starts at home and cds there itself, so its cd hooks run.
        None => match &folder {
            Some(f) => {
                command.env("PRR_GH_DIR", f);
            }
            None => {
                command.env_remove("PRR_GH_DIR");
            }
        },
    }
    command.env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    let mut child = command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("couldn't run gh ({program}): {e}"))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(bytes).await.map_err(|e| e.to_string())?;
    }
    let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| "gh didn't answer within 60 seconds".to_string())?
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.code() == Some(127) || (!out.status.success() && stderr.contains("command not found")) {
        return Err(format!("gh isn't installed. Run: {HOW_TO_FIX}"));
    }
    Ok(Output { ok: out.status.success(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr })
}

/// What your shell sets up for gh in one folder: the gh program it finds and
/// the environment. Starting the shell (and its startup files, a company
/// wrapper's setup…) for every gh call is slow, so it's done once per folder
/// and reused for 10 minutes (in case that setup hands out short-lived tokens).
#[derive(Clone)]
struct Session {
    gh: String,
    env: Vec<(String, String)>,
    at: std::time::Instant,
}

static SESSIONS: tokio::sync::Mutex<BTreeMap<String, Session>> = tokio::sync::Mutex::const_new(BTreeMap::new());

/// The session for `folder`, or None to run gh through the shell each time
/// (gh is a shell function or alias there, or capturing failed).
async fn session(folder: Option<&str>) -> Option<Session> {
    let key = folder.unwrap_or("").to_string();
    let mut sessions = SESSIONS.lock().await;
    if let Some(s) = sessions.get(&key).filter(|s| s.at.elapsed() < Duration::from_secs(600)) {
        return (!s.gh.is_empty()).then(|| s.clone());
    }
    let captured = capture(folder).await.unwrap_or(Session { gh: String::new(), env: Vec::new(), at: std::time::Instant::now() });
    sessions.insert(key, captured.clone());
    (!captured.gh.is_empty()).then_some(captured)
}

const MARK: &str = "\u{1}PRR_SESSION\u{1}";

/// Runs your shell once, as a terminal would (login, interactive, cd'd into
/// the folder), and reads where `gh` resolves and the environment.
async fn capture(folder: Option<&str>) -> Option<Session> {
    let shell = user_shell();
    let fish = shell.ends_with("fish");
    let script = if fish {
        "test -n \"$PRR_GH_DIR\"; and cd $PRR_GH_DIR; printf '\\001PRR_SESSION\\001'; command -v gh; printf '\\001PRR_SESSION\\001'; env -0".to_string()
    } else {
        "[ -n \"$PRR_GH_DIR\" ] && cd -- \"$PRR_GH_DIR\"; printf '\\001PRR_SESSION\\001'; command -v gh; printf '\\001PRR_SESSION\\001'; env -0".to_string()
    };
    let home = std::env::var("HOME").unwrap_or("/".into());
    let mut command = tokio::process::Command::new(&shell);
    command.args(["-l", "-i", "-c", &script]).current_dir(&home).stdin(Stdio::null()).kill_on_drop(true);
    match folder {
        Some(f) => command.env("PRR_GH_DIR", f),
        None => command.env_remove("PRR_GH_DIR"),
    };
    let out = tokio::time::timeout(Duration::from_secs(30), command.output()).await.ok()?.ok()?;
    parse_session(&String::from_utf8_lossy(&out.stdout))
}

fn parse_session(stdout: &str) -> Option<Session> {
    let mut parts = stdout.split(MARK);
    parts.next()?; // whatever the startup files printed
    let gh = parts.next()?.trim().to_string();
    // Only a program: a shell function or alias has to run through the shell.
    if !gh.starts_with('/') {
        return Some(Session { gh: String::new(), env: Vec::new(), at: std::time::Instant::now() });
    }
    let env: Vec<(String, String)> = parts.next()?.split('\0').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if env.is_empty() {
        return None;
    }
    Some(Session { gh, env, at: std::time::Instant::now() })
}

/// `gh api`: a REST path ("repos/o/r/pulls/1") or "graphql", with an
/// optional JSON body (method POST when there is one). The parsed JSON reply.
pub async fn api(path: &str, body: Option<&Value>) -> Result<Value, String> {
    let path = path.trim_start_matches('/');
    let body_bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let mut args = vec!["api", path];
    if body.is_some() {
        args.extend(["--method", "POST", "--input", "-"]);
    }
    let out = run(&args, body_bytes.as_deref()).await?;
    let json = json_in(&out.stdout);
    if !out.ok {
        // GraphQL errors come back as JSON.
        if let Some(errors) = json.as_ref().and_then(|v| v["errors"].as_array()).filter(|e| !e.is_empty()) {
            let msgs: Vec<&str> = errors.iter().filter_map(|e| e["message"].as_str()).collect();
            return Err(format!("GitHub: {}", msgs.join("; ")));
        }
        // gh prints the reply's message, or its own error, on stderr ("gh: Not Found (HTTP 404)").
        let message = json.as_ref().and_then(|v| v["message"].as_str().map(String::from)).unwrap_or_default();
        let err = out.stderr.lines().rev().find(|l| l.trim().starts_with("gh:") || l.contains("HTTP")).unwrap_or(out.stderr.trim()).trim().to_string();
        return Err(match (message.is_empty(), err.is_empty()) {
            (false, _) if !err.contains(&message) => format!("GitHub: {message} ({err})"),
            (_, false) => format!("GitHub: {}", err.trim_start_matches("gh: ")),
            _ => "gh api failed".into(),
        });
    }
    json.ok_or_else(|| format!("gh api {path}: unexpected reply: {}", out.stdout.chars().take(200).collect::<String>()))
}

/// `gh api --paginate` for a REST list: every page's items, in one gh call.
pub async fn api_all(path: &str) -> Result<Vec<Value>, String> {
    let path = path.trim_start_matches('/');
    let out = run(&["api", "--paginate", path], None).await?;
    if !out.ok {
        return Err(format!("GitHub: {}", out.stderr.trim().lines().last().unwrap_or("gh api failed").trim_start_matches("gh: ")));
    }
    // gh prints each page's array one after another: [..][..]
    let text = out.stdout;
    let start = text.find('[').ok_or_else(|| format!("gh api {path}: unexpected reply"))?;
    let mut items = Vec::new();
    for page in serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>() {
        match page.map_err(|e| e.to_string())? {
            Value::Array(page) => items.extend(page),
            other => items.push(other),
        }
    }
    Ok(items)
}

/// The JSON in gh's output, skipping anything shell startup files printed first.
fn json_in(text: &str) -> Option<Value> {
    let start = text.find(['{', '['])?;
    serde_json::from_str(text[start..].trim()).ok()
}

/// Whether gh runs at all (for the setup check).
pub async fn check() -> Result<(), String> {
    let out = run(&["--version"], None).await?;
    if out.ok { Ok(()) } else { Err(out.stderr.trim().lines().last().unwrap_or("gh failed").to_string()) }
}

/// Git config passed through the environment for downloads from github.com
/// over HTTPS (prr's own clones, or a PR its clone's remote doesn't have):
/// after your own git credentials, git can sign in through gh, as
/// `gh auth setup-git` would set up.
pub fn git_env() -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "credential.https://github.com.helper".into()),
        ("GIT_CONFIG_VALUE_0".into(), "!gh auth git-credential".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_json_after_shell_noise() {
        assert_eq!(json_in("Welcome!\n{\"login\":\"me\"}\n").unwrap()["login"], "me");
        assert!(json_in("[1,2]").unwrap().is_array());
        assert!(json_in("nothing").is_none());
    }

    #[test]
    fn reads_paginated_pages() {
        let text = "[{\"a\":1},{\"a\":2}]\n[{\"a\":3}]";
        let items: Vec<Value> = serde_json::Deserializer::from_str(text).into_iter::<Value>().flat_map(|p| p.unwrap().as_array().cloned().unwrap()).collect();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn reads_a_shell_session() {
        let s = parse_session(&format!("Welcome!\n{MARK}/opt/corp/bin/gh\n{MARK}PATH=/opt/corp/bin:/usr/bin\0GH_ENTERPRISE_TOKEN=x\0")).unwrap();
        assert_eq!(s.gh, "/opt/corp/bin/gh");
        assert!(s.env.contains(&("GH_ENTERPRISE_TOKEN".into(), "x".into())));
        let f = parse_session(&format!("{MARK}gh\n{MARK}PATH=/usr/bin\0")).unwrap();
        assert!(f.gh.is_empty(), "a function or alias runs through the shell");
    }

    #[test]
    fn runs_gh_through_the_shell() {
        let (_, args) = invocation(&["api", "user"]);
        assert!(args.contains(&"-i".to_string()) && args.ends_with(&["api".to_string(), "user".to_string()]));
    }
}
