//! Hardware backend primitives: session, GPU discovery/open, and the
//! GBM/EGL/GLES rendering context for single-GPU DRM compositing.
//!
//! Orchestration (event sources, connector lifecycle, frame scheduling)
//! lives in the `katnip` crate; this module exposes only smithay types and
//! constructors so the compositor owns its control flow.

use std::path::{Path, PathBuf};

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmNode, NodeType};
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::session::Session;
use smithay::backend::session::libseat::LibSeatSession;
use tracing::{info, warn};

/// Color formats tried in order when creating a DRM swapchain.
pub const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

/// Cursor plane size used for the hardware cursor buffer (logical px).
pub const CURSOR_SIZE: (u32, u32) = (64, 64);

/// An opened session on the current seat (logind or seatd).
pub struct HardwareSession {
    pub session: LibSeatSession,
    /// Session pause/activate events; caller registers with calloop.
    /// Session pause/activate events; taken by the caller to register with
    /// calloop (`None` afterwards).
    pub notifier: Option<smithay::backend::session::libseat::LibSeatSessionNotifier>,
}

impl HardwareSession {
    pub fn new() -> Result<Self, String> {
        let (session, notifier) =
            LibSeatSession::new().map_err(|err| format!("libseat session: {err}"))?;
        Ok(Self {
            session,
            notifier: Some(notifier),
        })
    }

    pub fn seat(&self) -> String {
        self.session.seat()
    }

    /// Opens a device path through the session (grants DRM master).
    pub fn open_device(&mut self, path: &Path) -> Result<DrmDeviceFd, String> {
        let fd = self.session.open(
            path,
            smithay::reexports::rustix::fs::OFlags::RDWR
                | smithay::reexports::rustix::fs::OFlags::CLOEXEC
                | smithay::reexports::rustix::fs::OFlags::NOCTTY
                | smithay::reexports::rustix::fs::OFlags::NONBLOCK,
        );
        match fd {
            Ok(fd) => Ok(DrmDeviceFd::new(fd.into())),
            Err(err) => Err(format!("session open {}: {err}", path.display())),
        }
    }
}

/// Picks the primary GPU: `$KATNIP_DRM_DEVICE` override, else the seat's
/// primary GPU preferring its render node, else the first GPU found.
pub fn select_primary_gpu(seat: &str) -> Result<(DrmNode, PathBuf), String> {
    if let Ok(var) = std::env::var("KATNIP_DRM_DEVICE") {
        let node = DrmNode::from_path(&var).map_err(|err| format!("{var}: {err}"))?;
        return Ok((node, PathBuf::from(&var)));
    }

    if let Ok(Some(path)) = smithay::backend::udev::primary_gpu(seat) {
        if let Ok(node) = DrmNode::from_path(&path) {
            // Prefer a render node so buffers can be shared without DRM
            // master on the scanout device.
            let usable = match node.node_with_type(NodeType::Render) {
                Some(Ok(render)) => render,
                _ => node,
            };
            return Ok((usable, path));
        }
    }

    let all = smithay::backend::udev::all_gpus(seat).map_err(|err| format!("all_gpus: {err}"))?;
    all.into_iter()
        .find_map(|path| DrmNode::from_path(&path).ok().map(|node| (node, path)))
        .ok_or_else(|| "no suitable DRM device found".into())
}

/// Everything needed to render on one GPU.
pub struct GpuContext {
    pub drm: DrmDevice,
    pub gbm: GbmDevice<DrmDeviceFd>,
    pub allocator: GbmAllocator<DrmDeviceFd>,
    pub exporter: GbmFramebufferExporter<DrmDeviceFd>,
    pub renderer: GlesRenderer,
    pub render_node: Option<DrmNode>,
    /// vblank notifier; the caller registers it with calloop.
    pub notifier: DrmDeviceNotifier,
}

/// Opens a DRM device and binds a GLES renderer to it via EGL/GBM.
pub fn open_gpu(session: &mut HardwareSession, path: &Path) -> Result<GpuContext, String> {
    let fd = session.open_device(path)?;
    let (drm, notifier) =
        DrmDevice::new(fd.clone(), true).map_err(|err| format!("DrmDevice: {err}"))?;
    let gbm = GbmDevice::new(fd).map_err(|err| format!("GbmDevice: {err}"))?;

    // SAFETY: the underlying gbm device is owned by `gbm`, which outlives
    // the EGL display/context created from it.
    let egl_display =
        unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|err| format!("EGLDisplay: {err}"))?;
    let egl_device =
        EGLDevice::device_for_display(&egl_display).map_err(|err| format!("EGLDevice: {err}"))?;
    if egl_device.is_software() {
        warn!("GPU reports software rendering; continuing anyway");
    }
    let render_node = egl_device.try_get_render_node().ok().flatten();

    let egl_context = EGLContext::new(&egl_display).map_err(|err| format!("EGLContext: {err}"))?;
    // SAFETY: single-threaded compositor startup; the context is used only
    // from the event-loop thread.
    let renderer =
        unsafe { GlesRenderer::new(egl_context) }.map_err(|err| format!("GlesRenderer: {err}"))?;

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);

    info!(?render_node, "GPU context ready");

    Ok(GpuContext {
        drm,
        gbm,
        allocator,
        exporter,
        renderer,
        render_node,
        notifier,
    })
}

/// Detects the `non-desktop` property (VR / lease connectors).
pub fn connector_is_non_desktop(
    drm: &DrmDevice,
    connector: smithay::reexports::drm::control::connector::Handle,
) -> bool {
    use smithay::reexports::drm::control::Device as ControlDevice;
    drm.get_properties(connector)
        .ok()
        .and_then(|props| {
            props
                .into_iter()
                .filter_map(|(handle, value)| {
                    let info = drm.get_property(handle).ok()?;
                    Some((info, value))
                })
                .find(|(info, _)| info.name().to_str() == Ok("non-desktop"))
                .and_then(|(info, value)| info.value_type().convert_value(value).as_boolean())
        })
        .unwrap_or(false)
}
