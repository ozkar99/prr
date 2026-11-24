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
    // git signs in to github.com through gh (see gh::git_env), for anything that downloads.
    let mut env: Vec<(String, String)> = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
    env.extend(crate::backend::gh::git_env());
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    run("git", args, cwd, &env).await
}

/// prr's own clones are over HTTPS; git signs in through gh.
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
    // Not run inside a clone (opened from the home screen, a link, another
    // folder): a clone prr has reviewed this repo in before still beats a new
    // copy. Its copy-on-write worktree leaves the clone itself alone.
    if local_repo.is_none()
        && let Some(clone) = crate::backend::gh::clone_for(&pr.owner, &pr.repo)
    {
        return checkout_clone_worktree(pr, Path::new(&clone), progress).await.map(CheckoutResult::Ready);
    }
    match local_repo {
        Some(path) if local_worktree => checkout_clone_worktree(pr, Path::new(path), progress).await.map(CheckoutResult::Ready),
        Some(path) => match checkout_local(pr, Path::new(path), allow_switch, &progress).await? {
            // Uncommitted changes: the clone can't be switched, so review in a
            // copy-on-write worktree of it instead (cheap even for a monorepo).
            CheckoutResult::NeedsSwitch { dirty: true, .. } => checkout_clone_worktree(pr, Path::new(path), progress).await.map(CheckoutResult::Ready),
            other => Ok(other),
        },
        None => checkout_managed(pr, progress).await.map(CheckoutResult::Ready),
    }
}

// ---- The user's own clone ----

/// The remote in `repo` that points at <owner>/<name> (on github.com or an
/// internal mirror of it), if any.
async fn matching_remote(repo: &Path, owner: &str, name: &str) -> Option<String> {
    let remotes = git(&["remote", "-v"], Some(repo)).await.ok()?;
    let wanted = format!("{owner}/{name}").to_lowercase();
    remotes.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (remote, url) = (parts.next()?, parts.next()?);
        let path = url.trim_end_matches('/').trim_end_matches(".git").to_lowercase();
        // Any host: a company may serve its GitHub repos from an internal git
        // host, which may not mention the organization (git@host:repo.git).
        let repo_only = name.to_lowercase();
        let found = path.ends_with(&format!("/{wanted}"))
            || path.ends_with(&format!(":{wanted}"))
            || path.ends_with(&format!("/{repo_only}"))
            || path.ends_with(&format!(":{repo_only}"));
        found.then(|| remote.to_string())
    })
}

/// The clone's top-level directory, with the PR's head and base fetched into it
/// (no destination refs: the commits become available without adding branches
/// or refs to the user's repo). Returns (top, merge base).
async fn fetch_into_clone(pr: &PRDetails, repo: &Path, progress: &impl Fn(String)) -> Result<(PathBuf, String), String> {
    let top = git(&["rev-parse", "--show-toplevel"], Some(repo)).await.map_err(|_| format!("{} isn't a git clone", repo.display()))?;
    let top = PathBuf::from(top.trim());
    // Only what's missing: in a big clone the base commit is usually there
    // already, and so is the PR head if it was fetched before.
    let has = |sha: String| {
        let top = top.clone();
        async move { git(&["cat-file", "-e", &format!("{sha}^{{commit}}")], Some(&top)).await.is_ok() }
    };
    let mut refs: Vec<String> = Vec::new();
    if !has(pr.head_sha.clone()).await {
        refs.push(format!("refs/pull/{}/head", pr.number));
    }
    if !has(pr.base_sha.clone()).await {
        refs.push(format!("refs/heads/{}", pr.base_ref));
    }
    if refs.is_empty() {
        let merge_base = git(&["merge-base", &pr.base_sha, &pr.head_sha], Some(&top)).await?.trim().to_string();
        return Ok((top, merge_base));
    }
    progress(format!("Fetching PR #{} into your clone…", pr.number));
    let github = remote_url(&pr.owner, &pr.repo).await;
    let fetch = |remote: String| {
        let (top, refs) = (top.clone(), refs.clone());
        async move {
            let mut args = vec!["fetch", "--no-tags", "--no-write-fetch-head", remote.as_str()];
            args.extend(refs.iter().map(String::as_str));
            git(&args, Some(&top)).await
        }
    };
    match matching_remote(&top, &pr.owner, &pr.repo).await {
        // The clone's own remote (its credentials, possibly an internal mirror); a
        // mirror may not carry GitHub's pull/<n>/head refs, so fall back to GitHub.
        Some(remote) => {
            if let Err(first) = fetch(remote.clone()).await
                && let Err(second) = fetch(github).await
            {
                return Err(format!("Couldn't fetch PR #{} from your remote \"{remote}\" ({first}) or from GitHub ({second})", pr.number));
            }
        }
        None => fetch(github).await.map(|_| ())?,
    }
    let merge_base = git(&["merge-base", &pr.base_sha, &pr.head_sha], Some(&top)).await?.trim().to_string();
    Ok((top, merge_base))
}

async fn checkout_local(pr: &PRDetails, repo: &Path, allow_switch: bool, progress: &impl Fn(String)) -> Result<CheckoutResult, String> {
    let (top, merge_base) = fetch_into_clone(pr, repo, progress).await?;
    let top_s = top.to_string_lossy().to_string();

    let head = git(&["rev-parse", "HEAD"], Some(&top)).await?.trim().to_string();
    let ready = |switched_from: Option<String>, pr_branch: Option<String>| {
        crate::backend::fsaccess::allow_root(&top);
        CheckoutResult::Ready(Checkout {
            root: top_s.clone(),
            merge_base: merge_base.clone(),
            head_sha: pr.head_sha.clone(),
            local: Some(LocalClone { path: top_s.clone(), switched_from, pr_branch }),
            worktree_of: None,
        })
    };
    if head == pr.head_sha {
        return Ok(ready(None, None));
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
    // The PR gets its own branch, never a detached HEAD: anything done in the
    // clone while reviewing stays on that branch, and switching back is safe.
    let pr_branch = pr_branch_for(&top, pr).await?;
    progress(format!("Switching your clone to {pr_branch}…"));
    git(&["switch", &pr_branch], Some(&top)).await?;
    // What to switch back to when the PR is closed.
    Ok(ready(Some(branch.unwrap_or(head)), Some(pr_branch)))
}

/// A branch at the PR head for reviewing it in the user's clone: prr/pr-<n>,
/// created there, or moved forward if it only lags behind the PR. If it holds
/// commits of the user's own, they're left alone and a new branch is made.
async fn pr_branch_for(top: &Path, pr: &PRDetails) -> Result<String, String> {
    let name = format!("prr/pr-{}", pr.number);
    let tip = git(&["rev-parse", "--verify", "-q", &format!("refs/heads/{name}")], Some(top)).await.ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let Some(tip) = tip else {
        git(&["branch", &name, &pr.head_sha], Some(top)).await?;
        return Ok(name);
    };
    if tip == pr.head_sha {
        return Ok(name);
    }
    // Behind the PR head (the PR got new commits): move it forward.
    if git(&["merge-base", "--is-ancestor", &tip, &pr.head_sha], Some(top)).await.is_ok() {
        git(&["branch", "-f", &name, &pr.head_sha], Some(top)).await?;
        return Ok(name);
    }
    // It has commits that aren't in the PR: keep them, use another branch.
    let other = format!("{name}-{}", &pr.head_sha[..pr.head_sha.len().min(7)]);
    git(&["branch", "-f", &other, &pr.head_sha], Some(top)).await?;
    Ok(other)
}

/// Puts the user's clone back on the branch it was on, if it's still on the
/// PR's branch with no uncommitted changes. The PR branch is kept, with
/// anything committed on it.
pub async fn restore_local(path: &str, switched_from: &str, pr_branch: &str) -> Result<bool, String> {
    let repo = Path::new(path);
    let current = git(&["symbolic-ref", "--short", "-q", "HEAD"], Some(repo)).await.unwrap_or_default().trim().to_string();
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"], Some(repo)).await?.trim().is_empty();
    if current != pr_branch || dirty {
        return Ok(false); // the user moved on or changed something; leave it alone
    }
    // A branch name, or a commit if the clone wasn't on a branch before.
    if git(&["switch", switched_from], Some(repo)).await.is_err() {
        git(&["switch", "--detach", switched_from], Some(repo)).await?;
    }
    Ok(true)
}

/// Deletes prr's branch for a PR once it's done with, unless it holds
/// commits beyond the PR head (yours). True if deleted.
pub async fn delete_pr_branch(path: &str, branch: &str, head_sha: &str) -> bool {
    let repo = Path::new(path);
    let tip = git(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")], Some(repo)).await.map(|s| s.trim().to_string()).unwrap_or_default();
    tip == head_sha && git(&["branch", "-D", branch], Some(repo)).await.is_ok()
}

// ---- App-managed clone + worktrees ----

/// Above this (KB, as GitHub reports a repository's size), prr won't make its own copy.
const LARGE_REPO_KB: u64 = 2_000_000;

async fn checkout_managed(pr: &PRDetails, progress: impl Fn(String)) -> Result<Checkout, String> {
    let bare = bare_repo_path(&pr.owner, &pr.repo);
    let bare_s = bare.to_string_lossy().to_string();
    if !bare.exists() {
        // Never download a huge repository unasked: a monorepo belongs in the
        // user's own clone.
        if let Ok(kb) = crate::backend::github::repo_size_kb(&pr.owner, &pr.repo).await
            && kb > LARGE_REPO_KB
        {
            return Err(format!(
                "{}/{} is about {:.1} GB, too big for prr to download a copy of. Run prr inside your clone of it instead: cd <your clone> && prr {}",
                pr.owner,
                pr.repo,
                kb as f64 / 1_000_000.0,
                pr.url
            ));
        }
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
    // Mark it as used now (for the 14-day cleanup).
    if let Ok(f) = std::fs::File::options().append(true).open(root.join(".git")) {
        let _ = f.set_modified(std::time::SystemTime::now());
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

/// Startup: copies of user clones are kept for reopening their PR; remove
/// the ones whose PR was merged or closed, or that weren't opened for 14 days.
pub async fn remove_stale_clone_worktrees() {
    let base = clone_worktrees_dir();
    let Ok(mut owners) = tokio::fs::read_dir(&base).await else { return };
    while let Ok(Some(owner)) = owners.next_entry().await {
        let Ok(mut repos) = tokio::fs::read_dir(owner.path()).await else { continue };
        while let Ok(Some(repo)) = repos.next_entry().await {
            let Ok(mut prs) = tokio::fs::read_dir(repo.path()).await else { continue };
            while let Ok(Some(pr)) = prs.next_entry().await {
                let (owner_s, repo_s) = (owner.file_name().to_string_lossy().into_owned(), repo.file_name().to_string_lossy().into_owned());
                let number = pr.file_name().to_string_lossy().strip_prefix("pr-").and_then(|n| n.parse::<u64>().ok());
                // .git is "gitdir: <clone>/.git/worktrees/<name>"; its time is when the PR was last opened.
                let gitfile = pr.path().join(".git");
                let unused = tokio::fs::metadata(&gitfile)
                    .await
                    .and_then(|m| m.modified())
                    .map(|t| t.elapsed().unwrap_or_default() > std::time::Duration::from_secs(14 * 24 * 3600))
                    .unwrap_or(true);
                let done = match number {
                    Some(n) => crate::backend::github::pr_is_done(&owner_s, &repo_s, n).await,
                    None => true,
                };
                if !(unused || done) {
                    continue;
                }
                let clone = tokio::fs::read_to_string(&gitfile)
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

    #[tokio::test]
    async fn reviews_on_a_pr_branch_and_switches_back() {
        let dir = std::env::temp_dir().join(format!("prr-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |args: &[&str]| {
            let out = std::process::Command::new("git").args(args).current_dir(&dir).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        sh(&["init", "-q", "-b", "main"]);
        sh(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "base"]);
        let base = sh(&["rev-parse", "HEAD"]);
        sh(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "pr"]);
        let head = sh(&["rev-parse", "HEAD"]);
        sh(&["reset", "-q", "--hard", &base]);
        let pr = PRDetails {
            owner: "o".into(), repo: "r".into(), number: 7, title: String::new(), body: String::new(), author: String::new(),
            url: String::new(), base_ref: "main".into(), base_sha: base.clone(), head_sha: head.clone(), files: vec![],
        };

        let branch = pr_branch_for(&dir, &pr).await.unwrap();
        assert_eq!(branch, "prr/pr-7");
        sh(&["switch", "-q", &branch]);
        assert_eq!(sh(&["rev-parse", "HEAD"]), head);
        assert_eq!(sh(&["rev-parse", "main"]), base, "main untouched");

        assert!(restore_local(&dir.to_string_lossy(), "main", &branch).await.unwrap());
        assert_eq!(sh(&["symbolic-ref", "--short", "HEAD"]), "main");
        assert_eq!(sh(&["rev-parse", "prr/pr-7"]), head, "the PR branch stays");

        // Work committed on the PR branch is never overwritten: another branch is used.
        sh(&["switch", "-q", "prr/pr-7"]);
        sh(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "mine"]);
        let mine = sh(&["rev-parse", "HEAD"]);
        sh(&["switch", "-q", "main"]);
        let again = pr_branch_for(&dir, &pr).await.unwrap();
        assert_ne!(again, "prr/pr-7");
        assert_eq!(sh(&["rev-parse", "prr/pr-7"]), mine, "the user's commit is kept");
        assert_eq!(sh(&["rev-parse", &again]), head);

        // Approved: the branch goes, unless it holds the user's commits.
        assert!(!delete_pr_branch(&dir.to_string_lossy(), "prr/pr-7", &head).await, "has the user's commit");
        assert!(delete_pr_branch(&dir.to_string_lossy(), &again, &head).await);
        let again = pr_branch_for(&dir, &pr).await.unwrap();

        // Not restored when something changed.
        sh(&["switch", "-q", &again]);
        std::fs::write(dir.join("f"), "x").unwrap();
        sh(&["add", "f"]);
        assert!(!restore_local(&dir.to_string_lossy(), "main", &again).await.unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}