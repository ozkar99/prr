//! Test hook for development builds only: with REVIEW_TOOL_DEBUG_EVAL=<port> set,
//! POST a JavaScript expression to http://127.0.0.1:<port>/ and get back its
//! JSON-encoded result from the main window. (WebKit has no Chrome-style
//! remote protocol to script the page.) Not compiled into release builds.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

static WAITERS: Mutex<Option<HashMap<u64, mpsc::Sender<String>>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

#[tauri::command]
pub fn debug_result(id: u64, value: String) {
    if let Some(tx) = WAITERS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id) {
        let _ = tx.send(value);
    }
}

pub fn start(app: &AppHandle) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(port) = std::env::var("REVIEW_TOOL_DEBUG_EVAL").ok().and_then(|p| p.parse::<u16>().ok()) else { return };
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else { return };
        for stream in listener.incoming().flatten() {
            let app = app.clone();
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
                let js = String::from_utf8_lossy(&body);

                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel();
                WAITERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, tx);
                let script = format!(
                    "(async () => {{ let v; try {{ v = JSON.stringify(await ({js})) ?? 'undefined' }} catch (e) {{ v = JSON.stringify('ERR ' + (e && e.stack || e)) }} window.__TAURI_INTERNALS__.invoke('debug_result', {{ id: {id}, value: v }}) }})()"
                );
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.eval(script);
                }
                let result = rx.recv_timeout(Duration::from_secs(300)).unwrap_or_else(|_| "\"timeout\"".into());
                let mut stream = stream;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", result.len(), result);
            });
        }
    });
}
