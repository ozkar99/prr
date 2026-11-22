//! Test driver for development builds only (like the Tauri version's
//! devtools.rs): with REVIEW_TOOL_DEBUG_PORT=<port> set, POST a JSON command to
//! http://127.0.0.1:<port>/ and it is played into the app's own window, so the
//! UI can be exercised without real mouse/keyboard input. Not compiled into
//! release builds.
//!
//! Commands (`x`/`y` are window points, origin at the top-left of the content):
//!   {"op":"move","x":..,"y":..,"mods":"cmd"}     pointer move (hover)
//!   {"op":"click","x":..,"y":..,"mods":"cmd","button":"right","count":2}
//!   {"op":"drag","x":..,"y":..,"x2":..,"y2":..}  press, move, release
//!   {"op":"scroll","x":..,"y":..,"dy":-300}
//!   {"op":"key","keys":"cmd-l"}                   one keystroke
//!   {"op":"type","text":"hello"}                  text input
//!   {"op":"open","url":"https://github.com/o/r/pull/1"}
//!   {"op":"shot","path":"/tmp/x.png"}             screenshot of the window
//!   {"op":"info"}                                 window size and number
#![cfg(debug_assertions)]
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use gpui::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

type Request = (Value, oneshot::Sender<Value>);

pub fn start(cx: &mut App, open_pr: impl Fn(String, &mut Window, &mut App) + 'static) {
    let Some(port) = std::env::var("REVIEW_TOOL_DEBUG_PORT").ok().and_then(|p| p.parse::<u16>().ok()) else { return };
    let (tx, mut rx) = mpsc::unbounded::<Request>();
    std::thread::spawn(move || {
        let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else { return };
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                let _ = reader.read_exact(&mut body);
                let cmd: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let (rtx, rrx) = oneshot::channel();
                let _ = tx.unbounded_send((cmd, rtx));
                let result = futures::executor::block_on(rrx).unwrap_or(json!({ "error": "dropped" }));
                let body = result.to_string();
                let mut stream = stream;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            });
        }
    });
    let open_pr = std::rc::Rc::new(open_pr);
    cx.spawn(async move |cx| {
        while let Some((cmd, reply)) = rx.next().await {
            let open_pr = open_pr.clone();
            let result = cx.update(|cx| {
                let Some(handle) = cx.windows().into_iter().next() else { return json!({ "error": "no window" }) };
                handle
                    .update(cx, |_, window, cx| run(&cmd, window, cx, &*open_pr))
                    .unwrap_or_else(|e| json!({ "error": e.to_string() }))
            });
            let _ = reply.send(result);
        }
    })
    .detach();
}

fn modifiers(cmd: &Value) -> Modifiers {
    let mods = cmd["mods"].as_str().unwrap_or("");
    Modifiers {
        platform: mods.contains("cmd"),
        shift: mods.contains("shift"),
        alt: mods.contains("alt"),
        control: mods.contains("ctrl"),
        function: false,
    }
}

fn run(cmd: &Value, window: &mut Window, cx: &mut App, open_pr: &dyn Fn(String, &mut Window, &mut App)) -> Value {
    let at = point(px(cmd["x"].as_f64().unwrap_or(0.) as f32), px(cmd["y"].as_f64().unwrap_or(0.) as f32));
    let mods = modifiers(cmd);
    let button = match cmd["button"].as_str() {
        Some("right") => MouseButton::Right,
        _ => MouseButton::Left,
    };
    let ok = json!({ "ok": true });
    let result = match cmd["op"].as_str().unwrap_or("") {
        "move" => {
            window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: mods }), cx);
            ok
        }
        "click" => {
            let count = cmd["count"].as_u64().unwrap_or(1) as usize;
            window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: mods }), cx);
            for n in 1..=count {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent { button, position: at, modifiers: mods, click_count: n, first_mouse: false }),
                    cx,
                );
                window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button, position: at, modifiers: mods, click_count: n }), cx);
            }
            ok
        }
        "drag" => {
            let to = point(px(cmd["x2"].as_f64().unwrap_or(0.) as f32), px(cmd["y2"].as_f64().unwrap_or(0.) as f32));
            window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: None, modifiers: mods }), cx);
            window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button, position: at, modifiers: mods, click_count: 1, first_mouse: false }), cx);
            for i in 1..=10 {
                let p = at + (to - at) * (i as f32 / 10.);
                window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(button), modifiers: mods }), cx);
            }
            window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button, position: to, modifiers: mods, click_count: 1 }), cx);
            ok
        }
        "scroll" => {
            let dy = cmd["dy"].as_f64().unwrap_or(0.) as f32;
            let dx = cmd["dx"].as_f64().unwrap_or(0.) as f32;
            window.dispatch_event(
                PlatformInput::Scroll(ScrollEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                    modifiers: mods,
                    ..Default::default()
                }),
                cx,
            );
            ok
        }
        "key" => match Keystroke::parse_with_os(cmd["keys"].as_str().unwrap_or(""), OperatingSystem::Mac) {
            Ok(k) => json!({ "ok": true, "handled": window.dispatch_keystroke(k, cx) }),
            Err(e) => json!({ "error": e.to_string() }),
        },
        "type" => {
            for c in cmd["text"].as_str().unwrap_or("").chars() {
                let k = match c {
                    '\n' => Keystroke { modifiers: Modifiers::default(), key: "enter".into(), key_char: None },
                    c => Keystroke { modifiers: Modifiers { shift: c.is_uppercase(), ..Default::default() }, key: c.to_lowercase().to_string(), key_char: Some(c.to_string()) },
                };
                window.dispatch_keystroke(k, cx);
            }
            ok
        }
        "open" => {
            open_pr(cmd["url"].as_str().unwrap_or("").to_string(), window, cx);
            ok
        }
        "shot" => {
            let path = cmd["path"].as_str().unwrap_or("/tmp/review-tool-shot.png");
            match window_number(window) {
                Some(n) => {
                    let status = std::process::Command::new("/usr/sbin/screencapture").args(["-x", "-o", &format!("-l{n}"), path]).status();
                    json!({ "ok": status.map(|s| s.success()).unwrap_or(false), "path": path })
                }
                None => json!({ "error": "no window number" }),
            }
        }
        "info" => {
            let size = window.viewport_size();
            json!({ "width": f32::from(size.width), "height": f32::from(size.height), "scale": window.scale_factor(), "window": window_number(window) })
        }
        other => json!({ "error": format!("unknown op: {other}") }),
    };
    window.refresh();
    result
}

/// The macOS window number (for `screencapture -l`).
fn window_number(window: &Window) -> Option<i64> {
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
    let view = handle.ns_view.as_ptr() as *mut objc::runtime::Object;
    unsafe {
        let ns_window: *mut objc::runtime::Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let n: isize = msg_send![ns_window, windowNumber];
        Some(n as i64)
    }
}
