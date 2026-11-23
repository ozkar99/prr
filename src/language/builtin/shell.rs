//! Shell scripts, with bash-language-server (bundled with the app, runs on Node.js).
use super::node_server;
use crate::language::{Language, ServerSpec, file_and_ext};
use std::path::Path;

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
            || matches!(
                file,
                ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" | "bashrc" | "zshrc" | "profile"
            )
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
