//! Input routing: keybind table, pointer focus/click handling, and
//! SUPER+drag move/resize grabs.

use katnip_core::keybinds::{Action, Mods};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Rectangle, SERIAL_COUNTER};
use tracing::debug;

use crate::grabs::{MoveSurfaceGrab, ResizeSurfaceGrab, edges_for_point};
use crate::state::Katnip;

impl Katnip {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::<I>::time_msec(&event);
                let pressed = event.state() == smithay::backend::input::KeyState::Pressed;

                let mut fired_action = None;
                if let Some(keyboard) = self.seat.get_keyboard() {
                    let binds = self.binds.clone();
                    let result = keyboard.input::<Action, _>(
                        self,
                        event.key_code(),
                        event.state(),
                        serial,
                        time,
                        move |_, modifiers, sym_handle| {
                            if !pressed {
                                return FilterResult::Forward;
                            }
                            let mods = Mods {
                                shift: modifiers.shift,
                                ctrl: modifiers.ctrl,
                                alt: modifiers.alt,
                                logo: modifiers.logo,
                            };
                            match binds.lookup(&mods, sym_handle.modified_sym().raw()) {
                                Some(action) => FilterResult::Intercept(action.clone()),
                                None => FilterResult::Forward,
                            }
                        },
                    );
                    fired_action = result;
                }

                // Bound keys are consumed (never forwarded to clients).
                if let Some(action) = fired_action {
                    self.execute_action(&action);
                }
            }
            InputEvent::PointerMotion { .. } => {
                // winit delivers absolute motion; see below.
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                let under = self.surface_under(pos);

                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
                // Software cursor follows the pointer only in DRM mode.
                self.request_repaint_all();
            }
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    let logo_held = self
                        .seat
                        .get_keyboard()
                        .is_some_and(|kb| kb.modifier_state().logo);

                    let clicked = self.space.element_under(pointer.current_location());
                    match clicked {
                        Some((window, _)) if logo_held && button == 0x110 => {
                            // SUPER+LMB: focus, float if needed, start move.
                            let window = window.clone();
                            self.focus_window(Some(&window));
                            self.start_move_grab(&window, &pointer, serial);
                        }
                        Some((window, _)) if logo_held && button == 0x111 => {
                            // SUPER+RMB: focus and start resize.
                            let window = window.clone();
                            self.focus_window(Some(&window));
                            self.start_resize_grab(&window, &pointer, serial);
                        }
                        Some((window, _)) => {
                            let window = window.clone();
                            self.focus_window(Some(&window));
                        }
                        None => {
                            self.focus_window(None);
                        }
                    }
                };

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
                // Focus ring / float placement changes.
                self.request_repaint_all();
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let horizontal_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            _ => {}
        }
    }

    /// Executes a bind action.
    pub fn execute_action(&mut self, action: &Action) {
        match action {
            Action::Exec(cmd) if cmd == "terminal" => self.spawn_terminal(),
            Action::Exec(cmd) => {
                match std::process::Command::new("sh").arg("-c").arg(cmd).spawn() {
                    Ok(child) => debug!(pid = child.id(), %cmd, "exec"),
                    Err(err) => tracing::warn!(%cmd, %err, "exec failed"),
                }
            }
            Action::CloseFocused => self.close_focused(),
            Action::ToggleFloating => self.toggle_focused_floating(),
            Action::FocusWorkspace(i) => self.switch_workspace(*i),
            Action::MoveToWorkspace(i) => self.move_focused_to_workspace(*i),
            Action::Quit => {
                tracing::info!("quit requested via keybind");
                self.loop_signal.stop();
            }
        }
    }

    fn start_move_grab(
        &mut self,
        window: &smithay::desktop::Window,
        pointer: &smithay::input::pointer::PointerHandle<Katnip>,
        serial: smithay::utils::Serial,
    ) {
        let Some(start_data) = valid_grab_start(pointer, serial, window) else {
            return;
        };
        let initial = self.space.element_location(window).unwrap_or_default();

        // Dragging a tiled window floats it first (Hyprland behavior).
        if !self.is_floating(window) {
            let under_cursor = pointer.current_location().to_i32_round();
            self.ensure_floating_at(window, under_cursor);
        }

        let grab = MoveSurfaceGrab {
            start_data,
            window: window.clone(),
            initial_window_location: initial,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn start_resize_grab(
        &mut self,
        window: &smithay::desktop::Window,
        pointer: &smithay::input::pointer::PointerHandle<Katnip>,
        serial: smithay::utils::Serial,
    ) {
        let Some(start_data) = valid_grab_start(pointer, serial, window) else {
            return;
        };
        let loc = self.space.element_location(window).unwrap_or_default();
        let size = window.geometry().size;
        let rect = Rectangle::new(loc, size);

        // Floats resize freely; tiled windows are floated first so the
        // layout does not fight the drag.
        if !self.is_floating(window) {
            let under_cursor = pointer.current_location().to_i32_round();
            self.ensure_floating_at(window, under_cursor);
        }

        let edges = edges_for_point(&rect, pointer.current_location());
        let grab = ResizeSurfaceGrab::start(start_data, window.clone(), edges, rect);
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

/// Validates that this click-serial belongs to a grab started on `window`.
fn valid_grab_start(
    pointer: &smithay::input::pointer::PointerHandle<Katnip>,
    serial: smithay::utils::Serial,
    window: &smithay::desktop::Window,
) -> Option<GrabStartData<Katnip>> {
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    let target = window.toplevel()?.wl_surface().clone();
    if !focus.id().same_client_as(&target.id()) {
        return None;
    }
    Some(start_data)
}
