//! Languages people add (or built-ins they adjust) in
//! `~/Library/Application Support/review-tool/languages.toml`:
//!
//! ```toml
//! [[language]]
//! name = "Zig"
//! extensions = ["zig"]
//! command = "zls"
//!
//! [[language]]
//! id = "go"                       # same id as a built-in: adjust it
//! command = "/opt/tools/gopls"    # only what's set changes
//! ```
use super::{Language, Requirement, ServerSpec, file_and_ext};
use crate::backend::exec::{data_dir, which};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn path() -> PathBuf {
    data_dir().join("languages.toml")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, rename = "language")]
    languages: Vec<Entry>,
}

/// One `[[language]]` table.
#[derive(Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Identifier; defaults to the name in lowercase. A built-in's id adjusts that built-in.
    pub id: Option<String>,
    pub name: Option<String>,
    /// File extensions without the dot ("rs").
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Exact file names ("Justfile").
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Server program (on the PATH, or an absolute path).
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// LSP `languageId` (defaults to the id).
    pub language_id: Option<String>,
    /// Borrow a built-in grammar for syntax colors: go, c, cpp, python, rust or bash.
    pub grammar: Option<String>,
    /// Shown when the server isn't installed.
    pub install: Option<String>,
    /// Sent as `initializationOptions`.
    pub initialization_options: Option<toml::Value>,
}

/// A language from `languages.toml`, optionally on top of a built-in.
pub struct Configured {
    id: String,
    name: String,
    entry: Entry,
    base: Option<Arc<dyn Language>>,
}

impl Configured {
    fn own_patterns(&self) -> bool {
        !self.entry.extensions.is_empty() || !self.entry.filenames.is_empty()
    }
}

const GRAMMARS: &[&str] = &["go", "c", "cpp", "python", "rust", "bash"];

impl Language for Configured {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn matches(&self, path: &str) -> bool {
        if !self.own_patterns() {
            return self.base.as_ref().is_some_and(|b| b.matches(path));
        }
        let (file, ext) = file_and_ext(path);
        self.entry.extensions.iter().any(|e| e.trim_start_matches('.') == ext && !ext.is_empty()) || self.entry.filenames.iter().any(|f| f == file)
    }
    fn grammar(&self, path: &str) -> Option<&'static str> {
        match &self.entry.grammar {
            Some(g) => GRAMMARS.iter().copied().find(|k| k == g),
            None => self.base.as_ref().and_then(|b| b.grammar(path)),
        }
    }
    fn lsp_language_id(&self, path: &str) -> String {
        match (&self.entry.language_id, &self.base) {
            (Some(id), _) => id.clone(),
            (None, Some(b)) => b.lsp_language_id(path),
            (None, None) => self.id.clone(),
        }
    }
    fn server_name(&self) -> String {
        match (&self.entry.command, &self.base) {
            (Some(c), _) => Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(c.clone()),
            (None, Some(b)) => b.server_name(),
            (None, None) => self.name.clone(),
        }
    }
    fn server(&self, root: &Path) -> Option<Result<ServerSpec, String>> {
        let mut spec = match (&self.entry.command, &self.base) {
            // A built-in's server, adjusted below.
            (None, Some(b)) => match b.server(root)? {
                Ok(spec) => spec,
                Err(e) => return Some(Err(e)),
            },
            (Some(c), base) => {
                // A built-in with another command keeps its arguments, environment and
                // options (set `args` etc. to change them); a new language starts bare.
                let base_spec = base.as_ref().and_then(|b| b.server(root)).and_then(Result::ok).unwrap_or_default();
                ServerSpec { command: c.clone(), ..base_spec }
            }
            (None, None) => return None,
        };
        if let Some(args) = &self.entry.args {
            spec.args = args.clone();
        }
        for (k, v) in &self.entry.env {
            spec.env.retain(|(key, _)| key != k);
            spec.env.push((k.clone(), v.clone()));
        }
        if let Some(opts) = &self.entry.initialization_options {
            spec.initialization_options = serde_json::to_value(opts).unwrap_or(Value::Null);
        }
        Some(Ok(spec))
    }
    fn install_hint(&self) -> String {
        match (&self.entry.install, &self.base) {
            (Some(h), _) => h.clone(),
            (None, Some(b)) if self.entry.command.is_none() => b.install_hint(),
            _ => format!("put {} on your PATH, or set its full path in {}", self.server_name(), path().display()),
        }
    }
    fn requirement(&self) -> Option<Requirement> {
        let command = self.entry.command.clone().or_else(|| self.base.as_ref().map(|b| b.server_name()))?;
        Some(Requirement {
            name: self.server_name(),
            purpose: format!("{}: types and navigation (from languages.toml)", self.name),
            installed: if Path::new(&command).is_absolute() { Path::new(&command).exists() } else { which(&command).is_some() },
            fix: self.install_hint(),
        })
    }
    fn render_status(&self, status: &super::ServerStatus, note: Option<&(String, String)>, cx: &mut gpui::App) -> gpui::AnyElement {
        match &self.base {
            // An adjusted built-in keeps its own status drawing.
            Some(b) if self.entry.command.is_none() => b.render_status(status, note, cx),
            _ => super::status_chip(&self.id, &self.server_name(), status, note),
        }
    }
}

/// Reads `languages.toml`: the configured languages, and any problems with the file.
pub fn load(builtins: &[Arc<dyn Language>]) -> (Vec<Arc<dyn Language>>, Vec<String>) {
    let file = path();
    let Ok(text) = std::fs::read_to_string(&file) else { return (Vec::new(), Vec::new()) };
    parse(&text, builtins, &file.display().to_string())
}

pub fn parse(text: &str, builtins: &[Arc<dyn Language>], source: &str) -> (Vec<Arc<dyn Language>>, Vec<String>) {
    let parsed: File = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => return (Vec::new(), vec![format!("{source}: {e}")]),
    };
    let mut languages: Vec<Arc<dyn Language>> = Vec::new();
    let mut errors = Vec::new();
    for (i, entry) in parsed.languages.into_iter().enumerate() {
        let n = i + 1;
        let Some(id) = entry.id.clone().or_else(|| entry.name.as_ref().map(|n| n.to_lowercase().replace(' ', "-"))) else {
            errors.push(format!("{source}: language #{n} needs a name or an id"));
            continue;
        };
        let base = builtins.iter().find(|b| b.id() == id).cloned();
        if base.is_none() && entry.extensions.is_empty() && entry.filenames.is_empty() {
            errors.push(format!("{source}: language \"{id}\" needs extensions or filenames (or the id of a built-in: go, cpp, python, rust, shell, starlark)"));
            continue;
        }
        if base.is_none() && entry.command.is_none() && entry.grammar.is_none() {
            errors.push(format!("{source}: language \"{id}\" needs a command (its language server) or a grammar"));
            continue;
        }
        if let Some(g) = &entry.grammar
            && !GRAMMARS.contains(&g.as_str())
        {
            errors.push(format!("{source}: language \"{id}\": grammar must be one of {}", GRAMMARS.join(", ")));
            continue;
        }
        if languages.iter().any(|l| l.id() == id) {
            errors.push(format!("{source}: language \"{id}\" is listed twice"));
            continue;
        }
        let name = entry.name.clone().or_else(|| base.as_ref().map(|b| b.name().to_string())).unwrap_or(id.clone());
        languages.push(Arc::new(Configured { id, name, entry, base }));
    }
    (languages, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::builtin;

    #[test]
    fn adds_and_adjusts_languages() {
        let builtins = builtin::all();
        let toml = r#"
            [[language]]
            name = "Zig"
            extensions = ["zig"]
            command = "zls"

            [[language]]
            id = "go"
            command = "/opt/tools/gopls"
            env = { GOFLAGS = "-mod=mod" }
        "#;
        let (langs, errors) = parse(toml, &builtins, "test");
        assert!(errors.is_empty(), "{errors:?}");
        let zig = langs.iter().find(|l| l.id() == "zig").unwrap();
        assert!(zig.matches("src/main.zig") && !zig.matches("main.go"));
        assert_eq!(zig.grammar("x.zig"), None);
        let spec = zig.server(Path::new("/tmp")).unwrap().unwrap();
        assert_eq!((spec.command.as_str(), spec.args.len()), ("zls", 0));

        let go = langs.iter().find(|l| l.id() == "go").unwrap();
        assert!(go.matches("cmd/main.go"), "an adjusted built-in keeps its files");
        assert_eq!(go.grammar("a.go"), Some("go"));
        assert_eq!(go.server_name(), "gopls");
        let spec = go.server(Path::new("/tmp")).unwrap().unwrap();
        assert_eq!(spec.command, "/opt/tools/gopls");
        assert_eq!(spec.args, vec!["serve".to_string()], "keeps the built-in's arguments");
        assert!(spec.env.contains(&("GOTOOLCHAIN".into(), "local".into())), "keeps the built-in's safety env");
        assert!(spec.env.contains(&("GOFLAGS".into(), "-mod=mod".into())));
    }

    #[test]
    fn reports_mistakes() {
        let builtins = builtin::all();
        let (langs, errors) = parse("[[language]]\nname = \"Zig\"\ncommand = \"zls\"\n", &builtins, "test");
        assert!(langs.is_empty() && errors[0].contains("needs extensions"), "{errors:?}");
        let (_, errors) = parse("[[language]]\nname = \"X\"\nextensions = [\"x\"]\ngrammar = \"kotlin\"\n", &builtins, "test");
        assert!(errors[0].contains("grammar must be"), "{errors:?}");
        let (_, errors) = parse("[[language]\n", &builtins, "test");
        assert!(!errors.is_empty());
    }
}
