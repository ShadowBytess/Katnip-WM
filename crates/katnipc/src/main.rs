//! katnipc: control client for the Katnip compositor.
//!
//! Usage: `katnipc <command> [args...]`
//! Examples:
//!   katnipc ping
//!   katnipc get workspaces
//!   katnipc get windows
//!   katnipc dispatch exec lumiterm
//!   katnipc dispatch workspace 3

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

fn main() -> ExitCode {
    let command = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        eprintln!("usage: katnipc <command>");
        eprintln!(
            "commands: ping | version | dispatch <action> | get <workspaces|windows|activewindow>"
        );
        return ExitCode::from(2);
    }

    let socket = match katnip_core::ipc::discover_socket() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("katnipc: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("katnipc: cannot connect to {}: {err}", socket.display());
            return ExitCode::FAILURE;
        }
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));

    if let Err(err) = stream
        .write_all(command.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
    {
        eprintln!("katnipc: send failed: {err}");
        return ExitCode::FAILURE;
    }

    let mut response = String::new();
    if let Err(err) = stream.read_to_string(&mut response) {
        eprintln!("katnipc: read failed: {err}");
        return ExitCode::FAILURE;
    }
    let response = response.trim_end();
    println!("{response}");

    if response.starts_with("ERR") {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
