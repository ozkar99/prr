//! One app instance: a second launch (e.g. `review-tool owner/repo#1` while the
//! app is open) hands its PR to the running window over a Unix socket in the
//! app's data folder, then exits. (The Tauri version used a plugin for this.)
use crate::backend::exec::data_dir;
use crate::backend::types::PRRef;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Development builds get their own socket, so they never hand off to (or take
/// launches from) the installed app.
fn socket_path() -> PathBuf {
    data_dir().join(if cfg!(debug_assertions) { "instance-dev.sock" } else { "instance.sock" })
}

/// What a launch asks the running app to do: open a PR, or just come forward.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Request {
    pub open_pr: Option<String>,
    pub local_repo: Option<String>,
}

impl Request {
    pub fn pr(&self) -> Option<PRRef> {
        let mut pr = crate::backend::types::parse_pr_ref(self.open_pr.as_deref()?)?;
        pr.local_repo = self.local_repo.clone();
        Some(pr)
    }
}

/// Sends `request` to a running instance; true if one took it.
pub fn hand_off(request: &Request) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else { return false };
    let line = serde_json::to_string(request).unwrap_or_default();
    if writeln!(stream, "{line}").is_err() {
        return false;
    }
    // Wait for the acknowledgement so we only exit once it's been received.
    let mut ack = String::new();
    BufReader::new(stream).read_line(&mut ack).is_ok() && ack.trim() == "ok"
}

/// Becomes the instance: calls `on_request` (from a background thread) for each
/// later launch.
pub fn listen(on_request: impl Fn(Request) + Send + 'static) {
    let path = socket_path();
    let _ = std::fs::create_dir_all(data_dir());
    // A socket file left by a crashed instance: nobody answers on it.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok()
                && let Ok(request) = serde_json::from_str::<Request>(line.trim())
            {
                on_request(request);
                let _ = (&stream).write_all(b"ok\n");
            }
        }
    });
}
