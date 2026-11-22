//! Disk cleanup for app-managed checkouts. The user's own clones are never
//! touched. On startup: remove PR worktrees not opened for 14 days, and the
//! Chromium caches left behind by the old Electron version of the app.
use crate::exec::{data_dir, run, worktrees_dir};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const UNUSED_AFTER: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// Files the Electron version (same data folder) left behind; WebKit keeps its data elsewhere.
const ELECTRON_LEFTOVERS: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GraphiteDawnCache",
    "GPUPersistentCache",
    "blob_storage",
    "Local Storage",
    "Session Storage",
    "Shared Dictionary",
    "DIPS",
    "Trust Tokens",
    "Trust Tokens-journal",
    "Cookies",
    "Cookies-journal",
    "Extension State",
    "Network Persistent State",
    "TransportSecurity",
    "Local State",
    "Preferences",
    "DevToolsActivePort",
    "declarative_performance_observer.db",
    "declarative_performance_observer.db-journal",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsage {
    pub worktrees: usize,
    pub worktree_bytes: u64,
    pub repos: usize,
    pub repo_bytes: u64,
}

fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path).map(|entries| entries.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Directories exactly `depth` levels below `root` (worktrees: owner/repo/pr-N; repos: owner/name.git).
fn dirs_at_depth(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        level = level
            .iter()
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
    }
    level
}

fn repos_dir() -> PathBuf {
    data_dir().join("repos")
}

pub fn usage() -> DiskUsage {
    let worktrees = dirs_at_depth(&worktrees_dir(), 3);
    let repos = dirs_at_depth(&repos_dir(), 2);
    DiskUsage {
        worktrees: worktrees.len(),
        worktree_bytes: worktrees.iter().map(|p| dir_size(p)).sum(),
        repos: repos.len(),
        repo_bytes: repos.iter().map(|p| dir_size(p)).sum(),
    }
}

/// Removes a worktree properly (so its bare repo forgets it), then any empty parent folders.
async fn remove_worktree(path: &Path) {
    // worktrees/<owner>/<repo>/pr-N -> repos/<owner>/<repo>.git
    let bare = path.parent().and_then(|repo_dir| {
        let owner = repo_dir.parent()?.file_name()?;
        Some(repos_dir().join(owner).join(format!("{}.git", repo_dir.file_name()?.to_string_lossy())))
    });
    let path_s = path.to_string_lossy();
    let removed = match &bare {
        Some(b) if b.exists() => run("git", &["worktree", "remove", "--force", &path_s], Some(b), &[]).await.is_ok(),
        _ => false,
    };
    if !removed {
        let _ = tokio::fs::remove_dir_all(path).await;
        if let Some(b) = &bare {
            let _ = run("git", &["worktree", "prune"], Some(b), &[]).await;
        }
    }
    // Drop now-empty <repo> and <owner> folders.
    for dir in path.ancestors().skip(1).take(2) {
        let _ = std::fs::remove_dir(dir); // only succeeds when empty
    }
}

/// Removes worktrees not opened for 14 days (`except` is the one currently open, if any).
pub async fn remove_unused_worktrees(except: Option<&Path>) -> usize {
    let mut removed = 0;
    for wt in dirs_at_depth(&worktrees_dir(), 3) {
        if except.is_some_and(|e| e == wt) {
            continue;
        }
        let opened = std::fs::metadata(&wt).and_then(|m| m.modified()).unwrap_or(SystemTime::now());
        if SystemTime::now().duration_since(opened).unwrap_or_default() > UNUSED_AFTER {
            remove_worktree(&wt).await;
            removed += 1;
        }
    }
    removed
}

/// "Clear checkouts" (worktrees only) or "clear everything" (also the clones).
pub async fn clear(include_clones: bool) {
    for wt in dirs_at_depth(&worktrees_dir(), 3) {
        remove_worktree(&wt).await;
    }
    if include_clones {
        let _ = tokio::fs::remove_dir_all(repos_dir()).await;
        let _ = tokio::fs::remove_dir_all(worktrees_dir()).await;
    }
}

pub fn remove_electron_leftovers() {
    let dir = data_dir();
    for name in ELECTRON_LEFTOVERS {
        let path = dir.join(name);
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Startup housekeeping, in the background.
pub async fn on_startup() {
    remove_electron_leftovers();
    remove_unused_worktrees(None).await;
}
