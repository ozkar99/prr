#![allow(dead_code)] // while the rewrite is in progress, parts of the backend aren't wired up yet

mod app;
mod backend;
mod diffview;
mod highlight;
mod home;
mod markdown;
mod pr_view;
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

fn main() {
    backend::exec::augment_path();
    let launch = launch_pr();
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_component::init(cx);
        diffview::init(cx);
        theme::apply(cx);

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
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open the window");
        cx.activate(true);
        cx.spawn(async move |_| backend::cleanup::on_startup().await).detach();
    });
}
