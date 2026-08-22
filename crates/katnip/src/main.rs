//! Katnip: a Hyprland-inspired Wayland compositor written in Rust.

mod bar;
mod binds;
mod grabs;
mod handlers;
mod input;
mod output;
mod state;
mod text;

use std::error::Error;
use std::sync::Arc;

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::state::{CalloopData, Katnip};

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Katnip starting (nested development mode)");

    let config = match load_config() {
        Ok(config) => config,
        Err(err) => {
            tracing::error!("{err}");
            tracing::warn!("continuing with default configuration");
            katnip_config::Config::with_defaults()
        }
    };
    let table = match config.keybind_table() {
        Ok(table) if !table.is_empty() => table,
        Ok(_) => {
            tracing::warn!("no keybinds configured; falling back to defaults");
            katnip_config::Config::with_defaults()
                .keybind_table()
                .expect("defaults are valid")
        }
        Err(err) => {
            tracing::error!("{err}");
            return Err(err.into());
        }
    };
    let resolved_binds = Arc::new(binds::ResolvedBinds::build(&table));
    info!("loaded {} keybinds", resolved_binds.len());

    // Environment for autostart children (and this process).
    for (key, value) in &config.env {
        // SAFETY: single-threaded startup, before any thread reads env.
        unsafe {
            std::env::set_var(key, value);
        }
    }

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;
    let display: Display<Katnip> = Display::new()?;
    let display_handle = display.handle();

    let state = Katnip::new(&mut event_loop, display, resolved_binds, &config)?;
    let mut data = CalloopData {
        state,
        display_handle,
    };

    output::init_winit(&mut event_loop, &mut data)?;

    info!(
        "listening on WAYLAND_DISPLAY={}",
        data.state.socket_name.display()
    );

    // Autostart runs once the socket is up so children connect to Katnip.
    for cmd in &config.autostart {
        spawn_autostart(cmd);
    }

    event_loop.run(None, &mut data, |data| {
        data.state.space.refresh();
        data.state.popups.cleanup();
        let _ = data.state.display_handle.flush_clients();
    })?;

    info!("Katnip exited cleanly");
    Ok(())
}

/// Loads config from the standard path, creating a documented default file
/// on first run so users have a starting point.
fn load_config() -> Result<katnip_config::Config, Box<dyn Error>> {
    let path = katnip_config::default_config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, katnip_config::DEFAULT_CONFIG_TOML)?;
        tracing::info!("wrote default config to {}", path.display());
    }
    Ok(katnip_config::Config::load(&path)?)
}

/// Spawns one autostart command through `sh -c`, detached.
fn spawn_autostart(cmd: &str) {
    match std::process::Command::new("sh").arg("-c").arg(cmd).spawn() {
        Ok(child) => info!(pid = child.id(), %cmd, "autostart"),
        Err(err) => warn!(%cmd, %err, "autostart failed"),
    }
}
