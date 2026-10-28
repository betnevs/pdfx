//! Logical page arrangement and page operations.
//!
//! PDFium has no supported way to reorder the pages of a live document, so the
//! session keeps a logical arrangement on top of the physical page tree:
//! `order` maps each visible (logical) page slot to its PDFium page index.
//! Rotation, deletion, reordering and blank-page insertion all update this
//! arrangement and one unified undo action, so the viewer can refresh
//! immediately while the saved file is rewritten from the arrangement in the
//! save pipeline. Physical pages are never removed from the PDFium document:
//! deleting a page only hides it, which is what lets undo restore the page
//! with all of its content, annotations and form widgets intact.

use serde::{Deserialize, Serialize};

use crate::commands::{CommandError, CommandErrorCode, OpenDocument};
use crate::text_edit::{EditAction, EditHistoryView, EditStepView};

/// Paper size of a page inserted by the user, in PDF points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlankPageSize {
    pub width: f32,
    pub height: f32,
}

/// One physical page created by an insertion. The page exists in the PDFium
/// document from the moment it is inserted, so it renders immediately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlankPage {
    pub(crate) size: BlankPageSize,
}

/// One physical page created by `copy_page`.
///
/// The page is a real PDFium page cloned from the source page (and from the
/// current, fully edited document state) by `FPDF_ImportPages`, so it renders
/// immediately and its content, links and annotations are independent objects.
/// `source` is the physical page it was copied from. `inherited` holds the
/// source page's form values at the moment of the copy, keyed by the raw
/// Pdfium field name; discovery and the save pipeline use it so a copy starts
/// with the same field values as its source even when those values only live
/// in the session overlay.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CopiedPage {
    pub(crate) source: u32,
    pub(crate) inherited: crate::form::FormOverlay,
}

/// The visible page arrangement.
///
/// `order[logical]` is the physical PDFium page index shown at that slot.
/// Pages absent from `order` are deleted (hidden) and are excluded from the
/// saved page tree. `blanks` remembers which physical pages were inserted so
/// the save pipeline can give them explicit page boxes. `copies` records the
/// physical pages created by page duplication; the save pipeline registers
/// their form fields under names unique within the copied AcroForm.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PageModel {
    pub(crate) order: Vec<u32>,
    pub(crate) blanks: std::collections::HashMap<u32, BlankPage>,
    pub(crate) copies: std::collections::HashMap<u32, CopiedPage>,
}

/// Field name used by the copy of a form field. The physical page index makes
/// it unique among copies and stable across saves, and form discovery,
/// fill routing and the save pipeline all derive the same name from it.
pub(crate) fn copied_field_name(base: &str, physical_page: u32) -> String {
    format!("{base}__copy_p{physical_page}")
}

impl PageModel {
    /// The arrangement of a freshly opened document: physical order, no
    /// blanks.
    pub(crate) fn new(page_count: u32) -> Self {
        Self {
            order: (0..page_count).collect(),
            blanks: std::collections::HashMap::new(),
            copies: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn len(&self) -> u32 {
        self.order.len() as u32
    }

    /// Resolves a logical page slot to its PDFium page index.
    pub(crate) fn physical(&self, logical: u32) -> Result<u32, CommandError> {
        self.order.get(logical as usize).copied().ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::PageOutOfRange,
                format!("page {logical} is outside 0..{}", self.order.len()),
            )
        })
    }

    /// Resolves a PDFium page index to its logical slot, or `None` when the
    /// page is deleted.
    pub(crate) fn logical(&self, physical: u32) -> Option<u32> {
        self.order
            .iter()
            .position(|candidate| *candidate == physical)
            .map(|index| index as u32)
    }

    /// Clamps a preferred logical index to this arrangement's page range.
    fn clamp(&self, logical: u32) -> u32 {
        self.order.len().saturating_sub(1).min(logical as usize) as u32
    }
}

impl OpenDocument {
    /// Number of visible pages (the logical page count shown to the user).
    pub(crate) fn logical_page_count(&self) -> u32 {
        self.pages.len()
    }

    /// Maps a logical page index from the command boundary to the PDFium page
    /// index the engines work with.
    pub(crate) fn physical_page(&self, logical: u32) -> Result<u32, CommandError> {
        self.pages.physical(logical)
    }

    /// Maps a physical page index back to the logical slot. `None` means the
    /// page was deleted.
    pub(crate) fn logical_page(&self, physical: u32) -> Option<u32> {
        self.pages.logical(physical)
    }
}

/// Rotation restored by a page action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RotationChange {
    pub(crate) physical_page: u32,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    pub(crate) rotation: u16,
}

/// One committed page operation in the unified edit history.
#[derive(Debug, Clone)]
pub(crate) struct PageEditAction {
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: PageModel,
    pub(crate) after: PageModel,
    /// Set for a rotation action; `None` for structural operations.
    pub(crate) before_rotation: Option<RotationChange>,
    pub(crate) after_rotation: Option<RotationChange>,
    /// Preferred page selection after this side is applied, clamped to the
    /// resulting page count.
    pub(crate) before_page_index: u32,
    pub(crate) after_page_index: u32,
}

/// The outcome of a page operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageOperationView {
    /// Visible page count after the operation.
    pub page_count: u32,
    /// Logical page the viewer should select next, clamped to range.
    pub page_index: u32,
    pub history: EditHistoryView,
}

fn invalid_argument(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

/// Page operations are content edits: the Permission Restrictions must grant
/// document content modification. Checked before any state changes so a
/// read-only document leaves the history untouched.
pub(crate) fn ensure_page_editable(entry: &OpenDocument) -> Result<(), CommandError> {
    crate::text_edit::ensure_content_editable(&entry.document)
}

fn rotation_from_degrees(degrees: u16) -> pdfium_render::prelude::PdfPageRenderRotation {
    use pdfium_render::prelude::PdfPageRenderRotation;
    match degrees % 360 {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

fn page_rotation(entry: &OpenDocument, physical_page: u32) -> Result<u16, CommandError> {
    let page = crate::commands::load_page(&entry.document, physical_page)?;
    page.rotation()
        .map(|rotation| rotation.as_degrees() as u16)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {physical_page} rotation: {error:?}"),
            )
        })
}

fn set_page_rotation(
    entry: &mut OpenDocument,
    physical_page: u32,
    rotation: u16,
) -> Result<(), CommandError> {
    let mut page = crate::commands::load_page(&entry.document, physical_page)?;
    page.set_rotation(rotation_from_degrees(rotation));
    Ok(())
}

/// Applies one side of a page action: the arrangement and, for a rotation,
/// the physical page rotation.
fn apply_page_model(
    entry: &mut OpenDocument,
    model: &PageModel,
    rotation: Option<RotationChange>,
) -> Result<(), CommandError> {
    if let Some(change) = rotation {
        set_page_rotation(entry, change.physical_page, change.rotation)?;
    }
    entry.pages = model.clone();
    Ok(())
}

/// Records a page operation as one undo action and returns the operation view.
#[allow(clippy::too_many_arguments)]
fn push_page_action(
    entry: &mut OpenDocument,
    before: PageModel,
    after: PageModel,
    before_rotation: Option<RotationChange>,
    after_rotation: Option<RotationChange>,
    before_page_index: u32,
    after_page_index: u32,
) -> PageOperationView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry.edit.undo_stack.push(EditAction::Page(PageEditAction {
        before_state,
        after_state,
        before,
        after: after.clone(),
        before_rotation,
        after_rotation,
        before_page_index,
        after_page_index,
    }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.pages = after;

    PageOperationView {
        page_count: entry.pages.len(),
        page_index: entry.pages.clamp(after_page_index),
        history: entry.edit.history_view(),
    }
}

pub(crate) fn undo_page(
    entry: &mut OpenDocument,
    action: PageEditAction,
) -> Result<EditStepView, CommandError> {
    apply_page_model(entry, &action.before, action.before_rotation)?;
    let page_index = entry.pages.clamp(action.before_page_index);
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Page(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: true,
        history: entry.edit.history_view(),
    })
}

pub(crate) fn redo_page(
    entry: &mut OpenDocument,
    action: PageEditAction,
) -> Result<EditStepView, CommandError> {
    apply_page_model(entry, &action.after, action.after_rotation)?;
    let page_index = entry.pages.clamp(action.after_page_index);
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Page(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: true,
        history: entry.edit.history_view(),
    })
}

/// The `delete_page` command: hides one page from the logical arrangement as
/// one undoable action.
///
/// The physical PDFium page stays in the document, so undo restores the page
/// with its content, annotations and form widgets untouched.
#[tauri::command]
pub fn delete_page(
    document_id: String,
    page_index: u32,
) -> Result<PageOperationView, CommandError> {
    crate::commands::with_open_document(&document_id, |entry| {
        ensure_page_editable(entry)?;
        let before = entry.pages.clone();
        before.physical(page_index)?;
        if before.order.len() <= 1 {
            return Err(invalid_argument("文档至少需要保留一页，不能删除最后一页。"));
        }
        let mut after = before.clone();
        after.order.remove(page_index as usize);
        let after_page_index = after.clamp(page_index);
        Ok(push_page_action(
            entry,
            before,
            after,
            None,
            None,
            page_index,
            after_page_index,
        ))
    })
}

/// The `rotate_page` command: adds `delta_degrees` (90, 180 or 270) clockwise
/// to one page's rotation as one undoable action.
#[tauri::command]
pub fn rotate_page(
    document_id: String,
    page_index: u32,
    delta_degrees: u16,
) -> Result<PageOperationView, CommandError> {
    if !matches!(delta_degrees, 90 | 180 | 270) {
        return Err(invalid_argument(format!(
            "rotation must be 90, 180 or 270 degrees, got {delta_degrees}"
        )));
    }
    crate::commands::with_open_document(&document_id, |entry| {
        ensure_page_editable(entry)?;
        let physical_page = entry.physical_page(page_index)?;
        let before_rotation = page_rotation(entry, physical_page)?;
        let after_rotation = (before_rotation + delta_degrees) % 360;
        set_page_rotation(entry, physical_page, after_rotation)?;
        let model = entry.pages.clone();
        Ok(push_page_action(
            entry,
            model.clone(),
            model,
            Some(RotationChange {
                physical_page,
                rotation: before_rotation,
            }),
            Some(RotationChange {
                physical_page,
                rotation: after_rotation,
            }),
            page_index,
            page_index,
        ))
    })
}

/// The `move_page` command: moves one page to another logical slot as one
/// undoable action.
#[tauri::command]
pub fn move_page(
    document_id: String,
    from_index: u32,
    to_index: u32,
) -> Result<PageOperationView, CommandError> {
    crate::commands::with_open_document(&document_id, |entry| {
        ensure_page_editable(entry)?;
        let before = entry.pages.clone();
        let physical_page = before.physical(from_index)?;
        before.physical(to_index)?;
        if from_index == to_index {
            return Ok(PageOperationView {
                page_count: before.len(),
                page_index: to_index,
                history: entry.edit.history_view(),
            });
        }
        let mut after = before.clone();
        after.order.remove(from_index as usize);
        after.order.insert(to_index as usize, physical_page);
        Ok(push_page_action(
            entry, before, after, None, None, from_index, to_index,
        ))
    })
}

/// The `insert_blank_page` command: creates an empty page with the requested
/// paper size at a logical slot as one undoable action.
///
/// The physical page is appended to the PDFium document so existing physical
/// indices stay valid, and the logical arrangement places it at `page_index`.
/// The save pipeline gives it explicit boxes and resources in the page tree.
#[tauri::command]
pub fn insert_blank_page(
    document_id: String,
    page_index: u32,
    size: BlankPageSize,
) -> Result<PageOperationView, CommandError> {
    // PDF user-space coordinates are limited to 14,400 units per side; a
    // non-positive or unrepresentable size would produce an unrenderable page.
    if !size.width.is_finite()
        || !size.height.is_finite()
        || size.width <= 0.0
        || size.height <= 0.0
        || size.width > 14_400.0
        || size.height > 14_400.0
    {
        return Err(invalid_argument(format!(
            "invalid blank page size {} x {} pt",
            size.width, size.height
        )));
    }
    crate::commands::with_open_document(&document_id, |entry| {
        ensure_page_editable(entry)?;
        let before = entry.pages.clone();
        if page_index > before.len() {
            return Err(CommandError::new(
                CommandErrorCode::PageOutOfRange,
                format!(
                    "insert position {page_index} is outside 0..={}",
                    before.len()
                ),
            ));
        }
        let physical_page = entry.document.pages().len();
        {
            use pdfium_render::prelude::{PdfPagePaperSize, PdfPoints};
            let pages = entry.document.pages_mut();
            pages
                .create_page_at_index(
                    PdfPagePaperSize::Custom(
                        PdfPoints::new(size.width),
                        PdfPoints::new(size.height),
                    ),
                    physical_page,
                )
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::RenderFailed,
                        format!("could not create a blank page: {error:?}"),
                    )
                })?;
        }
        let physical_page = physical_page as u32;
        let mut after = before.clone();
        after.order.insert(page_index as usize, physical_page);
        after.blanks.insert(physical_page, BlankPage { size });
        Ok(push_page_action(
            entry, before, after, None, None, page_index, page_index,
        ))
    })
}

/// The `copy_page` command: duplicates the current page immediately after it
/// as one undoable action.
///
/// The duplicate is a real PDFium page, not a second logical reference to the
/// source page. To get independent page objects PDFium's own importer is used
/// (`FPDF_ImportPagesByIndex`) against a serialization of the current live
/// document state, and the new physical page is appended to the live document.
/// Appending keeps every existing physical page index (and therefore every
/// session record keyed by it) valid.
///
/// The importer clones the page dictionary, content streams, resources, links
/// and annotation objects. Form widgets keep their original field names at
/// import time because Pdfium only recognizes a widget whose name belongs to a
/// field it knows; the session therefore namespaces the copy's fields for
/// display and value routing, and the save pipeline renames the cloned widgets
/// and clones their parent fields into the output AcroForm.
#[tauri::command]
pub fn copy_page(document_id: String, page_index: u32) -> Result<PageOperationView, CommandError> {
    crate::commands::with_open_document(&document_id, |entry| {
        ensure_page_editable(entry)?;
        let before = entry.pages.clone();
        let physical_page = before.physical(page_index)?;
        let new_physical = entry.document.pages().len() as u32;
        let inherited = inherited_form_values(entry, physical_page)?;
        // Pdfium's importer drops page link destinations; capture them while
        // the copy still points at the live source page.
        let source_links =
            crate::links::capture_copied_link_sources(&entry.document, physical_page)?;

        // Import from Pdfium's own serialization, not the finalized save
        // output: it carries every PDFium-side edit while keeping physical
        // page indices and raw field names, which is what the importer and the
        // session's copy namespacing rely on.
        let snapshot = crate::save::serialize_pdfium_state(entry)?;
        let engine = crate::engine::Engine::shared()
            .map_err(|message| CommandError::new(CommandErrorCode::PdfiumUnavailable, message))?;
        let source = engine.open_bytes(snapshot, None).map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not reopen the document snapshot to copy the page: {error:?}"),
            )
        })?;
        entry
            .document
            .pages_mut()
            .copy_page_from_document(&source, physical_page as i32, new_physical as i32)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::RenderFailed,
                    format!("could not copy page {page_index}: {error:?}"),
                )
            })?;
        // Pdfium shares field objects (radio parents and nested fields) across
        // the import; keep the snapshot so those pointers stay valid.
        entry.copy_snapshots.push(source);
        // The copy must be usable in the viewer before it is saved: give its
        // dropped page destinations back through PDFX's private page key. A
        // failure here does not invalidate the copied page, and the save
        // pipeline still repairs the output from the source page.
        if let Err(error) =
            crate::links::restore_copied_link_sources(&entry.document, new_physical, &source_links)
        {
            eprintln!("[pdfx] could not restore copied link destinations: {error:?}");
        }

        let mut after = before.clone();
        after.order.insert((page_index + 1) as usize, new_physical);
        after.copies.insert(
            new_physical,
            CopiedPage {
                source: physical_page,
                inherited,
            },
        );
        Ok(push_page_action(
            entry,
            before,
            after,
            None,
            None,
            page_index,
            page_index + 1,
        ))
    })
}

/// Captures the source page's current form values, keyed by raw field name.
///
/// Discovery already merges the session overlay, so this is the value the user
/// sees, including radio and choice selections that Pdfium does not store and
/// values filled on a copied source page. The copy uses these as fallbacks
/// until the user edits the copy's own field.
fn inherited_form_values(
    entry: &OpenDocument,
    source_physical: u32,
) -> Result<crate::form::FormOverlay, CommandError> {
    use crate::form::{FormFieldKind, FormOverlay};
    let mut inherited = FormOverlay::default();
    for field in crate::form::discover_all(entry)? {
        if field.created || field.page_index != source_physical {
            continue;
        }
        match field.kind {
            FormFieldKind::Text => {
                if let Some(value) = field.value {
                    inherited.text.insert(field.raw_name, value);
                }
            }
            FormFieldKind::Checkbox => {
                inherited.checked.insert(field.raw_name, field.checked);
            }
            FormFieldKind::Radio => {
                if let Some(value) = field.value {
                    inherited.radio.insert(field.raw_name, value);
                }
            }
            FormFieldKind::ComboBox | FormFieldKind::ListBox => {
                if let Some(value) = field.value {
                    inherited.choice.insert(field.raw_name, value);
                }
            }
            FormFieldKind::PushButton | FormFieldKind::Signature | FormFieldKind::Unknown => {}
        }
    }
    Ok(inherited)
}
