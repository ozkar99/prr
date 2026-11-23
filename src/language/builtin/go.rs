//! Go, with gopls.
use super::installed;
use crate::language::{Language, Requirement, ServerSpec, file_and_ext};
use serde_json::json;
use std::path::Path;

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
