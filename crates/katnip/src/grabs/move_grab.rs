//! Move grab: drags a floating window with the pointer.

use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::state::Katnip;

pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<Katnip>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<Katnip> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Katnip,
        handle: &mut PointerInnerHandle<'_, Katnip>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus.
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);
        // Persist so later re-arranges keep the float where it was dropped.
        data.update_float_loc(&self.window, new_location.to_i32_round());
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
        const BTN_LEFT: u32 = 0x110;

        if !handle.current_pressed().contains(&BTN_LEFT) {
            // All buttons released: end the grab.
            handle.unset_grab(self, data, event.serial, event.time, true);
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
