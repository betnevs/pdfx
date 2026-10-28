use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfPageBoundaryBoxType, PdfiumError, PdfiumInternalError,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

use crate::engine::Engine;

/// Rendering limits. Full-page bitmaps above this size are rejected instead of
/// exhausting memory; tiled rendering for very high zoom arrives with the
/// viewport work.
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 40.0;
const MAX_RENDER_PIXELS: i64 = 64_000_000;

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

/// Extracted text for one page. Scanned pages legitimately return an empty
/// string; that is a document property, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedTextView {
    pub page_index: u32,
    pub text: String,
    pub char_count: u32,
}

/// One selectable character of a page's text layer.
///
/// All geometry is in display-space points: the coordinate system of the
/// rendered page bitmap divided by the render scale, with the origin at the
/// page's top-left corner. Intrinsic page rotation and the CropBox origin are
/// already applied, so overlays can scale these rectangles by the render scale
/// without knowing anything else about the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextCharView {
    /// Unicode text of the character. Surrogate pairs are combined.
    pub text: String,
    /// Left edge in display-space points.
    pub x: f32,
    /// Top edge in display-space points.
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Baseline origin in display-space points.
    pub origin_x: f32,
    pub origin_y: f32,
    pub font_size: f32,
    /// PDFium generated this character (line breaks and similar spacing).
    /// Generated characters have a degenerate box and are not hit-testable,
    /// but they carry the page's line breaks when copying.
    pub generated: bool,
}

/// The positioned text layer of one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLayerView {
    pub page_index: u32,
    /// Display width in points, with intrinsic rotation applied.
    pub page_width: f32,
    /// Display height in points, with intrinsic rotation applied.
    pub page_height: f32,
    /// Intrinsic page rotation in degrees, clockwise (0, 90, 180 or 270).
    pub rotation: u16,
    /// Lower-left corner of the CropBox in PDF user space. Character geometry
    /// is already relative to the cropped, displayed page.
    pub crop_x: f32,
    pub crop_y: f32,
    /// False for scanned pages and pages whose text is entirely outside the
    /// visible page box. The frontend shows the "no editable text" state.
    pub has_text: bool,
    /// Visible text in character order. Equals the concatenation of
    /// [`TextCharView::text`].
    pub text: String,
    pub chars: Vec<TextCharView>,
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

/// The `render_page` command. `scale` is relative to 72 DPI.
#[tauri::command]
pub fn render_page(
    document_id: String,
    page_index: u32,
    scale: f32,
) -> Result<PageImageView, CommandError> {
    if !scale.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidScale,
            format!("scale must be between {MIN_SCALE} and {MAX_SCALE}, got {scale}"),
        ));
    }

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

    let pixel_width = (page.width().value * scale).round().max(1.0);
    let pixel_height = (page.height().value * scale).round().max(1.0);

    if pixel_width > i32::MAX as f32
        || pixel_height > i32::MAX as f32
        || pixel_width as i64 * pixel_height as i64 > MAX_RENDER_PIXELS
    {
        return Err(CommandError::new(
            CommandErrorCode::RenderFailed,
            format!(
                "rendering at {scale}x would produce {pixel_width}x{pixel_height} pixels, \
                 above the {MAX_RENDER_PIXELS}-pixel limit"
            ),
        ));
    }

    let (pixel_width, pixel_height) = (pixel_width as u32, pixel_height as u32);
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

    let png = encode_png(&rgba, pixel_width, pixel_height)?;

    Ok(PageImageView {
        document_id,
        page_index,
        scale,
        width: pixel_width,
        height: pixel_height,
        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
    })
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

/// The `extract_text_layer` command. Returns positioned characters for one
/// page so the frontend can hit-test, select, highlight and copy text.
///
/// Coordinates are display-space points (top-left origin, intrinsic rotation
/// applied), which is the rendered bitmap's coordinate system divided by the
/// render scale. Scanned pages return an empty layer.
#[tauri::command]
pub fn extract_text_layer(
    document_id: String,
    page_index: u32,
) -> Result<TextLayerView, CommandError> {
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

    build_text_layer(&page, page_index)
}

/// Display-space transform for one page: the box PDFium renders and the
/// intrinsic rotation it applies.
struct DisplayGeometry {
    left: f32,
    bottom: f32,
    width: f32,
    height: f32,
    display_width: f32,
    display_height: f32,
    rotation: u16,
}

impl DisplayGeometry {
    fn new(page: &PdfPage<'_>, rotation: u16) -> Result<Self, CommandError> {
        // FPDF_GetPageBoundingBox is the MediaBox/CropBox intersection, which
        // is exactly the region PDFium renders. A page without a CropBox
        // reports its MediaBox here. The box is in unrotated page coordinates;
        // for 90/270 degree pages the display axes are swapped.
        let bounds = page
            .boundaries()
            .get(PdfPageBoundaryBoxType::Bounding)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::RenderFailed,
                    format!("could not read the page box: {error:?}"),
                )
            })?
            .bounds;

        let width = bounds.right().value - bounds.left().value;
        let height = bounds.top().value - bounds.bottom().value;
        let (display_width, display_height) = match rotation {
            90 | 270 => (height, width),
            _ => (width, height),
        };

        Ok(Self {
            left: bounds.left().value,
            bottom: bounds.bottom().value,
            width,
            height,
            display_width,
            display_height,
            rotation,
        })
    }

    /// Maps a point from PDF user space to display-space points with a
    /// top-left origin. The four cases are clockwise page rotations.
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let u = x - self.left;
        let v = y - self.bottom;
        match self.rotation {
            90 => (v, u),
            180 => (self.width - u, v),
            270 => (self.height - v, self.width - u),
            _ => (u, self.height - v),
        }
    }

    /// Maps a PDF user-space box to a display-space box, clamped to the page.
    fn rect(&self, left: f32, bottom: f32, right: f32, top: f32) -> (f32, f32, f32, f32) {
        let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
        let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for (x, y) in [(left, bottom), (left, top), (right, bottom), (right, top)] {
            let (x, y) = self.point(x, y);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }

        let min_x = min_x.clamp(0.0, self.display_width);
        let min_y = min_y.clamp(0.0, self.display_height);
        let max_x = max_x.clamp(0.0, self.display_width);
        let max_y = max_y.clamp(0.0, self.display_height);
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

/// Tolerance for visibility checks; PDFium reports boxes in floating point.
const GEOMETRY_EPSILON: f32 = 0.01;

fn point_inside(x: f32, y: f32, rect: (f32, f32, f32, f32)) -> bool {
    x >= rect.0 - GEOMETRY_EPSILON
        && x <= rect.2 + GEOMETRY_EPSILON
        && y >= rect.1 - GEOMETRY_EPSILON
        && y <= rect.3 + GEOMETRY_EPSILON
}

/// True when two PDF user-space boxes overlap with positive area.
fn boxes_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

fn is_high_surrogate(value: u32) -> bool {
    (0xD800..=0xDBFF).contains(&value)
}

fn is_low_surrogate(value: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&value)
}

/// Builds the positioned text layer for one loaded page.
fn build_text_layer(page: &PdfPage<'_>, page_index: u32) -> Result<TextLayerView, CommandError> {
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(page, rotation)?;
    let page_width = page.width().value;
    let page_height = page.height().value;
    let (crop_x, crop_y) = page
        .boundaries()
        .crop()
        .map(|boundary| (boundary.bounds.left().value, boundary.bounds.bottom().value))
        .unwrap_or((0.0, 0.0));

    // A missing text layer is normal for scanned documents; treat a PDFium
    // failure to page the text the same way so the UI can show the
    // "no editable text" state instead of an error.
    let Some(text_page) = page.text().ok() else {
        return Ok(empty_text_layer(
            page_index,
            page_width,
            page_height,
            rotation,
            crop_x,
            crop_y,
        ));
    };

    let raw_chars = text_page.chars();
    let count = raw_chars.len();
    let mut chars = Vec::with_capacity(count);
    let mut index = 0;

    while index < count {
        let character = raw_chars.get(index).map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read character {index} on page {page_index}: {error:?}"),
            )
        })?;
        let value = character.unicode_value();
        if value == 0 {
            // PDFium uses zero for characters without a Unicode mapping.
            index += 1;
            continue;
        }

        let mut generated = character.is_generated().unwrap_or(false);
        let mut text = char::from_u32(value).map(|c| c.to_string());
        let mut char_box = character.loose_bounds().ok().map(pdf_rect_tuple);
        let origin = character
            .origin()
            .map(|(x, y)| (x.value, y.value))
            .unwrap_or((0.0, 0.0));
        let font_size = character.unscaled_font_size().value;

        // PDFium stores text as UTF-16 code units; combine a surrogate pair
        // into a single selectable character with the union of both boxes.
        if is_high_surrogate(value) {
            if let Ok(next) = raw_chars.get(index + 1) {
                let next_value = next.unicode_value();
                if is_low_surrogate(next_value) {
                    let combined = 0x10000 + ((value - 0xD800) << 10) + (next_value - 0xDC00);
                    text = char::from_u32(combined).map(|c| c.to_string());
                    generated = generated && next.is_generated().unwrap_or(false);
                    char_box = union_boxes(char_box, next.loose_bounds().ok().map(pdf_rect_tuple));
                    index += 1;
                }
            }
        }

        index += 1;
        let Some(text) = text else {
            // A lone surrogate has no Unicode scalar value; keep its slot out
            // of the layer rather than inventing text.
            continue;
        };

        let visible_box = char_box
            .map(|b| {
                boxes_overlap(
                    b,
                    (
                        geometry.left,
                        geometry.bottom,
                        geometry.left + geometry.width,
                        geometry.bottom + geometry.height,
                    ),
                )
            })
            .unwrap_or(false);
        let visible_origin = point_inside(
            origin.0,
            origin.1,
            (
                geometry.left,
                geometry.bottom,
                geometry.left + geometry.width,
                geometry.bottom + geometry.height,
            ),
        );
        if !visible_box && !visible_origin {
            // Text outside the rendered MediaBox/CropBox is not selectable on
            // this page.
            continue;
        }

        let (x, y, width, height) = match char_box {
            Some((left, bottom, right, top)) => geometry.rect(left, bottom, right, top),
            None => {
                let (x, y) = geometry.point(origin.0, origin.1);
                (
                    x.clamp(0.0, page_width),
                    y.clamp(0.0, page_height),
                    0.0,
                    0.0,
                )
            }
        };
        let (origin_x, origin_y) = geometry.point(origin.0, origin.1);

        chars.push(TextCharView {
            text,
            x,
            y,
            width,
            height,
            origin_x,
            origin_y,
            font_size,
            generated,
        });
    }

    let text: String = chars
        .iter()
        .map(|character| character.text.as_str())
        .collect();
    let has_text = !text.trim().is_empty();

    Ok(TextLayerView {
        page_index,
        page_width,
        page_height,
        rotation,
        crop_x,
        crop_y,
        has_text,
        text,
        chars,
    })
}

fn empty_text_layer(
    page_index: u32,
    page_width: f32,
    page_height: f32,
    rotation: u16,
    crop_x: f32,
    crop_y: f32,
) -> TextLayerView {
    TextLayerView {
        page_index,
        page_width,
        page_height,
        rotation,
        crop_x,
        crop_y,
        has_text: false,
        text: String::new(),
        chars: Vec::new(),
    }
}

fn pdf_rect_tuple(rect: pdfium_render::prelude::PdfRect) -> (f32, f32, f32, f32) {
    (
        rect.left().value,
        rect.bottom().value,
        rect.right().value,
        rect.top().value,
    )
}

fn union_boxes(
    a: Option<(f32, f32, f32, f32)>,
    b: Option<(f32, f32, f32, f32)>,
) -> Option<(f32, f32, f32, f32)> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
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
