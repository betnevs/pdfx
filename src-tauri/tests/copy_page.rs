//! Command-boundary tests for page duplication and its save/undo behaviour.
//!
//! A copied page must be a real, independent page: it renders immediately,
//! keeps the source content, links, rotation and size, and its annotations and
//! form widgets can be edited without changing the source page. Saved output is
//! parsed with lopdf independently of PDFium, so assertions describe the file an
//! external viewer reads.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::annotations::{
    create_shape_with_author, delete_annotation, list_annotations, AnnotationKind,
    CreateShapeRequest, ShapeGeometry, ShapeKind, ShapeStyle,
};
use pdfx_lib::commands::{
    close_document, extract_text, extract_text_layer, open_document, page_info, CommandErrorCode,
    DocumentView,
};
use pdfx_lib::form::{
    fill_form_field, form_fields, FormFieldKind, FormFieldRect, FormFieldValueInput, FormFieldView,
};
use pdfx_lib::form_create::{create_form_field, CreateFormFieldRequest};
use pdfx_lib::links::{
    list_links, update_link_target, LinkTarget, LinkUnsupportedReason, LinkView,
    UpdateLinkTargetRequest,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveResultView};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::synthetic;
use support::temp::TempDir;

fn save(document_id: &str, target: &Path) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(target.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save should succeed")
}

fn open(path: &Path) -> DocumentView {
    open_document(path.to_string_lossy().into_owned(), None).expect("open document")
}

fn fixture_copy(id: &str, dir: &TempDir, name: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture '{id}' is missing from the manifest"));
    let target = dir.path().join(name);
    fs::copy(support::fixtures::fixture_path(fixture), &target).expect("copy fixture");
    target
}

fn page_dict(document: &lopdf::Document, number: u32) -> &lopdf::Dictionary {
    let id = *document
        .get_pages()
        .get(&number)
        .expect("page number in page tree");
    document
        .get_object(id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary")
}

fn page_text(document: &lopdf::Document, number: u32) -> String {
    let page = page_dict(document, number);
    let contents = page
        .get(b"Contents")
        .expect("page /Contents")
        .as_reference()
        .expect("/Contents reference");
    let stream = document
        .get_object(contents)
        .expect("content stream object")
        .as_stream()
        .expect("content stream");
    String::from_utf8_lossy(&stream.decompressed_content().expect("decompress")).into_owned()
}

fn rotation_of(document: &lopdf::Document, number: u32) -> i64 {
    page_dict(document, number)
        .get(b"Rotate")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(0)
}

fn page_box(document: &lopdf::Document, number: u32, key: &[u8]) -> [f32; 4] {
    let array = page_dict(document, number)
        .get(key)
        .unwrap_or_else(|_| panic!("page {:?}", String::from_utf8_lossy(key)))
        .as_array()
        .expect("/MediaBox or /CropBox array");
    let value = |index: usize| match &array[index] {
        lopdf::Object::Integer(number) => *number as f32,
        lopdf::Object::Real(number) => *number,
        other => panic!("unexpected page box entry {other:?}"),
    };
    [value(0), value(1), value(2), value(3)]
}

fn media_box(document: &lopdf::Document, number: u32) -> [f32; 4] {
    page_box(document, number, b"MediaBox")
}

fn crop_box(document: &lopdf::Document, number: u32) -> [f32; 4] {
    page_box(document, number, b"CropBox")
}

fn catalog(document: &lopdf::Document) -> &lopdf::Dictionary {
    let root = document
        .trailer
        .get(b"Root")
        .expect("trailer Root")
        .as_reference()
        .expect("Root reference");
    document
        .get_object(root)
        .expect("catalog object")
        .as_dict()
        .expect("catalog dictionary")
}

fn dict_of<'a>(document: &'a lopdf::Document, object: &'a lopdf::Object) -> &'a lopdf::Dictionary {
    match object.as_reference() {
        Ok(id) => document
            .get_object(id)
            .expect("referenced object")
            .as_dict()
            .expect("referenced dictionary"),
        Err(_) => object.as_dict().expect("direct dictionary"),
    }
}

/// Page annotations with their object id when the entry is a reference.
fn page_annots<'a>(
    document: &'a lopdf::Document,
    number: u32,
) -> Vec<(Option<lopdf::ObjectId>, &'a lopdf::Dictionary)> {
    let page = page_dict(document, number);
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    let array = match annots {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("annots object")
            .as_array()
            .expect("annots array"),
        other => other.as_array().expect("annots array"),
    };
    array
        .iter()
        .filter_map(|entry| match entry {
            lopdf::Object::Reference(id) => {
                Some((Some(*id), document.get_object(*id).ok()?.as_dict().ok()?))
            }
            lopdf::Object::Dictionary(dictionary) => Some((None, dictionary)),
            _ => None,
        })
        .collect()
}

fn widgets_on_page<'a>(
    document: &'a lopdf::Document,
    number: u32,
) -> Vec<(Option<lopdf::ObjectId>, &'a lopdf::Dictionary)> {
    page_annots(document, number)
        .into_iter()
        .filter(|(_, dictionary)| {
            dictionary
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .ok()
                == Some(b"Widget".as_slice())
        })
        .collect()
}

fn widget_ids_on_page(document: &lopdf::Document, number: u32) -> Vec<lopdf::ObjectId> {
    widgets_on_page(document, number)
        .into_iter()
        .filter_map(|(id, _)| id)
        .collect()
}

/// The page object a link annotation's `/GoTo` action points at.
fn link_destination(document: &lopdf::Document, number: u32) -> Option<lopdf::ObjectId> {
    for (_, annotation) in page_annots(document, number) {
        if annotation
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .ok()
            != Some(b"Link".as_slice())
        {
            continue;
        }
        let Ok(action) = annotation.get(b"A") else {
            continue;
        };
        let action = dict_of(document, action);
        if action.get(b"S").and_then(lopdf::Object::as_name).ok() != Some(b"GoTo".as_slice()) {
            continue;
        }
        let destination = match action.get(b"D").expect("/D") {
            lopdf::Object::Reference(id) => document.get_object(*id).expect("destination object"),
            other => other,
        };
        return destination
            .as_array()
            .ok()
            .and_then(|array| array.first())
            .and_then(|page| page.as_reference().ok());
    }
    None
}

fn annotation_ids<'a>(
    annotations: &[(Option<lopdf::ObjectId>, &'a lopdf::Dictionary)],
) -> Vec<lopdf::ObjectId> {
    annotations.iter().filter_map(|(id, _)| *id).collect()
}

fn fields_array<'a>(document: &'a lopdf::Document) -> &'a [lopdf::Object] {
    let catalog = catalog(document);
    let Ok(acro_form) = catalog.get(b"AcroForm") else {
        return &[];
    };
    let acro_form = dict_of(document, acro_form);
    let Ok(fields) = acro_form.get(b"Fields") else {
        return &[];
    };
    match fields {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("field list object")
            .as_array()
            .expect("field list array"),
        other => other.as_array().expect("/Fields array"),
    }
}

fn form_fields_of<'a>(document: &'a lopdf::Document) -> Vec<&'a lopdf::Dictionary> {
    fields_array(document)
        .iter()
        .map(|field| dict_of(document, field))
        .collect()
}

fn form_field_names(document: &lopdf::Document) -> Vec<String> {
    form_fields_of(document)
        .into_iter()
        .filter_map(|field| {
            field
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(|title| String::from_utf8_lossy(title).into_owned())
        })
        .collect()
}

fn field_named<'a>(document: &'a lopdf::Document, name: &str) -> &'a lopdf::Dictionary {
    form_fields_of(document)
        .into_iter()
        .find(|field| {
            field
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(|title| String::from_utf8_lossy(title) == name)
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("saved field {name}"))
}

fn field_ids_named(document: &lopdf::Document, name: &str) -> Vec<lopdf::ObjectId> {
    fields_array(document)
        .iter()
        .filter_map(|field| field.as_reference().ok())
        .filter(|id| {
            document
                .get_object(*id)
                .ok()
                .and_then(|object| object.as_dict().ok())
                .and_then(|field| field.get(b"T").ok())
                .and_then(|title| title.as_str().ok())
                .map(|title| String::from_utf8_lossy(title) == name)
                .unwrap_or(false)
        })
        .collect()
}

fn kid_ids(document: &lopdf::Document, field: &lopdf::Dictionary) -> Vec<lopdf::ObjectId> {
    field
        .get(b"Kids")
        .ok()
        .and_then(|kids| match kids {
            lopdf::Object::Reference(id) => document.get_object(*id).ok(),
            other => Some(other),
        })
        .and_then(|kids| kids.as_array().ok())
        .map(|kids| {
            kids.iter()
                .filter_map(|kid| kid.as_reference().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `/AS` state names of the widgets referenced by a radio group's `/Kids`.
fn kid_states(document: &lopdf::Document, field: &lopdf::Dictionary) -> Vec<String> {
    kid_ids(document, field)
        .into_iter()
        .map(|id| {
            document
                .get_object(id)
                .expect("kid object")
                .as_dict()
                .expect("kid dictionary")
                .get(b"AS")
                .ok()
                .and_then(|state| state.as_name().ok())
                .map(|state| String::from_utf8_lossy(state).into_owned())
                .unwrap_or_default()
        })
        .collect()
}

fn lopdf_text(object: &lopdf::Object) -> String {
    String::from_utf8_lossy(object.as_str().expect("string")).into_owned()
}

/// A checkbox/radio `/V` or `/AS` state, which readers store as a name or a
/// string.
fn state_of(object: &lopdf::Object) -> String {
    let state = match object {
        lopdf::Object::Name(name) => String::from_utf8_lossy(name).into_owned(),
        other => String::from_utf8_lossy(other.as_str().expect("state")).into_owned(),
    };
    state.strip_prefix('/').map(str::to_owned).unwrap_or(state)
}

fn field_view<'a>(fields: &'a [FormFieldView], name: &str, page_index: u32) -> &'a FormFieldView {
    fields
        .iter()
        .find(|field| field.name == name && field.page_index == page_index)
        .unwrap_or_else(|| panic!("field {name} on page {page_index}"))
}

fn fill_text(document_id: &str, field_id: &str, text: &str) {
    fill_form_field(
        document_id.to_owned(),
        field_id.to_owned(),
        FormFieldValueInput::Text {
            text: text.to_owned(),
        },
    )
    .expect("fill text field");
}

fn fill_checkbox(document_id: &str, field_id: &str, checked: bool) {
    fill_form_field(
        document_id.to_owned(),
        field_id.to_owned(),
        FormFieldValueInput::Checked { checked },
    )
    .expect("fill checkbox");
}

fn fill_choice(document_id: &str, field_id: &str, value: &str) {
    fill_form_field(
        document_id.to_owned(),
        field_id.to_owned(),
        FormFieldValueInput::Choice {
            value: value.to_owned(),
        },
    )
    .expect("fill choice");
}

fn radio_widget<'a>(
    fields: &'a [FormFieldView],
    name: &str,
    page_index: u32,
    export: &str,
) -> &'a FormFieldView {
    fields
        .iter()
        .find(|field| {
            field.name == name
                && field.page_index == page_index
                && field.export_value.as_deref() == Some(export)
        })
        .unwrap_or_else(|| panic!("radio widget {name}/{export} on page {page_index}"))
}

fn radio_states(fields: &[FormFieldView], name: &str, page_index: u32) -> Vec<(String, bool)> {
    fields
        .iter()
        .filter(|field| field.name == name && field.page_index == page_index)
        .map(|field| {
            (
                field.export_value.clone().unwrap_or_default(),
                field.checked,
            )
        })
        .collect()
}

fn source_unchanged(source: &Path, bytes: &[u8], mtime: std::time::SystemTime) {
    assert_eq!(fs::read(source).expect("reread source"), bytes);
    assert_eq!(
        fs::metadata(source)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        mtime
    );
}

#[test]
fn copy_inserts_after_current_with_identical_content_size_rotation_and_links() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_bytes = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let document = open(&source);
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 3);

    // The source page's rotation must travel with the copy.
    pdfx_lib::page_ops::rotate_page(document_id.clone(), 0, 90).expect("rotate source page");

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!(view.page_count, 4);
    assert_eq!(view.page_index, 1, "the copy follows the source page");
    assert!(view.history.can_undo);
    assert!(view.history.dirty);

    // The live viewer can render the duplicate immediately: same size and
    // rotation, and the same text as the source page.
    let source_info = page_info(document_id.clone(), 0).expect("source page info");
    let copy_info = page_info(document_id.clone(), 1).expect("copy page info");
    assert_eq!(copy_info.rotation, 90);
    assert_eq!(copy_info.width, source_info.width);
    assert_eq!(copy_info.height, source_info.height);
    let copy_layer = extract_text_layer(document_id.clone(), 1).expect("copy text layer");
    assert!(
        copy_layer.text.contains("Page one"),
        "copy text {:?}",
        copy_layer.text
    );
    let fourth = page_info(document_id.clone(), 3).expect("fourth page info");
    assert!((fourth.width - 612.0).abs() < 0.5);
    assert_eq!(fourth.rotation, 0, "untouched pages keep their rotation");

    let dir = TempDir::new("copy-page-content");
    let output = dir.path().join("copied.pdf");
    save(&document_id, &output);

    // The source file is never modified.
    source_unchanged(&source, &source_bytes, source_mtime);

    // Independent parse: four pages in the expected order, each with its own
    // content, and identical size/rotation on the copied pair.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(parsed.get_pages().len(), 4);
    assert_eq!(media_box(&parsed, 1), media_box(&parsed, 2));
    assert_eq!(rotation_of(&parsed, 1), 90);
    assert_eq!(rotation_of(&parsed, 2), 90);
    assert_eq!(rotation_of(&parsed, 3), 0);
    assert!(page_text(&parsed, 1).contains("Page one"));
    assert!(page_text(&parsed, 2).contains("Page one"));
    assert!(page_text(&parsed, 3).contains("Page two"));
    assert!(page_text(&parsed, 4).contains("Page three"));

    // The copied page owns separate annotation objects, but link destinations
    // still resolve to the same page object as the original's.
    let original_annots = page_annots(&parsed, 1);
    let copy_annots = page_annots(&parsed, 2);
    assert_eq!(
        link_destination(&parsed, 1).expect("source page link"),
        link_destination(&parsed, 2).expect("copied page link")
    );
    let original_ids = annotation_ids(&original_annots);
    let copy_ids = annotation_ids(&copy_annots);
    assert!(!original_ids.is_empty(), "source annotations are present");
    assert!(!copy_ids.is_empty(), "copied annotations are present");
    for id in original_ids {
        assert!(
            !copy_ids.contains(&id),
            "the copy must not reuse annotation object {id:?}"
        );
    }

    close_document(document_id).expect("close document");
}

#[test]
fn copied_page_keeps_the_crop_box_and_crop_relative_geometry() {
    let dir = TempDir::new("copy-page-crop");
    let source = synthetic::crop_offset_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 1);

    let original_info = page_info(document_id.clone(), 0).expect("source page info");
    assert_eq!((original_info.width, original_info.height), (400.0, 600.0));
    assert_eq!((original_info.crop_x, original_info.crop_y), (100.0, 100.0));
    let original_layer = extract_text_layer(document_id.clone(), 0).expect("source text layer");

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (2, 1));

    let copy_info = page_info(document_id.clone(), 1).expect("copy page info");
    assert_eq!(
        (
            copy_info.width,
            copy_info.height,
            copy_info.crop_x,
            copy_info.crop_y
        ),
        (
            original_info.width,
            original_info.height,
            original_info.crop_x,
            original_info.crop_y
        )
    );
    // Text geometry is relative to the CropBox, so an equal character layer
    // proves the copy keeps the same visible content region.
    let copy_layer = extract_text_layer(document_id.clone(), 1).expect("copy text layer");
    assert_eq!(copy_layer.text, original_layer.text);
    assert_eq!(copy_layer.chars, original_layer.chars);
    assert_eq!(copy_layer.page_width, original_layer.page_width);
    assert_eq!(copy_layer.page_height, original_layer.page_height);
    assert_eq!(copy_layer.crop_x, original_layer.crop_x);
    assert_eq!(copy_layer.crop_y, original_layer.crop_y);

    let output = dir.path().join("copied-crop.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(parsed.get_pages().len(), 2);
    assert_eq!(media_box(&parsed, 1), [0.0, 0.0, 612.0, 792.0]);
    assert_eq!(media_box(&parsed, 2), media_box(&parsed, 1));
    assert_eq!(crop_box(&parsed, 1), [100.0, 100.0, 500.0, 700.0]);
    assert_eq!(
        crop_box(&parsed, 2),
        crop_box(&parsed, 1),
        "the copy keeps the source CropBox"
    );
    assert!(page_text(&parsed, 1).contains("Crop offset"));
    assert!(page_text(&parsed, 2).contains("Crop offset"));
}

#[test]
fn copied_form_page_is_independent_through_save_and_reopen() {
    let dir = TempDir::new("copy-page-forms");
    let source = fixture_copy("form-acroform", &dir, "form.pdf");
    let source_bytes = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let document = open(&source);
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 2);

    // Give the source page values for every field type before copying.
    let fields = form_fields(document_id.clone()).expect("fields");
    fill_text(
        &document_id,
        &field_view(&fields, "full_name", 0).field_id,
        "SOURCE VALUE",
    );
    fill_checkbox(
        &document_id,
        &field_view(&fields, "subscribe", 0).field_id,
        true,
    );
    let female = radio_widget(&fields, "gender", 0, "female");
    fill_checkbox(&document_id, &female.field_id, true);
    fill_choice(
        &document_id,
        &field_view(&fields, "country", 0).field_id,
        "Germany",
    );
    fill_choice(
        &document_id,
        &field_view(&fields, "department", 0).field_id,
        "Design",
    );

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!(view.page_count, 3);
    assert_eq!(view.page_index, 1);

    // The copy is discovered on the new logical slot under names unique to the
    // copy, and starts with the source page's current session-visible values.
    let fields = form_fields(document_id.clone()).expect("fields after copy");
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("SOURCE VALUE")
    );
    assert!(field_view(&fields, "subscribe__copy_p2", 1).checked);
    assert_eq!(
        radio_states(&fields, "gender__copy_p2", 1),
        vec![
            ("male".to_owned(), false),
            ("female".to_owned(), true),
            ("other".to_owned(), false),
        ]
    );
    assert_eq!(
        field_view(&fields, "country__copy_p2", 1).value.as_deref(),
        Some("Germany")
    );
    assert_eq!(
        field_view(&fields, "department__copy_p2", 1)
            .value
            .as_deref(),
        Some("Design")
    );
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1).kind,
        FormFieldKind::Text
    );
    assert!(
        fields
            .iter()
            .all(|field| field.name != "full_name" || field.page_index == 0),
        "the original field keeps its own name on the source page"
    );

    // Edit only the copy.
    fill_text(
        &document_id,
        &field_view(&fields, "full_name__copy_p2", 1).field_id,
        "COPY VALUE",
    );
    fill_checkbox(
        &document_id,
        &field_view(&fields, "subscribe__copy_p2", 1).field_id,
        false,
    );
    let male = radio_widget(&fields, "gender__copy_p2", 1, "male");
    fill_checkbox(&document_id, &male.field_id, true);
    fill_choice(
        &document_id,
        &field_view(&fields, "country__copy_p2", 1).field_id,
        "China",
    );
    fill_choice(
        &document_id,
        &field_view(&fields, "department__copy_p2", 1).field_id,
        "Sales",
    );

    // The source page is untouched in the live session.
    let fields = form_fields(document_id.clone()).expect("fields after copy edits");
    assert_eq!(
        field_view(&fields, "full_name", 0).value.as_deref(),
        Some("SOURCE VALUE")
    );
    assert!(field_view(&fields, "subscribe", 0).checked);
    assert_eq!(
        radio_states(&fields, "gender", 0),
        vec![
            ("male".to_owned(), false),
            ("female".to_owned(), true),
            ("other".to_owned(), false),
        ]
    );
    assert_eq!(
        field_view(&fields, "country", 0).value.as_deref(),
        Some("Germany")
    );
    assert_eq!(
        field_view(&fields, "department", 0).value.as_deref(),
        Some("Design")
    );

    // ... and the copy holds the new values.
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("COPY VALUE")
    );
    assert!(!field_view(&fields, "subscribe__copy_p2", 1).checked);
    assert_eq!(
        radio_states(&fields, "gender__copy_p2", 1),
        vec![
            ("male".to_owned(), true),
            ("female".to_owned(), false),
            ("other".to_owned(), false),
        ]
    );
    assert_eq!(
        field_view(&fields, "country__copy_p2", 1).value.as_deref(),
        Some("China")
    );
    assert_eq!(
        field_view(&fields, "department__copy_p2", 1)
            .value
            .as_deref(),
        Some("Sales")
    );

    let output = dir.path().join("copied-form.pdf");
    save(&document_id, &output);
    source_unchanged(&source, &source_bytes, source_mtime);

    // Saved structure, read without PDFium.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(parsed.get_pages().len(), 3);
    let names = form_field_names(&parsed);
    for original in [
        "full_name",
        "email",
        "subscribe",
        "gender",
        "country",
        "department",
        "notes",
        "agree",
        "js_guard",
        "readonly_note",
    ] {
        assert!(
            names.iter().any(|name| name == original),
            "original field {original} is missing: {names:?}"
        );
    }
    for copied in [
        "full_name__copy_p2",
        "email__copy_p2",
        "subscribe__copy_p2",
        "gender__copy_p2",
        "country__copy_p2",
        "department__copy_p2",
    ] {
        assert_eq!(
            names.iter().filter(|name| *name == copied).count(),
            1,
            "copy field {copied} must appear exactly once: {names:?}"
        );
    }
    assert_eq!(names.len(), 16, "10 original + 6 copy fields: {names:?}");

    // Text values are independent.
    assert_eq!(
        lopdf_text(field_named(&parsed, "full_name").get(b"V").expect("/V")),
        "SOURCE VALUE"
    );
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "full_name__copy_p2")
                .get(b"V")
                .expect("/V")
        ),
        "COPY VALUE"
    );

    // Checkbox values and appearance states are independent.
    let subscribe = field_named(&parsed, "subscribe");
    assert_eq!(state_of(subscribe.get(b"V").expect("/V")), "Yes");
    assert_eq!(state_of(subscribe.get(b"AS").expect("/AS")), "Yes");
    let copied_subscribe = field_named(&parsed, "subscribe__copy_p2");
    assert_eq!(state_of(copied_subscribe.get(b"V").expect("/V")), "Off");
    assert_eq!(state_of(copied_subscribe.get(b"AS").expect("/AS")), "Off");

    // Choice values use the field's own `/V` and `/I`.
    let country = field_named(&parsed, "country");
    assert_eq!(lopdf_text(country.get(b"V").expect("/V")), "Germany");
    assert_eq!(
        country
            .get(b"I")
            .expect("/I")
            .as_array()
            .expect("/I array")
            .iter()
            .map(|index| index.as_i64().expect("index"))
            .collect::<Vec<_>>(),
        [2]
    );
    let copied_country = field_named(&parsed, "country__copy_p2");
    assert_eq!(lopdf_text(copied_country.get(b"V").expect("/V")), "China");
    assert_eq!(
        copied_country
            .get(b"I")
            .expect("/I")
            .as_array()
            .expect("/I array")
            .iter()
            .map(|index| index.as_i64().expect("index"))
            .collect::<Vec<_>>(),
        [0]
    );
    let department = field_named(&parsed, "department");
    assert_eq!(lopdf_text(department.get(b"V").expect("/V")), "Design");
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "department__copy_p2")
                .get(b"V")
                .expect("/V")
        ),
        "Sales"
    );

    // Radio groups: the copy owns a separate parent field with its own `/V`,
    // its kids are the copy's widgets, and the original group is untouched.
    let gender = field_named(&parsed, "gender");
    assert_eq!(state_of(gender.get(b"V").expect("/V")), "female");
    assert_eq!(kid_states(&parsed, gender), ["Off", "female", "Off"]);
    let original_widgets = widget_ids_on_page(&parsed, 1);
    let original_parent_id = field_ids_named(&parsed, "gender");
    assert_eq!(original_parent_id.len(), 1);
    let original_kids = kid_ids(&parsed, gender);
    assert_eq!(original_kids.len(), 3);
    for widget_id in &original_kids {
        assert!(original_widgets.contains(widget_id));
        let widget = parsed
            .get_object(*widget_id)
            .expect("original widget")
            .as_dict()
            .expect("widget dictionary");
        let parent = widget
            .get(b"Parent")
            .expect("/Parent")
            .as_reference()
            .expect("/Parent reference");
        assert_eq!(parent, original_parent_id[0]);
    }

    let copied_gender = field_named(&parsed, "gender__copy_p2");
    assert_eq!(state_of(copied_gender.get(b"V").expect("/V")), "male");
    assert_eq!(kid_states(&parsed, copied_gender), ["male", "Off", "Off"]);
    let copied_widgets = widget_ids_on_page(&parsed, 2);
    let copied_parent_id = field_ids_named(&parsed, "gender__copy_p2");
    assert_eq!(copied_parent_id.len(), 1);
    assert_ne!(copied_parent_id, original_parent_id);
    let copied_kids = kid_ids(&parsed, copied_gender);
    assert_eq!(copied_kids.len(), 3);
    for widget_id in &copied_kids {
        assert!(copied_widgets.contains(widget_id));
        let widget = parsed
            .get_object(*widget_id)
            .expect("copy widget")
            .as_dict()
            .expect("widget dictionary");
        let parent = widget
            .get(b"Parent")
            .expect("/Parent")
            .as_reference()
            .expect("/Parent reference");
        assert_eq!(parent, copied_parent_id[0]);
    }
    assert!(
        copied_kids.iter().all(|id| !original_kids.contains(id)),
        "the copied group must not reuse the source widgets"
    );

    // Fields and page content on the untouched second page survive.
    assert!(
        form_field_names(&parsed).iter().any(|name| name == "notes"),
        "a field on an untouched page stays in the saved AcroForm"
    );
    assert_eq!(parsed.get_pages().len(), 3);

    // Reopen through the command boundary: PDFium recognises both copies with
    // the same values it sees in the saved file.
    let reopened = open(&output);
    let reopened_id = reopened.document_id.clone();
    let reopened_fields = form_fields(reopened_id.clone()).expect("reopened fields");
    assert_eq!(
        field_view(&reopened_fields, "full_name", 0)
            .value
            .as_deref(),
        Some("SOURCE VALUE")
    );
    assert_eq!(
        field_view(&reopened_fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("COPY VALUE")
    );
    assert!(field_view(&reopened_fields, "subscribe", 0).checked);
    assert!(!field_view(&reopened_fields, "subscribe__copy_p2", 1).checked);
    assert_eq!(
        radio_states(&reopened_fields, "gender", 0),
        vec![
            ("male".to_owned(), false),
            ("female".to_owned(), true),
            ("other".to_owned(), false),
        ]
    );
    assert_eq!(
        radio_states(&reopened_fields, "gender__copy_p2", 1),
        vec![
            ("male".to_owned(), true),
            ("female".to_owned(), false),
            ("other".to_owned(), false),
        ]
    );
    assert_eq!(
        field_view(&reopened_fields, "country", 0).value.as_deref(),
        Some("Germany")
    );
    assert_eq!(
        field_view(&reopened_fields, "country__copy_p2", 1)
            .value
            .as_deref(),
        Some("China")
    );
    let reopened_text = extract_text(reopened_id.clone(), 2).expect("untouched page text");
    assert!(
        reopened_text.text.contains("fixture") || !reopened_text.text.is_empty(),
        "the untouched page keeps content: {:?}",
        reopened_text.text
    );

    // Editing the reopened copy still cannot reach the original.
    fill_text(
        &reopened_id,
        &field_view(&reopened_fields, "full_name__copy_p2", 1).field_id,
        "REOPEN COPY",
    );
    let after_fill = form_fields(reopened_id.clone()).expect("fields after reopen fill");
    assert_eq!(
        field_view(&after_fill, "full_name", 0).value.as_deref(),
        Some("SOURCE VALUE")
    );
    assert_eq!(
        field_view(&after_fill, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("REOPEN COPY")
    );
    let second_output = dir.path().join("copied-form-edited.pdf");
    save(&reopened_id, &second_output);
    let parsed = lopdf::Document::load(&second_output).expect("parse second output");
    assert_eq!(
        lopdf_text(field_named(&parsed, "full_name").get(b"V").expect("/V")),
        "SOURCE VALUE"
    );
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "full_name__copy_p2")
                .get(b"V")
                .expect("/V")
        ),
        "REOPEN COPY"
    );

    close_document(reopened_id).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn copying_a_copied_form_page_keeps_names_unique_and_values_independent() {
    let dir = TempDir::new("copy-page-twice");
    let source = fixture_copy("form-acroform", &dir, "form.pdf");
    let document = open(&source);
    let document_id = document.document_id.clone();

    pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("first copy");
    let fields = form_fields(document_id.clone()).expect("fields");
    fill_text(
        &document_id,
        &field_view(&fields, "full_name__copy_p2", 1).field_id,
        "COPY ONE",
    );

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 1).expect("copy of copy");
    assert_eq!(view.page_count, 4);
    assert_eq!(view.page_index, 2);

    let fields = form_fields(document_id.clone()).expect("fields after second copy");
    assert_eq!(
        field_view(&fields, "full_name", 0).value.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("COPY ONE")
    );
    assert_eq!(
        field_view(&fields, "full_name__copy_p3", 2)
            .value
            .as_deref(),
        Some("COPY ONE"),
        "the second copy inherits the visible value of the first copy"
    );
    assert!(
        fields
            .iter()
            .any(|field| field.name == "gender__copy_p3" && field.page_index == 2),
        "radio widgets on the copy of a copy are namespaced too"
    );

    fill_text(
        &document_id,
        &field_view(&fields, "full_name__copy_p3", 2).field_id,
        "COPY TWO",
    );
    assert_eq!(
        field_view(
            &form_fields(document_id.clone()).expect("fields after edit"),
            "full_name__copy_p2",
            1
        )
        .value
        .as_deref(),
        Some("COPY ONE")
    );

    let output = dir.path().join("copied-twice.pdf");
    save(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(parsed.get_pages().len(), 4);
    let names = form_field_names(&parsed);
    for name in [
        "full_name",
        "full_name__copy_p2",
        "full_name__copy_p3",
        "gender",
        "gender__copy_p2",
        "gender__copy_p3",
    ] {
        assert_eq!(
            names.iter().filter(|candidate| *candidate == name).count(),
            1,
            "field {name} must be unique in the copied AcroForm: {names:?}"
        );
    }
    assert_eq!(
        lopdf_text(field_named(&parsed, "full_name").get(b"V").expect("/V")),
        "Ada Lovelace"
    );
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "full_name__copy_p2")
                .get(b"V")
                .expect("/V")
        ),
        "COPY ONE"
    );
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "full_name__copy_p3")
                .get(b"V")
                .expect("/V")
        ),
        "COPY TWO"
    );

    // Reopening sees three independent widgets with independent values.
    let reopened = open(&output);
    let reopened_id = reopened.document_id.clone();
    let reopened_fields = form_fields(reopened_id.clone()).expect("reopened fields");
    assert_eq!(
        field_view(&reopened_fields, "full_name", 0)
            .value
            .as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        field_view(&reopened_fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("COPY ONE")
    );
    assert_eq!(
        field_view(&reopened_fields, "full_name__copy_p3", 2)
            .value
            .as_deref(),
        Some("COPY TWO")
    );

    close_document(reopened_id).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn undo_and_redo_cover_the_copy_and_its_form_edits() {
    let dir = TempDir::new("copy-page-undo");
    let source = fixture_copy("form-acroform", &dir, "form.pdf");
    let document = open(&source);
    let document_id = document.document_id.clone();

    pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    let fields = form_fields(document_id.clone()).expect("fields");
    fill_text(
        &document_id,
        &field_view(&fields, "full_name__copy_p2", 1).field_id,
        "COPY VALUE",
    );

    // Undo the copy's field edit: the inherited value comes back.
    let step = undo_text_edit(document_id.clone()).expect("undo field");
    assert_eq!(step.page_count, 3);
    assert!(!step.page_structure_changed);
    let fields = form_fields(document_id.clone()).expect("fields after undo");
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("Ada Lovelace")
    );

    // Undo the copy itself: the new logical slot disappears and the copy's
    // fields are no longer discovered.
    let step = undo_text_edit(document_id.clone()).expect("undo copy");
    assert_eq!(step.page_count, 2);
    assert!(step.page_structure_changed);
    assert!(!step.history.can_undo);
    assert!(step.history.can_redo);
    assert!(!step.history.dirty, "undo returns to the saved baseline");
    let fields = form_fields(document_id.clone()).expect("fields after undo copy");
    assert!(
        fields.iter().all(|field| !field.name.contains("__copy_")),
        "copy fields must disappear with the copy: {fields:?}"
    );
    assert_eq!(
        field_view(&fields, "full_name", 0).value.as_deref(),
        Some("Ada Lovelace")
    );

    // A save with the copy undone contains only the source pages and fields.
    let without = dir.path().join("without-copy.pdf");
    save(&document_id, &without);
    let parsed = lopdf::Document::load(&without).expect("parse without copy");
    assert_eq!(parsed.get_pages().len(), 2);
    let names = form_field_names(&parsed);
    assert!(
        names.iter().all(|name| !name.contains("__copy_")),
        "an undone copy never reaches the output: {names:?}"
    );

    // Redo restores the copy, its inherited value, then the edited value.
    let step = redo_text_edit(document_id.clone()).expect("redo copy");
    assert_eq!(step.page_count, 3);
    assert!(step.page_structure_changed);
    let fields = form_fields(document_id.clone()).expect("fields after redo copy");
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("Ada Lovelace")
    );
    let step = redo_text_edit(document_id.clone()).expect("redo field");
    assert_eq!(step.page_count, 3);
    assert!(!step.page_structure_changed);
    let fields = form_fields(document_id.clone()).expect("fields after redo field");
    assert_eq!(
        field_view(&fields, "full_name__copy_p2", 1)
            .value
            .as_deref(),
        Some("COPY VALUE")
    );
    assert_eq!(
        field_view(&fields, "full_name", 0).value.as_deref(),
        Some("Ada Lovelace")
    );

    let with_copy = dir.path().join("with-copy-again.pdf");
    save(&document_id, &with_copy);
    let parsed = lopdf::Document::load(&with_copy).expect("parse with copy");
    assert_eq!(parsed.get_pages().len(), 3);
    assert_eq!(
        lopdf_text(
            field_named(&parsed, "full_name__copy_p2")
                .get(b"V")
                .expect("/V")
        ),
        "COPY VALUE"
    );
    assert_eq!(
        lopdf_text(field_named(&parsed, "full_name").get(b"V").expect("/V")),
        "Ada Lovelace"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn repeat_saves_of_a_copied_page_are_stable() {
    let dir = TempDir::new("copy-page-repeat");
    let source = fixture_copy("form-acroform", &dir, "form.pdf");
    let document = open(&source);
    let document_id = document.document_id.clone();

    pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    let fields = form_fields(document_id.clone()).expect("fields");
    fill_text(
        &document_id,
        &field_view(&fields, "full_name__copy_p2", 1).field_id,
        "COPY VALUE",
    );
    fill_choice(
        &document_id,
        &field_view(&fields, "country__copy_p2", 1).field_id,
        "China",
    );

    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    save(&document_id, &first);
    save(&document_id, &second);

    let first = lopdf::Document::load(&first).expect("parse first");
    let second = lopdf::Document::load(&second).expect("parse second");
    assert_eq!(first.get_pages().len(), 3);
    assert_eq!(second.get_pages().len(), 3);
    let mut first_names = form_field_names(&first);
    let mut second_names = form_field_names(&second);
    first_names.sort();
    second_names.sort();
    assert_eq!(first_names, second_names);
    assert_eq!(first_names.len(), 16, "no duplicated copy fields");
    for name in [
        "full_name",
        "full_name__copy_p2",
        "country",
        "country__copy_p2",
    ] {
        assert_eq!(
            first_names
                .iter()
                .filter(|candidate| *candidate == name)
                .count(),
            1,
            "{name} appears exactly once in each save"
        );
    }
    for parsed in [&first, &second] {
        assert_eq!(
            lopdf_text(
                field_named(parsed, "full_name__copy_p2")
                    .get(b"V")
                    .expect("/V")
            ),
            "COPY VALUE"
        );
        assert_eq!(
            lopdf_text(
                field_named(parsed, "country__copy_p2")
                    .get(b"V")
                    .expect("/V")
            ),
            "China"
        );
        assert_eq!(
            lopdf_text(field_named(parsed, "full_name").get(b"V").expect("/V")),
            "Ada Lovelace"
        );
    }

    close_document(document_id).expect("close document");
}

#[test]
fn copied_page_keeps_link_targets_live_and_after_save() {
    let dir = TempDir::new("copy-page-links");
    let source = synthetic::destination_links_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 2);

    let signature = |link: &LinkView| (link.target.clone(), link.unsupported, link.rects.clone());
    let original: Vec<_> = list_links(document_id.clone())
        .expect("list links")
        .into_iter()
        .filter(|link| link.page_index == 0)
        .map(|link| signature(&link))
        .collect();
    assert_eq!(original.len(), 4, "the fixture carries four links");
    // Three destinations resolve to page 2; the fourth names a missing
    // destination and stays visibly unsupported.
    assert_eq!(original[0].0, Some(LinkTarget::Page { page_index: 1 }));
    assert_eq!(original[1].0, Some(LinkTarget::Page { page_index: 1 }));
    assert_eq!(original[2].0, Some(LinkTarget::Page { page_index: 1 }));
    assert_eq!(original[3].1, Some(LinkUnsupportedReason::Destination));

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (3, 1));

    // The viewer reads links from the live document, so the duplicate must
    // expose the same targets without waiting for a save. The source page's
    // own targets moved one slot because the copy is now between them and the
    // destination page.
    let after_copy = list_links(document_id.clone()).expect("list links after copy");
    let original_after: Vec<_> = after_copy
        .iter()
        .filter(|link| link.page_index == 0)
        .map(|link| signature(link))
        .collect();
    assert_eq!(original_after.len(), 4, "the source keeps its four links");
    for (target, unsupported, _) in &original_after[..3] {
        assert_eq!(target, &Some(LinkTarget::Page { page_index: 2 }));
        assert_eq!(*unsupported, None);
    }
    assert_eq!(
        original_after[3].1,
        Some(LinkUnsupportedReason::Destination)
    );
    let live: Vec<_> = after_copy
        .iter()
        .filter(|link| link.page_index == 1)
        .map(|link| signature(link))
        .collect();
    assert_eq!(
        live, original_after,
        "live copy links match the source page"
    );

    let output = dir.path().join("copied-links.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let reopened = open(&output);
    let saved: Vec<_> = list_links(reopened.document_id.clone())
        .expect("list links in saved output")
        .into_iter()
        .filter(|link| link.page_index == 1)
        .map(|link| signature(&link))
        .collect();
    // The copy keeps the source page's link targets: the original page 2 is
    // slot 2 in the saved arrangement too.
    assert_eq!(
        saved, original_after,
        "saved copy links match the source page"
    );
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn retargeting_a_copied_link_leaves_the_source_link_untouched() {
    let dir = TempDir::new("copy-page-link-edit");
    let source = synthetic::destination_links_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (3, 1));

    let copy_link = list_links(document_id.clone())
        .expect("list links after copy")
        .into_iter()
        .find(|link| link.page_index == 1 && matches!(link.target, Some(LinkTarget::Page { .. })))
        .expect("the copy carries a page link");
    let url = "https://example.com/copied-page".to_string();
    update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: copy_link.id,
            target: LinkTarget::Url { url: url.clone() },
        },
    )
    .expect("retarget the copied link");

    let after_edit = list_links(document_id.clone()).expect("list links after edit");
    let source_links: Vec<_> = after_edit
        .iter()
        .filter(|link| link.page_index == 0)
        .collect();
    assert_eq!(source_links.len(), 4, "the source keeps its four links");
    for link in &source_links[..3] {
        assert_eq!(link.target, Some(LinkTarget::Page { page_index: 2 }));
    }
    assert_eq!(
        source_links[3].unsupported,
        Some(LinkUnsupportedReason::Destination)
    );
    let copy_links: Vec<_> = after_edit
        .iter()
        .filter(|link| link.page_index == 1)
        .collect();
    assert_eq!(copy_links.len(), 4, "the copied page keeps all four links");
    assert!(
        copy_links
            .iter()
            .any(|link| link.target == Some(LinkTarget::Url { url: url.clone() })),
        "the copied link was retargeted"
    );

    let output = dir.path().join("edited-copy-link.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let reopened = open(&output);
    let saved = list_links(reopened.document_id.clone()).expect("list saved links");
    let saved_source: Vec<_> = saved.iter().filter(|link| link.page_index == 0).collect();
    assert_eq!(saved_source.len(), 4, "the source links survive the save");
    for link in &saved_source[..3] {
        assert_eq!(
            link.target,
            Some(LinkTarget::Page { page_index: 2 }),
            "the source link still points at its page"
        );
    }
    let saved_copy: Vec<_> = saved.iter().filter(|link| link.page_index == 1).collect();
    assert_eq!(saved_copy.len(), 4, "the copied links survive the save");
    assert!(
        saved_copy
            .iter()
            .any(|link| link.target == Some(LinkTarget::Url { url: url.clone() })),
        "the save fixup must not overwrite a URI the user chose"
    );
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn restricted_documents_refuse_copy_page_without_touching_history() {
    let dir = TempDir::new("copy-page-permissions");
    let source = fixture_copy("encrypted-owner-restrictions", &dir, "restricted.pdf");
    let source_bytes = fs::read(&source).expect("read source");

    let document = open(&source);
    assert!(document.read_only);
    let document_id = document.document_id.clone();
    let before = edit_history(document_id.clone()).expect("history");

    let error = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert_eq!(document.page_count, 1);
    assert_eq!(
        edit_history(document_id.clone()).expect("history after"),
        before
    );
    assert_eq!(fs::read(&source).expect("reread source"), source_bytes);

    close_document(document_id).expect("close document");
}

/// Builds a minimal PDF from raw object bodies, so tests can exercise form
/// structures the shared fixtures do not cover.
fn build_pdf(objects: Vec<(u32, Vec<u8>)>, root: u32) -> Vec<u8> {
    let mut objects = objects;
    objects.sort_by_key(|(number, _)| *number);
    let last = objects.last().expect("at least one object").0;
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = vec![0usize; (last + 1) as usize];
    for (number, body) in &objects {
        offsets[*number as usize] = out.len();
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", last + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for number in 1..=last {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[number as usize]).as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root {root} 0 R >>\n", last + 1).as_bytes(),
    );
    out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    out
}

/// One page whose text field is nested below a parent field:
/// `/AcroForm /Fields [person]`, `person /Kids [name]`, `name` carries `/FT`
/// and `/V`, and the widget's `/Parent` points at `name`.
fn nested_acrofom_pdf(dir: &TempDir) -> PathBuf {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 6 0 R /Annots [8 0 R] >>".to_vec()),
        (4, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()),
        (5, b"<< /T (person) /Kids [7 0 R] >>".to_vec()),
        (6, b"<< /Length 0 >>\nstream\n\nendstream".to_vec()),
        (7, b"<< /T (name) /Parent 5 0 R /FT /Tx /V (Ada) /DA (/Helv 12 Tf 0 g) /Kids [8 0 R] >>".to_vec()),
        (8, b"<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [72 620 320 644] /P 3 0 R /F 4 >>".to_vec()),
    ];
    let path = dir.path().join("nested-form.pdf");
    fs::write(&path, build_pdf(objects, 1)).expect("write nested form");
    path
}

#[test]
fn copied_annotations_get_fresh_names_and_deleting_the_copy_keeps_the_source() {
    let dir = TempDir::new("copy-page-annotation-identity");
    let source = synthetic::page_operations_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();

    let created = create_shape_with_author(
        document_id.clone(),
        "Auditor".to_owned(),
        CreateShapeRequest {
            page_index: 0,
            kind: ShapeKind::Square,
            geometry: ShapeGeometry {
                x: 100.0,
                y: 100.0,
                width: 50.0,
                height: 40.0,
                rotation: 0.0,
            },
            style: ShapeStyle {
                fill_color: None,
                stroke_color: Some("#ff0000".to_owned()),
                opacity: 100,
                line_width: 2.0,
            },
        },
    )
    .expect("create shape");
    let source_name = created.annotation.expect("created annotation").name;

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (4, 1));

    let squares: Vec<_> = list_annotations(document_id.clone())
        .expect("list annotations")
        .into_iter()
        .filter(|annotation| annotation.kind == AnnotationKind::Square)
        .collect();
    assert_eq!(squares.len(), 2, "the copy carries the shape too");
    assert_eq!((squares[0].page_index, squares[1].page_index), (0, 1));
    assert_eq!(squares[0].name, source_name, "the source keeps its name");
    assert_ne!(
        squares[0].name, squares[1].name,
        "the copied annotation must not answer to the source's /NM"
    );
    let copy_name = squares[1].name.clone();

    // Both names must survive a save with distinct identities.
    let output = dir.path().join("fresh-names.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let reopened = open(&output);
    let saved_squares: Vec<_> = list_annotations(reopened.document_id.clone())
        .expect("list saved annotations")
        .into_iter()
        .filter(|annotation| annotation.kind == AnnotationKind::Square)
        .collect();
    assert_eq!(saved_squares.len(), 2);
    assert_eq!(saved_squares[0].name, source_name);
    assert_eq!(saved_squares[0].page_index, 0);
    assert_eq!(saved_squares[1].name, copy_name);
    assert_eq!(saved_squares[1].page_index, 1);

    // Deleting the copy's annotation must delete the copy, never the source.
    delete_annotation(reopened.document_id.clone(), copy_name).expect("delete copied annotation");
    let remaining: Vec<_> = list_annotations(reopened.document_id.clone())
        .expect("list annotations after delete")
        .into_iter()
        .filter(|annotation| annotation.kind == AnnotationKind::Square)
        .collect();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, source_name);
    assert_eq!(remaining[0].page_index, 0);

    let second = dir.path().join("source-kept.pdf");
    save(&reopened.document_id, &second);
    close_document(reopened.document_id).expect("close reopened document");

    let final_document = open(&second);
    let final_squares: Vec<_> = list_annotations(final_document.document_id.clone())
        .expect("list final annotations")
        .into_iter()
        .filter(|annotation| annotation.kind == AnnotationKind::Square)
        .collect();
    assert_eq!(final_squares.len(), 1);
    assert_eq!(final_squares[0].name, source_name);
    assert_eq!(final_squares[0].page_index, 0);
    close_document(final_document.document_id).expect("close final document");
}

#[test]
fn copying_a_page_with_nested_acrofom_fields_saves_independent_fields() {
    let dir = TempDir::new("copy-page-nested-form");
    let source = nested_acrofom_pdf(&dir);
    let document = open(&source);
    let document_id = document.document_id.clone();

    let fields = form_fields(document_id.clone()).expect("fields");
    let source_field = field_view(&fields, "person.name", 0);
    assert_eq!(source_field.value.as_deref(), Some("Ada"));
    assert_eq!(source_field.kind, FormFieldKind::Text);

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (2, 1));

    let fields = form_fields(document_id.clone()).expect("fields after copy");
    let copied = field_view(&fields, "person.name__copy_p1", 1);
    assert_eq!(copied.value.as_deref(), Some("Ada"));
    assert_eq!(copied.kind, FormFieldKind::Text);

    fill_text(&document_id, &copied.field_id, "Copy Ada");

    let output = dir.path().join("nested-copy.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let parsed = lopdf::Document::load(&output).expect("parse saved output");
    let roots = form_field_names(&parsed);
    assert!(
        roots.iter().any(|name| name == "person"),
        "roots: {roots:?}"
    );
    assert!(
        roots.iter().any(|name| name == "person.name__copy_p1"),
        "the flattened copy is registered in /AcroForm /Fields: {roots:?}"
    );

    let reopened = open(&output);
    let fields = form_fields(reopened.document_id.clone()).expect("fields after reopen");
    assert_eq!(
        field_view(&fields, "person.name", 0).value.as_deref(),
        Some("Ada"),
        "editing the copy must not touch the nested source field"
    );
    assert_eq!(
        field_view(&fields, "person.name__copy_p1", 1)
            .value
            .as_deref(),
        Some("Copy Ada")
    );
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn copying_a_page_duplicates_session_created_fields_with_independent_values() {
    let dir = TempDir::new("copy-page-created-field");
    let source = fixture_copy("form-acroform", &dir, "form.pdf");
    let document = open(&source);
    let document_id = document.document_id.clone();

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Text {
            rect: FormFieldRect {
                x: 100.0,
                y: 100.0,
                width: 200.0,
                height: 30.0,
            },
            name: Some("session_field".to_owned()),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: Some(12.0),
        },
    )
    .expect("create field");
    let fields = form_fields(document_id.clone()).expect("fields");
    let source_field = field_view(&fields, "session_field", 0);
    assert!(source_field.created);
    fill_text(&document_id, &source_field.field_id, "source value");

    let view = pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    assert_eq!((view.page_count, view.page_index), (3, 1));

    let fields = form_fields(document_id.clone()).expect("fields after copy");
    let copied = field_view(&fields, "session_field__copy_p2", 1);
    assert!(copied.created, "the duplicate is a session field");
    assert_eq!(copied.value.as_deref(), Some("source value"));

    // The copy action owns the duplicate: undo hides it with the page and redo
    // brings it back.
    undo_text_edit(document_id.clone()).expect("undo copy");
    let without_copy = form_fields(document_id.clone()).expect("fields after undo");
    assert!(
        !without_copy
            .iter()
            .any(|field| field.name == "session_field__copy_p2"),
        "undoing the copy hides its created field too"
    );
    redo_text_edit(document_id.clone()).expect("redo copy");
    let fields = form_fields(document_id.clone()).expect("fields after redo");
    let copied = field_view(&fields, "session_field__copy_p2", 1).clone();

    fill_text(&document_id, &copied.field_id, "copy value");
    let source_field = field_view(&fields, "session_field", 0).clone();
    fill_text(&document_id, &source_field.field_id, "changed source");

    let output = dir.path().join("created-copy.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let parsed = lopdf::Document::load(&output).expect("parse saved output");
    let roots = form_field_names(&parsed);
    assert!(
        roots.iter().any(|name| name == "session_field"),
        "{roots:?}"
    );
    assert!(
        roots.iter().any(|name| name == "session_field__copy_p2"),
        "the created field is materialized for the copy: {roots:?}"
    );

    let reopened = open(&output);
    let fields = form_fields(reopened.document_id.clone()).expect("fields after reopen");
    assert_eq!(
        field_view(&fields, "session_field", 0).value.as_deref(),
        Some("changed source")
    );
    assert_eq!(
        field_view(&fields, "session_field__copy_p2", 1)
            .value
            .as_deref(),
        Some("copy value")
    );
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn copied_links_follow_pages_moved_after_the_copy() {
    let dir = TempDir::new("copy-page-link-reorder");
    let source = synthetic::destination_links_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 2);

    pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    // [source, copy, destination] -> [destination, source, copy]
    pdfx_lib::page_ops::move_page(document_id.clone(), 2, 0).expect("move destination page");

    let links = list_links(document_id.clone()).expect("list links after move");
    let page_links = |page_index: u32| -> Vec<_> {
        links
            .iter()
            .filter(|link| link.page_index == page_index)
            .map(|link| (link.target.clone(), link.unsupported))
            .collect()
    };
    for page_index in [1, 2] {
        let on_page = page_links(page_index);
        assert_eq!(on_page.len(), 4, "page {page_index} keeps all links");
        for (target, unsupported) in &on_page[..3] {
            assert_eq!(target, &Some(LinkTarget::Page { page_index: 0 }));
            assert_eq!(*unsupported, None);
        }
        assert_eq!(
            on_page[3].1,
            Some(LinkUnsupportedReason::Destination),
            "the unsupported link stays visibly unsupported"
        );
    }

    let output = dir.path().join("links-moved.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let reopened = open(&output);
    let saved = list_links(reopened.document_id.clone()).expect("list saved links");
    for page_index in [1, 2] {
        let targets: Vec<_> = saved
            .iter()
            .filter(|link| link.page_index == page_index)
            .map(|link| link.target.clone())
            .collect();
        assert_eq!(targets.len(), 4);
        for target in &targets[..3] {
            assert_eq!(target, &Some(LinkTarget::Page { page_index: 0 }));
        }
    }
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn copied_links_to_a_deleted_page_stay_visibly_unsupported() {
    let dir = TempDir::new("copy-page-link-delete");
    let source = synthetic::destination_links_pdf();
    let document = open(Path::new(&source.path_string()));
    let document_id = document.document_id.clone();

    pdfx_lib::page_ops::copy_page(document_id.clone(), 0).expect("copy page");
    pdfx_lib::page_ops::delete_page(document_id.clone(), 2).expect("delete destination page");

    let links = list_links(document_id.clone()).expect("list links after delete");
    for page_index in [0, 1] {
        let page_links: Vec<_> = links
            .iter()
            .filter(|link| link.page_index == page_index)
            .collect();
        assert_eq!(page_links.len(), 4, "page {page_index} keeps its links");
        for link in &page_links[..3] {
            assert_eq!(
                link.unsupported,
                Some(LinkUnsupportedReason::Destination),
                "a link to the deleted page must not point somewhere else"
            );
        }
    }

    let output = dir.path().join("links-deleted.pdf");
    save(&document_id, &output);
    close_document(document_id).expect("close document");

    let reopened = open(&output);
    let saved = list_links(reopened.document_id.clone()).expect("list saved links");
    for page_index in [0, 1] {
        let unsupported = saved
            .iter()
            .filter(|link| link.page_index == page_index)
            .filter(|link| link.unsupported == Some(LinkUnsupportedReason::Destination))
            .count();
        assert_eq!(unsupported, 4, "the saved link stays unresolved");
    }
    close_document(reopened.document_id).expect("close reopened document");
}
