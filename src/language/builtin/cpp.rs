//! C, C++ and Objective-C, with clangd. Without compile_commands.json it
//! guesses flags, and says so in the header.
use super::installed;
use crate::language::{Language, Requirement, ServerSpec, file_and_ext};
use serde_json::json;
use std::path::Path;

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
        matches!(
            file_and_ext(path).1,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "ipp" | "inl" | "m" | "mm"
        )
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
        let mut args: Vec<String> = [
            "--background-index",
            "--header-insertion=never",
            "--clang-tidy=false",
            "--pch-storage=memory",
        ]
        .map(String::from)
        .into();
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
