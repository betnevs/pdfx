use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfPage, PdfPageBoundaryBoxType, PdfRenderConfig,
    PdfSecurityHandlerRevision, PdfiumError, PdfiumInternalError,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::engine::Engine;
use crate::file_association;
use crate::logging;
use crate::settings::{self, Settings, SettingsStore, SettingsUpdateResult};
use crate::updater::{self, UpdateCheckResult, UpdateError};

/// Rendering limits. A full-page bitmap above this size is rejected instead of
/// exhausting memory; the frontend switches to `render_page_region` (viewport
/// tiling) at high zoom.
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 40.0;
pub(crate) const MAX_RENDER_PIXELS: i64 = 64_000_000;

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
    /// The page has no text layer, so there is nothing to edit here.
    NoEditableText,
    /// The clicked Text Object uses a font that cannot represent the
    /// replacement text without emitting missing glyphs.
    UnsupportedFont,
    /// The chosen file is not a PNG or JPEG image.
    UnsupportedImageFormat,
    /// The image file is corrupt or its pixels cannot be decoded.
    ImageDecodeFailed,
    /// The image exceeds the file-size or pixel-count limit.
    ImageTooLarge,
    RenderFailed,
    /// The exported page image could not be written to disk.
    FileWriteFailed,
    /// Page-image export targeted the open Source Document, which is never
    /// modified.
    ExportTargetIsSource,
    /// Page-image export targeted the current Output Document; replacing it
    /// with image bytes would destroy the saved PDF.
    ExportTargetIsOutput,
    /// The platform print operation could not be started.
    PrintFailed,
    /// The flattened copy could not be built or written. Nothing was written
    /// to the destination and the open document is unchanged; the export may
    /// be retried.
    FlattenExportFailed,
    WindowFailed,
    /// Writing the Output Document failed. The Source Document and any
    /// previous Output Document are unchanged; the save may be retried.
    SaveFailed,
    /// Save As targeted the Source Document itself, which is never modified.
    SaveTargetIsSource,
    /// Save Changes has no Output Document yet; the first save must be Save As.
    NoOutputDocument,
    /// The Source Document is encrypted and the caller did not confirm that the
    /// output is written unencrypted (ADR-0007).
    UnencryptedOutputConfirmationRequired,
    /// Permission Restrictions forbid saving changes to this document.
    ReadOnlyDocument,
    /// Writing, reading or removing a Recovery File failed.
    RecoveryFailed,
    /// The requested Recovery File no longer exists.
    RecoveryNotFound,
    /// The requested form widget no longer exists in the open document.
    FormFieldNotFound,
    /// The form widget carries the ReadOnly field flag.
    FormFieldReadOnly,
    /// Permission Restrictions forbid filling existing form fields.
    FormFillNotPermitted,
    /// The value does not match the widget type or one of its options.
    FormValueInvalid,
    /// A new form field needs a non-blank, bounded name.
    FormFieldNameInvalid,
    /// Another field in the document already uses the requested name.
    FormFieldNameDuplicate,
    UpdateNotConfigured,
    UpdateCheckFailed,
    UpdateManifestInvalid,
    UpdateOpenFailed,
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

/// The Source Document's security state, remembered independently of the live
/// PDFium copy.
///
/// A Document restored from a Recovery File keeps the Source's encryption and
/// Permission Restrictions even though the snapshot PDF itself is already
/// decrypted: ADR-0007 still requires an explicit confirmation before writing
/// an unencrypted Output Document, and a restricted Source stays restricted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentSecurity {
    pub(crate) encrypted: bool,
    pub(crate) read_only: bool,
    pub(crate) permissions: DocumentPermissions,
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
    /// Output Document this Document will save to, when it has one. Restoring
    /// a Recovery File reuses the previous Output Document only while it still
    /// exists and is not the Source Document.
    #[serde(default)]
    pub output_path: Option<String>,
    /// True when the Document was opened from a Recovery File. `path` is then
    /// the original Source Document (provenance) and is never written.
    #[serde(default)]
    pub recovered: bool,
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

pub(crate) struct OpenDocument {
    /// Path the Source Document was opened from. Export and print refuse to
    /// overwrite it, and Save As refuses to target it.
    pub(crate) source_path: PathBuf,
    /// The unmodified bytes read from disk, kept for the incremental-save
    /// pipeline. PDFium owns its own copy of the input buffer.
    pub(crate) source_bytes: Vec<u8>,
    /// Save target state: the Output Document path and object-level edits.
    pub(crate) save: crate::save::SaveState,
    /// Session-level edit state for this Document. Declared before `document`
    /// so detached page-object snapshots held by undo/redo are destroyed
    /// before PDFium closes the owning document.
    pub(crate) edit: crate::text_edit::EditSession,
    /// Snapshot bookkeeping for this Document, created on the first edit.
    pub(crate) recovery: Option<crate::recovery::RecoveryState>,
    /// The Source Document's security, preserved by a recovered Document.
    pub(crate) source_security: DocumentSecurity,
    pub(crate) document: PdfDocument<'static>,
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

/// Runs `action` against the open Document with the given id while holding the
/// registry lock. Text editing borrows both the PDFium document and the
/// session history, so it needs mutable access to the whole entry.
pub(crate) fn with_open_document<R>(
    document_id: &str,
    action: impl FnOnce(&mut OpenDocument) -> Result<R, CommandError>,
) -> Result<R, CommandError> {
    let mut registry = registry().lock().expect("document registry poisoned");
    let entry = registry.documents.get_mut(document_id).ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::DocumentNotFound,
            format!("no open document with id {document_id}"),
        )
    })?;
    action(entry)
}

/// The `open_document` command.
///
/// Every open, successful or not, is recorded in the local rolling log; no
/// document contents leave the device.
#[tauri::command]
pub fn open_document(path: String, password: Option<String>) -> Result<DocumentView, CommandError> {
    let result = open_document_impl(path, password);
    match &result {
        Ok(view) => {
            logging::event(
                "document.open",
                &format!(
                    "name={} pages={} path={}",
                    view.name, view.page_count, view.path
                ),
            );
            crate::recovery::record_recent_document(std::path::Path::new(&view.path));
        }
        Err(error) => logging::error(
            "document.open.failed",
            &format!("code={:?} message={}", error.code, error.message),
        ),
    }
    result
}

fn open_document_impl(
    path: String,
    password: Option<String>,
) -> Result<DocumentView, CommandError> {
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

    let display_name = document_name(&path);
    insert_document(
        path,
        bytes,
        password,
        NewDocument {
            display_name,
            output_path: None,
            recovered: false,
            encrypted: None,
            read_only: None,
            permissions: None,
            recovery: None,
            edit: crate::text_edit::EditSession::default(),
        },
    )
}

/// Everything that differs between opening a Source Document and opening a
/// Recovery File. `None` security fields mean "derive from the live PDFium
/// document", which is what a normal open does.
struct NewDocument {
    display_name: String,
    output_path: Option<String>,
    recovered: bool,
    encrypted: Option<bool>,
    read_only: Option<bool>,
    permissions: Option<DocumentPermissions>,
    recovery: Option<crate::recovery::RecoveryState>,
    edit: crate::text_edit::EditSession,
}

/// Opens bytes into a new registry entry and returns its frontend view.
fn insert_document(
    path: PathBuf,
    bytes: Vec<u8>,
    password: Option<String>,
    options: NewDocument,
) -> Result<DocumentView, CommandError> {
    let engine = Engine::shared()
        .map_err(|message| CommandError::new(CommandErrorCode::PdfiumUnavailable, message))?;

    // PDFium's API is not thread-safe, and tests (and multiple windows) open
    // documents concurrently. The registry lock already serializes every
    // command that uses an open document's PDFium handles; hold it here too so
    // reading page dimensions and permissions cannot overlap those commands.
    let mut registry = registry().lock().expect("document registry poisoned");

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

    let (actual_encrypted, actual_read_only, actual_permissions) = document_permissions(&document);
    let encrypted = options.encrypted.unwrap_or(actual_encrypted);
    let read_only = options.read_only.unwrap_or(actual_read_only);
    let permissions = options.permissions.unwrap_or(actual_permissions);

    registry.next_id += 1;
    let document_id = format!("doc-{}", registry.next_id);
    let view = DocumentView {
        document_id: document_id.clone(),
        name: options.display_name,
        path: path.to_string_lossy().into_owned(),
        page_count: page_count as u32,
        page_width,
        page_height,
        encrypted,
        read_only,
        permissions,
        output_path: options.output_path.clone(),
        recovered: options.recovered,
    };
    let save = crate::save::SaveState {
        output_path: options.output_path.map(PathBuf::from),
        ..Default::default()
    };
    registry.documents.insert(
        document_id,
        OpenDocument {
            source_path: path,
            source_bytes: bytes,
            save,
            edit: options.edit,
            recovery: options.recovery,
            source_security: DocumentSecurity {
                encrypted,
                read_only,
                permissions,
            },
            document,
        },
    );

    Ok(view)
}

/// Reads the effective permissions the PDF security handler grants.
///
/// Returns `(encrypted, read_only, permissions)`. PDFium reports every
/// capability for unencrypted documents and for documents opened with the
/// Owner Password; Permission Restrictions only narrow the flags when the
/// document was opened with the User Password.
pub(crate) fn document_permissions(
    document: &PdfDocument<'_>,
) -> (bool, bool, DocumentPermissions) {
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
///
/// A normal close discards the Document's Recovery File: the close guard only
/// closes after saving or explicitly discarding the edits. An abnormal exit
/// never reaches this command, which is exactly when the Recovery File must
/// survive for the next start-up.
#[tauri::command]
pub fn close_document(document_id: String) -> Result<(), CommandError> {
    let mut registry = registry().lock().expect("document registry poisoned");
    if let Some(mut entry) = registry.documents.remove(&document_id) {
        crate::recovery::clear_for_entry(&mut entry);
    }
    logging::event("document.close", &format!("documentId={document_id}"));
    Ok(())
}

/// The `recovery_candidates` command: valid Recovery Files, newest first.
///
/// The scan deliberately removes orphan or unreadable pairs so a broken file
/// cannot prompt forever. It never opens anything.
#[tauri::command]
pub fn recovery_candidates() -> Result<Vec<crate::recovery::RecoverySnapshot>, CommandError> {
    let Some(store) = crate::recovery::recovery_store() else {
        return Ok(Vec::new());
    };
    store
        .scan()
        .map(|scan| scan.candidates)
        .map_err(crate::recovery::map_recovery_error)
}

/// The `restore_recovery` command: opens a Recovery File as the Document.
///
/// The snapshot bytes become the live document (including every unsaved edit);
/// the original Source path is provenance only and the previous Output path is
/// reused only while it still exists and is not the Source Document. The
/// Recovery File stays on disk until the restored Document is saved or closed,
/// so a second crash is still recoverable.
#[tauri::command]
pub fn restore_recovery(session_id: String) -> Result<DocumentView, CommandError> {
    let Some(store) = crate::recovery::recovery_store() else {
        return Err(CommandError::new(
            CommandErrorCode::RecoveryNotFound,
            "recovery storage is unavailable",
        ));
    };
    let restored = store
        .read_snapshot(&session_id)
        .map_err(crate::recovery::map_recovery_error)?
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::RecoveryNotFound,
                format!("no recovery file for session {session_id}"),
            )
        })?;
    let snapshot = restored.snapshot;

    let output_path = snapshot
        .output_path
        .as_deref()
        .filter(|target| {
            target.is_file() && !crate::target_path::same_file(target, &snapshot.source_path)
        })
        .map(|target| target.to_string_lossy().into_owned());

    // Start the restored history above the crashed revision and mark it dirty:
    // the snapshot contains edits the last save never wrote. The undo/redo
    // stacks are empty (they cannot cross a process), so only Save can make it
    // clean again.
    let revision = snapshot.revision.saturating_add(1);
    let edit = crate::text_edit::EditSession {
        current_state: revision,
        next_state: revision,
        saved_state: snapshot.revision,
        ..Default::default()
    };
    let display_name = if snapshot.display_name.is_empty() {
        document_name(&snapshot.source_path)
    } else {
        snapshot.display_name.clone()
    };

    let view = insert_document(
        snapshot.source_path.clone(),
        restored.bytes,
        None,
        NewDocument {
            display_name,
            output_path,
            recovered: true,
            encrypted: Some(snapshot.source_encrypted),
            read_only: Some(snapshot.source_read_only),
            permissions: snapshot.source_permissions,
            recovery: Some(crate::recovery::RecoveryState::restored(
                session_id.clone(),
                store,
            )),
            edit,
        },
    )?;

    crate::recovery::record_recent_document(&snapshot.source_path);
    logging::event(
        "recovery.restore",
        &format!(
            "sessionId={session_id} documentId={} pages={}",
            view.document_id, view.page_count
        ),
    );
    Ok(view)
}

/// The `discard_recovery` command: forget one Recovery File.
#[tauri::command]
pub fn discard_recovery(session_id: String) -> Result<(), CommandError> {
    let Some(store) = crate::recovery::recovery_store() else {
        return Ok(());
    };
    store
        .remove_snapshot(&session_id)
        .map_err(crate::recovery::map_recovery_error)?;
    logging::event("recovery.discard", &format!("sessionId={session_id}"));
    Ok(())
}

/// The `list_recent_files` command. Reading the list never opens a Document.
#[tauri::command]
pub fn list_recent_files() -> Result<Vec<crate::recovery::RecentFile>, CommandError> {
    Ok(crate::recovery::recent_files_store()
        .map(|store| store.list())
        .unwrap_or_default())
}

/// The `forget_recent_file` command: removes one missing or unwanted entry.
#[tauri::command]
pub fn forget_recent_file(path: String) -> Result<Vec<crate::recovery::RecentFile>, CommandError> {
    match crate::recovery::recent_files_store() {
        Some(store) => store
            .forget(&path)
            .map_err(crate::recovery::map_recovery_error),
        None => Ok(Vec::new()),
    }
}

/// Serializes the current in-memory document, including every applied edit.
///
/// This is the save-pipeline seam ticket 10 builds on. It returns the bytes an
/// output file must contain without closing or reloading the Document, so the
/// session's undo/redo history keeps working afterwards. Callers that write
/// the bytes should then call [`crate::text_edit::mark_document_saved`] to move
/// the dirty baseline without clearing that history.
pub fn save_document_bytes(document_id: &str) -> Result<Vec<u8>, CommandError> {
    with_open_document(document_id, |entry| {
        entry.document.save_to_bytes().map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not serialize the document: {error:?}"),
            )
        })
    })
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

    let mut bitmap = PdfBitmap::empty(width as i32, height as i32, PdfBitmapFormat::default())
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

/// Loads a page with the document's own lifetime.
///
/// Returning `PdfPage<'static>` rather than a page tied to the borrow of
/// `document` lets callers keep a page across other borrows, and lets the edit
/// session store detached page-object snapshots for undo/redo.
pub(crate) fn load_page(
    document: &PdfDocument<'static>,
    page_index: u32,
) -> Result<PdfPage<'static>, CommandError> {
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
pub(crate) fn checked_page_pixels(
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

pub(crate) fn page_pixel_size(page: &PdfPage<'_>, scale: f32) -> (u32, u32) {
    let width = (page.width().value * scale).round().max(1.0);
    let height = (page.height().value * scale).round().max(1.0);
    (width as u32, height as u32)
}

/// Renders a full page into straight RGBA bytes, the bitmap the page viewer
/// and every export share. Keeping one rendering path guarantees an exported
/// page and the on-screen canvas agree pixel for pixel.
pub(crate) fn render_page_rgba(
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

    Ok(rgba)
}

fn render_full_page_png(
    page: &PdfPage<'_>,
    page_index: u32,
    pixel_width: u32,
    pixel_height: u32,
) -> Result<Vec<u8>, CommandError> {
    let rgba = render_page_rgba(page, page_index, pixel_width, pixel_height)?;
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
pub(crate) struct DisplayGeometry {
    left: f32,
    bottom: f32,
    width: f32,
    height: f32,
    display_width: f32,
    display_height: f32,
    rotation: u16,
}

impl DisplayGeometry {
    pub(crate) fn rotation(&self) -> u16 {
        self.rotation
    }

    pub(crate) fn new(page: &PdfPage<'_>, rotation: u16) -> Result<Self, CommandError> {
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

    /// The page size in display-space points, after CropBox and intrinsic
    /// rotation. Overlay content is laid out in this coordinate system.
    pub(crate) fn display_size(&self) -> (f32, f32) {
        (self.display_width, self.display_height)
    }

    /// Maps a point from PDF user space to display-space points with a
    /// top-left origin. The four cases are clockwise page rotations.
    pub(crate) fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let u = x - self.left;
        let v = y - self.bottom;
        match self.rotation {
            90 => (v, u),
            180 => (self.width - u, v),
            270 => (self.height - v, self.width - u),
            _ => (u, self.height - v),
        }
    }

    /// Maps a display-space point (top-left origin) back to PDF user space.
    ///
    /// Overlay tools that create content (Text Boxes, shapes) need the inverse
    /// of [`Self::point`] to place page objects in the document's own
    /// coordinate system, including crop offset and intrinsic rotation.
    pub(crate) fn point_inverse(&self, x: f32, y: f32) -> (f32, f32) {
        match self.rotation {
            90 => (self.left + y, self.bottom + x),
            180 => (self.left + self.width - x, self.bottom + y),
            270 => (self.left + self.width - y, self.bottom + self.height - x),
            _ => (self.left + x, self.bottom + self.height - y),
        }
    }

    /// Maps a PDF user-space box to a display-space box, clamped to the page.
    pub(crate) fn rect(
        &self,
        left: f32,
        bottom: f32,
        right: f32,
        top: f32,
    ) -> (f32, f32, f32, f32) {
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
    build_text_layer_with_raw_indices(page, page_index).map(|(view, _)| view)
}

/// Builds the text layer and, alongside it, the PDFium raw character index for
/// each returned layer character.
///
/// The two index spaces differ: PDFium drops characters without a Unicode
/// mapping and stores surrogate pairs as two code units, so callers that need
/// to address a PDFium character (text editing does) cannot use the layer
/// index directly.
pub(crate) fn build_text_layer_with_raw_indices(
    page: &PdfPage<'_>,
    page_index: u32,
) -> Result<(TextLayerView, Vec<usize>), CommandError> {
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
        return Ok((
            empty_text_layer(
                page_index,
                page_width,
                page_height,
                rotation,
                crop_x,
                crop_y,
            ),
            Vec::new(),
        ));
    };

    let raw_chars = text_page.chars();
    let count = raw_chars.len();
    let mut chars = Vec::with_capacity(count);
    let mut raw_indices = Vec::with_capacity(count);
    let mut index = 0;

    while index < count {
        let raw_index = index;
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
        raw_indices.push(raw_index);
    }

    let text: String = chars
        .iter()
        .map(|character| character.text.as_str())
        .collect();
    let has_text = !text.trim().is_empty();

    Ok((
        TextLayerView {
            page_index,
            page_width,
            page_height,
            rotation,
            crop_x,
            crop_y,
            has_text,
            text,
            chars,
        },
        raw_indices,
    ))
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

pub(crate) fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CommandError> {
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

/// The result of a completed diagnostic export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExportView {
    pub path: String,
    pub bytes: u64,
    pub log_files: u32,
}

pub(crate) fn settings_store(app: &AppHandle) -> Result<SettingsStore, CommandError> {
    let directory = app.path().app_config_dir().map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not resolve the app config directory: {error}"),
        )
    })?;
    Ok(SettingsStore::new(directory))
}

fn map_settings_error(error: settings::SettingsError) -> CommandError {
    match error {
        settings::SettingsError::InvalidArgument(message) => {
            CommandError::new(CommandErrorCode::InvalidArgument, message)
        }
        other => CommandError::new(CommandErrorCode::Internal, other.to_string()),
    }
}

/// The `get_settings` command. Missing or corrupt settings load as defaults.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, CommandError> {
    Ok(settings_store(&app)?.load())
}

/// The `update_settings` command. Persists the settings, then applies a change
/// to the file-association preference through the platform boundary.
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    settings: Settings,
) -> Result<SettingsUpdateResult, CommandError> {
    let store = settings_store(&app)?;
    let platform = file_association::platform_for(&app);
    let result =
        settings::apply_update(&store, platform.as_ref(), settings).map_err(map_settings_error)?;

    // The annotation author is deliberately not logged: it is user-entered.
    logging::event(
        "settings.update",
        &format!(
            "language={:?} theme={:?} fileAssociationEnabled={} association={:?}",
            result.settings.language,
            result.settings.theme,
            result.settings.file_association_enabled,
            result.association.as_ref().map(|outcome| outcome.status)
        ),
    );

    Ok(result)
}

/// The `export_diagnostics` command. Writes a copy of the local logs plus
/// version and platform information to a path the user chose.
#[tauri::command]
pub fn export_diagnostics(destination: String) -> Result<DiagnosticsExportView, CommandError> {
    let export = logging::export_diagnostics(
        std::path::Path::new(&destination),
        "PDFX",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not export diagnostics: {error}"),
        )
    })?;

    Ok(DiagnosticsExportView {
        path: export.path.to_string_lossy().into_owned(),
        bytes: export.bytes,
        log_files: export.log_files,
    })
}

fn map_update_error(error: UpdateError) -> CommandError {
    match error {
        UpdateError::NotConfigured => {
            CommandError::new(CommandErrorCode::UpdateNotConfigured, "未配置更新地址。")
        }
        // The settings page validates this before it can be persisted; treat a
        // bad address here as a bad argument rather than a network failure.
        UpdateError::InvalidManifestUrl(message) => {
            CommandError::new(CommandErrorCode::InvalidArgument, message)
        }
        UpdateError::InvalidDownloadUrl(message) | UpdateError::ManifestInvalid(message) => {
            CommandError::new(CommandErrorCode::UpdateManifestInvalid, message)
        }
        UpdateError::Http(message) => {
            CommandError::new(CommandErrorCode::UpdateCheckFailed, message)
        }
        UpdateError::OpenFailed(message) => {
            CommandError::new(CommandErrorCode::UpdateOpenFailed, message)
        }
    }
}

/// The `check_for_updates` command.
///
/// ADR-0009: this is the only network path in PDFX, and it runs only when the
/// user clicks "检查更新". It reads the persisted manifest address, performs
/// one GET with a finite timeout and no retries, and never sends document
/// data, cookies or usage data. An unset address returns a dedicated error
/// without touching the network.
#[tauri::command]
pub fn check_for_updates(app: AppHandle) -> Result<UpdateCheckResult, CommandError> {
    let settings = settings_store(&app)?.load();
    match updater::check_for_updates(&settings.update_manifest_url, env!("CARGO_PKG_VERSION")) {
        Ok(result) => {
            // The user-configured address is not logged: only the outcome.
            logging::event(
                "update.check",
                &format!(
                    "status={:?} version={} currentVersion={}",
                    result.status, result.version, result.current_version
                ),
            );
            Ok(result)
        }
        Err(error) => {
            logging::event("update.check.failed", &format!("error={error}"));
            Err(map_update_error(error))
        }
    }
}

/// The `open_download_page` command. Opens the manifest's download page in the
/// system browser; never in an in-app webview. The URL is re-validated here so
/// the launcher only receives an absolute http(s) address.
#[tauri::command]
pub fn open_download_page(url: String) -> Result<(), CommandError> {
    updater::open_download_page(&updater::SystemOpener, &url)
        .map_err(map_update_error)
        .inspect_err(|error| {
            logging::event("update.open.failed", &format!("code={:?}", error.code));
        })
}
