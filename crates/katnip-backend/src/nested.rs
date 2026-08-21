//! Nested winit backend: renders a test background and echoes input events.
//!
//! This is the M0 proof-of-life loop. It intentionally contains no Wayland
//! globals yet; xdg_shell surface management arrives in M1 on top of this
//! render/dispatch structure.

use smithay::backend::SwapBuffersError;
use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::damage::{Error as DamageTrackerError, OutputDamageTracker};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent};
use smithay::reexports::winit::platform::pump_events::PumpStatus;
use smithay::utils::{Physical, Scale, Transform};
use tracing::{debug, error, info, warn};

/// Deep charcoal with a hint of green, so the window is obviously Katnip.
const CLEAR_COLOR: Color32F = Color32F::new(0.086, 0.106, 0.098, 1.0);

type PhysSize = smithay::utils::Size<i32, Physical>;

fn make_tracker(size: PhysSize, scale: f64) -> OutputDamageTracker {
    OutputDamageTracker::new(size, Scale::from(scale.max(1.0)), Transform::Normal)
}

pub fn run() -> anyhow::Result<()> {
    let (mut backend, mut winit) = winit::init::<GlesRenderer>()
        .map_err(|err| anyhow::anyhow!("failed to initialize winit backend: {err}"))?;

    let size: PhysSize = backend.window_size();
    let scale = backend.scale_factor();
    info!(?size, scale, "winit backend initialized");

    let mut damage_tracker = make_tracker(size, scale);
    let elements: Vec<SolidColorRenderElement> = Vec::new();

    loop {
        let status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Input(input) => debug!(?input, "input"),
            WinitEvent::Resized { .. } => {
                debug!("host window resized");
                damage_tracker = make_tracker(backend.window_size(), backend.scale_factor());
            }
            _ => {}
        });

        if let PumpStatus::Exit(code) = status {
            info!(code, "host session closed the window");
            return Ok(());
        }

        // age 0 = always full redraw; fine for a solid background.
        let render_res = backend.bind().and_then(|(renderer, mut framebuffer)| {
            damage_tracker
                .render_output(renderer, &mut framebuffer, 0, &elements, CLEAR_COLOR)
                .map(|result| result.damage)
                .map_err(|err| match err {
                    DamageTrackerError::Rendering(err) => err.into(),
                    other => SwapBuffersError::ContextLost(Box::new(std::io::Error::other(
                        format!("damage tracking failure: {other:?}"),
                    ))),
                })
        });

        match render_res {
            Ok(damage) => {
                if let Some(damage) = damage {
                    if let Err(err) = backend.submit(Some(damage)) {
                        warn!("failed to submit buffer: {err}");
                    }
                }
            }
            Err(SwapBuffersError::ContextLost(err)) => {
                error!("GPU context lost: {err}");
                anyhow::bail!("GPU context lost: {err}");
            }
            Err(err) => warn!("rendering error: {err}"),
        }
    }
}
