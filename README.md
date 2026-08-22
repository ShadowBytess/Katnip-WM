# Katnip

A Hyprland-inspired Wayland compositor for CachyOS, written in Rust on top of
[Smithay](https://github.com/Smithay/smithay). Part of the Luminous ecosystem
alongside [Lush](https://github.com/ShadowBytess/Lush) (shell),
[LumiTerm](https://github.com/ShadowBytess/LumiTerm) (terminal emulator), and
[Luminousity](https://github.com/ShadowBytess/Luminousity) (text editor).

## Status: M0 — skeleton / proof of life

Katnip currently runs **nested** inside an existing Wayland or X11 session and
renders a test background. No windows are managed yet.

## Roadmap

| Milestone | Scope |
|---|---|
| M0 | Workspace scaffolding, winit backend, colored background, logging |
| M1 | xdg_shell surfaces, keyboard focus, dwindling tiling, gaps + borders |
| M2 | Keybind engine, workspaces, floating toggle, mouse move/resize |
| M3 | TOML config (`katnip.conf.toml`), autostart, ecosystem defaults |
| M4 | Built-in status bar + `wlr-layer-shell` support |
| M5 | IPC socket + `katnipc` CLI |
| M6 | Plugins: Rhai (sandboxed) then native `.so` with ABI versioning |
| M7 | Polish: decorations, screenshots, idle protocols |
| M8 | Real session: DRM/udev/libinput backend, seatd/logind, packaging |

## Building & running (CachyOS/Arch)

```bash
sudo pacman -S --needed base-devel rustup pkgconf wayland libxkbcommon \
    libinput mesa seatd
rustup default stable
```

Run nested inside your current session:

```bash
RUST_LOG=debug cargo run -p katnip
```

A window appears rendering a solid Katnip-green background; keyboard and
pointer events are echoed to the log at debug level. Close the window to exit.

## Layout

| Crate | Purpose |
|---|---|
| `katnip` | Compositor binary: session entry point |
| `katnip-backend` | Smithay wrapper: nested winit now, DRM/libinput later |
| `katnip-core` | Pure WM logic: workspaces, layouts, focus (unit-testable) |
| `katnip-config` | TOML config model, validation, defaults |
| `katnip-api` | Stable plugin contract (traits, events, commands, ABI version) |
| `katnip-plugins` | Plugin host: Rhai runtime + native `.so` loader |
| `katnipc` | CLI control client over the Katnip IPC socket |

## License

[Apache-2.0](https://github.com/ShadowBytess/Katnip-WM/blob/main/LICENSE)
