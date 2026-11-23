//! Starlark / Bazel, with starpls.
use crate::language::{Language, Requirement, ServerSpec, file_and_ext};
use super::{installed};
use serde_json::{Value};
use std::path::Path;

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
