//! Pseudo-terminals for the bottom panel (where Claude Code runs). Output goes
//! to the page as raw bytes over a Channel; input comes back via pty_write.
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};

struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

static PTYS: Mutex<Option<HashMap<u32, Pty>>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn with<T>(id: u32, f: impl FnOnce(&mut Pty) -> Result<T, String>) -> Result<T, String> {
    let mut map = PTYS.lock().unwrap();
    let pty = map.get_or_insert_with(HashMap::new).get_mut(&id).ok_or("terminal not running")?;
    f(pty)
}

pub fn spawn(
    app: AppHandle,
    program: &str,
    args: &[String],
    cwd: &str,
    cols: u16,
    rows: u16,
    on_data: Channel<InvokeResponseBody>,
) -> Result<u32, String> {
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // If the app itself was started from inside a Claude Code session, don't let
    // the embedded one think it's that session's child (it would, e.g., stop saving transcripts).
    for (key, _) in std::env::vars() {
        let session_marker = key == "CLAUDECODE"
            || key == "CLAUDE_PID"
            || key == "CLAUDE_EFFORT"
            || key == "CLAUDE_CODE_ENTRYPOINT"
            || key == "CLAUDE_CODE_EXECPATH"
            || ["CLAUDE_CODE_SESSION_", "CLAUDE_CODE_CHILD_", "CLAUDE_CODE_MESSAGING_"].iter().any(|p| key.starts_with(p));
        if session_marker {
            cmd.env_remove(&key);
        }
    }
    let child = pair.slave.spawn_command(cmd).map_err(|e| format!("{program}: {e}"))?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    PTYS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, Pty { master: pair.master, writer, child });

    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if on_data.send(InvokeResponseBody::Raw(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        PTYS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id);
        let _ = app.emit("pty:exit", id);
    });
    Ok(id)
}

pub fn write(id: u32, data: &str) -> Result<(), String> {
    with(id, |p| {
        p.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
        p.writer.flush().map_err(|e| e.to_string())
    })
}

pub fn resize(id: u32, cols: u16, rows: u16) -> Result<(), String> {
    with(id, |p| p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string()))
}

pub fn kill(id: u32) {
    if let Some(mut p) = PTYS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id) {
        let _ = p.child.kill();
    }
}

pub fn kill_all() {
    if let Some(map) = PTYS.lock().unwrap().as_mut() {
        for (_, mut p) in map.drain() {
            let _ = p.child.kill();
        }
    }
}
