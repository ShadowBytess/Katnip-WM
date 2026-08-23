//! Display/input backend layer for Katnip.
//!
//! Two backends share one entry point shape:
//! - [`nested`]: winit window inside an existing session (development)
//! - [`hardware`]: DRM/KMS + udev + libinput + libseat (real sessions)

pub mod hardware;
pub mod nested;

pub use nested::{NestedBackend, init_nested};

/// Convenience re-export of the renderer type used by all backends.
pub type KatnipRenderer = smithay::backend::renderer::gles::GlesRenderer;
