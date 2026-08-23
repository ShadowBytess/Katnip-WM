//! Nested-output setup: winit event source, output global, render loop.

use std::time::Duration;

use katnip_backend::KatnipRenderer;
use smithay::backend::SwapBuffersError;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::winit::{WinitEvent, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::utils::{Point, Rectangle, Scale, Size, Transform};
use tracing::{debug, error, info, warn};

use crate::bar::KatnipElements;
use crate::state::{CalloopData, Katnip};

/// Katnip accent teal for the focused tile border.
const BORDER_FOCUSED: Color32F = Color32F::new(0.30, 0.85, 0.70, 1.0);
/// Muted gray for unfocused tile borders.
const BORDER_UNFOCUSED: Color32F = Color32F::new(0.22, 0.24, 0.24, 1.0);
/// Background clear color (deep charcoal).
const CLEAR: Color32F = Color32F::new(0.086, 0.106, 0.098, 1.0);

type GraphicsBackend = WinitGraphicsBackend<KatnipRenderer>;

/// Shared startup log line (both backends).
pub fn info_listening(socket: &str) {
    tracing::info!("listening on WAYLAND_DISPLAY={socket}");
}

/// Creates the nested output and inserts the winit backend as a calloop
/// source; the graphics backend is captured inside the source closure.
pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> anyhow::Result<()> {
    let backend = katnip_backend::init_nested()?;

    let mode = Mode {
        size: backend.graphics.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "katnip-winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Katnip".into(),
            model: "Nested".into(),
        },
    );
    let _global = output.create_global::<Katnip>(&data.display_handle);
    // Flipped180 matches how smallvil/anvil compensate winit's coordinate space.
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    {
        let state = &mut data.state;
        state.space.map_output(&output, (0, 0));
        state.output = Some(output.clone());
        state.damage_tracker =
            Some(smithay::backend::renderer::damage::OutputDamageTracker::from_output(&output));
        state.arrange_force(false);
    }

    // Clients spawned by Katnip should connect to our socket by default.
    // SAFETY: single-threaded startup, before any other thread reads env.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", data.state.socket_name.clone());
    }

    let mut graphics = backend.graphics;
    event_loop
        .handle()
        .insert_source(backend.events, move |event, _, data| match event {
            WinitEvent::Resized { size, .. } => {
                debug!(?size, "host window resized");
                if let Some(output) = data.state.output.clone() {
                    output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                }
                data.state.arrange_force(false);
            }
            WinitEvent::Input(event) => data.state.process_input_event(event),
            WinitEvent::Redraw => redraw(&mut graphics, &mut data.state),
            WinitEvent::CloseRequested => {
                info!("close requested");
                data.state.loop_signal.stop();
            }
            _ => {}
        })
        .map_err(|err| anyhow::anyhow!("failed to insert winit event source: {err}"))?;

    Ok(())
}

fn redraw(graphics: &mut GraphicsBackend, state: &mut Katnip) {
    state.cleanup_dead();

    let Some(ref output) = state.output else {
        return;
    };

    let scale_f64 = output.current_scale().fractional_scale();
    let scale = Scale::from(scale_f64);
    let mut custom = border_elements(state, scale);
    let bar_info = crate::bar::BarInfo::from_state(state);
    let window_size_logical = graphics.window_size().to_f64().to_logical(scale);

    // Render first (borrowing the damage tracker), then submit after the
    // renderer/framebuffer borrows of `graphics` have ended.
    let frame: Result<Option<Vec<Rectangle<i32, smithay::utils::Physical>>>, String> = {
        // Split state borrows so the bar and damage tracker can coexist.
        let Katnip {
            bar,
            damage_tracker,
            ..
        } = state;
        let Some(damage_tracker) = damage_tracker.as_mut() else {
            return;
        };
        match graphics.bind() {
            Ok((renderer, mut framebuffer)) => {
                if bar.enabled {
                    custom.extend(bar.elements(
                        renderer,
                        window_size_logical.w as i32,
                        scale_f64,
                        &bar_info,
                    ));
                }
                match smithay::desktop::space::render_output(
                    output,
                    renderer,
                    &mut framebuffer,
                    1.0,
                    0,
                    [&state.space],
                    &custom,
                    damage_tracker,
                    CLEAR,
                ) {
                    Ok(result) => Ok(result.damage.cloned()),
                    Err(err) => {
                        warn!("rendering error: {err:?}");
                        Ok(None)
                    }
                }
            }
            Err(SwapBuffersError::ContextLost(err)) => Err(format!("GPU context lost: {err}")),
            Err(err) => {
                warn!("failed to bind framebuffer: {err}");
                Ok(None)
            }
        }
    };

    match frame {
        Ok(Some(damage)) => {
            if let Err(err) = graphics.submit(Some(damage.as_slice())) {
                warn!("failed to submit buffer: {err}");
            }
        }
        Ok(None) => {}
        Err(msg) => {
            error!("{msg}");
            state.loop_signal.stop();
            return;
        }
    }

    // Advance frame callbacks so clients keep drawing.
    for window in state.space.elements() {
        window.send_frame(
            output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    }

    graphics.window().request_redraw();
}

/// Builds one solid-color border element per mapped tile.
/// Public alias for hardware-mode rendering.
pub(crate) fn border_elements_pub(
    state: &Katnip,
    scale: Scale<f64>,
) -> Vec<KatnipElements<GlesRenderer>> {
    border_elements(state, scale)
}

fn border_elements(state: &Katnip, scale: Scale<f64>) -> Vec<KatnipElements<GlesRenderer>> {
    let bw = state.layout.border_width;
    let mut elements = Vec::new();

    for window in state.active_windows() {
        let Some(location) = state.space.element_location(window) else {
            continue;
        };
        let size = window.geometry().size;
        if size.w <= 0 || size.h <= 0 {
            continue;
        }

        // The border element covers the full tile rect; client content sits
        // inset by the configured border width on every side.
        let rect = Rectangle::<i32, smithay::utils::Physical>::new(
            Point::from((location.x - bw, location.y - bw)).to_physical_precise_round(scale),
            Size::from((size.w + 2 * bw, size.h + 2 * bw)).to_physical_precise_round(scale),
        );

        let color = if state.focused_window().is_some_and(|f| &f == window) {
            BORDER_FOCUSED
        } else {
            BORDER_UNFOCUSED
        };

        elements.push(KatnipElements::Solid(SolidColorRenderElement::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            color,
            Kind::Unspecified,
        )));
    }

    elements
}
