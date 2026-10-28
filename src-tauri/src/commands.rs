use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfDocument, PdfSecurityHandlerRevision, PdfiumError, PdfiumInternalError,
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

/// Editing capabilities granted by the PDF security handler.
///
/// Unencrypted documents grant every capability. Encrypted documents report
/// the capabilities granted by the Permission Restrictions unless they were
/// opened with the Owner Password, which grants all of them. Individual edit
/// tools consult these flags so a restriction on annotations, for example,
/// does not wrongly disable text editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPermissions {
    pub can_modify_content: bool,
    pub can_annotate: bool,
    pub can_assemble: bool,
    pub can_fill_forms: bool,
    pub can_print: bool,
    pub can_copy_content: bool,
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
    /// True when the Source Document is encrypted (User or Owner Password).
    /// Save As must warn that the output is written unencrypted (ADR-0007).
    pub encrypted: bool,
    /// True when the Permission Restrictions deny every editing capability
    /// (content modification, annotation, form filling and page assembly).
    /// Save Changes is disabled for such documents; individual tools use
    /// `permissions` for finer gating.
    pub read_only: bool,
    pub permissions: DocumentPermissions,
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

    let (encrypted, read_only, permissions) = document_permissions(&document);

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
            encrypted,
            read_only,
            permissions,
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

/// Reads the effective permissions the PDF security handler grants.
///
/// Returns `(encrypted, read_only, permissions)`. PDFium reports every
/// capability for unencrypted documents and for documents opened with the
/// Owner Password; Permission Restrictions only narrow the flags when the
/// document was opened with the User Password.
fn document_permissions(document: &PdfDocument<'_>) -> (bool, bool, DocumentPermissions) {
    let permissions = document.permissions();

    // `Unprotected` is the only unencrypted revision. PDFium reports an error
    // for revisions this binding does not name (for example AES-256 revision 6),
    // and those documents are encrypted, so fail towards warning about it.
    let encrypted = !matches!(
        permissions.security_handler_revision(),
        Ok(PdfSecurityHandlerRevision::Unprotected)
    );

    // A missing permission read is treated as denied: the editor must never
    // allow an edit the security handler may forbid.
    let granted = |result: Result<bool, PdfiumError>| result.unwrap_or(false);
    let flags = DocumentPermissions {
        can_modify_content: granted(permissions.can_modify_document_content()),
        can_annotate: granted(permissions.can_add_or_modify_text_annotations()),
        can_assemble: granted(permissions.can_assemble_document()),
        can_fill_forms: granted(permissions.can_fill_existing_interactive_form_fields()),
        can_print: granted(permissions.can_print_high_quality())
            || granted(permissions.can_print_only_low_quality()),
        can_copy_content: granted(permissions.can_extract_text_and_graphics()),
    };

    // Read-only means no editing surface at all; a document that only forbids
    // annotations stays editable and gates that tool through `permissions`.
    let read_only = !(flags.can_modify_content
        || flags.can_annotate
        || flags.can_assemble
        || flags.can_fill_forms);

    (encrypted, read_only, flags)
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
