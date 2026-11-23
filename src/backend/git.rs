//! Getting a PR's code onto disk, two ways:
//!
//! - **The user's own clone** (when `prr` runs inside a clone of the
//!   PR's repo): the PR's commits are fetched into it without creating refs,
//!   and it's switched to the PR head only with the user's OK. No extra clone,
//!   and its language-server caches are reused. Big repos stay cheap.
//! - **App-managed**: one shared bare clone per repo plus one worktree per PR.
//!   Worktrees not opened for a while are removed by cleanup.rs.
use crate::backend::exec::{clone_worktrees_dir, data_dir, run, worktrees_dir};
use crate::backend::types::{Checkout, CheckoutResult, LocalClone, PRDetails};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn bare_repo_path(owner: &str, repo: &str) -> PathBuf {
    data_dir().join("repos").join(owner).join(format!("{repo}.git"))
}

async fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    // With the GitHub token, for anything that downloads (see auth::git_env).
    let mut env: Vec<(String, String)> = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
    env.extend(crate::backend::auth::git_env().await);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    run("git", args, cwd, &env).await
}

/// Clones are over HTTPS, authenticated with the same token as the API.
async fn remote_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}.git")
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
    local_worktree: bool,
    allow_switch: bool,
    progress: impl Fn(String),
) -> Result<CheckoutResult, String> {
    match local_repo {
        Some(path) if local_worktree => checkout_clone_worktree(pr, Path::new(path), progress).await.map(CheckoutResult::Ready),
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

/// The clone's top-level directory, with the PR's head and base fetched into it
/// (no destination refs: the commits become available without adding branches
/// or refs to the user's repo). Returns (top, merge base).
async fn fetch_into_clone(pr: &PRDetails, repo: &Path, progress: &impl Fn(String)) -> Result<(PathBuf, String), String> {
    let top = git(&["rev-parse", "--show-toplevel"], Some(repo)).await.map_err(|_| format!("{} isn't a git clone", repo.display()))?;
    let top = PathBuf::from(top.trim());
    progress(format!("Fetching PR #{} into your clone…", pr.number));
    let remote = match matching_remote(&top, &pr.owner, &pr.repo).await {
        Some(r) => r,
        None => remote_url(&pr.owner, &pr.repo).await,
    };
    let pr_ref = format!("refs/pull/{}/head", pr.number);
    let base_ref = format!("refs/heads/{}", pr.base_ref);
    git(&["fetch", "--no-tags", "--no-write-fetch-head", &remote, &pr_ref, &base_ref], Some(&top)).await?;
    let merge_base = git(&["merge-base", &pr.base_sha, &pr.head_sha], Some(&top)).await?.trim().to_string();
    Ok((top, merge_base))
}

async fn checkout_local(pr: &PRDetails, repo: &Path, allow_switch: bool, progress: impl Fn(String)) -> Result<CheckoutResult, String> {
    let (top, merge_base) = fetch_into_clone(pr, repo, &progress).await?;
    let top_s = top.to_string_lossy().to_string();

    let head = git(&["rev-parse", "HEAD"], Some(&top)).await?.trim().to_string();
    let ready = |switched_from: Option<String>| {
        crate::backend::fsaccess::allow_root(&top);
        CheckoutResult::Ready(Checkout {
            root: top_s.clone(),
            merge_base: merge_base.clone(),
            head_sha: pr.head_sha.clone(),
            local: Some(LocalClone { path: top_s.clone(), switched_from }),
            worktree_of: None,
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
    Ok(Checkout { root: root_s, merge_base, head_sha: pr.head_sha.clone(), local: None, worktree_of: None })
}

// ---- Worktrees of the user's clone (copy-on-write) ----

/// A worktree of the user's clone at the PR head, for when the clone itself is
/// busy (another window) or has changes. Objects are shared with the clone, and
/// the files are APFS copy-on-write clones of the clone's checkout (`cp -c`), so
/// a big monorepo costs only the files that differ, not another full checkout.
/// Git then rewrites just the files that differ from the PR head.
async fn checkout_clone_worktree(pr: &PRDetails, repo: &Path, progress: impl Fn(String)) -> Result<Checkout, String> {
    let (top, merge_base) = fetch_into_clone(pr, repo, &progress).await?;
    let top_s = top.to_string_lossy().to_string();
    let root = clone_worktrees_dir().join(&pr.owner).join(&pr.repo).join(format!("pr-{}", pr.number));
    let root_s = root.to_string_lossy().to_string();
    // Trust size + mtime for the copied files (they carry the clone's times), so
    // git doesn't re-read the whole tree; only for these commands, not saved in config.
    let fast = ["-c", "core.checkStat=minimal", "-c", "core.trustctime=false"];

    if !root.join(".git").exists() {
        let _ = tokio::fs::remove_dir_all(&root).await;
        git(&["worktree", "prune"], Some(&top)).await?;
        tokio::fs::create_dir_all(root.parent().unwrap()).await.map_err(|e| e.to_string())?;
        let base_commit = git(&["rev-parse", "HEAD"], Some(&top)).await?.trim().to_string();
        git(&["worktree", "add", "--no-checkout", "--detach", &root_s, &base_commit], Some(&top)).await?;

        progress("Copying your checkout (copy-on-write, no extra disk)…".into());
        let copied = clone_tree(&top, &root).await;
        if copied {
            // The clone's index describes the copied files (same sizes and times).
            let index_of = |dir: PathBuf| async move {
                git(&["rev-parse", "--path-format=absolute", "--git-path", "index"], Some(&dir)).await.map(|p| PathBuf::from(p.trim()))
            };
            if let (Ok(from), Ok(to)) = (index_of(top.clone()).await, index_of(root.clone()).await) {
                let _ = tokio::fs::copy(&from, &to).await;
            }
        }
        if !copied {
            progress("Checking out PR head…".into());
        }
    }
    progress("Updating to the PR head…".into());
    let mut args: Vec<&str> = fast.to_vec();
    args.extend(["reset", "--hard", "--quiet", &pr.head_sha]);
    git(&args, Some(&root)).await?;
    crate::backend::fsaccess::allow_root(&root);
    Ok(Checkout { root: root_s, merge_base, head_sha: pr.head_sha.clone(), local: None, worktree_of: Some(top_s) })
}

/// Copy-on-write copies of everything in the clone's checkout except `.git`.
/// False if the volume can't do it (then `reset --hard` writes the files).
async fn clone_tree(from: &Path, to: &Path) -> bool {
    let Ok(mut entries) = tokio::fs::read_dir(from).await else { return false };
    let mut sources = Vec::new();
    while let Ok(Some(e)) = entries.next_entry().await {
        if e.file_name() != ".git" {
            sources.push(e.path());
        }
    }
    if sources.is_empty() {
        return true;
    }
    let mut cmd = tokio::process::Command::new("/bin/cp");
    // -c: clonefile (APFS copy-on-write); -R recursive, keeping symlinks; -p keeps times.
    cmd.arg("-c").arg("-R").arg("-p").args(&sources).arg(to);
    matches!(cmd.output().await, Ok(o) if o.status.success())
}

/// Removes a worktree made from the user's clone (the window closed). Deleting
/// the folder and pruning avoids `git worktree remove` re-reading a huge tree.
pub async fn remove_clone_worktree(root: &str, clone: &str) {
    let _ = tokio::fs::remove_dir_all(root).await;
    let _ = git(&["worktree", "prune"], Some(Path::new(clone))).await;
}

/// Startup: worktrees of user clones left by a crash (nothing is open yet).
pub async fn remove_stale_clone_worktrees() {
    let base = clone_worktrees_dir();
    let Ok(mut owners) = tokio::fs::read_dir(&base).await else { return };
    while let Ok(Some(owner)) = owners.next_entry().await {
        let Ok(mut repos) = tokio::fs::read_dir(owner.path()).await else { continue };
        while let Ok(Some(repo)) = repos.next_entry().await {
            let Ok(mut prs) = tokio::fs::read_dir(repo.path()).await else { continue };
            while let Ok(Some(pr)) = prs.next_entry().await {
                // .git is "gitdir: <clone>/.git/worktrees/<name>"
                let clone = tokio::fs::read_to_string(pr.path().join(".git"))
                    .await
                    .ok()
                    .and_then(|t| t.trim().strip_prefix("gitdir: ").map(PathBuf::from))
                    .and_then(|g| g.parent()?.parent().map(|p| p.to_path_buf()));
                let _ = tokio::fs::remove_dir_all(pr.path()).await;
                if let Some(git_dir) = clone {
                    let _ = git(&["--git-dir", &git_dir.to_string_lossy(), "worktree", "prune"], None).await;
                }
            }
        }
    }
}

/// File contents at a commit, or None if the file doesn't exist there. `root`
/// is the checkout (worktree or the user's clone).
pub async fn read_at_commit(root: &str, sha: &str, path: &str) -> Result<Option<String>, String> {
    valid_sha(sha)?;
    if !crate::backend::fsaccess::is_checkout_root(Path::new(root)).await {
        return Err(format!("Not a review checkout: {root}"));
    }
    Ok(git(&["show", &format!("{sha}:{path}")], Some(Path::new(root))).await.ok())
}

/// One file's change as a unified diff with the whole file as context (like the
/// web version's full-file diff view). `previous_path` is the old name of a renamed file.
pub async fn file_patch(root: &str, merge_base: &str, head_sha: &str, path: &str, previous_path: Option<&str>) -> Result<String, String> {
    valid_sha(merge_base)?;
    valid_sha(head_sha)?;
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-M", "-U100000000", merge_base, head_sha, "--"];
    if let Some(old) = previous_path {
        args.push(old);
    }
    args.push(path);
    git(&args, Some(Path::new(root))).await
}

#[cfg(test)]
mod clone_worktree_tests {
    use super::*;

    /// Needs network and `gh` login: cargo test clone_worktree -- --ignored --nocapture
    /// REVIEW_TOOL_TEST_CLONE must be a clone of github.com/cli/cli.
    #[tokio::test]
    #[ignore]
    async fn clone_worktree_round_trip() {
        crate::backend::exec::augment_path();
        let clone = std::env::var("REVIEW_TOOL_TEST_CLONE").expect("REVIEW_TOOL_TEST_CLONE");
        let pr = crate::backend::github::get_pr("cli", "cli", 14925).await.unwrap();
        let started = std::time::Instant::now();
        let co = checkout(&pr, Some(&clone), true, false, |m| println!("progress: {m}")).await.unwrap();
        let CheckoutResult::Ready(co) = co else { panic!("expected a checkout") };
        println!("ready in {:?} at {}", started.elapsed(), co.root);
        let head = git(&["rev-parse", "HEAD"], Some(Path::new(&co.root))).await.unwrap();
        assert_eq!(head.trim(), pr.head_sha);
        // The PR's version of a changed file is there, and nothing is modified.
        assert!(Path::new(&co.root).join("internal/identity/ssh_command.go").exists());
        let status = git(&["status", "--porcelain", "--untracked-files=no"], Some(Path::new(&co.root))).await.unwrap();
        assert!(status.trim().is_empty(), "worktree has changes: {status}");
        // The clone itself didn't move.
        let clone_head = git(&["rev-parse", "HEAD"], Some(Path::new(&clone))).await.unwrap();
        assert_ne!(clone_head.trim(), pr.head_sha);
        assert_eq!(co.worktree_of.as_deref(), Some(git(&["rev-parse", "--show-toplevel"], Some(Path::new(&clone))).await.unwrap().trim()));

        remove_clone_worktree(&co.root, co.worktree_of.as_deref().unwrap()).await;
        assert!(!Path::new(&co.root).exists());
        let list = git(&["worktree", "list"], Some(Path::new(&clone))).await.unwrap();
        assert_eq!(list.lines().count(), 1, "{list}");
    }
}
