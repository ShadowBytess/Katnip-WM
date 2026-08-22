//! Text rasterization for UI surfaces (status bar).
//!
//! Uses fontdue to rasterize glyphs from a discovered system monospace
//! font into RGBA buffers ready for GPU upload via `TextureBuffer`.

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A rendered single-line text run as RGBA pixels.
pub struct RasterText {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Glyph rasterizer bound to one discovered font.
pub struct TextEngine {
    font: fontdue::Font,
}

impl TextEngine {
    /// Discovers a usable system monospace font and builds the engine.
    /// Returns `None` when no candidate can be found or parsed.
    pub fn discover() -> Option<Self> {
        let path = find_font()?;
        match Self::from_file(&path) {
            Some(engine) => {
                debug!(font = %path.display(), "text engine ready");
                Some(engine)
            }
            None => {
                warn!(font = %path.display(), "found font but failed to parse it");
                None
            }
        }
    }

    pub fn from_file(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let font = fontdue::Font::from_bytes(
            bytes,
            fontdue::FontSettings {
                collection_index: 0,
                ..Default::default()
            },
        )
        .ok()?;
        Some(Self { font })
    }

    /// Height of the font's line box at `px` (ascent - descent).
    pub fn line_height(&self, px: f32) -> i32 {
        self.font
            .horizontal_line_metrics(px)
            .map(|m| (m.ascent - m.descent).ceil() as i32)
            .unwrap_or_else(|| px.ceil() as i32)
    }

    /// Advance width of a run in pixels at `px` size.
    pub fn measure(&self, text: &str, px: f32) -> i32 {
        let mut width = 0.0f32;
        for ch in text.chars() {
            let metrics = self.font.metrics(ch, px);
            width += metrics.advance_width;
        }
        width.ceil() as i32
    }

    /// Rasterizes a single-line run into an RGBA buffer sized to the
    /// font's line box at `px`.
    pub fn rasterize(&self, text: &str, px: f32, color: [u8; 4]) -> RasterText {
        let line = self.font.horizontal_line_metrics(px);
        let Some(line) = line else {
            return RasterText {
                rgba: Vec::new(),
                width: 0,
                height: 0,
            };
        };
        let width = self.measure(text, px).max(1) as usize;
        let height = (line.ascent - line.descent).ceil().max(1.0) as usize;
        let mut rgba = vec![0u8; width * height * 4];
        if text.is_empty() {
            return RasterText {
                rgba,
                width,
                height,
            };
        }

        let baseline_y = line.ascent;
        let mut pen_x = 0i64;
        for ch in text.chars() {
            let (metrics, coverage) = self.font.rasterize(ch, px);

            // Glyph top row in image coordinates (y grows downward).
            let top = (baseline_y - metrics.ymin as f32) as i64 - metrics.height as i64;
            let left = pen_x + metrics.xmin as i64;

            for row in 0..metrics.height as i64 {
                let img_y = top + row;
                if img_y < 0 || img_y >= height as i64 {
                    continue;
                }
                for col in 0..metrics.width as i64 {
                    let img_x = left + col;
                    if img_x < 0 || img_x >= width as i64 {
                        continue;
                    }
                    let cov = coverage[(row * metrics.width as i64 + col) as usize] as u32;
                    if cov == 0 {
                        continue;
                    }
                    let idx = ((img_y as usize * width) + img_x as usize) * 4;
                    let alpha = ((cov * color[3] as u32) / 255) as u8;
                    // Straight-alpha overwrite of whatever was there.
                    rgba[idx] = color[0];
                    rgba[idx + 1] = color[1];
                    rgba[idx + 2] = color[2];
                    rgba[idx + 3] = alpha;
                }
            }

            pen_x += metrics.advance_width.round() as i64;
        }

        RasterText {
            rgba,
            width,
            height,
        }
    }
}

/// Preferred font filename fragments, best first.
const FONT_CANDIDATES: &[&str] = &[
    "JetBrainsMono-Regular",
    "JetBrainsMonoNL-Regular",
    "FiraCode-Regular",
    "Hack-Regular",
    "CascadiaCode-Regular",
    "CascadiaMono-Regular",
    "DejaVuSansMono",
    "LiberationMono-Regular",
    "NotoSansMono-Regular",
    "UbuntuMono-R",
    "Inconsolata-Regular",
];

/// Directories scanned recursively for fonts.
fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok();
    let xdg_data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| home.as_ref().map(|h| format!("{h}/.local/share")));
    let dirs = [
        xdg_data.map(|d| format!("{d}/fonts")),
        home.as_ref().map(|h| format!("{h}/.fonts")),
        Some("/usr/share/fonts".into()),
        Some("/usr/local/share/fonts".into()),
    ];
    dirs.into_iter().flatten().map(PathBuf::from).collect()
}

/// Finds the first existing font matching [`FONT_CANDIDATES`], falling back
/// to any TTF/OTF containing "mono", then to any TTF/OTF at all.
fn find_font() -> Option<PathBuf> {
    let mut files = Vec::new();
    for dir in font_dirs() {
        collect_font_files(&dir, 0, &mut files);
    }
    if files.is_empty() {
        return None;
    }

    for candidate in FONT_CANDIDATES {
        for file in &files {
            if file.to_string_lossy().contains(candidate) {
                return Some(file.clone());
            }
        }
    }
    for file in &files {
        if file.to_string_lossy().to_ascii_lowercase().contains("mono") {
            return Some(file.clone());
        }
    }
    files.into_iter().next()
}

/// Recursively collects .ttf/.otf paths under `dir` with depth and count caps.
fn collect_font_files(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    const MAX_DEPTH: u8 = 6;
    if depth > MAX_DEPTH || out.len() > 4096 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, depth + 1, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ttf") | Some("otf")
        ) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_measures_zero_and_rasterizes_blank() {
        let Some(engine) = TextEngine::discover() else {
            // No fonts installed in this environment; nothing to assert.
            return;
        };
        assert_eq!(engine.measure("", 13.0), 0);
        let raster = engine.rasterize("", 13.0, [255, 255, 255, 255]);
        assert!(raster.rgba.iter().all(|b| *b == 0));
    }

    #[test]
    fn ascii_run_has_positive_size() {
        let Some(engine) = TextEngine::discover() else {
            return;
        };
        assert!(engine.measure("1234", 13.0) > 0);
        let raster = engine.rasterize("1", 13.0, [255, 255, 255, 255]);
        assert_eq!(raster.rgba.len(), raster.width * raster.height * 4);
        assert!(
            raster.rgba.chunks(4).any(|px| px[3] > 0),
            "glyph should paint pixels"
        );
    }
}
