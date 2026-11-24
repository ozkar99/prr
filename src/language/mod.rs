//! Language support behind one interface.
//!
//! A [`Language`] says which files it handles, which built-in grammar colors
//! them, how to run its language server for a checkout, and how its status is
//! drawn in the PR header. The built-ins live in [`builtin`]; users add or
//! override languages in `languages.toml` ([`config`]). Everything else asks
//! the [`Registry`] instead of knowing about specific languages.
pub mod builtin;
pub mod config;

use crate::theme;
use crate::ui;
use gpui::*;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

/// How to run a language server for one checkout.
#[derive(Clone, Debug, Default)]
pub struct ServerSpec {
    /// Program name (looked up on the PATH) or absolute path.
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Sent as `initializationOptions` in the LSP handshake.
    pub initialization_options: Value,
    /// Something worth showing next to the server's status (e.g. "guessing
    /// compile flags"): short text and a longer explanation.
    pub note: Option<(String, String)>,
}

/// A language server's state for one PR window.
#[derive(Clone, Debug)]
pub enum ServerStatus {
    Starting,
    Running,
    Failed(String),
}

/// A tool the home screen's setup check lists.
pub struct Requirement {
    pub name: String,
    pub purpose: String,
    pub installed: bool,
    pub fix: String,
}

pub trait Language: Send + Sync {
    /// Stable identifier, used in `languages.toml` and as the server's key ("go").
    fn id(&self) -> &str;
    /// Name shown to people ("Go").
    fn name(&self) -> &str;
    /// Whether this language handles a file (repo-relative or absolute path).
    fn matches(&self, path: &str) -> bool;
    /// The built-in grammar that colors this file (see `highlight.rs`), if any.
    fn grammar(&self, _path: &str) -> Option<&'static str> {
        None
    }
    /// The LSP `languageId` for a file.
    fn lsp_language_id(&self, _path: &str) -> String {
        self.id().to_string()
    }
    /// Short name of the server, for its status ("gopls").
    fn server_name(&self) -> String;
    /// How to run the server for the checkout at `root`. `None`: no server.
    fn server(&self, root: &Path) -> Option<Result<ServerSpec, String>>;
    /// Adjusts the server for one PR (`changed`: the paths it changes), e.g.
    /// to keep a huge repository's server to the PR's folders.
    fn adjust_for_pr(&self, _spec: &mut ServerSpec, _root: &Path, _changed: &[String]) {}
    /// What to tell someone whose server isn't installed.
    fn install_hint(&self) -> String {
        String::new()
    }
    /// The home screen's setup-check entry, if the server needs installing.
    fn requirement(&self) -> Option<Requirement> {
        None
    }
    /// Draws the server's status in the PR header.
    fn render_status(&self, status: &ServerStatus, note: Option<&(String, String)>, _cx: &mut App) -> AnyElement {
        status_chip(self.id(), &self.server_name(), status, note)
    }
}

/// The standard status chip: "gopls: running", with the error or note in a tooltip.
pub fn status_chip(id: &str, server: &str, status: &ServerStatus, note: Option<&(String, String)>) -> AnyElement {
    let (text, color, tip) = match status {
        ServerStatus::Starting => ("starting".to_string(), theme::WARN, None),
        ServerStatus::Running => ("running".to_string(), theme::ADD, None),
        ServerStatus::Failed(e) if e.contains("not installed") => ("not installed".to_string(), theme::DEL, Some(e.clone())),
        ServerStatus::Failed(e) => ("failed".to_string(), theme::DEL, Some(e.clone())),
    };
    let running = matches!(status, ServerStatus::Running);
    let label = match note.filter(|_| running) {
        Some((short, _)) => format!("{server}: {text} · {short}"),
        None => format!("{server}: {text}"),
    };
    let color = if running && note.is_some() { theme::WARN } else { color };
    let tip = tip.or_else(|| note.filter(|_| running).map(|(_, long)| long.clone()));
    let chip = ui::chip(label, color);
    match tip {
        // Hover for the whole message; click to copy the install command (or the error).
        Some(tip) => {
            let copy = tip.split_once("To install: ").map(|(_, cmd)| cmd.to_string()).unwrap_or(tip.clone());
            let hint = if tip.contains("To install: ") { "Click to copy the install command." } else { "Click to copy." };
            let tip = format!("{tip}\n\n{hint}");
            div()
                .id(SharedString::from(format!("lsp-{id}")))
                .cursor_pointer()
                .child(chip)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element()
        }
        None => chip.into_any_element(),
    }
}

/// The languages the app knows: configured ones first (they override
/// built-ins with the same id), then the built-ins.
pub struct Registry {
    languages: Vec<Arc<dyn Language>>,
    /// Problems reading `languages.toml`, shown on the home screen.
    pub config_errors: Vec<String>,
}

impl Registry {
    fn load() -> Self {
        let builtins = builtin::all();
        let (configured, config_errors) = config::load(&builtins);
        let mut languages: Vec<Arc<dyn Language>> = configured;
        for b in builtins {
            if !languages.iter().any(|l| l.id() == b.id()) {
                languages.push(b);
            }
        }
        Self { languages, config_errors }
    }

    pub fn all(&self) -> &[Arc<dyn Language>] {
        &self.languages
    }

    /// The language for a file: the first that matches.
    pub fn for_path(&self, path: &str) -> Option<Arc<dyn Language>> {
        self.languages.iter().find(|l| l.matches(path)).cloned()
    }

    pub fn by_id(&self, id: &str) -> Option<Arc<dyn Language>> {
        self.languages.iter().find(|l| l.id() == id).cloned()
    }

    /// The grammar for a file's syntax colors, or "text".
    pub fn grammar_for(&self, path: &str) -> &'static str {
        self.for_path(path).and_then(|l| l.grammar(path)).unwrap_or("text")
    }
}

static REGISTRY: OnceLock<RwLock<Arc<Registry>>> = OnceLock::new();

/// The current languages (loaded on first use).
pub fn registry() -> Arc<Registry> {
    REGISTRY.get_or_init(|| RwLock::new(Arc::new(Registry::load()))).read().unwrap().clone()
}

/// Re-reads `languages.toml` (newly opened PRs use the result).
pub fn reload() {
    let fresh = Arc::new(Registry::load());
    match REGISTRY.get() {
        Some(lock) => *lock.write().unwrap() = fresh,
        None => {
            let _ = REGISTRY.set(RwLock::new(fresh));
        }
    }
}

/// File name and extension of a path, for matching.
pub(crate) fn file_and_ext(path: &str) -> (&str, &str) {
    let file = path.rsplit('/').next().unwrap_or(path);
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    (file, ext)
}
