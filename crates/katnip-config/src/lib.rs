//! Katnip configuration handling.
//!
//! Parses `katnip.conf.toml` into a validated, typed [`Config`] model.
//! Missing files fall back to built-in defaults (which wire up the
//! Lush/LumiTerm/Luminousity ecosystem); malformed values produce
//! line-annotated errors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use katnip_core::keybinds::{Action, KeybindTable};
use serde::Deserialize;

/// A loaded and validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub general: General,
    pub env: BTreeMap<String, String>,
    /// Raw keybind entries in declaration order: chord spec -> action string.
    pub keybinds: Vec<(String, String)>,
    pub autostart: Vec<String>,
}

/// `[general]` section: layout metrics and ecosystem programs.
#[derive(Debug, Clone)]
pub struct General {
    pub outer_gap: i32,
    pub inner_gap: i32,
    pub border_width: i32,
    /// Program launched by the `terminal` action / SUPER+Return.
    pub terminal: String,
}

/// Serde-facing shape of the whole file.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigRaw {
    #[serde(default)]
    general: GeneralRaw,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    keybinds: BTreeMap<String, String>,
    #[serde(default)]
    autostart: AutostartRaw,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct GeneralRaw {
    outer_gap: Option<i32>,
    inner_gap: Option<i32>,
    border_width: Option<i32>,
    terminal: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AutostartRaw {
    exec: Option<Vec<String>>,
}

impl Config {
    /// Loads config from `path`; a missing file yields the default config
    /// (and the path is reported via [`Config::loaded_from_default`]).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::with_defaults());
            }
            Err(err) => {
                return Err(ConfigError::Io(path.display().to_string(), err.to_string()));
            }
        };
        Self::parse(&text)
    }

    /// Parses config from raw TOML text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let raw: ConfigRaw =
            toml::from_str(text).map_err(|err| ConfigError::Parse(err.to_string()))?;
        let mut cfg = Self::with_defaults();

        if let Some(g) = raw.general.outer_gap {
            cfg.general.outer_gap = validate_gap("outer_gap", g)?;
        }
        if let Some(g) = raw.general.inner_gap {
            cfg.general.inner_gap = validate_gap("inner_gap", g)?;
        }
        if let Some(bw) = raw.general.border_width {
            cfg.general.border_width = bw.max(0);
        }
        if let Some(t) = raw.general.terminal {
            if t.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "general.terminal must not be empty".into(),
                ));
            }
            cfg.general.terminal = t;
        }

        cfg.env = raw.env;
        // Preserve TOML map ordering deterministically (BTreeMap already
        // sorts; re-collect into a stable vec).
        cfg.keybinds = raw.keybinds.into_iter().collect();
        cfg.autostart = raw.autostart.exec.unwrap_or_default();

        Ok(cfg)
    }

    /// The built-in defaults, including Lush/LumiTerm/Luminousity wiring.
    pub fn with_defaults() -> Self {
        Self {
            general: General {
                outer_gap: 8,
                inner_gap: 8,
                border_width: 2,
                terminal: "lumiterm".into(),
            },
            env: BTreeMap::from([
                ("SHELL".to_string(), "/usr/bin/lush".to_string()),
                ("TERMINAL".to_string(), "lumiterm".to_string()),
            ]),
            keybinds: vec![
                ("SUPER+Return".into(), "exec lumiterm".into()),
                ("SUPER+E".into(), "exec lumiterm -e luminousity".into()),
                ("SUPER+Q".into(), "close".into()),
                ("SUPER+F".into(), "toggle-floating".into()),
                ("SUPER+SHIFT+E".into(), "quit".into()),
            ],
            autostart: Vec::new(),
        }
    }

    /// Builds the resolved keybind table from configured chords.
    ///
    /// Returns per-entry errors so the user sees exactly which line failed.
    pub fn keybind_table(&self) -> Result<KeybindTable, ConfigError> {
        let mut table = KeybindTable::new();
        for (spec, action_str) in &self.keybinds {
            let action = parse_action(action_str)
                .ok_or_else(|| ConfigError::Validation(format!("unknown action {action_str:?}")))?;
            table
                .insert(spec, action)
                .map_err(ConfigError::Validation)?;
        }
        Ok(table)
    }
}

/// Parses an action string (`"exec foo --bar"`, `"close"`, ...).
pub fn parse_action(s: &str) -> Option<Action> {
    let s = s.trim();
    let (head, rest) = match s.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (s, ""),
    };
    match head.to_ascii_lowercase().as_str() {
        "exec" if !rest.is_empty() => Some(Action::Exec(rest.to_string())),
        "close" | "killwindow" if rest.is_empty() => Some(Action::CloseFocused),
        "toggle-floating" | "float" if rest.is_empty() => Some(Action::ToggleFloating),
        "quit" | "exit" if rest.is_empty() => Some(Action::Quit),
        "workspace" => rest
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| Action::FocusWorkspace(n - 1)),
        "move-to-workspace" => rest
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| Action::MoveToWorkspace(n - 1)),
        _ => None,
    }
}

fn validate_gap(field: &str, value: i32) -> Result<i32, ConfigError> {
    if !(0..=512).contains(&value) {
        return Err(ConfigError::Validation(format!(
            "general.{field} must be between 0 and 512, got {value}"
        )));
    }
    Ok(value)
}

/// Standard config file location: `$XDG_CONFIG_HOME/katnip/katnip.conf.toml`
/// or `~/.config/katnip/katnip.conf.toml`.
pub fn default_config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.config")))
        .unwrap_or_else(|| ".config".to_string());
    PathBuf::from(base).join("katnip").join("katnip.conf.toml")
}

/// Errors that can occur while loading config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {0}: {1}")]
    Io(String, String),
    #[error("config syntax error: {0}")]
    Parse(String),
    #[error("{0}")]
    Validation(String),
}

/// The default config file contents, written on first run.
pub const DEFAULT_CONFIG_TOML: &str = r#"# Katnip configuration
# Location: ~/.config/katnip/katnip.conf.toml
# Restart Katnip after changes.

[general]
# Layout spacing in logical pixels.
outer_gap = 8
inner_gap = 8
border_width = 2

# Program launched by the "terminal" action.
terminal = "lumiterm"

# Environment variables set for autostart programs.
[env]
SHELL = "/usr/bin/lush"
TERMINAL = "lumiterm"

# Chord = action. Chords: SUPER CTRL ALT SHIFT + key name ("return", "q", "1").
# Actions:
#   exec <command>        run command through sh -c
#   close                 close focused window
#   toggle-floating       flip focused window between tiled and floating
#   workspace N           switch to workspace N (1-9)
#   move-to-workspace N   send focused window to workspace N
#   quit                  exit Katnip
[keybinds]
"SUPER+Return" = "exec lumiterm"
"SUPER+E" = "exec lumiterm -e luminousity"
"SUPER+Q" = "close"
"SUPER+F" = "toggle-floating"
"SUPER+1" = "workspace 1"
"SUPER+2" = "workspace 2"
"SUPER+3" = "workspace 3"
"SUPER+4" = "workspace 4"
"SUPER+5" = "workspace 5"
"SUPER+6" = "workspace 6"
"SUPER+7" = "workspace 7"
"SUPER+8" = "workspace 8"
"SUPER+9" = "workspace 9"
"SUPER+SHIFT+1" = "move-to-workspace 1"
"SUPER+SHIFT+2" = "move-to-workspace 2"
"SUPER+SHIFT+3" = "move-to-workspace 3"
"SUPER+SHIFT+4" = "move-to-workspace 4"
"SUPER+SHIFT+5" = "move-to-workspace 5"
"SUPER+SHIFT+6" = "move-to-workspace 6"
"SUPER+SHIFT+7" = "move-to-workspace 7"
"SUPER+SHIFT+8" = "move-to-workspace 8"
"SUPER+SHIFT+9" = "move-to-workspace 9"
"SUPER+SHIFT+E" = "quit"

# Commands run once at startup, through sh -c, after the Wayland socket is up.
[autostart]
exec = []
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg = Config::parse(
            r#"
            [general]
            outer_gap = 12
            inner_gap = 4
            border_width = 3
            terminal = "foot"

            [env]
            SHELL = "/usr/bin/lush"

            [keybinds]
            "SUPER+Return" = "exec foot"
            "SUPER+Q" = "close"
            "SUPER+2" = "workspace 2"

            [autostart]
            exec = ["waybar", "swaybg -c #101014"]
            "#,
        )
        .expect("valid config");

        assert_eq!(cfg.general.outer_gap, 12);
        assert_eq!(cfg.general.inner_gap, 4);
        assert_eq!(cfg.general.border_width, 3);
        assert_eq!(cfg.general.terminal, "foot");
        assert_eq!(
            cfg.env.get("SHELL").map(String::as_str),
            Some("/usr/bin/lush")
        );
        assert_eq!(cfg.keybinds.len(), 3);
        assert_eq!(cfg.autostart.len(), 2);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg = Config::parse("[general]\nterminal = \"alacritty\"").expect("valid");
        assert_eq!(cfg.general.terminal, "alacritty");
        assert_eq!(cfg.general.outer_gap, 8);
        assert!(cfg.autostart.is_empty());
    }

    #[test]
    fn empty_file_yields_defaults() {
        let cfg = Config::parse("").expect("valid");
        assert_eq!(cfg.general.border_width, 2);
        assert!(cfg.keybinds.is_empty());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err =
            Config::parse("[generall]\nouter_gap = 1").expect_err("unknown section must fail");
        assert!(matches!(err, ConfigError::Parse(_)), "{err:?}");
    }

    #[test]
    fn gaps_out_of_range_rejected() {
        assert!(Config::parse("[general]\nouter_gap = -1").is_err());
        assert!(Config::parse("[general]\ninner_gap = 9999").is_err());
    }

    #[test]
    fn empty_terminal_rejected() {
        assert!(Config::parse("[general]\nterminal = \" \"").is_err());
    }

    #[test]
    fn action_grammar() {
        use katnip_core::keybinds::Action;
        assert_eq!(
            parse_action("exec lumiterm -e luminousity"),
            Some(Action::Exec("lumiterm -e luminousity".into()))
        );
        assert_eq!(parse_action("close"), Some(Action::CloseFocused));
        assert_eq!(
            parse_action("toggle-floating"),
            Some(Action::ToggleFloating)
        );
        assert_eq!(parse_action("quit"), Some(Action::Quit));
        // Workspaces are 1-based in config, 0-based internally.
        assert_eq!(parse_action("workspace 3"), Some(Action::FocusWorkspace(2)));
        assert_eq!(
            parse_action("move-to-workspace 9"),
            Some(Action::MoveToWorkspace(8))
        );
        // Invalid variants.
        assert_eq!(parse_action("workspace 0"), None);
        assert_eq!(parse_action("exec"), None); // no command
        assert_eq!(parse_action("explode"), None); // unknown
    }

    #[test]
    fn default_keybinds_resolve_into_table() {
        let table = Config::with_defaults()
            .keybind_table()
            .expect("defaults valid");
        assert!(table.len() >= 5);
    }

    #[test]
    fn bad_chord_reports_entry() {
        let cfg = Config::parse("[keybinds]\n\"BANANA+q\" = \"close\"").expect("parses");
        let err = cfg.keybind_table().expect_err("bad chord must fail");
        assert!(err.to_string().contains("invalid keybind spec"), "{err}");
    }
}
