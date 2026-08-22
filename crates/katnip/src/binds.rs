//! Keybind resolution: chord spec -> raw xkb keysym lookup table.

use std::collections::HashMap;

use katnip_core::keybinds::{Action, KeybindTable, Mods};

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

    pub fn len(&self) -> usize {
        self.by_chord.len()
    }
}

/// Resolves an xkb keysym name ("return", "1", "q") to its raw value.
fn resolve_keysym(name: &str) -> Option<u32> {
    use xkbcommon::xkb::{KEYSYM_CASE_INSENSITIVE, keysym_from_name};
    let sym = keysym_from_name(name, KEYSYM_CASE_INSENSITIVE);
    (sym.raw() != 0).then_some(sym.raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_chords() {
        let mut table = KeybindTable::new();
        table
            .insert("SUPER+Return", Action::Exec("term".into()))
            .expect("valid");
        table
            .insert("SUPER+q", Action::CloseFocused)
            .expect("valid");

        let resolved = ResolvedBinds::build(&table);
        assert_eq!(resolved.len(), 2);

        // Raw keysyms: Return = 0xff0d, q = 'q' as u32.
        let logo = Mods {
            logo: true,
            ..Default::default()
        };
        assert_eq!(
            resolved.lookup(&logo, 0xff0d),
            Some(&Action::Exec("term".into()))
        );
        assert_eq!(
            resolved.lookup(&logo, b'q' as u32),
            Some(&Action::CloseFocused)
        );
        assert_eq!(resolved.lookup(&logo, 0), None);
    }
}
