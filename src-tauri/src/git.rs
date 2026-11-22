//! Getting a PR's code onto disk, two ways:
//!
//! - **The user's own clone** (when `review-tool` runs inside a clone of the
//!   PR's repo): the PR's commits are fetched into it without creating refs,
//!   and it's switched to the PR head only with the user's OK. No extra clone,
//!   and its language-server caches are reused. Big repos stay cheap.
//! - **App-managed**: one shared bare clone per repo plus one worktree per PR.
//!   Worktrees not opened for a while are removed by cleanup.rs.
use crate::exec::{data_dir, run, worktrees_dir};
use crate::types::{Checkout, CheckoutResult, LocalClone, PRDetails};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn bare_repo_path(owner: &str, repo: &str) -> PathBuf {
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

fn valid_sha(sha: &str) -> Result<(), String> {
    if regex::Regex::new(r"^[0-9a-f]{7,64}$").unwrap().is_match(sha) {
        Ok(())
    } else {
        Err(format!("Invalid commit: {sha}"))
    }
}

pub async fn checkout(
    pr: &PRDetails,
    local_repo: Option<&str>,
    allow_switch: bool,
    progress: impl Fn(String),
) -> Result<CheckoutResult, String> {
    match local_repo {
        Some(path) => checkout_local(pr, Path::new(path), allow_switch, progress).await,
        None => checkout_managed(pr, progress).await.map(CheckoutResult::Ready),
    }
}

// ---- The user's own clone ----

/// The remote in `repo` that points at github.com/<owner>/<name>, if any.
async fn matching_remote(repo: &Path, owner: &str, name: &str) -> Option<String> {
    let remotes = git(&["remote", "-v"], Some(repo)).await.ok()?;
    let wanted = format!("{owner}/{name}").to_lowercase();
    remotes.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (remote, url) = (parts.next()?, parts.next()?);
        let path = url.trim_end_matches(".git").to_lowercase();
        (path.contains("github.com") && (path.ends_with(&format!("/{wanted}")) || path.ends_with(&format!(":{wanted}"))))
            .then(|| remote.to_string())
    })
}

async fn checkout_local(pr: &PRDetails, repo: &Path, allow_switch: bool, progress: impl Fn(String)) -> Result<CheckoutResult, String> {
    let top = git(&["rev-parse", "--show-toplevel"], Some(repo)).await.map_err(|_| format!("{} isn't a git clone", repo.display()))?;
    let top = PathBuf::from(top.trim());
    let top_s = top.to_string_lossy().to_string();

    // Fetch the PR head and base into the clone. No destination refs: the
    // commits become available without adding branches or refs to the user's repo.
    progress(format!("Fetching PR #{} into your clone…", pr.number));
    let remote = match matching_remote(&top, &pr.owner, &pr.repo).await {
        Some(r) => r,
        None => remote_url(&pr.owner, &pr.repo).await,
    };
    let pr_ref = format!("refs/pull/{}/head", pr.number);
    let base_ref = format!("refs/heads/{}", pr.base_ref);
    git(&["fetch", "--no-tags", "--no-write-fetch-head", &remote, &pr_ref, &base_ref], Some(&top)).await?;
    let merge_base = git(&["merge-base", &pr.base_sha, &pr.head_sha], Some(&top)).await?.trim().to_string();

    let head = git(&["rev-parse", "HEAD"], Some(&top)).await?.trim().to_string();
    let ready = |switched_from: Option<String>| {
        crate::fsaccess::allow_root(&top);
        CheckoutResult::Ready(Checkout {
            root: top_s.clone(),
            merge_base: merge_base.clone(),
            head_sha: pr.head_sha.clone(),
            local: Some(LocalClone { path: top_s.clone(), switched_from }),
        })
    };
    if head == pr.head_sha {
        return Ok(ready(None));
    }

    // Not at the PR head: ask before moving the user's clone, and never with uncommitted changes.
    let branch = git(&["symbolic-ref", "--short", "-q", "HEAD"], Some(&top)).await.ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"], Some(&top)).await?.trim().is_empty();
    if !allow_switch || dirty {
        return Ok(CheckoutResult::NeedsSwitch {
            path: top_s.clone(),
            current: branch.clone().unwrap_or_else(|| format!("{} (detached)", &head[..head.len().min(8)])),
            dirty,
        });
    }
    progress("Switching your clone to the PR…".into());
    git(&["checkout", "--detach", &pr.head_sha], Some(&top)).await?;
    // What to switch back to when the PR is closed.
    Ok(ready(Some(branch.unwrap_or(head))))
}

/// Puts the user's clone back where it was, if it's still at the PR head with no changes.
pub async fn restore_local(path: &str, switched_from: &str, head_sha: &str) -> Result<bool, String> {
    let repo = Path::new(path);
    let head = git(&["rev-parse", "HEAD"], Some(repo)).await?.trim().to_string();
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"], Some(repo)).await?.trim().is_empty();
    if head != head_sha || dirty {
        return Ok(false); // the user moved on or changed something; leave it alone
    }
    git(&["checkout", switched_from], Some(repo)).await?;
    Ok(true)
}

// ---- App-managed clone + worktrees ----

async fn checkout_managed(pr: &PRDetails, progress: impl Fn(String)) -> Result<Checkout, String> {
    let bare = bare_repo_path(&pr.owner, &pr.repo);
    let bare_s = bare.to_string_lossy().to_string();
    if !bare.exists() {
        progress(format!("Cloning {}/{} (first time only)…", pr.owner, pr.repo));
        tokio::fs::create_dir_all(bare.parent().unwrap()).await.map_err(|e| e.to_string())?;
        // Blobless: history without file contents; contents arrive when checked out.
        // Keeps the first clone of a big repo fast.
        git(&["clone", "--bare", "--no-tags", "--filter=blob:none", &remote_url(&pr.owner, &pr.repo).await, &bare_s], None).await?;
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
    // Last-opened time, for cleanup of unused worktrees.
    if let Ok(dir) = std::fs::File::open(&root) {
        let _ = dir.set_modified(SystemTime::now());
    }
    Ok(Checkout { root: root_s, merge_base, head_sha: pr.head_sha.clone(), local: None })
}

/// File contents at a commit, or None if the file doesn't exist there. `root`
/// is the checkout (worktree or the user's clone).
pub async fn read_at_commit(root: &str, sha: &str, path: &str) -> Result<Option<String>, String> {
    valid_sha(sha)?;
    if !crate::fsaccess::is_checkout_root(Path::new(root)).await {
        return Err(format!("Not a review checkout: {root}"));
    }
    Ok(git(&["show", &format!("{sha}:{path}")], Some(Path::new(root))).await.ok())
}
