//! Rust, with rust-analyzer. PR code is never built or run, so build scripts
//! and proc macros are off, the toolchain is pinned to the user's default (a
//! PR's rust-toolchain.toml can't make rustup install another), and a PR's
//! .cargo/config.toml can't swap in its own rustc or rustc wrapper.
use crate::language::{Language, Requirement, ServerSpec, file_and_ext};
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct Rust;

/// Output of a command, if it ran and succeeded.
fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// rust-analyzer runs (rustup's proxy exists even when the component isn't installed).
fn available() -> bool {
    output("rust-analyzer", &["--version"]).is_some()
}

impl Language for Rust {
    fn id(&self) -> &str {
        "rust"
    }
    fn name(&self) -> &str {
        "Rust"
    }
    fn matches(&self, path: &str) -> bool {
        file_and_ext(path).1 == "rs"
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        Some("rust")
    }
    fn server_name(&self) -> String {
        "rust-analyzer".into()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        if !available() {
            return Some(Err(format!("not installed: rust-analyzer. To install: {}", self.install_hint())));
        }
        let mut env = vec![
            // Ignore wrappers a PR's .cargo/config.toml might set (they'd run its code).
            ("RUSTC_WRAPPER".to_string(), String::new()),
            ("CARGO_BUILD_RUSTC_WRAPPER".to_string(), String::new()),
            ("RUSTC_WORKSPACE_WRAPPER".to_string(), String::new()),
            ("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER".to_string(), String::new()),
        ];
        // The user's own toolchain, not one a PR's rust-toolchain.toml asks for.
        if let Some(default) = output("rustup", &["default"]).and_then(|d| d.split_whitespace().next().map(String::from)) {
            env.push(("RUSTUP_TOOLCHAIN".into(), default));
        }
        // The real rustc, not one a PR's .cargo/config.toml points at.
        if let Some(rustc) =
            output("rustup", &["which", "rustc"]).or_else(|| crate::backend::exec::which("rustc").map(|p| p.to_string_lossy().into_owned()))
        {
            env.push(("RUSTC".into(), rustc));
        }
        Some(Ok(ServerSpec {
            command: "rust-analyzer".into(),
            args: vec![],
            env,
            initialization_options: json!({
                "cargo": { "buildScripts": { "enable": false } },
                "procMacro": { "enable": false },
                "checkOnSave": false,
                "check": { "invocationStrategy": "once" },
            }),
            note: None,
        }))
    }
    fn install_hint(&self) -> String {
        "rustup component add rust-analyzer".into()
    }
    fn requirement(&self) -> Option<Requirement> {
        Some(Requirement {
            name: "rust-analyzer".into(),
            purpose: "Rust: types and navigation".into(),
            installed: available(),
            fix: "rustup component add rust-analyzer".into(),
        })
    }
}
