//! Shared IPC socket path resolution for Katnip and katnipc.

use std::path::PathBuf;

/// Base directory holding Katnip runtime sockets:
/// `$XDG_RUNTIME_DIR/katnip`.
pub fn runtime_dir() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(runtime).join("katnip")
}

/// Socket path for a given Wayland display name.
pub fn socket_path(wayland_display: &str) -> PathBuf {
    runtime_dir().join(format!("{wayland_display}.sock"))
}

/// Finds the socket to talk to: `$KATNIP_SOCKET` if set, otherwise the only
/// `*.sock` in the runtime dir. Returns an error string when ambiguous.
pub fn discover_socket() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("KATNIP_SOCKET") {
        return Ok(PathBuf::from(explicit));
    }
    let dir = runtime_dir();
    let mut socks: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|err| format!("cannot read {}: {err}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sock"))
        .collect();
    match socks.len() {
        0 => Err(format!(
            "no Katnip socket found in {} (is Katnip running?)",
            dir.display()
        )),
        1 => Ok(socks.swap_remove(0)),
        _ => {
            socks.sort();
            Err(format!(
                "multiple Katnip instances found; pick one via KATNIP_SOCKET: {:?}",
                socks
            ))
        }
    }
}
