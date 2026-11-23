//! Everything that isn't UI: GitHub, git checkouts, cleanup, tool checks, and the
//! coding-agent plumbing (MCP server, launch command). Ported from the Tauri app's
//! src-tauri/src with the Tauri types removed.
//!
//! The backend is async on tokio (reqwest, child processes). The UI runs on gpui's
//! executor, so backend work is started on a tokio runtime with [`task`] and the
//! resulting future is awaited from gpui (`cx.spawn`).
pub mod agent;
pub mod auth;
pub mod cleanup;
pub mod deps;
pub mod exec;
pub mod fsaccess;
pub mod git;
pub mod github;
pub mod mcp;
pub mod types;

use std::future::Future;
use std::sync::OnceLock;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("backend")
            .enable_all()
            .build()
            .expect("failed to start the backend runtime")
    })
}

/// Runs `fut` to completion on the backend runtime, blocking this thread (the CLI).
pub fn block_on<T>(fut: impl Future<Output = T>) -> T {
    runtime().block_on(fut)
}

/// Runs `fut` on the backend's tokio runtime; the returned future can be awaited
/// from any executor (gpui's included).
pub fn task<F, T>(fut: F) -> impl Future<Output = T> + Send + 'static
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().spawn(fut);
    async move { handle.await.expect("backend task panicked") }
}

/// Like [`task`], for blocking work.
pub fn blocking<F, T>(f: F) -> impl Future<Output = T> + Send + 'static
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = runtime().spawn_blocking(f);
    async move { handle.await.expect("backend task panicked") }
}
