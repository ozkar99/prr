//! GitHub access through the GitHub CLI's login: prr has no login of its own.
//! gh can be logged in to several accounts; Settings picks which one prr uses
//! (by default, gh's active account).
use crate::backend::exec::{run, which};
use serde_json::Value;
use tokio::sync::Mutex;

pub const HOW_TO_FIX: &str = "brew install gh && gh auth login";

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
    if let Some(t) = cache.as_ref().filter(|t| t.account == account) {
        return Ok(t.clone());
    }
    if which("gh").is_none() {
        return Err(format!("prr uses the GitHub CLI's login, and gh isn't installed. Run: {HOW_TO_FIX}"));
    }
    let mut args = vec!["auth", "token", "--hostname", "github.com"];
    if let Some(a) = &account {
        args.extend(["--user", a.as_str()]);
    }
    let value = run("gh", &args, None, &[]).await.map(|s| s.trim().to_string()).unwrap_or_default();
    if value.is_empty() {
        return Err(match &account {
            Some(a) => format!("gh isn't logged in to the account @{a} (picked in Settings). Run: gh auth login, or pick another account."),
            None => "gh isn't logged in. Run: gh auth login".into(),
        });
    }
    let found = Token { value, account };
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
    if which("gh").is_none() {
        return Vec::new();
    }
    // `gh auth status` exits non-zero when any account has a problem, but still prints the list.
    let out = tokio::process::Command::new("gh").args(["auth", "status", "--hostname", "github.com", "--json", "hosts"]).output().await;
    let Ok(out) = out else { return Vec::new() };
    parse_accounts(&String::from_utf8_lossy(&out.stdout))
}

fn parse_accounts(json: &str) -> Vec<Account> {
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
    }
}
