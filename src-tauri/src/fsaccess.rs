//! The editor may read files from the PR worktrees plus the places
//! go-to-definition can lead to: the Go stdlib and module cache, system /
//! Homebrew C++ headers, Python's stdlib and packages, pyright's stubs, and
//! Bazel's external repositories. Everything else is refused.
use crate::exec::{run, worktrees_dir};
use crate::types::FileStat;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use tokio::sync::OnceCell;

static ROOTS: OnceCell<Vec<PathBuf>> = OnceCell::const_new();

/// Where the bundled (or, in development, the npm-installed) pyright lives.
pub static PYRIGHT_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

async fn allowed_roots() -> &'static Vec<PathBuf> {
    ROOTS
        .get_or_init(|| async {
            let lines = |out: Result<String, String>| -> Vec<String> {
                out.unwrap_or_default().lines().map(|l| l.trim().to_string()).filter(|l| l.starts_with('/')).collect()
            };
            let go_dirs = lines(run("go", &["env", "GOROOT", "GOMODCACHE"], None, &[]).await);
            let python_dirs =
                lines(run("python3", &["-c", "import sys; print(\"\\n\".join(p for p in sys.path if p))"], None, &[]).await);
            let home = std::env::var("HOME").unwrap_or_default();
            let user = std::env::var("USER").unwrap_or_default();
            let mut candidates: Vec<String> = vec![worktrees_dir().to_string_lossy().into()];
            candidates.extend(go_dirs);
            candidates.extend(python_dirs);
            if let Some(Some(dir)) = PYRIGHT_DIR.get() {
                candidates.push(dir.to_string_lossy().into());
            }
            candidates.extend([
                format!("/private/var/tmp/_bazel_{user}"),
                format!("{home}/.cache/bazel"),
                "/usr/include".into(),
                "/Library/Developer/CommandLineTools".into(),
                "/Applications/Xcode.app/Contents/Developer".into(),
                "/opt/homebrew/include".into(),
                "/opt/homebrew/opt".into(),
                "/opt/homebrew/Cellar".into(),
            ]);
            let mut roots = Vec::new();
            for c in candidates {
                if let Ok(real) = tokio::fs::canonicalize(&c).await {
                    roots.push(real);
                }
            }
            roots
        })
        .await
}

async fn checked(path: &str) -> Result<PathBuf, String> {
    let real = tokio::fs::canonicalize(path).await.map_err(|e| format!("{path}: {e}"))?;
    if allowed_roots().await.iter().any(|root| real.starts_with(root)) {
        Ok(real)
    } else {
        Err(format!("Access outside allowed folders: {path}"))
    }
}

fn millis(t: std::io::Result<std::time::SystemTime>) -> f64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

pub async fn stat(path: &str) -> Option<FileStat> {
    let real = checked(path).await.ok()?;
    let m = tokio::fs::metadata(&real).await.ok()?;
    Some(FileStat {
        kind: if m.is_dir() { "directory" } else { "file" },
        size: m.len(),
        mtime: millis(m.modified()),
        ctime: millis(m.created()),
    })
}

pub async fn read(path: &str) -> Result<Vec<u8>, String> {
    tokio::fs::read(checked(path).await?).await.map_err(|e| e.to_string())
}

pub async fn readdir(path: &str) -> Result<Vec<(String, &'static str)>, String> {
    let mut entries = tokio::fs::read_dir(checked(path).await?).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(e) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        out.push((e.file_name().to_string_lossy().into_owned(), if is_dir { "directory" } else { "file" }));
    }
    Ok(out)
}
