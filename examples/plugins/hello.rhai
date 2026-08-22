// Example Katnip Rhai plugin.
// Copy to ~/.config/katnip/plugins/ to load it at startup.

katnip.log("hello plugin loaded");

// Register a chord (config action grammar).
katnip.bind("SUPER+P", "exec sh -c 'echo katnip | dnotify 2>/dev/null || true'");

// Optional event hooks:
fn on_window_open(title, floating) {
    katnip.log(`window opened: "${title}" floating=${floating}`);
}

fn on_workspace_switch(id) {
    katnip.log(`switched to workspace ${id}`);
}
