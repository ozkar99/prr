//! The languages that ship with the app, one file each, plus helpers they share.
mod cpp;
mod go;
mod python;
mod rust;
mod shell;
mod starlark;

pub use cpp::Cpp;
pub use go::Go;
pub use python::Python;
pub use rust::Rust;
pub use shell::Shell;
pub use starlark::Starlark;

use super::{Language, ServerSpec};
use crate::backend::exec::which;
use std::path::PathBuf;
use std::sync::Arc;

pub fn all() -> Vec<Arc<dyn Language>> {
    vec![
        Arc::new(Go),
        Arc::new(Cpp),
        Arc::new(Python),
        Arc::new(Rust),
        Arc::new(Shell),
        Arc::new(Starlark),
    ]
}

pub(super) fn installed(cmd: &str) -> bool {
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/language-servers/node_modules")
            .join(rel),
    ];
    candidates.into_iter().find(|p| p.exists()).and_then(|p| p.canonicalize().ok())
}

pub(super) fn node_server(script: &str, arg: &str, what: &str) -> Result<ServerSpec, String> {
    let js = bundled(script).ok_or_else(|| format!("{what} isn't bundled with this build"))?;
    Ok(ServerSpec {
        command: "node".into(),
        args: vec![js.to_string_lossy().into(), arg.into()],
        ..Default::default()
    })
}
