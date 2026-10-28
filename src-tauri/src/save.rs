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
    /// Sets a copied text field's `/V` in the appended revision.
    ///
    /// Text values on copied pages are session values: the copy's widget may
    /// still resolve to the source page's field in Pdfium, so writing through
    /// Pdfium could change the original. The save pipeline renames the copy's
    /// widget first, then this edit writes the value into the copy's object.
    SetTextValue { field_name: String, text: String },
    /// Sets a copied checkbox's `/V` and every widget's `/AS`.
    SetCheckboxValue { field_name: String, checked: bool },
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
        // Use the Source Document's remembered security: a Document restored
        // from a Recovery File is already decrypted in PDFium, but ADR-0007
        // and the Source's Permission Restrictions still apply to its saves.
        let encrypted = entry.source_security.encrypted;
        let read_only = entry.source_security.read_only;

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
        let output_bytes = serialize_live_document(entry)?;

        emit(document_id, SaveStage::Finalizing, 2, progress);
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

        // The unsaved edits are now on disk, so the Recovery File is stale.
        // The successful Output Document also enters the recent-files list.
        crate::recovery::clear_for_entry(entry);
        crate::recovery::record_recent_document(&target);

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

/// Serializes the live document state — including unsaved edits — into one
/// complete, unencrypted PDF in memory.
///
/// This is the shared seam for writing an Output Document and for flattened-
/// copy export: both must start from exactly the bytes Save Changes would
/// write, without touching the PDFium handle, its undo history or the save
/// baseline. Object-level edits are re-derived from the edit history on every
/// call, so repeat calls are idempotent.
pub(crate) fn serialize_live_document(
    entry: &crate::commands::OpenDocument,
) -> Result<Vec<u8>, CommandError> {
    let (encrypted, _, _) = crate::commands::document_permissions(&entry.document);
    let output_bytes = serialize_output(entry, encrypted)?;

    // Form values that live in the edit history (radio groups, choice fields)
    // are re-derived on every save, so undo/redo is reflected in the output
    // without a second copy of form state.
    let mut object_edits = entry.save.object_edits.clone();
    object_edits.extend(crate::form::save_object_edits(&entry.edit, &entry.pages));
    // Created form fields are re-derived from the session on every save, so
    // repeated saves cannot duplicate them and undo/redo is reflected in the
    // output. Their appearances follow the page's current rotation.
    object_edits.extend(crate::form_create::save_object_edits(
        &entry.edit,
        &entry.pages,
        &entry.document,
    )?);
    // Every lopdf-side change — object edits, deferred annotation dictionary
    // entries, the page arrangement and the attachment name tree — is written
    // in one appended revision after PDFium's incremental one, keeping a
    // single writer per revision.
    let attachment_edit = entry.attachments.save_edit();
    finalize_lopdf_revision(
        output_bytes,
        &object_edits,
        &entry.pages,
        attachment_edit.as_ref(),
    )
}

/// Serializes just PDFium's view of the document, before the appended lopdf
/// revision.
///
/// `copy_page` imports a page from these bytes. Unlike the save output, they
/// keep physical page indices and the live form field names Pdfium assigned at
/// load time, which the importer needs to resolve widgets; values that live
/// only in the session overlay are captured separately by the copy command.
pub(crate) fn serialize_pdfium_state(
    entry: &crate::commands::OpenDocument,
) -> Result<Vec<u8>, CommandError> {
    let (encrypted, _, _) = crate::commands::document_permissions(&entry.document);
    serialize_output(entry, encrypted)
}

/// Applies every lopdf-side change to the PDFium output in one incremental
/// revision. Returns the input untouched when there is nothing to write.
fn finalize_lopdf_revision(
    base: Vec<u8>,
    edits: &[ObjectEdit],
    pages: &crate::page_ops::PageModel,
    attachment_edit: Option<&crate::attachments::EmbeddedFilesEdit>,
) -> Result<Vec<u8>, CommandError> {
    let previous = lopdf::Document::load_mem(&base).map_err(|error| {
        lopdf_error(
            "could not parse the PDFium revision for object-level edits",
            error,
        )
    })?;
    let (page_edits, referenced_edits) = annotation_dictionary_edits(&previous);
    let page_tree = page_tree_edit(&previous, pages)?;
    let form_fields_prune = form_fields_prune(&previous, pages)?;
    let has_copy_pages = pages
        .copies
        .keys()
        .any(|physical| pages.order.contains(physical));
    if edits.is_empty()
        && page_edits.is_empty()
        && referenced_edits.is_empty()
        && page_tree.is_none()
        && form_fields_prune.is_none()
        && attachment_edit.is_none()
        && !has_copy_pages
    {
        return Ok(base);
    }

    let mut incremental = lopdf::IncrementalDocument::create_from(base, previous);
    // Annotation dictionaries are rewritten first: the array comes from the
    // PDFium revision, so created form widgets appended by the object edits
    // below must come after it or they would be overwritten on pages that
    // also carry a PDFX free text annotation.
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
            .map_err(|error| {
                lopdf_error("could not clone an annotation for dictionary edits", error)
            })?;
        let dictionary = incremental
            .new_document
            .get_dictionary_mut(edit.id)
            .map_err(|error| lopdf_error("could not edit an annotation dictionary", error))?;
        apply_dictionary_edit(dictionary, &edit.edit);
    }
    // Pruning comes before the object edits: created fields are appended to
    // this `/Fields` array by `CreateFormFields`, so an earlier prune is not
    // overwritten, and fields on deleted pages never reach the output.
    if let Some(prune) = &form_fields_prune {
        apply_form_fields_prune(&mut incremental, prune)?;
    }
    // Copied pages need their cloned widgets and fields registered under names
    // that are unique in the output AcroForm before the value edits below run.
    apply_copy_fields_fixup(&mut incremental, pages)?;
    // Pdfium's importer keeps link annotations but drops their destination;
    // restore it from the source page before the revision is written.
    apply_copy_links_fixup(&mut incremental, pages)?;
    for edit in edits {
        apply_object_edit(&mut incremental, edit)?;
    }
    if let Some(page_tree) = &page_tree {
        apply_page_tree_edit(&mut incremental, page_tree)?;
    }
    if let Some(attachment_edit) = attachment_edit {
        crate::attachments::apply_embedded_files_edit(&mut incremental, attachment_edit)?;
    }

    let mut output = Vec::new();
    incremental
        .save_to(&mut output)
        .map_err(|error| lopdf_error("could not append the object-level revision", error))?;
    Ok(output)
}

/// The lopdf-side page tree rewrite for a logical arrangement.
///
/// Destinations are deliberately left alone. Bookmark and link destinations
/// reference page objects, so they follow a page through reordering and
/// insertion. Deleting a destination page leaves the destination pointing at
/// an object that is no longer in the page tree (readers show it as
/// unresolved) instead of rewriting an untouched page's annotation.
struct PageTreeEdit {
    /// The document's `/Pages` node; included page dictionaries become its
    /// direct `/Kids` so order, count and hidden pages are expressed there.
    pages_id: lopdf::ObjectId,
    /// Page dictionaries in logical order.
    kids: Vec<lopdf::ObjectId>,
    /// Page boxes and resources to give inserted blank pages.
    blanks: Vec<(lopdf::ObjectId, crate::page_ops::BlankPageSize)>,
}

/// Plans the page tree rewrite, or `None` when the arrangement is the physical
/// order of every page (nothing to change).
fn page_tree_edit(
    document: &lopdf::Document,
    model: &crate::page_ops::PageModel,
) -> Result<Option<PageTreeEdit>, CommandError> {
    let physical: Vec<lopdf::ObjectId> = document.get_pages().values().copied().collect();
    let identity = model.blanks.is_empty()
        && model.order.len() == physical.len()
        && model
            .order
            .iter()
            .enumerate()
            .all(|(index, page)| *page as usize == index);
    if identity {
        return Ok(None);
    }

    let mut kids = Vec::with_capacity(model.order.len());
    for physical_index in &model.order {
        let id = physical
            .get(*physical_index as usize)
            .copied()
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("page arrangement references missing page {physical_index}"),
                )
            })?;
        kids.push(id);
    }

    let catalog = document.catalog().map_err(|error| {
        lopdf_error(
            "could not read the document catalog for the page tree",
            error,
        )
    })?;
    let pages_id = catalog
        .get(b"Pages")
        .and_then(lopdf::Object::as_reference)
        .map_err(|error| lopdf_error("the document catalog has no /Pages reference", error))?;

    let blanks = model
        .blanks
        .iter()
        .filter(|(physical_index, _)| model.order.contains(physical_index))
        .filter_map(|(physical_index, blank)| {
            physical
                .get(*physical_index as usize)
                .map(|id| (*id, blank.size))
        })
        .collect();

    Ok(Some(PageTreeEdit {
        pages_id,
        kids,
        blanks,
    }))
}

/// A `/AcroForm /Fields` rewrite that leaves out fields whose widgets only
/// lived on pages the saved arrangement removes. The AcroForm may sit inline
/// in the catalog or behind a reference; both forms are carried here.
struct FormFieldsPrune {
    catalog: lopdf::ObjectId,
    acro_form: Option<lopdf::ObjectId>,
    dictionary: lopdf::Dictionary,
}

/// Plans dropping root form fields that no longer have a widget on a kept
/// page. Without this, deleting a page would leave its field in the saved
/// AcroForm and readers would still list it even though its page is gone.
/// Fields carrying no `/P` are kept: without a page reference there is no
/// evidence they belong to a removed page.
fn form_fields_prune(
    document: &lopdf::Document,
    model: &crate::page_ops::PageModel,
) -> Result<Option<FormFieldsPrune>, CommandError> {
    let physical: Vec<lopdf::ObjectId> = document.get_pages().values().copied().collect();
    if model.order.len() == physical.len() {
        // No page was removed, so no field can have lost its only widget.
        return Ok(None);
    }
    let included: std::collections::HashSet<lopdf::ObjectId> = model
        .order
        .iter()
        .filter_map(|index| physical.get(*index as usize).copied())
        .collect();
    let catalog = document.catalog().map_err(|error| {
        lopdf_error(
            "could not read the document catalog to prune form fields",
            error,
        )
    })?;
    let catalog_id = document
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .map_err(|error| lopdf_error("the saved document has no /Root reference", error))?;
    let (acro_form_id, acro_form) = match catalog.get(b"AcroForm").ok() {
        Some(lopdf::Object::Reference(id)) => match document.get_dictionary(*id) {
            Ok(dictionary) => (Some(*id), dictionary.clone()),
            Err(_) => return Ok(None),
        },
        Some(lopdf::Object::Dictionary(dictionary)) => (None, dictionary.clone()),
        // No AcroForm: nothing to prune.
        _ => return Ok(None),
    };
    let Ok(fields_object) = acro_form.get_deref(b"Fields", document) else {
        return Ok(None);
    };
    let Ok(fields) = fields_object.as_array() else {
        return Ok(None);
    };
    let kept: Vec<lopdf::Object> = fields
        .iter()
        .filter(|field| field_uses_kept_page(document, field, &included, 0))
        .cloned()
        .collect();
    if kept.len() == fields.len() {
        return Ok(None);
    }
    let mut dictionary = acro_form;
    dictionary.set(b"Fields".to_vec(), lopdf::Object::Array(kept));
    Ok(Some(FormFieldsPrune {
        catalog: catalog_id,
        acro_form: acro_form_id,
        dictionary,
    }))
}

/// Whether a field or any of its `/Kids` has a widget on a kept page. A
/// radio group keeps one widget per option, so a group survives while at
/// least one option's page remains.
fn field_uses_kept_page(
    document: &lopdf::Document,
    object: &lopdf::Object,
    included: &std::collections::HashSet<lopdf::ObjectId>,
    depth: usize,
) -> bool {
    if depth > 32 {
        // Malformed nesting: keep the field rather than risk dropping data.
        return true;
    }
    let dictionary = match object {
        lopdf::Object::Reference(id) => match document.get_dictionary(*id) {
            Ok(dictionary) => dictionary,
            Err(_) => return true,
        },
        lopdf::Object::Dictionary(dictionary) => dictionary,
        _ => return true,
    };
    if let Ok(kids) = dictionary.get_deref(b"Kids", document) {
        if let Ok(kids) = kids.as_array() {
            if !kids.is_empty() {
                return kids
                    .iter()
                    .any(|kid| field_uses_kept_page(document, kid, included, depth + 1));
            }
        }
    }
    match dictionary
        .get(b"P")
        .ok()
        .and_then(|page| page.as_reference().ok())
    {
        Some(page) => included.contains(&page),
        None => true,
    }
}

fn apply_form_fields_prune(
    incremental: &mut lopdf::IncrementalDocument,
    edit: &FormFieldsPrune,
) -> Result<(), CommandError> {
    match edit.acro_form {
        Some(acro_form) => {
            incremental
                .opt_clone_object_to_new_document(acro_form)
                .map_err(|error| {
                    lopdf_error("could not clone the AcroForm for field pruning", error)
                })?;
            *incremental
                .new_document
                .get_dictionary_mut(acro_form)
                .map_err(|error| {
                    lopdf_error("could not edit the AcroForm for field pruning", error)
                })? = edit.dictionary.clone();
        }
        None => {
            incremental
                .opt_clone_object_to_new_document(edit.catalog)
                .map_err(|error| {
                    lopdf_error("could not clone the catalog for field pruning", error)
                })?;
            let catalog = incremental
                .new_document
                .get_dictionary_mut(edit.catalog)
                .map_err(|error| {
                    lopdf_error("could not edit the catalog for field pruning", error)
                })?;
            catalog.set(
                b"AcroForm".to_vec(),
                lopdf::Object::Dictionary(edit.dictionary.clone()),
            );
        }
    }
    Ok(())
}

/// Registers the form fields cloned onto copied pages under names unique in
/// the output AcroForm.
///
/// `FPDF_ImportPages` clones page annotations but not form fields: a merged
/// widget keeps the source field name, and a widget whose value lives on a
/// `/Parent` field keeps a reference to the *source* parent. Appending the
/// output as-is would leave duplicate field names and, for radio groups, two
/// pages editing one field. This pass:
///
/// * renames every merged widget on a visible copy page to
///   `copied_field_name(raw_name, physical_page)` and appends it to `/Fields`;
/// * groups widgets whose parent is shared with the source page, clones their
///   parent under such a name, points the copied widgets at the clone and
///   appends the clone to `/Fields`;
/// * leaves the source page, the original fields and every non-copy page
///   untouched.
///
/// Names derive from the raw Pdfium name, exactly like form discovery's copy
/// namespacing, so the session overlay edits written by the object edits that
/// follow this pass resolve to these objects.
fn apply_copy_fields_fixup(
    incremental: &mut lopdf::IncrementalDocument,
    pages: &crate::page_ops::PageModel,
) -> Result<(), CommandError> {
    use std::collections::BTreeMap;

    let mut copy_pages: Vec<u32> = pages
        .copies
        .keys()
        .copied()
        .filter(|physical| pages.order.contains(physical))
        .collect();
    if copy_pages.is_empty() {
        return Ok(());
    }
    copy_pages.sort_unstable();

    let mut next_id = incremental.new_document.max_id.saturating_add(1);
    let allocate = |next: &mut u32| -> Result<lopdf::ObjectId, CommandError> {
        let id = (*next, 0);
        *next = next
            .checked_add(1)
            .ok_or_else(|| lopdf_error("could not allocate a form object id", "object id space"))?;
        Ok(id)
    };

    let mut objects: BTreeMap<lopdf::ObjectId, lopdf::Dictionary> = BTreeMap::new();
    let mut page_annots: Vec<(lopdf::ObjectId, Vec<lopdf::Object>)> = Vec::new();
    let mut new_fields: Vec<lopdf::ObjectId> = Vec::new();

    {
        let previous = incremental.get_prev_documents();
        let new_document = &incremental.new_document;
        let physical_pages: Vec<lopdf::ObjectId> = previous.get_pages().values().copied().collect();

        for physical in &copy_pages {
            let Some(page_id) = physical_pages.get(*physical as usize).copied() else {
                continue;
            };
            let Some(annots) = current_annots(incremental, page_id) else {
                continue;
            };
            let mut rewritten = annots.clone();
            let mut changed = false;
            let mut parents: BTreeMap<lopdf::ObjectId, Vec<lopdf::ObjectId>> = BTreeMap::new();

            for (index, entry) in annots.iter().enumerate() {
                let (dictionary, object_id) = match entry {
                    lopdf::Object::Reference(id) => {
                        let Some(dictionary) = new_document
                            .get_dictionary(*id)
                            .ok()
                            .or_else(|| previous.get_dictionary(*id).ok())
                        else {
                            continue;
                        };
                        (dictionary, Some(*id))
                    }
                    lopdf::Object::Dictionary(dictionary) => (dictionary, None),
                    _ => continue,
                };
                if dictionary
                    .get(b"Subtype")
                    .and_then(lopdf::Object::as_name)
                    .ok()
                    != Some(b"Widget".as_slice())
                {
                    continue;
                }
                let parent_id = dictionary
                    .get(b"Parent")
                    .ok()
                    .and_then(|parent| parent.as_reference().ok());
                if parent_id.is_none() && !dictionary.has(b"FT") {
                    continue;
                }

                // `/Fields` and `/Kids` need object references, so an inline
                // annotation dictionary becomes an object of its own.
                let object_id = match object_id {
                    Some(id) => id,
                    None => {
                        let id = allocate(&mut next_id)?;
                        rewritten[index] = lopdf::Object::Reference(id);
                        changed = true;
                        objects.insert(id, dictionary.clone());
                        id
                    }
                };

                match parent_id {
                    Some(parent_id) => {
                        parents.entry(parent_id).or_default().push(object_id);
                    }
                    None => {
                        let Some(base) = dictionary
                            .get(b"T")
                            .and_then(lopdf::Object::as_str)
                            .ok()
                            .map(decode_pdf_text)
                        else {
                            continue;
                        };
                        let name = crate::page_ops::copied_field_name(&base, *physical);
                        let mut updated = objects
                            .remove(&object_id)
                            .unwrap_or_else(|| dictionary.clone());
                        updated.set(b"T".to_vec(), text_string_object(&name));
                        objects.insert(object_id, updated);
                        new_fields.push(object_id);
                    }
                }
            }

            if changed {
                page_annots.push((page_id, rewritten));
            }

            for (parent_id, widget_ids) in parents {
                let Some(parent) = new_document
                    .get_dictionary(parent_id)
                    .ok()
                    .or_else(|| previous.get_dictionary(parent_id).ok())
                else {
                    continue;
                };
                let shallow_base = parent
                    .get(b"T")
                    .and_then(lopdf::Object::as_str)
                    .ok()
                    .map(decode_pdf_text)
                    .or_else(|| {
                        widget_ids.iter().find_map(|id| {
                            dictionary_any(incremental, *id)
                                .and_then(|widget| widget.get(b"T").ok())
                                .and_then(|name| name.as_str().ok())
                                .map(decode_pdf_text)
                        })
                    })
                    .unwrap_or_default();
                // A nested field takes its user-visible name from the whole
                // `/T` chain (`person.name`). PDFium returns that dotted name,
                // so the duplicate must be registered under the same full name
                // or save-time value edits cannot find it.
                let nested = parent.has(b"Parent");
                let base = if nested {
                    let full = full_field_name(incremental, parent_id);
                    if full.is_empty() {
                        shallow_base
                    } else {
                        full
                    }
                } else {
                    shallow_base
                };
                let name = crate::page_ops::copied_field_name(&base, *physical);
                let new_parent_id = allocate(&mut next_id)?;
                let mut parent_copy = parent.clone();
                parent_copy.set(b"T".to_vec(), text_string_object(&name));
                parent_copy.set(
                    b"Kids".to_vec(),
                    lopdf::Object::Array(
                        widget_ids
                            .iter()
                            .map(|id| lopdf::Object::Reference(*id))
                            .collect(),
                    ),
                );
                if nested {
                    // Flattening keeps the copied field self-contained: a
                    // clone that still pointed at the source ancestor would be
                    // discoverable only through the source tree, and a shallow
                    // `/T` would not match the dotted name discovery reports.
                    copy_inherited_field_attributes(incremental, parent, &mut parent_copy);
                    parent_copy.remove(b"Parent");
                }
                objects.insert(new_parent_id, parent_copy);
                for widget_id in &widget_ids {
                    let mut widget = objects.remove(widget_id).unwrap_or_else(|| {
                        dictionary_any(incremental, *widget_id)
                            .cloned()
                            .unwrap_or_default()
                    });
                    widget.set(b"Parent".to_vec(), lopdf::Object::Reference(new_parent_id));
                    objects.insert(*widget_id, widget);
                }
                new_fields.push(new_parent_id);
            }
        }
    }

    new_fields.sort_unstable_by_key(|id| id.0);
    new_fields.dedup();
    if new_fields.is_empty() && objects.is_empty() && page_annots.is_empty() {
        return Ok(());
    }

    for (id, dictionary) in objects {
        if incremental.get_prev_documents().has_object(id) {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| lopdf_error("could not clone a copied form object", error))?;
        }
        incremental.new_document.set_object(id, dictionary);
    }
    for (page_id, annots) in page_annots {
        incremental
            .opt_clone_object_to_new_document(page_id)
            .map_err(|error| lopdf_error("could not clone a copied page's annotations", error))?;
        let page = incremental
            .new_document
            .get_dictionary_mut(page_id)
            .map_err(|error| lopdf_error("could not edit a copied page's annotations", error))?;
        page.set(b"Annots".to_vec(), lopdf::Object::Array(annots));
    }
    if !new_fields.is_empty() {
        append_acro_form_fields(incremental, &new_fields)?;
    }
    if next_id > incremental.new_document.max_id {
        incremental.new_document.max_id = next_id - 1;
    }
    Ok(())
}

/// Restores the destinations PDFium's page importer drops from copied links.
///
/// `FPDF_ImportPages` clones a page's link annotations but not the `/D`
/// destination of their `/GoTo` action (nor a direct `/Dest`), leaving a
/// visible link that does nothing. A copy has the same links as its source, so
/// the missing entries are taken from the source page. The chain of copies is
/// followed back to the original page because every intermediate copy already
/// lost its destination at import time.
///
/// Links are matched by rectangle first and by order second, so a link deleted
/// from the copy is not resurrected with another link's destination.
fn apply_copy_links_fixup(
    incremental: &mut lopdf::IncrementalDocument,
    pages: &crate::page_ops::PageModel,
) -> Result<(), CommandError> {
    use std::collections::BTreeMap;

    let mut copy_pages: Vec<(u32, u32)> = pages
        .copies
        .keys()
        .copied()
        .filter(|physical| pages.order.contains(physical))
        .map(|physical| (physical, original_source_page(pages, physical)))
        .collect();
    copy_pages.sort_unstable();
    if copy_pages.is_empty() {
        return Ok(());
    }

    let mut next_id = incremental.new_document.max_id.saturating_add(1);
    let mut objects: BTreeMap<lopdf::ObjectId, lopdf::Dictionary> = BTreeMap::new();
    let mut page_annots: Vec<(lopdf::ObjectId, Vec<lopdf::Object>)> = Vec::new();

    {
        let previous = incremental.get_prev_documents();
        let new_document = &incremental.new_document;
        let physical_pages: Vec<lopdf::ObjectId> = previous.get_pages().values().copied().collect();

        for (copy_physical, source_physical) in copy_pages {
            let Some(copy_page_id) = physical_pages.get(copy_physical as usize).copied() else {
                continue;
            };
            let Some(source_page_id) = physical_pages.get(source_physical as usize).copied() else {
                continue;
            };
            let Some(annots) = current_annots(incremental, copy_page_id) else {
                continue;
            };
            let source_links = page_link_annotations(previous, source_page_id);
            if source_links.is_empty() {
                continue;
            }

            let mut rewritten = annots.clone();
            let mut changed = false;
            let mut link_index = 0usize;
            for (index, entry) in annots.iter().enumerate() {
                let (dictionary, object_id) = match entry {
                    lopdf::Object::Reference(id) => {
                        let Some(dictionary) = new_document
                            .get_dictionary(*id)
                            .ok()
                            .or_else(|| previous.get_dictionary(*id).ok())
                        else {
                            continue;
                        };
                        (dictionary, Some(*id))
                    }
                    lopdf::Object::Dictionary(dictionary) => (dictionary, None),
                    _ => continue,
                };
                if dictionary
                    .get(b"Subtype")
                    .and_then(lopdf::Object::as_name)
                    .ok()
                    != Some(b"Link".as_slice())
                {
                    continue;
                }

                let source_link = source_links
                    .iter()
                    .find(|candidate| same_rect(candidate, dictionary))
                    .or_else(|| source_links.get(link_index));
                link_index += 1;
                let Some(source_link) = source_link else {
                    continue;
                };

                let source_action = source_link.get(b"A").ok();
                let source_action_has_destination = source_action
                    .and_then(|action| resolve_previous_object(previous, action))
                    .and_then(|action| action.as_dict().ok())
                    .map(|action| action.has(b"D"))
                    .unwrap_or(false);
                let copy_action = dictionary
                    .get(b"A")
                    .ok()
                    .and_then(|action| resolve_object_any(incremental, action))
                    .and_then(|action| action.as_dict().ok());
                let copy_action_has_destination =
                    copy_action.map(|action| action.has(b"D")).unwrap_or(false);
                let copy_action_is_goto = copy_action
                    .and_then(|action| action.get(b"S").ok())
                    .and_then(|kind| kind.as_name().ok())
                    .is_some_and(|kind| kind == b"GoTo".as_slice());
                let copy_has_destination = dictionary.has(b"Dest") || copy_action_has_destination;
                // A destination may already have been materialized from PDFX's
                // private page key by the annotation pass; do not add the
                // source's `/Dest` on top of that GoTo action. A URI action is
                // a link the user (or PDFX) chose independently and must never
                // be replaced by a page destination that happens to share the
                // source link's rectangle.
                let needs_action = source_action_has_destination
                    && !copy_has_destination
                    && (copy_action.is_none() || copy_action_is_goto);
                let source_destination = source_link.get(b"Dest").ok();
                let needs_destination =
                    source_destination.is_some() && !copy_has_destination && copy_action.is_none();
                if !needs_action && !needs_destination {
                    continue;
                }

                // `/Annots` entries may be inline; objectify so the updated
                // dictionary has a stable id to write into the revision.
                let object_id = match object_id {
                    Some(id) => id,
                    None => {
                        let id = (next_id, 0);
                        next_id = next_id.checked_add(1).ok_or_else(|| {
                            lopdf_error("could not allocate a link object id", "object id space")
                        })?;
                        rewritten[index] = lopdf::Object::Reference(id);
                        changed = true;
                        objects.insert(id, dictionary.clone());
                        id
                    }
                };
                let mut updated = objects
                    .remove(&object_id)
                    .unwrap_or_else(|| dictionary.clone());
                if needs_action {
                    if let Some(action) = source_action {
                        updated.set(b"A".to_vec(), action.clone());
                    }
                }
                if needs_destination {
                    if let Some(destination) = source_destination {
                        updated.set(b"Dest".to_vec(), destination.clone());
                    }
                }
                objects.insert(object_id, updated);
            }

            if changed {
                page_annots.push((copy_page_id, rewritten));
            }
        }
    }

    for (id, dictionary) in objects {
        if incremental.get_prev_documents().has_object(id) {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| lopdf_error("could not clone a copied link", error))?;
        }
        incremental.new_document.set_object(id, dictionary);
    }
    for (page_id, annots) in page_annots {
        incremental
            .opt_clone_object_to_new_document(page_id)
            .map_err(|error| lopdf_error("could not clone a copied page's links", error))?;
        let page = incremental
            .new_document
            .get_dictionary_mut(page_id)
            .map_err(|error| lopdf_error("could not edit a copied page's links", error))?;
        page.set(b"Annots".to_vec(), lopdf::Object::Array(annots));
    }
    if next_id > incremental.new_document.max_id {
        incremental.new_document.max_id = next_id - 1;
    }
    Ok(())
}

/// Follows the copy chain to the page a link annotation's destination lives
/// on. A copied page's own annotations may already lack the destination.
fn original_source_page(pages: &crate::page_ops::PageModel, physical: u32) -> u32 {
    let mut current = physical;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current) {
        match pages.copies.get(&current) {
            Some(copy) => current = copy.source,
            None => return current,
        }
    }
    physical
}

/// The Link annotations of a page in the previous revision.
fn page_link_annotations(
    document: &lopdf::Document,
    page_id: lopdf::ObjectId,
) -> Vec<&lopdf::Dictionary> {
    let Ok(page) = document.get_dictionary(page_id) else {
        return Vec::new();
    };
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    let array = match annots {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .ok()
            .and_then(|annots| annots.as_array().ok()),
        other => other.as_array().ok(),
    };
    let Some(array) = array else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|entry| match entry {
            lopdf::Object::Reference(id) => document.get_dictionary(*id).ok(),
            lopdf::Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        })
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .ok()
                == Some(b"Link".as_slice())
        })
        .collect()
}

/// Resolves an object in the previous revision only.
fn resolve_previous_object<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Object> {
    match object {
        lopdf::Object::Reference(id) => document.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Whether two annotation dictionaries have the same `/Rect`.
fn same_rect(left: &lopdf::Dictionary, right: &lopdf::Dictionary) -> bool {
    let number = |object: &lopdf::Object| match object {
        lopdf::Object::Integer(value) => Some(*value as f64),
        lopdf::Object::Real(value) => Some(*value as f64),
        _ => None,
    };
    let rect = |dictionary: &lopdf::Dictionary| {
        dictionary
            .get(b"Rect")
            .ok()
            .and_then(|rect| rect.as_array().ok())
            .map(|array| array.iter().filter_map(number).collect::<Vec<_>>())
    };
    match (rect(left), rect(right)) {
        (Some(left), Some(right)) => {
            left.len() == 4
                && right.len() == 4
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(left, right)| (left - right).abs() < 0.01)
        }
        _ => false,
    }
}

/// The page's current `/Annots` array, resolving a reference through the
/// appended revision first so earlier passes' edits are preserved.
fn current_annots(
    incremental: &lopdf::IncrementalDocument,
    page_id: lopdf::ObjectId,
) -> Option<Vec<lopdf::Object>> {
    let page = dictionary_any(incremental, page_id)?;
    let annots = resolve_object_any(incremental, page.get(b"Annots").ok()?)?;
    annots.as_array().ok().cloned()
}

/// Appends the copy-page fields to the output `/AcroForm /Fields`, keeping the
/// existing fields in place. The AcroForm may be a reference object or inline
/// in the catalog, and `/Fields` may itself be a reference.
fn append_acro_form_fields(
    incremental: &mut lopdf::IncrementalDocument,
    additions: &[lopdf::ObjectId],
) -> Result<(), CommandError> {
    let (root, acro_form) = {
        let previous = incremental.get_prev_documents();
        let root = previous
            .trailer
            .get(b"Root")
            .and_then(lopdf::Object::as_reference)
            .map_err(|error| lopdf_error("the saved document has no /Root reference", error))?;
        let acro_form = previous
            .catalog()
            .ok()
            .and_then(|catalog| catalog.get(b"AcroForm").ok().cloned());
        (root, acro_form)
    };

    match acro_form {
        Some(lopdf::Object::Reference(acro_form_id)) => {
            incremental
                .opt_clone_object_to_new_document(acro_form_id)
                .map_err(|error| lopdf_error("could not clone the AcroForm", error))?;
            append_fields_to(incremental, FieldsOwner::Reference(acro_form_id), additions)
        }
        Some(lopdf::Object::Dictionary(_)) => {
            incremental
                .opt_clone_object_to_new_document(root)
                .map_err(|error| lopdf_error("could not clone the catalog", error))?;
            append_fields_to(incremental, FieldsOwner::Inline(root), additions)
        }
        _ => {
            // A document with form widgets but no AcroForm: create one so the
            // copied widgets are discoverable by other readers.
            incremental
                .opt_clone_object_to_new_document(root)
                .map_err(|error| lopdf_error("could not clone the catalog", error))?;
            let acro_form_id = incremental
                .new_document
                .try_new_object_id()
                .map_err(|error| lopdf_error("could not allocate an AcroForm", error))?;
            let mut acro_form = lopdf::Dictionary::new();
            acro_form.set(b"Fields".to_vec(), fields_array(additions, &[]));
            incremental.new_document.set_object(acro_form_id, acro_form);
            let catalog = incremental
                .new_document
                .get_dictionary_mut(root)
                .map_err(|error| lopdf_error("could not edit the catalog", error))?;
            catalog.set(b"AcroForm".to_vec(), lopdf::Object::Reference(acro_form_id));
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum FieldsOwner {
    Reference(lopdf::ObjectId),
    Inline(lopdf::ObjectId),
}

fn fields_owner_dictionary_mut(
    incremental: &mut lopdf::IncrementalDocument,
    owner: FieldsOwner,
) -> Result<&mut lopdf::Dictionary, CommandError> {
    match owner {
        FieldsOwner::Reference(id) => incremental
            .new_document
            .get_dictionary_mut(id)
            .map_err(|error| lopdf_error("could not edit the AcroForm", error)),
        FieldsOwner::Inline(root) => incremental
            .new_document
            .get_dictionary_mut(root)
            .map_err(|error| lopdf_error("could not edit the catalog", error))?
            .get_mut(b"AcroForm")
            .map_err(|error| lopdf_error("could not read the AcroForm", error))?
            .as_dict_mut()
            .map_err(|error| lopdf_error("the AcroForm is not a dictionary", error)),
    }
}

fn append_fields_to(
    incremental: &mut lopdf::IncrementalDocument,
    owner: FieldsOwner,
    additions: &[lopdf::ObjectId],
) -> Result<(), CommandError> {
    let current = {
        let dictionary = match owner {
            FieldsOwner::Reference(id) => incremental
                .new_document
                .get_dictionary(id)
                .or_else(|_| incremental.get_prev_documents().get_dictionary(id))
                .map_err(|error| lopdf_error("could not read the AcroForm", error))?,
            FieldsOwner::Inline(root) => incremental
                .new_document
                .get_dictionary(root)
                .or_else(|_| incremental.get_prev_documents().get_dictionary(root))
                .map_err(|error| lopdf_error("could not read the catalog", error))?
                .get(b"AcroForm")
                .and_then(lopdf::Object::as_dict)
                .map_err(|error| lopdf_error("could not read the AcroForm", error))?,
        };
        dictionary.get(b"Fields").ok().cloned()
    };

    let existing: Vec<lopdf::Object> = match current {
        Some(lopdf::Object::Reference(fields_id)) => {
            incremental
                .opt_clone_object_to_new_document(fields_id)
                .map_err(|error| lopdf_error("could not clone the form field list", error))?;
            incremental
                .new_document
                .get_object(fields_id)
                .and_then(lopdf::Object::as_array)
                .map_err(|error| lopdf_error("the form field list is not an array", error))?
                .clone()
        }
        Some(lopdf::Object::Array(fields)) => fields,
        Some(_) => {
            return Err(lopdf_error(
                "the form field list is not an array",
                "AcroForm /Fields",
            ))
        }
        None => Vec::new(),
    };

    let array = fields_array(additions, &existing);
    let dictionary = fields_owner_dictionary_mut(incremental, owner)?;
    dictionary.set(b"Fields".to_vec(), array);
    Ok(())
}

/// Builds a `/Fields` array from the existing entries plus the additions,
/// keeping every reference at most once.
fn fields_array(additions: &[lopdf::ObjectId], existing: &[lopdf::Object]) -> lopdf::Object {
    let mut seen = std::collections::HashSet::new();
    let mut objects = Vec::with_capacity(existing.len() + additions.len());
    for object in existing {
        if let Ok(id) = object.as_reference() {
            if !seen.insert(id) {
                continue;
            }
        }
        objects.push(object.clone());
    }
    for id in additions {
        if seen.insert(*id) {
            objects.push(lopdf::Object::Reference(*id));
        }
    }
    lopdf::Object::Array(objects)
}

/// Resolves a page attribute through the `/Parent` chain, matching the PDF
/// inheritance rules for `Resources`, `MediaBox`, `CropBox` and `Rotate`.
/// Flattening the tree to a single level requires materializing these onto the
/// page dictionary.
fn inherited_page_attribute(
    document: &lopdf::Document,
    page: &lopdf::Dictionary,
    key: &[u8],
) -> Option<lopdf::Object> {
    let mut current = page;
    for _ in 0..32 {
        if let Ok(value) = current.get(key) {
            return Some(value.clone());
        }
        let parent = current.get(b"Parent").ok()?.as_reference().ok()?;
        current = document.get_dictionary(parent).ok()?;
    }
    None
}

/// Writes the arrangement into the appended revision: included pages become
/// direct children of the root `/Pages` node in logical order, each page keeps
/// inherited attributes, and blank pages get explicit page boxes/resources.
fn apply_page_tree_edit(
    incremental: &mut lopdf::IncrementalDocument,
    edit: &PageTreeEdit,
) -> Result<(), CommandError> {
    // Collect inherited attributes before the mutable new-document edits
    // (the previous document is borrowed immutably).
    type InheritedAttributes = (lopdf::ObjectId, Vec<(Vec<u8>, lopdf::Object)>);
    let inherited: Vec<InheritedAttributes> = {
        let previous = incremental.get_prev_documents();
        edit.kids
            .iter()
            .map(|page_id| {
                let mut values = Vec::new();
                if let Ok(page) = previous.get_dictionary(*page_id) {
                    for key in [&b"MediaBox"[..], b"CropBox", b"Rotate", b"Resources"] {
                        if page.has(key) {
                            continue;
                        }
                        if let Some(value) = inherited_page_attribute(previous, page, key) {
                            values.push((key.to_vec(), value));
                        }
                    }
                }
                (*page_id, values)
            })
            .collect()
    };

    let blank_sizes: std::collections::HashMap<lopdf::ObjectId, (f32, f32)> = edit
        .blanks
        .iter()
        .map(|(id, size)| (*id, (size.width, size.height)))
        .collect();

    for (page_id, values) in &inherited {
        incremental
            .opt_clone_object_to_new_document(*page_id)
            .map_err(|error| lopdf_error("could not clone a page for the page tree", error))?;
        let page = incremental
            .new_document
            .get_dictionary_mut(*page_id)
            .map_err(|error| lopdf_error("could not edit a page for the page tree", error))?;
        page.set(b"Parent".to_vec(), lopdf::Object::Reference(edit.pages_id));
        for (key, value) in values {
            page.set(key.clone(), value.clone());
        }
        if let Some((width, height)) = blank_sizes.get(page_id) {
            let media_box = vec![
                lopdf::Object::Integer(0),
                lopdf::Object::Integer(0),
                lopdf::Object::Real(*width),
                lopdf::Object::Real(*height),
            ];
            page.set(
                b"MediaBox".to_vec(),
                lopdf::Object::Array(media_box.clone()),
            );
            page.set(b"CropBox".to_vec(), lopdf::Object::Array(media_box));
            if !page.has(b"Resources") {
                page.set(
                    b"Resources".to_vec(),
                    lopdf::Object::Dictionary(lopdf::Dictionary::new()),
                );
            }
        }
    }

    incremental
        .opt_clone_object_to_new_document(edit.pages_id)
        .map_err(|error| lopdf_error("could not clone the page tree root", error))?;
    let pages = incremental
        .new_document
        .get_dictionary_mut(edit.pages_id)
        .map_err(|error| lopdf_error("could not edit the page tree root", error))?;
    pages.set(
        b"Kids".to_vec(),
        lopdf::Object::Array(
            edit.kids
                .iter()
                .map(|id| lopdf::Object::Reference(*id))
                .collect(),
        ),
    );
    pages.set(
        b"Count".to_vec(),
        lopdf::Object::Integer(edit.kids.len() as i64),
    );
    Ok(())
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

/// A standard annotation entry PDFX keeps in a private key while the document
/// lives in Pdfium, because Pdfium's public API cannot write it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AnnotationDictionaryEdit {
    /// `/Q` and `/IT` for a PDFX free text annotation.
    FreeText { q: Option<i64>, it: Vec<u8> },
    /// The reserved placeholder URI of a PDFX page link, replaced by a real
    /// in-document GoTo destination naming the target page object.
    PageLink {
        target_page: Option<lopdf::ObjectId>,
    },
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
    edit: AnnotationDictionaryEdit,
}

/// Builds `<< /S /GoTo /D [page /XYZ null null null] >>`.
///
/// The XYZ destination with null coordinates asks the reader to keep its
/// current position and zoom, which is what users expect from an internal
/// link.
fn page_goto_action(page_id: lopdf::ObjectId) -> lopdf::Object {
    let mut action = lopdf::Dictionary::new();
    action.set(b"S".to_vec(), lopdf::Object::Name(b"GoTo".to_vec()));
    action.set(
        b"D".to_vec(),
        lopdf::Object::Array(vec![
            lopdf::Object::Reference(page_id),
            lopdf::Object::Name(b"XYZ".to_vec()),
            lopdf::Object::Null,
            lopdf::Object::Null,
            lopdf::Object::Null,
        ]),
    );
    lopdf::Object::Dictionary(action)
}

/// Applies one deferred dictionary edit to an annotation dictionary.
fn apply_dictionary_edit(dictionary: &mut lopdf::Dictionary, edit: &AnnotationDictionaryEdit) {
    match edit {
        AnnotationDictionaryEdit::FreeText { q, it } => {
            if let Some(q) = q {
                dictionary.set(b"Q".to_vec(), lopdf::Object::Integer(*q));
            }
            dictionary.set(b"IT".to_vec(), lopdf::Object::Name(it.clone()));
        }
        AnnotationDictionaryEdit::PageLink { target_page } => {
            // A missing page object would only happen if the document lost
            // pages while open, which PDFX cannot do; leave the annotation
            // untouched rather than pointing it at the wrong page.
            if let Some(page_id) = target_page {
                dictionary.set(b"A".to_vec(), page_goto_action(*page_id));
                dictionary.remove(b"Dest");
                // The private page index is meaningful only inside the live
                // Pdfium session, where page reordering can differ from the
                // saved tree. Removing it makes a reopened output resolve the
                // real destination instead of a stale session index.
                dictionary.remove(crate::links::LINK_TARGET_KEY.as_bytes());
            }
        }
    }
}

/// Derives the standard entries PDFX stores in private keys for every
/// annotation in a PDFium revision.
///
/// Pdfium writes annotation dictionaries inline in `/Annots`, so inline entries
/// are rewritten through their page object while referenced entries are edited
/// directly. Foreign annotations have no PDFX private keys and are left
/// untouched.
fn annotation_dictionary_edits(
    document: &lopdf::Document,
) -> (Vec<PageAnnotsEdit>, Vec<ReferencedAnnotationEdit>) {
    fn edit_for(
        dictionary: &lopdf::Dictionary,
        document: &lopdf::Document,
    ) -> Option<AnnotationDictionaryEdit> {
        let subtype = dictionary
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .ok()?;
        if subtype == b"FreeText".as_slice() {
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
            return Some(AnnotationDictionaryEdit::FreeText { q, it });
        }
        if subtype == b"Link".as_slice() {
            let page = dictionary
                .get(crate::links::LINK_TARGET_KEY.as_bytes())
                .ok()
                .and_then(|object| object.as_str().ok())
                .map(decode_pdf_text)
                .and_then(|value| value.strip_prefix("page:")?.parse::<u32>().ok())?;
            // lopdf page numbers are one-based while PDFX page indices are
            // zero-based.
            let target_page = document.get_pages().get(&(page + 1)).copied();
            return Some(AnnotationDictionaryEdit::PageLink { target_page });
        }
        None
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
                lopdf::Object::Dictionary(dictionary) => match edit_for(dictionary, document) {
                    Some(edit) => {
                        let mut updated = dictionary.clone();
                        apply_dictionary_edit(&mut updated, &edit);
                        rewritten.push(lopdf::Object::Dictionary(updated));
                        changed = true;
                    }
                    None => rewritten.push(object.clone()),
                },
                lopdf::Object::Reference(id) => {
                    if let Ok(dictionary) = document.get_dictionary(*id) {
                        if let Some(edit) = edit_for(dictionary, document) {
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
        ObjectEdit::SetTextValue { field_name, text } => {
            apply_text_value(incremental, field_name, text)
        }
        ObjectEdit::SetCheckboxValue {
            field_name,
            checked,
        } => apply_checkbox_value(incremental, field_name, *checked),
        ObjectEdit::CreateFormFields { fields } => {
            crate::form_create::apply_created_fields(incremental, fields)
        }
    }
}

/// Decodes a PDF text string: UTF-16BE when it carries the BOM, otherwise
/// PDFDocEncoding (PDF 32000-1 Annex D.2).
///
/// PDFium reports attachment, field and bookmark names decoded the same way, so
/// a name read from a lopdf object (a name-tree key, for example) compares equal
/// to the session name even when the file stored it as single-byte
/// PDFDocEncoding rather than UTF-16BE.
pub(crate) fn decode_pdf_text(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|byte| pdfdoc_character(*byte)).collect()
    }
}

/// One PDFDocEncoding code point. Undefined codes decode to U+FFFD so a name
/// still has the same length and cannot silently merge with another.
fn pdfdoc_character(byte: u8) -> char {
    match byte {
        0x09 => '\t',
        0x0A => '\n',
        0x0D => '\r',
        0x18 => '\u{02D8}',
        0x19 => '\u{02C7}',
        0x1A => '\u{02C6}',
        0x1B => '\u{02D9}',
        0x1C => '\u{02DD}',
        0x1D => '\u{02DB}',
        0x1E => '\u{02DA}',
        0x1F => '\u{02DC}',
        0x20..=0x7E => byte as char,
        0x80 => '\u{2022}',
        0x81 => '\u{2020}',
        0x82 => '\u{2021}',
        0x83 => '\u{2026}',
        0x84 => '\u{2014}',
        0x85 => '\u{2013}',
        0x86 => '\u{0192}',
        0x87 => '\u{2044}',
        0x88 => '\u{2039}',
        0x89 => '\u{203A}',
        0x8A => '\u{2212}',
        0x8B => '\u{2030}',
        0x8C => '\u{201E}',
        0x8D => '\u{201C}',
        0x8E => '\u{201D}',
        0x8F => '\u{2018}',
        0x90 => '\u{2019}',
        0x91 => '\u{201A}',
        0x92 => '\u{2122}',
        0x93 => '\u{FB01}',
        0x94 => '\u{FB02}',
        0x95 => '\u{0141}',
        0x96 => '\u{0152}',
        0x97 => '\u{0160}',
        0x98 => '\u{0178}',
        0x99 => '\u{017D}',
        0x9A => '\u{0131}',
        0x9B => '\u{0142}',
        0x9C => '\u{0153}',
        0x9D => '\u{0161}',
        0x9E => '\u{017E}',
        0xA0 => '\u{20AC}',
        0xA1..=0xFF => char::from_u32(u32::from(byte)).expect("Latin-1 is valid Unicode"),
        _ => '\u{FFFD}',
    }
}

pub(crate) fn text_string_object(value: &str) -> lopdf::Object {
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

/// Resolves an object that may live in the appended revision or in the
/// document it appends to. Copy-page fields are written into the new
/// revision, so every lookup in the save pipeline must consider both.
fn resolve_object_any<'a>(
    incremental: &'a lopdf::IncrementalDocument,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Object> {
    match object {
        lopdf::Object::Reference(id) => incremental
            .new_document
            .get_object(*id)
            .ok()
            .or_else(|| incremental.get_prev_documents().get_object(*id).ok()),
        other => Some(other),
    }
}

fn dictionary_any(
    incremental: &lopdf::IncrementalDocument,
    id: lopdf::ObjectId,
) -> Option<&lopdf::Dictionary> {
    incremental
        .new_document
        .get_dictionary(id)
        .ok()
        .or_else(|| incremental.get_prev_documents().get_dictionary(id).ok())
}

/// Field entries a terminal field inherits from its `/Parent` chain
/// (PDF 32000-1 Table 220). Flattening a copied nested field copies them down
/// so the new root field stays usable without the source ancestor objects.
const INHERITABLE_FIELD_KEYS: [&[u8]; 11] = [
    b"FT", b"Ff", b"V", b"DV", b"AA", b"DA", b"Q", b"DS", b"RV", b"Opt", b"MaxLen",
];

/// Copies inheritable field entries from the ancestors of `parent` into the
/// flattened copy, nearest ancestor first. Existing entries win.
fn copy_inherited_field_attributes(
    incremental: &lopdf::IncrementalDocument,
    parent: &lopdf::Dictionary,
    copy: &mut lopdf::Dictionary,
) {
    let mut next = parent
        .get(b"Parent")
        .ok()
        .and_then(|value| value.as_reference().ok());
    for _ in 0..64 {
        let Some(id) = next else { break };
        let Some(ancestor) = dictionary_any(incremental, id) else {
            break;
        };
        for key in INHERITABLE_FIELD_KEYS {
            if copy.has(key) {
                continue;
            }
            if let Ok(value) = ancestor.get(key) {
                copy.set(key.to_vec(), value.clone());
            }
        }
        next = ancestor
            .get(b"Parent")
            .ok()
            .and_then(|value| value.as_reference().ok());
    }
}

/// The full dotted field name of `id`, built from `/T` entries up the
/// `/Parent` chain (`person.name` for nested fields).
fn full_field_name(incremental: &lopdf::IncrementalDocument, id: lopdf::ObjectId) -> String {
    let mut parts = Vec::new();
    let mut next = Some(id);
    for _ in 0..64 {
        let Some(id) = next else { break };
        let Some(dictionary) = dictionary_any(incremental, id) else {
            break;
        };
        if let Ok(title) = dictionary.get(b"T").and_then(lopdf::Object::as_str) {
            parts.push(decode_pdf_text(title));
        }
        next = dictionary
            .get(b"Parent")
            .ok()
            .and_then(|value| value.as_reference().ok());
    }
    parts.reverse();
    parts.join(".")
}

/// Finds a field by name in the appended revision's `/AcroForm`, where the
/// copy-page fixup registers renamed widgets and cloned parent fields.
fn find_form_field_id_any(
    incremental: &lopdf::IncrementalDocument,
    name: &str,
) -> Option<lopdf::ObjectId> {
    if let Some(id) = find_form_field_id_in_new(incremental, name) {
        return Some(id);
    }
    find_form_field_id(incremental.get_prev_documents(), name)
}

fn find_form_field_id_in_new(
    incremental: &lopdf::IncrementalDocument,
    name: &str,
) -> Option<lopdf::ObjectId> {
    let previous = incremental.get_prev_documents();
    let root = previous
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .ok()?;
    let acro_form = previous.catalog().ok()?.get(b"AcroForm").ok()?.clone();
    let acro_form = match acro_form {
        lopdf::Object::Reference(id) => incremental.new_document.get_dictionary(id).ok()?,
        lopdf::Object::Dictionary(_) => incremental
            .new_document
            .get_dictionary(root)
            .ok()?
            .get(b"AcroForm")
            .ok()?
            .as_dict()
            .ok()?,
        _ => return None,
    };
    let fields = resolve_object_any(incremental, acro_form.get(b"Fields").ok()?)?;
    fields.as_array().ok()?.iter().find_map(|field| {
        let id = field.as_reference().ok()?;
        let dictionary = dictionary_any(incremental, id)?;
        let field_name = dictionary
            .get(b"T")
            .and_then(lopdf::Object::as_str)
            .ok()
            .map(decode_pdf_text)?;
        (field_name == name).then_some(id)
    })
}

/// Returns the non-Off state name from a widget's normal appearance
/// dictionary, resolving objects across the previous revision and the
/// appended one.
fn widget_on_state_any(
    incremental: &lopdf::IncrementalDocument,
    widget: &lopdf::Dictionary,
) -> Option<Vec<u8>> {
    let appearance = resolve_object_any(incremental, widget.get(b"AP").ok()?)?;
    let normal = resolve_object_any(incremental, appearance.as_dict().ok()?.get(b"N").ok()?)?;
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
        let field_id = find_form_field_id_any(incremental, field_name).ok_or_else(|| {
            lopdf_error(
                "could not find a radio field in the saved document",
                field_name,
            )
        })?;
        let field = dictionary_any(incremental, field_id)
            .ok_or_else(|| lopdf_error("could not read the radio field dictionary", field_name))?;
        let widget_ids: Vec<lopdf::ObjectId> = match field
            .get(b"Kids")
            .ok()
            .and_then(|kids| resolve_object_any(incremental, kids))
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
                dictionary_any(incremental, *id)
                    .and_then(|widget| widget_on_state_any(incremental, widget))
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
        let field_id = find_form_field_id_any(incremental, field_name).ok_or_else(|| {
            lopdf_error(
                "could not find a choice field in the saved document",
                field_name,
            )
        })?;
        let field = dictionary_any(incremental, field_id)
            .ok_or_else(|| lopdf_error("could not read the choice field dictionary", field_name))?;
        let index = field
            .get(b"Opt")
            .ok()
            .and_then(|options| resolve_object_any(incremental, options))
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

fn apply_text_value(
    incremental: &mut lopdf::IncrementalDocument,
    field_name: &str,
    text: &str,
) -> Result<(), CommandError> {
    let field_id = find_form_field_id_any(incremental, field_name).ok_or_else(|| {
        lopdf_error(
            "could not find a text field in the saved document",
            field_name,
        )
    })?;
    incremental
        .opt_clone_object_to_new_document(field_id)
        .map_err(|error| lopdf_error("could not clone the text field", error))?;
    let field = incremental
        .new_document
        .get_dictionary_mut(field_id)
        .map_err(|error| lopdf_error("could not edit the text field", error))?;
    field.set(b"V".to_vec(), text_string_object(text));
    Ok(())
}

fn apply_checkbox_value(
    incremental: &mut lopdf::IncrementalDocument,
    field_name: &str,
    checked: bool,
) -> Result<(), CommandError> {
    let (field_id, widget_ids, on_state) = {
        let field_id = find_form_field_id_any(incremental, field_name).ok_or_else(|| {
            lopdf_error(
                "could not find a checkbox field in the saved document",
                field_name,
            )
        })?;
        let field = dictionary_any(incremental, field_id).ok_or_else(|| {
            lopdf_error("could not read the checkbox field dictionary", field_name)
        })?;
        let widget_ids: Vec<lopdf::ObjectId> = match field
            .get(b"Kids")
            .ok()
            .and_then(|kids| resolve_object_any(incremental, kids))
            .and_then(|kids| kids.as_array().ok())
        {
            Some(kids) => kids
                .iter()
                .filter_map(|kid| kid.as_reference().ok())
                .collect(),
            None => vec![field_id],
        };
        let on_state = widget_ids
            .iter()
            .find_map(|id| {
                dictionary_any(incremental, *id)
                    .and_then(|widget| widget_on_state_any(incremental, widget))
            })
            .unwrap_or_else(|| b"Yes".to_vec());
        (field_id, widget_ids, on_state)
    };

    incremental
        .opt_clone_object_to_new_document(field_id)
        .map_err(|error| lopdf_error("could not clone the checkbox field", error))?;
    for id in &widget_ids {
        incremental
            .opt_clone_object_to_new_document(*id)
            .map_err(|error| lopdf_error("could not clone a checkbox widget", error))?;
    }
    let appearance = if checked {
        on_state.clone()
    } else {
        b"Off".to_vec()
    };
    let field = incremental
        .new_document
        .get_dictionary_mut(field_id)
        .map_err(|error| lopdf_error("could not edit the checkbox field", error))?;
    field.set(b"V".to_vec(), lopdf::Object::Name(appearance.clone()));
    for id in &widget_ids {
        let widget = incremental
            .new_document
            .get_dictionary_mut(*id)
            .map_err(|error| lopdf_error("could not edit a checkbox widget", error))?;
        widget.set(b"AS".to_vec(), lopdf::Object::Name(appearance.clone()));
    }
    Ok(())
}

pub(crate) fn lopdf_error(context: &str, error: impl std::fmt::Display) -> CommandError {
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
