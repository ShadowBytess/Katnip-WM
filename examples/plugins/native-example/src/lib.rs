//! Example native Katnip plugin.
//!
//! Build with `cargo build --release`, then copy
//! `target/release/libkatnip_native_example.so` into
//! `~/.config/katnip/plugins/` and start Katnip.

use std::ffi::CString;
use katnip_api::{KatnipNativeApi, KATNIP_ABI_VERSION};

static mut API: Option<&'static KatnipNativeApi> = None;

#[unsafe(no_mangle)]
pub extern "C" fn katnip_plugin_abi() -> u32 {
    KATNIP_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn katnip_plugin_init(api: *const KatnipNativeApi) {
    // SAFETY: the host guarantees the pointer is valid for init's duration;
    // we keep the referent alive for the process lifetime (host never unloads).
    unsafe {
        API = Some(&*api);

        let spec = CString::new("SUPER+O").unwrap_or_default();
        let action = CString::new("exec alacritty").unwrap_or_default();
        let api_ref = API.unwrap();
        (api_ref.bind)(api_ref.ctx, spec.as_ptr(), action.as_ptr());

        let hello = CString::new("native example plugin loaded").unwrap_or_default();
        (api_ref.log)((*api).ctx, hello.as_ptr());
    }
}
