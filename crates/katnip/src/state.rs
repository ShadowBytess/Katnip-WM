//! Global compositor state: windows, focus, layout, smithay protocol states.

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;
use std::time::Instant;

use katnip_core::layout::{self, Rect};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::desktop::{PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{EventLoop, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use tracing::{debug, info, warn};

/// Outer margin between screen edges and the tiled area (logical px).
pub const OUTER_GAP: i32 = 8;
/// Gap between adjacent tiles (logical px).
pub const INNER_GAP: i32 = 8;
/// Border thickness drawn around each tile (logical px).
pub const BORDER_WIDTH: i32 = 2;

pub struct CalloopData {
    pub state: Katnip,
    pub display_handle: DisplayHandle,
}

pub struct Katnip {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// Spatial view: window positions, stacking, output mapping.
    pub space: Space<Window>,
    /// All tiled windows in layout order. Windows are "mapped" into the
    /// space during [`Katnip::arrange`].
    pub tiles: Vec<Window>,
    pub focused: Option<Window>,
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
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
}

impl Katnip {
    pub fn new(
        event_loop: &mut EventLoop<CalloopData>,
        display: Display<Self>,
    ) -> anyhow::Result<Self> {
        let start_time = Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
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
            tiles: Vec::new(),
            focused: None,
            output: None,
            damage_tracker: None,
            last_sizes: HashMap::new(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
        })
    }

    /// Adds a new window as the last tile and focuses it.
    pub fn add_tile(&mut self, window: Window) {
        info!("new tile ({} total)", self.tiles.len() + 1);
        self.focused = Some(window.clone());
        self.tiles.push(window);
        self.arrange_force(false);
    }

    /// Removes dead windows and refills focus; re-arranges if changed.
    pub fn cleanup_dead(&mut self) {
        let before = self.tiles.len();
        self.tiles.retain(|w| w.alive());
        if self.tiles.len() == before {
            return;
        }
        debug!("removed {} dead tile(s)", before - self.tiles.len());
        if let Some(focused) = &self.focused {
            if !focused.alive() {
                self.focused = None;
            }
        }
        let next_focus = self
            .focused
            .is_none()
            .then(|| self.tiles.last().cloned())
            .flatten();
        match next_focus {
            Some(window) => self.focus_window(Some(&window)),
            None => self.update_activated_states(),
        }
        // Drop stale size entries so a replacement surface maps cleanly.
        let live_surfaces: Vec<WlSurface> = self
            .tiles
            .iter()
            .filter_map(|w| w.toplevel().map(|t| t.wl_surface().clone()))
            .collect();
        self.last_sizes
            .retain(|surface, _| live_surfaces.contains(surface));
        self.arrange_force(false);
    }

    /// Focuses a window: keyboard focus + Activated state + border recolor.
    pub fn focus_window(&mut self, window: Option<&Window>) {
        self.focused = window.cloned();

        let serial = SERIAL_COUNTER.next_serial();
        let surface = window
            .and_then(|w| w.toplevel())
            .map(|t| t.wl_surface().clone());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, serial);
        }
        self.update_activated_states();
    }

    /// Pushes the Activated xdg state to every toplevel to match `focused`.
    pub fn update_activated_states(&mut self) {
        for w in &self.tiles {
            let Some(toplevel) = w.toplevel() else {
                continue;
            };
            let active = self.focused.as_ref() == Some(w);
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

    /// Recomputes tile geometry from the dwindling layout and applies it:
    /// positions windows in the space, requests client sizes, draws borders.
    ///
    /// `force_assert` re-sends sized configures even when the client has
    /// already acked the right size - needed once right after first map,
    /// because some toolkits ack pre-map configures but map at their own
    /// preferred size anyway.
    pub fn arrange_force(&mut self, force_assert: bool) {
        let Some(output) = self.output.clone() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };

        let usable = Rect::new(
            output_geo.loc.x + OUTER_GAP,
            output_geo.loc.y + OUTER_GAP,
            (output_geo.size.w - 2 * OUTER_GAP).max(0),
            (output_geo.size.h - 2 * OUTER_GAP).max(0),
        );
        let rects = layout::dwindle(usable, self.tiles.len(), INNER_GAP);
        debug!(
            output_geo = ?(output_geo.size.w, output_geo.size.h),
            tiles = self.tiles.len(),
            "arrange"
        );

        for (window, tile) in self.tiles.iter().zip(rects.iter()) {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            let inner = tile.shrink(BORDER_WIDTH);

            // Ask the client to match its tile size (xdg size-honoring
            // configure). Guarded so we do not configure-loop.
            let desired = smithay::utils::Size::from((inner.w.max(1), inner.h.max(1)));
            let declared_ok = toplevel.current_state().size == Some(desired);
            if force_assert || !declared_ok {
                debug!(?desired, "configuring tile size");
                toplevel.with_pending_state(|state| {
                    state.size = Some(desired);
                });
                toplevel.send_pending_configure();
            }

            self.space
                .map_element(window.clone(), Point::from((inner.x, inner.y)), false);
        }

        self.update_activated_states();
    }

    /// Called on toplevel commits: re-arranges when a window's size changes.
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
            // Transition from unmapped/degenerate to a real geometry = the
            // window just mapped; some toolkits ack pre-map configures but
            // open at their preferred size, so force one re-assert.
            let was_unmapped = self
                .last_sizes
                .get(root)
                .is_none_or(|&(w, h)| w == 0 || h == 0);
            let first_map = was_unmapped && !size.is_empty();
            debug!(?size, first_map, "tile resized, re-arranging");
            self.last_sizes.insert(root.clone(), entry);
            self.arrange_force(first_map);
        }
    }

    /// Finds the tracked window whose toplevel owns this root surface.
    pub fn window_for_surface(&self, root: &WlSurface) -> Option<&Window> {
        self.tiles
            .iter()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == root))
    }

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
        if let Some(window) = &self.focused {
            info!("closing focused window");
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            }
        }
    }

    /// Demo terminal launcher until the M2/M3 keybind+config engine exists.
    pub fn spawn_terminal(&mut self) {
        let candidates: Vec<String> = std::env::var("KATNIP_TERMINAL")
            .map(|t| vec![t])
            .unwrap_or_else(|_| {
                ["lumiterm", "foot", "alacritty", "kitty"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            });
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

/// The xdg_toplevel state bit used for keyboard-focus indication.
const ACTIVATED: xdg_toplevel::State = xdg_toplevel::State::Activated;

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
            .insert_client(client_stream, std::sync::Arc::new(ClientState::default()))
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
