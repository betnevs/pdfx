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
//!    output rather than to the source. Free text annotations need this pass
//!    because PDFium has no integer annotation-value API: `/Q` (alignment) and
//!    `/IT` are written from the annotation's private PDFX keys here.
//!
//! Font subsets follow the same rule. CJK text loads a `subsetter`-produced
//! CIDFontType2 into the live document with `FPDFText_LoadCidType2Font`, so
//! PDFium writes the font program, `/W`, ToUnicode and CIDToGIDMap into its
//! incremental revision. Fonts therefore need no `ObjectEdit` and cannot end
//! up in a second save path. The edit session caches the font token per
//! character set, so repeat saves re-emit the same subset instead of embedding
//! a duplicate (see `src-tauri/tests/cjk_fonts.rs`).
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
/// AcroForm creation and its embedded appearance font do.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectEdit {
    /// Sets an entry in the document `/Info` dictionary.
    SetInfoEntry { key: String, value: String },
    /// Sets a radio group's value and every widget's appearance state.
    ///
    /// Pdfium's public API writes `/V` on one widget annotation only, while a
    /// radio group stores its value on the parent field. The edit writes the
    /// parent `/V` and each widget's `/AS` in the appended revision.
    SetRadioGroup { field_name: String, state: String },
    /// Sets a choice field's `/V` and, when the label is one of `/Opt`, `/I`.
    SetChoiceValue { field_name: String, label: String },
    /// Writes the AcroForm fields created in this session: `/AcroForm`,
    /// `/Fields`, `/DA`, `/DR`, widget dictionaries, appearance streams and —
    /// when text fields exist — the embedded CJK font subset.
    CreateFormFields {
        fields: Vec<crate::form_create::FormFieldSpec>,
    },
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
        // Form values that live in the edit history (radio groups, choice
        // fields) are re-derived on every save, so undo/redo is reflected in
        // the output without a second copy of form state.
        let mut object_edits = entry.save.object_edits.clone();
        object_edits.extend(crate::form::save_object_edits(&entry.edit));
        // Created form fields are re-derived from the session on every save,
        // so repeated saves cannot duplicate them and undo/redo is reflected
        // in the output.
        object_edits.extend(crate::form_create::save_object_edits(&entry.edit));
        let output_bytes = append_object_edits(output_bytes, &object_edits)?;
        // Free text alignment lives in a private string key because PDFium
        // cannot write an integer annotation entry. Translate it into `/Q`
        // (and `/IT`) in one appended revision; undo/redo and repeat saves
        // therefore always reflect the current annotation state.
        let output_bytes = apply_annotation_dictionary_edits(output_bytes)?;

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
        entry.edit.mark_document_saved();

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
    if crate::target_path::same_file(source, target) {
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

/// The deferred standard entries for one PDFX free text annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FreeTextDictionaryEdit {
    q: Option<i64>,
    it: Vec<u8>,
}

/// A page whose `/Annots` array is rewritten because Pdfium stores the
/// annotation dictionaries inline. Writing a direct dictionary requires
/// cloning its containing page object into the appended revision.
struct PageAnnotsEdit {
    page_id: lopdf::ObjectId,
    annots: Vec<lopdf::Object>,
}

/// A referenced annotation that can be edited by cloning its own object.
struct ReferencedAnnotationEdit {
    id: lopdf::ObjectId,
    edit: FreeTextDictionaryEdit,
}

/// Derives the standard `/Q` and `/IT` entries for every PDFX free text
/// annotation in a PDFium revision.
///
/// Pdfium writes annotation dictionaries inline in `/Annots`, so inline entries
/// are rewritten through their page object while referenced entries are edited
/// directly. Foreign free text annotations have no PDFX style state and are
/// left untouched.
fn free_text_dictionary_edits(
    document: &lopdf::Document,
) -> (Vec<PageAnnotsEdit>, Vec<ReferencedAnnotationEdit>) {
    fn edit_for(dictionary: &lopdf::Dictionary) -> Option<FreeTextDictionaryEdit> {
        if dictionary
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .ok()
            != Some(b"FreeText".as_slice())
        {
            return None;
        }
        let kind = dictionary
            .get(b"PDFXFreeTextKind")
            .ok()
            .and_then(|object| object.as_str().ok())
            .map(decode_pdf_text)?;
        let q = dictionary
            .get(b"PDFXFreeTextAlign")
            .ok()
            .and_then(|object| object.as_str().ok())
            .map(decode_pdf_text)
            .and_then(|value| crate::annotations::FreeTextAlign::parse(&value))
            .map(crate::annotations::FreeTextAlign::pdf_q);
        let it = if kind == "callout" {
            b"FreeTextCallout".to_vec()
        } else {
            b"FreeText".to_vec()
        };
        Some(FreeTextDictionaryEdit { q, it })
    }

    let mut page_edits = Vec::new();
    let mut referenced_edits = Vec::new();
    for page_id in document.get_pages().values() {
        let Ok(page) = document.get_dictionary(*page_id) else {
            continue;
        };
        let Ok(annots) = page.get_deref(b"Annots", document) else {
            continue;
        };
        let Ok(array) = annots.as_array() else {
            continue;
        };
        let mut rewritten = Vec::with_capacity(array.len());
        let mut changed = false;
        for object in array {
            match object {
                lopdf::Object::Dictionary(dictionary) => match edit_for(dictionary) {
                    Some(edit) => {
                        let mut updated = dictionary.clone();
                        if let Some(q) = edit.q {
                            updated.set(b"Q".to_vec(), lopdf::Object::Integer(q));
                        }
                        updated.set(b"IT".to_vec(), lopdf::Object::Name(edit.it));
                        rewritten.push(lopdf::Object::Dictionary(updated));
                        changed = true;
                    }
                    None => rewritten.push(object.clone()),
                },
                lopdf::Object::Reference(id) => {
                    if let Ok(dictionary) = document.get_dictionary(*id) {
                        if let Some(edit) = edit_for(dictionary) {
                            referenced_edits.push(ReferencedAnnotationEdit { id: *id, edit });
                        }
                    }
                    rewritten.push(object.clone());
                }
                other => rewritten.push(other.clone()),
            }
        }
        if changed {
            page_edits.push(PageAnnotsEdit {
                page_id: *page_id,
                annots: rewritten,
            });
        }
    }
    (page_edits, referenced_edits)
}

/// Writes `/Q` and `/IT` for PDFX free text annotations into one appended
/// revision.
///
/// PDFium's annotation API cannot set an integer dictionary entry, so the
/// alignment is mirrored as a string while editing and translated here on every
/// save. The pass is stateless: it reads the current PDFium output, so undo,
/// redo and repeated saves all produce the same result and never accumulate
/// duplicate annotations.
pub fn apply_annotation_dictionary_edits(base: Vec<u8>) -> Result<Vec<u8>, CommandError> {
    let previous = lopdf::Document::load_mem(&base).map_err(|error| {
        lopdf_error(
            "could not parse the PDFium revision for annotation dictionary edits",
            error,
        )
    })?;
    let edits = free_text_dictionary_edits(&previous);
    let (page_edits, referenced_edits) = edits;
    if page_edits.is_empty() && referenced_edits.is_empty() {
        return Ok(base);
    }

    let mut incremental = lopdf::IncrementalDocument::create_from(base, previous);
    for edit in &page_edits {
        incremental
            .opt_clone_object_to_new_document(edit.page_id)
            .map_err(|error| lopdf_error("could not clone a page for annotation edits", error))?;
        let page = incremental
            .new_document
            .get_dictionary_mut(edit.page_id)
            .map_err(|error| lopdf_error("could not edit a page for annotation edits", error))?;
        page.set(
            b"Annots".to_vec(),
            lopdf::Object::Array(edit.annots.clone()),
        );
    }
    for edit in &referenced_edits {
        incremental
            .opt_clone_object_to_new_document(edit.id)
            .map_err(|error| lopdf_error("could not clone a free text annotation", error))?;
        let dictionary = incremental
            .new_document
            .get_dictionary_mut(edit.id)
            .map_err(|error| lopdf_error("could not edit a free text annotation", error))?;
        if let Some(q) = edit.edit.q {
            dictionary.set(b"Q".to_vec(), lopdf::Object::Integer(q));
        }
        dictionary.set(b"IT".to_vec(), lopdf::Object::Name(edit.edit.it.clone()));
    }

    let mut output = Vec::new();
    incremental
        .save_to(&mut output)
        .map_err(|error| lopdf_error("could not append the annotation revision", error))?;
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
        ObjectEdit::SetRadioGroup { field_name, state } => {
            apply_radio_group(incremental, field_name, state)
        }
        ObjectEdit::SetChoiceValue { field_name, label } => {
            apply_choice_value(incremental, field_name, label)
        }
        ObjectEdit::CreateFormFields { fields } => {
            crate::form_create::apply_created_fields(incremental, fields)
        }
    }
}

/// Decodes a PDF text string: UTF-16BE when it carries the BOM, otherwise the
/// bytes are compared as-is (PDFDocEncoding is ASCII-compatible for the
/// characters form option labels use in practice).
fn decode_pdf_text(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn text_string_object(value: &str) -> lopdf::Object {
    if value.is_ascii() {
        lopdf::Object::string_literal(value.as_bytes().to_vec())
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in value.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        lopdf::Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }
}

fn option_label(object: &lopdf::Object) -> Option<String> {
    match object {
        lopdf::Object::String(bytes, _) => Some(decode_pdf_text(bytes)),
        lopdf::Object::Array(items) => items
            .first()
            .and_then(|item| item.as_str().ok())
            .map(decode_pdf_text),
        _ => None,
    }
}

fn find_form_field_id(document: &lopdf::Document, name: &str) -> Option<lopdf::ObjectId> {
    let catalog = document.catalog().ok()?;
    let acro_form = catalog
        .get_deref(b"AcroForm", document)
        .ok()?
        .as_dict()
        .ok()?;
    let fields = acro_form
        .get_deref(b"Fields", document)
        .ok()?
        .as_array()
        .ok()?;
    fields.iter().find_map(|field| {
        let id = field.as_reference().ok()?;
        let dictionary = document.get_dictionary(id).ok()?;
        let field_name = dictionary
            .get(b"T")
            .and_then(lopdf::Object::as_str)
            .ok()
            .map(decode_pdf_text)?;
        (field_name == name).then_some(id)
    })
}

/// Returns the non-Off state name from a widget's normal appearance dictionary.
fn widget_on_state(widget: &lopdf::Dictionary, document: &lopdf::Document) -> Option<Vec<u8>> {
    let normal = widget
        .get_deref(b"AP", document)
        .ok()?
        .as_dict()
        .ok()?
        .get_deref(b"N", document)
        .ok()?;
    normal
        .as_dict()
        .ok()?
        .iter()
        .find(|(state, _)| state.as_slice() != b"Off")
        .map(|(state, _)| state.clone())
}

fn apply_radio_group(
    incremental: &mut lopdf::IncrementalDocument,
    field_name: &str,
    state: &str,
) -> Result<(), CommandError> {
    let (field_id, widget_ids, on_states) = {
        let previous = incremental.get_prev_documents();
        let field_id = find_form_field_id(previous, field_name).ok_or_else(|| {
            lopdf_error(
                "could not find a radio field in the saved document",
                field_name,
            )
        })?;
        let field = previous
            .get_dictionary(field_id)
            .map_err(|error| lopdf_error("could not read the radio field dictionary", error))?;
        let widget_ids: Vec<lopdf::ObjectId> = match field
            .get_deref(b"Kids", previous)
            .ok()
            .and_then(|kids| kids.as_array().ok())
        {
            Some(kids) => kids
                .iter()
                .filter_map(|kid| kid.as_reference().ok())
                .collect(),
            None => vec![field_id],
        };
        let on_states: Vec<Option<Vec<u8>>> = widget_ids
            .iter()
            .map(|id| {
                previous
                    .get_dictionary(*id)
                    .ok()
                    .and_then(|widget| widget_on_state(widget, previous))
            })
            .collect();
        (field_id, widget_ids, on_states)
    };

    incremental
        .opt_clone_object_to_new_document(field_id)
        .map_err(|error| lopdf_error("could not clone the radio field", error))?;
    for id in &widget_ids {
        incremental
            .opt_clone_object_to_new_document(*id)
            .map_err(|error| lopdf_error("could not clone a radio widget", error))?;
    }

    let field = incremental
        .new_document
        .get_dictionary_mut(field_id)
        .map_err(|error| lopdf_error("could not edit the radio field", error))?;
    field.set(
        b"V".to_vec(),
        lopdf::Object::Name(state.as_bytes().to_vec()),
    );

    for (id, on_state) in widget_ids.iter().zip(on_states) {
        let widget = incremental
            .new_document
            .get_dictionary_mut(*id)
            .map_err(|error| lopdf_error("could not edit a radio widget", error))?;
        let appearance: &[u8] = if on_state.as_deref() == Some(state.as_bytes()) {
            state.as_bytes()
        } else {
            b"Off"
        };
        widget.set(b"AS".to_vec(), lopdf::Object::Name(appearance.to_vec()));
    }

    Ok(())
}

fn apply_choice_value(
    incremental: &mut lopdf::IncrementalDocument,
    field_name: &str,
    label: &str,
) -> Result<(), CommandError> {
    let (field_id, index) = {
        let previous = incremental.get_prev_documents();
        let field_id = find_form_field_id(previous, field_name).ok_or_else(|| {
            lopdf_error(
                "could not find a choice field in the saved document",
                field_name,
            )
        })?;
        let field = previous
            .get_dictionary(field_id)
            .map_err(|error| lopdf_error("could not read the choice field dictionary", error))?;
        let index = field
            .get_deref(b"Opt", previous)
            .ok()
            .and_then(|options| options.as_array().ok())
            .and_then(|options| {
                options
                    .iter()
                    .position(|option| option_label(option).as_deref() == Some(label))
            })
            .map(|index| index as i64);
        (field_id, index)
    };

    incremental
        .opt_clone_object_to_new_document(field_id)
        .map_err(|error| lopdf_error("could not clone the choice field", error))?;
    let field = incremental
        .new_document
        .get_dictionary_mut(field_id)
        .map_err(|error| lopdf_error("could not edit the choice field", error))?;
    field.set(b"V".to_vec(), text_string_object(label));
    match index {
        Some(index) => field.set(
            b"I".to_vec(),
            lopdf::Object::Array(vec![lopdf::Object::Integer(index)]),
        ),
        None => {
            field.remove(b"I");
        }
    }

    Ok(())
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
