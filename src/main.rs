#![allow(dead_code)] // while the rewrite is in progress, parts of the backend aren't wired up yet

mod app;
mod cli;
mod instance;
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

actions!(review_tool, [Quit, Hide, HideOthers, ShowAll]);

/// The macOS menu bar: the app menu with Settings, Hide and Quit.
fn set_up_menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
    ]);
    cx.set_menus([Menu::new("Review Tool").items([
        MenuItem::action("Settings…", settings::OpenSettings),
        MenuItem::separator(),
        MenuItem::action("Hide Review Tool", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit Review Tool", Quit),
    ])]);
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
    // `review-tool [PR]` typed in a terminal: hand the PR to the app and return.
    if cli::handle(&args) {
        return;
    }
    backend::exec::augment_path();
    forget_parent_agent_session();
    let launch = launch_pr();
    // Already running: give it the PR (or just bring it forward) and quit.
    let request = instance::Request {
        open_pr: args.iter().find_map(|a| a.strip_prefix("--open-pr=").map(String::from)),
        local_repo: args.iter().find_map(|a| a.strip_prefix("--local-repo=").map(String::from)),
    };
    if instance::hand_off(&request) {
        return;
    }
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        diffview::init(cx);
        pr_view::init(cx);
        theme::apply(cx);
        settings::init(cx);
        set_up_menus(cx);

        let bounds = Bounds::centered(None, size(px(1500.), px(950.)), cx);
        let app_view: std::rc::Rc<std::cell::RefCell<Option<WeakEntity<app::AppView>>>> = Default::default();
        let app_view_slot = app_view.clone();
        let window = cx.open_window(
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
                *app_view_slot.borrow_mut() = Some(view.downgrade());
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

        // Later launches (another `review-tool …`, or opening the app again) land here.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<instance::Request>();
        instance::listen(move |request| {
            let _ = tx.unbounded_send(request);
        });
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(request) = rx.next().await {
                let view = app_view.borrow().clone();
                window
                    .update(cx, |_, window, cx| {
                        cx.activate(true);
                        window.activate_window();
                        if let (Some(pr), Some(view)) = (request.pr(), view) {
                            view.update(cx, |view, cx| view.open(pr, window, cx)).ok();
                        }
                    })
                    .ok();
            }
        })
        .detach();
        cx.spawn(async move |_| backend::cleanup::on_startup().await).detach();
    });
}
