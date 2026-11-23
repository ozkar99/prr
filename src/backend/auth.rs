//! Where the GitHub token comes from. prr has no login of its own; it uses the
//! first token it finds that GitHub hasn't rejected:
//!
//! 1. one saved in Settings (kept in the macOS Keychain), only if you saved one,
//! 2. git's own credentials for https://github.com (`git credential fill`),
//!    e.g. the token osxkeychain or Git Credential Manager saved when you last
//!    pushed over HTTPS. It's already approved for your organizations' single
//!    sign-on, so it's the first thing tried,
//! 3. the `GH_TOKEN` or `GITHUB_TOKEN` environment variable,
//! 4. the GitHub CLI's login (`gh auth token`), when gh is installed.
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const KEYCHAIN_SERVICE: &str = "prr GitHub token";
const KEYCHAIN_ACCOUNT: &str = "github.com";

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Settings,
    Env(&'static str),
    Gh,
    GitCredential,
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Settings => "the token saved in Settings".into(),
            Source::Env(var) => format!("${var}"),
            Source::Gh => "the GitHub CLI's login".into(),
            Source::GitCredential => "git's credentials for github.com".into(),
        }
    }
}

#[derive(Clone)]
pub struct Token {
    pub value: String,
    pub source: Source,
}

static CACHE: Mutex<Option<Token>> = Mutex::const_new(None);
/// The same token, for code that can't wait (git commands run synchronously).
static CURRENT: std::sync::Mutex<Option<Token>> = std::sync::Mutex::new(None);
/// Sources whose token GitHub rejected; skipped until the saved token changes.
static REJECTED: std::sync::Mutex<Vec<Source>> = std::sync::Mutex::new(Vec::new());

/// What to tell someone with no token.
pub const HOW_TO_FIX: &str = "Settings (⌘,) → GitHub token, or export GH_TOKEN";

/// The token to use, looked up once and then remembered (until [`forget`]).
pub async fn token() -> Result<Token, String> {
    let mut cache = CACHE.lock().await;
    if let Some(t) = cache.as_ref() {
        return Ok(t.clone());
    }
    let found = find().await.ok_or_else(|| format!("No GitHub token found. Add one in {HOW_TO_FIX}."))?;
    *cache = Some(found.clone());
    *CURRENT.lock().unwrap() = Some(found.clone());
    Ok(found)
}

/// Looks again next time, trying every source (after the saved token changes).
pub async fn forget() {
    REJECTED.lock().unwrap().clear();
    *CACHE.lock().await = None;
    *CURRENT.lock().unwrap() = None;
}

/// GitHub rejected this source's token: use the next source from now on.
pub async fn reject(source: &Source) {
    let mut cache = CACHE.lock().await;
    REJECTED.lock().unwrap().push(source.clone());
    *cache = None;
    *CURRENT.lock().unwrap() = None;
}

fn usable(source: &Source) -> bool {
    !REJECTED.lock().unwrap().contains(source)
}

async fn find() -> Option<Token> {
    let non_empty = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    if usable(&Source::Settings)
        && let Some(value) = saved().and_then(non_empty)
    {
        return Some(Token { value, source: Source::Settings });
    }
    if usable(&Source::GitCredential)
        && let Some(value) = git_credential().await.and_then(non_empty)
    {
        return Some(Token { value, source: Source::GitCredential });
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if usable(&Source::Env(var))
            && let Some(value) = std::env::var(var).ok().and_then(non_empty)
        {
            return Some(Token { value, source: Source::Env(var) });
        }
    }
    if usable(&Source::Gh)
        && crate::backend::exec::which("gh").is_some()
        && let Some(value) = crate::backend::exec::run("gh", &["auth", "token"], None, &[]).await.ok().and_then(non_empty)
    {
        return Some(Token { value, source: Source::Gh });
    }
    None
}

/// Asks git's credential helpers for github.com, without ever prompting.
async fn git_credential() -> Option<String> {
    let mut child = tokio::process::Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    stdin.write_all(b"protocol=https\nhost=github.com\n\n").await.ok()?;
    drop(stdin);
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_credential(&String::from_utf8_lossy(&out.stdout))
}

fn parse_credential(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("password=")).map(str::to_string)
}

// ---- The token saved in Settings (macOS Keychain) ----

pub fn saved() -> Option<String> {
    security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

pub fn save(token: &str) -> Result<(), String> {
    security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, token.trim().as_bytes()).map_err(|e| e.to_string())
}

pub fn remove() {
    let _ = security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT);
}

/// Git config passed through the environment (not the command line, where
/// other processes could read it) so git reaches github.com over HTTPS with
/// the token, even without credentials of its own. Every git command gets it:
/// prr's clones are partial, so even `git show` may download.
pub async fn git_env() -> Vec<(String, String)> {
    let _ = token().await;
    git_env_now()
}

/// [`git_env`] with the token already looked up (empty before that).
pub fn git_env_now() -> Vec<(String, String)> {
    let Some(t) = CURRENT.lock().unwrap().clone() else { return Vec::new() };
    // Git credentials already work for git; don't override them.
    if t.source == Source::GitCredential {
        return Vec::new();
    }
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
    fn reads_the_password_line() {
        assert_eq!(parse_credential("protocol=https\nhost=github.com\nusername=me\npassword=gho_abc\n").as_deref(), Some("gho_abc"));
        assert_eq!(parse_credential("protocol=https\nhost=github.com\n"), None);
    }

    #[tokio::test]
    async fn asks_git_credential_helpers() {
        let dir = std::env::temp_dir().join(format!("prr-cred-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("gitconfig");
        std::fs::write(&config, "[credential]\n\thelper = \"!f() { echo username=me; echo password=fake_token; }; f\"\n").unwrap();
        // SAFETY: the only test that touches these variables.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", &config);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        assert_eq!(git_credential().await.as_deref(), Some("fake_token"));
        std::fs::write(&config, "").unwrap();
        assert_eq!(git_credential().await, None, "no helper: nothing, and no prompt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
