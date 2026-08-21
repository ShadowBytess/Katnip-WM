//! Display/input backend layer for Katnip.
//!
//! Currently exposes the nested winit backend used for development inside an
//! existing Wayland/X11 session. Hardware DRM/libinput/udev backends are
//! added in a later milestone (M8) behind the same entry point.

mod nested;

/// Runs Katnip nested inside the current session (Wayland or X11) via winit.
pub fn run_nested() -> anyhow::Result<()> {
    nested::run()
}
