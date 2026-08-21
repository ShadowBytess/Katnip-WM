//! Default keybind table and keysym resolution.
//!
//! M3 will replace the hardcoded defaults with TOML-sourced binds; the
//! resolution layer (chord spec -> raw xkb keysym) stays.

use katnip_core::keybinds::{Action, KeybindTable, Mods};
use std::collections::HashMap;

use crate::state::WORKSPACE_COUNT;

/// The built-in, Hyprland-flavored default binds.
pub fn default_table() -> KeybindTable {
    let mut table = KeybindTable::new();
    let mut bind = |spec: &str, action: Action| {
        table
            .insert(spec, action)
            .expect("default binds use valid specs");
    };

    bind("SUPER+Return", Action::Exec("terminal".into()));
    bind("SUPER+Q", Action::CloseFocused);
    bind("SUPER+F", Action::ToggleFloating);
    bind("SUPER+SHIFT+E", Action::Quit);
    for i in 0..WORKSPACE_COUNT {
        bind(&format!("SUPER+{}", i + 1), Action::FocusWorkspace(i));
        bind(
            &format!("SUPER+SHIFT+{}", i + 1),
            Action::MoveToWorkspace(i),
        );
    }

    table
}

/// A lookup-ready table: raw xkb keysym + modifiers -> action.
pub struct ResolvedBinds {
    by_chord: HashMap<(Mods, u32), Action>,
}

impl ResolvedBinds {
    /// Resolves every chord in `table` against xkb keysym names.
    pub fn build(table: &KeybindTable) -> Self {
        let mut by_chord = HashMap::new();
        for (mods, key, action) in table.chords() {
            match resolve_keysym(key) {
                Some(raw) => {
                    by_chord.insert((*mods, raw), action.clone());
                }
                None => tracing::warn!(%key, "unknown keysym name in keybind"),
            }
        }
        Self { by_chord }
    }

    pub fn lookup(&self, mods: &Mods, raw_keysym: u32) -> Option<&Action> {
        self.by_chord.get(&(*mods, raw_keysym))
    }
}

/// Resolves an xkb keysym name ("return", "1", "q") to its raw value.
fn resolve_keysym(name: &str) -> Option<u32> {
    use xkbcommon::xkb::{KEYSYM_CASE_INSENSITIVE, keysym_from_name};
    let sym = keysym_from_name(name, KEYSYM_CASE_INSENSITIVE);
    (sym.raw() != 0).then_some(sym.raw())
}
