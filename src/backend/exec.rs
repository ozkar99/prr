use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Apps launched from Finder don't inherit the shell PATH, so add the usual
/// locations for gh, git, go, gopls, clangd, node and starpls.
pub fn augment_path() {
    let home = std::env::var("HOME").unwrap_or_default();
    let extra = [
        format!("{home}/go/bin"),
        format!("{home}/.cargo/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
    ];
    let mut parts: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for p in extra {
        if !parts.contains(&p) {
            parts.push(p);
        }
    }
    // SAFETY: called once at startup, before any other threads exist.
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

/// Absolute path of an executable on PATH.
pub fn which(cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(|dir| Path::new(dir).join(cmd))
        .find(|candidate| candidate.is_file())
}

/// Runs a command and returns its stdout; the error carries stderr.
pub async fn run(cmd: &str, args: &[&str], cwd: Option<&Path>, env: &[(&str, &str)]) -> Result<String, String> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().await.map_err(|e| format!("{cmd} {}: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!("{cmd} {} failed: {}", args.join(" "), stderr.trim()))
    }
}

/// ~/Library/Application Support/review-tool (same place as the Electron version, so clones are shared).
pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/review-tool")
}

pub fn worktrees_dir() -> PathBuf {
    data_dir().join("worktrees")
}

/// Worktrees made from the user's own clones (copy-on-write), one per open PR window.
pub fn clone_worktrees_dir() -> PathBuf {
    data_dir().join("clone-worktrees")
}
