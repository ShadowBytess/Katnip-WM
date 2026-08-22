# Katnip

A Hyprland-inspired Wayland compositor for CachyOS, written from scratch in
Rust on top of [Smithay](https://github.com/Smithay/smithay). Part of the
Luminous ecosystem alongside [Lush](https://github.com/ShadowBytess/Lush)
(shell), [LumiTerm](https://github.com/ShadowBytess/LumiTerm) (terminal
emulator), and [Luminousity](https://github.com/ShadowBytess/Luminousity)
(text editor).

## Status: v0.1.0-alpha — working nested compositor

Katnip tiles real windows, manages workspaces, floats, keybinds, a status
bar, IPC, and loads both Rhai and native plugins. It runs **nested** inside
your existing Wayland/X11 session for development; hardware DRM/libinput
sessions are the next major milestone.

```
┌──────────────────────────────────────────────────────────┐
│ 1 2 3 4 5 6 7 8 9        window title              21:34 │  ← built-in bar
├────────────────────────────┬─────────────────────────────┤
│                            │                             │
│   dwindle tiling with      │      focused border         │
│   gaps + thin borders      │      teal / gray idle       │
│                            │                             │
└────────────────────────────┴─────────────────────────────┘
```

## Feature highlights

- **Dwindling tiling** (Hyprland-style BSP) with configurable outer/inner
  gaps and per-window borders; layout math is pure and unit-tested
- **9 workspaces** — `SUPER+1..9` to focus, `SUPER+SHIFT+1..9` to send a
  window there
- **Floating layer** — `SUPER+F` toggles; floats keep position, stack above
  tiles, and drag/resize freely
- **Mouse** — click-to-focus, `SUPER+LMB` drag-move (auto-floats tiled
  windows), `SUPER+RMB` quadrant resize
- **Built-in status bar** — workspace indicators, focused title, clock;
  rendered directly into the compositor's render pass
- **TOML config** at `~/.config/katnip/katnip.conf.toml`, auto-generated
  and documented on first run
- **IPC control socket** + `katnipc` CLI (`hyprctl` equivalent)
- **Hybrid plugin system** — sandboxed Rhai scripts and native `.so`
  libraries behind a versioned C ABI
- **Ecosystem defaults** — LumiTerm is the default terminal, Luminousity
  opens inside it via `SUPER+E`, children get `SHELL=/usr/bin/lush`

## Building & running (CachyOS/Arch)

```bash
sudo pacman -S --needed base-devel rustup pkgconf wayland libxkbcommon \
    libinput mesa seatd
rustup default stable
cargo build --release
```

Run nested inside your current session:

```bash
./target/release/katnip          # or: cargo run -p katnip
RUST_LOG=debug ./target/debug/katnip   # verbose input/layout logging
```

A window opens running Katnip. Launch clients against it with
`WAYLAND_DISPLAY=<name> <app>` using the name Katnip logs at startup, or
just press `SUPER+Enter` inside Katnip to spawn your terminal.

### Default keybinds

| Chord | Action |
|---|---|
| `SUPER+Enter` | launch terminal |
| `SUPER+E` | Luminousity in LumiTerm |
| `SUPER+Q` | close focused window |
| `SUPER+F` | toggle floating |
| `SUPER+1..9` | switch workspace |
| `SUPER+SHIFT+1..9` | move window to workspace |
| `SUPER+SHIFT+E` | quit Katnip |

All binds live in `katnip.conf.toml` under `[keybinds]`.

## Configuration

`~/.config/katnip/katnip.conf.toml` (created with comments on first run):

```toml
[general]
outer_gap = 8
inner_gap = 8
border_width = 2
terminal = "lumiterm"

[bar]
enabled = true
height = 28

[env]
SHELL = "/usr/bin/lush"
TERMINAL = "lumiterm"

[keybinds]
"SUPER+Return" = "exec lumiterm"
"SUPER+Q" = "close"
"SUPER+2" = "workspace 2"

[autostart]
exec = []
```

Actions: `exec <cmd>`, `close`, `toggle-floating`,
`workspace N`, `move-to-workspace N`, `quit`.
Restart Katnip after changes.

## IPC

```bash
katnipc ping
katnipc version
katnipc get workspaces            # JSON
katnipc get windows               # JSON
katnipc get activewindow          # JSON or null
katnipc dispatch exec alacritty   # any config action
```

The socket lives at `$XDG_RUNTIME_DIR/katnip/<wayland-display>.sock`;
`katnipc` finds it automatically (or set `KATNIP_SOCKET`).

## Plugins

Plugins live in `~/.config/katnip/plugins/`. Two flavors share one contract:

**Rhai scripts** (`*.rhai`) — sandboxed, no file/network access, CPU-capped:

```rhai
katnip.log("plugin loaded");
katnip.bind("SUPER+T", "exec foot");

fn on_window_open(title, floating) {
    katnip.log(`opened: ${title}`);
}

fn on_workspace_switch(id) {
    katnip.log(`workspace ${id}`);
}
```

**Native libraries** (`*.so`) — export `katnip_plugin_abi()` returning the
host's ABI version and receive a C API table in `katnip_plugin_init()`. See
`examples/plugins/native-example/`. The loader refuses ABI mismatches; a
broken `.so` is reported and skipped, never fatal.

See `examples/plugins/hello.rhai` for a runnable script example.

## Layout

| Crate | Purpose |
|---|---|
| `katnip` | Compositor binary: session assembly, input, bar, IPC, plugins |
| `katnip-backend` | Smithay wrapper: nested winit now, DRM/libinput later |
| `katnip-core` | Pure WM logic: layouts, keybinds, IPC paths (unit-tested) |
| `katnip-config` | TOML model, validation, action grammar, defaults |
| `katnip-api` | Stable plugin contract + native C ABI versioning |
| `katnip-plugins` | Plugin host: Rhai runtime + native loader |
| `katnipc` | CLI control client |

## Roadmap

- [x] M0 — workspace scaffolding, nested backend, render loop
- [x] M1 — xdg_shell windows, dwindling tiling, gaps + borders
- [x] M2 — keybind engine, workspaces, floating, mouse grabs
- [x] M3 — TOML config, autostart, ecosystem defaults
- [x] M4 — built-in status bar
- [x] M5 — IPC socket + `katnipc`
- [x] M6 — hybrid plugin system (Rhai + native `.so`)
- [ ] M7 — polish: screenshots (wlr-screencopy), idle protocols
- [ ] M8 — real session: DRM/udev/libinput backend, seatd/logind,
      display-manager entry, AUR packaging

## License

Apache-2.0
