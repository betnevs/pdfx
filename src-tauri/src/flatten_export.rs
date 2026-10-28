//! Flattened-copy export.
//!
//! A flattened copy draws every visual annotation's normal appearance
//! (`/AP /N`) into the page content and removes the annotation from that copy.
//! Links and form widgets (including signature widgets) stay native and
//! interactive. The Source Document, the current Output Document, the live
//! PDFium Document and its undo history are never modified: the command
//! serializes the current live state into memory, edits the bytes with
//! `lopdf`, and writes a new file atomically.
//!
//! Appearance data is authoritative. The exported page references each
//! annotation's own form XObject from the page resources and paints it with
//! `/Name Do`; where the producer's `/BBox` and `/Matrix` do not already land
//! on `/Rect`, the standard viewer mapping from the transformed bounding box
//! to the annotation rectangle is applied first. Page `/Rotate` needs no
//! special handling: annotations and page content share the unrotated page
//! coordinate system, so a viewer rotates both together.
//!
//! Appearance-less annotations are removed with an explicit, reported policy:
//! popups (`/Popup`, which have no page appearance by definition) and hidden
//! annotations are omitted; foreign annotations without a usable form
//! appearance are omitted and counted rather than left to render
//! inconsistently. Nothing is ever left behind that could corrupt the copy.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::commands::{with_open_document, CommandError, CommandErrorCode};
use crate::logging;
use crate::target_path::same_file;

/// How many `/Parent` links the resource search follows before giving up.
const MAX_PARENT_DEPTH: usize = 64;
/// How many reference indirections a single value may have.
const MAX_REFERENCE_DEPTH: usize = 32;
/// Annotation `/F` bits that keep an annotation off the page.
const HIDDEN_FLAGS: i64 = 1 | 2 | 32;

/// Why an annotation was omitted from the flattened copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlattenSkipReason {
    /// A `/Popup` has no page appearance; it is dropped with its note.
    Popup,
    /// The annotation has no usable normal appearance stream.
    NoAppearance,
    /// The annotation's appearance is not a form XObject.
    UnsupportedAppearance,
    /// Invisible, hidden or no-view annotations are not painted by viewers.
    Hidden,
}

/// One annotation that was removed without being drawn into the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenSkipView {
    pub page_index: u32,
    /// The annotation's `/Subtype` (without the slash).
    pub subtype: String,
    pub reason: FlattenSkipReason,
}

/// The outcome of a flattened-copy export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenExportView {
    pub document_id: String,
    pub path: String,
    pub bytes: u64,
    /// Visual annotations drawn into page content.
    pub flattened: u32,
    /// Links and form widgets kept interactive in the copy.
    pub preserved: u32,
    /// Annotations removed without being drawn, with the reason.
    pub skipped: Vec<FlattenSkipView>,
}

/// The `export_flattened_copy` command.
///
/// Writes a new PDF in which visual annotations are page content and links,
/// form fields and signature widgets remain native. Encrypted Source Documents
/// follow the save pipeline's ADR-0007 rule: the copy is written unencrypted
/// only after `confirm_unencrypted_output` is true.
#[tauri::command]
pub fn export_flattened_copy(
    document_id: String,
    destination: String,
    confirm_unencrypted_output: bool,
) -> Result<FlattenExportView, CommandError> {
    let result = export_flattened_copy_impl(&document_id, &destination, confirm_unencrypted_output);
    match &result {
        Ok(view) => logging::event(
            "document.export_flattened_copy",
            &format!(
                "documentId={document_id} path={} bytes={} flattened={} preserved={} skipped={}",
                view.path,
                view.bytes,
                view.flattened,
                view.preserved,
                view.skipped.len()
            ),
        ),
        Err(error) => logging::error(
            "document.export_flattened_copy.failed",
            &format!("code={:?} message={}", error.code, error.message),
        ),
    }
    result
}

fn export_flattened_copy_impl(
    document_id: &str,
    destination: &str,
    confirm_unencrypted_output: bool,
) -> Result<FlattenExportView, CommandError> {
    if destination.trim().is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "the flattened-copy destination must not be empty",
        ));
    }
    let path = Path::new(destination);

    let (bytes, stats) = with_open_document(document_id, |entry| {
        ensure_target_is_not_open_document(
            &entry.source_path,
            entry.save.output_path.as_deref(),
            path,
        )?;

        // ADR-0007: an encrypted Source Document always produces an
        // unencrypted copy, and only after the user explicitly confirmed it.
        let (encrypted, _, _) = crate::commands::document_permissions(&entry.document);
        if encrypted && !confirm_unencrypted_output {
            return Err(CommandError::new(
                CommandErrorCode::UnencryptedOutputConfirmationRequired,
                "the Source Document is encrypted; the flattened copy is written unencrypted",
            ));
        }

        // Serialize the live state (including unsaved edits) without touching
        // the PDFium document, its history or the save baseline.
        let base = crate::save::serialize_live_document(entry)?;
        let flattened = flatten_document(base)?;
        Ok((flattened.bytes, flattened.stats))
    })?;

    crate::atomic_write::write_atomically(path, &bytes)?;

    Ok(FlattenExportView {
        document_id: document_id.to_owned(),
        path: path.to_string_lossy().into_owned(),
        bytes: bytes.len() as u64,
        flattened: stats.flattened,
        preserved: stats.preserved,
        skipped: stats.skipped,
    })
}

/// Refuses a target that names the open Source Document or the current Output
/// Document, including symlink aliases and case variants (`same_file`).
fn ensure_target_is_not_open_document(
    source: &Path,
    output: Option<&Path>,
    destination: &Path,
) -> Result<(), CommandError> {
    if same_file(source, destination) {
        return Err(CommandError::new(
            CommandErrorCode::ExportTargetIsSource,
            format!(
                "{} is the open Source Document; choose a different export file",
                destination.display()
            ),
        ));
    }
    if output.is_some_and(|output| same_file(output, destination)) {
        return Err(CommandError::new(
            CommandErrorCode::ExportTargetIsOutput,
            format!(
                "{} is the current Output Document; choose a different export file",
                destination.display()
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
struct FlattenStats {
    flattened: u32,
    preserved: u32,
    skipped: Vec<FlattenSkipView>,
}

impl FlattenStats {
    fn skip(&mut self, page_number: u32, subtype: &[u8], reason: FlattenSkipReason) {
        self.skipped.push(FlattenSkipView {
            page_index: page_number.saturating_sub(1),
            subtype: String::from_utf8_lossy(subtype).into_owned(),
            reason,
        });
    }
}

struct FlattenedDocument {
    bytes: Vec<u8>,
    stats: FlattenStats,
}

/// One appearance form XObject referenced from the page resources.
struct Drawing {
    name: Vec<u8>,
    xobject: lopdf::Object,
    /// Placement matrix to apply before `Do`, or `None` when the stream's own
    /// `/Matrix` already places it.
    matrix: Option<[f32; 6]>,
}

/// The changes a page needs. Pages without visual annotations never get a plan,
/// so their content and resources remain byte-identical to the input.
struct PagePlan {
    page_id: lopdf::ObjectId,
    retained_annots: Vec<lopdf::Object>,
    annots_changed: bool,
    drawings: Vec<Drawing>,
    resources: lopdf::Dictionary,
    contents: Option<lopdf::Object>,
}

/// Draws every visual annotation into page content and removes it from the
/// copy. `base` must be an unencrypted PDF (the save pipeline produces that
/// output for encrypted sources).
fn flatten_document(base: Vec<u8>) -> Result<FlattenedDocument, CommandError> {
    let document = lopdf::Document::load_mem(&base)
        .map_err(|error| flatten_error("could not parse the document for flattening", error))?;

    let mut stats = FlattenStats::default();
    let mut plans = Vec::new();
    let mut name_counter = 0u32;
    for (page_number, page_id) in document.get_pages() {
        if let Some(plan) = plan_page(
            &document,
            page_number,
            page_id,
            &mut stats,
            &mut name_counter,
        )? {
            plans.push(plan);
        }
    }

    // Nothing to flatten (for example a document with only links): return the
    // serialized bytes unchanged instead of appending an empty revision.
    if plans.is_empty() {
        return Ok(FlattenedDocument { bytes: base, stats });
    }

    let mut incremental = lopdf::IncrementalDocument::create_from(base, document);
    for plan in plans {
        apply_plan(&mut incremental, plan)?;
    }
    let mut bytes = Vec::new();
    incremental
        .save_to(&mut bytes)
        .map_err(|error| flatten_error("could not append the flattened revision", error))?;
    Ok(FlattenedDocument { bytes, stats })
}

/// Builds the page's changes, or returns `None` when the page keeps every
/// annotation (links and widgets only) and can stay untouched.
fn plan_page(
    document: &lopdf::Document,
    page_number: u32,
    page_id: lopdf::ObjectId,
    stats: &mut FlattenStats,
    name_counter: &mut u32,
) -> Result<Option<PagePlan>, CommandError> {
    let page = document
        .get_dictionary(page_id)
        .map_err(|error| flatten_error("could not read a page dictionary", error))?;
    let Ok(annots_object) = page.get(b"Annots") else {
        return Ok(None);
    };
    let Some(entries) = resolved_array(document, annots_object) else {
        // A malformed /Annots is left untouched rather than guessed at.
        return Ok(None);
    };
    if entries.is_empty() {
        return Ok(None);
    }

    let resources = effective_resources(document, page_id);
    let mut existing_names = xobject_names(&resources);
    let mut retained = Vec::with_capacity(entries.len());
    let mut drawings = Vec::new();
    let mut annots_changed = false;

    for entry in entries {
        let Some(annotation) = resolved_dictionary(document, entry) else {
            retained.push(entry.clone());
            continue;
        };
        let subtype = annotation
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .unwrap_or_default();

        if subtype == b"Link" || subtype == b"Widget" {
            retained.push(entry.clone());
            stats.preserved += 1;
            continue;
        }
        if subtype == b"Popup" {
            stats.skip(page_number, subtype, FlattenSkipReason::Popup);
            annots_changed = true;
            continue;
        }
        if is_hidden(annotation) {
            stats.skip(page_number, subtype, FlattenSkipReason::Hidden);
            annots_changed = true;
            continue;
        }

        match normal_appearance(document, annotation) {
            Some(appearance) => {
                let Some(matrix) = placement_for(document, annotation, &appearance.stream) else {
                    stats.skip(
                        page_number,
                        subtype,
                        FlattenSkipReason::UnsupportedAppearance,
                    );
                    annots_changed = true;
                    continue;
                };
                let name = unique_xobject_name(name_counter, &mut existing_names);
                drawings.push(Drawing {
                    name,
                    xobject: appearance.object,
                    matrix,
                });
                stats.flattened += 1;
                annots_changed = true;
            }
            None => {
                stats.skip(page_number, subtype, FlattenSkipReason::NoAppearance);
                annots_changed = true;
            }
        }
    }

    if drawings.is_empty() && !annots_changed {
        return Ok(None);
    }

    Ok(Some(PagePlan {
        page_id,
        retained_annots: retained,
        annots_changed,
        drawings,
        resources,
        contents: page.get(b"Contents").ok().cloned(),
    }))
}

/// Writes one page's plan into the appended revision.
fn apply_plan(
    incremental: &mut lopdf::IncrementalDocument,
    plan: PagePlan,
) -> Result<(), CommandError> {
    // Direct appearance streams (non-conforming but seen in the wild) must
    // become indirect objects to be referenced from page resources.
    let mut placements = Vec::with_capacity(plan.drawings.len());
    for drawing in &plan.drawings {
        let object = match &drawing.xobject {
            lopdf::Object::Stream(stream) => {
                let id = incremental
                    .new_document
                    .try_new_object_id()
                    .map_err(|error| {
                        flatten_error("could not allocate an appearance object", error)
                    })?;
                incremental
                    .new_document
                    .set_object(id, lopdf::Object::Stream(stream.clone()));
                lopdf::Object::Reference(id)
            }
            other => other.clone(),
        };
        placements.push((drawing.name.clone(), object));
    }

    let content_id = incremental
        .new_document
        .try_new_object_id()
        .map_err(|error| flatten_error("could not allocate a content stream", error))?;
    incremental.new_document.set_object(
        content_id,
        lopdf::Object::Stream(lopdf::Stream::new(
            lopdf::Dictionary::new(),
            drawing_content(&plan.drawings),
        )),
    );

    let mut resources = plan.resources;
    let mut xobjects = match resources.get(b"XObject") {
        Ok(lopdf::Object::Dictionary(dictionary)) => dictionary.clone(),
        _ => lopdf::Dictionary::new(),
    };
    for (name, object) in placements {
        xobjects.set(name, object);
    }
    resources.set(b"XObject".to_vec(), lopdf::Object::Dictionary(xobjects));

    let mut contents = resolve_contents(plan.contents.as_ref(), incremental.get_prev_documents())?;
    // A direct content stream in an array is illegal; move it to an object.
    for item in &mut contents {
        if let lopdf::Object::Stream(stream) = item {
            let id = incremental
                .new_document
                .try_new_object_id()
                .map_err(|error| flatten_error("could not allocate a content stream", error))?;
            incremental
                .new_document
                .set_object(id, lopdf::Object::Stream(stream.clone()));
            *item = lopdf::Object::Reference(id);
        }
    }
    contents.push(lopdf::Object::Reference(content_id));

    incremental
        .opt_clone_object_to_new_document(plan.page_id)
        .map_err(|error| flatten_error("could not clone a page for flattening", error))?;
    let page = incremental
        .new_document
        .get_dictionary_mut(plan.page_id)
        .map_err(|error| flatten_error("could not edit a page for flattening", error))?;

    if plan.annots_changed {
        if plan.retained_annots.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set(
                b"Annots".to_vec(),
                lopdf::Object::Array(plan.retained_annots),
            );
        }
    }
    page.set(b"Resources".to_vec(), lopdf::Object::Dictionary(resources));
    page.set(b"Contents".to_vec(), lopdf::Object::Array(contents));

    Ok(())
}

/// The drawing operators for one page: one isolated `q ... Q` per form so a
/// malformed stream cannot leak graphics state into its neighbours.
fn drawing_content(drawings: &[Drawing]) -> Vec<u8> {
    let mut content = String::from("q\n");
    for drawing in drawings {
        content.push_str("q\n");
        if let Some(matrix) = drawing.matrix {
            if !is_identity(matrix) {
                content.push_str(&format!(
                    "{} {} {} {} {} {} cm\n",
                    matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]
                ));
            }
        }
        content.push('/');
        content.push_str(&String::from_utf8_lossy(&drawing.name));
        content.push_str(" Do\nQ\n");
    }
    content.push_str("Q\n");
    content.into_bytes()
}

/// The annotation's normal appearance stream, together with the object to
/// reference from page resources.
struct NormalAppearance {
    object: lopdf::Object,
    stream: lopdf::Stream,
}

/// Resolves `/AP /N` (a stream, or a state dictionary selected by `/AS` or the
/// first non-`Off` state) to a drawable form XObject.
fn normal_appearance(
    document: &lopdf::Document,
    annotation: &lopdf::Dictionary,
) -> Option<NormalAppearance> {
    let appearance = resolved_dictionary(document, annotation.get(b"AP").ok()?)?;
    let normal = appearance.get(b"N").ok()?;
    match resolved_object(document, normal)? {
        lopdf::Object::Stream(_) => appearance_stream(document, normal),
        lopdf::Object::Dictionary(states) => {
            let selected = annotation
                .get(b"AS")
                .and_then(lopdf::Object::as_name)
                .ok()
                .and_then(|state| states.get(state).ok())
                .and_then(|value| appearance_stream(document, value))
                .or_else(|| {
                    states.iter().find_map(|(state, value)| {
                        (state.as_slice() != b"Off")
                            .then(|| appearance_stream(document, value))
                            .flatten()
                    })
                });
            selected
        }
        _ => None,
    }
}

/// Resolves one appearance entry to a usable form stream.
fn appearance_stream(
    document: &lopdf::Document,
    object: &lopdf::Object,
) -> Option<NormalAppearance> {
    let reference = match object {
        lopdf::Object::Reference(id) => Some(lopdf::Object::Reference(*id)),
        _ => None,
    };
    let resolved = resolved_object(document, object)?;
    let stream = resolved.as_stream().ok()?;
    if !is_form_xobject(&stream.dict) {
        return None;
    }
    // A direct stream has no object id; the caller adds it to the revision.
    Some(NormalAppearance {
        object: reference.unwrap_or_else(|| lopdf::Object::Stream(stream.clone())),
        stream: stream.clone(),
    })
}

fn is_form_xobject(dictionary: &lopdf::Dictionary) -> bool {
    match dictionary.get(b"Subtype").and_then(lopdf::Object::as_name) {
        Ok(subtype) => subtype == b"Form",
        // Tolerate producers that omit /Subtype on an appearance stream.
        Err(_) => true,
    }
}

/// Returns the matrix that maps the appearance's transformed bounding box onto
/// the annotation rectangle, as viewers do, or `None` when the appearance has
/// no usable `/BBox`.
///
/// PDFium's own appearance streams use `/BBox` equal to `/Rect` and no
/// `/Matrix`, which makes the result the identity matrix.
fn placement_for(
    document: &lopdf::Document,
    annotation: &lopdf::Dictionary,
    stream: &lopdf::Stream,
) -> Option<Option<[f32; 6]>> {
    let bbox = stream
        .dict
        .get(b"BBox")
        .ok()
        .and_then(|object| number_array(document, object, 4))?;
    let bbox = normalized_rect(&bbox);
    let matrix = stream
        .dict
        .get(b"Matrix")
        .ok()
        .and_then(|object| number_array(document, object, 6))
        .map(|values| {
            [
                values[0], values[1], values[2], values[3], values[4], values[5],
            ]
        })
        .unwrap_or(IDENTITY);

    let Some(rect) = annotation
        .get(b"Rect")
        .ok()
        .and_then(|object| number_array(document, object, 4))
    else {
        return Some(None);
    };
    let rect = normalized_rect(&rect);
    let bounds = transformed_bounds(bbox, matrix);
    let width = bounds[2] - bounds[0];
    let height = bounds[3] - bounds[1];
    if width.abs() < 1e-6 || height.abs() < 1e-6 {
        // Degenerate bounding box: paint the form with its own matrix.
        return Some(None);
    }
    let scale_x = (rect[2] - rect[0]) / width;
    let scale_y = (rect[3] - rect[1]) / height;
    if !scale_x.is_finite() || !scale_y.is_finite() {
        return Some(None);
    }
    let to_origin = [1.0, 0.0, 0.0, 1.0, -bounds[0], -bounds[1]];
    let scale = [scale_x, 0.0, 0.0, scale_y, 0.0, 0.0];
    let to_rect = [1.0, 0.0, 0.0, 1.0, rect[0], rect[1]];
    Some(Some(multiply(
        to_rect,
        multiply(scale, multiply(to_origin, matrix)),
    )))
}

const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn is_identity(matrix: [f32; 6]) -> bool {
    matrix == IDENTITY
}

/// Composes two PDF matrices (`left` applied after `right`).
fn multiply(left: [f32; 6], right: [f32; 6]) -> [f32; 6] {
    [
        left[0] * right[0] + left[2] * right[1],
        left[1] * right[0] + left[3] * right[1],
        left[0] * right[2] + left[2] * right[3],
        left[1] * right[2] + left[3] * right[3],
        left[0] * right[4] + left[2] * right[5] + left[4],
        left[1] * right[4] + left[3] * right[5] + left[5],
    ]
}

fn transformed_bounds(bbox: [f32; 4], matrix: [f32; 6]) -> [f32; 4] {
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
        (bbox[0], bbox[3]),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in corners {
        let next_x = matrix[0] * x + matrix[2] * y + matrix[4];
        let next_y = matrix[1] * x + matrix[3] * y + matrix[5];
        min_x = min_x.min(next_x);
        min_y = min_y.min(next_y);
        max_x = max_x.max(next_x);
        max_y = max_y.max(next_y);
    }
    [min_x, min_y, max_x, max_y]
}

fn normalized_rect(values: &[f32]) -> [f32; 4] {
    [
        values[0].min(values[2]),
        values[1].min(values[3]),
        values[0].max(values[2]),
        values[1].max(values[3]),
    ]
}

/// Reads an array of `length` numbers, dereferencing the object first.
fn number_array(
    document: &lopdf::Document,
    object: &lopdf::Object,
    length: usize,
) -> Option<Vec<f32>> {
    let resolved = resolved_object(document, object)?;
    let array = resolved.as_array().ok()?;
    if array.len() != length {
        return None;
    }
    array
        .iter()
        .map(|item| match document.dereference(item) {
            Ok((_, value)) => value.as_float().ok(),
            Err(_) => None,
        })
        .collect()
}

fn is_hidden(annotation: &lopdf::Dictionary) -> bool {
    let flags = annotation
        .get(b"F")
        .and_then(lopdf::Object::as_i64)
        .ok()
        .unwrap_or(0);
    flags & HIDDEN_FLAGS != 0
}

/// Allocates a page-unique resource name for a form XObject.
fn unique_xobject_name(counter: &mut u32, existing: &mut HashSet<Vec<u8>>) -> Vec<u8> {
    loop {
        *counter = counter.wrapping_add(1);
        let name = format!("PDFXFlat{counter}");
        if existing.insert(name.clone().into_bytes()) {
            return name.into_bytes();
        }
    }
}

/// The keys of a resources dictionary's `/XObject` entry.
fn xobject_names(resources: &lopdf::Dictionary) -> HashSet<Vec<u8>> {
    match resources.get(b"XObject") {
        Ok(lopdf::Object::Dictionary(dictionary)) => {
            dictionary.iter().map(|(name, _)| name.clone()).collect()
        }
        _ => HashSet::new(),
    }
}

/// A copy of the page's effective resources with `/XObject` detached from any
/// shared dictionary, so adding forms cannot affect other pages. Resource
/// entries themselves stay shared references, which is normal PDF structure.
/// The search walks inherited `/Resources` entries through `/Parent`.
fn effective_resources(document: &lopdf::Document, page_id: lopdf::ObjectId) -> lopdf::Dictionary {
    let mut node_id = page_id;
    for _ in 0..MAX_PARENT_DEPTH {
        let Ok(node) = document.get_dictionary(node_id) else {
            break;
        };
        if let Ok(resources) = node.get(b"Resources") {
            if let Some(dictionary) = resolved_dictionary(document, resources) {
                let mut copy = dictionary.clone();
                let xobjects = dictionary
                    .get(b"XObject")
                    .ok()
                    .and_then(|object| resolved_dictionary(document, object))
                    .cloned()
                    .unwrap_or_default();
                copy.set(b"XObject".to_vec(), lopdf::Object::Dictionary(xobjects));
                return copy;
            }
        }
        let Ok(parent) = node.get(b"Parent").and_then(lopdf::Object::as_reference) else {
            break;
        };
        node_id = parent;
    }
    lopdf::Dictionary::new()
}

/// Resolves `/Annots` to its array, whether written directly or referenced.
fn resolved_array<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a [lopdf::Object]> {
    resolved_object(document, object)?
        .as_array()
        .ok()
        .map(|array| array.as_slice())
}

fn resolved_dictionary<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Dictionary> {
    resolved_object(document, object)?.as_dict().ok()
}

fn resolved_object<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Object> {
    let mut current = object;
    for _ in 0..MAX_REFERENCE_DEPTH {
        match current {
            lopdf::Object::Reference(id) => {
                current = document.get_object(*id).ok()?;
            }
            other => return Some(other),
        }
    }
    None
}

/// The contents streams of a page as an ordered list, expanding a referenced
/// array (which must not be modified in place) and leaving a referenced stream
/// referenced.
fn resolve_contents(
    existing: Option<&lopdf::Object>,
    document: &lopdf::Document,
) -> Result<Vec<lopdf::Object>, CommandError> {
    let Some(existing) = existing else {
        return Ok(Vec::new());
    };
    let mut current = existing;
    for _ in 0..MAX_REFERENCE_DEPTH {
        match current {
            lopdf::Object::Reference(id) => match document.get_object(*id) {
                Ok(lopdf::Object::Stream(_)) => {
                    return Ok(vec![lopdf::Object::Reference(*id)]);
                }
                Ok(lopdf::Object::Array(array)) => return Ok(array.clone()),
                Ok(lopdf::Object::Reference(_)) => {
                    current = document.get_object(*id).map_err(|error| {
                        flatten_error("could not resolve a page content stream", error)
                    })?;
                }
                _ => {
                    return Err(flatten_error(
                        "unsupported page /Contents entry",
                        format!("object {} has no stream or array", id.0),
                    ));
                }
            },
            lopdf::Object::Array(array) => return Ok(array.clone()),
            lopdf::Object::Stream(stream) => {
                return Ok(vec![lopdf::Object::Stream(stream.clone())]);
            }
            lopdf::Object::Null => return Ok(Vec::new()),
            other => {
                return Err(flatten_error(
                    "unsupported page /Contents entry",
                    format!("{other:?}"),
                ));
            }
        }
    }
    Err(flatten_error(
        "page /Contents reference chain is too deep",
        "reference limit reached",
    ))
}

fn flatten_error(context: &str, error: impl std::fmt::Display) -> CommandError {
    CommandError::new(
        CommandErrorCode::FlattenExportFailed,
        format!("{context}: {error}"),
    )
}
