//! Display/input backend layer for Katnip.
//!
//! Currently exposes the nested winit backend used for development inside an
//! existing Wayland/X11 session. Hardware DRM/libinput/udev backends are
//! added in a later milestone (M8) behind the same entry point.

pub mod nested;

pub use nested::{NestedBackend, init_nested};

/// Convenience re-export of the renderer type used by all backends.
pub type KatnipRenderer = smithay::backend::renderer::gles::GlesRenderer;
