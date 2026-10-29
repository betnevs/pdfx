//! Optional content groups (layers).
//!
//! PDFium can render `/OCProperties` but this binding exposes no API to read or
//! change it, so the document-wide layer list is parsed with `lopdf` from the
//! live document's unencrypted serialization. Visibility, rename and delete are
//! session state recorded in the unified edit history; the save pipeline writes
//! the final state as one appended `lopdf` revision.
//!
//! Rendering uses a shadow PDFium document built from the same serialization
//! with the session's `/OCProperties` state applied. It is rebuilt lazily only
//! while visibility differs from the baseline or a layer is deleted, and every
//! read path (page, thumbnail, region, text, export, print) renders through it.
//! The live PDFium document is never modified for a layer edit, which is what
//! makes undo/redo cheap.

use std::collections::{HashMap, HashSet};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, IncrementalDocument, Object, ObjectId, Stream};
use pdfium_render::prelude::PdfDocument;
use serde::{Deserialize, Serialize};

use crate::commands::{with_open_document, CommandError, CommandErrorCode, OpenDocument};
use crate::text_edit::{EditAction, EditHistoryView, EditStepView};

/// Longest layer name accepted at the command boundary. Acrobat allows long
/// names; this keeps the panel and the PDF text string manageable.
const MAX_LAYER_NAME_CHARS: usize = 120;

/// Upper bound for one decoded content stream while looking for marked
/// content. A stream that expands beyond this is treated as unsafe to rewrite
/// instead of risking unbounded memory use on hostile input.
const MAX_LAYER_CONTENT_BYTES: usize = 256 * 1024 * 1024;

/// Maximum form-XObject recursion. Content cycles are also stopped by a
/// visited set; this bounds pathological direct nesting.
const MAX_FORM_DEPTH: usize = 32;

/// One layer as shown in the layer panel. `index` is the layer's position in
/// `/OCProperties /OCGs`; it is stable for the session and is the identity the
/// frontend sends back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerView {
    pub index: u32,
    pub name: String,
    pub visible: bool,
}

/// The layer list for one Document. `document_id` lets the panel discard a
/// response that belongs to a document it is no longer showing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerListView {
    pub document_id: String,
    pub layers: Vec<LayerView>,
}

/// The outcome of a visibility, rename or delete command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerEditResultView {
    pub layers: Vec<LayerView>,
    pub history: EditHistoryView,
}

/// One layer's session state. `deleted` layers stay in the history snapshot so
/// undo can restore them but disappear from the panel and from `/OCGs` at save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerRecord {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) deleted: bool,
}

/// The final layer state carried by the save pipeline.
///
/// It is derived from the session on every save (never accumulated), so
/// repeated saves cannot duplicate objects and undo/redo is reflected in the
/// output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSaveState {
    pub(crate) records: Vec<LayerSaveRecord>,
}

/// One `/OCGs` position in the save state. `index` addresses the original
/// array; object ids are re-resolved from the base bytes on every save because
/// PDFium can renumber them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSaveRecord {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) deleted: bool,
}

impl LayerSaveState {
    fn from_records(records: &[LayerRecord]) -> Self {
        Self {
            records: records
                .iter()
                .map(|record| LayerSaveRecord {
                    index: record.index,
                    name: record.name.clone(),
                    visible: record.visible,
                    deleted: record.deleted,
                })
                .collect(),
        }
    }
}

/// A shadow PDFium document that renders the session's layer visibility.
pub(crate) struct LayerShadow {
    document: PdfDocument<'static>,
    /// `EditSession::current_state` the shadow was built from.
    state: u64,
}

/// Per-document layer state, loaded lazily by the first layer command.
#[derive(Default)]
pub(crate) struct LayerStore {
    loaded: bool,
    baseline: Vec<LayerRecord>,
    current: Vec<LayerRecord>,
    shadow: Option<LayerShadow>,
    /// True while the arrangement includes a copied page. PDFium's importer
    /// clones the OCG dictionaries the copy's resources point at, so rendering
    /// must go through the shadow even at the baseline state to map the clones
    /// back to the canonical layers. Recomputed from the page model on every
    /// shadow check, so undoing the copy drops it again.
    force_shadow: bool,
}

impl LayerStore {
    fn load_from_bytes(&mut self, bytes: &[u8]) {
        let records = lopdf::Document::load_mem(bytes)
            .ok()
            .and_then(|document| read_layer_records(&document))
            .unwrap_or_default();
        self.baseline = records.clone();
        self.current = records;
        self.loaded = true;
        self.shadow = None;
    }

    fn views(&self) -> Vec<LayerView> {
        self.current
            .iter()
            .filter(|record| !record.deleted)
            .map(|record| LayerView {
                index: record.index,
                name: display_name(record),
                visible: record.visible,
            })
            .collect()
    }

    fn record(&self, index: u32) -> Option<usize> {
        self.current
            .iter()
            .position(|record| record.index == index && !record.deleted)
    }

    fn snapshot(&self) -> Vec<LayerRecord> {
        self.current.clone()
    }

    fn restore(&mut self, records: &[LayerRecord]) {
        self.current = records.to_vec();
        self.loaded = true;
    }

    pub(crate) fn save_state(&self) -> Option<LayerSaveState> {
        if !self.loaded || self.current.is_empty() || self.current == self.baseline {
            return None;
        }
        Some(LayerSaveState::from_records(&self.current))
    }

    fn full_save_state(&self) -> LayerSaveState {
        LayerSaveState::from_records(&self.current)
    }

    fn needs_shadow(&self) -> bool {
        if self.current.is_empty() {
            // A document without optional content never needs a shadow, even
            // when a copied page made `force_shadow` true.
            return false;
        }
        self.force_shadow
            || self.current.iter().any(|record| {
                if record.deleted {
                    return true;
                }
                self.baseline
                    .iter()
                    .find(|baseline| baseline.index == record.index)
                    .map(|baseline| baseline.visible != record.visible)
                    .unwrap_or(true)
            })
    }
}

fn display_name(record: &LayerRecord) -> String {
    if record.name.trim().is_empty() {
        format!("图层 {}", record.index + 1)
    } else {
        record.name.clone()
    }
}

/// Reads the default configuration's layers from a base PDF.
///
/// Returns `None` when `/OCProperties` or `/OCGs` is absent or malformed, and
/// an empty list when `/OCGs` is an empty array. Entries that are not
/// references to dictionaries are skipped; their `/OCGs` positions are kept
/// when the save pipeline rewrites the array.
fn read_layer_records(document: &lopdf::Document) -> Option<Vec<LayerRecord>> {
    let catalog = document.catalog().ok()?;
    let ocp = catalog.get_deref(b"OCProperties", document).ok()?;
    let ocp = ocp.as_dict().ok()?;
    let ocgs = ocp.get_deref(b"OCGs", document).ok()?;
    let ocgs = ocgs.as_array().ok()?;
    let config = ocp
        .get_deref(b"D", document)
        .ok()
        .and_then(|object| object.as_dict().ok());

    let (base_visible, on, off) = match config {
        Some(config) => {
            let base_visible = config
                .get(b"BaseState")
                .ok()
                .and_then(|object| object.as_name().ok())
                != Some(b"OFF");
            let mut on = HashSet::new();
            let mut off = HashSet::new();
            collect_config_refs(document, config.get(b"ON").ok(), &mut on);
            collect_config_refs(document, config.get(b"OFF").ok(), &mut off);
            (base_visible, on, off)
        }
        None => (true, HashSet::new(), HashSet::new()),
    };

    let mut records = Vec::new();
    for (index, entry) in ocgs.iter().enumerate() {
        let Object::Reference(id) = entry else {
            continue;
        };
        let Ok(dictionary) = document.get_dictionary(*id) else {
            continue;
        };
        let name = dictionary
            .get(b"Name")
            .ok()
            .and_then(|object| object.as_str().ok())
            .map(crate::save::decode_pdf_text)
            .unwrap_or_default();
        // An explicit /ON entry wins over /OFF, matching PDF readers.
        let visible = if on.contains(id) {
            true
        } else if off.contains(id) {
            false
        } else {
            base_visible
        };
        records.push(LayerRecord {
            index: index as u32,
            name,
            visible,
            deleted: false,
        });
    }
    Some(records)
}

fn collect_config_refs(
    document: &lopdf::Document,
    entry: Option<&Object>,
    refs: &mut HashSet<ObjectId>,
) {
    let Some(entry) = entry else {
        return;
    };
    let Ok((_, value)) = document.dereference(entry) else {
        return;
    };
    let Ok(array) = value.as_array() else {
        return;
    };
    for item in array {
        if let Ok(id) = item.as_reference() {
            refs.insert(id);
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Loads the layer list from the live document on first use.
fn ensure_layers_loaded(entry: &mut OpenDocument) -> Result<(), CommandError> {
    if entry.layers.loaded {
        return Ok(());
    }
    let base = crate::save::serialize_pdfium_state(entry)?;
    entry.layers.load_from_bytes(&base);
    Ok(())
}

fn ensure_layer_editable(entry: &OpenDocument) -> Result<(), CommandError> {
    crate::text_edit::ensure_content_editable(&entry.document)
}

fn layer_not_found(index: u32) -> CommandError {
    CommandError::new(
        CommandErrorCode::LayerNotFound,
        format!("layer {index} is not in this document"),
    )
}

/// The `list_layers` command: document-wide optional content groups.
#[tauri::command]
pub fn list_layers(document_id: String) -> Result<LayerListView, CommandError> {
    let view_id = document_id.clone();
    with_open_document(&document_id, |entry| {
        ensure_layers_loaded(entry)?;
        Ok(LayerListView {
            document_id: view_id.clone(),
            layers: entry.layers.views(),
        })
    })
}

/// The `set_layer_visible` command.
#[tauri::command]
pub fn set_layer_visible(
    document_id: String,
    layer_index: u32,
    visible: bool,
) -> Result<LayerEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_layer_editable(entry)?;
        ensure_layers_loaded(entry)?;
        let position = entry
            .layers
            .record(layer_index)
            .ok_or_else(|| layer_not_found(layer_index))?;
        if entry.layers.current[position].visible == visible {
            return Ok(LayerEditResultView {
                layers: entry.layers.views(),
                history: entry.edit.history_view(),
            });
        }
        let before = entry.layers.snapshot();
        entry.layers.current[position].visible = visible;
        let after = entry.layers.snapshot();
        let history = push_layer_action(entry, before, after);
        Ok(LayerEditResultView {
            layers: entry.layers.views(),
            history,
        })
    })
}

/// The `rename_layer` command. Names are trimmed; blank, control-containing
/// and over-long names are rejected.
#[tauri::command]
pub fn rename_layer(
    document_id: String,
    layer_index: u32,
    name: String,
) -> Result<LayerEditResultView, CommandError> {
    let name = validate_layer_name(&name)?;
    with_open_document(&document_id, |entry| {
        ensure_layer_editable(entry)?;
        ensure_layers_loaded(entry)?;
        let position = entry
            .layers
            .record(layer_index)
            .ok_or_else(|| layer_not_found(layer_index))?;
        if entry.layers.current[position].name == name {
            return Ok(LayerEditResultView {
                layers: entry.layers.views(),
                history: entry.edit.history_view(),
            });
        }
        let before = entry.layers.snapshot();
        entry.layers.current[position].name = name;
        let after = entry.layers.snapshot();
        let history = push_layer_action(entry, before, after);
        Ok(LayerEditResultView {
            layers: entry.layers.views(),
            history,
        })
    })
}

/// The `delete_layer` command.
///
/// The UI owns the confirmation prompt. Before the deletion enters the history
/// the save-time transformation is dry-run against the current base bytes, so
/// a layer whose content cannot be removed safely is refused instead of
/// producing a corrupt or partially hidden output.
#[tauri::command]
pub fn delete_layer(
    document_id: String,
    layer_index: u32,
) -> Result<LayerEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_layer_editable(entry)?;
        ensure_layers_loaded(entry)?;
        let position = entry
            .layers
            .record(layer_index)
            .ok_or_else(|| layer_not_found(layer_index))?;
        let before = entry.layers.snapshot();
        let mut after = before.clone();
        after[position].deleted = true;

        let base = crate::save::serialize_pdfium_state(entry)?;
        apply_layer_state_to_bytes(base, &LayerSaveState::from_records(&after))?;

        entry.layers.restore(&after);
        let history = push_layer_action(entry, before, after);
        Ok(LayerEditResultView {
            layers: entry.layers.views(),
            history,
        })
    })
}

fn validate_layer_name(name: &str) -> Result<String, CommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::LayerNameInvalid,
            "layer name must not be blank",
        ));
    }
    if trimmed.chars().count() > MAX_LAYER_NAME_CHARS {
        return Err(CommandError::new(
            CommandErrorCode::LayerNameInvalid,
            format!("layer name must be at most {MAX_LAYER_NAME_CHARS} characters"),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(CommandError::new(
            CommandErrorCode::LayerNameInvalid,
            "layer name must not contain control characters",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Appends one layer change to the unified undo stack.
fn push_layer_action(
    entry: &mut OpenDocument,
    before: Vec<LayerRecord>,
    after: Vec<LayerRecord>,
) -> EditHistoryView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::Layer(LayerEditAction {
            before_state,
            after_state,
            before,
            after,
        }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.edit.history_view()
}

/// One committed layer change. The full record list is small, so snapshots
/// restore exact visibility, names and deletions on undo/redo.
pub(crate) struct LayerEditAction {
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: Vec<LayerRecord>,
    pub(crate) after: Vec<LayerRecord>,
}

fn layer_step(entry: &OpenDocument) -> EditStepView {
    EditStepView {
        page_index: 0,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    }
}

pub(crate) fn undo_layer(
    entry: &mut OpenDocument,
    action: LayerEditAction,
) -> Result<EditStepView, CommandError> {
    entry.layers.restore(&action.before);
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Layer(action));
    Ok(layer_step(entry))
}

pub(crate) fn redo_layer(
    entry: &mut OpenDocument,
    action: LayerEditAction,
) -> Result<EditStepView, CommandError> {
    entry.layers.restore(&action.after);
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Layer(action));
    Ok(layer_step(entry))
}

// ---------------------------------------------------------------------------
// Shadow rendering
// ---------------------------------------------------------------------------

impl OpenDocument {
    /// The PDFium document read paths should render from.
    ///
    /// At the source baseline this is the live document. While a layer is
    /// hidden or deleted it is a shadow document rebuilt from the live
    /// serialization with the session's layer state applied. The shadow is
    /// reused until `current_state` changes; returning to the baseline drops
    /// it, so an ordinary document never pays for a second PDFium document.
    pub(crate) fn render_document(&mut self) -> Result<&PdfDocument<'static>, CommandError> {
        self.sync_layer_shadow()?;
        Ok(match &self.layers.shadow {
            Some(shadow) => &shadow.document,
            None => &self.document,
        })
    }

    fn sync_layer_shadow(&mut self) -> Result<(), CommandError> {
        // A copied page references OCG clones that only the shadow's alias
        // rewrite maps back to the canonical layers. Keep rendering through
        // the shadow while such a page is included so copied hidden layers do
        // not reappear on the canvas.
        self.layers.force_shadow = self.has_included_copies();
        if self.layers.force_shadow && !self.layers.loaded {
            ensure_layers_loaded(self)?;
        }
        if !self.layers.loaded || !self.layers.needs_shadow() {
            self.layers.shadow = None;
            return Ok(());
        }
        if self.layers.shadow.as_ref().map(|shadow| shadow.state) == Some(self.edit.current_state) {
            return Ok(());
        }

        let save_state = self.layers.full_save_state();
        let base = crate::save::serialize_pdfium_state(self)?;
        let bytes = apply_layer_state_to_bytes(base, &save_state)?;
        // Drop the previous shadow before allocating its replacement.
        self.layers.shadow = None;
        let document = crate::commands::open_pdfium_bytes(bytes)?;
        self.layers.shadow = Some(LayerShadow {
            document,
            state: self.edit.current_state,
        });
        Ok(())
    }
}

/// Applies the layer state to a fresh base serialization and returns complete
/// PDF bytes. Used for the shadow document and for delete-time validation.
pub(crate) fn apply_layer_state_to_bytes(
    base: Vec<u8>,
    state: &LayerSaveState,
) -> Result<Vec<u8>, CommandError> {
    let previous = lopdf::Document::load_mem(&base).map_err(|error| {
        unsupported_error(format!(
            "could not parse the document for a layer change: {error}"
        ))
    })?;
    let mut incremental = IncrementalDocument::create_from(base, previous);
    apply_layer_edit(&mut incremental, state)?;
    let mut output = Vec::new();
    incremental
        .save_to(&mut output)
        .map_err(|error| unsupported_error(format!("could not write the layer change: {error}")))?;
    Ok(output)
}

// ---------------------------------------------------------------------------
// Save-time transformation
// ---------------------------------------------------------------------------

/// Applies the final layer state as part of the save pipeline's one `lopdf`
/// revision. Called before the page-tree rewrite.
pub(crate) fn apply_layer_edit(
    incremental: &mut IncrementalDocument,
    state: &LayerSaveState,
) -> Result<(), CommandError> {
    if state.records.is_empty() {
        return Ok(());
    }
    let plan = LayerPlan::read(incremental.get_prev_documents(), state)?;
    // PDFium's page importer clones the OCG objects a copied page's resources
    // point at, so the copy references OCGs that are not in /OCProperties and
    // would ignore every visibility rule. Map those clones back to the layer
    // they mirror before the plan is written and before content is filtered.
    let aliases = {
        let canonical_ids: Vec<ObjectId> = plan.ocg_ids.iter().flatten().copied().collect();
        collect_ocg_aliases(incremental.get_prev_documents(), &canonical_ids)
    };
    write_layer_plan(incremental, state, &plan)?;
    if !aliases.is_empty() || !plan.deleted_ids.is_empty() {
        // Besides remapping clones, this drops resource entries that pointed
        // at a deleted layer, so the retired OCG is not reachable from a page.
        rewrite_resource_ocg_refs(incremental, &aliases, &plan.deleted_ids)?;
    }
    if !plan.deleted_ids.is_empty() {
        let rewrites = ContentFilter::new(
            incremental.get_prev_documents(),
            &plan.deleted_ids,
            &aliases,
        )
        .run()?;
        apply_content_rewrites(incremental, rewrites)?;
    }
    Ok(())
}

/// Everything the layer rewrite needs from the base document, resolved before
/// the incremental document is mutated.
struct LayerPlan {
    catalog_id: ObjectId,
    ocp_id: Option<ObjectId>,
    ocp_dict: Dictionary,
    d_id: Option<ObjectId>,
    d_dict: Dictionary,
    ocgs: Vec<Object>,
    ocg_ids: Vec<Option<ObjectId>>,
    /// Alternative configurations (`/OCProperties /Configs`), resolved so a
    /// deleted layer can be removed from them too.
    configs: Option<Vec<Object>>,
    order: Option<Vec<Object>>,
    rbgroups: Option<Vec<Object>>,
    original_visible: Vec<bool>,
    deleted_ids: HashSet<ObjectId>,
}

impl LayerPlan {
    fn read(document: &lopdf::Document, state: &LayerSaveState) -> Result<Self, CommandError> {
        let catalog_id = document
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|_| internal_error("the document has no catalog reference"))?;
        let catalog = document
            .get_dictionary(catalog_id)
            .map_err(|error| internal_error(format!("could not read the catalog: {error}")))?;
        let ocp_entry = catalog
            .get(b"OCProperties")
            .map_err(|_| internal_error("the document has no /OCProperties"))?
            .clone();
        let (ocp_id, ocp_dict) = match &ocp_entry {
            Object::Reference(id) => (
                Some(*id),
                document
                    .get_dictionary(*id)
                    .map_err(|error| {
                        internal_error(format!("could not read /OCProperties: {error}"))
                    })?
                    .clone(),
            ),
            Object::Dictionary(dictionary) => (None, dictionary.clone()),
            _ => return Err(internal_error("/OCProperties is not a dictionary")),
        };

        let ocgs_entry = ocp_dict
            .get(b"OCGs")
            .map_err(|_| internal_error("the document has no /OCGs array"))?
            .clone();
        let ocgs = match &ocgs_entry {
            Object::Reference(id) => document
                .get_object(*id)
                .and_then(Object::as_array)
                .map_err(|error| internal_error(format!("could not read /OCGs: {error}")))?
                .clone(),
            Object::Array(array) => array.clone(),
            _ => return Err(internal_error("/OCGs is not an array")),
        };
        let ocg_ids: Vec<Option<ObjectId>> =
            ocgs.iter().map(|entry| entry.as_reference().ok()).collect();

        let (d_id, d_dict) = match ocp_dict.get(b"D").ok() {
            Some(Object::Reference(id)) => (
                Some(*id),
                document
                    .get_dictionary(*id)
                    .map_err(|error| {
                        internal_error(format!("could not read the default layer config: {error}"))
                    })?
                    .clone(),
            ),
            Some(Object::Dictionary(dictionary)) => (None, dictionary.clone()),
            Some(_) => {
                return Err(internal_error(
                    "the default layer config is not a dictionary",
                ))
            }
            None => (None, Dictionary::new()),
        };

        let order = d_dict
            .get(b"Order")
            .ok()
            .and_then(|entry| resolve_array(document, entry));
        let rbgroups = d_dict
            .get(b"RBGroups")
            .ok()
            .and_then(|entry| resolve_array(document, entry));
        let configs = ocp_dict
            .get(b"Configs")
            .ok()
            .and_then(|entry| resolve_array(document, entry));

        let (base_visible, on, off) = default_visibility(document, &d_dict);
        let original_visible = ocg_ids
            .iter()
            .map(|id| match id {
                Some(id) if on.contains(id) => true,
                Some(id) if off.contains(id) => false,
                _ => base_visible,
            })
            .collect();

        let mut seen = HashSet::new();
        let mut deleted_ids = HashSet::new();
        for record in &state.records {
            let position = record.index as usize;
            if !seen.insert(record.index) {
                return Err(internal_error(format!(
                    "layer index {} appears twice in the save state",
                    record.index
                )));
            }
            let Some(id) = ocg_ids.get(position).and_then(|entry| *entry) else {
                return Err(internal_error(format!(
                    "layer index {} does not identify an OCG",
                    record.index
                )));
            };
            if record.deleted {
                deleted_ids.insert(id);
            }
        }

        Ok(Self {
            catalog_id,
            ocp_id,
            ocp_dict,
            d_id,
            d_dict,
            ocgs,
            ocg_ids,
            configs,
            order,
            rbgroups,
            original_visible,
            deleted_ids,
        })
    }
}

fn resolve_array(document: &lopdf::Document, entry: &Object) -> Option<Vec<Object>> {
    match entry {
        Object::Reference(id) => document
            .get_object(*id)
            .ok()
            .and_then(|object| object.as_array().ok())
            .cloned(),
        Object::Array(array) => Some(array.clone()),
        _ => None,
    }
}

fn default_visibility(
    document: &lopdf::Document,
    config: &Dictionary,
) -> (bool, HashSet<ObjectId>, HashSet<ObjectId>) {
    let base_visible = config
        .get(b"BaseState")
        .ok()
        .and_then(|object| object.as_name().ok())
        != Some(b"OFF");
    let mut on = HashSet::new();
    let mut off = HashSet::new();
    collect_config_refs(document, config.get(b"ON").ok(), &mut on);
    collect_config_refs(document, config.get(b"OFF").ok(), &mut off);
    (base_visible, on, off)
}

fn write_layer_plan(
    incremental: &mut IncrementalDocument,
    state: &LayerSaveState,
    plan: &LayerPlan,
) -> Result<(), CommandError> {
    let catalog_id = plan.catalog_id;
    incremental
        .opt_clone_object_to_new_document(catalog_id)
        .map_err(|error| internal_error(format!("could not clone the catalog: {error}")))?;

    // Normalize /OCProperties and /D to indirect objects so unknown keys are
    // preserved while the known ones are replaced.
    let ocp_id = match plan.ocp_id {
        Some(id) => {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| {
                    internal_error(format!("could not clone /OCProperties: {error}"))
                })?;
            id
        }
        None => {
            let id = incremental
                .new_document
                .try_new_object_id()
                .map_err(|error| {
                    internal_error(format!("could not allocate /OCProperties: {error}"))
                })?;
            incremental
                .new_document
                .set_object(id, Object::Dictionary(Dictionary::new()));
            let catalog = incremental
                .new_document
                .get_dictionary_mut(catalog_id)
                .map_err(|error| internal_error(format!("could not edit the catalog: {error}")))?;
            catalog.set(b"OCProperties".to_vec(), Object::Reference(id));
            id
        }
    };
    let d_id = match plan.d_id {
        Some(id) => {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| {
                    internal_error(format!("could not clone the layer config: {error}"))
                })?;
            id
        }
        None => incremental
            .new_document
            .try_new_object_id()
            .map_err(|error| {
                internal_error(format!("could not allocate the layer config: {error}"))
            })?,
    };

    // Rename the OCG objects that remain visible in the panel.
    for record in &state.records {
        if record.deleted {
            continue;
        }
        let Some(id) = plan
            .ocg_ids
            .get(record.index as usize)
            .and_then(|entry| *entry)
        else {
            continue;
        };
        incremental
            .opt_clone_object_to_new_document(id)
            .map_err(|error| internal_error(format!("could not clone an OCG: {error}")))?;
        let dictionary = incremental
            .new_document
            .get_dictionary_mut(id)
            .map_err(|error| internal_error(format!("could not edit an OCG: {error}")))?;
        dictionary.set(
            b"Name".to_vec(),
            crate::save::text_string_object(&record.name),
        );
    }

    let mut new_ocgs = Vec::new();
    let mut on = Vec::new();
    let mut off = Vec::new();
    for (position, entry) in plan.ocgs.iter().enumerate() {
        let record = state
            .records
            .iter()
            .find(|record| record.index as usize == position);
        let deleted = record.map(|record| record.deleted).unwrap_or(false);
        let Some(id) = plan.ocg_ids[position] else {
            if !deleted {
                new_ocgs.push(entry.clone());
            }
            continue;
        };
        if deleted {
            continue;
        }
        new_ocgs.push(Object::Reference(id));
        let visible = record
            .map(|record| record.visible)
            .unwrap_or(plan.original_visible[position]);
        if visible {
            on.push(Object::Reference(id));
        } else {
            off.push(Object::Reference(id));
        }
    }

    // Preserve every unknown /D key; replace only the visibility configuration
    // entries PDFX owns. `/AS` refers to the old automatic state and would
    // contradict the explicit /ON and /OFF arrays.
    let mut d_dict = plan.d_dict.clone();
    d_dict.set(b"ON".to_vec(), Object::Array(on));
    d_dict.set(b"OFF".to_vec(), Object::Array(off));
    d_dict.set(b"BaseState".to_vec(), Object::Name(b"OFF".to_vec()));
    d_dict.remove(b"AS");
    if let Some(order) = &plan.order {
        d_dict.set(
            b"Order".to_vec(),
            Object::Array(filter_deleted_refs(order, &plan.deleted_ids)),
        );
    }
    if let Some(groups) = &plan.rbgroups {
        d_dict.set(
            b"RBGroups".to_vec(),
            Object::Array(filter_deleted_refs(groups, &plan.deleted_ids)),
        );
    }
    // Unknown /D keys are preserved, but a deleted layer must not stay
    // reachable through them (for example an `/AS`-style array PDFX does not
    // model). Only references to deleted OCGs are removed.
    scrub_config_dictionary(&mut d_dict, &plan.deleted_ids);
    incremental
        .new_document
        .set_object(d_id, Object::Dictionary(d_dict));

    let mut ocp_dict = plan.ocp_dict.clone();
    ocp_dict.set(b"OCGs".to_vec(), Object::Array(new_ocgs));
    ocp_dict.set(b"D".to_vec(), Object::Reference(d_id));
    // Alternative configurations keep listing the remaining layers, so a
    // deleted OCG must not linger there either. Direct dictionaries are
    // filtered in place; an indirect dictionary is cloned so every reference
    // to it sees the filtered version.
    if let Some(configs) = &plan.configs {
        let mut filtered_configs = Vec::with_capacity(configs.len());
        for entry in configs {
            match entry {
                Object::Reference(id) if !plan.deleted_ids.is_empty() => {
                    let is_dictionary =
                        incremental.get_prev_documents().get_dictionary(*id).is_ok();
                    if is_dictionary {
                        incremental
                            .opt_clone_object_to_new_document(*id)
                            .map_err(|error| {
                                internal_error(format!(
                                    "could not clone an alternative layer config: {error}"
                                ))
                            })?;
                        if let Ok(dictionary) = incremental.new_document.get_dictionary_mut(*id) {
                            scrub_config_dictionary(dictionary, &plan.deleted_ids);
                        }
                    }
                    filtered_configs.push(entry.clone());
                }
                Object::Dictionary(dictionary) => {
                    let mut dictionary = dictionary.clone();
                    scrub_config_dictionary(&mut dictionary, &plan.deleted_ids);
                    filtered_configs.push(Object::Dictionary(dictionary));
                }
                other => filtered_configs.push(other.clone()),
            }
        }
        ocp_dict.set(b"Configs".to_vec(), Object::Array(filtered_configs));
    }
    incremental
        .new_document
        .set_object(ocp_id, Object::Dictionary(ocp_dict));
    Ok(())
}

/// Removes references to deleted OCGs from a configuration dictionary.
///
/// The known keys are filtered by `filter_deleted_refs`; this covers the
/// remaining values (including nested arrays and dictionaries) so the deleted
/// object does not stay reachable from `/OCProperties`.
fn scrub_config_dictionary(dictionary: &mut Dictionary, deleted: &HashSet<ObjectId>) {
    if deleted.is_empty() {
        return;
    }
    let keys: Vec<Vec<u8>> = dictionary.iter().map(|(key, _)| key.clone()).collect();
    for key in keys {
        let Some(value) = dictionary.get(key.as_slice()).ok().cloned() else {
            continue;
        };
        match filter_deleted_refs_deep(&value, deleted) {
            Some(value) => dictionary.set(key, value),
            None => {
                dictionary.remove(key.as_slice());
            }
        }
    }
}

fn filter_deleted_refs_deep(value: &Object, deleted: &HashSet<ObjectId>) -> Option<Object> {
    match value {
        Object::Reference(id) if deleted.contains(id) => None,
        Object::Array(items) => Some(Object::Array(
            items
                .iter()
                .filter_map(|item| filter_deleted_refs_deep(item, deleted))
                .collect(),
        )),
        Object::Dictionary(dictionary) => {
            let mut filtered = dictionary.clone();
            scrub_config_dictionary(&mut filtered, deleted);
            Some(Object::Dictionary(filtered))
        }
        other => Some(other.clone()),
    }
}

fn filter_deleted_refs(items: &[Object], deleted: &HashSet<ObjectId>) -> Vec<Object> {
    items
        .iter()
        .filter_map(|item| match item {
            Object::Reference(id) if deleted.contains(id) => None,
            Object::Array(inner) => Some(Object::Array(filter_deleted_refs(inner, deleted))),
            other => Some(other.clone()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Copied-page OCG aliases
// ---------------------------------------------------------------------------

/// Maps OCG objects that duplicate a `/OCProperties /OCGs` entry to the
/// canonical object.
///
/// PDFium's page importer clones the OCG dictionaries a copied page's
/// `/Resources /Properties` point at. The clones keep the layer's `/Name` but
/// are not listed in `/OCProperties`, so PDFium would treat their content as
/// always visible. Matching by name gives the copy the source layer's
/// visibility and deletion behaviour. Ambiguous names (two canonical OCGs with
/// the same `/Name`) are left alone rather than guessing.
fn collect_ocg_aliases(
    document: &lopdf::Document,
    canonical_ids: &[ObjectId],
) -> HashMap<ObjectId, ObjectId> {
    let canonical: HashSet<ObjectId> = canonical_ids.iter().copied().collect();
    let mut canonical_by_name: HashMap<Vec<u8>, Option<ObjectId>> = HashMap::new();
    for id in &canonical {
        let Some(name) = ocg_name(document, *id) else {
            continue;
        };
        match canonical_by_name.get_mut(&name) {
            Some(existing) => *existing = None,
            None => {
                canonical_by_name.insert(name, Some(*id));
            }
        }
    }

    let mut aliases = HashMap::new();
    for (id, object) in &document.objects {
        if canonical.contains(id) {
            continue;
        }
        let Ok(dictionary) = object.as_dict() else {
            continue;
        };
        if !dictionary.has_type(b"OCG") {
            continue;
        }
        let Some(name) = dictionary
            .get(b"Name")
            .ok()
            .and_then(|object| object.as_str().ok())
            .map(<[u8]>::to_vec)
        else {
            continue;
        };
        if let Some(Some(target)) = canonical_by_name.get(&name) {
            aliases.insert(*id, *target);
        }
    }
    aliases
}

/// The OCG references listed by `/OCProperties /OCGs`, tolerating a document
/// without optional content or with a malformed configuration.
fn canonical_ocg_ids(document: &lopdf::Document) -> Vec<ObjectId> {
    let Ok(catalog) = document.catalog() else {
        return Vec::new();
    };
    let Ok(ocp) = catalog.get_deref(b"OCProperties", document) else {
        return Vec::new();
    };
    let Ok(ocp) = ocp.as_dict() else {
        return Vec::new();
    };
    let Ok(ocgs) = ocp.get_deref(b"OCGs", document) else {
        return Vec::new();
    };
    let Ok(ocgs) = ocgs.as_array() else {
        return Vec::new();
    };
    ocgs.iter()
        .filter_map(|entry| entry.as_reference().ok())
        .collect()
}

/// Maps the OCG clones a copied page references back to the canonical layer
/// objects without changing any visibility state.
///
/// The save pipeline normally performs this inside `apply_layer_edit`, which
/// runs whenever the layer state differs from the baseline. A copy saved
/// before any layer edit would otherwise keep clones that are absent from
/// `/OCProperties`, so a layer hidden by the source configuration would show
/// up visible in the output.
pub(crate) fn remap_copied_page_aliases(
    incremental: &mut IncrementalDocument,
) -> Result<(), CommandError> {
    let aliases = {
        let document = incremental.get_prev_documents();
        let canonical_ids = canonical_ocg_ids(document);
        if canonical_ids.is_empty() {
            return Ok(());
        }
        collect_ocg_aliases(document, &canonical_ids)
    };
    if aliases.is_empty() {
        return Ok(());
    }
    rewrite_resource_ocg_refs(incremental, &aliases, &HashSet::new())
}

fn ocg_name(document: &lopdf::Document, id: ObjectId) -> Option<Vec<u8>> {
    document
        .get_dictionary(id)
        .ok()?
        .get(b"Name")
        .ok()?
        .as_str()
        .ok()
        .map(<[u8]>::to_vec)
}

/// One object the alias pass must rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ResourceHolder {
    /// A page, form XObject or indirect resources dictionary: reachable
    /// `/Resources` and/or its own `/Properties` entry.
    Container,
    /// An indirect `/Properties` dictionary: its own entries are rewritten.
    Properties,
}

/// Rewrites resource property references in the appended revision: cloned OCG
/// ids are mapped to their canonical layer, and references to deleted layers
/// are removed so the deleted OCG does not stay reachable from a page.
fn rewrite_resource_ocg_refs(
    incremental: &mut IncrementalDocument,
    aliases: &HashMap<ObjectId, ObjectId>,
    retired: &HashSet<ObjectId>,
) -> Result<(), CommandError> {
    let holders = {
        let document = incremental.get_prev_documents();
        resource_holders(document, aliases, retired)
    };
    for (id, holder) in holders {
        incremental
            .opt_clone_object_to_new_document(id)
            .map_err(|error| {
                internal_error(format!("could not clone a resource dictionary: {error}"))
            })?;
        let object = incremental
            .new_document
            .get_object_mut(id)
            .map_err(|error| internal_error(format!("could not edit a resource: {error}")))?;
        let dictionary = match object {
            Object::Dictionary(dictionary) => dictionary,
            Object::Stream(stream) => &mut stream.dict,
            _ => continue,
        };
        match holder {
            ResourceHolder::Container => {
                // A page or form XObject reaches its resources through
                // /Resources; an indirect resources dictionary holds
                // /Properties directly, so cover both shapes.
                if let Ok(Object::Dictionary(resources)) = dictionary.get_mut(b"Resources") {
                    rewrite_properties_entry(resources, aliases, retired);
                }
                rewrite_properties_entry(dictionary, aliases, retired);
            }
            ResourceHolder::Properties => rewrite_property_entries(dictionary, aliases, retired),
        }
    }
    Ok(())
}

fn rewrite_properties_entry(
    resources: &mut Dictionary,
    aliases: &HashMap<ObjectId, ObjectId>,
    retired: &HashSet<ObjectId>,
) {
    if let Ok(Object::Dictionary(properties)) = resources.get_mut(b"Properties") {
        rewrite_property_entries(properties, aliases, retired);
    }
}

fn rewrite_property_entries(
    properties: &mut Dictionary,
    aliases: &HashMap<ObjectId, ObjectId>,
    retired: &HashSet<ObjectId>,
) {
    let keys: Vec<Vec<u8>> = properties.iter().map(|(key, _)| key.clone()).collect();
    for key in keys {
        let Some(value) = properties.get(key.as_slice()).ok().cloned() else {
            continue;
        };
        let Ok(id) = value.as_reference() else {
            continue;
        };
        let canonical = aliases.get(&id).copied().unwrap_or(id);
        if retired.contains(&canonical) {
            properties.remove(key.as_slice());
        } else if canonical != id {
            properties.set(key, Object::Reference(canonical));
        }
    }
}

fn properties_need_rewrite(
    properties: &Dictionary,
    aliases: &HashMap<ObjectId, ObjectId>,
    retired: &HashSet<ObjectId>,
) -> bool {
    properties.iter().any(|(_, value)| {
        value
            .as_reference()
            .map(|id| aliases.contains_key(&id) || retired.contains(&id))
            .unwrap_or(false)
    })
}

/// Collects the objects whose direct `/Properties` dictionary references a
/// cloned or deleted OCG: pages, form XObjects, indirect resources
/// dictionaries and indirect properties dictionaries.
///
/// Only objects that need a direct edit are returned. A property dictionary
/// stored as an indirect object is edited in place, so its parent container
/// does not need to be rewritten.
fn resource_holders(
    document: &lopdf::Document,
    aliases: &HashMap<ObjectId, ObjectId>,
    retired: &HashSet<ObjectId>,
) -> Vec<(ObjectId, ResourceHolder)> {
    let page_ids: HashSet<ObjectId> = document.get_pages().values().copied().collect();
    let mut pending: Vec<(ObjectId, ResourceHolder)> = page_ids
        .iter()
        .map(|id| (*id, ResourceHolder::Container))
        .collect();
    let mut visited = HashSet::new();
    let mut holders = Vec::new();
    while let Some((id, holder)) = pending.pop() {
        if !visited.insert((id, holder)) {
            continue;
        }
        let Ok(object) = document.get_object(id) else {
            continue;
        };
        let dictionary = match object {
            Object::Dictionary(dictionary) => dictionary,
            Object::Stream(stream) => &stream.dict,
            _ => continue,
        };
        if holder == ResourceHolder::Properties {
            if properties_need_rewrite(dictionary, aliases, retired) {
                holders.push((id, holder));
            }
            continue;
        }

        let mut resources: Vec<(ObjectId, &Dictionary)> = Vec::new();
        if page_ids.contains(&id) {
            let (direct, ids) = document
                .get_page_resources(id)
                .unwrap_or((None, Vec::new()));
            if let Some(direct) = direct {
                resources.push((id, direct));
            }
            for resources_id in ids {
                pending.push((resources_id, ResourceHolder::Container));
            }
        }
        if let Ok(entry) = dictionary.get(b"Resources") {
            match entry {
                Object::Dictionary(direct) => resources.push((id, direct)),
                Object::Reference(resources_id) => {
                    pending.push((*resources_id, ResourceHolder::Container));
                }
                _ => {}
            }
        }
        // An indirect resources dictionary is visited as a container; its own
        // /Properties and /XObject entries are handled here.
        if dictionary.has(b"Properties") || dictionary.has(b"XObject") {
            resources.push((id, dictionary));
        }
        for (owner, resources) in resources {
            match resources.get(b"Properties") {
                Ok(Object::Reference(properties_id)) => {
                    pending.push((*properties_id, ResourceHolder::Properties));
                }
                Ok(Object::Dictionary(properties))
                    if properties_need_rewrite(properties, aliases, retired) =>
                {
                    holders.push((owner, ResourceHolder::Container));
                }
                _ => {}
            }
            if let Ok(entry) = resources.get(b"XObject") {
                let Ok((_, value)) = document.dereference(entry) else {
                    continue;
                };
                let Ok(xobjects) = value.as_dict() else {
                    continue;
                };
                for (_, xobject) in xobjects.iter() {
                    if let Ok(xobject_id) = xobject.as_reference() {
                        pending.push((xobject_id, ResourceHolder::Container));
                    }
                }
            }
        }
    }
    holders
}

// ---------------------------------------------------------------------------
// Marked-content removal
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Kept,
    Removed,
}

enum OcMatch {
    No,
    Match,
    Unsafe,
}

/// Walks every page and reachable form XObject, refuses structures whose
/// optional content cannot be removed safely, and collects rewritten content
/// streams for the deleted OCGs.
struct ContentFilter<'a> {
    document: &'a lopdf::Document,
    deleted: &'a HashSet<ObjectId>,
    /// Cloned OCG ids (created by PDFium's page importer) mapped to the
    /// canonical layer they mirror, so a copied page's blocks follow the
    /// canonical layer's visibility and deletion.
    aliases: &'a HashMap<ObjectId, ObjectId>,
    rewrites: HashMap<ObjectId, Vec<u8>>,
    visited_xobjects: HashSet<ObjectId>,
}

impl<'a> ContentFilter<'a> {
    fn new(
        document: &'a lopdf::Document,
        deleted: &'a HashSet<ObjectId>,
        aliases: &'a HashMap<ObjectId, ObjectId>,
    ) -> Self {
        Self {
            document,
            deleted,
            aliases,
            rewrites: HashMap::new(),
            visited_xobjects: HashSet::new(),
        }
    }

    /// Resolves an OCG reference to the canonical layer it mirrors.
    fn canonical(&self, id: ObjectId) -> ObjectId {
        self.aliases.get(&id).copied().unwrap_or(id)
    }

    fn is_deleted(&self, id: ObjectId) -> bool {
        self.deleted.contains(&self.canonical(id))
    }

    fn run(mut self) -> Result<HashMap<ObjectId, Vec<u8>>, CommandError> {
        let pages: Vec<ObjectId> = self.document.get_pages().values().copied().collect();
        for page_id in pages {
            self.check_annotations(page_id)?;
            let resources = page_resources(self.document, page_id);
            self.process_page_contents(page_id, &resources)?;
            self.process_xobjects(&resources, 0)?;
        }
        Ok(self.rewrites)
    }

    fn check_annotations(&self, page_id: ObjectId) -> Result<(), CommandError> {
        let annotations = self
            .document
            .get_page_annotations(page_id)
            .map_err(|error| {
                unsupported_error(format!(
                    "could not inspect the annotations on page {page_id:?}: {error}"
                ))
            })?;
        for annotation in annotations {
            let Ok(entry) = annotation.get(b"OC") else {
                continue;
            };
            if self.oc_entry_is_unsafe(entry)? {
                return Err(unsupported_error(
                    "an annotation references this layer and cannot be rewritten safely",
                ));
            }
        }
        Ok(())
    }

    fn process_page_contents(
        &mut self,
        page_id: ObjectId,
        resources: &[&'a Dictionary],
    ) -> Result<(), CommandError> {
        let mut streams = Vec::new();
        for id in self.document.get_page_contents(page_id) {
            let Ok(object) = self.document.get_object(id) else {
                return Err(unsupported_error(format!(
                    "page {page_id:?} references a missing content stream"
                )));
            };
            let Ok(stream) = object.as_stream() else {
                return Err(unsupported_error(format!(
                    "page {page_id:?} has a content entry that is not a stream"
                )));
            };
            streams.push((id, decode_operations(stream, page_id)?));
        }
        self.filter_streams(streams, resources)
    }

    fn process_xobjects(
        &mut self,
        resources: &[&'a Dictionary],
        depth: usize,
    ) -> Result<(), CommandError> {
        if depth > MAX_FORM_DEPTH {
            return Ok(());
        }
        for resource in resources {
            let Ok(entry) = resource.get(b"XObject") else {
                continue;
            };
            let Ok((_, value)) = self.document.dereference(entry) else {
                continue;
            };
            let Ok(xobjects) = value.as_dict() else {
                continue;
            };
            for (_, xobject) in xobjects.iter() {
                match xobject {
                    Object::Reference(id) => {
                        if !self.visited_xobjects.insert(*id) {
                            continue;
                        }
                        let Ok(object) = self.document.get_object(*id) else {
                            continue;
                        };
                        let Ok(stream) = object.as_stream() else {
                            continue;
                        };
                        self.visit_xobject(Some(*id), stream, depth)?;
                    }
                    Object::Stream(stream) => {
                        self.visit_xobject(None, stream, depth)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn visit_xobject(
        &mut self,
        id: Option<ObjectId>,
        stream: &'a Stream,
        depth: usize,
    ) -> Result<(), CommandError> {
        if let Ok(entry) = stream.dict.get(b"OC") {
            if self.oc_entry_is_unsafe(entry)? {
                return Err(unsupported_error(
                    "a page object references this layer and cannot be rewritten safely",
                ));
            }
        }
        if stream
            .dict
            .get(b"Subtype")
            .ok()
            .and_then(|object| object.as_name().ok())
            != Some(b"Form")
        {
            return Ok(());
        }
        let resources = xobject_resources(self.document, &stream.dict);
        let operations = decode_operations(stream, id.unwrap_or((0, 0)))?;
        let before = self.rewrites.len();
        let single_id = id.unwrap_or((0, 0));
        self.filter_streams(vec![(single_id, operations)], &resources)?;
        if id.is_none() && self.rewrites.len() > before {
            return Err(unsupported_error(
                "a direct form XObject contains this layer and cannot be rewritten safely",
            ));
        }
        self.process_xobjects(&resources, depth + 1)
    }

    fn filter_streams(
        &mut self,
        streams: Vec<(ObjectId, Vec<Operation>)>,
        resources: &[&'a Dictionary],
    ) -> Result<(), CommandError> {
        let mut stack: Vec<Marker> = Vec::new();
        let mut any_removed = false;
        for (id, operations) in streams {
            let mut kept = Vec::with_capacity(operations.len());
            let mut changed = false;
            for operation in operations {
                match operation.operator.as_str() {
                    "BDC" => {
                        let parent_removed = stack.last() == Some(&Marker::Removed);
                        let removed = if parent_removed {
                            true
                        } else {
                            match self.bdc_matches_deleted(&operation, resources)? {
                                OcMatch::Match => true,
                                OcMatch::Unsafe => {
                                    return Err(unsupported_error(
                                        "a marked-content property uses a layer policy PDFX \
                                         cannot rewrite safely",
                                    ))
                                }
                                OcMatch::No => false,
                            }
                        };
                        stack.push(if removed {
                            Marker::Removed
                        } else {
                            Marker::Kept
                        });
                        if removed {
                            changed = true;
                        } else {
                            kept.push(operation);
                        }
                    }
                    "BMC" => {
                        let removed = stack.last() == Some(&Marker::Removed);
                        stack.push(if removed {
                            Marker::Removed
                        } else {
                            Marker::Kept
                        });
                        if removed {
                            changed = true;
                        } else {
                            kept.push(operation);
                        }
                    }
                    "EMC" => match stack.pop() {
                        Some(Marker::Removed) => changed = true,
                        Some(Marker::Kept) => kept.push(operation),
                        // A stray EMC is malformed but not something a layer
                        // removal needs to change.
                        None => kept.push(operation),
                    },
                    _ => {
                        if stack.last() == Some(&Marker::Removed) {
                            changed = true;
                        } else {
                            kept.push(operation);
                        }
                    }
                }
            }
            if changed {
                any_removed = true;
                let encoded = Content { operations: kept }.encode().map_err(|error| {
                    unsupported_error(format!("could not rewrite a content stream: {error}"))
                })?;
                self.rewrites.insert(id, encoded);
            }
        }
        if any_removed && !stack.is_empty() {
            return Err(unsupported_error(
                "a marked-content block in this layer is not balanced across the page content",
            ));
        }
        Ok(())
    }

    fn bdc_matches_deleted(
        &self,
        operation: &Operation,
        resources: &[&'a Dictionary],
    ) -> Result<OcMatch, CommandError> {
        let Some(property) = operation.operands.get(1) else {
            return Ok(OcMatch::No);
        };
        match property {
            Object::Name(name) => {
                let Some(value) = named_property(self.document, resources, name) else {
                    return Ok(OcMatch::No);
                };
                self.oc_value_match(value)
            }
            Object::Reference(id) => {
                let Ok(object) = self.document.get_object(*id) else {
                    return Ok(OcMatch::No);
                };
                let Ok(dictionary) = object.as_dict() else {
                    return Ok(OcMatch::No);
                };
                if dictionary.has_type(b"OCG") {
                    Ok(if self.is_deleted(*id) {
                        OcMatch::Match
                    } else {
                        OcMatch::No
                    })
                } else if dictionary.has(b"OCGs") {
                    Ok(if self.ocmd_contains(dictionary)? {
                        OcMatch::Unsafe
                    } else {
                        OcMatch::No
                    })
                } else {
                    Ok(OcMatch::No)
                }
            }
            Object::Dictionary(dictionary) => self.inline_dictionary_match(dictionary),
            _ => Ok(OcMatch::No),
        }
    }

    fn oc_value_match(&self, value: &Object) -> Result<OcMatch, CommandError> {
        match value {
            Object::Reference(id) => {
                let Ok(object) = self.document.get_object(*id) else {
                    return Ok(OcMatch::No);
                };
                let Ok(dictionary) = object.as_dict() else {
                    return Ok(OcMatch::No);
                };
                if dictionary.has_type(b"OCG") {
                    Ok(if self.is_deleted(*id) {
                        OcMatch::Match
                    } else {
                        OcMatch::No
                    })
                } else if dictionary.has(b"OCGs") {
                    Ok(if self.ocmd_contains(dictionary)? {
                        OcMatch::Unsafe
                    } else {
                        OcMatch::No
                    })
                } else {
                    Ok(OcMatch::No)
                }
            }
            Object::Dictionary(dictionary) => self.inline_dictionary_match(dictionary),
            _ => Ok(OcMatch::No),
        }
    }

    fn inline_dictionary_match(&self, dictionary: &Dictionary) -> Result<OcMatch, CommandError> {
        if dictionary.has_type(b"OCG") {
            // A direct OCG dictionary is a separate object from the indirect
            // one in /OCGs, so it cannot be the layer being deleted.
            return Ok(OcMatch::No);
        }
        if !dictionary.has(b"OCGs") {
            return Ok(OcMatch::No);
        }
        if !self.ocmd_contains(dictionary)? {
            return Ok(OcMatch::No);
        }
        if dictionary.has(b"P") || dictionary.has(b"VE") {
            Ok(OcMatch::Unsafe)
        } else {
            Ok(OcMatch::Match)
        }
    }

    fn ocmd_contains(&self, dictionary: &Dictionary) -> Result<bool, CommandError> {
        let Ok(entry) = dictionary.get(b"OCGs") else {
            return Ok(false);
        };
        let Ok((_, value)) = self.document.dereference(entry) else {
            return Ok(false);
        };
        let Ok(array) = value.as_array() else {
            return Ok(false);
        };
        Ok(array.iter().any(|item| {
            item.as_reference()
                .map(|id| self.is_deleted(id))
                .unwrap_or(false)
        }))
    }

    fn oc_entry_is_unsafe(&self, entry: &Object) -> Result<bool, CommandError> {
        match entry {
            Object::Reference(id) => {
                let Ok(object) = self.document.get_object(*id) else {
                    return Ok(false);
                };
                let Ok(dictionary) = object.as_dict() else {
                    return Ok(false);
                };
                if dictionary.has_type(b"OCG") {
                    Ok(self.is_deleted(*id))
                } else if dictionary.has(b"OCGs") {
                    self.ocmd_contains(dictionary)
                } else {
                    Ok(false)
                }
            }
            Object::Dictionary(dictionary) => {
                if dictionary.has(b"OCGs") {
                    self.ocmd_contains(dictionary)
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false),
        }
    }
}

fn page_resources(document: &lopdf::Document, page_id: ObjectId) -> Vec<&Dictionary> {
    let (direct, ids) = document
        .get_page_resources(page_id)
        .unwrap_or((None, Vec::new()));
    let mut resources = Vec::new();
    if let Some(direct) = direct {
        resources.push(direct);
    }
    for id in ids {
        if let Ok(dictionary) = document.get_dictionary(id) {
            resources.push(dictionary);
        }
    }
    resources
}

fn xobject_resources<'a>(
    document: &'a lopdf::Document,
    dictionary: &'a Dictionary,
) -> Vec<&'a Dictionary> {
    let Ok(entry) = dictionary.get(b"Resources") else {
        return Vec::new();
    };
    match entry {
        Object::Dictionary(direct) => vec![direct],
        Object::Reference(id) => document
            .get_dictionary(*id)
            .map(|dictionary| vec![dictionary])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn named_property<'a>(
    document: &'a lopdf::Document,
    resources: &[&'a Dictionary],
    name: &[u8],
) -> Option<&'a Object> {
    for resource in resources {
        let Ok(entry) = resource.get(b"Properties") else {
            continue;
        };
        let Ok((_, value)) = document.dereference(entry) else {
            continue;
        };
        let Ok(properties) = value.as_dict() else {
            continue;
        };
        if let Ok(value) = properties.get(name) {
            return Some(value);
        }
    }
    None
}

fn decode_operations(stream: &Stream, context: ObjectId) -> Result<Vec<Operation>, CommandError> {
    let bytes = stream
        .decompressed_content_with_limit(MAX_LAYER_CONTENT_BYTES)
        .map_err(|error| {
            unsupported_error(format!(
                "could not decode content stream {context:?}: {error}"
            ))
        })?;
    let content = Content::decode(&bytes).map_err(|error| {
        unsupported_error(format!(
            "could not parse content stream {context:?}: {error}"
        ))
    })?;
    Ok(content.operations)
}

fn apply_content_rewrites(
    incremental: &mut IncrementalDocument,
    rewrites: HashMap<ObjectId, Vec<u8>>,
) -> Result<(), CommandError> {
    for (id, plain) in rewrites {
        incremental
            .opt_clone_object_to_new_document(id)
            .map_err(|error| {
                internal_error(format!("could not clone a content stream: {error}"))
            })?;
        let object = incremental
            .new_document
            .get_object_mut(id)
            .map_err(|error| internal_error(format!("could not edit a content stream: {error}")))?;
        let stream = object
            .as_stream_mut()
            .map_err(|error| internal_error(format!("a content entry is not a stream: {error}")))?;
        stream.set_plain_content(plain);
        stream.compress().map_err(|error| {
            internal_error(format!(
                "could not compress a rewritten content stream: {error}"
            ))
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn unsupported_error(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::LayerDeleteUnsupported, message)
}

fn internal_error(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::Internal, message)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../fixtures/pdfs/layers.pdf");

    fn stream_object(data: &[u8]) -> Vec<u8> {
        let mut body = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        body
    }

    /// A one-page PDF with one layer (object 10) whose `/OC /MC0` property is
    /// object 10. `annotate` adds a text annotation carrying `/OC 10 0 R`.
    fn layer_pdf(content: &[u8], annotate: bool) -> Vec<u8> {
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
             /Resources << /Properties << /MC0 10 0 R >> >> /Contents 4 0 R{} >>",
            if annotate { " /Annots [11 0 R]" } else { "" }
        );
        let mut objects = vec![
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
            ),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
            (3, page.into_bytes()),
            (4, stream_object(content)),
            (
                7,
                b"<< /OCGs [10 0 R] /D << /Order [10 0 R] /ON [10 0 R] >> >>".to_vec(),
            ),
            (10, b"<< /Type /OCG /Name (Layer) >>".to_vec()),
        ];
        if annotate {
            objects.push((
                11,
                b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /OC 10 0 R >>".to_vec(),
            ));
        }
        build_test_pdf(&objects)
    }

    fn build_test_pdf(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let max = objects.iter().map(|(number, _)| *number).max().unwrap_or(0);
        let mut output = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![0usize; max as usize + 1];
        for number in 1..=max {
            offsets[number as usize] = output.len();
            output.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
            match objects.iter().find(|(candidate, _)| *candidate == number) {
                Some((_, body)) => output.extend_from_slice(body),
                None => output.extend_from_slice(b"null"),
            }
            output.extend_from_slice(b"\nendobj\n");
        }
        let xref = output.len();
        output.extend_from_slice(format!("xref\n0 {}\n", max + 1).as_bytes());
        output.extend_from_slice(b"0000000000 65535 f \n");
        for number in 1..=max {
            output.extend_from_slice(
                format!("{:010} 00000 n \n", offsets[number as usize]).as_bytes(),
            );
        }
        output.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                max + 1
            )
            .as_bytes(),
        );
        output
    }

    fn single_layer_state(deleted: bool) -> LayerSaveState {
        LayerSaveState {
            records: vec![LayerSaveRecord {
                index: 0,
                name: "Layer".to_owned(),
                visible: true,
                deleted,
            }],
        }
    }

    #[test]
    fn reads_utf16_names_and_explicit_visibility() {
        let document = lopdf::Document::load_mem(FIXTURE).expect("fixture parses");
        let records = read_layer_records(&document).expect("layers are present");

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].name, "红色图层");
        assert!(records[0].visible);
        assert_eq!(records[1].name, "绿色图层");
        assert!(records[1].visible);
        assert_eq!(records[2].name, "蓝色图层");
        assert!(!records[2].visible, "blue is listed in /OFF");
    }

    #[test]
    fn saving_rewrites_visibility_and_preserves_unknown_config_keys() {
        let state = LayerSaveState {
            records: vec![
                LayerSaveRecord {
                    index: 0,
                    name: "红色图层".to_owned(),
                    visible: false,
                    deleted: false,
                },
                LayerSaveRecord {
                    index: 1,
                    name: "绿色图层".to_owned(),
                    visible: true,
                    deleted: false,
                },
                LayerSaveRecord {
                    index: 2,
                    name: "蓝色图层".to_owned(),
                    visible: true,
                    deleted: false,
                },
            ],
        };
        let output = apply_layer_state_to_bytes(FIXTURE.to_vec(), &state).expect("edit applies");
        let document = lopdf::Document::load_mem(&output).expect("output parses");
        let catalog = document.catalog().expect("catalog");
        let ocp = catalog
            .get_deref(b"OCProperties", &document)
            .and_then(Object::as_dict)
            .expect("OCProperties");
        let config = ocp
            .get_deref(b"D", &document)
            .and_then(Object::as_dict)
            .expect("default config");

        assert_eq!(
            config.get(b"BaseState").and_then(Object::as_name).unwrap(),
            b"OFF"
        );
        assert_eq!(
            config.get(b"ListMode").and_then(Object::as_name).unwrap(),
            b"VisiblePages",
            "unknown /D keys survive"
        );
        assert!(!config.has(b"AS"), "/AS is dropped");
        let on = config.get(b"ON").and_then(Object::as_array).unwrap();
        let off = config.get(b"OFF").and_then(Object::as_array).unwrap();
        assert_eq!(
            on,
            &vec![Object::Reference((11, 0)), Object::Reference((12, 0))]
        );
        assert_eq!(off, &vec![Object::Reference((10, 0))]);
    }

    #[test]
    fn deleting_removes_nested_marked_content_and_keeps_the_rest() {
        // Delete blue (index 2); its nested block is inside the red block.
        let state = LayerSaveState {
            records: vec![
                LayerSaveRecord {
                    index: 0,
                    name: "红色图层".to_owned(),
                    visible: true,
                    deleted: false,
                },
                LayerSaveRecord {
                    index: 1,
                    name: "绿色图层".to_owned(),
                    visible: true,
                    deleted: false,
                },
                LayerSaveRecord {
                    index: 2,
                    name: "蓝色图层".to_owned(),
                    visible: true,
                    deleted: true,
                },
            ],
        };
        let output = apply_layer_state_to_bytes(FIXTURE.to_vec(), &state).expect("delete applies");
        let document = lopdf::Document::load_mem(&output).expect("output parses");
        let page_one = document.get_pages()[&1];
        let content = document.get_page_content(page_one);
        let text = String::from_utf8_lossy(&content);
        assert!(text.contains("1 0 0 rg"), "red content remains");
        assert!(!text.contains("0 0 1 rg"), "blue content is gone");
        assert_eq!(text.matches("BDC").count(), text.matches("EMC").count());

        let catalog = document.catalog().expect("catalog");
        let ocp = catalog
            .get_deref(b"OCProperties", &document)
            .and_then(Object::as_dict)
            .expect("OCProperties");
        let ocgs = ocp.get(b"OCGs").and_then(Object::as_array).unwrap();
        assert_eq!(ocgs.len(), 2, "deleted OCG is removed from /OCGs");
        let order = ocp
            .get_deref(b"D", &document)
            .and_then(Object::as_dict)
            .and_then(|config| config.get(b"Order"))
            .and_then(Object::as_array)
            .unwrap();
        assert_eq!(order.len(), 2, "deleted OCG is removed from /Order");
    }

    #[test]
    fn malformed_content_stream_refuses_deletion() {
        let bytes = layer_pdf(b"/OC /MC0 BDC\n(open\n", false);
        let error =
            apply_layer_state_to_bytes(bytes, &single_layer_state(true)).expect_err("refused");
        assert_eq!(error.code, CommandErrorCode::LayerDeleteUnsupported);
    }

    #[test]
    fn annotation_oc_reference_refuses_deletion() {
        let bytes = layer_pdf(b"", true);
        let error =
            apply_layer_state_to_bytes(bytes, &single_layer_state(true)).expect_err("refused");
        assert_eq!(error.code, CommandErrorCode::LayerDeleteUnsupported);
        assert!(error.message.contains("annotation"));
    }

    #[test]
    fn inline_ocgs_dictionary_marks_matching_blocks() {
        let bytes = layer_pdf(
            b"/OC << /OCGs [10 0 R] >> BDC\nq Q\nEMC\n/OC << /OCGs [11 0 R] >> BDC\nq Q\nEMC\n",
            false,
        );
        let output =
            apply_layer_state_to_bytes(bytes, &single_layer_state(true)).expect("delete applies");
        let document = lopdf::Document::load_mem(&output).expect("output parses");
        let page = document.get_pages()[&1];
        let text = String::from_utf8_lossy(&document.get_page_content(page)).into_owned();
        assert_eq!(text.matches("BDC").count(), 1);
        assert!(text.contains("11 0 R"));
    }

    #[test]
    fn inline_image_inside_removed_block_is_removed_without_corruption() {
        let content =
            b"/OC /MC0 BDC\nq Q\nBI /W 1 /H 1 /CS /G /BPC 8 ID \x00 EI\nEMC\n0 0 m 10 10 l S\n";
        let bytes = layer_pdf(content, false);
        let output =
            apply_layer_state_to_bytes(bytes, &single_layer_state(true)).expect("delete applies");
        let document = lopdf::Document::load_mem(&output).expect("output parses");
        let page = document.get_pages()[&1];
        let decoded = document
            .get_and_decode_page_content(page)
            .expect("rewritten content parses");
        assert!(decoded
            .operations
            .iter()
            .any(|operation| operation.operator == "S"));
        assert!(!decoded
            .operations
            .iter()
            .any(|operation| operation.operator == "BI"));
    }

    #[test]
    fn invalid_layer_name_is_rejected() {
        for name in ["", "   ", "a\nb", &"x".repeat(MAX_LAYER_NAME_CHARS + 1)] {
            let error = validate_layer_name(name).expect_err("invalid");
            assert_eq!(error.code, CommandErrorCode::LayerNameInvalid);
        }
        assert_eq!(validate_layer_name("  新图层  ").unwrap(), "新图层");
    }
}
