//! Embedded-file attachments.
//!
//! Existing attachments are read through PDFium, which also decrypts them for
//! an encrypted Source Document. New attachments are kept in the session until
//! the save pipeline writes them into the PDF's `/Names /EmbeddedFiles` name
//! tree in a single `lopdf` revision; the live PDFium attachment collection is
//! never modified, so undo/redo and repeated saves stay deterministic.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::commands::{with_open_document, CommandError, CommandErrorCode, OpenDocument};
use crate::text_edit::{EditAction, EditHistoryView, EditStepView};

/// Upper bound for one attachment read from disk. The whole file is embedded
/// in the PDF, so this also bounds the in-memory copy held by the session.
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

/// Longest attachment name kept in the name tree. File systems allow longer
/// names, but a bounded name keeps the PDF dictionary and extraction path
/// manageable.
const MAX_ATTACHMENT_NAME_CHARS: usize = 120;

/// One attachment as shown in the attachment list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentView {
    /// Stable for the lifetime of the open document.
    pub id: u64,
    pub name: String,
    pub size: u64,
    /// MIME type derived from the file name; `application/octet-stream` when
    /// the extension is unknown.
    pub media_type: String,
}

/// The outcome of adding or deleting an attachment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentEditResultView {
    pub attachments: Vec<AttachmentView>,
    pub history: EditHistoryView,
}

/// The outcome of saving one attachment to a chosen file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSaveView {
    pub path: String,
    pub bytes_written: u64,
}

/// Where an attachment's bytes come from.
#[derive(Debug, Clone)]
pub(crate) enum AttachmentSource {
    /// An attachment already embedded in the Source Document, addressed by its
    /// PDFium collection index. PDFium never mutates that collection here, so
    /// the index stays valid for the whole session.
    Existing { index: u16 },
    /// A file added in this session.
    Created { bytes: Arc<Vec<u8>> },
}

/// Session record for one attachment. Clones share created bytes.
#[derive(Debug, Clone)]
pub(crate) struct AttachmentRecord {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) media_type: String,
    pub(crate) source: AttachmentSource,
}

impl AttachmentRecord {
    fn view(&self) -> AttachmentView {
        AttachmentView {
            id: self.id,
            name: self.name.clone(),
            size: self.size,
            media_type: self.media_type.clone(),
        }
    }
}

/// One committed insertion or deletion, holding both attachment lists.
///
/// Attachment lists are small and created bytes are shared through `Arc`, so a
/// full before/after snapshot restores the exact list, ids and order on undo
/// and redo.
pub(crate) struct AttachmentEditAction {
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: Vec<AttachmentRecord>,
    pub(crate) after: Vec<AttachmentRecord>,
}

/// Attachments visible in this session, plus the baseline read when the
/// document was opened.
///
/// The save pipeline diffs `baseline` against `visible`: a baseline name no
/// longer visible is deleted from the name tree, and every visible created
/// record is upserted by name. Deriving the edits from the lists (instead of
/// accumulating commands) makes repeated saves idempotent.
#[derive(Debug, Default)]
pub struct AttachmentStore {
    baseline: Vec<AttachmentRecord>,
    visible: Vec<AttachmentRecord>,
    next_id: u64,
}

impl AttachmentStore {
    /// Reads the Source Document's attachments through PDFium.
    pub(crate) fn from_document(document: &pdfium_render::prelude::PdfDocument<'_>) -> Self {
        let attachments = document.attachments();
        let mut visible = Vec::new();
        for index in 0..attachments.len() {
            let Ok(attachment) = attachments.get(index) else {
                continue;
            };
            let name = attachment.name();
            visible.push(AttachmentRecord {
                id: visible.len() as u64 + 1,
                media_type: media_type_for_name(&name).to_owned(),
                size: attachment.len() as u64,
                name,
                source: AttachmentSource::Existing { index },
            });
        }
        let next_id = visible.len() as u64;
        Self {
            baseline: visible.clone(),
            visible,
            next_id,
        }
    }

    pub(crate) fn views(&self) -> Vec<AttachmentView> {
        self.visible.iter().map(AttachmentRecord::view).collect()
    }

    /// Adds a created record with a unique name, returning it.
    fn push_created(&mut self, name: String, bytes: Arc<Vec<u8>>) -> &AttachmentRecord {
        self.next_id += 1;
        let size = bytes.len() as u64;
        self.visible.push(AttachmentRecord {
            id: self.next_id,
            media_type: media_type_for_name(&name).to_owned(),
            name,
            size,
            source: AttachmentSource::Created { bytes },
        });
        self.visible.last().expect("a record was just pushed")
    }

    /// Returns `desired`, suffixed with ` (2)`, ` (3)`, ... before the
    /// extension until it differs from every visible attachment name.
    fn unique_name(&self, desired: &str) -> String {
        let taken: HashSet<&str> = self
            .visible
            .iter()
            .map(|record| record.name.as_str())
            .collect();
        if !taken.contains(desired) {
            return desired.to_owned();
        }
        let (stem, extension) = split_extension(desired);
        for counter in 2..10_000 {
            let candidate = match extension {
                Some(extension) => format!("{stem} ({counter}).{extension}"),
                None => format!("{stem} ({counter})"),
            };
            if !taken.contains(candidate.as_str()) {
                return candidate;
            }
        }
        format!("{stem}-{}.{}", self.next_id + 1, extension.unwrap_or("bin"))
    }

    /// The attachment list to apply on the next save, or `None` when the
    /// visible list is exactly the baseline.
    pub(crate) fn save_edit(&self) -> Option<EmbeddedFilesEdit> {
        let visible_names: HashSet<&str> = self
            .visible
            .iter()
            .map(|record| record.name.as_str())
            .collect();
        let mut remove = Vec::new();
        for record in &self.baseline {
            if !visible_names.contains(record.name.as_str()) && !remove.contains(&record.name) {
                remove.push(record.name.clone());
            }
        }
        let create: Vec<CreatedAttachmentSpec> = self
            .visible
            .iter()
            .filter_map(|record| match &record.source {
                AttachmentSource::Existing { .. } => None,
                AttachmentSource::Created { bytes } => Some(CreatedAttachmentSpec {
                    name: record.name.clone(),
                    media_type: record.media_type.clone(),
                    bytes: Arc::clone(bytes),
                }),
            })
            .collect();
        if remove.is_empty() && create.is_empty() {
            None
        } else {
            Some(EmbeddedFilesEdit { remove, create })
        }
    }
}

fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem, Some(extension))
        }
        _ => (name, None),
    }
}

/// The `/Names /EmbeddedFiles` rewrite derived from the session lists.
pub(crate) struct EmbeddedFilesEdit {
    /// Names of Source attachments the user deleted.
    pub(crate) remove: Vec<String>,
    /// New embedded files, upserted by name.
    pub(crate) create: Vec<CreatedAttachmentSpec>,
}

pub(crate) struct CreatedAttachmentSpec {
    pub(crate) name: String,
    pub(crate) media_type: String,
    pub(crate) bytes: Arc<Vec<u8>>,
}

/// The `list_attachments` command: the attachment list for a Document.
#[tauri::command]
pub fn list_attachments(document_id: String) -> Result<Vec<AttachmentView>, CommandError> {
    with_open_document(&document_id, |entry| Ok(entry.attachments.views()))
}

/// The `add_attachment` command: reads the chosen file and adds it to the
/// session. The file is embedded in the PDF only when the document is saved.
#[tauri::command]
pub fn add_attachment(
    document_id: String,
    path: String,
) -> Result<AttachmentEditResultView, CommandError> {
    let path = PathBuf::from(path);
    let raw_name = attachment_file_name(&path);
    let bytes = read_attachment_bytes(&path)?;
    with_open_document(&document_id, |entry| {
        ensure_attachment_editable(entry)?;
        let before = entry.attachments.visible.clone();
        let name = entry
            .attachments
            .unique_name(&sanitize_attachment_name(&raw_name));
        entry.attachments.push_created(name, Arc::new(bytes));
        let after = entry.attachments.visible.clone();
        let history = push_attachment_action(entry, before, after);
        Ok(AttachmentEditResultView {
            attachments: entry.attachments.views(),
            history,
        })
    })
}

/// The `delete_attachment` command. The confirmation prompt belongs to the UI;
/// the backend records the deletion as one undoable action.
#[tauri::command]
pub fn delete_attachment(
    document_id: String,
    attachment_id: u64,
) -> Result<AttachmentEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_attachment_editable(entry)?;
        let index = entry
            .attachments
            .visible
            .iter()
            .position(|record| record.id == attachment_id)
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::AttachmentNotFound,
                    format!("attachment {attachment_id} is not in this document"),
                )
            })?;
        let before = entry.attachments.visible.clone();
        entry.attachments.visible.remove(index);
        let after = entry.attachments.visible.clone();
        let history = push_attachment_action(entry, before, after);
        Ok(AttachmentEditResultView {
            attachments: entry.attachments.views(),
            history,
        })
    })
}

pub(crate) fn ensure_attachment_editable(entry: &OpenDocument) -> Result<(), CommandError> {
    crate::text_edit::ensure_content_editable(&entry.document)
}

/// Appends an attachment change to the unified undo stack.
pub(crate) fn push_attachment_action(
    entry: &mut OpenDocument,
    before: Vec<AttachmentRecord>,
    after: Vec<AttachmentRecord>,
) -> EditHistoryView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::Attachment(AttachmentEditAction {
            before_state,
            after_state,
            before,
            after,
        }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.edit.history_view()
}

fn attachment_step(entry: &OpenDocument) -> EditStepView {
    EditStepView {
        page_index: 0,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    }
}

/// The `save_attachment_as` command: extracts one attachment to a user-chosen
/// file with an atomic write. The Source Document and the current Output
/// Document are never overwritten.
#[tauri::command]
pub fn save_attachment_as(
    document_id: String,
    attachment_id: u64,
    destination: String,
) -> Result<AttachmentSaveView, CommandError> {
    let destination = PathBuf::from(destination);
    with_open_document(&document_id, |entry| {
        let record = entry
            .attachments
            .visible
            .iter()
            .find(|record| record.id == attachment_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::AttachmentNotFound,
                    format!("attachment {attachment_id} is not in this document"),
                )
            })?;
        if crate::target_path::same_file(&entry.source_path, &destination) {
            return Err(CommandError::new(
                CommandErrorCode::AttachmentTargetIsSource,
                format!(
                    "{} is the Source Document; choose a different attachment path",
                    destination.display()
                ),
            ));
        }
        if let Some(output_path) = &entry.save.output_path {
            if crate::target_path::same_file(output_path, &destination) {
                return Err(CommandError::new(
                    CommandErrorCode::AttachmentTargetIsOutput,
                    format!(
                        "{} is the Output Document; choose a different attachment path",
                        destination.display()
                    ),
                ));
            }
        }

        let bytes = attachment_bytes(entry, &record)?;
        crate::atomic_write::write_atomically(&destination, &bytes).map_err(|error| {
            CommandError::new(CommandErrorCode::AttachmentSaveFailed, error.message)
        })?;
        Ok(AttachmentSaveView {
            path: destination.to_string_lossy().into_owned(),
            bytes_written: bytes.len() as u64,
        })
    })
}

/// The `open_attachment` command. Attachments are never opened automatically:
/// the caller must pass `confirm: true` after showing the unsafe-attachment
/// warning. The bytes are extracted to the app cache first; the OS default
/// handler is launched only after that extraction succeeds.
#[tauri::command]
pub fn open_attachment(
    app: tauri::AppHandle,
    document_id: String,
    attachment_id: u64,
    confirm: bool,
) -> Result<(), CommandError> {
    use tauri::Manager as _;

    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::AttachmentOpenFailed,
                format!("could not resolve the attachment staging directory: {error}"),
            )
        })?
        .join("attachments");
    open_attachment_with(&document_id, attachment_id, confirm, &directory, |path| {
        open::that_detached(path).map_err(|error| error.to_string())
    })?;
    Ok(())
}

/// The testable open seam: writes the attachment under `staging_root` and hands
/// the staged path to `launch`.
///
/// The staged file name is the sanitized attachment name, so a hostile name
/// cannot escape `staging_root`.
pub fn open_attachment_with(
    document_id: &str,
    attachment_id: u64,
    confirm: bool,
    staging_root: &Path,
    launch: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, CommandError> {
    with_open_document(document_id, |entry| {
        if !confirm {
            return Err(CommandError::new(
                CommandErrorCode::AttachmentOpenConfirmationRequired,
                "opening an attachment requires explicit confirmation",
            ));
        }
        let record = entry
            .attachments
            .visible
            .iter()
            .find(|record| record.id == attachment_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::AttachmentNotFound,
                    format!("attachment {attachment_id} is not in this document"),
                )
            })?;
        let bytes = attachment_bytes(entry, &record)?;

        std::fs::create_dir_all(staging_root).map_err(|error| {
            CommandError::new(
                CommandErrorCode::AttachmentOpenFailed,
                format!("could not create the attachment staging directory: {error}"),
            )
        })?;
        let staged = staging_root.join(sanitize_attachment_name(&record.name));
        if staged.parent() != Some(staging_root) {
            return Err(CommandError::new(
                CommandErrorCode::AttachmentOpenFailed,
                "the attachment name does not resolve inside the staging directory",
            ));
        }
        crate::atomic_write::write_atomically(&staged, &bytes).map_err(|error| {
            CommandError::new(CommandErrorCode::AttachmentOpenFailed, error.message)
        })?;
        launch(&staged).map_err(|error| {
            CommandError::new(
                CommandErrorCode::AttachmentOpenFailed,
                format!(
                    "could not open the attachment with the system default application: {error}"
                ),
            )
        })?;
        Ok(staged)
    })
}

fn attachment_bytes(
    entry: &OpenDocument,
    record: &AttachmentRecord,
) -> Result<Vec<u8>, CommandError> {
    match &record.source {
        AttachmentSource::Existing { index } => entry
            .document
            .attachments()
            .get(*index)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not read attachment {}: {error:?}", record.name),
                )
            })?
            .save_to_bytes()
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not extract attachment {}: {error:?}", record.name),
                )
            }),
        AttachmentSource::Created { bytes } => Ok(bytes.as_ref().clone()),
    }
}

pub(crate) fn undo_attachment(
    entry: &mut OpenDocument,
    action: AttachmentEditAction,
) -> Result<EditStepView, CommandError> {
    entry.attachments.visible = action.before.clone();
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Attachment(action));
    Ok(attachment_step(entry))
}

pub(crate) fn redo_attachment(
    entry: &mut OpenDocument,
    action: AttachmentEditAction,
) -> Result<EditStepView, CommandError> {
    entry.attachments.visible = action.after.clone();
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Attachment(action));
    Ok(attachment_step(entry))
}

fn attachment_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_owned())
}

fn read_attachment_bytes(path: &Path) -> Result<Vec<u8>, CommandError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => CommandErrorCode::FileNotFound,
            std::io::ErrorKind::PermissionDenied => CommandErrorCode::PermissionDenied,
            _ => CommandErrorCode::FileReadFailed,
        };
        CommandError::new(code, format!("could not read attachment metadata: {error}"))
    })?;
    if metadata.is_dir() {
        return Err(CommandError::new(
            CommandErrorCode::FileReadFailed,
            "the selected attachment path is a directory",
        ));
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(attachment_too_large(metadata.len()));
    }

    // Read at most one byte past the limit so a file that grew after the
    // metadata check is still rejected without loading it whole.
    let file = File::open(path).map_err(|error| {
        let code = match error.kind() {
            std::io::ErrorKind::PermissionDenied => CommandErrorCode::PermissionDenied,
            _ => CommandErrorCode::FileReadFailed,
        };
        CommandError::new(code, format!("could not open attachment: {error}"))
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::FileReadFailed,
                format!("could not read attachment: {error}"),
            )
        })?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(attachment_too_large(bytes.len() as u64));
    }
    Ok(bytes)
}

fn attachment_too_large(size: u64) -> CommandError {
    CommandError::new(
        CommandErrorCode::AttachmentTooLarge,
        format!("attachment is {size} bytes, above the {MAX_ATTACHMENT_BYTES} byte limit"),
    )
}

/// Makes a user-chosen file name safe as a PDF attachment name and as a
/// single extraction-path component.
///
/// Path separators are stripped (only the base name survives), control and
/// Windows-reserved characters become `_`, leading/trailing dots are removed,
/// reserved device names are prefixed, and the length is bounded while keeping
/// the extension.
pub(crate) fn sanitize_attachment_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut cleaned: String = base
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    cleaned = cleaned.trim().trim_matches('.').to_owned();
    if cleaned.is_empty() {
        return "attachment".to_owned();
    }
    if is_reserved_device_name(&cleaned) {
        cleaned.insert(0, '_');
    }
    truncate_name(cleaned)
}

fn is_reserved_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

fn truncate_name(name: String) -> String {
    if name.chars().count() <= MAX_ATTACHMENT_NAME_CHARS {
        return name;
    }
    let (stem, extension) = split_extension(&name);
    let extension = extension.unwrap_or_default();
    let reserved = extension.chars().count() + 1;
    let keep = MAX_ATTACHMENT_NAME_CHARS.saturating_sub(reserved).max(1);
    let mut truncated: String = stem.chars().take(keep).collect();
    if !extension.is_empty() {
        truncated.push('.');
        truncated.push_str(extension);
    }
    truncated
}

/// Applies the name-tree rewrite to the appended `lopdf` revision.
///
/// The tree is flattened to one sorted leaf while references to existing
/// filespec objects are preserved; unrelated name trees (`/Dests` and any
/// others) are untouched because only `/EmbeddedFiles` is replaced.
pub(crate) fn apply_embedded_files_edit(
    incremental: &mut lopdf::IncrementalDocument,
    edit: &EmbeddedFilesEdit,
) -> Result<(), CommandError> {
    let catalog_id = incremental
        .get_prev_documents()
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .map_err(|error| {
            crate::save::lopdf_error("the saved document has no /Root reference", error)
        })?;

    let (names_id, mut pairs) = {
        let previous = incremental.get_prev_documents();
        let catalog = previous.get_dictionary(catalog_id).map_err(|error| {
            crate::save::lopdf_error("could not read the catalog for attachments", error)
        })?;
        let names_object = catalog.get(b"Names").ok();
        let names_id = match names_object {
            Some(lopdf::Object::Reference(id)) => Some(*id),
            _ => None,
        };
        let mut pairs = Vec::new();
        let names_dictionary = names_object.and_then(|object| match object {
            lopdf::Object::Dictionary(dictionary) => Some(dictionary),
            lopdf::Object::Reference(id) => previous.get_dictionary(*id).ok(),
            _ => None,
        });
        if let Some(names_dictionary) = names_dictionary {
            if let Ok(root) = names_dictionary.get(b"EmbeddedFiles") {
                collect_name_pairs(previous, root, &mut pairs, 0);
            }
        }
        (names_id, pairs)
    };

    let remove: HashSet<&str> = edit.remove.iter().map(String::as_str).collect();
    let replace: HashSet<&str> = edit.create.iter().map(|spec| spec.name.as_str()).collect();
    pairs.retain(|(key, _)| match key {
        lopdf::Object::String(bytes, _) => {
            let name = crate::save::decode_pdf_text(bytes);
            !remove.contains(name.as_str()) && !replace.contains(name.as_str())
        }
        _ => true,
    });
    for spec in &edit.create {
        pairs.push(create_filespec(incremental, spec)?);
    }

    pairs.sort_by(|(left, _), (right, _)| key_bytes(left).cmp(key_bytes(right)));
    let mut unique_pairs: Vec<(lopdf::Object, lopdf::ObjectId)> = Vec::new();
    for pair in pairs {
        if let Some(last) = unique_pairs.last_mut() {
            if key_bytes(&last.0) == key_bytes(&pair.0) {
                // Stable sort plus append order means a created entry replaces
                // the baseline entry with the same name.
                *last = pair;
                continue;
            }
        }
        unique_pairs.push(pair);
    }

    let limits = match (unique_pairs.first(), unique_pairs.last()) {
        (Some((first, _)), Some((last, _))) => {
            Some(lopdf::Object::Array(vec![first.clone(), last.clone()]))
        }
        _ => None,
    };
    let mut names_array = Vec::with_capacity(unique_pairs.len() * 2);
    for (key, id) in unique_pairs {
        names_array.push(key);
        names_array.push(lopdf::Object::Reference(id));
    }
    let mut leaf = lopdf::Dictionary::new();
    leaf.set(b"Names", lopdf::Object::Array(names_array));
    if let Some(limits) = limits {
        leaf.set(b"Limits", limits);
    }

    incremental
        .opt_clone_object_to_new_document(catalog_id)
        .map_err(|error| {
            crate::save::lopdf_error("could not clone the catalog for attachments", error)
        })?;
    if let Some(names_id) = names_id {
        incremental
            .opt_clone_object_to_new_document(names_id)
            .map_err(|error| {
                crate::save::lopdf_error("could not clone the /Names dictionary", error)
            })?;
        let names = incremental
            .new_document
            .get_dictionary_mut(names_id)
            .map_err(|error| {
                crate::save::lopdf_error("could not edit the /Names dictionary", error)
            })?;
        names.set(b"EmbeddedFiles", lopdf::Object::Dictionary(leaf));
    } else {
        let catalog = incremental
            .new_document
            .get_dictionary_mut(catalog_id)
            .map_err(|error| {
                crate::save::lopdf_error("could not edit the catalog for attachments", error)
            })?;
        if matches!(catalog.get(b"Names"), Ok(lopdf::Object::Dictionary(_))) {
            let names = catalog
                .get_mut(b"Names")
                .and_then(lopdf::Object::as_dict_mut)
                .map_err(|error| {
                    crate::save::lopdf_error("could not edit the /Names dictionary", error)
                })?;
            names.set(b"EmbeddedFiles", lopdf::Object::Dictionary(leaf));
        } else {
            let mut names = lopdf::Dictionary::new();
            names.set(b"EmbeddedFiles", lopdf::Object::Dictionary(leaf));
            catalog.set(b"Names", lopdf::Object::Dictionary(names));
        }
    }
    Ok(())
}

fn key_bytes(object: &lopdf::Object) -> &[u8] {
    match object {
        lopdf::Object::String(bytes, _) => bytes,
        _ => &[],
    }
}

fn collect_name_pairs(
    document: &lopdf::Document,
    node: &lopdf::Object,
    pairs: &mut Vec<(lopdf::Object, lopdf::ObjectId)>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let Some(dictionary) = resolve_dictionary(document, node) else {
        return;
    };
    if let Ok(names) = dictionary.get(b"Names") {
        if let Ok(array) = names.as_array() {
            for pair in array.chunks_exact(2) {
                if matches!(pair[0], lopdf::Object::String(_, _)) {
                    if let Ok(id) = pair[1].as_reference() {
                        pairs.push((pair[0].clone(), id));
                    }
                }
            }
        }
    }
    if let Ok(kids) = dictionary.get(b"Kids") {
        if let Ok(array) = kids.as_array() {
            for kid in array {
                collect_name_pairs(document, kid, pairs, depth + 1);
            }
        }
    }
}

fn resolve_dictionary<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Dictionary> {
    match object {
        lopdf::Object::Dictionary(dictionary) => Some(dictionary),
        lopdf::Object::Reference(id) => document.get_dictionary(*id).ok(),
        _ => None,
    }
}

fn create_filespec(
    incremental: &mut lopdf::IncrementalDocument,
    spec: &CreatedAttachmentSpec,
) -> Result<(lopdf::Object, lopdf::ObjectId), CommandError> {
    let mut params = lopdf::Dictionary::new();
    params.set(b"Size", lopdf::Object::Integer(spec.bytes.len() as i64));
    let mut stream_dictionary = lopdf::Dictionary::new();
    stream_dictionary.set(b"Type", lopdf::Object::Name(b"EmbeddedFile".to_vec()));
    stream_dictionary.set(
        b"Subtype",
        lopdf::Object::Name(spec.media_type.as_bytes().to_vec()),
    );
    stream_dictionary.set(b"Params", lopdf::Object::Dictionary(params));
    let stream_id = incremental
        .new_document
        .try_new_object_id()
        .map_err(|error| crate::save::lopdf_error("could not allocate an embedded file", error))?;
    incremental.new_document.set_object(
        stream_id,
        lopdf::Stream::new(stream_dictionary, spec.bytes.as_ref().clone()),
    );

    let mut ef = lopdf::Dictionary::new();
    ef.set(b"F", lopdf::Object::Reference(stream_id));
    let mut filespec = lopdf::Dictionary::new();
    filespec.set(b"Type", lopdf::Object::Name(b"Filespec".to_vec()));
    filespec.set(b"F", crate::save::text_string_object(&spec.name));
    filespec.set(b"UF", crate::save::text_string_object(&spec.name));
    filespec.set(b"EF", lopdf::Object::Dictionary(ef));
    let filespec_id = incremental
        .new_document
        .try_new_object_id()
        .map_err(|error| crate::save::lopdf_error("could not allocate a filespec", error))?;
    incremental.new_document.set_object(filespec_id, filespec);
    Ok((crate::save::text_string_object(&spec.name), filespec_id))
}

/// MIME type for an attachment file name. Attachment metadata is displayed
/// from the file name, and new embedded-file streams carry the same value in
/// `/Subtype`, so a saved attachment round-trips with the type shown here.
pub(crate) fn media_type_for_name(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "text" | "log" => "text/plain",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}
