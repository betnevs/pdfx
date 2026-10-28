//! Save As / Save Changes pipeline.
//!
//! The Source Document is never written. The first save chooses an Output
//! Document path (Save As); later Save Changes overwrite that same output.
//! Each save rebuilds the output from the original Source Document bytes plus
//! staged revisions:
//!
//! 1. PDFium writes its edits incrementally on the live document handle, so the
//!    result begins with the source bytes (or, for encrypted sources, is a full
//!    rewrite with `FPDF_REMOVE_SECURITY`).
//! 2. `lopdf` appends one revision for object-level edits PDFium cannot make.
//!    Only one engine writes a revision, and lopdf always appends to PDFium's
//!    output rather than to the source.
//!
//! The output is written to a temp file in the target directory, fsynced and
//! renamed into place; when an Output Document already exists it is first
//! copied to a backup. A failure at any step leaves both the Source Document
//! and the previous Output Document unchanged.
//!
//! Design deviation from the ticket's "reopen PDFium from the saved bytes":
//! the live PDFium handle is kept across saves. Undo/redo snapshots are
//! detached page objects bound to that document, so reopening from saved bytes
//! would invalidate the history the user still expects to be able to undo.
//! A probe confirmed that an incremental save from the same handle re-emits
//! every edited object, so each output still contains exactly one PDFium
//! revision appended to the unmodified source bytes.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::commands::{with_open_document, CommandError, CommandErrorCode};
use crate::logging;
use crate::pdfium_adapter::{save_document_with_mode, SaveMode};
use crate::text_edit::EditHistoryView;

/// Event emitted to the saving window for every progress update.
pub const SAVE_PROGRESS_EVENT: &str = "pdfx://save-progress";

/// The two save entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveKind {
    /// Choose a new Output Document path and write it.
    SaveAs,
    /// Overwrite the Output Document chosen by a previous Save As.
    SaveChanges,
}

/// Progress stages of one save. Large files spend most time in `Serializing`;
/// the frontend shows an indeterminate indicator for that stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveStage {
    Preparing,
    Serializing,
    Finalizing,
    Writing,
    Done,
}

/// One progress update crossing the command boundary. Stages are emitted in
/// order and `Done` is only emitted after the output is replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProgress {
    pub document_id: String,
    pub stage: SaveStage,
    pub completed: u32,
    pub total: u32,
}

/// A save request. `save_as_path` is `Some` for Save As and `None` for Save
/// Changes to the remembered Output Document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDocumentRequest {
    pub save_as_path: Option<String>,
    /// Set only after the user accepted that an encrypted Source Document is
    /// written as an unencrypted Output Document (ADR-0007).
    pub confirm_unencrypted_output: bool,
}

/// The outcome of a completed save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResultView {
    pub document_id: String,
    pub output_path: String,
    pub kind: SaveKind,
    pub bytes_written: u64,
    /// The previous Output Document, preserved before it was replaced.
    pub backup_path: Option<String>,
    /// Clean state after the write. Undo/redo history is untouched.
    pub history: EditHistoryView,
}

/// The `save_document` command.
#[tauri::command]
pub async fn save_document(
    app: AppHandle,
    document_id: String,
    request: SaveDocumentRequest,
) -> Result<SaveResultView, CommandError> {
    save_document_with_progress(&document_id, request, &mut |progress| {
        let _ = app.emit(SAVE_PROGRESS_EVENT, progress);
    })
}

/// An object-level change PDFium cannot make itself. These are applied as one
/// `lopdf` revision appended after PDFium's incremental revision. Text and
/// Text Box edits are PDFium page-object edits, so they do not appear here;
/// AcroForm and font-embedding edits will.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectEdit {
    /// Sets an entry in the document `/Info` dictionary.
    SetInfoEntry { key: String, value: String },
}

/// Per-document save state, kept next to the PDFium document.
#[derive(Debug, Default)]
pub struct SaveState {
    /// Output Document chosen by Save As; Save Changes overwrites it.
    pub(crate) output_path: Option<PathBuf>,
    /// Object-level edits re-applied to every revision. They are only ever
    /// cleared by an edit command that moves the change into the live PDFium
    /// document, never by saving.
    pub(crate) object_edits: Vec<ObjectEdit>,
}

/// The save-pipeline seam: the same code the Tauri command runs, with progress
/// delivered to a callback instead of a window.
pub fn save_document_with_progress(
    document_id: &str,
    request: SaveDocumentRequest,
    progress: &mut dyn FnMut(SaveProgress),
) -> Result<SaveResultView, CommandError> {
    with_open_document(document_id, |entry| {
        let (encrypted, read_only, _) = crate::commands::document_permissions(&entry.document);

        // Save Changes is disabled for a Permission-Restricted document; Save
        // As remains available so the user can keep an unencrypted copy.
        if request.save_as_path.is_none() && read_only {
            return Err(CommandError::new(
                CommandErrorCode::ReadOnlyDocument,
                "this document's Permission Restrictions forbid saving changes",
            ));
        }

        let (kind, target) = resolve_target(&entry.save, &request)?;
        ensure_target_is_not_source(&entry.source_path, &target)?;

        // ADR-0007: an encrypted Source Document is always written as an
        // unencrypted Output Document, and only after explicit confirmation.
        if encrypted && !request.confirm_unencrypted_output {
            return Err(CommandError::new(
                CommandErrorCode::UnencryptedOutputConfirmationRequired,
                "the Source Document is encrypted; the Output Document is written unencrypted",
            ));
        }

        emit(document_id, SaveStage::Preparing, 0, progress);
        emit(document_id, SaveStage::Serializing, 1, progress);
        let output_bytes = serialize_output(entry, encrypted)?;

        emit(document_id, SaveStage::Finalizing, 2, progress);
        let output_bytes = append_object_edits(output_bytes, &entry.save.object_edits)?;

        emit(document_id, SaveStage::Writing, 3, progress);
        let written = write_output_atomically(&target, &output_bytes)?;

        // The successful disk-write point for ticket 16's local log: path and
        // outcome only, never file contents.
        logging::event(
            "document.save",
            &format!(
                "documentId={document_id} kind={} path={} bytes={} outcome=ok{}",
                match kind {
                    SaveKind::SaveAs => "save_as",
                    SaveKind::SaveChanges => "save_changes",
                },
                target.display(),
                written.bytes_written,
                written
                    .backup_path
                    .as_ref()
                    .map(|path| format!(" backup={}", path.display()))
                    .unwrap_or_default(),
            ),
        );

        // The output now exists and is complete; only now move the clean
        // baseline and remember the target. A failure above leaves both dirty
        // state and the previous target untouched.
        entry.save.output_path = Some(target.clone());
        entry.edit.saved_state = entry.edit.current_state;

        emit(document_id, SaveStage::Done, 4, progress);

        Ok(SaveResultView {
            document_id: document_id.to_owned(),
            output_path: target.to_string_lossy().into_owned(),
            kind,
            bytes_written: written.bytes_written,
            backup_path: written
                .backup_path
                .map(|path| path.to_string_lossy().into_owned()),
            history: entry.edit.history_view(),
        })
    })
}

/// Progress total: Preparing, Serializing, Finalizing, Writing, Done.
const PROGRESS_TOTAL: u32 = 4;

fn emit(
    document_id: &str,
    stage: SaveStage,
    completed: u32,
    progress: &mut dyn FnMut(SaveProgress),
) {
    progress(SaveProgress {
        document_id: document_id.to_owned(),
        stage,
        completed,
        total: PROGRESS_TOTAL,
    });
}

/// Resolves the Output Document path and save kind, applying the read-only and
/// Save-Changes-needs-a-target rules.
fn resolve_target(
    state: &SaveState,
    request: &SaveDocumentRequest,
) -> Result<(SaveKind, PathBuf), CommandError> {
    match &request.save_as_path {
        Some(path) if !path.is_empty() => Ok((SaveKind::SaveAs, PathBuf::from(path))),
        Some(_) => Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "the Save As target path must not be empty",
        )),
        None => match &state.output_path {
            Some(path) => Ok((SaveKind::SaveChanges, path.clone())),
            None => Err(CommandError::new(
                CommandErrorCode::NoOutputDocument,
                "this document has no Output Document yet; use Save As first",
            )),
        },
    }
}

/// Refuses a target that resolves to the Source Document. The Source Document
/// is never modified, including through a symlink that points at it.
fn ensure_target_is_not_source(source: &Path, target: &Path) -> Result<(), CommandError> {
    if same_file(source, target) {
        Err(CommandError::new(
            CommandErrorCode::SaveTargetIsSource,
            format!(
                "{} is the Source Document; choose a different Output Document path",
                source.display()
            ),
        ))
    } else {
        Ok(())
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    // Canonical paths resolve symlinks and relative components. For a target
    // that does not exist yet, resolve its parent so `../` cannot hide it.
    let normalize = |path: &Path| -> Option<String> {
        let resolved = match fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(_) => {
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let name = path.file_name()?;
                fs::canonicalize(parent).ok()?.join(name)
            }
        };
        let text = resolved.to_string_lossy().into_owned();
        // macOS and Windows filesystems are case-insensitive by default.
        Some(if cfg!(any(target_os = "macos", target_os = "windows")) {
            text.to_lowercase()
        } else {
            text
        })
    };

    match (normalize(left), normalize(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Serializes the live document.
///
/// PDFium writes every edit made through its page-object API in one incremental
/// revision. The very first save of an unedited document is copied byte for
/// byte, which is the highest fidelity result and the fastest path. Encrypted
/// sources always take the `FPDF_REMOVE_SECURITY` full rewrite because ADR-0007
/// requires an unencrypted Output Document.
///
/// The byte-verbatim fast path is only valid before an Output Document exists:
/// once PDFium edits were saved, the live document (not the Source Document)
/// holds the user's work, so a clean Save Changes must re-serialize the handle
/// — copying the source bytes there would silently drop every previously saved
/// edit from the Output Document.
fn serialize_output(
    entry: &crate::commands::OpenDocument,
    encrypted: bool,
) -> Result<Vec<u8>, CommandError> {
    if !encrypted
        && entry.save.output_path.is_none()
        && entry.edit.current_state == entry.edit.saved_state
    {
        return Ok(entry.source_bytes.clone());
    }

    let mode = if encrypted {
        SaveMode::RemoveSecurity
    } else {
        SaveMode::Incremental
    };
    save_document_with_mode(&entry.document, mode).map_err(adapter_error)
}

fn adapter_error(error: crate::pdfium_adapter::AdapterError) -> CommandError {
    CommandError::new(
        CommandErrorCode::SaveFailed,
        format!("PDFium could not serialize the document: {error}"),
    )
}

/// Appends one `lopdf` revision carrying object-level edits PDFium cannot make.
/// With no object-level edits the PDFium bytes are already the final output.
///
/// The revision always appends to `base` (PDFium's output), never to the
/// Source Document, and only the named objects are cloned into the revision.
/// Callers must pass bytes from an unencrypted source: encrypted documents are
/// rewritten with `FPDF_REMOVE_SECURITY` first.
pub fn append_object_edits(base: Vec<u8>, edits: &[ObjectEdit]) -> Result<Vec<u8>, CommandError> {
    if edits.is_empty() {
        return Ok(base);
    }

    let previous = lopdf::Document::load_mem(&base).map_err(|error| {
        lopdf_error(
            "could not parse the PDFium revision for object-level edits",
            error,
        )
    })?;
    let mut incremental = lopdf::IncrementalDocument::create_from(base, previous);

    for edit in edits {
        apply_object_edit(&mut incremental, edit)?;
    }

    let mut output = Vec::new();
    incremental
        .save_to(&mut output)
        .map_err(|error| lopdf_error("could not append the object-level revision", error))?;
    Ok(output)
}

fn apply_object_edit(
    incremental: &mut lopdf::IncrementalDocument,
    edit: &ObjectEdit,
) -> Result<(), CommandError> {
    match edit {
        ObjectEdit::SetInfoEntry { key, value } => {
            let info_id = incremental
                .get_prev_documents()
                .trailer
                .get(b"Info")
                .ok()
                .and_then(|object| object.as_reference().ok());

            match info_id {
                Some(id) => {
                    incremental
                        .opt_clone_object_to_new_document(id)
                        .map_err(|error| {
                            lopdf_error("could not clone the Info dictionary", error)
                        })?;
                    let info = incremental
                        .new_document
                        .get_object_mut(id)
                        .and_then(lopdf::Object::as_dict_mut)
                        .map_err(|error| {
                            lopdf_error("could not edit the Info dictionary", error)
                        })?;
                    info.set(
                        key.as_bytes().to_vec(),
                        lopdf::Object::string_literal(value.as_bytes().to_vec()),
                    );
                }
                None => {
                    // No /Info yet: add one and point the appended trailer at it.
                    let id = incremental
                        .new_document
                        .try_new_object_id()
                        .map_err(|error| lopdf_error("could not allocate an Info object", error))?;
                    let mut info = lopdf::Dictionary::new();
                    info.set(
                        key.as_bytes().to_vec(),
                        lopdf::Object::string_literal(value.as_bytes().to_vec()),
                    );
                    incremental.new_document.set_object(id, info);
                    incremental
                        .new_document
                        .trailer
                        .set(b"Info", lopdf::Object::Reference(id));
                }
            }
            Ok(())
        }
    }
}

fn lopdf_error(context: &str, error: impl std::fmt::Display) -> CommandError {
    CommandError::new(CommandErrorCode::SaveFailed, format!("{context}: {error}"))
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Backup naming convention: the output's stem gains `.pdfx-backup` before its
/// extension, so `report.pdf` becomes `report.pdfx-backup.pdf` next to it.
/// Each Save Changes replaces that backup with the version it is about to
/// overwrite.
pub fn backup_path_for(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    let extension = output
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()));
    output.with_file_name(format!(
        "{stem}.pdfx-backup{}",
        extension.unwrap_or_default()
    ))
}

struct WrittenOutput {
    bytes_written: u64,
    backup_path: Option<PathBuf>,
}

/// Writes `bytes` to a temp file next to `target`, fsyncs it and renames it into
/// place. The temp file lives in the target directory so the rename stays on one
/// filesystem and is atomic.
///
/// When `target` already exists it is first copied to the backup path through
/// another temp file and rename, so a failed save leaves the previous Output
/// Document byte-identical.
fn write_output_atomically(target: &Path, bytes: &[u8]) -> Result<WrittenOutput, CommandError> {
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("{} is not a file path", target.display()),
            )
        })?;

    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = directory.join(format!(
        ".{}.pdfx-save-{}-{unique}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let mut temp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| {
            save_failure("could not create the temporary output", &temp_path, error)
        })?;

    let write_result = temp.write_all(bytes).and_then(|()| temp.sync_all());
    drop(temp);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(save_failure(
            "could not write the temporary output",
            &temp_path,
            error,
        ));
    }

    let backup_path = if target.exists() {
        let backup = backup_path_for(target);
        if let Err(error) = copy_to_backup(target, &backup) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(&temp_path, target) {
        let _ = fs::remove_file(&temp_path);
        return Err(save_failure(
            "could not replace the Output Document",
            target,
            error,
        ));
    }

    // Make the rename durable. Directory fsync is not available on Windows and
    // is best-effort elsewhere: the file is already in place.
    #[cfg(unix)]
    if let Ok(directory_handle) = fs::File::open(directory) {
        let _ = directory_handle.sync_all();
    }

    Ok(WrittenOutput {
        bytes_written: bytes.len() as u64,
        backup_path,
    })
}

/// Copies the existing Output Document to the backup path atomically.
fn copy_to_backup(output: &Path, backup: &Path) -> Result<(), CommandError> {
    let directory = backup
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = backup
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "backup".to_owned());
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = directory.join(format!(
        ".{file_name}.pdfx-backup-{}-{unique}.tmp",
        std::process::id()
    ));

    fs::copy(output, &temp_path)
        .map_err(|error| save_failure("could not back up the Output Document", output, error))?;
    let synced = fs::File::open(&temp_path).and_then(|file| file.sync_all());
    if let Err(error) = synced {
        let _ = fs::remove_file(&temp_path);
        return Err(save_failure(
            "could not flush the Output Document backup",
            &temp_path,
            error,
        ));
    }
    if let Err(error) = fs::rename(&temp_path, backup) {
        let _ = fs::remove_file(&temp_path);
        return Err(save_failure(
            "could not install the Output Document backup",
            backup,
            error,
        ));
    }
    Ok(())
}

fn save_failure(context: &str, path: &Path, error: io::Error) -> CommandError {
    CommandError::new(
        CommandErrorCode::SaveFailed,
        format!("{context} ({}): {error}", path.display()),
    )
}
