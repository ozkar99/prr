//! Which external tools are available on this machine, for the setup check on
//! the home screen. git and a logged-in gh are required; the rest unlock
//! language features or the agent panel.
use crate::backend::auth;
use crate::backend::exec::which;
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

    let github = github_status().await;

    let agent = crate::backend::agent::info();
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

    let mut tools = vec![tool("git", "checking out PRs", true, has("git"), "xcode-select --install"), github];
    // Each language's server (built-in or from languages.toml), once per tool.
    let registry = crate::language::registry();
    for lang in registry.all() {
        if let Some(r) = lang.requirement()
            && !tools.iter().any(|t| t.name == r.name)
        {
            tools.push(tool(&r.name, &r.purpose, false, r.installed, &r.fix));
        }
    }
    for e in &registry.config_errors {
        tools.push(ToolStatus { name: "languages.toml".into(), purpose: "your language settings".into(), required: false, ok: false, problem: Some(e.clone()), fix: None });
    }
    tools.push(agent_status);
    tools
}

/// Whether gh is installed and logged in, and that GitHub takes its token.
async fn github_status() -> ToolStatus {
    let mut status = ToolStatus { name: "gh".into(), purpose: "GitHub access (prr uses its login)".into(), required: true, ok: false, problem: None, fix: None };
    let program = auth::gh_program();
    if program != "gh" {
        status.name = program.rsplit('/').next().unwrap_or(&program).to_string();
        status.purpose = "GitHub access (the gh command from Settings)".into();
    }
    if let Err(e) = auth::check_gh().await {
        status.problem = Some(if program == "gh" && e.contains("isn't installed") { "not installed".into() } else { e });
        status.fix = (program == "gh").then(|| auth::HOW_TO_FIX.into());
        return status;
    }
    if let Err(e) = auth::token().await {
        status.problem = Some("not logged in".into());
        status.fix = Some(if e.contains("picked in Settings") { e } else { "gh auth login".into() });
        return status;
    }
    match crate::backend::github::viewer().await {
        Ok(login) => {
            status.ok = true;
            status.purpose = format!("GitHub access, signed in as @{login}");
        }
        Err(e) => {
            status.problem = Some(e);
            status.fix = Some("gh auth refresh -h github.com".into());
        }
    }
    status
}
