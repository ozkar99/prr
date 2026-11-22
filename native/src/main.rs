#![allow(dead_code)] // while the rewrite is in progress, parts of the backend aren't wired up yet

mod app;
mod avatar;
mod backend;
#[cfg(debug_assertions)]
mod devdriver;
mod diffview;
mod highlight;
mod home;
mod lsp;
mod markdown;
mod pr_view;
mod review;
mod settings;
mod terminal;
mod theme;
mod ui;

use gpui::*;
use gpui_component::Root;

/// `--open-pr=<url>` (and `--local-repo=<path>`), as passed by the `review-tool` command.
fn launch_pr() -> Option<backend::types::PRRef> {
    let args: Vec<String> = std::env::args().collect();
    let mut pr = args.iter().find_map(|a| a.strip_prefix("--open-pr=").and_then(backend::types::parse_pr_ref))?;
    pr.local_repo = args.iter().find_map(|a| a.strip_prefix("--local-repo=").map(String::from));
    Some(pr)
}

/// When launched from inside a Claude Code session (e.g. `review-tool` typed in its
/// terminal), drop that session's markers so the agent panel starts its own,
/// independent session. User settings like CLAUDE_CODE_USE_BEDROCK are kept.
fn forget_parent_agent_session() {
    let markers = ["CLAUDECODE", "CLAUDE_PID", "CLAUDE_EFFORT", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_SSE_PORT"];
    let prefixes = ["CLAUDE_CODE_CHILD_", "CLAUDE_CODE_MESSAGING_", "CLAUDE_CODE_SESSION_"];
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if markers.contains(&key) || prefixes.iter().any(|p| key.starts_with(p)) {
            // SAFETY: called at startup, before any other threads exist.
            unsafe { std::env::remove_var(key) };
        }
    }
}

fn main() {
    // `review-tool --mcp-stdio <url> <token>`: the stdio MCP bridge agents start (no UI).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--mcp-stdio") {
        if let (Some(url), Some(token)) = (args.get(2), args.get(3)) {
            backend::agent::run_stdio_bridge(url, token);
        }
        return;
    }
    backend::exec::augment_path();
    forget_parent_agent_session();
    let launch = launch_pr();
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        diffview::init(cx);
        pr_view::init(cx);
        theme::apply(cx);
        settings::init(cx);

        let bounds = Bounds::centered(None, size(px(1500.), px(950.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Review Tool".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(13.))),
                }),
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| app::AppView::new(window, cx));
                if let Some(pr) = launch.clone() {
                    view.update(cx, |view, cx| view.open(pr, window, cx));
                }
                #[cfg(debug_assertions)]
                {
                    let weak = view.downgrade();
                    devdriver::start(cx, move |url, window, cx| {
                        if let Some(pr) = backend::types::parse_pr_ref(&url) {
                            weak.update(cx, |view, cx| view.open(pr, window, cx)).ok();
                        }
                    });
                }
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open the window");
        cx.activate(true);
        cx.spawn(async move |_| backend::cleanup::on_startup().await).detach();
    });
}
