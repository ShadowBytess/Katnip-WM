//! Resize grab: drags a window edge/corner, reconfiguring the client.

use std::cell::RefCell;

use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::state::Katnip;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ResizeEdge: u32 {
        const TOP          = 0b0001;
        const BOTTOM       = 0b0010;
        const LEFT         = 0b0100;
        const RIGHT        = 0b1000;

        const TOP_LEFT     = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT  = Self::BOTTOM.bits() | Self::LEFT.bits();

        const TOP_RIGHT    = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

/// Picks the closest edge pair for a pointer position relative to a window
/// rect (used when starting a resize from the window body).
pub fn edges_for_point(rect: &Rectangle<i32, Logical>, point: Point<f64, Logical>) -> ResizeEdge {
    let mid_x = rect.loc.x + rect.size.w / 2;
    let mid_y = rect.loc.y + rect.size.h / 2;
    let mut edges = ResizeEdge::empty();
    if (point.x as i32) < mid_x {
        edges |= ResizeEdge::LEFT;
    } else {
        edges |= ResizeEdge::RIGHT;
    }
    if (point.y as i32) < mid_y {
        edges |= ResizeEdge::TOP;
    } else {
        edges |= ResizeEdge::BOTTOM;
    }
    edges
}

pub struct ResizeSurfaceGrab {
    start_data: PointerGrabStartData<Katnip>,
    window: Window,

    edges: ResizeEdge,

    initial_rect: Rectangle<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: PointerGrabStartData<Katnip>,
        window: Window,
        edges: ResizeEdge,
        initial_window_rect: Rectangle<i32, Logical>,
    ) -> Self {
        let initial_rect = initial_window_rect;
        ResizeSurfaceState::with(window.toplevel().expect("toplevel").wl_surface(), |state| {
            *state = ResizeSurfaceState::Resizing {
                edges,
                initial_rect,
            };
        });

        Self {
            start_data,
            window,
            edges,
            initial_rect,
            last_window_size: initial_rect.size,
        }
    }
}

impl PointerGrab<Katnip> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus.
        handle.motion(data, None, event);

        let mut delta = event.location - self.start_data.location;

        let mut new_window_width = self.initial_rect.size.w;
        let mut new_window_height = self.initial_rect.size.h;

        if self.edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                delta.x = -delta.x;
            }
            new_window_width = (self.initial_rect.size.w as f64 + delta.x) as i32;
        }

        if self.edges.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
            if self.edges.intersects(ResizeEdge::TOP) {
                delta.y = -delta.y;
            }
            new_window_height = (self.initial_rect.size.h as f64 + delta.y) as i32;
        }

        let (min_size, max_size) = compositor::with_states(
            self.window.toplevel().expect("toplevel").wl_surface(),
            |states| {
                let mut guard = states.cached_state.get::<SurfaceCachedState>();
                let state = guard.current();
                (state.min_size, state.max_size)
            },
        );

        let min_width = min_size.w.max(1);
        let min_height = min_size.h.max(1);

        let max_width = if max_size.w == 0 {
            i32::MAX
        } else {
            max_size.w
        };
        let max_height = if max_size.h == 0 {
            i32::MAX
        } else {
            max_size.h
        };

        self.last_window_size = Size::from((
            new_window_width.max(min_width).min(max_width),
            new_window_height.max(min_height).min(max_height),
        ));

        let xdg = self.window.toplevel().expect("toplevel");
        xdg.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(self.last_window_size);
        });
        xdg.send_pending_configure();
    }

    fn relative_motion(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        // BTN_LEFT per linux/input-event-codes.h.
        // Note: RMB-initiated resizes still END on LMB release only if LMB is
        // held; we end on ANY button release while the grab holds buttons.
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);

            let xdg = self.window.toplevel().expect("toplevel");
            xdg.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
                state.size = Some(self.last_window_size);
            });
            xdg.send_pending_configure();

            ResizeSurfaceState::with(xdg.wl_surface(), |state| {
                *state = ResizeSurfaceState::WaitingForLastCommit {
                    edges: self.edges,
                    initial_rect: self.initial_rect,
                };
            });
        }
    }

    fn axis(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Katnip, handle: &mut PointerInnerHandle<'_, Katnip>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<Katnip> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Katnip) {}
}

/// State of the resize operation, stored in the surface's data map.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
enum ResizeSurfaceState {
    #[default]
    Idle,
    Resizing {
        edges: ResizeEdge,
        /// The initial window size and location.
        initial_rect: Rectangle<i32, Logical>,
    },
    /// Resize done; waiting for the final commit to fix up position.
    WaitingForLastCommit {
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    },
}

impl ResizeSurfaceState {
    fn with<F, T>(surface: &WlSurface, cb: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        compositor::with_states(surface, |states| {
            states.data_map.insert_if_missing(RefCell::<Self>::default);
            let state = states
                .data_map
                .get::<RefCell<Self>>()
                .expect("resize state");
            cb(&mut state.borrow_mut())
        })
    }

    fn commit(&mut self) -> Option<(ResizeEdge, Rectangle<i32, Logical>)> {
        match *self {
            Self::Resizing {
                edges,
                initial_rect,
            } => Some((edges, initial_rect)),
            Self::WaitingForLastCommit {
                edges,
                initial_rect,
            } => {
                *self = Self::Idle;
                Some((edges, initial_rect))
            }
            Self::Idle => None,
        }
    }
}

/// Called on toplevel commits: fixes up position after TOP/LEFT resizes.
pub fn handle_commit(state: &mut Katnip, surface: &WlSurface) {
    let space = &mut state.space;
    let Some(window) = space
        .elements()
        .find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == surface))
        .cloned()
    else {
        return;
    };

    let mut window_loc = match space.element_location(&window) {
        Some(loc) => loc,
        None => return,
    };
    let geometry = window.geometry();

    let new_loc: Point<Option<i32>, Logical> = ResizeSurfaceState::with(surface, |state| {
        state
            .commit()
            .and_then(|(edges, initial_rect)| {
                // Resizing by top/left moves the origin too.
                edges.intersects(ResizeEdge::TOP_LEFT).then(|| {
                    let new_x = edges
                        .intersects(ResizeEdge::LEFT)
                        .then_some(initial_rect.loc.x + (initial_rect.size.w - geometry.size.w));
                    let new_y = edges
                        .intersects(ResizeEdge::TOP)
                        .then_some(initial_rect.loc.y + (initial_rect.size.h - geometry.size.h));
                    (new_x, new_y).into()
                })
            })
            .unwrap_or_default()
    });

    if let Some(new_x) = new_loc.x {
        window_loc.x = new_x;
    }
    if let Some(new_y) = new_loc.y {
        window_loc.y = new_y;
    }

    if new_loc.x.is_some() || new_loc.y.is_some() {
        space.map_element(window.clone(), window_loc, false);
        // Keep the stored float position in sync with post-resize placement.
        state.update_float_loc(&window, window_loc);
    }
}
