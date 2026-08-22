//! Control IPC: a Unix socket accepting one command per connection.
//!
//! Protocol (newline-terminated, response also newline-terminated):
//! - `ping`                    -> `pong`
//! - `version`                 -> JSON version object
//! - `dispatch <action>`       -> `ok` (action uses the config grammar)
//! - `get workspaces`          -> JSON array
//! - `get windows`             -> JSON array
//! - `get activewindow`        -> JSON object or `null`
//!
//! Errors are reported as `ERR <message>`.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
use tracing::{debug, info, warn};

use katnip_config::parse_action;

use crate::state::Katnip;

/// Binds the control socket and registers it with the event loop.
pub fn init_ipc(
    event_loop: &smithay::reexports::calloop::LoopHandle<'static, crate::state::CalloopData>,
    wayland_display: &str,
) -> anyhow::Result<PathBuf> {
    let dir = katnip_core::ipc::runtime_dir();
    std::fs::create_dir_all(&dir)?;
    let path = katnip_core::ipc::socket_path(wayland_display);
    let _ = std::fs::remove_file(&path); // clear stale socket from a crash

    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    info!(socket = %path.display(), "IPC listening");

    event_loop.insert_source(
        Generic::new(listener, Interest::READ, Mode::Level),
        |_readiness, listener, data| {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(&mut data.state, stream),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => {
                        warn!(%err, "ipc accept failed");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    )?;

    Ok(path)
}

/// Removes the socket file on clean shutdown.
pub fn cleanup(path: &PathBuf) {
    if let Err(err) = std::fs::remove_file(path) {
        debug!(%err, "socket cleanup");
    }
}

fn handle_connection(state: &mut Katnip, mut stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(250)));
    let mut line = String::new();
    let mut byte = [0u8; 1];
    // Read one line, bounded by the timeout and a size cap.
    while line.len() < 4096 {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0] as char);
            }
        }
    }

    let response = handle_command(state, line.trim());
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(b"\n");
}

fn handle_command(state: &mut Katnip, cmd: &str) -> String {
    debug!(command = %cmd, "ipc");
    if cmd.is_empty() {
        return "ERR empty command".into();
    }
    let (verb, rest) = match cmd.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim()),
        None => (cmd, ""),
    };

    match verb {
        "ping" => "pong".into(),
        "version" => serde_json::json!({
            "name": "katnip",
            "version": env!("CARGO_PKG_VERSION"),
            "branch": option_env!("GIT_BRANCH").unwrap_or("main"),
        })
        .to_string(),
        "dispatch" => match parse_action(rest) {
            Some(action) => {
                state.execute_action(&action);
                "ok".into()
            }
            None => format!("ERR unknown action {rest:?}"),
        },
        "get" => get_query(state, rest),
        other => format!(
            "ERR unknown command {other:?} (try: ping, version, dispatch <action>, get <workspaces|windows|activewindow>)"
        ),
    }
}

fn get_query(state: &mut Katnip, query: &str) -> String {
    match query {
        "workspaces" => serde_json::to_string(
            &(0..crate::state::WORKSPACE_COUNT)
                .map(|i| {
                    serde_json::json!({
                        "id": i + 1,
                        "active": i == state.active_workspace,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .expect("workspace json"),
        "windows" => serde_json::to_string(
            &state
                .all_windows()
                .map(|entry| {
                    serde_json::json!({
                        "address": entry.address,
                        "title": entry.title,
                        "class": entry.class,
                        "floating": entry.floating,
                        "workspace": entry.workspace + 1,
                        "mapped": entry.mapped,
                        "focused": entry.focused,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .expect("window json"),
        "activewindow" => state
            .focused_window()
            .and_then(|w| {
                state.window_entry(&w).map(|e| {
                    serde_json::json!({
                        "address": e.address,
                        "title": e.title,
                        "class": e.class,
                        "floating": e.floating,
                        "workspace": e.workspace + 1,
                    })
                })
            })
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        other => {
            format!("ERR unknown get query {other:?} (try: workspaces, windows, activewindow)")
        }
    }
}
