//! xdg_shell handling: toplevel creation, popups, move/resize requests.

use smithay::delegate_xdg_shell;
use smithay::desktop::{PopupKind, Window, find_popup_root_surface, get_popup_toplevel_coords};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat as WlSeatResource;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use tracing::debug;

use crate::state::Katnip;

impl XdgShellHandler for Katnip {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        debug!("new toplevel");
        let window = Window::new_wayland_window(surface);
        self.add_tile(window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            debug!(?err, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, _surface: ToplevelSurface, seat: WlSeatResource, _serial: Serial) {
        // Floating move grabs arrive with M2; tiled windows ignore this.
        let _ = seat;
        debug!("move_request ignored (tiling mode)");
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        seat: WlSeatResource,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // Interactive resize arrives with M2; the layout owns tile sizes.
        let _ = seat;
        debug!("resize_request ignored (tiling mode)");
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeatResource, _serial: Serial) {
        // TODO(M2): popup keyboard grabs.
    }
}

delegate_xdg_shell!(Katnip);

impl Katnip {
    /// Positions a popup's unconstrained geometry relative to its parent so
    /// menus are not clipped by output edges.
    pub(super) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root))
        else {
            return;
        };

        let Some(output) = self.output.as_ref() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(window) else {
            return;
        };

        // The positioner geometry is relative to the parent toplevel.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
