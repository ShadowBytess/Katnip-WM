//! Katnip plugin host.
//!
//! Loads and supervises plugins in two flavors: sandboxed Rhai scripts
//! (hot-reloadable) and native `.so` dylibs behind a C-ABI vtable with
//! version handshake. A crashing plugin is disabled; Katnip survives.
//! Populated in M6.
