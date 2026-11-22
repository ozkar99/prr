//! Which folders count as review checkouts: app-managed worktrees, plus users' own
//! clones opened for review this session. (The web version also sandboxed the
//! editor's file reads here; the native UI reads files in-process.)
use crate::backend::exec::worktrees_dir;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOCAL_ROOTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Records a user's clone that a PR is being reviewed in.
pub fn allow_root(path: &Path) {
    if let Ok(real) = path.canonicalize() {
        let mut roots = LOCAL_ROOTS.lock().unwrap();
        if !roots.contains(&real) {
            roots.push(real);
        }
    }
}

/// Whether `path` is a checkout the app opened: an app-managed worktree or a recorded user clone.
pub async fn is_checkout_root(path: &Path) -> bool {
    let Ok(real) = tokio::fs::canonicalize(path).await else { return false };
    let worktrees = tokio::fs::canonicalize(worktrees_dir()).await.ok();
    worktrees.is_some_and(|w| real.starts_with(w)) || LOCAL_ROOTS.lock().unwrap().contains(&real)
}
