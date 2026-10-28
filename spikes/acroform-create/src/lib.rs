//! Spike-only AcroForm creation helpers built on `lopdf`.
//!
//! This crate exists to answer one question: can PDFX create interactive
//! AcroForm fields by appending a `lopdf::IncrementalDocument` revision to an
//! existing file, and will Preview/PDFKit, Chrome and PDFium show and fill the
//! result? It is deliberately not a production API.
//!
//! The generated file is always built the same way the save pipeline works:
//!
//! 1. start from a valid PDF (a blank page for creation, the source bytes for a
//!    merge);
//! 2. parse it with `lopdf`;
//! 3. append one incremental revision that clones only the catalog and page it
//!    touches, adds the widget/field/appearance objects, and leaves every other
//!    object byte-for-byte in the previous revision.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream};

/// The font resource name the generated fields use in `/DA` and appearance streams.
pub const FONT_RESOURCE: &str = "Helv";

/// Page geometry for a newly created form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    /// Degrees, clockwise, as `/Rotate`.
    pub rotate: i32,
    /// `[llx lly urx ury]`; `None` uses the MediaBox.
    pub crop_box: Option<[f32; 4]>,
}

/// One field to create. Coordinates are raw PDF default user-space coordinates
/// (`[llx lly urx ury]`), not screen coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSpec {
    pub name: String,
    pub tooltip: Option<String>,
    pub rect: [f32; 4],
    pub required: bool,
    pub font_size: f32,
    pub kind: FieldKind,
}

/// The field types the spike probes.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        value: String,
        multiline: bool,
    },
    Checkbox {
        checked: bool,
    },
    Radio {
        options: Vec<RadioOption>,
        selected: Option<usize>,
    },
    List {
        options: Vec<String>,
        selected: Vec<usize>,
    },
    Dropdown {
        options: Vec<String>,
        selected: Option<usize>,
    },
    Signature,
}

/// One radio button: its visible label and its raw widget rectangle.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioOption {
    pub label: String,
    pub rect: [f32; 4],
}

/// The bytes written and the final (uniquified) field names in spec order.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedForm {
    pub bytes: Vec<u8>,
    pub field_names: Vec<String>,
}

/// Converts a rectangle expressed in *displayed page* coordinates to the raw
/// default user-space rectangle a widget `/Rect` needs.
///
/// `crop_box` is the page's effective CropBox (or MediaBox when absent),
/// `rotate` is `/Rotate` (0, 90, 180, 270), and `rect` is
/// `[left bottom right top]` with the origin at the bottom-left of the
/// displayed, already-rotated and already-cropped page, in points.
///
/// This is the transform the editor needs before writing a widget for a
/// rectangle the user drew on screen. Skipping it is the classic AcroForm bug:
/// `/Rect` is raw user space, so on a rotated or offset-CropBox page the field
/// appears somewhere else (or off-page) in Preview/Chrome.
pub fn display_rect_to_user_space(
    crop_box: [f32; 4],
    rotate: i32,
    rect: [f32; 4],
) -> Result<[f32; 4]> {
    let [cx0, cy0, cx1, cy1] = normalized_rect(crop_box);
    let crop_width = cx1 - cx0;
    let crop_height = cy1 - cy0;
    if crop_width <= 0.0 || crop_height <= 0.0 {
        bail!("crop box must have a positive size");
    }
    let rotate = rotate.rem_euclid(360);
    if rotate % 90 != 0 {
        bail!("page /Rotate must be a multiple of 90, got {rotate}");
    }
    let [u0, v0, u1, v1] = normalized_rect(rect);

    let (width, height) = match rotate {
        0 | 180 => (crop_width, crop_height),
        90 | 270 => (crop_height, crop_width),
        _ => unreachable!(),
    };
    if u0 < 0.0 || v0 < 0.0 || u1 > width || v1 > height {
        bail!("display rectangle {rect:?} is outside the {width}x{height} displayed page");
    }

    let to_user = |u: f32, v: f32| -> (f32, f32) {
        match rotate {
            0 => (cx0 + u, cy0 + v),
            90 => (cx0 + crop_width - v, cy0 + u),
            180 => (cx0 + crop_width - u, cy0 + crop_height - v),
            270 => (cx0 + v, cy0 + crop_height - u),
            _ => unreachable!(),
        }
    };
    let (xa, ya) = to_user(u0, v0);
    let (xb, yb) = to_user(u1, v1);
    Ok([xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)])
}

/// Creates a one-page document with the requested fields.
pub fn create_form(page: PageSetup, fields: &[FieldSpec]) -> Result<GeneratedForm> {
    let base = blank_page(page)?;
    add_fields(&base, 0, fields)
}

/// Appends the requested fields to page `page_index` (0-based) of `base`.
///
/// Existing fields, page content and every untouched object stay in the
/// previous revision; only the catalog, the target page, and new field/appearance
/// objects are written by this revision.
pub fn add_fields(base: &[u8], page_index: usize, specs: &[FieldSpec]) -> Result<GeneratedForm> {
    for spec in specs {
        validate_spec(spec)?;
    }

    let previous = Document::load_mem(base).context("parse the base PDF with lopdf")?;
    let page_number = u32::try_from(page_index + 1).context("page index overflows u32")?;
    let page_id = *previous
        .get_pages()
        .get(&page_number)
        .with_context(|| format!("page {page_number} does not exist"))?;
    let root_id = previous
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .context("the base PDF trailer has no /Root reference")?;

    let existing_names = collect_existing_names(&previous, root_id)?;
    let mut used_names = existing_names;
    let mut field_names = Vec::with_capacity(specs.len());
    for spec in specs {
        field_names.push(unique_name(&spec.name, &mut used_names));
    }

    let mut incremental = IncrementalDocument::create_from(base.to_vec(), previous);

    // Clone the container objects this revision edits.
    incremental
        .opt_clone_object_to_new_document(root_id)
        .context("clone the catalog into the new revision")?;
    incremental
        .opt_clone_object_to_new_document(page_id)
        .context("clone the target page into the new revision")?;

    let font_id = ensure_helvetica_font(&mut incremental.new_document)?;
    let acroform_id = ensure_acroform(&mut incremental, root_id)?;
    let existing_fields = read_acroform_fields(&incremental.new_document, acroform_id)?;

    let mut new_field_refs = Vec::with_capacity(specs.len());
    let mut new_widget_refs = Vec::new();

    for (spec, name) in specs.iter().zip(field_names.iter()) {
        let widgets = match &spec.kind {
            FieldKind::Text { value, multiline } => {
                let field = make_text_field(
                    &mut incremental.new_document,
                    spec,
                    name,
                    value,
                    *multiline,
                    page_id,
                    font_id,
                )?;
                new_field_refs.push(Object::Reference(field));
                vec![field]
            }
            FieldKind::Checkbox { checked } => {
                let field = make_checkbox_field(
                    &mut incremental.new_document,
                    spec,
                    name,
                    *checked,
                    page_id,
                    font_id,
                )?;
                new_field_refs.push(Object::Reference(field));
                vec![field]
            }
            FieldKind::Radio { options, selected } => {
                let (field, kids) = make_radio_field(
                    &mut incremental.new_document,
                    spec,
                    name,
                    options,
                    *selected,
                    page_id,
                    font_id,
                )?;
                new_field_refs.push(Object::Reference(field));
                kids
            }
            FieldKind::List { options, selected } => {
                let field = make_choice_field(
                    &mut incremental.new_document,
                    spec,
                    name,
                    options,
                    DropdownState::List(selected),
                    page_id,
                    font_id,
                )?;
                new_field_refs.push(Object::Reference(field));
                vec![field]
            }
            FieldKind::Dropdown { options, selected } => {
                let field = make_choice_field(
                    &mut incremental.new_document,
                    spec,
                    name,
                    options,
                    DropdownState::Dropdown(*selected),
                    page_id,
                    font_id,
                )?;
                new_field_refs.push(Object::Reference(field));
                vec![field]
            }
            FieldKind::Signature => {
                let field =
                    make_signature_field(&mut incremental.new_document, spec, name, page_id)?;
                new_field_refs.push(Object::Reference(field));
                vec![field]
            }
        };
        new_widget_refs.extend(widgets.into_iter().map(Object::Reference));
    }

    append_acroform_entries(
        &mut incremental.new_document,
        root_id,
        acroform_id,
        font_id,
        &existing_fields,
        &new_field_refs,
        specs,
    )?;
    append_page_widgets(&mut incremental.new_document, page_id, &new_widget_refs)?;

    let mut bytes = Vec::new();
    incremental
        .save_to(&mut bytes)
        .context("save the lopdf incremental revision")?;

    Ok(GeneratedForm { bytes, field_names })
}

/// Flattens every AcroForm widget on every page into page content and removes
/// the form. This is a probe for the M3 "flatten copy" export, not a production
/// implementation: it relies on every widget having an appearance stream, and
/// it rewrites the document instead of appending a revision (an export copy is
/// allowed to do that).
///
/// Expected results and limits are recorded in the spike report:
/// - widgets without `/AP` (notably unsigned signature fields) are dropped;
/// - `/NeedAppearances true` on a source form means the file may not carry the
///   appearances the user saw, so flattening such a file can bake stale or empty
///   appearances;
/// - page rotation is irrelevant here because `/Rect` and the page content are
///   both raw user space; viewers rotate the flattened page exactly as before.
pub fn flatten_form(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut document = Document::load_mem(bytes).context("parse the form before flattening")?;
    let page_ids: Vec<ObjectId> = document.get_pages().values().copied().collect();
    let mut flattened = 0usize;

    for page_id in page_ids {
        let annots = {
            let page = document
                .get_object(page_id)
                .and_then(Object::as_dict)
                .context("page is not a dictionary")?;
            match page.get(b"Annots").ok() {
                None => Vec::new(),
                Some(annots) => document
                    .dereference(annots)
                    .map(|(_, object)| object)
                    .context("/Annots is a dangling reference")?
                    .as_array()
                    .context("/Annots is not an array")?
                    .clone(),
            }
        };

        let mut drawings: Vec<(String, ObjectId, [f32; 4])> = Vec::new();
        let mut remaining_annots = Vec::new();
        for annot in annots {
            let Some(annot_id) = annot.as_reference().ok() else {
                remaining_annots.push(annot);
                continue;
            };
            if !is_widget(&document, annot_id)? {
                remaining_annots.push(annot);
                continue;
            }
            flattened += 1;
            // A widget without an appearance (an unsigned signature field, for
            // example) has nothing to bake; remove it from the flattened copy.
            if let Some(widget) = widget_appearance(&document, annot_id)? {
                drawings.push(widget);
            }
        }

        if drawings.is_empty() {
            continue;
        }

        let mut operators = String::new();
        {
            let page = document
                .get_object(page_id)
                .and_then(Object::as_dict)
                .context("page is not a dictionary")?;
            let resources = page
                .get(b"Resources")
                .ok()
                .and_then(|object| document.dereference(object).ok())
                .and_then(|(_, object)| object.as_dict().ok())
                .cloned()
                .unwrap_or_default();
            let mut xobjects = resources
                .get(b"XObject")
                .ok()
                .and_then(|object| document.dereference(object).ok())
                .and_then(|(_, object)| object.as_dict().ok())
                .cloned()
                .unwrap_or_default();
            for (index, (name, xobject_id, rect)) in drawings.iter().enumerate() {
                let mut resource_name = name.clone();
                while xobjects.has(resource_name.as_bytes()) {
                    resource_name = format!("{name}_{}", index + 1);
                }
                xobjects.set(resource_name.as_bytes(), Object::Reference(*xobject_id));
                let bbox = xobject_bbox(&document, *xobject_id)?;
                operators.push_str(&placement_operators(&resource_name, bbox, *rect));
            }
            let mut resources = resources;
            resources.set("XObject", xobjects);
            let page = document
                .get_object_mut(page_id)
                .and_then(Object::as_dict_mut)
                .context("page is not a dictionary")?;
            page.set("Resources", resources);
            page.set("Annots", Object::Array(remaining_annots));
        }

        let mut content = document.get_page_content(page_id);
        content.extend_from_slice(operators.as_bytes());
        let mut stream = Stream::new(Dictionary::new(), content);
        stream
            .dict
            .set("Length", Object::Integer(stream.content.len() as i64));
        let content_id = document.add_object(stream);
        let page = document
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .context("page is not a dictionary")?;
        page.set("Contents", Object::Reference(content_id));
    }

    if flattened == 0 {
        bail!("the document has no widget annotations to flatten");
    }

    let catalog = document
        .catalog_mut()
        .context("catalog is not a dictionary")?;
    catalog.remove(b"AcroForm");

    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .context("save the flattened document")?;
    Ok(output)
}

fn is_widget(document: &Document, annot_id: ObjectId) -> Result<bool> {
    let annotation = document
        .get_object(annot_id)
        .and_then(Object::as_dict)
        .context("annotation is not a dictionary")?;
    Ok(annotation
        .get(b"Subtype")
        .and_then(Object::as_name)
        .map(|name| name == b"Widget")
        .unwrap_or(false))
}

/// Resolves a widget's current appearance stream (`/AP /N`, following `/AS` for
/// checkboxes and radio buttons). Returns `(unique resource prefix, stream id,
/// widget rect)` or `None` when the widget has no `/AP`.
fn widget_appearance(
    document: &Document,
    annot_id: ObjectId,
) -> Result<Option<(String, ObjectId, [f32; 4])>> {
    let annotation = document
        .get_object(annot_id)
        .and_then(Object::as_dict)
        .context("annotation is not a dictionary")?;
    let Some(rect) = annotation.get(b"Rect").ok().and_then(|object| {
        document
            .dereference(object)
            .ok()
            .and_then(|(_, object)| object.as_array().ok())
    }) else {
        return Ok(None);
    };
    let rect_values: Vec<f32> = rect
        .iter()
        .filter_map(|object| object.as_float().ok())
        .collect();
    if rect_values.len() != 4 {
        return Ok(None);
    }
    let rect = [
        rect_values[0],
        rect_values[1],
        rect_values[2],
        rect_values[3],
    ];

    let Some(ap) = annotation
        .get(b"AP")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .and_then(|(_, object)| object.as_dict().ok())
    else {
        return Ok(None);
    };
    let Some(normal) = ap.get(b"N").ok() else {
        return Ok(None);
    };
    let stream_id = if let Ok(reference) = normal.as_reference() {
        reference
    } else if let Some(states) = document
        .dereference(normal)
        .ok()
        .and_then(|(_, object)| object.as_dict().ok())
    {
        let state = annotation
            .get(b"AS")
            .and_then(Object::as_name)
            .unwrap_or(b"Off");
        let Some(entry) = states.get(state).ok().or_else(|| states.get(b"Off").ok()) else {
            return Ok(None);
        };
        entry
            .as_reference()
            .context("appearance state is not a stream reference")?
    } else {
        return Ok(None);
    };
    Ok(Some((format!("PdfxField{}", annot_id.0), stream_id, rect)))
}

fn xobject_bbox(document: &Document, xobject_id: ObjectId) -> Result<[f32; 4]> {
    let stream = document
        .get_object(xobject_id)
        .and_then(Object::as_stream)
        .context("appearance is not a stream")?;
    let bbox = stream
        .dict
        .get(b"BBox")
        .and_then(Object::as_array)
        .context("appearance stream has no /BBox")?;
    let values: Vec<f32> = bbox
        .iter()
        .filter_map(|object| object.as_float().ok())
        .collect();
    if values.len() != 4 {
        bail!("appearance /BBox is not a four-number array");
    }
    Ok([values[0], values[1], values[2], values[3]])
}

fn placement_operators(resource_name: &str, bbox: [f32; 4], rect: [f32; 4]) -> String {
    let [bx0, by0, bx1, by1] = normalized_rect(bbox);
    let [rx0, ry0, rx1, ry1] = normalized_rect(rect);
    let bbox_width = (bx1 - bx0).max(f32::EPSILON);
    let bbox_height = (by1 - by0).max(f32::EPSILON);
    let a = (rx1 - rx0) / bbox_width;
    let d = (ry1 - ry0) / bbox_height;
    let e = rx0 - bx0 * a;
    let f = ry0 - by0 * d;
    format!(
        "q {} 0 0 {} {} {} cm /{} Do Q\n",
        number(a),
        number(d),
        number(e),
        number(f),
        resource_name
    )
}

fn validate_spec(spec: &FieldSpec) -> Result<()> {
    if spec.name.trim().is_empty() {
        bail!("field name must not be empty");
    }
    let [x0, y0, x1, y1] = spec.rect;
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        bail!("field '{}' has a non-finite rectangle", spec.name);
    }
    if x1 <= x0 || y1 <= y0 {
        bail!(
            "field '{}' needs a positive rectangle, got {:?}",
            spec.name,
            spec.rect
        );
    }
    if !(spec.font_size > 0.0 && spec.font_size.is_finite()) {
        bail!("field '{}' needs a positive font size", spec.name);
    }
    Ok(())
}

/// Builds a minimal one-page PDF with no AcroForm. `add_fields` then appends the
/// form, so creation and merging exercise the same incremental path.
pub fn blank_page(page: PageSetup) -> Result<Vec<u8>> {
    if !(page.width > 0.0 && page.height > 0.0) {
        bail!("page size must be positive");
    }
    let mut document = Document::with_version("1.7");
    let catalog_id = document.add_object(dictionary([
        ("Type", Object::Name(b"Catalog".to_vec())),
        ("Pages", Object::Reference((2, 0))),
    ]));
    debug_assert_eq!(catalog_id, (1, 0));
    document.trailer.set("Root", Object::Reference(catalog_id));
    let pages_id = document.add_object(dictionary([
        ("Type", Object::Name(b"Pages".to_vec())),
        ("Kids", Object::Array(vec![Object::Reference((3, 0))])),
        ("Count", Object::Integer(1)),
    ]));
    debug_assert_eq!(pages_id, (2, 0));

    let mut page_dict = dictionary([
        ("Type", Object::Name(b"Page".to_vec())),
        ("Parent", Object::Reference(pages_id)),
        (
            "MediaBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(page.width),
                Object::Real(page.height),
            ]),
        ),
        ("Resources", Dictionary::new().into()),
        ("Contents", Object::Reference((4, 0))),
    ]);
    if page.rotate != 0 {
        page_dict.set("Rotate", Object::Integer(i64::from(page.rotate)));
    }
    if let Some([x0, y0, x1, y1]) = page.crop_box {
        page_dict.set(
            "CropBox",
            Object::Array(vec![
                Object::Real(x0),
                Object::Real(y0),
                Object::Real(x1),
                Object::Real(y1),
            ]),
        );
    }
    let page_id = document.add_object(page_dict);
    debug_assert_eq!(page_id, (3, 0));
    document.add_object(Stream::new(Dictionary::new(), Vec::new()));

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .context("serialize the blank page")?;
    Ok(bytes)
}

fn dictionary<const N: usize>(entries: [(&str, Object); N]) -> Dictionary {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        dictionary.set(key, value);
    }
    dictionary
}

fn ensure_helvetica_font(document: &mut Document) -> Result<ObjectId> {
    let font = dictionary([
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Helvetica".to_vec())),
        ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
    ]);
    Ok(document.add_object(font))
}

/// Makes sure the cloned catalog has an `/AcroForm` dictionary and returns the
/// object id that owns it: the catalog itself for a direct dictionary, or the
/// cloned indirect object's id.
fn ensure_acroform(incremental: &mut IncrementalDocument, root_id: ObjectId) -> Result<ObjectId> {
    let existing = incremental
        .new_document
        .get_object(root_id)
        .and_then(Object::as_dict)
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok())
        .cloned();

    match existing {
        None => {
            let catalog = incremental
                .new_document
                .get_object_mut(root_id)
                .and_then(Object::as_dict_mut)
                .context("catalog is not a dictionary")?;
            catalog.set("AcroForm", Dictionary::new());
            Ok(root_id)
        }
        Some(Object::Reference(id)) => {
            incremental
                .opt_clone_object_to_new_document(id)
                .with_context(|| format!("clone the /AcroForm object {id:?}"))?;
            Ok(id)
        }
        Some(Object::Dictionary(_)) => Ok(root_id),
        Some(other) => bail!("/AcroForm is not a dictionary: {other:?}"),
    }
}

/// The object that actually holds the AcroForm dictionary.
fn acroform_dict<'a>(
    document: &'a Document,
    acroform_id: ObjectId,
    root_id: ObjectId,
) -> Result<&'a Dictionary> {
    if acroform_id == root_id {
        document
            .get_object(root_id)
            .and_then(Object::as_dict)
            .context("catalog is not a dictionary")?
            .get(b"AcroForm")
            .and_then(Object::as_dict)
            .context("catalog /AcroForm is not a dictionary")
    } else {
        document
            .get_object(acroform_id)
            .and_then(Object::as_dict)
            .context("/AcroForm object is not a dictionary")
    }
}

fn acroform_dict_mut<'a>(
    document: &'a mut Document,
    acroform_id: ObjectId,
    root_id: ObjectId,
) -> Result<&'a mut Dictionary> {
    if acroform_id == root_id {
        document
            .get_object_mut(root_id)
            .and_then(Object::as_dict_mut)
            .context("catalog is not a dictionary")?
            .get_mut(b"AcroForm")
            .and_then(Object::as_dict_mut)
            .context("catalog /AcroForm is not a dictionary")
    } else {
        document
            .get_object_mut(acroform_id)
            .and_then(Object::as_dict_mut)
            .context("/AcroForm object is not a dictionary")
    }
}

/// Reads the existing top-level field references, resolving an indirect /Fields
/// array to a plain vector.
fn read_acroform_fields(document: &Document, acroform_id: ObjectId) -> Result<Vec<Object>> {
    let root_id = document
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .context("new revision trailer has no /Root")?;
    let acroform = acroform_dict(document, acroform_id, root_id)?;
    let Some(fields) = acroform.get(b"Fields").ok() else {
        return Ok(Vec::new());
    };
    let fields = document
        .dereference(fields)
        .map(|(_, object)| object)
        .context("/Fields is a dangling reference")?;
    Ok(fields
        .as_array()
        .context("/AcroForm /Fields is not an array")?
        .clone())
}

fn collect_existing_names(document: &Document, root_id: ObjectId) -> Result<HashSet<String>> {
    let catalog = document
        .get_object(root_id)
        .and_then(Object::as_dict)
        .context("catalog is not a dictionary")?;
    let Some(acroform) = catalog.get(b"AcroForm").ok() else {
        return Ok(HashSet::new());
    };
    let acroform = document
        .dereference(acroform)
        .map(|(_, object)| object)
        .context("/AcroForm is a dangling reference")?;
    let acroform = acroform
        .as_dict()
        .context("/AcroForm is not a dictionary")?;
    let mut names = HashSet::new();
    if let Ok(fields) = acroform.get(b"Fields") {
        let fields = document
            .dereference(fields)
            .map(|(_, object)| object)
            .context("/Fields is a dangling reference")?;
        if let Ok(fields) = fields.as_array() {
            for field in fields {
                collect_field_names(document, field, "", &mut names, 0)?;
            }
        }
    }
    Ok(names)
}

fn collect_field_names(
    document: &Document,
    field: &Object,
    prefix: &str,
    names: &mut HashSet<String>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        bail!("AcroForm field tree is deeper than 32 levels");
    }
    let field = document
        .dereference(field)
        .map(|(_, object)| object)
        .context("field is a dangling reference")?;
    let field = field.as_dict().context("field is not a dictionary")?;
    let partial = field
        .get(b"T")
        .ok()
        .and_then(|object| document.dereference(object).ok())
        .and_then(|(_, object)| object.as_str().ok())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .unwrap_or("");
    let full_name = if prefix.is_empty() {
        partial.to_owned()
    } else if partial.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}.{partial}")
    };
    if !full_name.is_empty() {
        names.insert(full_name.clone());
    }
    if let Ok(kids) = field.get(b"Kids") {
        let kids = document
            .dereference(kids)
            .map(|(_, object)| object)
            .context("/Kids is a dangling reference")?;
        if let Ok(kids) = kids.as_array() {
            for kid in kids {
                collect_field_names(document, kid, &full_name, names, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn unique_name(requested: &str, used: &mut HashSet<String>) -> String {
    if used.insert(requested.to_owned()) {
        return requested.to_owned();
    }
    for suffix in 2.. {
        let candidate = format!("{requested}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("u32 suffix space exhausted")
}

fn make_text_field(
    document: &mut Document,
    spec: &FieldSpec,
    name: &str,
    value: &str,
    multiline: bool,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(spec.rect);
    let appearance = text_appearance(
        document,
        [0.0, 0.0, x1 - x0, y1 - y0],
        value,
        spec.font_size,
        font_id,
    )?;

    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }
    if multiline {
        flags |= 1 << 12; // Multiline
    }

    let mut field = dictionary([
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(b"Widget".to_vec())),
        ("FT", Object::Name(b"Tx".to_vec())),
        ("T", text_string(name)),
        ("Rect", rect_array(spec.rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        (
            "DA",
            text_string(&format!(
                "/{FONT_RESOURCE} {} Tf 0 g",
                number(spec.font_size)
            )),
        ),
        ("AP", appearance_dictionary(appearance)),
        (
            "Border",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        ),
    ]);
    if let Some(tooltip) = &spec.tooltip {
        field.set("TU", text_string(tooltip));
    }
    if !value.is_empty() {
        field.set("V", text_string(value));
    }
    Ok(document.add_object(field))
}

enum DropdownState<'a> {
    List(&'a [usize]),
    Dropdown(Option<usize>),
}

fn make_signature_field(
    document: &mut Document,
    spec: &FieldSpec,
    name: &str,
    page_id: ObjectId,
) -> Result<ObjectId> {
    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }
    let mut field = dictionary([
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(b"Widget".to_vec())),
        ("FT", Object::Name(b"Sig".to_vec())),
        ("T", text_string(name)),
        ("Rect", rect_array(spec.rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        (
            "DA",
            text_string(&format!(
                "/{FONT_RESOURCE} {} Tf 0 g",
                number(spec.font_size)
            )),
        ),
        (
            "Border",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        ),
    ]);
    if let Some(tooltip) = &spec.tooltip {
        field.set("TU", text_string(tooltip));
    }
    // Deliberately no /V, /AP, /Lock or /SV: an empty signature field is just
    // its widget and field dictionary. Signing is explicitly out of scope.
    Ok(document.add_object(field))
}

fn make_choice_field(
    document: &mut Document,
    spec: &FieldSpec,
    name: &str,
    options: &[String],
    state: DropdownState<'_>,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(spec.rect);
    let bbox = [0.0, 0.0, x1 - x0, y1 - y0];

    let (is_combo, selected_indices, primary) = match &state {
        DropdownState::List(selected) => {
            for index in *selected {
                if *index >= options.len() {
                    bail!(
                        "list '{}' selected index {index} is out of range",
                        spec.name
                    );
                }
            }
            let primary = selected.iter().copied().min();
            (false, selected.to_vec(), primary)
        }
        DropdownState::Dropdown(selected) => {
            if let Some(index) = selected {
                if *index >= options.len() {
                    bail!(
                        "dropdown '{}' selected index {index} is out of range",
                        spec.name
                    );
                }
            }
            (true, selected.iter().copied().collect(), *selected)
        }
    };
    if is_combo && selected_indices.len() > 1 {
        bail!(
            "a combo box '{}' cannot have multiple selections",
            spec.name
        );
    }

    let appearance = choice_appearance(
        document,
        bbox,
        options,
        &selected_indices,
        is_combo,
        spec.font_size,
        font_id,
    )?;

    let mut flags = 0i64;
    if is_combo {
        flags |= 1 << 17; // Combo
    }
    if !is_combo && selected_indices.len() > 1 {
        flags |= 1 << 21; // MultiSelect
    }
    if spec.required {
        flags |= 1 << 1; // Required
    }

    let option_array: Vec<Object> = options.iter().map(|option| text_string(option)).collect();
    let mut field = dictionary([
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(b"Widget".to_vec())),
        ("FT", Object::Name(b"Ch".to_vec())),
        ("T", text_string(name)),
        ("Rect", rect_array(spec.rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        ("Opt", Object::Array(option_array)),
        (
            "DA",
            text_string(&format!(
                "/{FONT_RESOURCE} {} Tf 0 g",
                number(spec.font_size)
            )),
        ),
        ("AP", appearance_dictionary(appearance)),
        (
            "Border",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        ),
    ]);
    if let Some(primary) = primary {
        field.set("V", text_string(&options[primary]));
        field.set(
            "I",
            Object::Array(
                selected_indices
                    .iter()
                    .map(|index| Object::Integer(*index as i64))
                    .collect(),
            ),
        );
    }
    if let Some(tooltip) = &spec.tooltip {
        field.set("TU", text_string(tooltip));
    }
    Ok(document.add_object(field))
}

fn choice_appearance(
    document: &mut Document,
    bbox: [f32; 4],
    options: &[String],
    selected_indices: &[usize],
    is_combo: bool,
    font_size: f32,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(bbox);
    let width = x1 - x0;
    let height = y1 - y0;
    let mut content = format!(
        "q 1 1 1 rg 0 0 {w} {h} re f 0 0 0 RG {line} w 0.5 0.5 {bw} {bh} re S",
        w = number(width),
        h = number(height),
        line = number(0.75),
        bw = number((width - 1.0).max(0.0)),
        bh = number((height - 1.0).max(0.0)),
    );

    if is_combo {
        let text_size = font_size.min((height - 4.0).max(1.0));
        if let Some(index) = selected_indices.first() {
            content.push_str(&format!(
                " BT 0 0 0 rg /{FONT_RESOURCE} {size} Tf {x} {y} Td {text} Tj ET",
                size = number(text_size),
                x = number(3.0),
                y = number((height - text_size) / 2.0),
                text = pdf_literal(&options[*index]),
            ));
        }
        let arrow_width = (width * 0.12).clamp(6.0, 14.0);
        let ax = width - arrow_width - 3.0;
        let ay = height / 2.0;
        content.push_str(&format!(
            " 0 0 0 rg {ax} {y1} m {ax2} {y1} l {ax3} {y2} l f",
            ax = number(ax),
            ax2 = number(ax + arrow_width),
            ax3 = number(ax + arrow_width / 2.0),
            y1 = number(ay + arrow_width * 0.35),
            y2 = number(ay - arrow_width * 0.35),
        ));
    } else {
        let count = options.len().max(1) as f32;
        let line_height = (height / count).max(1.0);
        let text_size = font_size.min((line_height * 0.62).max(1.0));
        for (index, option) in options.iter().enumerate() {
            let top = height - index as f32 * line_height;
            let bottom = top - line_height;
            if selected_indices.contains(&index) {
                content.push_str(&format!(
                    " 0.20 0.45 0.85 rg 1 {bottom} {width_inner} {row_height} re f",
                    bottom = number(bottom),
                    width_inner = number((width - 2.0).max(0.0)),
                    row_height = number(line_height),
                ));
                content.push_str(&format!(
                    " BT 1 1 1 rg /{FONT_RESOURCE} {size} Tf {x} {y} Td {text} Tj ET",
                    size = number(text_size),
                    x = number(3.0),
                    y = number(bottom + (line_height - text_size) / 2.0),
                    text = pdf_literal(option),
                ));
            } else {
                content.push_str(&format!(
                    " BT 0 0 0 rg /{FONT_RESOURCE} {size} Tf {x} {y} Td {text} Tj ET",
                    size = number(text_size),
                    x = number(3.0),
                    y = number(bottom + (line_height - text_size) / 2.0),
                    text = pdf_literal(option),
                ));
            }
        }
    }
    content.push_str(" Q\n");
    form_xobject(document, bbox, content.into_bytes(), font_id)
}

fn make_radio_field(
    document: &mut Document,
    spec: &FieldSpec,
    name: &str,
    options: &[RadioOption],
    selected: Option<usize>,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<(ObjectId, Vec<ObjectId>)> {
    let parent_id = document
        .try_new_object_id()
        .context("allocate the radio parent field")?;
    let mut kid_ids = Vec::with_capacity(options.len());
    let mut kid_refs = Vec::with_capacity(options.len());

    for (index, option) in options.iter().enumerate() {
        let on_name = radio_export_value(index, &option.label);
        let [x0, y0, x1, y1] = normalized_rect(option.rect);
        let bbox = [0.0, 0.0, x1 - x0, y1 - y0];
        let off = radio_appearance(document, bbox, false, font_id)?;
        let on = radio_appearance(document, bbox, true, font_id)?;
        let mut normal = Dictionary::new();
        normal.set("Off", Object::Reference(off));
        normal.set(on_name.as_bytes(), Object::Reference(on));
        let mut appearance = Dictionary::new();
        appearance.set("N", normal);

        let is_selected = selected == Some(index);
        let mut kid = dictionary([
            ("Type", Object::Name(b"Annot".to_vec())),
            ("Subtype", Object::Name(b"Widget".to_vec())),
            ("Parent", Object::Reference(parent_id)),
            ("Rect", rect_array(option.rect)),
            ("P", Object::Reference(page_id)),
            ("F", Object::Integer(4)), // Print
            (
                "AS",
                Object::Name(if is_selected {
                    on_name.as_bytes().to_vec()
                } else {
                    b"Off".to_vec()
                }),
            ),
            ("AP", Object::Dictionary(appearance)),
            (
                "Border",
                Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(0),
                ]),
            ),
            (
                "MK",
                Object::Dictionary(dictionary([("CA", text_string("4"))])),
            ),
        ]);
        if let Some(tooltip) = &spec.tooltip {
            kid.set("TU", text_string(tooltip));
        }
        let kid_id = document.add_object(kid);
        kid_ids.push(kid_id);
        kid_refs.push(Object::Reference(kid_id));
    }

    let mut flags = 1 << 15; // Radio
    if spec.required {
        flags |= 1 << 1; // Required
    }
    let mut parent = dictionary([
        ("FT", Object::Name(b"Btn".to_vec())),
        ("T", text_string(name)),
        ("Ff", Object::Integer(flags)),
        ("Kids", Object::Array(kid_refs)),
    ]);
    if let Some(selected) = selected {
        let option = options
            .get(selected)
            .with_context(|| format!("radio '{}' selected index is out of range", spec.name))?;
        parent.set(
            "V",
            Object::Name(radio_export_value(selected, &option.label).into_bytes()),
        );
    }
    if let Some(tooltip) = &spec.tooltip {
        parent.set("TU", text_string(tooltip));
    }
    let options_array: Vec<Object> = options
        .iter()
        .map(|option| text_string(&option.label))
        .collect();
    parent.set("Opt", Object::Array(options_array));
    document.set_object(parent_id, parent);

    Ok((parent_id, kid_ids))
}

fn radio_export_value(index: usize, label: &str) -> String {
    let sanitized: String = label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if sanitized.is_empty() {
        format!("Choice{}", index + 1)
    } else {
        sanitized
    }
}

fn radio_appearance(
    document: &mut Document,
    bbox: [f32; 4],
    selected: bool,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(bbox);
    let width = x1 - x0;
    let height = y1 - y0;
    let radius = (width.min(height) / 2.0) - 1.0;
    let center_x = width / 2.0;
    let center_y = height / 2.0;
    let mut content = format!(
        "q 1 1 1 rg 0 0 {w} {h} re f 0 0 0 RG {line} w {} S",
        circle_path(center_x, center_y, radius),
        w = number(width),
        h = number(height),
        line = number(1.0),
    );
    if selected {
        content.push_str(&format!(
            " 0 0 0 rg {} f",
            circle_path(center_x, center_y, (radius * 0.55).max(0.5))
        ));
    }
    content.push_str(" Q\n");
    form_xobject(document, bbox, content.into_bytes(), font_id)
}

fn circle_path(center_x: f32, center_y: f32, radius: f32) -> String {
    let kappa = 0.552_284_8;
    let control = radius * kappa;
    format!(
        "{x0} {cy} m {x0} {cy1} {cx1} {y0} {cx} {y0} c {cx2} {y0} {x1} {cy1} {x1} {cy} c {x1} {cy2} {cx2} {y1} {cx} {y1} c {cx1} {y1} {x0} {cy2} {x0} {cy} c",
        x0 = number(center_x - radius),
        x1 = number(center_x + radius),
        cx = number(center_x),
        y0 = number(center_y - radius),
        y1 = number(center_y + radius),
        cy = number(center_y),
        cx1 = number(center_x - control),
        cx2 = number(center_x + control),
        cy1 = number(center_y - control),
        cy2 = number(center_y + control),
    )
}

fn make_checkbox_field(
    document: &mut Document,
    spec: &FieldSpec,
    name: &str,
    checked: bool,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(spec.rect);
    let bbox = [0.0, 0.0, x1 - x0, y1 - y0];
    let off = checkbox_appearance(document, bbox, false, font_id)?;
    let on = checkbox_appearance(document, bbox, true, font_id)?;

    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }

    let mut normal = Dictionary::new();
    normal.set("Off", Object::Reference(off));
    normal.set("Yes", Object::Reference(on));

    let mut appearance = Dictionary::new();
    appearance.set("N", normal);

    let state = if checked { "Yes" } else { "Off" };
    let mut field = dictionary([
        ("Type", Object::Name(b"Annot".to_vec())),
        ("Subtype", Object::Name(b"Widget".to_vec())),
        ("FT", Object::Name(b"Btn".to_vec())),
        ("T", text_string(name)),
        ("Rect", rect_array(spec.rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        ("V", Object::Name(state.as_bytes().to_vec())),
        ("AS", Object::Name(state.as_bytes().to_vec())),
        ("AP", Object::Dictionary(appearance)),
        (
            "Border",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        ),
    ]);
    if let Some(tooltip) = &spec.tooltip {
        field.set("TU", text_string(tooltip));
    }
    Ok(document.add_object(field))
}

fn checkbox_appearance(
    document: &mut Document,
    bbox: [f32; 4],
    checked: bool,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(bbox);
    let width = x1 - x0;
    let height = y1 - y0;
    let mut content = format!(
        "q 1 1 1 rg 0 0 {w} {h} re f 0 0 0 RG {line} w 0.5 0.5 {bw} {bh} re S",
        w = number(width),
        h = number(height),
        line = number(1.0),
        bw = number((width - 1.0).max(0.0)),
        bh = number((height - 1.0).max(0.0)),
    );
    if checked {
        content.push_str(&format!(
            " {line} w {x1} {y1} m {x2} {y2} l {x3} {y3} l S",
            line = number(1.6),
            x1 = number(width * 0.22),
            y1 = number(height * 0.52),
            x2 = number(width * 0.42),
            y2 = number(height * 0.28),
            x3 = number(width * 0.78),
            y3 = number(height * 0.72),
        ));
    }
    content.push_str(" Q\n");
    form_xobject(document, bbox, content.into_bytes(), font_id)
}

fn text_appearance(
    document: &mut Document,
    bbox: [f32; 4],
    value: &str,
    font_size: f32,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let [x0, y0, x1, y1] = normalized_rect(bbox);
    let width = x1 - x0;
    let height = y1 - y0;
    let text = if value.is_empty() {
        String::new()
    } else {
        format!(
            // Reset the fill colour after the white background; otherwise the
            // value is painted white-on-white in static appearances.
            "0 0 0 rg BT /{FONT_RESOURCE} {} Tf {} {} Td {} Tj ET",
            number(font_size),
            number(2.0),
            number((height - font_size) / 2.0),
            pdf_literal(value)
        )
    };
    let content = format!(
        "q 1 1 1 rg 0 0 {w} {h} re f 0 0 0 RG {line} w 0.5 0.5 {bw} {bh} re S q {text} Q Q\n",
        w = number(width),
        h = number(height),
        line = number(0.75),
        bw = number((width - 1.0).max(0.0)),
        bh = number((height - 1.0).max(0.0)),
    );
    form_xobject(document, bbox, content.into_bytes(), font_id)
}

fn form_xobject(
    document: &mut Document,
    bbox: [f32; 4],
    content: Vec<u8>,
    font_id: ObjectId,
) -> Result<ObjectId> {
    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set(FONT_RESOURCE, Object::Reference(font_id));
    resources.set("Font", fonts);

    let mut dictionary = dictionary([
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Form".to_vec())),
        ("FormType", Object::Integer(1)),
        ("BBox", rect_array(bbox)),
        ("Resources", Object::Dictionary(resources)),
    ]);
    dictionary.set("Length", Object::Integer(content.len() as i64));
    Ok(document.add_object(Stream::new(dictionary, content)))
}

fn append_acroform_entries(
    document: &mut Document,
    root_id: ObjectId,
    acroform_id: ObjectId,
    font_id: ObjectId,
    existing_fields: &[Object],
    new_field_refs: &[Object],
    specs: &[FieldSpec],
) -> Result<()> {
    // Read the existing resource/font dictionaries first so no immutable borrow
    // is held while the new revision is mutated.
    let (mut dr, mut fonts, has_da) = {
        let acroform = acroform_dict(document, acroform_id, root_id)?;
        let dr = acroform
            .get(b"DR")
            .ok()
            .and_then(|object| document.dereference(object).ok())
            .and_then(|(_, object)| object.as_dict().ok())
            .cloned()
            .unwrap_or_default();
        let fonts = dr
            .get(b"Font")
            .ok()
            .and_then(|object| document.dereference(object).ok())
            .and_then(|(_, object)| object.as_dict().ok())
            .cloned()
            .unwrap_or_default();
        let has_da = acroform.has(b"DA");
        (dr, fonts, has_da)
    };

    if !fonts.has(FONT_RESOURCE.as_bytes()) {
        fonts.set(FONT_RESOURCE, Object::Reference(font_id));
    }
    dr.set("Font", fonts);

    let mut fields: Vec<Object> = existing_fields.to_vec();
    fields.extend(new_field_refs.iter().cloned());

    let acroform = acroform_dict_mut(document, acroform_id, root_id)?;
    acroform.set("DR", dr);
    acroform.set("Fields", Object::Array(fields));
    if !has_da {
        acroform.set("DA", text_string(&format!("/{FONT_RESOURCE} 0 Tf 0 g")));
    }
    // Do not set /NeedAppearances. Every field carries a static appearance that
    // Chrome and PDFKit paint; asking readers to regenerate appearances makes
    // PDFium's form-fill path responsible for rendering them. An existing
    // setting on a merged form is preserved untouched.
    if specs
        .iter()
        .any(|spec| matches!(spec.kind, FieldKind::Signature))
    {
        // Bit 1: SignaturesExist. The empty signature field is a placeholder;
        // the spike never signs it and therefore does not set AppendOnly.
        let existing = acroform
            .get(b"SigFlags")
            .ok()
            .and_then(|object| object.as_i64().ok())
            .unwrap_or(0);
        acroform.set("SigFlags", Object::Integer(existing | 1));
    }
    Ok(())
}

fn append_page_widgets(
    document: &mut Document,
    page_id: ObjectId,
    new_widget_refs: &[Object],
) -> Result<()> {
    let existing = {
        let page = document
            .get_object(page_id)
            .and_then(Object::as_dict)
            .context("page is not a dictionary")?;
        match page.get(b"Annots").ok() {
            None => Vec::new(),
            Some(annots) => document
                .dereference(annots)
                .map(|(_, object)| object)
                .context("/Annots is a dangling reference")?
                .as_array()
                .context("/Annots is not an array")?
                .clone(),
        }
    };
    let mut annots = existing;
    annots.extend(new_widget_refs.iter().cloned());
    let page = document
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .context("page is not a dictionary")?;
    page.set("Annots", Object::Array(annots));
    Ok(())
}

fn appearance_dictionary(appearance_id: ObjectId) -> Object {
    let mut ap = Dictionary::new();
    ap.set("N", Object::Reference(appearance_id));
    ap.into()
}

fn normalized_rect(rect: [f32; 4]) -> [f32; 4] {
    let [x0, y0, x1, y1] = rect;
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

fn rect_array(rect: [f32; 4]) -> Object {
    let [x0, y0, x1, y1] = normalized_rect(rect);
    Object::Array(vec![
        Object::Real(x0),
        Object::Real(y0),
        Object::Real(x1),
        Object::Real(y1),
    ])
}

fn text_string(value: &str) -> Object {
    if value.is_ascii() {
        Object::string_literal(value.as_bytes().to_vec())
    } else {
        // PDF text strings outside ASCII must be UTF-16BE with a BOM; raw UTF-8
        // bytes would be interpreted as PDFDocEncoding and read back as mojibake.
        let mut bytes = vec![0xfe, 0xff];
        for unit in value.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Literal)
    }
}

fn number(value: f32) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

fn pdf_literal(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('(');
    for character in value.chars() {
        match character {
            '\\' | '(' | ')' => {
                output.push('\\');
                output.push(character);
            }
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_ascii() => output.push(character),
            // The spike uses the built-in Helvetica/WinAnsi encoding. Non-ASCII
            // field text needs the production font pipeline; fail loudly rather
            // than write a misleading appearance.
            character => output.push_str(&format!("\\{:03o}", u32::from(character) & 0xff)),
        }
    }
    output.push(')');
    output
}
