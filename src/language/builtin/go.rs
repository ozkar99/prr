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
    /// In a Bazel repository (a monorepo, typically), gopls would walk Bazel's
    /// output trees (bazel-bin, bazel-out… at the top, links into a huge
    /// cache) and load every package before answering anything. There it
    /// skips those and loads only the folders this PR changes; what they
    /// import still loads when needed, so definitions and hover work, but
    /// Find All References searches only those folders.
    fn adjust_for_pr(&self, spec: &mut ServerSpec, root: &Path, changed: &[String]) {
        let bazel = ["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"].iter().any(|f| root.join(f).is_file());
        if !bazel {
            return;
        }
        let dirs = go_dirs(changed);
        let mut filters: Vec<String> = Vec::new();
        if !dirs.is_empty() {
            filters.push("-".into());
            filters.extend(dirs.iter().map(|d| format!("+{d}")));
        }
        // Last match wins: these excludes go after the includes.
        let mut outputs: Vec<String> = std::fs::read_dir(root)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("bazel-")).collect())
            .unwrap_or_default();
        outputs.sort();
        filters.extend(outputs.iter().map(|o| format!("-{o}")));
        filters.push("-**/node_modules".into());
        if let Some(opts) = spec.initialization_options.as_object_mut() {
            opts.insert("directoryFilters".into(), json!(filters));
        }
        if !dirs.is_empty() {
            let n = dirs.len();
            spec.note = Some((
                format!("{n} folder{}", if n == 1 { "" } else { "s" }),
                format!(
                    "A Bazel repository: gopls loads only the folders this PR changes ({}), and skips Bazel's output folders. Definitions and hover still reach other packages; Find All References searches only these folders.",
                    dirs.join(", ")
                ),
            ));
        }
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

/// The folders (repo-relative) holding the changed Go files, without ones
/// inside another listed folder.
fn go_dirs(changed: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = changed
        .iter()
        .filter(|p| p.ends_with(".go"))
        .map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default())
        .collect();
    dirs.sort();
    dirs.dedup();
    // The repo root itself: no narrowing possible.
    if dirs.iter().any(|d| d.is_empty()) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        if !out.iter().any(|o| d.starts_with(&format!("{o}/"))) {
            out.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_gopls_in_bazel_repos() {
        let root = std::env::temp_dir().join(format!("prr-gobazel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bazel-out")).unwrap();
        std::fs::create_dir_all(root.join("svc")).unwrap();
        let changed = vec!["svc/api/a.go".to_string(), "svc/api/b.go".to_string(), "svc/api/inner/c.go".to_string(), "lib/x/d.go".to_string(), "README.md".to_string()];

        let mut spec = Go.server(&root).unwrap().unwrap();
        Go.adjust_for_pr(&mut spec, &root, &changed);
        assert!(spec.initialization_options.get("directoryFilters").is_none(), "not Bazel: untouched");

        std::fs::write(root.join("MODULE.bazel"), "").unwrap();
        let mut spec = Go.server(&root).unwrap().unwrap();
        Go.adjust_for_pr(&mut spec, &root, &changed);
        assert_eq!(spec.initialization_options["directoryFilters"], json!(["-", "+lib/x", "+svc/api", "-bazel-out", "-**/node_modules"]));
        assert_eq!(spec.note.as_ref().unwrap().0, "2 folders");
        let _ = std::fs::remove_dir_all(&root);
    }
}
