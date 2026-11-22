//! One shared bare clone per repo, plus one worktree per PR checked out at the
//! PR head. Re-opening a PR reuses its worktree, so caches survive between sessions.
use crate::exec::{data_dir, run, worktrees_dir};
use crate::types::{Checkout, PRDetails};
use std::path::{Path, PathBuf};

fn bare_repo_path(owner: &str, repo: &str) -> PathBuf {
    data_dir().join("repos").join(owner).join(format!("{repo}.git"))
}

async fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    run("git", args, cwd, &[("GIT_TERMINAL_PROMPT", "0")]).await
}

async fn remote_url(owner: &str, repo: &str) -> String {
    let protocol = run("gh", &["config", "get", "git_protocol"], None, &[]).await.unwrap_or_default();
    if protocol.trim() == "ssh" {
        format!("git@github.com:{owner}/{repo}.git")
    } else {
        format!("https://github.com/{owner}/{repo}.git")
    }
}

pub async fn checkout(pr: &PRDetails, progress: impl Fn(String)) -> Result<Checkout, String> {
    let bare = bare_repo_path(&pr.owner, &pr.repo);
    let bare_s = bare.to_string_lossy().to_string();
    if !bare.exists() {
        progress(format!("Cloning {}/{} (first time only)…", pr.owner, pr.repo));
        tokio::fs::create_dir_all(bare.parent().unwrap()).await.map_err(|e| e.to_string())?;
        git(&["clone", "--bare", "--no-tags", &remote_url(&pr.owner, &pr.repo).await, &bare_s], None).await?;
    }

    progress(format!("Fetching PR #{}…", pr.number));
    let pr_ref = format!("+refs/pull/{}/head:refs/review/pr/{}", pr.number, pr.number);
    let base_ref = format!("+refs/heads/{}:refs/review/base/{}", pr.base_ref, pr.base_ref);
    git(&["fetch", "--no-tags", "origin", &pr_ref, &base_ref], Some(&bare)).await?;

    let merge_base = git(&["merge-base", &pr.base_sha, &pr.head_sha], Some(&bare)).await?.trim().to_string();

    let root = worktrees_dir().join(&pr.owner).join(&pr.repo).join(format!("pr-{}", pr.number));
    let root_s = root.to_string_lossy().to_string();
    progress("Checking out PR head…".into());
    if root.join(".git").exists() {
        git(&["checkout", "--detach", "--force", &pr.head_sha], Some(&root)).await?;
    } else {
        git(&["worktree", "prune"], Some(&bare)).await?;
        tokio::fs::create_dir_all(root.parent().unwrap()).await.map_err(|e| e.to_string())?;
        git(&["worktree", "add", "--detach", "--force", &root_s, &pr.head_sha], Some(&bare)).await?;
    }
    Ok(Checkout { root: root_s, merge_base, head_sha: pr.head_sha.clone() })
}

/// File contents at a commit, or None if the file doesn't exist there.
pub async fn read_at_commit(owner: &str, repo: &str, sha: &str, path: &str) -> Result<Option<String>, String> {
    if !regex::Regex::new(r"^[0-9a-f]{7,64}$").unwrap().is_match(sha) {
        return Err(format!("Invalid commit: {sha}"));
    }
    Ok(git(&["show", &format!("{sha}:{path}")], Some(&bare_repo_path(owner, repo))).await.ok())
}
