# Katnip

A Hyprland-inspired Wayland compositor for CachyOS, written from scratch in
Rust on top of [Smithay](https://github.com/Smithay/smithay). Part of the
Luminous ecosystem alongside [Lush](https://github.com/ShadowBytess/Lush)
(shell), [LumiTerm](https://github.com/ShadowBytess/LumiTerm) (terminal
emulator), and [Luminousity](https://github.com/ShadowBytess/Luminousity)
(text editor).

## Status: nested sessions are stable; hardware sessions land in M8

Katnip tiles real windows, manages workspaces, floats, keybinds, a status
bar, IPC, and loads both Rhai and native plugins.

Two run modes:

- **Nested** (default): runs as a window inside your current session — the
  daily development loop.
- **Hardware** (`--drm` or `KATNIP_BACKEND=drm`): drives DRM/KMS outputs,
  libinput devices, and a logind/seatd session directly. Alpha quality —
  single GPU, software dot cursor, continuous repaint.

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
- **10 workspaces** — `SUPER+1..9,0` to focus, `SUPER+SHIFT+1..9,0` to send
  a window there
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

### Nested (development)

```bash
./target/release/katnip          # or: cargo run -p katnip
RUST_LOG=debug ./target/debug/katnip   # verbose input/layout logging
```

A window opens running Katnip. Launch clients against it with
`WAYLAND_DISPLAY=<name> <app>` using the name Katnip logs at startup, or
just press `SUPER+Enter` inside Katnip to spawn your terminal.

### Hardware session (alpha)

From a TTY (not inside another compositor), with your user in the `seat`
group or seatd/logind active:

```bash
sudo systemctl enable --now seatd
sudo usermod -aG seat $USER   # re-login after this
KATNIP_BACKEND=drm ./target/release/katnip
```

Once stable, install the package (`packaging/PKGBUILD`) and pick **Katnip**
from your display manager; it launches `katnip --drm`.

### Default keybinds

| Chord | Action |
|---|---|
| `SUPER+Q` | launch terminal (alacritty) |
| `SUPER+E` | opal file manager in alacritty |
| `SUPER+S` | oview launcher |
| `SUPER+B` | waterfox browser |
| `SUPER+SHIFT+T` | fish shell in alacritty |
| `SUPER+C` | close focused window |
| `SUPER+F` | toggle floating |
| `Print` | screenshot region -> swappy (needs wlr-screencopy) |
| `SUPER+1..9,0` | switch workspace |
| `SUPER+SHIFT+1..9,0` | move window to workspace |
| `SUPER+SHIFT+E` | quit Katnip |

Defaults mirror the author's Hyprland keybind file. Mouse: click-to-focus,
`SUPER+LMB` drag-move, `SUPER+RMB` resize.

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
- [x] M7 — polish pass: xdg-decoration, focus refill, docs
- [x] M8 — hardware sessions: DRM/udev/libinput/libseat backend,
      `--drm` flag, session desktop entry, PKGBUILD (alpha)
- [ ] Post-M8: multi-GPU, xcursor pipeline, screenshots, idle/lock protocols

## License

Apache-2.0
