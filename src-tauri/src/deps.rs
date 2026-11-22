//! Which external tools are available on this machine, for the setup check on
//! the home screen. git and a logged-in gh are required; the rest unlock
//! language features or the agent panel.
use crate::exec::{run, which};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: String,
    /// What it's for, shown next to the name.
    pub purpose: String,
    pub required: bool,
    pub ok: bool,
    /// Why it isn't ok (not installed, not logged in, …).
    pub problem: Option<String>,
    /// Command that fixes it.
    pub fix: Option<String>,
}

fn tool(name: &str, purpose: &str, required: bool, installed: bool, fix: &str) -> ToolStatus {
    ToolStatus {
        name: name.into(),
        purpose: purpose.into(),
        required,
        ok: installed,
        problem: (!installed).then(|| "not installed".into()),
        fix: (!installed).then(|| fix.into()),
    }
}

pub async fn check() -> Vec<ToolStatus> {
    let has = |cmd: &str| which(cmd).is_some();

    let mut gh = tool("gh", "GitHub access (the app uses its login)", true, has("gh"), "brew install gh && gh auth login");
    if gh.ok && run("gh", &["auth", "status"], None, &[]).await.is_err() {
        gh.ok = false;
        gh.problem = Some("not logged in".into());
        gh.fix = Some("gh auth login".into());
    }

    let agent = crate::agent::info();
    let agent_status = match &agent.command {
        // A custom agent command runs through the shell; trust the user's file.
        Some(_) => ToolStatus {
            name: agent.name.clone(),
            purpose: format!("agent panel (from {})", agent.config_path),
            required: false,
            ok: true,
            problem: None,
            fix: None,
        },
        None => tool("claude", "agent panel (Claude Code)", false, has("claude"), "curl -fsSL https://claude.ai/install.sh | bash"),
    };

    vec![
        tool("git", "checking out PRs", true, has("git"), "xcode-select --install"),
        gh,
        tool("gopls", "Go: types and navigation", false, has("gopls"), "brew install go && go install golang.org/x/tools/gopls@latest"),
        tool(
            "clangd",
            "C/C++: types and navigation",
            false,
            has("clangd") || std::path::Path::new("/opt/homebrew/opt/llvm/bin/clangd").exists(),
            "xcode-select --install",
        ),
        tool("node", "Python: types and navigation (runs pyright)", false, has("node"), "brew install node"),
        tool(
            "starpls",
            "Starlark/Bazel: types and navigation",
            false,
            has("starpls"),
            "download from https://github.com/withered-magic/starpls/releases and put it on your PATH",
        ),
        agent_status,
    ]
}
