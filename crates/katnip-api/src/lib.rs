//! Katnip plugin API contract.
//!
//! The stable surface plugins program against. Two flavors share this
//! contract:
//!
//! - **Rhai scripts** (`~/.config/katnip/plugins/*.rhai`) get a global
//!   `katnip` object with `log(msg)` and `bind(chord, action)`, and may
//!   define optional event hooks: `on_window_open(title, floating)` and
//!   `on_workspace_switch(id)`.
//!
//! - **Native `.so` plugins** export `katnip_plugin_abi() -> u32` (must
//!   equal [`KATNIP_ABI_VERSION`]) and receive a [`KatnipNativeApi`] table
//!   of C function pointers in `katnip_plugin_init`.

/// ABI version for native plugins. Bump on any layout/semantic change;
/// the loader refuses mismatched plugins.
pub const KATNIP_ABI_VERSION: u32 = 1;

use std::os::raw::{c_char, c_void};

/// Opaque host context pointer passed back into every callback.
pub type NativeCtx = *mut c_void;

/// C-callable API handed to native plugins at init time.
#[repr(C)]
pub struct KatnipNativeApi {
    /// Host context, passed as the first argument of every callback.
    pub ctx: NativeCtx,
    /// Logs a message from the plugin.
    pub log: extern "C" fn(ctx: NativeCtx, msg: *const c_char),
    /// Registers a keybind chord ("SUPER+T") mapped to an action string
    /// using the config action grammar ("exec foo", "close", ...).
    pub bind: extern "C" fn(ctx: NativeCtx, spec: *const c_char, action: *const c_char),
}

/// What a plugin did during registration, collected by the host.
#[derive(Debug, Default, Clone)]
pub struct PluginRegistration {
    pub binds: Vec<(String, String)>,
}
