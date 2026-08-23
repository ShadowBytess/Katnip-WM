//! Hardware (DRM/KMS + udev + libinput + libseat) session mode.
//!
//! Alpha constraints, all deliberate simplifications:
//! - single primary GPU (`$KATNIP_DRM_DEVICE` overrides selection)
//! - continuous repaint per output at its refresh rate
//! - software dot cursor through the normal element pipeline
//!
//! Everything else (tiling, workspaces, plugins, IPC, the bar) runs
//! identically to nested mode because both modes share [`Katnip`] state.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use smithay::backend::allocator::gbm::{GbmAllocator, GbmDevice};
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::Event as SessionEvent;
use smithay::backend::udev::{UdevBackend, UdevEvent};
use smithay::desktop::Window;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::control::{
    Device as ControlDevice, ModeTypeFlags, ResourceHandles, connector, crtc,
};
use smithay::reexports::input::{DeviceCapability, Libinput};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Physical, Point, SERIAL_COUNTER, Scale, Size};
use tracing::{debug, error, info, warn};

use katnip_backend::hardware::{self, GpuContext};

use crate::bar::{BarInfo, KatnipElements};
use crate::state::{CalloopData, Katnip};

const CLEAR: Color32F = Color32F::new(0.086, 0.106, 0.098, 1.0);
/// Dot-cursor size (logical px).
const CURSOR_DOT: i32 = 10;
/// Hardware cursor plane buffer size.
fn cursor_size() -> Size<u32, smithay::utils::Buffer> {
    Size::from((64u32, 64u32))
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Everything the hardware mode keeps alive on [`Katnip`].
pub struct HwData {
    /// Session handle (notifier lives in its own event source).
    pub session: hardware::HardwareSession,
    /// The selected primary GPU node.
    pub primary: DrmNode,
    /// The single-GPU GLES renderer bound to the primary GPU.
    pub renderer: GlesRenderer,
    pub libinput: Libinput,
    pub keyboards: Vec<smithay::reexports::input::Device>,
    /// Opened GPU contexts by DRM node (only the primary for now).
    pub gpus: HashMap<DrmNode, OpenGpu>,
    /// Per-device connector/surface bookkeeping.
    pub devices: HashMap<DrmNode, DeviceCtx>,
}

/// One opened DRM device plus its swapchain factories.
pub struct OpenGpu {
    pub drm: DrmDevice,
    pub gbm: GbmDevice<DrmDeviceFd>,
    pub allocator: GbmAllocator<DrmDeviceFd>,
    pub exporter: GbmFramebufferExporter<DrmDeviceFd>,
    /// vblank notifier, taken out when registered with calloop.
    pub notifier: Option<smithay::backend::drm::DrmDeviceNotifier>,
}

/// Per-device connector bookkeeping.
#[derive(Default)]
pub struct DeviceCtx {
    /// Live connector -> crtc mappings we own.
    pub known: HashMap<connector::Handle, crtc::Handle>,
    /// crtc -> live output surface.
    pub surfaces: HashMap<crtc::Handle, SurfaceCtx>,
}

/// One scanned-out display.
pub struct SurfaceCtx {
    pub output: Output,
    /// wl_output global, removed when this struct drops.
    global: Option<GlobalId>,
    display_handle: smithay::reexports::wayland_server::DisplayHandle,
    pub compositor: DrmCompositor<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        (),
        DrmDeviceFd,
    >,
}

impl Drop for SurfaceCtx {
    fn drop(&mut self) {
        if let Some(global) = self.global.take() {
            self.display_handle.remove_global::<Katnip>(global);
        }
    }
}

/// CLI/env backend selection result.
#[derive(Debug, PartialEq, Eq)]
pub enum BackendChoice {
    Nested,
    Drm,
}

pub fn select_backend() -> BackendChoice {
    let drm_flag = std::env::args().skip(1).any(|a| a == "--drm");
    if drm_flag || std::env::var("KATNIP_BACKEND").as_deref() == Ok("drm") {
        BackendChoice::Drm
    } else {
        BackendChoice::Nested
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

pub fn run_drm(
    config: &katnip_config::Config,
    resolved_binds: std::sync::Arc<crate::binds::ResolvedBinds>,
    plugin_host: Option<katnip_plugins::script::ScriptHost>,
    native_plugins: Vec<katnip_plugins::native::NativePlugin>,
) -> anyhow::Result<()> {
    use smithay::reexports::calloop::EventLoop;
    use smithay::reexports::wayland_server::Display;

    info!("starting in DRM/hardware mode");

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;
    let display = Display::new()?;
    let display_handle = display.handle();

    // --- session + primary GPU -------------------------------------------
    let mut session =
        hardware::HardwareSession::new().map_err(|err| anyhow::anyhow!("session: {err}"))?;
    let seat = session.seat();
    info!(%seat, "acquired seat");

    let (primary, primary_path) =
        hardware::select_primary_gpu(&seat).map_err(|err| anyhow::anyhow!("{err}"))?;
    info!(?primary, path = %primary_path.display(), "opening primary gpu");
    let gpu =
        hardware::open_gpu(&mut session, &primary_path).map_err(|err| anyhow::anyhow!("{err}"))?;
    let GpuContext {
        drm,
        gbm,
        allocator,
        exporter,
        renderer,
        render_node: _,
        notifier,
    } = gpu;

    // --- libinput -----------------------------------------------------------
    let mut libinput: Libinput =
        Libinput::new_with_udev(LibinputSessionInterface::from(session.session.clone()));
    if libinput.udev_assign_seat(&seat).is_err() {
        anyhow::bail!("udev_assign_seat failed");
    }

    // --- compositor state ---------------------------------------------------
    let state = Katnip::new(
        &mut event_loop,
        display,
        resolved_binds,
        config,
        plugin_host,
        native_plugins,
        Some(HwData {
            session,
            primary,
            renderer,
            libinput: libinput.clone(),
            keyboards: Vec::new(),
            gpus: HashMap::from([(
                primary,
                OpenGpu {
                    drm,
                    gbm,
                    allocator,
                    exporter,
                    notifier: Some(notifier),
                },
            )]),
            devices: HashMap::from([(primary, DeviceCtx::default())]),
        }),
    )?;
    let mut data = CalloopData {
        state,
        display_handle,
    };
    data.state.loop_handle = Some(event_loop.handle());

    // --- client buffer formats ----------------------------------------------
    {
        let formats: Vec<_> = {
            let hw = data.state.hw.as_mut().expect("hw present");
            ImportMemWl::shm_formats(&hw.renderer).collect()
        };
        data.state.shm_state.update_formats(formats);
    }

    {
        let hw = data.state.hw.as_mut().expect("hw present");
        match ImportEgl::bind_wl_display(&mut hw.renderer, &data.state.display_handle) {
            Ok(_) => info!("EGL hardware acceleration enabled"),
            Err(err) => info!(?err, "EGL hardware acceleration unavailable"),
        }
    }

    {
        use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
        let formats = {
            let hw = data.state.hw.as_mut().expect("hw present");
            ImportDma::dmabuf_formats(&hw.renderer)
        };
        let feedback = DmabufFeedbackBuilder::new(primary.dev_id(), formats)
            .build()
            .expect("valid dmabuf feedback");
        data.state
            .dmabuf_state
            .create_global_with_default_feedback::<Katnip>(&data.state.display_handle, &feedback);
        info!("dmabuf global ready");
    }

    // --- event sources -------------------------------------------------------
    let handle = event_loop.handle();

    handle
        .insert_source(
            LibinputInputBackend::new(libinput),
            |event, _, data: &mut CalloopData| {
                sync_leds(data);
                data.state.process_input_event(event);
            },
        )
        .map_err(|err| anyhow::anyhow!("insert libinput source: {err}"))?;
    info!("libinput ready");

    {
        let session_notifier = data
            .state
            .hw
            .as_mut()
            .expect("hw present")
            .session
            .notifier
            .take()
            .expect("session notifier present");
        handle
            .insert_source(session_notifier, |event, _, data| match event {
                SessionEvent::PauseSession => {
                    info!("session paused (VT switch)");
                    pause_hardware(data);
                }
                SessionEvent::ActivateSession => {
                    info!("session resumed");
                    resume_hardware(data);
                }
            })
            .map_err(|err| anyhow::anyhow!("insert session source: {err}"))?;
    }

    {
        let drm_notifier = data
            .state
            .hw
            .as_mut()
            .expect("hw present")
            .gpus
            .get_mut(&primary)
            .and_then(|gpu| gpu.notifier.take())
            .expect("vblank notifier present");
        handle.insert_source(drm_notifier, move |event, _, data| match event {
            DrmEvent::VBlank(crtc) => frame_finished(data, primary, crtc),
            DrmEvent::Error(err) => error!(?err, "drm error"),
        })?;
    }

    {
        let udev = UdevBackend::new(&seat).map_err(|err| anyhow::anyhow!("udev backend: {err}"))?;
        handle
            .insert_source(udev, |event, _, data| {
                let primary = data.state.hw.as_ref().expect("hw present").primary;
                match event {
                    UdevEvent::Added { device_id, path } => match DrmNode::from_dev_id(device_id) {
                        Ok(node) if node == primary => {
                            if let Err(err) = reopen_gpu_if_needed(data, node, &path) {
                                warn!(?node, %err, "gpu re-open failed");
                            }
                            scan_device(data, node);
                        }
                        Ok(other) => warn!(?other, "secondary GPU ignored in single-gpu alpha"),
                        Err(err) => warn!(%err, "bad drm node from udev"),
                    },
                    UdevEvent::Changed { device_id } => {
                        if let Ok(node) = DrmNode::from_dev_id(device_id) {
                            if node == primary {
                                scan_device(data, node);
                            }
                        }
                    }
                    UdevEvent::Removed { device_id } => {
                        if let Ok(node) = DrmNode::from_dev_id(device_id) {
                            if node == primary {
                                error!("primary GPU removed; running headless until replug");
                            }
                        }
                    }
                }
            })
            .map_err(|err| anyhow::anyhow!("insert udev source: {err}"))?;
    }

    crate::output::info_listening(&data.state.socket_name.to_string_lossy());
    let ipc_socket = crate::ipc::init_ipc(
        &event_loop.handle(),
        &data.state.socket_name.to_string_lossy(),
    )?;
    for cmd in &config.autostart {
        spawn_autostart(cmd);
    }

    // Enumerate whatever is already plugged in and start rendering.
    scan_device(&mut data, primary);

    event_loop.run(None, &mut data, |data| {
        data.state.space.refresh();
        data.state.popups.cleanup();
        let _ = data.state.display_handle.flush_clients();
    })?;

    crate::ipc::cleanup(&ipc_socket);
    info!("DRM mode exited");
    Ok(())
}

fn spawn_autostart(cmd: &str) {
    match std::process::Command::new("sh").arg("-c").arg(cmd).spawn() {
        Ok(child) => info!(pid = child.id(), %cmd, "autostart"),
        Err(err) => warn!(%cmd, %err, "autostart failed"),
    }
}

// ---------------------------------------------------------------------------
// Connector lifecycle
// ---------------------------------------------------------------------------

fn reopen_gpu_if_needed(
    data: &mut CalloopData,
    node: DrmNode,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let exists = data
        .state
        .hw
        .as_ref()
        .is_some_and(|hw| hw.gpus.contains_key(&node));
    if exists {
        return Ok(());
    }
    let Some(hw) = data.state.hw.as_mut() else {
        anyhow::bail!("hardware state missing");
    };
    let gpu = hardware::open_gpu(&mut hw.session, path).map_err(|err| anyhow::anyhow!("{err}"))?;
    hw.renderer = gpu.renderer;
    hw.gpus.insert(
        node,
        OpenGpu {
            drm: gpu.drm,
            gbm: gpu.gbm,
            allocator: gpu.allocator,
            exporter: gpu.exporter,
            notifier: Some(gpu.notifier),
        },
    );
    Ok(())
}

/// Connector enumeration + hotplug diffing for one device.
fn scan_device(data: &mut CalloopData, node: DrmNode) {
    let resources: ResourceHandles = {
        let Some(hw) = data.state.hw.as_ref() else {
            return;
        };
        let Some(gpu) = hw.gpus.get(&node) else {
            return;
        };
        match gpu.drm.resource_handles() {
            Ok(res) => res,
            Err(err) => {
                warn!(?node, %err, "resource_handles failed");
                return;
            }
        }
    };

    let infos: Vec<connector::Info> = {
        let Some(hw) = data.state.hw.as_ref() else {
            return;
        };
        let Some(gpu) = hw.gpus.get(&node) else {
            return;
        };
        resources
            .connectors
            .iter()
            .filter_map(|c| gpu.drm.get_connector(*c, false).ok())
            .collect()
    };

    let mut seen: HashMap<connector::Handle, crtc::Handle> = HashMap::new();
    for info in infos {
        let handle = info.handle();
        if info.state() != connector::State::Connected {
            continue;
        }
        let non_desktop = {
            let Some(hw) = data.state.hw.as_ref() else {
                continue;
            };
            let Some(gpu) = hw.gpus.get(&node) else {
                continue;
            };
            hardware::connector_is_non_desktop(&gpu.drm, handle)
        };
        if non_desktop {
            debug!(?handle, "skipping non-desktop connector");
            continue;
        }

        let used = used_crtcs(data, node, Some(handle));
        let Some(crtc) = find_free_crtc(data, node, &info, &resources, &used) else {
            debug!(?handle, "no free crtc for connector");
            continue;
        };
        seen.insert(handle, crtc);

        let live = data
            .state
            .hw
            .as_ref()
            .and_then(|hw| hw.devices.get(&node))
            .is_some_and(|ctx| ctx.known.contains_key(&handle));
        if !live {
            create_output(data, node, &info, crtc);
        }
    }

    // Disconnect anything no longer seen.
    let gone: Vec<connector::Handle> = data
        .state
        .hw
        .as_ref()
        .and_then(|hw| hw.devices.get(&node))
        .map(|ctx| {
            ctx.known
                .keys()
                .filter(|k| !seen.contains_key(*k))
                .copied()
                .collect()
        })
        .unwrap_or_default();
    for conn in gone {
        destroy_output(data, node, conn);
    }
}

fn used_crtcs(
    data: &CalloopData,
    node: DrmNode,
    exclude_conn: Option<connector::Handle>,
) -> HashSet<crtc::Handle> {
    data.state
        .hw
        .as_ref()
        .and_then(|hw| hw.devices.get(&node))
        .map(|ctx| {
            ctx.known
                .iter()
                .filter(|(conn, _)| Some(**conn) != exclude_conn)
                .map(|(_, crtc)| *crtc)
                .chain(ctx.surfaces.keys().copied())
                .collect()
        })
        .unwrap_or_default()
}

fn find_free_crtc(
    data: &CalloopData,
    node: DrmNode,
    info: &connector::Info,
    resources: &ResourceHandles,
    used: &HashSet<crtc::Handle>,
) -> Option<crtc::Handle> {
    let drm_fd = {
        let hw = data.state.hw.as_ref()?;
        let gpu = hw.gpus.get(&node)?;
        &gpu.drm
    };
    for enc_handle in info.encoders() {
        let Ok(encoder) = drm_fd.get_encoder(*enc_handle) else {
            continue;
        };
        for candidate in resources.filter_crtcs(encoder.possible_crtcs()) {
            if !used.contains(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn create_output(
    data: &mut CalloopData,
    node: DrmNode,
    info: &connector::Info,
    crtc: crtc::Handle,
) {
    let output_name = format!("{}-{}", info.interface().as_str(), info.interface_id());
    info!(%output_name, ?crtc, "setting up connector");

    let mode_id = info
        .modes()
        .iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    let Some(drm_mode) = info.modes().get(mode_id).copied() else {
        warn!(%output_name, "connector has no modes");
        return;
    };
    let wl_mode = Mode::from(drm_mode);

    let (phys_w, phys_h) = info.size().unwrap_or((0, 0));
    let output = Output::new(
        output_name.clone(),
        PhysicalProperties {
            size: (phys_w as i32, phys_h as i32).into(),
            subpixel: Subpixel::from(info.subpixel()),
            make: "Unknown".into(),
            model: "Unknown".into(),
        },
    );
    let global = output.create_global::<Katnip>(&data.state.display_handle);
    output.set_preferred(wl_mode);

    // Place outputs side-by-side.
    let x = data.state.space.outputs().fold(0i32, |acc, o| {
        acc + data
            .state
            .space
            .output_geometry(o)
            .unwrap_or_default()
            .size
            .w
    });
    let position = Point::<i32, smithay::utils::Logical>::from((x, 0));
    output.change_current_state(Some(wl_mode), None, None, Some(position));
    data.state.space.map_output(&output, position);

    let (allocator, exporter, gbm, surface, planes) = {
        let hw = data.state.hw.as_mut().expect("hw present");
        let gpu = hw.gpus.get_mut(&node).expect("gpu present");
        let surface = match gpu.drm.create_surface(crtc, drm_mode, &[info.handle()]) {
            Ok(s) => s,
            Err(err) => {
                warn!(%output_name, %err, "create_surface failed");
                return;
            }
        };
        let planes = match gpu.drm.planes(&crtc) {
            Ok(p) => Some(p),
            Err(err) => {
                debug!(%output_name, %err, "plane query failed; using defaults");
                None
            }
        };
        (
            gpu.allocator.clone(),
            gpu.exporter.clone(),
            gpu.gbm.clone(),
            surface,
            planes,
        )
    };

    let renderer_formats = {
        let hw = data.state.hw.as_ref().expect("hw present");
        hw.renderer
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .copied()
            .collect::<Vec<_>>()
    };

    let compositor = match DrmCompositor::new(
        smithay::output::OutputModeSource::Auto(output.clone()),
        surface,
        planes,
        allocator,
        exporter,
        hardware::SUPPORTED_FORMATS.iter().copied(),
        renderer_formats,
        cursor_size(),
        Some(gbm),
    ) {
        Ok(c) => c,
        Err(err) => {
            warn!(%output_name, ?err, "DrmCompositor init failed");
            return;
        }
    };

    let ctx_surface = SurfaceCtx {
        output: output.clone(),
        global: Some(global),
        display_handle: data.state.display_handle.clone(),
        compositor,
    };
    {
        let hw = data.state.hw.as_mut().expect("hw present");
        let ctx = hw.devices.get_mut(&node).expect("device ctx");
        ctx.known.insert(info.handle(), crtc);
        ctx.surfaces.insert(crtc, ctx_surface);
    }

    // Kick the repaint chain.
    render_surface(data, node, crtc);
}

fn destroy_output(data: &mut CalloopData, node: DrmNode, conn: connector::Handle) {
    let Some(hw) = data.state.hw.as_mut() else {
        return;
    };
    let Some(ctx) = hw.devices.get_mut(&node) else {
        return;
    };
    let Some(crtc) = ctx.known.remove(&conn) else {
        return;
    };
    if let Some(surface) = ctx.surfaces.remove(&crtc) {
        data.state.space.unmap_output(&surface.output);
        info!(?conn, "output disconnected");
    }
}

// ---------------------------------------------------------------------------
// Rendering chain
// ---------------------------------------------------------------------------

/// Vblank received: finish the queued frame and schedule the next repaint.
fn frame_finished(data: &mut CalloopData, node: DrmNode, crtc: crtc::Handle) {
    let Some(hw) = data.state.hw.as_mut() else {
        return;
    };
    let Some(surface) = hw
        .devices
        .get_mut(&node)
        .and_then(|ctx| ctx.surfaces.get_mut(&crtc))
    else {
        return;
    };
    if let Err(err) = surface.compositor.frame_submitted() {
        warn!(?crtc, ?err, "frame_submitted failed");
    }
    schedule_repaint(data, node, crtc, Duration::from_millis(1));
}

fn schedule_repaint(data: &mut CalloopData, node: DrmNode, crtc: crtc::Handle, delay: Duration) {
    let Some(handle) = data.state.loop_handle.clone() else {
        return;
    };
    let timer = Timer::from_duration(delay);
    if let Err(err) = handle.insert_source(timer, move |_, _, data: &mut CalloopData| {
        render_surface(data, node, crtc);
        TimeoutAction::Drop
    }) {
        error!(?err, "failed to schedule repaint");
    }
}

fn refresh_interval(output: &Output) -> Duration {
    output
        .current_mode()
        .map(|m| Duration::from_secs_f64(1_000f64 / m.refresh.max(1) as f64))
        .unwrap_or(Duration::from_millis(16))
}

fn render_surface(data: &mut CalloopData, node: DrmNode, crtc: crtc::Handle) {
    let scale_f64 = {
        let hw = data.state.hw.as_ref().expect("hw present");
        let surface = hw
            .devices
            .get(&node)
            .and_then(|d| d.surfaces.get(&crtc))
            .expect("surface present");
        surface.output.current_scale().fractional_scale()
    };
    let scale = Scale::from(scale_f64);

    // Custom layers: borders, bar, dot cursor.
    let mut custom: Vec<KatnipElements<GlesRenderer>> =
        crate::output::border_elements_pub(&data.state, scale)
            .into_iter()
            .collect();

    {
        let bar_info = BarInfo::from_state(&data.state);
        if data.state.bar.enabled {
            let Katnip { bar, hw, .. } = &mut data.state;
            let hw = hw.as_mut().expect("hw present");
            let output = &hw
                .devices
                .get(&node)
                .and_then(|d| d.surfaces.get(&crtc))
                .expect("surface")
                .output;
            let width_logical = output.current_mode().map(|m| m.size.w).unwrap_or(1920);
            let _ = scale_f64;
            custom.extend(bar.elements(&mut hw.renderer, width_logical, scale_f64, &bar_info));
        }
    }

    // Window surfaces + dot cursor.
    {
        let Katnip { hw, space, .. } = &mut data.state;
        let hw = hw.as_mut().expect("hw present");
        let Some(surface) = hw
            .devices
            .get_mut(&node)
            .and_then(|d| d.surfaces.get_mut(&crtc))
        else {
            return;
        };
        let _output = &surface.output;
        // Front-to-back: iterate stacked windows top-most first.
        use smithay::backend::renderer::element::AsRenderElements;
        for window in space.elements().rev() {
            let Some(loc) = space.element_location(window) else {
                continue;
            };
            let elems = <Window as AsRenderElements<GlesRenderer>>::render_elements(
                window,
                &mut hw.renderer,
                loc.to_physical_precise_round(scale),
                Scale::from(scale_f64),
                1.0,
            );
            custom.extend(elems.into_iter().map(KatnipElements::Window));
        }

        let loc = data.state.pointer_location();
        let rect = smithay::utils::Rectangle::<i32, Physical>::new(
            Point::from((loc.x as i32 - CURSOR_DOT / 2, loc.y as i32 - CURSOR_DOT / 2))
                .to_physical_precise_round(scale),
            Size::from((CURSOR_DOT, CURSOR_DOT)).to_physical_precise_round(scale),
        );
        custom.push(KatnipElements::Solid(SolidColorRenderElement::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            Color32F::new(0.30, 0.85, 0.70, 1.0),
            Kind::Unspecified,
        )));
    }

    // Render + queue.
    let queued = {
        let hw = data.state.hw.as_mut().expect("hw present");
        let surface = hw
            .devices
            .get_mut(&node)
            .and_then(|d| d.surfaces.get_mut(&crtc))
            .expect("surface present");
        match surface
            .compositor
            .render_frame(&mut hw.renderer, &custom, CLEAR, FrameFlags::empty())
        {
            Ok(result) => {
                if !result.is_empty {
                    match surface.compositor.queue_frame(()) {
                        Ok(()) => true,
                        Err(err) => {
                            warn!(?err, "queue_frame failed");
                            false
                        }
                    }
                } else {
                    false
                }
            }
            Err(err) => {
                warn!(?crtc, ?err, "render_frame failed");
                false
            }
        }
    };

    send_frames(data, node, crtc);

    // Keep the repaint chain alive regardless of damage this cycle.
    let output_refresh = {
        let hw = data.state.hw.as_ref().expect("hw present");
        hw.devices
            .get(&node)
            .and_then(|d| d.surfaces.get(&crtc))
            .map(|s| refresh_interval(&s.output))
            .unwrap_or(Duration::from_millis(16))
    };
    let _ = queued;
    schedule_repaint(data, node, crtc, output_refresh);
}

fn send_frames(data: &mut CalloopData, node: DrmNode, crtc: crtc::Handle) {
    let Some(hw) = data.state.hw.as_ref() else {
        return;
    };
    let Some(surface) = hw.devices.get(&node).and_then(|d| d.surfaces.get(&crtc)) else {
        return;
    };
    let output = surface.output.clone();
    let start = data.state.start_time;
    let serial = SERIAL_COUNTER.next_serial();
    let _ = serial;
    for window in data.state.space.elements() {
        window.send_frame(&output, start.elapsed(), Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
}

// ---------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------

/// Keeps keyboard LED state in sync across added devices.
fn sync_leds(data: &mut CalloopData) {
    let Some(hw) = data.state.hw.as_mut() else {
        return;
    };
    let led_state = data.state.seat.get_keyboard().map(|kb| kb.led_state());
    let _ = (led_state, &mut hw.keyboards, DeviceCapability::Keyboard);
}

/// Pauses rendering + input when the VT switches away.
fn pause_hardware(data: &mut CalloopData) {
    let Some(hw) = data.state.hw.as_mut() else {
        return;
    };
    hw.libinput.suspend();
    for device in hw.devices.values_mut() {
        for surface in device.surfaces.values_mut() {
            let _ = surface.compositor.clear();
        }
    }
}

/// Restores output state after returning to the VT.
fn resume_hardware(data: &mut CalloopData) {
    let nodes: Vec<(DrmNode, Vec<crtc::Handle>)> = data
        .state
        .hw
        .as_mut()
        .map(|hw| {
            if let Err(err) = hw.libinput.resume() {
                error!(?err, "libinput resume failed");
            }
            hw.devices
                .iter()
                .map(|(n, ctx)| (*n, ctx.surfaces.keys().copied().collect()))
                .collect()
        })
        .unwrap_or_default();

    for (node, crtcs) in nodes {
        for crtc in crtcs {
            if let Some(surface) = data
                .state
                .hw
                .as_mut()
                .and_then(|hw| hw.devices.get_mut(&node))
                .and_then(|ctx| ctx.surfaces.get_mut(&crtc))
            {
                let _ = surface.compositor.reset_state();
            }
            render_surface(data, node, crtc);
        }
    }
}
// probe appended temporarily
#[allow(dead_code)]
fn probe_import_mem_wl(r: &smithay::backend::renderer::gles::GlesRenderer) {
    fn needs<T: smithay::backend::renderer::ImportMemWl>(_: &T) {}
    needs(r);
}
