//! Katnip plugin host.
//!
//! Loads and supervises plugins in two flavors: sandboxed Rhai scripts
//! (hot-reload planned) and native `.so` dylibs behind a C-ABI vtable with
//! version handshake. A failing script is reported and skipped; Katnip
//! survives everything short of a native crash.

pub mod native;
pub mod script;

use std::path::{Path, PathBuf};

/// Where user plugins live: `$XDG_CONFIG_HOME/katnip/plugins`.
pub fn plugin_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))
        .unwrap_or_else(|| ".config".to_string());
    PathBuf::from(base).join("katnip").join("plugins")
}

/// Result of loading one plugin file.
#[derive(Debug)]
pub struct LoadedPlugin {
    pub name: String,
    pub path: PathBuf,
}

/// Loads every `.rhai` script in `dir`, returning the host plus the binds
/// scripts registered at load time.
pub fn load_scripts(dir: &Path) -> (script::ScriptHost, Vec<(String, String)>, Vec<String>) {
    let mut host = script::ScriptHost::default();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (host, Vec::new(), errors);
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rhai"))
        .collect();
    files.sort();

    for path in files {
        match host.load(&path) {
            Ok(binds) => {
                tracing::info!(plugin = %path.display(), "loaded rhai plugin");
                // load() already aggregated into host.binds; validate here
                // so bad entries are reported without aborting the rest.
                for (spec, action) in &binds {
                    if let Err(err) = validate_chord_and_action(spec, action) {
                        errors.push(format!("{}: {err}", path.display()));
                    }
                }
            }
            Err(err) => {
                tracing::warn!(plugin = %path.display(), %err, "rhai plugin failed");
                errors.push(format!("{}: {err}", path.display()));
            }
        }
    }
    let binds = host.binds.clone();
    (host, binds, errors)
}

fn validate_chord_and_action(spec: &str, action: &str) -> Result<(), String> {
    katnip_core::keybinds::Mods::parse_chord(spec)
        .ok_or_else(|| format!("invalid chord spec {spec:?}"))?;
    katnip_config::parse_action(action).ok_or_else(|| format!("unknown action {action:?}"))?;
    Ok(())
}

/// Loads every `.so`/`.dylib` native plugin in `dir`.
///
/// Returned plugins must be kept alive for the process lifetime (the loader
/// deliberately never unloads libraries); bindings collected during init are
/// validated the same way script binds are.
pub fn load_natives(
    dir: &Path,
) -> (Vec<native::NativePlugin>, Vec<(String, String)>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut binds = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (plugins, binds, errors);
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("so") | Some("dylib")
            )
        })
        .collect();
    files.sort();

    for path in files {
        match native::load(&path) {
            Ok((plugin, registration)) => {
                tracing::info!(plugin = %path.display(), "loaded native plugin");
                for (spec, action) in &registration.binds {
                    if let Err(err) = validate_chord_and_action(spec, action) {
                        errors.push(format!("{}: {err}", path.display()));
                    }
                }
                binds.extend(registration.binds);
                plugins.push(plugin);
            }
            Err(err) => {
                tracing::warn!(plugin = %path.display(), %err, "native plugin refused");
                errors.push(format!("{}: {err}", path.display()));
            }
        }
    }
    (plugins, binds, errors)
}
