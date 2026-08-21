//! Katnip: a Hyprland-inspired Wayland compositor written in Rust.

mod binds;
mod grabs;
mod handlers;
mod input;
mod output;
mod state;

use std::error::Error;
use std::sync::Arc;

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::{CalloopData, Katnip};

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Katnip starting (nested development mode)");

    let resolved_binds = Arc::new(binds::ResolvedBinds::build(&binds::default_table()));

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;
    let display: Display<Katnip> = Display::new()?;
    let display_handle = display.handle();

    let state = Katnip::new(&mut event_loop, display, resolved_binds)?;
    let mut data = CalloopData {
        state,
        display_handle,
    };

    output::init_winit(&mut event_loop, &mut data)?;

    info!(
        "listening on WAYLAND_DISPLAY={}",
        data.state.socket_name.display()
    );

    event_loop.run(None, &mut data, |data| {
        data.state.space.refresh();
        data.state.popups.cleanup();
        let _ = data.state.display_handle.flush_clients();
    })?;

    info!("Katnip exited cleanly");
    Ok(())
}
