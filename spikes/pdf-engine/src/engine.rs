//! PDFium binding, document opening, rendering, and per-character text extraction.
//!
//! The engine keeps two `PdfiumLibraryBindings` instances: one is handed to
//! `pdfium-render`'s high-level API, the other is retained for the few calls the
//! high-level API does not expose (notably `FPDF_SaveAsCopy` with
//! `FPDF_INCREMENTAL`, whose flag `pdfium-render` 0.9.4 hardcodes to 0).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use pdfium_render::prelude::*;

/// One extracted character, with the attributes the editor needs for a Text Object.
#[derive(Debug, Clone, PartialEq)]
pub struct CharInfo {
    pub unicode: Option<char>,
    pub font_name: String,
    pub font_size: f32,
    /// RGB fill color.
    pub fill_color: [u8; 3],
    /// Baseline origin in PDF page coordinates (origin bottom-left).
    pub origin: (f32, f32),
    /// Loose glyph box in PDF page coordinates: (left, bottom, right, top).
    pub bounds: (f32, f32, f32, f32),
}

/// Aggregate pixel statistics for a rendered page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    pub width: i32,
    pub height: i32,
    pub non_white_pixels: usize,
    pub dark_pixels: usize,
}

/// A rendered page as RGBA bytes (the format PDFium produces).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

impl RenderedPage {
    pub fn stats(&self) -> RenderStats {
        let mut non_white = 0usize;
        let mut dark = 0usize;
        for pixel in self.rgba.chunks_exact(4) {
            let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
            if r < 245 || g < 245 || b < 245 {
                non_white += 1;
            }
            if r < 96 && g < 96 && b < 96 {
                dark += 1;
            }
        }
        RenderStats {
            width: self.width,
            height: self.height,
            non_white_pixels: non_white,
            dark_pixels: dark,
        }
    }
}

/// A shared PDFium binding plus raw bindings for save operations.
pub struct SpikeEngine {
    pdfium: Pdfium,
    raw: Box<dyn PdfiumLibraryBindings>,
}

static ENGINE: OnceLock<std::result::Result<SpikeEngine, String>> = OnceLock::new();

impl SpikeEngine {
    /// Returns the process-wide engine, initializing PDFium on first use.
    ///
    /// PDFium's library initialization is global; a second `Pdfium` instance cannot
    /// be created in the same process, hence the shared singleton.
    pub fn shared() -> Result<&'static SpikeEngine> {
        let init = ENGINE.get_or_init(|| Self::bind_from_env().map_err(|error| format!("{error:#}")));
        match init {
            Ok(engine) => Ok(engine),
            Err(message) => Err(anyhow!("{message}")),
        }
    }

    fn bind_from_env() -> Result<Self> {
        let lib_path = pdfium_library_path()?;
        Self::bind(&lib_path)
    }

    /// Binds both a high-level and a raw view of the PDFium library at `lib_path`.
    ///
    /// Both calls must happen before `Pdfium::new`; `pdfium-render` records the first
    /// bindings in a process-wide `OnceCell` and refuses later binds.
    pub fn bind(lib_path: impl AsRef<Path>) -> Result<Self> {
        let lib_path = lib_path.as_ref();
        let raw = Pdfium::bind_to_library(lib_path).map_err(|error| {
            anyhow!(
                "failed to bind PDFium at {}: {error}. Run spikes/pdf-engine/scripts/setup-native.sh",
                lib_path.display()
            )
        })?;
        let high_level = Pdfium::bind_to_library(lib_path)
            .map_err(|error| anyhow!("failed to bind PDFium twice at {}: {error}", lib_path.display()))?;
        Ok(Self {
            pdfium: Pdfium::new(high_level),
            raw,
        })
    }

    pub fn pdfium(&self) -> &Pdfium {
        &self.pdfium
    }

    /// Raw PDFium function table retained for calls the high-level API cannot make.
    pub fn raw(&self) -> &dyn PdfiumLibraryBindings {
        self.raw.as_ref()
    }

    pub fn open_file<'a>(
        &'a self,
        path: &Path,
        password: Option<&str>,
    ) -> Result<PdfDocument<'a>> {
        self.pdfium
            .load_pdf_from_file(path, password)
            .with_context(|| format!("open {}", path.display()))
    }

    pub fn open_bytes<'a>(&'a self, bytes: &'a [u8], password: Option<&str>) -> Result<PdfDocument<'a>> {
        self.pdfium
            .load_pdf_from_byte_slice(bytes, password)
            .context("open PDF from memory")
    }

    /// Renders `page` at `scale` (1.0 = 72 DPI).
    pub fn render(&self, page: &PdfPage<'_>, scale: f32) -> Result<RenderedPage> {
        let width = ((page.width().value * scale).round() as i32).max(1);
        let height = ((page.height().value * scale).round() as i32).max(1);
        let bitmap = page
            .render(width, height, None)
            .context("render page to bitmap")?;
        Ok(RenderedPage {
            width,
            height,
            rgba: bitmap.as_rgba_bytes(),
        })
    }

    /// Renders `page` at `scale` (1.0 = 72 DPI) and counts visible pixels.
    pub fn render_stats(&self, page: &PdfPage<'_>, scale: f32) -> Result<RenderStats> {
        Ok(self.render(page, scale)?.stats())
    }

    /// Extracts every character on `page` with font, size, color, and position.
    pub fn extract_chars(&self, page: &PdfPage<'_>) -> Result<Vec<CharInfo>> {
        let text = page.text().context("read page text layer")?;
        let mut chars = Vec::with_capacity(text.len().max(0) as usize);
        for character in text.chars().iter() {
            let fill_color = character
                .fill_color()
                .map(|color| [color.red(), color.green(), color.blue()])
                .unwrap_or([0, 0, 0]);
            let origin = character
                .origin()
                .map(|(x, y)| (x.value, y.value))
                .unwrap_or((0.0, 0.0));
            let bounds = character
                .loose_bounds()
                .map(|rect| {
                    (
                        rect.left().value,
                        rect.bottom().value,
                        rect.right().value,
                        rect.top().value,
                    )
                })
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            chars.push(CharInfo {
                unicode: character.unicode_char(),
                font_name: character.font_name(),
                font_size: character.scaled_font_size().value,
                fill_color,
                origin,
                bounds,
            });
        }
        Ok(chars)
    }
}

/// Resolves the PDFium dynamic library: explicit env var first, then the vendored path.
pub fn pdfium_library_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        return Ok(PathBuf::from(path));
    }
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/pdfium");
    let candidate = if cfg!(target_os = "windows") {
        vendor.join("bin/pdfium.dll")
    } else if cfg!(target_os = "macos") {
        vendor.join("lib/libpdfium.dylib")
    } else {
        vendor.join("lib/libpdfium.so")
    };
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(anyhow!(
            "PDFium native library not found at {}; set PDFIUM_DYNAMIC_LIB_PATH or run spikes/pdf-engine/scripts/setup-native.sh",
            candidate.display()
        ))
    }
}

/// Resolves the CJK probe font: explicit env var first, then the vendored path.
pub fn cjk_font_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("PDFX_CJK_FONT") {
        return Ok(PathBuf::from(path));
    }
    let candidate =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/fonts/NotoSansSC-VF.ttf");
    if candidate.exists() {
        Ok(candidate)
    } else {
        Err(anyhow!(
            "CJK font not found at {}; set PDFX_CJK_FONT or run spikes/pdf-engine/scripts/setup-native.sh",
            candidate.display()
        ))
    }
}
