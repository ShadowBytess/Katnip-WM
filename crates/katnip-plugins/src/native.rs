//! Native `.so` plugin support behind a versioned C ABI.
//!
//! A native plugin is a `cdylib` exporting:
//!
//! ```c
//! uint32_t katnip_plugin_abi(void);
//! void katnip_plugin_init(const KatnipNativeApi *api);
//! ```
//!
//! The loader refuses plugins whose ABI version differs from the host's,
//! which prevents stale builds from corrupting memory. Libraries stay
//! loaded for the lifetime of the compositor (no unloading, by design).

use std::ffi::{CString, c_char};
use std::path::Path;

use katnip_api::{KATNIP_ABI_VERSION, KatnipNativeApi};

/// Collected registrations from one native plugin.
#[derive(Debug, Default, Clone)]
pub struct NativeRegistration {
    pub binds: Vec<(String, String)>,
}

/// Host-side context handed to the C callbacks.
struct NativeBridge {
    registration: NativeRegistration,
}

extern "C" fn bridge_log(ctx: *mut libc::c_void, msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let msg = unsafe { std::ffi::CStr::from_ptr(msg) };
    tracing::info!(target: "katnip::native_plugin", "{}", msg.to_string_lossy());
    let _ = ctx;
}

extern "C" fn bridge_bind(ctx: *mut libc::c_void, spec: *const c_char, action: *const c_char) {
    if spec.is_null() || action.is_null() {
        return;
    }
    let spec = unsafe { std::ffi::CStr::from_ptr(spec) }
        .to_string_lossy()
        .into_owned();
    let action = unsafe { std::ffi::CStr::from_ptr(action) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: ctx is the Box<NativeBridge> we passed at init.
    let bridge = unsafe { &mut *(ctx as *mut NativeBridge) };
    bridge.registration.binds.push((spec, action));
}

/// A loaded native plugin; keep alive forever (never unloaded).
pub struct NativePlugin {
    pub name: String,
    _library: libloading::Library,
}

/// Loads one native plugin and runs its init.
pub fn load(path: &Path) -> Result<(NativePlugin, NativeRegistration), String> {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().trim_start_matches("lib").to_owned())
        .unwrap_or_else(|| "plugin".into());

    // SAFETY: dlopen of a user-installed plugin; ABI handshake below gates
    // against stale builds. A malicious or buggy plugin can still crash us -
    // documented trade-off of native plugins.
    let library =
        unsafe { libloading::Library::new(path) }.map_err(|err| format!("dlopen failed: {err}"))?;

    let abi: libloading::Symbol<unsafe extern "C" fn() -> u32> =
        unsafe { library.get(b"katnip_plugin_abi") }
            .map_err(|_| "missing katnip_plugin_abi symbol".to_string())?;
    let reported = unsafe { abi() };
    if reported != KATNIP_ABI_VERSION {
        return Err(format!(
            "ABI mismatch: plugin reports {reported}, host expects {KATNIP_ABI_VERSION}"
        ));
    }

    let init: libloading::Symbol<unsafe extern "C" fn(*const KatnipNativeApi)> =
        unsafe { library.get(b"katnip_plugin_init") }
            .map_err(|_| "missing katnip_plugin_init symbol".to_string())?;

    let mut bridge = Box::new(NativeBridge {
        registration: NativeRegistration::default(),
    });
    let api = KatnipNativeApi {
        ctx: bridge.as_mut() as *mut NativeBridge as *mut _,
        log: bridge_log,
        bind: bridge_bind,
    };

    // Panic guard around third-party code where unwind boundaries allow.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { init(&api) }));
    if result.is_err() {
        return Err("plugin panicked during init".into());
    }

    let registration = std::mem::take(&mut bridge.registration);
    Ok((
        NativePlugin {
            name,
            _library: library,
        },
        registration,
    ))
}

/// Convenience for plugins/tests: converts a Rust string to a C string.
pub fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}
