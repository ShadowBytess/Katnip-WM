//! Nested winit backend.
//!
//! Provides the graphics backend and winit event source as separate pieces
//! so the compositor crate can insert the event source into its own calloop
//! loop alongside the Wayland display.

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{WinitEventLoop, WinitGraphicsBackend};

/// The nested backend: a window in the host session plus its event stream.
pub struct NestedBackend {
    /// Renders into and presents on the host window.
    pub graphics: WinitGraphicsBackend<GlesRenderer>,
    /// Calloop event source delivering `WinitEvent`s.
    pub events: WinitEventLoop,
}

/// Initializes the nested backend inside the current Wayland/X11 session.
pub fn init_nested() -> anyhow::Result<NestedBackend> {
    let (graphics, events) = smithay::backend::winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("failed to initialize winit backend: {err}"))?;
    Ok(NestedBackend { graphics, events })
}
