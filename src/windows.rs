//! One window per PR. `review-tool <PR>` (and the home screen's Open) bring an
//! already-open PR's window forward, reuse a window that's on the home screen,
//! or open a new window.
use crate::app::AppView;
use crate::backend::types::PRRef;
use crate::instance::Request;
use gpui::*;
use gpui_component::Root;

actions!(review_tool, [NewWindow, CloseWindow, MinimizeWindow, NextWindow]);

#[derive(Default)]
struct Windows(Vec<(WindowHandle<Root>, WeakEntity<AppView>)>);

impl Global for Windows {}

pub fn init(cx: &mut App) {
    cx.set_global(Windows::default());
    cx.on_action(|_: &NewWindow, cx| {
        open(None, cx);
    });
    cx.on_action(|_: &NextWindow, cx| cycle(cx));
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("cmd-`", NextWindow, None),
    ]);
    // Like the web version, the app quits with its last window.
    cx.on_window_closed(|cx, _| {
        prune(cx);
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

fn prune(cx: &mut App) {
    let open: Vec<AnyWindowHandle> = cx.windows();
    cx.global_mut::<Windows>().0.retain(|(h, v)| open.contains(&(*h).into()) && v.upgrade().is_some());
}

/// Opens a window, on the home screen or loading `pr`.
pub fn open(pr: Option<PRRef>, cx: &mut App) -> Option<WindowHandle<Root>> {
    prune(cx);
    // Cascade from the newest window, like other Mac apps.
    let last = cx.global::<Windows>().0.last().map(|(h, _)| *h);
    let bounds = last
        .and_then(|h| h.update(cx, |_, window, _| window.bounds()).ok())
        .map(|b| Bounds::new(b.origin + point(px(28.), px(28.)), b.size))
        .unwrap_or_else(|| Bounds::centered(None, size(px(1500.), px(950.)), cx));
    let mut view_slot = None;
    let handle = cx
        .open_window(
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
                let view = cx.new(|cx| AppView::new(window, cx));
                if let Some(pr) = pr {
                    view.update(cx, |view, cx| view.open(pr, window, cx));
                }
                // Closing the window cleans up its PR (servers, agent, your clone).
                let weak = view.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    weak.update(cx, |view, cx| view.close_pr(cx)).ok();
                    true
                });
                view_slot = Some(view.downgrade());
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .ok()?;
    if let Some(view) = view_slot {
        cx.global_mut::<Windows>().0.push((handle, view));
    }
    cx.activate(true);
    Some(handle)
}

/// Handles a launch request (from the command line or a second launch).
pub fn route(request: Request, cx: &mut App) {
    prune(cx);
    cx.activate(true);
    let windows = cx.global::<Windows>().0.clone();
    let Some(pr) = request.pr() else {
        // Just "open the app": bring a window forward, or make one.
        match cx.active_window().or_else(|| windows.last().map(|(h, _)| (*h).into())) {
            Some(w) => {
                let _ = w.update(cx, |_, window, _| window.activate_window());
            }
            None => {
                open(None, cx);
            }
        }
        return;
    };
    let mut pr = pr;
    let key = (pr.owner.to_lowercase(), pr.repo.to_lowercase(), pr.number);
    // Your clone can only be on one PR: if another window is reviewing in it,
    // this PR uses the app's own checkout instead of switching it underneath.
    if let Some(local) = pr.local_repo.clone() {
        let busy = windows.iter().filter_map(|(_, v)| v.upgrade()).any(|v| {
            let v = v.read(cx);
            v.local_clone(cx).as_deref() == Some(local.as_str()) && v.showing(cx).as_ref() != Some(&key)
        });
        if busy {
            pr.local_repo = None;
        }
    }
    // Already open (or loading): bring that window forward.
    for (handle, view) in &windows {
        let Some(view) = view.upgrade() else { continue };
        if view.read(cx).showing(cx).as_ref() == Some(&key) {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
            return;
        }
    }
    // A window on the home screen takes it.
    for (handle, view) in &windows {
        let Some(view) = view.upgrade() else { continue };
        if view.read(cx).is_free(cx) {
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                view.update(cx, |view, cx| view.open(pr.clone(), window, cx));
            });
            return;
        }
    }
    open(Some(pr), cx);
}

/// ⌘`: the next window.
fn cycle(cx: &mut App) {
    prune(cx);
    let windows: Vec<AnyWindowHandle> = cx.global::<Windows>().0.iter().map(|(h, _)| (*h).into()).collect();
    if windows.len() < 2 {
        return;
    }
    let active = cx.active_window();
    let ix = active.and_then(|a| windows.iter().position(|w| *w == a)).unwrap_or(0);
    let next = windows[(ix + 1) % windows.len()];
    let _ = next.update(cx, |_, window, _| window.activate_window());
}

/// For the dev driver: each window and the PR it shows.
#[cfg(debug_assertions)]
pub fn describe(cx: &App) -> Vec<String> {
    cx.global::<Windows>()
        .0
        .iter()
        .filter_map(|(_, v)| v.upgrade())
        .map(|v| match v.read(cx).showing(cx) {
            Some((o, r, n)) => format!("{o}/{r}#{n}"),
            None => "home".into(),
        })
        .collect()
}
