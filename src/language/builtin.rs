//! The languages that ship with the app.
use super::{Language, Requirement, ServerSpec, file_and_ext};
use crate::backend::exec::which;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn all() -> Vec<Arc<dyn Language>> {
    vec![Arc::new(Go), Arc::new(Cpp), Arc::new(Python), Arc::new(Shell), Arc::new(Starlark)]
}

fn installed(cmd: &str) -> bool {
    which(cmd).is_some()
}

/// Node-based servers ship with the app in Resources/language-servers
/// (scripts/bundle-macos.sh); development builds use target/language-servers
/// (`npm install --prefix target/language-servers pyright bash-language-server`).
fn bundled(rel: &str) -> Option<PathBuf> {
    // Resolved, so a symlinked `prr` command still finds the bundle's Resources.
    let exe_dir = std::env::current_exe().ok()?.canonicalize().ok()?.parent()?.to_path_buf();
    let candidates = [
        exe_dir.join("../Resources/language-servers/node_modules").join(rel),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/language-servers/node_modules").join(rel),
    ];
    candidates.into_iter().find(|p| p.exists()).and_then(|p| p.canonicalize().ok())
}

fn node_server(script: &str, arg: &str, what: &str) -> Result<ServerSpec, String> {
    let js = bundled(script).ok_or_else(|| format!("{what} isn't bundled with this build"))?;
    Ok(ServerSpec { command: "node".into(), args: vec![js.to_string_lossy().into(), arg.into()], ..Default::default() })
}

// ---------------------------------------------------------------- Go

pub struct Go;

impl Language for Go {
    fn id(&self) -> &str {
        "go"
    }
    fn name(&self) -> &str {
        "Go"
    }
    fn matches(&self, path: &str) -> bool {
        file_and_ext(path).1 == "go"
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        Some("go")
    }
    fn server_name(&self) -> String {
        "gopls".into()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        Some(Ok(ServerSpec {
            command: "gopls".into(),
            args: vec!["serve".into()],
            // A PR's go.mod mustn't make gopls download and run another Go toolchain.
            env: vec![("GOTOOLCHAIN".into(), "local".into())],
            initialization_options: json!({ "semanticTokens": true }),
            note: None,
        }))
    }
    fn install_hint(&self) -> String {
        "go install golang.org/x/tools/gopls@latest".into()
    }
    fn requirement(&self) -> Option<Requirement> {
        Some(Requirement {
            name: "gopls".into(),
            purpose: "Go: types and navigation".into(),
            installed: installed("gopls"),
            fix: "brew install go && go install golang.org/x/tools/gopls@latest".into(),
        })
    }
}

// ---------------------------------------------------------------- C / C++

pub struct Cpp;

const CLANGD_BREW: &str = "/opt/homebrew/opt/llvm/bin/clangd";

/// clangd: guessed flags for files without compile commands (repo root and usual include dirs).
fn clangd_fallback_flags(root: &Path) -> Vec<String> {
    let mut flags = vec!["-std=c++20".to_string()];
    for dir in ["", "include", "src", "lib", "third_party", "external"] {
        let p = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
        if p.is_dir() {
            flags.push(format!("-I{}", p.display()));
        }
    }
    flags
}

impl Language for Cpp {
    fn id(&self) -> &str {
        "cpp"
    }
    fn name(&self) -> &str {
        "C / C++"
    }
    fn matches(&self, path: &str) -> bool {
        matches!(file_and_ext(path).1, "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "ipp" | "inl" | "m" | "mm")
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        // Headers are usually C++ in mixed repos; the C++ grammar parses C fine.
        Some("cpp")
    }
    fn lsp_language_id(&self, path: &str) -> String {
        match file_and_ext(path).1 {
            "c" => "c",
            "m" => "objective-c",
            "mm" => "objective-cpp",
            _ => "cpp",
        }
        .into()
    }
    fn server_name(&self) -> String {
        "clangd".into()
    }
    fn server(&self, root: &Path) -> Option<Result<ServerSpec, String>> {
        let command = if Path::new(CLANGD_BREW).exists() { CLANGD_BREW } else { "clangd" };
        let mut args: Vec<String> =
            ["--background-index", "--header-insertion=never", "--clang-tidy=false", "--pch-storage=memory"].map(String::from).into();
        let compile_commands = ["", "build", "out", "cmake-build-debug", "builddir"]
            .iter()
            .map(|d| if d.is_empty() { root.to_path_buf() } else { root.join(d) })
            .find(|d| d.join("compile_commands.json").exists());
        let note = match &compile_commands {
            Some(dir) => {
                args.push(format!("--compile-commands-dir={}", dir.display()));
                None
            }
            // Shown in the header: types may be off without the real flags.
            None => Some((
                "guessed flags".to_string(),
                "No compile_commands.json in the checkout (root, build/, out/, cmake-build-debug/ or builddir/), so clangd guesses \
                 -std=c++20 and include paths. Most headers resolve, but types and overloads can be wrong."
                    .to_string(),
            )),
        };
        Some(Ok(ServerSpec {
            command: command.into(),
            args,
            env: vec![],
            initialization_options: json!({ "fallbackFlags": clangd_fallback_flags(root) }),
            note,
        }))
    }
    fn install_hint(&self) -> String {
        "xcode-select --install  (or: brew install llvm)".into()
    }
    fn requirement(&self) -> Option<Requirement> {
        Some(Requirement {
            name: "clangd".into(),
            purpose: "C/C++: types and navigation".into(),
            installed: installed("clangd") || Path::new(CLANGD_BREW).exists(),
            fix: "xcode-select --install  (or: brew install llvm)".into(),
        })
    }
}

// ---------------------------------------------------------------- Python

pub struct Python;

impl Language for Python {
    fn id(&self) -> &str {
        "python"
    }
    fn name(&self) -> &str {
        "Python"
    }
    fn matches(&self, path: &str) -> bool {
        matches!(file_and_ext(path).1, "py" | "pyi")
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        Some("python")
    }
    fn server_name(&self) -> String {
        "pyright".into()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        Some(node_server("pyright/langserver.index.js", "--stdio", "pyright"))
    }
    fn install_hint(&self) -> String {
        "runs on Node.js: brew install node".into()
    }
    fn requirement(&self) -> Option<Requirement> {
        // Shared with Shell: both bundled servers run on Node.js.
        Some(Requirement {
            name: "node".into(),
            purpose: "Python and shell scripts: types and navigation (runs pyright, bash-language-server)".into(),
            installed: installed("node"),
            fix: "brew install node".into(),
        })
    }
}

// ---------------------------------------------------------------- Shell

pub struct Shell;

impl Language for Shell {
    fn id(&self) -> &str {
        "shell"
    }
    fn name(&self) -> &str {
        "Shell"
    }
    fn matches(&self, path: &str) -> bool {
        let (file, ext) = file_and_ext(path);
        matches!(ext, "sh" | "bash" | "zsh" | "ksh")
            || matches!(file, ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" | "bashrc" | "zshrc" | "profile")
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        Some("bash")
    }
    fn lsp_language_id(&self, _: &str) -> String {
        "shellscript".into()
    }
    fn server_name(&self) -> String {
        "bash-language-server".into()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        Some(node_server("bash-language-server/out/cli.js", "start", "bash-language-server"))
    }
    fn install_hint(&self) -> String {
        "runs on Node.js: brew install node".into()
    }
}

// ---------------------------------------------------------------- Starlark / Bazel

pub struct Starlark;

impl Language for Starlark {
    fn id(&self) -> &str {
        "starlark"
    }
    fn name(&self) -> &str {
        "Starlark / Bazel"
    }
    fn matches(&self, path: &str) -> bool {
        let (file, ext) = file_and_ext(path);
        matches!(ext, "bzl" | "star" | "bazel" | "sky") || matches!(file, "BUILD" | "WORKSPACE" | "WORKSPACE.bzlmod" | "MODULE.bazel")
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        // Starlark is a Python dialect.
        Some("python")
    }
    fn server_name(&self) -> String {
        "starpls".into()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        Some(Ok(ServerSpec { command: "starpls".into(), args: vec!["server".into()], initialization_options: Value::Null, ..Default::default() }))
    }
    fn install_hint(&self) -> String {
        "download starpls from https://github.com/withered-magic/starpls/releases and put it on your PATH".into()
    }
    fn requirement(&self) -> Option<Requirement> {
        Some(Requirement {
            name: "starpls".into(),
            purpose: "Starlark / Bazel: types and navigation".into(),
            installed: installed("starpls"),
            fix: "download from https://github.com/withered-magic/starpls/releases and put it on your PATH".into(),
        })
    }
}
