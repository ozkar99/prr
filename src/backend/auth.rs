//! GitHub access through the GitHub CLI's login: prr has no login of its own.
//! gh can be logged in to several accounts; Settings picks which one prr uses
//! (by default, gh's active account).
//!
//! By default prr runs `gh` the way your terminal does: through your login
//! shell, so a wrapper function or alias around gh (that sets a company
//! token, say) is used too. Settings can name another gh command instead.
use crate::backend::exec::which;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::sync::Mutex;

pub const HOW_TO_FIX: &str = "brew install gh && gh auth login";

/// The gh command from Settings (empty: `gh`, as your shell runs it).
static GH: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// The folder gh runs in (a clone of the repo, so its environment applies; empty: home).
static FOLDER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// The account picked in Settings (None: gh's active account).
static ACCOUNT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static CACHE: Mutex<Option<Token>> = Mutex::const_new(None);
/// The same token, for code that can't wait (git commands run synchronously).
static CURRENT: std::sync::Mutex<Option<Token>> = std::sync::Mutex::new(None);

#[derive(Clone)]
pub struct Token {
    pub value: String,
    /// The gh account it belongs to (None: gh's active account).
    pub account: Option<String>,
    /// The gh command that gave it.
    pub program: String,
    /// The folder it ran in.
    pub folder: String,
}

/// Runs gh in another folder from now on: a clone of the repo you review,
/// so anything that sets gh's environment there (direnv, mise, a company
/// wrapper's per-repo settings) applies, as in a terminal `cd`'d into it.
pub fn set_folder(folder: &str) {
    let folder = expand_home(folder.trim());
    let mut current = FOLDER.lock().unwrap();
    if *current != folder {
        *current = folder;
        *CURRENT.lock().unwrap() = None;
    }
}

/// The folder gh runs in, if it exists.
pub fn folder() -> Option<String> {
    let f = FOLDER.lock().unwrap().clone();
    (!f.is_empty() && std::path::Path::new(&f).is_dir()).then_some(f)
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => path.to_string(),
    }
}

/// Runs another gh from now on (from Settings; empty: `gh` through your shell).
pub fn set_gh_command(command: &str) {
    let command = command.trim().to_string();
    let mut current = GH.lock().unwrap();
    if *current != command {
        *current = command;
        *CURRENT.lock().unwrap() = None;
    }
}

/// The gh command: the one from Settings (`~` expanded), or `gh`.
pub fn gh_program() -> String {
    let command = GH.lock().unwrap().clone();
    if command.is_empty() { "gh".into() } else { expand_home(&command) }
}

/// The user's shell (what Terminal runs).
fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| which(s).is_some()).unwrap_or_else(|| "/bin/zsh".into())
}

/// How to run gh with `args`: a path runs as it is; a name (`gh`, or a
/// wrapper's name) runs through an interactive login shell, like typing it
/// in a terminal, so shell functions and aliases work. The shell first `cd`s
/// into $PRR_GH_DIR (when set), which also runs its cd hooks (direnv, mise).
fn invocation(program: &str, args: &[&str]) -> (String, Vec<String>) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    if program.contains('/') {
        return (program.to_string(), args);
    }
    let shell = user_shell();
    let script = if shell.ends_with("fish") {
        format!("test -n \"$PRR_GH_DIR\"; and cd $PRR_GH_DIR; {program} $argv")
    } else {
        format!("[ -n \"$PRR_GH_DIR\" ] && cd -- \"$PRR_GH_DIR\"; {program} \"$@\"")
    };
    let mut full = vec!["-l".into(), "-i".into(), "-c".into(), script];
    if !shell.ends_with("fish") {
        full.push(program.to_string()); // $0
    }
    full.extend(args);
    (shell, full)
}

struct GhOutput {
    ok: bool,
    stdout: String,
    stderr: String,
}

async fn run_gh(args: &[&str]) -> Result<GhOutput, String> {
    let program = gh_program();
    if program.contains('/') && which(&program).is_none() {
        return Err(format!("{program} (the gh command in Settings) doesn't exist"));
    }
    let (cmd, full) = invocation(&program, args);
    let folder = folder();
    let mut command = tokio::process::Command::new(&cmd);
    // A path runs in the folder directly. The shell starts at home and `cd`s
    // there itself, so that its cd hooks run as they would in a terminal.
    let home = std::env::var("HOME").unwrap_or("/".into());
    match &folder {
        Some(f) if program.contains('/') => command.current_dir(f),
        Some(f) => command.current_dir(&home).env("PRR_GH_DIR", f),
        None => command.current_dir(&home).env_remove("PRR_GH_DIR"),
    };
    let child = command
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(30), child)
        .await
        .map_err(|_| format!("{program} didn't answer within 30 seconds"))?
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // 127: the shell didn't find the command.
    if out.status.code() == Some(127) || (stderr.contains("command not found") && !out.status.success()) {
        return Err(if program == "gh" {
            format!("gh isn't installed. Run: {HOW_TO_FIX}")
        } else {
            format!("{program} (the gh command in Settings) wasn't found")
        });
    }
    Ok(GhOutput { ok: out.status.success(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr })
}

/// Whether gh runs at all (for the setup check).
pub async fn check_gh() -> Result<(), String> {
    let out = run_gh(&["--version"]).await?;
    if out.ok { Ok(()) } else { Err(out.stderr.trim().lines().last().unwrap_or("gh failed").to_string()) }
}

/// The last line that looks like a token: shell startup files may print other things first.
fn token_from(stdout: &str) -> Option<String> {
    stdout.lines().rev().map(str::trim).find(|l| !l.is_empty() && !l.contains(char::is_whitespace)).map(str::to_string)
}

/// Uses another gh account from now on (from Settings; empty: gh's active one).
pub fn set_account(account: &str) {
    let account = Some(account.trim().to_string()).filter(|a| !a.is_empty());
    let mut current = ACCOUNT.lock().unwrap();
    if *current != account {
        *current = account;
        *CURRENT.lock().unwrap() = None;
    }
}

/// The token to use, looked up once and then remembered (until [`forget`]).
pub async fn token() -> Result<Token, String> {
    let mut cache = CACHE.lock().await;
    let account = ACCOUNT.lock().unwrap().clone();
    let program = gh_program();
    let folder = folder().unwrap_or_default();
    if let Some(t) = cache.as_ref().filter(|t| t.account == account && t.program == program && t.folder == folder) {
        return Ok(t.clone());
    }
    let mut args = vec!["auth", "token", "--hostname", "github.com"];
    if let Some(a) = &account {
        args.extend(["--user", a.as_str()]);
    }
    let out = run_gh(&args).await?;
    let value = if out.ok { token_from(&out.stdout).unwrap_or_default() } else { String::new() };
    if value.is_empty() {
        return Err(match &account {
            Some(a) => format!("gh isn't logged in to the account @{a} (picked in Settings). Run: gh auth login, or pick another account."),
            None => "gh isn't logged in. Run: gh auth login".into(),
        });
    }
    let found = Token { value, account, program, folder };
    *cache = Some(found.clone());
    *CURRENT.lock().unwrap() = Some(found.clone());
    Ok(found)
}

/// Looks the token up again next time (gh's login may have changed).
pub async fn forget() {
    *CACHE.lock().await = None;
    *CURRENT.lock().unwrap() = None;
}

/// A github.com account gh is logged in to.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub login: String,
    pub active: bool,
    /// False when gh's stored token no longer works.
    pub ok: bool,
}

/// The github.com accounts gh is logged in to (empty if gh is missing or too
/// old to list them).
pub async fn accounts() -> Vec<Account> {
    // `gh auth status` exits non-zero when any account has a problem, but still prints the list.
    let Ok(out) = run_gh(&["auth", "status", "--hostname", "github.com", "--json", "hosts"]).await else { return Vec::new() };
    parse_accounts(&out.stdout)
}

fn parse_accounts(text: &str) -> Vec<Account> {
    // The JSON, skipping anything shell startup files printed before it.
    let json = text.find('{').map(|i| &text[i..]).unwrap_or("");
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    v["hosts"]["github.com"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Account { login: a["login"].as_str()?.to_string(), active: a["active"].as_bool().unwrap_or(false), ok: a["state"].as_str() == Some("success") })
        })
        .collect()
}

/// Git config passed through the environment (not the command line, where
/// other processes could read it) so git reaches github.com over HTTPS as the
/// same account, without credentials of its own. Every git command gets it:
/// prr's clones are partial, so even `git show` may download.
pub async fn git_env() -> Vec<(String, String)> {
    let _ = token().await;
    git_env_now()
}

/// [`git_env`] with the token already looked up (empty before that).
pub fn git_env_now() -> Vec<(String, String)> {
    let Some(t) = CURRENT.lock().unwrap().clone() else { return Vec::new() };
    use base64::Engine as _;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{}", t.value));
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "http.https://github.com/.extraheader".into()),
        ("GIT_CONFIG_VALUE_0".into(), format!("Authorization: Basic {basic}")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gh_accounts() {
        let json = r#"{"hosts":{"github.com":[
            {"state":"success","active":true,"host":"github.com","login":"me","tokenSource":"keyring"},
            {"state":"error","active":false,"host":"github.com","login":"me-work","tokenSource":"keyring"}]}}"#;
        assert_eq!(
            parse_accounts(json),
            vec![Account { login: "me".into(), active: true, ok: true }, Account { login: "me-work".into(), active: false, ok: false }]
        );
        assert!(parse_accounts("unknown flag: --json").is_empty());
        assert_eq!(parse_accounts(&format!("Welcome!\n{json}")).len(), 2, "skips shell noise");
    }

    #[test]
    fn finds_the_token_line() {
        assert_eq!(token_from("Last login: today\ngho_abc123\n").as_deref(), Some("gho_abc123"));
        assert_eq!(token_from("\n"), None);
    }

    #[test]
    fn runs_names_through_the_shell() {
        let (cmd, args) = invocation("/opt/corp/gh", &["auth", "token"]);
        assert_eq!((cmd.as_str(), args.len()), ("/opt/corp/gh", 2));
        let (_, args) = invocation("gh", &["auth", "token"]);
        assert!(args.contains(&"-i".to_string()) && args.ends_with(&["auth".to_string(), "token".to_string()]));
    }
}
