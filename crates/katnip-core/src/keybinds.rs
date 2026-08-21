//! Keybind model: modifier chords parsed from spec strings plus an action
//! dispatch table.
//!
//! Deliberately dependency-free: chords store the keysym name (e.g.
//! `"return"`, `"1"`, `"q"`); the compositor resolves names to raw xkb
//! keysyms at load time via its own xkbcommon dependency.

/// Modifier set held during a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Mods {
    /// Parses one modifier token (`SUPER`, `CTRL`, `ALT`, `SHIFT`,
    /// case-insensitive).
    fn parse_token(token: &str) -> Option<&'static str> {
        match token.to_ascii_uppercase().as_str() {
            "SUPER" | "MOD4" => Some("logo"),
            "CTRL" | "CONTROL" => Some("ctrl"),
            "ALT" | "MOD1" => Some("alt"),
            "SHIFT" => Some("shift"),
            _ => None,
        }
    }

    /// Parses a full chord like `"SUPER+SHIFT+1"` into modifiers and the
    /// trailing key name. Returns `None` on empty input, unknown modifier,
    /// or an empty key segment.
    pub fn parse_chord(spec: &str) -> Option<(Self, String)> {
        let mut parts = spec.split('+').map(str::trim);
        let key = parts.next_back()?.to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        let mut mods = Self::default();
        for token in parts {
            let field = Self::parse_token(token)?;
            match field {
                "logo" => mods.logo = true,
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => unreachable!("parse_token only yields known fields"),
            }
        }
        Some((mods, key))
    }
}

/// What the compositor does when a chord fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Spawn a command through the shell-independent launcher.
    Exec(String),
    CloseFocused,
    ToggleFloating,
    FocusWorkspace(usize),
    MoveToWorkspace(usize),
    Quit,
}

/// Ordered chord-to-action table with last-insert-wins semantics.
#[derive(Debug, Clone, Default)]
pub struct KeybindTable {
    binds: Vec<(Mods, String, Action)>,
}

impl KeybindTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a bind; a later registration for the same chord replaces
    /// the earlier one.
    pub fn insert(&mut self, spec: &str, action: Action) -> Result<(), String> {
        let (mods, key) =
            Mods::parse_chord(spec).ok_or_else(|| format!("invalid keybind spec {spec:?}"))?;
        self.binds.retain(|(m, k, _)| !(m == &mods && k == &key));
        self.binds.push((mods, key, action));
        Ok(())
    }

    /// Resolves the action for a pressed chord, if any.
    pub fn lookup(&self, mods: &Mods, key_name: &str) -> Option<&Action> {
        let key_name = key_name.to_ascii_lowercase();
        self.binds
            .iter()
            .rev()
            .find(|(m, k, _)| m == mods && *k == key_name)
            .map(|(_, _, action)| action)
    }

    pub fn len(&self) -> usize {
        self.binds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.binds.is_empty()
    }

    /// Iterates all registered chords as (modifiers, key name, action).
    pub fn chords(&self) -> impl Iterator<Item = (&Mods, &str, &Action)> {
        self.binds
            .iter()
            .map(|(mods, key, action)| (mods, key.as_str(), action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_chord() {
        let (mods, key) = Mods::parse_chord("SUPER+Return").expect("valid");
        assert!(mods.logo);
        assert!(!mods.shift);
        assert_eq!(key, "return");
    }

    #[test]
    fn parses_multi_modifier_chord() {
        let (mods, key) = Mods::parse_chord("SUPER+SHIFT+1").expect("valid");
        assert!(mods.logo && mods.shift);
        assert!(!mods.ctrl);
        assert_eq!(key, "1");
    }

    #[test]
    fn case_and_alias_insensitive() {
        let a = Mods::parse_chord("mod4+Q");
        let b = Mods::parse_chord("SuPeR+q");
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(Mods::parse_chord("HYPER+q").is_none());
        assert!(Mods::parse_chord("").is_none());
        assert!(Mods::parse_chord("SUPER+").is_none());
    }

    #[test]
    fn table_lookup_is_exact_on_mods_and_key() {
        let mut table = KeybindTable::new();
        table
            .insert("SUPER+Return", Action::Exec("term".into()))
            .expect("valid spec");
        table
            .insert("SUPER+q", Action::CloseFocused)
            .expect("valid spec");

        let logo_only = Mods {
            logo: true,
            ..Default::default()
        };
        assert_eq!(
            table.lookup(&logo_only, "RETURN"),
            Some(&Action::Exec("term".into()))
        );
        assert_eq!(table.lookup(&logo_only, "q"), Some(&Action::CloseFocused));

        // Missing modifier or wrong key must not fire.
        let no_mods = Mods::default();
        assert_eq!(table.lookup(&no_mods, "return"), None);
        assert_eq!(table.lookup(&logo_only, "w"), None);

        // Extra modifiers held must not fire a chord without them.
        let logo_shift = Mods {
            logo: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(table.lookup(&logo_shift, "return"), None);
    }

    #[test]
    fn later_insert_replaces_earlier() {
        let mut table = KeybindTable::new();
        table
            .insert("SUPER+q", Action::CloseFocused)
            .expect("valid spec");
        table.insert("SUPER+q", Action::Quit).expect("valid spec");
        assert_eq!(table.len(), 1);
        let logo = Mods {
            logo: true,
            ..Default::default()
        };
        assert_eq!(table.lookup(&logo, "q"), Some(&Action::Quit));
    }
}
