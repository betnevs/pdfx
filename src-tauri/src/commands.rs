use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfPage, PdfRenderConfig, PdfiumError,
    PdfiumInternalError,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use crate::engine::Engine;

/// Rendering limits. A full-page bitmap above this size is rejected instead of
/// exhausting memory; the frontend switches to `render_page_region` (viewport
/// tiling) at high zoom.
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 40.0;
const MAX_RENDER_PIXELS: i64 = 64_000_000;

/// Thumbnail rendering bounds. `max_edge` is the long edge in pixels.
const MIN_THUMBNAIL_EDGE: u32 = 16;
const MAX_THUMBNAIL_EDGE: u32 = 1024;

/// Application identity returned across the Tauri command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

/// The `app_info` command: the first seam every frontend feature uses.
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "PDFX".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Stable, serializable error codes for the command boundary.
///
/// The frontend maps these codes to localized copy; `message` carries the
/// technical detail for logs and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    FileNotFound,
    FileReadFailed,
    PermissionDenied,
    PdfiumUnavailable,
    PasswordRequired,
    InvalidPassword,
    CorruptDocument,
    DocumentNotFound,
    PageOutOfRange,
    InvalidScale,
    InvalidArgument,
    RenderFailed,
    WindowFailed,
    Internal,
}

/// A structured error crossing the Tauri command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    pub fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// A document as seen by the frontend. It never contains PDFium handles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    pub document_id: String,
    pub name: String,
    pub path: String,
    pub page_count: u32,
    pub page_width: f32,
    pub page_height: f32,
}

/// A rendered page encoded as PNG. The payload is base64 so it crosses the IPC
/// boundary as JSON without a second binary channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageImageView {
    pub document_id: String,
    pub page_index: u32,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub png_base64: String,
}

/// Display metadata for one page. Dimensions already include the page's
/// intrinsic rotation, matching what PDFium renders; `rotation` lets the
/// frontend map between display coordinates and PDF user space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfoView {
    pub page_index: u32,
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
    /// Lower-left corner of the CropBox in PDF page coordinates. Overlay
    /// coordinates are relative to the cropped, displayed page.
    pub crop_x: f32,
    pub crop_y: f32,
}

/// A viewport-sized region of a rendered page. `x` and `y` are offsets in
/// display pixels at the requested scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRegionView {
    pub document_id: String,
    pub page_index: u32,
    pub scale: f32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub png_base64: String,
}

/// Extracted text for one page. Scanned pages legitimately return an empty
/// string; that is a document property, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedTextView {
    pub page_index: u32,
    pub text: String,
    pub char_count: u32,
}

struct OpenDocument {
    /// The unmodified bytes read from disk, kept for the incremental-save
    /// pipeline. PDFium owns its own copy of the input buffer.
    #[allow(dead_code)]
    source_bytes: Vec<u8>,
    document: PdfDocument<'static>,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    documents: HashMap<String, OpenDocument>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

/// PDFium records the last load error in process-global state. `pdfium-render`
/// reads `FPDF_GetLastError` after the load call, so two concurrent opens could
/// otherwise observe each other's error code. Loading is rare and cheap relative
/// to rendering, so serialize it.
static OPEN_LOCK: Mutex<()> = Mutex::new(());

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// The `open_document` command.
#[tauri::command]
pub fn open_document(path: String, password: Option<String>) -> Result<DocumentView, CommandError> {
    let path = PathBuf::from(path);
    let bytes = fs::read(&path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => CommandError::new(
            CommandErrorCode::FileNotFound,
            format!("{}: {error}", path.display()),
        ),
        std::io::ErrorKind::PermissionDenied => CommandError::new(
            CommandErrorCode::PermissionDenied,
            format!("{}: {error}", path.display()),
        ),
        _ => CommandError::new(
            CommandErrorCode::FileReadFailed,
            format!("{}: {error}", path.display()),
        ),
    })?;

    let engine = Engine::shared()
        .map_err(|message| CommandError::new(CommandErrorCode::PdfiumUnavailable, message))?;

    let document = {
        let _load_guard = OPEN_LOCK.lock().expect("PDFium open lock poisoned");
        engine
            .open_bytes(bytes.clone(), password.as_deref())
            .map_err(|error| map_open_error(&error, password.is_some()))?
    };

    let page_count = document.pages().len();
    if page_count <= 0 {
        return Err(CommandError::new(
            CommandErrorCode::CorruptDocument,
            "document contains no pages",
        ));
    }

    let (page_width, page_height) = {
        let page = document.pages().get(0).map_err(|error| {
            CommandError::new(CommandErrorCode::CorruptDocument, format!("{error:?}"))
        })?;
        (page.width().value, page.height().value)
    };

    let view = {
        let mut registry = registry().lock().expect("document registry poisoned");
        registry.next_id += 1;
        let document_id = format!("doc-{}", registry.next_id);
        let view = DocumentView {
            document_id: document_id.clone(),
            name: document_name(&path),
            path: path.to_string_lossy().into_owned(),
            page_count: page_count as u32,
            page_width,
            page_height,
        };
        registry.documents.insert(
            document_id,
            OpenDocument {
                source_bytes: bytes,
                document,
            },
        );
        view
    };

    Ok(view)
}

/// The `close_document` command. Closing an unknown document is a no-op so a
/// window teardown is always safe.
#[tauri::command]
pub fn close_document(document_id: String) -> Result<(), CommandError> {
    let mut registry = registry().lock().expect("document registry poisoned");
    registry.documents.remove(&document_id);
    Ok(())
}

static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The `open_document_window` command.
///
/// Each Document gets its own window. The new window receives its file through
/// the launch URL and opens it through the same `open_document` command, so a
/// window that already hosts a Document is never replaced.
#[tauri::command]
pub fn open_document_window(app: AppHandle, path: String) -> Result<(), CommandError> {
    let sequence = WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    let label = format!("document-{sequence}");
    let url = format!(
        "index.html?open={}",
        percent_encoding::utf8_percent_encode(&path, percent_encoding::NON_ALPHANUMERIC)
    );

    WebviewWindowBuilder::new(&app, label, WebviewUrl::App(url.into()))
        .title("PDFX")
        .inner_size(1200.0, 800.0)
        .min_inner_size(800.0, 600.0)
        .build()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::WindowFailed,
                format!("could not create a document window: {error}"),
            )
        })?;

    Ok(())
}

/// The `page_info` command. Returns display dimensions and intrinsic rotation
/// for one page, so the frontend never reads PDF internals itself.
#[tauri::command]
pub fn page_info(document_id: String, page_index: u32) -> Result<PageInfoView, CommandError> {
    let registry = registry().lock().expect("document registry poisoned");
    let entry = lookup_document(&registry, &document_id)?;
    let page = load_page(&entry.document, page_index)?;

    let rotation = page.rotation().map_err(|error| {
        CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("could not read page {page_index} rotation: {error:?}"),
        )
    })?;

    // A missing CropBox falls back to the page origin; the display dimensions
    // above already reflect whatever box PDFium renders.
    let (crop_x, crop_y) = page
        .boundaries()
        .crop()
        .map(|boundary| (boundary.bounds.left().value, boundary.bounds.bottom().value))
        .unwrap_or((0.0, 0.0));

    Ok(PageInfoView {
        page_index,
        width: page.width().value,
        height: page.height().value,
        rotation: rotation.as_degrees() as u16,
        crop_x,
        crop_y,
    })
}

/// The `render_page` command. `scale` is relative to 72 DPI.
#[tauri::command]
pub fn render_page(
    document_id: String,
    page_index: u32,
    scale: f32,
) -> Result<PageImageView, CommandError> {
    validate_scale(scale)?;

    let registry = registry().lock().expect("document registry poisoned");
    let entry = lookup_document(&registry, &document_id)?;
    let page = load_page(&entry.document, page_index)?;

    let (pixel_width, pixel_height) = checked_page_pixels(&page, page_index, scale)?;
    let png = render_full_page_png(&page, page_index, pixel_width, pixel_height)?;

    Ok(PageImageView {
        document_id,
        page_index,
        scale,
        width: pixel_width,
        height: pixel_height,
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

/// The `render_thumbnail` command. Renders a low-resolution page whose long
/// edge is at most `max_edge` pixels; thumbnails bypass the normal scale floor
/// because they are intentionally much smaller than a readable page.
#[tauri::command]
pub fn render_thumbnail(
    document_id: String,
    page_index: u32,
    max_edge: u32,
) -> Result<PageImageView, CommandError> {
    if !(MIN_THUMBNAIL_EDGE..=MAX_THUMBNAIL_EDGE).contains(&max_edge) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!(
                "thumbnail max_edge must be between {MIN_THUMBNAIL_EDGE} and \
                 {MAX_THUMBNAIL_EDGE}, got {max_edge}"
            ),
        ));
    }

    let registry = registry().lock().expect("document registry poisoned");
    let entry = lookup_document(&registry, &document_id)?;
    let page = load_page(&entry.document, page_index)?;

    let long_edge = page.width().value.max(page.height().value);
    if !long_edge.is_finite() || long_edge <= 0.0 {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("page {page_index} has no renderable area"),
        ));
    }

    let scale = max_edge as f32 / long_edge;
    let (pixel_width, pixel_height) = checked_page_pixels(&page, page_index, scale)?;
    let png = render_full_page_png(&page, page_index, pixel_width, pixel_height)?;

    Ok(PageImageView {
        document_id,
        page_index,
        scale,
        width: pixel_width,
        height: pixel_height,
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

/// The `render_page_region` command. Renders only a viewport-sized window of a
/// page at any supported zoom, which keeps 4000% usable without allocating a
/// full-page bitmap. Coordinates and sizes are display pixels at `scale`;
/// the requested region is clamped to the page.
#[tauri::command]
pub fn render_page_region(
    document_id: String,
    page_index: u32,
    scale: f32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<PageRegionView, CommandError> {
    validate_scale(scale)?;

    if width == 0 || height == 0 {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("region must have a positive size, got {width}x{height}"),
        ));
    }

    let registry = registry().lock().expect("document registry poisoned");
    let entry = lookup_document(&registry, &document_id)?;
    let page = load_page(&entry.document, page_index)?;

    let (full_width, full_height) = page_pixel_size(&page, scale);
    if full_width > i32::MAX as u32 || full_height > i32::MAX as u32 {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("page {page_index} is too large to render at {scale}x"),
        ));
    }

    if x >= full_width || y >= full_height {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!(
                "region origin ({x}, {y}) is outside the {full_width}x{full_height} \
                 page at {scale}x"
            ),
        ));
    }

    let width = width.min(full_width - x);
    let height = height.min(full_height - y);

    if width as i64 * height as i64 > MAX_RENDER_PIXELS {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!(
                "region of {width}x{height} pixels is above the \
                 {MAX_RENDER_PIXELS}-pixel limit"
            ),
        ));
    }

    let config = PdfRenderConfig::new()
        .set_fixed_size(full_width as i32, full_height as i32)
        .set_origin(-(x as i32), -(y as i32));

    let mut bitmap = PdfBitmap::empty(
        width as i32,
        height as i32,
        PdfBitmapFormat::default(),
    )
    .map_err(|error| {
        CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("could not allocate a {width}x{height} bitmap: {error:?}"),
        )
    })?;

    page.render_into_bitmap_with_config(&mut bitmap, &config)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("PDFium could not render region of page {page_index}: {error:?}"),
            )
        })?;

    let rgba = bitmap.as_rgba_bytes();
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!(
                "rendered region bitmap has {} bytes, expected {expected}",
                rgba.len()
            ),
        ));
    }

    let png = encode_png(&rgba, width, height)?;

    Ok(PageRegionView {
        document_id,
        page_index,
        scale,
        x,
        y,
        width,
        height,
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
    })
}

fn validate_scale(scale: f32) -> Result<(), CommandError> {
    if !scale.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidScale,
            format!("scale must be between {MIN_SCALE} and {MAX_SCALE}, got {scale}"),
        ));
    }
    Ok(())
}

fn lookup_document<'a>(
    registry: &'a Registry,
    document_id: &str,
) -> Result<&'a OpenDocument, CommandError> {
    registry.documents.get(document_id).ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::DocumentNotFound,
            format!("no open document with id {document_id}"),
        )
    })
}

fn load_page<'a>(
    document: &'a PdfDocument<'static>,
    page_index: u32,
) -> Result<PdfPage<'a>, CommandError> {
    let pages = document.pages();
    if page_index as i32 >= pages.len() {
        return Err(CommandError::new(
            CommandErrorCode::PageOutOfRange,
            format!("page {page_index} is outside 0..{}", pages.len()),
        ));
    }

    pages.get(page_index as i32).map_err(|error| {
        CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("could not load page {page_index}: {error:?}"),
        )
    })
}

/// Display pixel dimensions for a page at a scale, enforcing the full-page
/// bitmap budget.
fn checked_page_pixels(
    page: &PdfPage<'_>,
    page_index: u32,
    scale: f32,
) -> Result<(u32, u32), CommandError> {
    let (width, height) = page_pixel_size(page, scale);
    if width as i64 * height as i64 > MAX_RENDER_PIXELS {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!(
                "rendering page {page_index} at {scale}x would produce {width}x{height} \
                 pixels, above the {MAX_RENDER_PIXELS}-pixel limit; use a page region"
            ),
        ));
    }
    Ok((width, height))
}

fn page_pixel_size(page: &PdfPage<'_>, scale: f32) -> (u32, u32) {
    let width = (page.width().value * scale).round().max(1.0);
    let height = (page.height().value * scale).round().max(1.0);
    (width as u32, height as u32)
}

fn render_full_page_png(
    page: &PdfPage<'_>,
    page_index: u32,
    pixel_width: u32,
    pixel_height: u32,
) -> Result<Vec<u8>, CommandError> {
    let bitmap = page
        .render(pixel_width as i32, pixel_height as i32, None)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("PDFium could not render page {page_index}: {error:?}"),
            )
        })?;

    let rgba = bitmap.as_rgba_bytes();
    let expected = pixel_width as usize * pixel_height as usize * 4;
    if rgba.len() != expected {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!(
                "rendered bitmap has {} bytes, expected {expected}",
                rgba.len()
            ),
        ));
    }

    encode_png(&rgba, pixel_width, pixel_height)
}

/// The `extract_text` command. Returns an empty text layer for scanned pages.
#[tauri::command]
pub fn extract_text(
    document_id: String,
    page_index: u32,
) -> Result<ExtractedTextView, CommandError> {
    let registry = registry().lock().expect("document registry poisoned");
    let entry = registry.documents.get(&document_id).ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::DocumentNotFound,
            format!("no open document with id {document_id}"),
        )
    })?;

    let pages = entry.document.pages();
    if page_index as i32 >= pages.len() {
        return Err(CommandError::new(
            CommandErrorCode::PageOutOfRange,
            format!("page {page_index} is outside 0..{}", pages.len()),
        ));
    }

    let page = pages.get(page_index as i32).map_err(|error| {
        CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("could not load page {page_index}: {error:?}"),
        )
    })?;

    // A missing text layer is normal for scanned documents.
    let text = page
        .text()
        .map(|page_text| page_text.all())
        .unwrap_or_default();

    Ok(ExtractedTextView {
        page_index,
        char_count: text.chars().count() as u32,
        text,
    })
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CommandError> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not start PNG encoding: {error}"),
            )
        })?;
        writer.write_image_data(rgba).map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not encode rendered page as PNG: {error}"),
            )
        })?;
    }
    Ok(output)
}

fn document_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn map_open_error(error: &PdfiumError, password_supplied: bool) -> CommandError {
    if let PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) = error {
        return if password_supplied {
            CommandError::new(
                CommandErrorCode::InvalidPassword,
                "the supplied password did not open the document",
            )
        } else {
            CommandError::new(
                CommandErrorCode::PasswordRequired,
                "this document requires a password",
            )
        };
    }

    CommandError::new(
        CommandErrorCode::CorruptDocument,
        format!("PDFium could not open the document: {error:?}"),
    )
}
