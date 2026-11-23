//! Python, with pyright (bundled with the app, runs on Node.js).
use crate::language::{Language, Requirement, ServerSpec, file_and_ext};
use super::{installed, node_server};
use std::path::Path;

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
