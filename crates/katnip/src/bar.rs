//! Built-in status bar: workspaces on the left, window title centered,
//! clock on the right, drawn directly into the output render pass.

use std::collections::HashMap;

use chrono::Local;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, Renderer};
use smithay::utils::{Point, Rectangle, Size, Transform};

use crate::state::WORKSPACE_COUNT;
use crate::text::TextEngine;

use smithay::backend::renderer::element::solid::SolidColorRenderElement;

smithay::backend::renderer::element::render_elements! {
    /// Custom compositor-drawn elements: window borders and bar content.
    pub KatnipElements<R> where
        R: Renderer;
    Solid = SolidColorRenderElement,
    Text = TextureRenderElement<R::TextureId>,
}

/// Bar background (slightly lifted from the clear color).
const BAR_BG: Color32F = Color32F::new(0.075, 0.092, 0.086, 1.0);
/// Regular text.
const FG: [u8; 4] = [200, 205, 202, 255];
/// Dim text (inactive workspaces).
const DIM: [u8; 4] = [120, 128, 124, 255];
/// Accent teal - matches the focused border color.
const ACCENT: [u8; 4] = [77, 217, 178, 255];

const TEXT_PX: f32 = 13.0;
const PAD: i32 = 10;
const WS_GAP: i32 = 10;

/// Snapshot of what the bar should display this frame.
pub struct BarInfo {
    pub active_workspace: usize,
    pub title: Option<String>,
}

/// The built-in status bar.
pub struct Bar {
    pub enabled: bool,
    pub height: i32,
    engine: Option<TextEngine>,
    textures: HashMap<String, TextureBuffer<GlesTexture>>,
}

impl Bar {
    pub fn new(enabled: bool, height: i32) -> Self {
        let engine = TextEngine::discover();
        if engine.is_none() {
            tracing::warn!("no suitable font found; bar will render without text");
        }
        Self {
            enabled,
            height,
            engine,
            textures: HashMap::new(),
        }
    }

    /// Builds this frame's bar elements in physical output coordinates.
    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        width_logical: i32,
        scale: f64,
        info: &BarInfo,
    ) -> Vec<KatnipElements<GlesRenderer>> {
        let mut out = Vec::with_capacity(16);

        // Background strip across the top.
        let rect = Rectangle::<i32, smithay::utils::Physical>::new(
            Point::from((0, 0)).to_physical_precise_round(scale),
            Size::from((width_logical, self.height)).to_physical_precise_round(scale),
        );
        out.push(KatnipElements::Solid(SolidColorRenderElement::new(
            Id::new(),
            rect,
            CommitCounter::default(),
            BAR_BG,
            Kind::Unspecified,
        )));

        let Some(engine) = &self.engine else {
            return out;
        };
        let textures = &mut self.textures;

        let center_y = ((self.height - engine.line_height(TEXT_PX)) / 2).max(0);

        // -- left: workspace indicators -----------------------------------
        let mut x = PAD;
        for i in 0..WORKSPACE_COUNT {
            let label = (i + 1).to_string();
            let active = i == info.active_workspace;
            let color = if active { ACCENT } else { DIM };
            let w = engine.measure(&label, TEXT_PX);
            Self::push_text(
                engine, textures, renderer, &mut out, &label, TEXT_PX, color, x, center_y, scale,
            );
            x += w + WS_GAP;
        }

        // -- center: focused window title ---------------------------------
        if let Some(title) = info.title.as_deref().filter(|t| !t.is_empty()) {
            let max_width = width_logical / 2;
            let shown = truncate_to_width(engine, title, max_width, TEXT_PX);
            let tw = engine.measure(&shown, TEXT_PX);
            let tx = (width_logical - tw) / 2;
            if tx > 0 && tw <= max_width {
                Self::push_text(
                    engine, textures, renderer, &mut out, &shown, TEXT_PX, FG, tx, center_y, scale,
                );
            }
        }

        // -- right: clock --------------------------------------------------
        let clock = Local::now().format("%H:%M").to_string();
        let cw = engine.measure(&clock, TEXT_PX);
        Self::push_text(
            engine,
            textures,
            renderer,
            &mut out,
            &clock,
            TEXT_PX,
            FG,
            width_logical - cw - PAD,
            center_y,
            scale,
        );

        out
    }

    /// Uploads (or reuses) a texture for this text and appends its element.
    #[allow(clippy::too_many_arguments)]
    fn push_text(
        engine: &TextEngine,
        textures: &mut HashMap<String, TextureBuffer<GlesTexture>>,
        renderer: &mut GlesRenderer,
        out: &mut Vec<KatnipElements<GlesRenderer>>,
        text: &str,
        px: f32,
        color: [u8; 4],
        x: i32,
        y: i32,
        scale: f64,
    ) {
        if text.is_empty() || x < 0 {
            return;
        }
        let key = format!("{text}|{px}|{color:?}");
        if !textures.contains_key(&key) {
            if textures.len() > 256 {
                // Clock strings churn slowly; a wholesale refresh is fine.
                textures.clear();
            }
            let raster = engine.rasterize(text, px, color);
            match TextureBuffer::from_memory(
                renderer,
                &raster.rgba,
                Fourcc::Abgr8888,
                Size::from((raster.width as i32, raster.height as i32)),
                false,
                1,
                Transform::Normal,
                None,
            ) {
                Ok(buffer) => {
                    textures.insert(key.clone(), buffer);
                }
                Err(err) => {
                    tracing::warn!(%err, "failed to upload bar text");
                    return;
                }
            }
        }
        let buffer = &textures[&key];
        let location = Point::from((x, y))
            .to_f64()
            .to_physical_precise_round(scale);
        out.push(KatnipElements::Text(
            TextureRenderElement::from_texture_buffer(
                location,
                buffer,
                None,
                None,
                None,
                Kind::Unspecified,
            ),
        ));
    }
}

impl BarInfo {
    /// Assembles display info from compositor state.
    pub fn from_state(state: &crate::state::Katnip) -> Self {
        Self {
            active_workspace: state.active_workspace,
            title: state.focused_title(),
        }
    }
}

/// Cuts `text` down to at most `max_px` wide, appending an ellipsis.
fn truncate_to_width(engine: &TextEngine, text: &str, max_px: i32, px: f32) -> String {
    if engine.measure(text, px) <= max_px {
        return text.to_string();
    }
    let ellipsis = "\u{2026}";
    let mut cut: String = text.to_string();
    while cut.chars().count() > 1 {
        let mut candidate = cut.clone();
        candidate.pop();
        candidate.push_str(ellipsis);
        if engine.measure(&candidate, px) <= max_px {
            return candidate;
        }
        cut.pop();
    }
    ellipsis.to_string()
}
