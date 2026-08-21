//! wl_compositor / wl_shm handling: buffer tracking and commit routing.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::{PopupKind, PopupManager};
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{delegate_compositor, delegate_shm};

use crate::state::{ClientState, Katnip};

impl CompositorHandler for Katnip {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("client data")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if self.window_for_surface(&root).is_some() {
                crate::grabs::resize_grab::handle_commit(self, &root);
                self.on_window_commit(&root);
            }
        };

        handle_popup_commit(&mut self.popups, surface);
    }
}

impl BufferHandler for Katnip {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for Katnip {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(Katnip);
delegate_shm!(Katnip);

/// Tracks popup commits and sends their initial configure.
fn handle_popup_commit(popups: &mut PopupManager, surface: &WlSurface) {
    popups.commit(surface);
    if let Some(PopupKind::Xdg(xdg)) = popups.find_popup(surface) {
        if !xdg.is_initial_configure_sent() {
            // The initial configure is always allowed; failures are
            // protocol bugs.
            let _ = xdg.send_configure();
        }
    }
}
