//! Markdown: syntax colors only (headings, emphasis, code, links, and the
//! languages of fenced code blocks); there's no language server.
use crate::language::{Language, ServerSpec, file_and_ext};
use std::path::Path;

pub struct Markdown;

impl Language for Markdown {
    fn id(&self) -> &str {
        "markdown"
    }
    fn name(&self) -> &str {
        "Markdown"
    }
    fn matches(&self, path: &str) -> bool {
        let (_, ext) = file_and_ext(path);
        matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd")
    }
    fn grammar(&self, _: &str) -> Option<&'static str> {
        Some("markdown")
    }
    fn server_name(&self) -> String {
        String::new()
    }
    fn server(&self, _root: &Path) -> Option<Result<ServerSpec, String>> {
        None
    }
}
