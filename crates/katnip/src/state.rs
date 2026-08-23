//! Global compositor state: workspaces, tiles, focus, layout.

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use katnip_core::layout::{self, Rect};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::desktop::{PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{EventLoop, LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER, Size};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use tracing::{debug, info, warn};

/// Number of virtual workspaces (1-9, Hyprland style).
pub const WORKSPACE_COUNT: usize = 9;

/// Layout metrics from config, applied at runtime.
#[derive(Debug, Clone, Copy)]
pub struct LayoutMetrics {
    pub outer_gap: i32,
    pub inner_gap: i32,
    pub border_width: i32,
}

/// The xdg_toplevel state bit used for keyboard-focus indication.
const ACTIVATED: xdg_toplevel::State = xdg_toplevel::State::Activated;

pub struct CalloopData {
    pub state: Katnip,
    pub display_handle: DisplayHandle,
}

/// One managed window plus its per-workspace placement data.
#[derive(Debug, Clone)]
pub struct Tile {
    pub window: Window,
    pub floating: bool,
    /// Stored position for floating windows so re-arranges do not snap them
    /// back; `None` until the tile is first placed as a float.
    pub float_loc: Option<Point<i32, Logical>>,
}

impl Tile {
    fn new(window: Window) -> Self {
        Self {
            window,
            floating: false,
            float_loc: None,
        }
    }

    pub fn surface(&self) -> Option<&WlSurface> {
        self.window.toplevel().map(|t| t.wl_surface())
    }
}

/// IPC-facing snapshot of one managed window.
pub struct WindowEntry {
    pub address: String,
    pub title: Option<String>,
    pub class: Option<String>,
    pub floating: bool,
    pub workspace: usize,
    pub mapped: bool,
    pub focused: bool,
}

#[derive(Default)]
struct WorkspaceData {
    tiles: Vec<Tile>,
    focused: Option<Window>,
}

pub struct Katnip {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// Spatial view: window positions, stacking, output mapping.
    pub space: Space<Window>,
    /// Per-workspace tile lists; index 0 = workspace 1.
    workspaces: Vec<WorkspaceData>,
    pub active_workspace: usize,
    pub output: Option<Output>,
    pub damage_tracker: Option<OutputDamageTracker>,
    /// Last known geometry size per root surface, used to trigger
    /// re-arrangement when a client resizes.
    last_sizes: HashMap<WlSurface, (i32, i32)>,

    // Smithay protocol state.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    /// Owns the xdg_output global created at startup.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    /// xdg_decoration negotiation state (we always answer client-side).
    /// Held for the global's lifetime; read by the delegate macro.
    #[allow(dead_code)]
    pub decoration_state: smithay::wayland::shell::xdg::decoration::XdgDecorationState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
    /// Resolved keybind table (chord -> action).
    pub binds: Arc<crate::binds::ResolvedBinds>,
    /// Layout metrics from config.
    pub layout: LayoutMetrics,
    /// Terminal program used by the `terminal` action.
    pub terminal: String,
    /// Built-in status bar (owns textures; render via [`Katnip::bar`]).
    pub bar: crate::bar::Bar,
    /// Rhai script plugins; `None` when none loaded.
    pub plugins: Option<katnip_plugins::script::ScriptHost>,
    /// Hardware (DRM) backend state; `None` in nested mode.
    pub hw: Option<crate::hardware_mode::HwData>,
    /// dmabuf protocol state (hardware mode).
    pub dmabuf_state: DmabufState,
    /// Event-loop handle for timers; set by whichever backend runs.
    pub loop_handle: Option<LoopHandle<'static, CalloopData>>,
    /// Native `.so` plugins; kept loaded for the process lifetime.
    #[allow(dead_code)]
    pub native_plugins: Vec<katnip_plugins::native::NativePlugin>,
}

impl Katnip {
    pub fn new(
        event_loop: &mut EventLoop<CalloopData>,
        display: Display<Self>,
        binds: Arc<crate::binds::ResolvedBinds>,
        config: &katnip_config::Config,
        plugins: Option<katnip_plugins::script::ScriptHost>,
        native_plugins: Vec<katnip_plugins::native::NativePlugin>,
        hw: Option<crate::hardware_mode::HwData>,
    ) -> anyhow::Result<Self> {
        let start_time = Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let decoration_state =
            smithay::wayland::shell::xdg::decoration::XdgDecorationState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "katnip");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();

        let socket_name = init_wayland_listener(display, event_loop)?;
        let loop_signal = event_loop.get_signal();

        Ok(Self {
            start_time,
            socket_name,
            display_handle: dh,
            loop_signal,
            space: Space::default(),
            workspaces: (0..WORKSPACE_COUNT)
                .map(|_| WorkspaceData::default())
                .collect(),
            active_workspace: 0,
            output: None,
            damage_tracker: None,
            last_sizes: HashMap::new(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            decoration_state,
            seat_state,
            data_device_state,
            popups,
            seat,
            binds,
            layout: LayoutMetrics {
                outer_gap: config.general.outer_gap,
                inner_gap: config.general.inner_gap,
                border_width: config.general.border_width,
            },
            terminal: config.general.terminal.clone(),
            bar: crate::bar::Bar::new(config.bar.enabled, config.bar.height),
            plugins,
            native_plugins,
            hw,
            dmabuf_state: DmabufState::new(),
            loop_handle: None,
        })
    }

    // -- workspace / tile accessors --------------------------------------

    fn ws(&self) -> &WorkspaceData {
        &self.workspaces[self.active_workspace]
    }

    fn ws_mut(&mut self) -> &mut WorkspaceData {
        &mut self.workspaces[self.active_workspace]
    }

    /// All tracked windows across every workspace.
    fn all_tiles(&self) -> impl Iterator<Item = &Tile> {
        self.workspaces.iter().flat_map(|ws| ws.tiles.iter())
    }

    /// Finds the tile owning this root surface, searching every workspace.
    pub fn tile_for_surface(&self, root: &WlSurface) -> Option<&Tile> {
        self.all_tiles()
            .find(|t| t.surface().is_some_and(|s| s == root))
    }

    /// Legacy-style helper: the tracked Window for this root surface.
    pub fn window_for_surface(&self, root: &WlSurface) -> Option<&Window> {
        self.tile_for_surface(root).map(|t| &t.window)
    }

    /// Windows of the active workspace, for border rendering etc.
    pub fn active_windows(&self) -> impl Iterator<Item = &Window> {
        self.ws().tiles.iter().map(|t| &t.window)
    }

    /// The focused window of the active workspace.
    pub fn focused_window(&self) -> Option<Window> {
        self.ws().focused.clone()
    }

    /// Serializable view of every tracked window (for IPC).
    pub fn all_windows(&self) -> impl Iterator<Item = WindowEntry> + '_ {
        self.workspaces
            .iter()
            .enumerate()
            .flat_map(move |(ws_idx, ws)| {
                ws.tiles
                    .iter()
                    .map(move |tile| self.window_entry_of(tile, ws_idx))
            })
    }

    /// Serializable view of a single window.
    pub fn window_entry(&self, window: &Window) -> Option<WindowEntry> {
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            if let Some(tile) = ws.tiles.iter().find(|t| &t.window == window) {
                return Some(self.window_entry_of(tile, ws_idx));
            }
        }
        None
    }

    fn window_entry_of(&self, tile: &Tile, workspace: usize) -> WindowEntry {
        let surface = tile.surface().cloned();
        // Stable per-window identity for IPC consumers.
        let address = format!("0x{:x}", std::ptr::from_ref(&tile.window) as usize);
        let (title, class) = surface
            .map(|s| {
                smithay::wayland::compositor::with_states(&s, |states| {
                    let data = states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .expect("toplevel surface data")
                        .lock()
                        .expect("toplevel surface data lock");
                    (data.title.clone(), data.app_id.clone())
                })
            })
            .unwrap_or((None, None));
        WindowEntry {
            address,
            title,
            class,
            floating: tile.floating,
            workspace,
            mapped: workspace == self.active_workspace,
            focused: self.workspaces[workspace].focused.as_ref() == Some(&tile.window),
        }
    }

    /// Title of the focused window, if any.
    pub fn focused_title(&self) -> Option<String> {
        let surface = self.ws().focused.as_ref()?.toplevel()?.wl_surface().clone();
        Some(
            smithay::wayland::compositor::with_states(&surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .expect("toplevel surface data")
                    .lock()
                    .expect("toplevel surface data lock")
                    .title
                    .clone()
            })
            .unwrap_or_default(),
        )
    }

    // -- window lifecycle -------------------------------------------------

    /// Adds a new window as the last tile of the active workspace, focuses
    /// it, and re-arranges.
    pub fn add_tile(&mut self, window: Window) {
        info!(
            "new tile on workspace {} ({} total)",
            self.active_workspace + 1,
            self.ws().tiles.len() + 1
        );
        self.ws_mut().focused = Some(window.clone());
        self.ws_mut().tiles.push(Tile::new(window.clone()));
        let title_for_plugins = self.focused_title().unwrap_or_default();
        if let Some(host) = self.plugins.as_mut() {
            host.fire_window_open(&title_for_plugins, false);
        }
        self.arrange_force(false);
    }

    /// Removes dead windows everywhere, refills active focus, re-arranges.
    pub fn cleanup_dead(&mut self) {
        let mut removed = 0usize;
        for ws in &mut self.workspaces {
            let before = ws.tiles.len();
            ws.tiles.retain(|tile| tile.window.alive());
            removed += before - ws.tiles.len();
            if let Some(focused) = &ws.focused {
                if !focused.alive() {
                    ws.focused = None;
                }
            }
        }
        if removed == 0 {
            return;
        }
        debug!("removed {removed} dead tile(s)");

        let active_idx = self.active_workspace;
        if self.workspaces[active_idx].focused.is_none() {
            let next = self.workspaces[active_idx]
                .tiles
                .last()
                .map(|t| t.window.clone());
            self.focus_window(next.as_ref());
        } else {
            self.update_activated_states();
        }

        // Drop stale size entries so replacement surfaces map cleanly.
        let live: Vec<WlSurface> = self
            .all_tiles()
            .filter_map(|t| t.surface().cloned())
            .collect();
        self.last_sizes.retain(|surface, _| live.contains(surface));
        self.arrange_force(false);
    }

    /// Focuses a window (must belong to the active workspace): keyboard
    /// focus, Activated state, border recolor.
    pub fn focus_window(&mut self, window: Option<&Window>) {
        self.ws_mut().focused = window.cloned();

        let serial = SERIAL_COUNTER.next_serial();
        let surface = window
            .and_then(|w| w.toplevel())
            .map(|t| t.wl_surface().clone());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, serial);
        }
        self.update_activated_states();
    }

    /// Pushes the Activated xdg state to every active-workspace toplevel.
    pub fn update_activated_states(&mut self) {
        for tile in &self.ws().tiles {
            let Some(toplevel) = tile.window.toplevel() else {
                continue;
            };
            let active = self.ws().focused.as_ref() == Some(&tile.window);
            let is_active = toplevel.current_state().states.contains(ACTIVATED);
            if is_active != active {
                toplevel.with_pending_state(|state| {
                    if active {
                        state.states.set(ACTIVATED);
                    } else {
                        state.states.unset(ACTIVATED);
                    }
                });
                toplevel.send_pending_configure();
            }
        }
    }

    // -- layout -----------------------------------------------------------

    /// Recomputes tile geometry from the dwindling layout and applies it.
    ///
    /// `force_assert` re-sends sized configures even when the client already
    /// acked the right size - needed once right after first map, because
    /// some toolkits ack pre-map configures but open at their own preferred
    /// size anyway.
    pub fn arrange_force(&mut self, force_assert: bool) {
        let Some(output) = self.output.clone() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };

        let m = self.layout;
        let bar_reserve = if self.bar.enabled { self.bar.height } else { 0 };
        let usable = Rect::new(
            output_geo.loc.x + m.outer_gap,
            output_geo.loc.y + m.outer_gap + bar_reserve,
            (output_geo.size.w - 2 * m.outer_gap).max(0),
            (output_geo.size.h - 2 * m.outer_gap - bar_reserve).max(0),
        );

        let active = self.active_workspace;
        let tiled_count = self.workspaces[active]
            .tiles
            .iter()
            .filter(|t| !t.floating)
            .count();
        let rects = layout::dwindle(usable, tiled_count, m.inner_gap);
        debug!(
            output_geo = ?(output_geo.size.w, output_geo.size.h),
            ws = active + 1,
            tiled = tiled_count,
            "arrange"
        );
        let mut rect_iter = rects.iter();

        for i in 0..self.workspaces[active].tiles.len() {
            let tile = &mut self.workspaces[active].tiles[i];
            let window = tile.window.clone();

            if tile.floating {
                // Floating: keep stored position, assign a centered default
                // on first placement, never force sizes.
                let size = {
                    let geo = window.geometry().size;
                    if geo.w <= 0 || geo.h <= 0 {
                        Size::from((640, 480))
                    } else {
                        geo
                    }
                };
                let loc = *tile.float_loc.get_or_insert_with(|| {
                    Point::from((
                        usable.x + (usable.w - size.w).max(0) / 2,
                        usable.y + (usable.h - size.h).max(0) / 2,
                    ))
                });
                self.space.map_element(window, loc, false);
            } else {
                let Some(tile_rect) = rect_iter.next() else {
                    continue;
                };
                let inner = tile_rect.shrink(m.border_width);

                let Some(toplevel) = window.toplevel() else {
                    continue;
                };
                // Ask the client to match its tile size (xdg size-honoring
                // configure). Guarded so we do not configure-loop.
                let desired = Size::from((inner.w.max(1), inner.h.max(1)));
                let declared_ok = toplevel.current_state().size == Some(desired);
                if force_assert || !declared_ok {
                    debug!(?desired, "configuring tile size");
                    toplevel.with_pending_state(|state| {
                        state.size = Some(desired);
                    });
                    toplevel.send_pending_configure();
                }

                self.space
                    .map_element(window, Point::from((inner.x, inner.y)), false);
            }
        }

        // Floats render above tiled windows.
        for tile in &self.workspaces[active].tiles {
            if tile.floating {
                self.space.raise_element(&tile.window, true);
            }
        }

        self.update_activated_states();
    }

    /// Called on toplevel commits: records sizes and re-arranges on change.
    /// Handles windows on any workspace; only active-workspace changes
    /// trigger layout work.
    pub fn on_window_commit(&mut self, root: &WlSurface) {
        let Some(window) = self.window_for_surface(root).cloned() else {
            return;
        };

        window.on_commit();

        let initial_configure_sent = smithay::wayland::compositor::with_states(root, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .expect("toplevel surface data")
                .lock()
                .expect("toplevel surface data lock")
                .initial_configure_sent
        });

        if !initial_configure_sent {
            if let Some(toplevel) = window.toplevel() {
                debug!("sending initial configure");
                toplevel.send_configure();
            }
            return;
        }

        let size = window.geometry().size;
        let entry = (size.w, size.h);
        if self.last_sizes.get(root) != Some(&entry) {
            // Transition from unmapped/degenerate to real geometry = mapped.
            let was_unmapped = self
                .last_sizes
                .get(root)
                .is_none_or(|&(w, h)| w == 0 || h == 0);
            let first_map = was_unmapped && !size.is_empty();
            debug!(?size, first_map, "tile resized, re-arranging");
            self.last_sizes.insert(root.clone(), entry);
            let on_active = self
                .tile_for_surface(root)
                .is_some_and(|t| self.active_windows().any(|w| w == &t.window));
            if on_active {
                self.arrange_force(first_map);
            }
        }
    }

    // -- workspaces -------------------------------------------------------

    /// Switches the visible workspace, remapping clients as needed.
    pub fn switch_workspace(&mut self, idx: usize) {
        assert!(idx < WORKSPACE_COUNT);
        if idx == self.active_workspace {
            return;
        }
        debug!(
            from = self.active_workspace + 1,
            to = idx + 1,
            "switching workspace"
        );

        let windows_to_unmap: Vec<Window> =
            self.ws().tiles.iter().map(|t| t.window.clone()).collect();
        for window in windows_to_unmap {
            self.space.unmap_elem(&window);
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<WlSurface>::None, serial);
        }

        self.active_workspace = idx;
        self.arrange_force(true);

        match self.ws().focused.clone() {
            Some(focused) => self.focus_window(Some(&focused)),
            None => {
                // Arriving at a workspace nobody focused yet: take the last
                // tile so keybinds act immediately.
                if let Some(last) = self.ws().tiles.last().map(|t| t.window.clone()) {
                    self.focus_window(Some(&last));
                }
            }
        }
    }

    /// Moves the focused window to another workspace (it disappears from
    /// view unless target == active).
    pub fn move_focused_to_workspace(&mut self, idx: usize) {
        assert!(idx < WORKSPACE_COUNT);
        if idx == self.active_workspace {
            return;
        }
        let Some(focused) = self.ws().focused.clone() else {
            return;
        };
        let Some(pos) = self.ws().tiles.iter().position(|t| t.window == focused) else {
            return;
        };
        let mut tile = self.ws_mut().tiles.remove(pos);
        tile.float_loc = None;
        tile.floating = false;
        self.workspaces[idx].tiles.push(tile);
        self.workspaces[idx].focused = None;
        info!("moved window to workspace {}", idx + 1);

        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<WlSurface>::None, serial);
        }
        self.ws_mut().focused = None;
        self.update_activated_states();
        self.arrange_force(false);
    }

    // -- floating ---------------------------------------------------------

    /// Flips the focused window between tiled and floating.
    pub fn toggle_focused_floating(&mut self) {
        let Some(focused) = self.ws().focused.clone() else {
            return;
        };
        let Some(pos) = self.ws().tiles.iter().position(|t| t.window == focused) else {
            return;
        };

        let now_floating = !self.ws().tiles[pos].floating;
        self.ws_mut().tiles[pos].floating = now_floating;
        info!(floating = now_floating, "toggled floating");

        if now_floating {
            // Place under current pointer, clamped to the usable area.
            let loc = self.pointer_location();
            let size = focused.geometry().size;
            let w = if size.w > 0 { size.w } else { 640 };
            let h = if size.h > 0 { size.h } else { 480 };
            let clamped = Point::from((
                (loc.x as i32 - w / 2).max(self.layout.outer_gap),
                (loc.y as i32 - h / 2).max(self.layout.outer_gap),
            ));
            self.ws_mut().tiles[pos].float_loc = Some(clamped);
        } else {
            // Back to tiling: send to the end of the order, forget position.
            let tile = self.ws_mut().tiles.remove(pos);
            self.ws_mut().tiles.push(tile);
        }
        self.arrange_force(false);
    }

    /// Current pointer position in logical coordinates.
    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .map(|p| p.current_location())
            .unwrap_or_default()
    }

    /// Whether this window is currently floating.
    pub fn is_floating(&self, window: &Window) -> bool {
        self.ws()
            .tiles
            .iter()
            .find(|t| &t.window == window)
            .is_some_and(|t| t.floating)
    }

    /// Records a floating window's live position (called during move grabs).
    pub fn update_float_loc(&mut self, window: &Window, loc: Point<i32, Logical>) {
        for ws in &mut self.workspaces {
            if let Some(tile) = ws.tiles.iter_mut().find(|t| &t.window == window) {
                if tile.floating {
                    tile.float_loc = Some(loc);
                }
                return;
            }
        }
    }

    /// Converts a tiled window to floating without moving it to pointer.
    pub fn ensure_floating_at(&mut self, window: &Window, loc: Point<i32, Logical>) {
        let Some(pos) = self.ws().tiles.iter().position(|t| &t.window == window) else {
            return;
        };
        if !self.ws().tiles[pos].floating {
            self.ws_mut().tiles[pos].floating = true;
            self.ws_mut().tiles[pos].float_loc = Some(loc);
            self.arrange_force(false);
        }
    }

    // -- actions ----------------------------------------------------------

    /// Topmost surface under this position for pointer purposes.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
    }

    /// Closes the focused window via its toplevel close request.
    pub fn close_focused(&mut self) {
        if let Some(window) = &self.ws().focused {
            info!("closing focused window");
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            }
        }
    }

    /// Launches the configured terminal, falling back to common ones.
    pub fn spawn_terminal(&mut self) {
        let mut candidates = vec![self.terminal.clone()];
        if let Ok(env_term) = std::env::var("KATNIP_TERMINAL") {
            candidates.insert(0, env_term);
        }
        candidates.extend(["foot", "alacritty", "kitty"].iter().map(|s| s.to_string()));
        candidates.dedup();
        for term in candidates {
            match Command::new(&term).spawn() {
                Ok(child) => {
                    info!(%term, pid = child.id(), "launched terminal");
                    return;
                }
                Err(err) => warn!(%term, %err, "failed to launch terminal candidate"),
            }
        }
        warn!("no terminal found (tried KATNIP_TERMINAL, lumiterm, foot, alacritty, kitty)");
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Per-client protocol data.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

fn init_wayland_listener(
    display: Display<Katnip>,
    event_loop: &mut EventLoop<CalloopData>,
) -> anyhow::Result<OsString> {
    use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};

    let listening_socket = ListeningSocketSource::new_auto()?;
    let socket_name = listening_socket.socket_name().to_os_string();

    let loop_handle = event_loop.handle();
    loop_handle.insert_source(listening_socket, move |client_stream, _, state| {
        if let Err(err) = state
            .display_handle
            .insert_client(client_stream, Arc::new(ClientState::default()))
        {
            warn!(%err, "failed to insert new client");
        }
    })?;

    loop_handle.insert_source(
        Generic::new(display, Interest::READ, Mode::Level),
        |_, display, state| {
            // SAFETY: the display is owned by the event loop source and never
            // dropped while the loop runs.
            unsafe {
                display.get_mut().dispatch_clients(&mut state.state)?;
            }
            Ok(PostAction::Continue)
        },
    )?;

    Ok(socket_name)
}
