//! `prr [PR]` from a terminal: resolve the PR, hand it to the app
//! (starting it if needed), and give the terminal back. The installed app's
//! binary is the command, so nothing else needs installing.
use std::path::PathBuf;
use std::process::{Command, Stdio};

const USAGE: &str = "Usage: prr [PR]

Opens a GitHub pull request in prr.

PR can be:
  https://github.com/owner/repo/pull/123   a PR URL
  owner/repo#123                           a PR reference
  123                                      PR number in the GitHub repo of the current directory
  (nothing)                                the PR for the current branch, or just open the app

Options:
  -h, --help       show this help
  -v, --version    show the version";

fn fail(message: &str) -> ! {
    eprintln!("prr: {message}");
    std::process::exit(1)
}

/// Runs git in the current directory; trimmed stdout, or None on failure.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

/// "owner/repo" from a GitHub remote URL (https, ssh, or scp-style).
fn github_repo(url: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"github\.com[:/]([^/\s]+)/([^/\s]+?)(?:\.git)?/?$").unwrap();
    let c = re.captures(url.trim())?;
    Some((c[1].to_string(), c[2].to_string()))
}

/// The GitHub repo of the current directory's clone: its `upstream` remote
/// (a fork's original), else `origin`, else any GitHub remote.
fn current_repo() -> Option<(String, String)> {
    for remote in ["upstream", "origin"] {
        if let Some(r) = git(&["remote", "get-url", remote]).as_deref().and_then(github_repo) {
            return Some(r);
        }
    }
    git(&["remote"])?.lines().find_map(|r| git(&["remote", "get-url", r]).as_deref().and_then(github_repo))
}

/// The open PR for the current branch, if any.
fn current_branch_pr() -> Option<String> {
    let (owner, repo) = current_repo()?;
    let sha = git(&["rev-parse", "HEAD"]);
    // The branch as pushed: its upstream's remote (owner) and name.
    let (head_owner, branch) = match git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
        Some(up) => {
            let (remote, branch) = up.split_once('/')?;
            let owner = git(&["remote", "get-url", remote]).as_deref().and_then(github_repo).map(|(o, _)| o);
            (owner, Some(branch.to_string()))
        }
        None => (Some(owner.clone()), git(&["symbolic-ref", "--short", "-q", "HEAD"])),
    };
    let settings = crate::settings::load();
    crate::backend::auth::set_account(&settings.github_account);
    crate::backend::auth::set_gh_command(&settings.gh_command);
    // In a clone, gh runs right here, like typing it in this terminal.
    crate::backend::auth::set_folder(&git(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| settings.gh_folder_in_use().to_string()));
    crate::backend::block_on(crate::backend::github::pr_for_branch(&owner, &repo, head_owner.as_deref(), branch.as_deref(), sha.as_deref())).ok().flatten()
}

fn resolve_pr(arg: Option<&str>) -> Option<String> {
    let Some(arg) = arg else {
        return current_branch_pr();
    };
    if regex::Regex::new(r"github\.com/[^/]+/[^/]+/pull/\d+").unwrap().is_match(arg) {
        return Some(arg.to_string());
    }
    if let Some(r) = crate::backend::types::parse_pr_ref(arg) {
        return Some(format!("https://github.com/{}/{}/pull/{}", r.owner, r.repo, r.number));
    }
    if let Some(num) = arg.strip_prefix('#').unwrap_or(arg).parse::<u64>().ok() {
        let (owner, repo) = current_repo().unwrap_or_else(|| fail(&format!("\"{arg}\" is a PR number, but the current directory isn't a clone of a GitHub repository")));
        return Some(format!("https://github.com/{owner}/{repo}/pull/{num}"));
    }
    fail(&format!("don't understand \"{arg}\" (expected a PR URL, owner/repo#123, or a number)"))
}

/// When the current directory is inside a clone of the PR's repo (any of its
/// remotes points there), its top-level path: the app then reviews the PR in
/// that clone instead of its own checkout.
fn local_clone_for(pr_url: &str) -> Option<String> {
    let pr = crate::backend::types::parse_pr_ref(pr_url)?;
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let matches = git(&["remote"])?.lines().any(|r| {
        git(&["remote", "get-url", r]).as_deref().and_then(github_repo).is_some_and(|(o, n)| o.eq_ignore_ascii_case(&pr.owner) && n.eq_ignore_ascii_case(&pr.repo))
    });
    matches.then_some(top)
}

/// The .app bundle this binary is in, if any (Contents/MacOS/<binary>).
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    (bundle.extension().and_then(|e| e.to_str()) == Some("app")).then_some(bundle)
}

/// Handles a terminal invocation. Returns false when the app itself should start
/// (launched by macOS, or with the internal --open-pr argument).
pub fn handle(args: &[String]) -> bool {
    let user_args: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    if user_args.iter().any(|a| *a == "-v" || *a == "--version") {
        println!("prr {}", env!("CARGO_PKG_VERSION"));
        return true;
    }
    // Internal launch (macOS or this command passing --open-pr).
    if user_args.iter().any(|a| a.starts_with("--open-pr=")) || std::env::var_os("REVIEW_TOOL_APP").is_some() {
        return false;
    }
    // Launched from Finder/Dock (or by `open`): no terminal attached.
    let from_terminal = unsafe { libc::isatty(0) == 1 || libc::isatty(1) == 1 };
    if !from_terminal && user_args.is_empty() {
        return false;
    }
    if user_args.iter().any(|a| *a == "-h" || *a == "--help") {
        println!("{USAGE}");
        return true;
    }
    if user_args.len() > 1 {
        fail(USAGE);
    }

    let url = resolve_pr(user_args.first().copied());
    let local = url.as_deref().and_then(local_clone_for);
    let mut pr_args: Vec<String> = url.iter().map(|u| format!("--open-pr={u}")).collect();
    if let Some(path) = &local {
        pr_args.push(format!("--local-repo={path}"));
    }
    match (&url, &local) {
        (Some(u), Some(path)) => println!("Opening {u} in your clone at {path}"),
        (Some(u), None) => println!("Opening {u}"),
        _ => println!("Opening prr"),
    }

    let launched = match app_bundle() {
        // -n starts a new process even if the app is running; it then hands the PR
        // to the running window over the instance socket (src/instance.rs) and exits.
        Some(bundle) => Command::new("open").arg("-n").arg(&bundle).arg("--args").args(&pr_args).status().map(|s| s.success()),
        // Development build outside a bundle: start it detached from this terminal.
        None => {
            use std::os::unix::process::CommandExt;
            let exe = std::env::current_exe().unwrap_or_else(|e| fail(&e.to_string()));
            Command::new(exe)
                .args(&pr_args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .map(|_| true)
        }
    };
    if !matches!(launched, Ok(true)) {
        fail("couldn't start the app");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::github_repo;

    #[test]
    fn reads_github_remotes() {
        let want = Some(("ozkar99".to_string(), "prr".to_string()));
        assert_eq!(github_repo("https://github.com/ozkar99/prr.git"), want);
        assert_eq!(github_repo("https://github.com/ozkar99/prr"), want);
        assert_eq!(github_repo("git@github.com:ozkar99/prr.git"), want);
        assert_eq!(github_repo("ssh://git@github.com/ozkar99/prr.git"), want);
        assert_eq!(github_repo("https://gitlab.com/ozkar99/prr.git"), None);
    }
}
