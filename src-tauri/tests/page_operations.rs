//! Command-boundary tests for page operations: rotation, deletion, reorder
//! and blank-page insertion, plus their save and undo/redo behaviour.
//!
//! Saved output is parsed with lopdf independently of PDFium so assertions
//! check the file an external viewer reads, not the in-memory document.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pdfx_lib::annotations::{
    create_free_text_with_author, create_shape_with_author, list_annotations, AnnotationKind,
    CreateFreeTextRequest, CreateShapeRequest, FreeTextAlign, FreeTextKind, ShapeGeometry,
    ShapeKind, ShapeStyle, TextMarkupRect,
};
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, page_info, CommandErrorCode, DocumentView,
};
use pdfx_lib::form::{form_fields, FormFieldRect};
use pdfx_lib::form_create::{create_form_field, CreateFormFieldRequest};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveResultView};
use pdfx_lib::text_edit::{
    create_text_box, edit_history, redo_text_edit, text_boxes, undo_text_edit, CreateTextBoxRequest,
};
use support::synthetic;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory removed with the test.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfx-page-ops-{}-{unique}-{tag}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

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

fn page_ids(document: &lopdf::Document) -> Vec<lopdf::ObjectId> {
    document.get_pages().values().copied().collect()
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

/// `MediaBox` of a page, as `[left, bottom, right, top]`.
fn media_box(document: &lopdf::Document, number: u32) -> [f32; 4] {
    let array = page_dict(document, number)
        .get(b"MediaBox")
        .expect("page /MediaBox")
        .as_array()
        .expect("/MediaBox array");
    let value = |index: usize| match &array[index] {
        lopdf::Object::Integer(number) => *number as f32,
        lopdf::Object::Real(number) => *number,
        other => panic!("unexpected /MediaBox entry {other:?}"),
    };
    [value(0), value(1), value(2), value(3)]
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

/// Field names in the saved `/AcroForm /Fields` array.
fn form_field_names(document: &lopdf::Document) -> Vec<String> {
    let catalog = catalog(document);
    let Ok(acroform) = catalog.get(b"AcroForm") else {
        return Vec::new();
    };
    let acroform = dict_of(document, acroform);
    let Ok(fields) = acroform.get(b"Fields").and_then(lopdf::Object::as_array) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|object| {
            let field = dict_of(document, object);
            field
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(|title| String::from_utf8_lossy(title).into_owned())
        })
        .collect()
}

fn outline_titles(document: &lopdf::Document) -> Vec<String> {
    let catalog = catalog(document);
    let outlines = dict_of(document, catalog.get(b"Outlines").expect("Outlines"));
    let mut titles = Vec::new();
    let mut next = outlines.get(b"First").and_then(|o| o.as_reference()).ok();
    while let Some(id) = next {
        let item = document
            .get_object(id)
            .expect("outline item")
            .as_dict()
            .expect("outline dictionary");
        titles.push(
            String::from_utf8_lossy(item.get(b"Title").expect("Title").as_str().expect("string"))
                .into_owned(),
        );
        next = item.get(b"Next").and_then(|o| o.as_reference()).ok();
    }
    titles
}

/// Page object the first bookmark points at, read from `/Dest [page /XYZ ...]`.
fn outline_destination_page(document: &lopdf::Document) -> lopdf::ObjectId {
    let catalog = catalog(document);
    let outlines = dict_of(document, catalog.get(b"Outlines").expect("Outlines"));
    let first = outlines
        .get(b"First")
        .expect("First")
        .as_reference()
        .expect("First reference");
    let item = document
        .get_object(first)
        .expect("outline item")
        .as_dict()
        .expect("outline dictionary");
    item.get(b"Dest")
        .expect("Dest")
        .as_array()
        .expect("Dest array")
        .first()
        .expect("destination page")
        .as_reference()
        .expect("destination page reference")
}

fn save_changes(document_id: &str) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save changes should succeed");
}

fn shape_request(page_index: u32) -> CreateShapeRequest {
    CreateShapeRequest {
        page_index,
        kind: ShapeKind::Square,
        geometry: ShapeGeometry {
            x: 100.0,
            y: 120.0,
            width: 160.0,
            height: 90.0,
            rotation: 0.0,
        },
        style: ShapeStyle {
            fill_color: Some("#ff0000".to_owned()),
            stroke_color: Some("#000000".to_owned()),
            opacity: 100,
            line_width: 2.0,
        },
    }
}

/// Path of a named fixture, copied into `dir` so tests never modify the
/// repository corpus.
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

#[test]
fn rotate_pages_by_each_angle_and_save_reopen() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_bytes = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");
    let source_pages = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };

    let document = open(&source);
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 3);
    assert_eq!(page_info(document_id.clone(), 0).unwrap().rotation, 0);

    // Rotate the three pages by the three supported angles.
    let first = pdfx_lib::page_ops::rotate_page(document_id.clone(), 0, 90).expect("rotate 90");
    assert_eq!(first.page_count, 3);
    assert_eq!(first.page_index, 0);
    assert!(first.history.can_undo);
    assert!(first.history.dirty);

    let second = pdfx_lib::page_ops::rotate_page(document_id.clone(), 1, 180).expect("rotate 180");
    assert_eq!(second.page_index, 1);
    pdfx_lib::page_ops::rotate_page(document_id.clone(), 2, 270).expect("rotate 270");

    // Display geometry and the positioned text layer follow the rotation.
    let info = page_info(document_id.clone(), 0).unwrap();
    assert_eq!(info.rotation, 90);
    assert!((info.width - 792.0).abs() < 0.5, "width {}", info.width);
    assert!((info.height - 612.0).abs() < 0.5, "height {}", info.height);
    let layer = extract_text_layer(document_id.clone(), 0).unwrap();
    assert_eq!(layer.rotation, 90);
    assert!((layer.page_width - 792.0).abs() < 0.5);
    assert!((layer.page_height - 612.0).abs() < 0.5);
    assert!(layer.text.contains("Page one"), "text {:?}", layer.text);
    assert!(!layer.chars.is_empty());
    for character in &layer.chars {
        assert!(character.x >= -0.5 && character.y >= -0.5);
        assert!(character.x + character.width <= layer.page_width + 0.5);
        assert!(character.y + character.height <= layer.page_height + 0.5);
    }

    let dir = TempDir::new("rotate");
    let output = dir.path().join("rotated.pdf");
    save(&document_id, &output);

    // The source document is never modified.
    assert_eq!(fs::read(&source).expect("reread source"), source_bytes);
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_mtime
    );

    // Independent parse: page order is unchanged, rotations are persisted and
    // every page keeps its original content.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(page_ids(&parsed), source_pages);
    assert_eq!(
        (
            rotation_of(&parsed, 1),
            rotation_of(&parsed, 2),
            rotation_of(&parsed, 3)
        ),
        (90, 180, 270)
    );
    assert!(page_text(&parsed, 1).contains("Page one"));
    assert!(page_text(&parsed, 2).contains("Page two"));
    assert!(page_text(&parsed, 3).contains("Page three"));

    // A fresh PDFium open sees the same rotations and text.
    let reopened = open(&output);
    assert_eq!(reopened.page_count, 3);
    let reopened_info = page_info(reopened.document_id.clone(), 2).unwrap();
    assert_eq!(reopened_info.rotation, 270);
    let reopened_layer = extract_text_layer(reopened.document_id.clone(), 1).unwrap();
    assert!(reopened_layer.text.contains("Page two"));
    close_document(reopened.document_id).expect("close reopened");

    // Undo/redo survives the save and moves the dirty baseline with it.
    let undone = undo_text_edit(document_id.clone()).expect("undo rotation");
    assert_eq!(undone.page_index, 2);
    assert!(undone.page_structure_changed);
    assert!(undone.history.dirty);
    assert_eq!(page_info(document_id.clone(), 2).unwrap().rotation, 0);
    let redone = redo_text_edit(document_id.clone()).expect("redo rotation");
    assert_eq!(redone.page_index, 2);
    assert!(!redone.history.dirty);
    assert_eq!(page_info(document_id.clone(), 2).unwrap().rotation, 270);

    close_document(document_id).expect("close document");
}

#[test]
fn delete_page_updates_views_and_undo_restores_page_and_annotations() {
    let pdf = synthetic::page_operations_pdf();
    let document = open(Path::new(&pdf.path_string()));
    let document_id = document.document_id.clone();

    let shape =
        create_shape_with_author(document_id.clone(), "PDFX QA".to_owned(), shape_request(0))
            .expect("create shape");
    let name = shape.annotation.expect("shape view").name;
    let listed = list_annotations(document_id.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].page_index, 0);
    assert_eq!(listed[0].kind, AnnotationKind::Square);

    let deleted = pdfx_lib::page_ops::delete_page(document_id.clone(), 0).expect("delete page 1");
    assert_eq!(deleted.page_count, 2);
    assert_eq!(deleted.page_index, 0);
    assert!(deleted.history.dirty);

    // The remaining pages are renumbered immediately.
    assert_eq!(page_info(document_id.clone(), 0).unwrap().rotation, 0);
    assert!(extract_text_layer(document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page two"));
    assert!(extract_text_layer(document_id.clone(), 1)
        .unwrap()
        .text
        .contains("Page three"));
    assert!(list_annotations(document_id.clone()).unwrap().is_empty());
    let out_of_range = extract_text_layer(document_id.clone(), 2).unwrap_err();
    assert_eq!(out_of_range.code, CommandErrorCode::PageOutOfRange);

    // Undo restores the page at its original position with its annotation.
    let undone = undo_text_edit(document_id.clone()).expect("undo delete");
    assert_eq!(undone.page_count, 3);
    assert_eq!(undone.page_index, 0);
    assert!(undone.page_structure_changed);
    assert!(extract_text_layer(document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page one"));
    let restored = list_annotations(document_id.clone()).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, name);
    assert_eq!(restored[0].page_index, 0);

    // Redo hides it again; undo once more leaves the document as it was.
    let redone = redo_text_edit(document_id.clone()).expect("redo delete");
    assert_eq!(redone.page_count, 2);
    assert!(list_annotations(document_id.clone()).unwrap().is_empty());
    let undone = undo_text_edit(document_id.clone()).expect("undo delete again");
    assert_eq!(undone.page_count, 3);
    assert_eq!(list_annotations(document_id.clone()).unwrap().len(), 1);

    close_document(document_id).expect("close document");
}

#[test]
fn delete_page_saves_reduced_page_tree_and_keeps_bookmark() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_bytes = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };

    let document = open(&source);
    let document_id = document.document_id.clone();
    pdfx_lib::page_ops::delete_page(document_id.clone(), 0).expect("delete page 1");

    let dir = TempDir::new("delete-save");
    let output = dir.path().join("deleted.pdf");
    save(&document_id, &output);

    assert_eq!(fs::read(&source).expect("reread source"), source_bytes);
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_mtime
    );

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved_ids = page_ids(&parsed);
    assert_eq!(saved_ids, source_ids[1..].to_vec());
    assert!(page_text(&parsed, 1).contains("Page two"));
    assert!(page_text(&parsed, 2).contains("Page three"));
    assert!(!saved_ids.contains(&source_ids[0]), "deleted page removed");
    // The bookmark still points at page 2, which is untouched.
    assert_eq!(outline_titles(&parsed), vec!["Chapter Two"]);
    assert_eq!(outline_destination_page(&parsed), source_ids[1]);
    assert_eq!(outline_destination_page(&parsed), saved_ids[0]);

    // A fresh PDFium open sees the reduced, renumbered document.
    let reopened = open(&output);
    assert_eq!(reopened.page_count, 2);
    assert!(extract_text_layer(reopened.document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page two"));
    close_document(reopened.document_id).expect("close reopened");

    // Undo after a save is reflected in the next save.
    undo_text_edit(document_id.clone()).expect("undo delete");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after undo");
    assert_eq!(page_ids(&parsed), source_ids);
    assert!(page_text(&parsed, 1).contains("Page one"));
    assert_eq!(outline_titles(&parsed), vec!["Chapter Two"]);

    // Redo and save again: the reduced page tree is written once, not
    // accumulated across saves.
    redo_text_edit(document_id.clone()).expect("redo delete");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after redo");
    assert_eq!(page_ids(&parsed), source_ids[1..].to_vec());
    assert_eq!(parsed.get_pages().len(), 2);

    close_document(document_id).expect("close document");
}

#[test]
fn insert_blank_page_with_chosen_size_saves_valid_page_tree() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_bytes = fs::read(&source).expect("read source");
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };

    let document = open(&source);
    let document_id = document.document_id.clone();

    // A4-sized blank page between the first and second pages.
    let inserted = pdfx_lib::page_ops::insert_blank_page(
        document_id.clone(),
        1,
        pdfx_lib::page_ops::BlankPageSize {
            width: 595.0,
            height: 842.0,
        },
    )
    .expect("insert blank page");
    assert_eq!(inserted.page_count, 4);
    assert_eq!(inserted.page_index, 1);
    assert!(inserted.history.dirty);
    assert!(inserted.history.can_undo);

    // The inserted page renders immediately and renumbers the others.
    let blank = page_info(document_id.clone(), 1).unwrap();
    assert!((blank.width - 595.0).abs() < 0.5, "width {}", blank.width);
    assert!(
        (blank.height - 842.0).abs() < 0.5,
        "height {}",
        blank.height
    );
    assert_eq!(blank.rotation, 0);
    let blank_layer = extract_text_layer(document_id.clone(), 1).unwrap();
    assert!(blank_layer.text.is_empty(), "text {:?}", blank_layer.text);
    assert!(extract_text_layer(document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page one"));
    assert!(extract_text_layer(document_id.clone(), 2)
        .unwrap()
        .text
        .contains("Page two"));
    assert!(extract_text_layer(document_id.clone(), 3)
        .unwrap()
        .text
        .contains("Page three"));

    let dir = TempDir::new("insert");
    let output = dir.path().join("inserted.pdf");
    save(&document_id, &output);
    assert_eq!(fs::read(&source).expect("reread source"), source_bytes);

    // Independent parse: four pages in logical order, the blank page has the
    // requested boxes and the untouched pages keep their content and object
    // identities.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved_ids = page_ids(&parsed);
    assert_eq!(saved_ids.len(), 4);
    assert_eq!(saved_ids[0], source_ids[0]);
    assert_eq!(saved_ids[2], source_ids[1]);
    assert_eq!(saved_ids[3], source_ids[2]);
    assert!(!source_ids.contains(&saved_ids[1]), "blank page is new");
    assert_eq!(media_box(&parsed, 2), [0.0, 0.0, 595.0, 842.0]);
    let blank_dict = page_dict(&parsed, 2);
    let crop = blank_dict
        .get(b"CropBox")
        .expect("/CropBox")
        .as_array()
        .expect("/CropBox array");
    assert_eq!(crop.len(), 4);
    assert_eq!(media_box(&parsed, 3), [0.0, 0.0, 612.0, 792.0]);
    assert!(page_text(&parsed, 1).contains("Page one"));
    assert!(page_text(&parsed, 3).contains("Page two"));
    assert_eq!(outline_titles(&parsed), vec!["Chapter Two"]);
    assert_eq!(outline_destination_page(&parsed), source_ids[1]);

    // PDFium reopens the inserted page at its logical position.
    let reopened = open(&output);
    assert_eq!(reopened.page_count, 4);
    let reopened_blank = page_info(reopened.document_id.clone(), 1).unwrap();
    assert!((reopened_blank.width - 595.0).abs() < 0.5);
    assert!((reopened_blank.height - 842.0).abs() < 0.5);
    assert!(extract_text_layer(reopened.document_id.clone(), 2)
        .unwrap()
        .text
        .contains("Page two"));
    close_document(reopened.document_id).expect("close reopened");

    // Undo removes the page from the logical arrangement; redo restores it.
    // The save above moved the dirty baseline to the four-page state.
    let undone = undo_text_edit(document_id.clone()).expect("undo insert");
    assert_eq!(undone.page_count, 3);
    assert!(undone.page_structure_changed);
    assert!(undone.history.dirty);
    assert!(extract_text_layer(document_id.clone(), 1)
        .unwrap()
        .text
        .contains("Page two"));
    let redone = redo_text_edit(document_id.clone()).expect("redo insert");
    assert_eq!(redone.page_count, 4);
    assert!(!redone.history.dirty);
    assert!(page_info(document_id.clone(), 1).unwrap().width - 595.0 < 0.5);

    // Saving after undo removes the blank page again.
    undo_text_edit(document_id.clone()).expect("undo insert again");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse after undo");
    assert_eq!(page_ids(&parsed), source_ids);
    close_document(document_id).expect("close document");
}

#[test]
fn move_page_reorders_content_annotations_and_saved_page_tree() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };
    let document = open(&source);
    let document_id = document.document_id.clone();

    let shape =
        create_shape_with_author(document_id.clone(), "PDFX QA".to_owned(), shape_request(0))
            .expect("create shape");
    let name = shape.annotation.expect("shape view").name;

    let moved = pdfx_lib::page_ops::move_page(document_id.clone(), 0, 2).expect("move page");
    assert_eq!(moved.page_count, 3);
    assert_eq!(moved.page_index, 2);
    assert!(moved.history.dirty);

    assert!(extract_text_layer(document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page two"));
    assert!(extract_text_layer(document_id.clone(), 1)
        .unwrap()
        .text
        .contains("Page three"));
    assert!(extract_text_layer(document_id.clone(), 2)
        .unwrap()
        .text
        .contains("Page one"));
    // The annotation follows its page rather than staying at an index.
    let listed = list_annotations(document_id.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, name);
    assert_eq!(listed[0].page_index, 2);

    let dir = TempDir::new("move");
    let output = dir.path().join("moved.pdf");
    save(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(
        page_ids(&parsed),
        vec![source_ids[1], source_ids[2], source_ids[0]]
    );
    assert!(page_text(&parsed, 1).contains("Page two"));
    assert!(page_text(&parsed, 2).contains("Page three"));
    assert!(page_text(&parsed, 3).contains("Page one"));
    // The shape annotation moved with page 1 to the last slot, next to the
    // page's original link annotation.
    let last = page_dict(&parsed, 3);
    assert_eq!(
        last.get(b"Annots")
            .expect("/Annots")
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // The bookmark still points at page 2, which is now the first page.
    assert_eq!(outline_titles(&parsed), vec!["Chapter Two"]);
    assert_eq!(outline_destination_page(&parsed), source_ids[1]);
    assert_eq!(outline_destination_page(&parsed), page_ids(&parsed)[0]);

    let reopened = open(&output);
    assert!(extract_text_layer(reopened.document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page two"));
    assert!(extract_text_layer(reopened.document_id.clone(), 2)
        .unwrap()
        .text
        .contains("Page one"));
    let reopened_annotations = list_annotations(reopened.document_id.clone()).unwrap();
    assert_eq!(reopened_annotations.len(), 1);
    assert_eq!(reopened_annotations[0].page_index, 2);
    close_document(reopened.document_id).expect("close reopened");

    // Undo restores the original order, including the annotation slot. The
    // save above moved the dirty baseline to the reordered state.
    let undone = undo_text_edit(document_id.clone()).expect("undo move");
    assert_eq!(undone.page_index, 0);
    assert!(undone.page_structure_changed);
    assert!(undone.history.dirty);
    assert!(extract_text_layer(document_id.clone(), 0)
        .unwrap()
        .text
        .contains("Page one"));
    assert_eq!(
        list_annotations(document_id.clone()).unwrap()[0].page_index,
        0
    );
    let redone = redo_text_edit(document_id.clone()).expect("redo move");
    assert!(!redone.history.dirty);
    assert!(extract_text_layer(document_id.clone(), 2)
        .unwrap()
        .text
        .contains("Page one"));

    // An out-of-range target is rejected without changing history.
    let before = edit_history(document_id.clone()).unwrap();
    let error = pdfx_lib::page_ops::move_page(document_id.clone(), 0, 3).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);
    assert_eq!(edit_history(document_id.clone()).unwrap(), before);

    close_document(document_id).expect("close document");
}

#[test]
fn edits_after_page_operations_follow_the_visible_page() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };
    let document = open(&source);
    let document_id = document.document_id.clone();

    // Visible pages are original pages 2 and 3, at logical slots 0 and 1.
    pdfx_lib::page_ops::delete_page(document_id.clone(), 0).expect("delete page 1");

    // A shape created on logical page 1 belongs to original page 3, the page
    // the user sees, not to the hidden first page.
    let shape =
        create_shape_with_author(document_id.clone(), "PDFX QA".to_owned(), shape_request(1))
            .expect("create shape");
    let annotation = shape.annotation.clone().expect("shape view");
    assert_eq!(shape.page_index, 1);
    assert_eq!(annotation.page_index, 1);
    let listed = list_annotations(document_id.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, annotation.name);
    assert_eq!(listed[0].page_index, 1);

    let text_box = create_text_box(
        document_id.clone(),
        1,
        CreateTextBoxRequest {
            x: 40.0,
            y: 50.0,
            width: 220.0,
            height: 90.0,
            style: None,
        },
    )
    .expect("create text box");
    assert_eq!(text_box.page_index, 1);
    assert_eq!(text_box.target.page_index, 1);
    assert_eq!(text_boxes(document_id.clone(), 1).unwrap().len(), 1);
    assert!(text_boxes(document_id.clone(), 0).unwrap().is_empty());

    // Reorder so the edited page moves to logical slot 0; its session state
    // follows the page, not the slot number.
    pdfx_lib::page_ops::move_page(document_id.clone(), 0, 1).expect("move page");
    let boxes = text_boxes(document_id.clone(), 0).unwrap();
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].page_index, 0);
    assert!(text_boxes(document_id.clone(), 1).unwrap().is_empty());
    let listed = list_annotations(document_id.clone()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].page_index, 0);

    // Saved output attaches the shape to the page object the user edited.
    let dir = TempDir::new("align");
    let output = dir.path().join("aligned.pdf");
    save(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(page_ids(&parsed), vec![source_ids[2], source_ids[1]]);
    let edited = page_dict(&parsed, 1);
    assert_eq!(
        edited
            .get(b"Annots")
            .expect("/Annots")
            .as_array()
            .unwrap()
            .len(),
        1,
        "the created shape is the edited page's only annotation"
    );
    let untouched = page_dict(&parsed, 2);
    assert!(
        untouched.get(b"Annots").is_err(),
        "the other page must not gain the shape"
    );
    let hidden = parsed
        .get_object(source_ids[0])
        .expect("hidden page object")
        .as_dict()
        .expect("hidden page dictionary");
    assert_eq!(
        hidden
            .get(b"Annots")
            .expect("/Annots")
            .as_array()
            .unwrap()
            .len(),
        1,
        "the deleted page keeps only its original link annotation"
    );

    // Undoing the reorder and then the text box creation reports the box's
    // logical slot, not its physical page index.
    let undone = undo_text_edit(document_id.clone()).expect("undo move");
    assert_eq!(undone.page_index, 0);
    let undone = undo_text_edit(document_id.clone()).expect("undo text box");
    assert_eq!(undone.page_index, 1);
    assert!(text_boxes(document_id.clone(), 1).unwrap().is_empty());

    close_document(document_id).expect("close document");
}

#[test]
fn delete_page_hides_and_undo_restores_its_form_field() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };
    let document = open(&source);
    let document_id = document.document_id.clone();

    let created = create_form_field(
        document_id.clone(),
        1,
        CreateFormFieldRequest::Text {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("page_two_field".to_owned()),
            tooltip: None,
            required: true,
            multiline: false,
            font_size: None,
        },
    )
    .expect("create form field");
    let field_id = created.field_id.clone();
    let created_field = created
        .fields
        .iter()
        .find(|field| field.name == "page_two_field")
        .expect("created field is listed");
    assert_eq!(created_field.page_index, 1);
    assert_eq!(created_field.field_id, field_id);

    // Deleting the page hides its widget from the form panel.
    pdfx_lib::page_ops::delete_page(document_id.clone(), 1).expect("delete page 2");
    let fields = form_fields(document_id.clone()).unwrap();
    assert!(
        fields.iter().all(|field| field.name != "page_two_field"),
        "hidden page field must not be listed: {fields:?}"
    );

    let dir = TempDir::new("form-pages");
    let output = dir.path().join("form-deleted.pdf");
    save(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(page_ids(&parsed), vec![source_ids[0], source_ids[2]]);
    assert!(
        !form_field_names(&parsed).contains(&"page_two_field".to_owned()),
        "a field on a deleted page must not be written: {:?}",
        form_field_names(&parsed)
    );

    // Undo restores the page and its widget, on its original slot.
    let undone = undo_text_edit(document_id.clone()).expect("undo delete");
    assert_eq!(undone.page_count, 3);
    let fields = form_fields(document_id.clone()).unwrap();
    let restored = fields
        .iter()
        .find(|field| field.name == "page_two_field")
        .expect("field restored with the page");
    assert_eq!(restored.page_index, 1);
    assert_eq!(restored.field_id, field_id);

    // Saving again writes the widget with the restored page.
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse restored output");
    assert_eq!(page_ids(&parsed), source_ids);
    assert_eq!(form_field_names(&parsed), vec!["page_two_field".to_owned()]);
    let page_two = page_dict(&parsed, 2);
    let annots = page_two
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    assert_eq!(annots.len(), 1, "the widget is attached to page 2");

    close_document(document_id).expect("close document");
}

#[test]
fn free_text_and_created_form_field_share_one_save_revision() {
    let pdf = synthetic::page_operations_pdf();
    let document = open(Path::new(&pdf.path_string()));
    let document_id = document.document_id.clone();

    // Both lopdf passes touch the same page's `/Annots`: free text needs its
    // `/Q`/`/IT` dictionary entries rewritten, while a created form widget is
    // appended to the same array. One must not overwrite the other.
    create_free_text_with_author(
        document_id.clone(),
        "PDFX QA".to_owned(),
        CreateFreeTextRequest {
            page_index: 0,
            kind: FreeTextKind::FreeText,
            rect: TextMarkupRect {
                x: 60.0,
                y: 80.0,
                width: 220.0,
                height: 70.0,
            },
            color: "#2e7d32".to_owned(),
            opacity: 100,
            text: "Free text beside a field".to_owned(),
            font_size: 12.0,
            align: FreeTextAlign::Left,
        },
    )
    .expect("create free text");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Text {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("same_page_field".to_owned()),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: None,
        },
    )
    .expect("create form field");

    let dir = TempDir::new("freetext-form");
    let output = dir.path().join("shared-annots.pdf");
    save(&document_id, &output);

    // Independent parse: the page keeps both annotations, and the free text
    // still carries the translated standard entries.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    let page = page_dict(&parsed, 1);
    let annots = page
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    let mut widget_names = Vec::new();
    let mut free_text = None;
    for object in annots {
        let dictionary = dict_of(&parsed, object);
        let subtype = dictionary
            .get(b"Subtype")
            .and_then(lopdf::Object::as_name)
            .unwrap_or_default();
        if subtype == b"Widget" {
            if let Ok(title) = dictionary.get(b"T").and_then(lopdf::Object::as_str) {
                widget_names.push(String::from_utf8_lossy(title).into_owned());
            }
        } else if subtype == b"FreeText" {
            free_text = Some(dictionary);
        }
    }
    assert_eq!(widget_names, vec!["same_page_field".to_owned()]);
    let free_text = free_text.expect("free text annotation survives the save");
    assert_eq!(
        free_text.get(b"Q").and_then(lopdf::Object::as_i64).ok(),
        Some(0),
        "left-aligned free text writes /Q"
    );
    assert_eq!(
        free_text.get(b"IT").and_then(lopdf::Object::as_name).ok(),
        Some(b"FreeText".as_slice())
    );

    // A fresh PDFium open sees the field attached to its page and the free
    // text listed, so nothing was detached by the other pass.
    let reopened = open(&output);
    let fields = form_fields(reopened.document_id.clone()).unwrap();
    assert!(
        fields.iter().any(|field| field.name == "same_page_field"),
        "widget must stay attached to its page: {fields:?}"
    );
    let annotations = list_annotations(reopened.document_id.clone()).unwrap();
    assert!(
        annotations
            .iter()
            .any(|annotation| annotation.kind == AnnotationKind::FreeText),
        "free text must still be listed: {annotations:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn deleting_the_last_page_is_refused_in_chinese_without_history_change() {
    let pdf = synthetic::page_operations_pdf();
    let document = open(Path::new(&pdf.path_string()));
    let document_id = document.document_id.clone();

    pdfx_lib::page_ops::delete_page(document_id.clone(), 0).expect("delete first page");
    pdfx_lib::page_ops::delete_page(document_id.clone(), 0).expect("delete second page");
    let before = edit_history(document_id.clone()).unwrap();

    let error = pdfx_lib::page_ops::delete_page(document_id.clone(), 0).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    assert!(
        error.message.contains("至少需要保留一页"),
        "error should be Chinese: {}",
        error.message
    );
    assert_eq!(edit_history(document_id.clone()).unwrap(), before);
    assert_eq!(page_info(document_id.clone(), 0).unwrap().rotation, 0);

    close_document(document_id).expect("close document");
}

#[test]
fn restricted_documents_refuse_page_operations_without_touching_history() {
    let dir = TempDir::new("page-permissions");
    let source = fixture_copy("encrypted-owner-restrictions", &dir, "restricted.pdf");
    let source_bytes = fs::read(&source).expect("read source");

    let document = open(&source);
    assert!(document.read_only);
    let document_id = document.document_id.clone();
    let before = edit_history(document_id.clone()).unwrap();

    let error = pdfx_lib::page_ops::rotate_page(document_id.clone(), 0, 90).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = pdfx_lib::page_ops::delete_page(document_id.clone(), 0).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = pdfx_lib::page_ops::move_page(document_id.clone(), 1, 0).unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = pdfx_lib::page_ops::insert_blank_page(
        document_id.clone(),
        0,
        pdfx_lib::page_ops::BlankPageSize {
            width: 595.0,
            height: 842.0,
        },
    )
    .unwrap_err();
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);

    let after = edit_history(document_id.clone()).unwrap();
    assert_eq!(before, after);
    assert!(!after.can_undo);
    assert!(!after.can_redo);
    assert!(!after.dirty);
    assert_eq!(page_info(document_id.clone(), 0).unwrap().rotation, 0);
    assert_eq!(fs::read(&source).expect("reread source"), source_bytes);

    close_document(document_id).expect("close document");
}

#[test]
fn deleting_the_page_of_an_existing_form_field_does_not_leak_it() {
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, "form").expect("form fixture");
    let source = support::fixtures::fixture_path(fixture);
    let document = open(&source);
    let document_id = document.document_id.clone();
    assert_eq!(document.page_count, 2);
    let before = form_fields(document_id.clone()).expect("list fields");
    assert!(before
        .iter()
        .any(|field| field.name == "notes" && field.page_index == 1));
    assert!(before
        .iter()
        .any(|field| field.name == "full_name" && field.page_index == 0));

    pdfx_lib::page_ops::delete_page(document_id.clone(), 1).expect("delete page 2");
    let after = form_fields(document_id.clone()).expect("list fields after delete");
    assert!(
        after.iter().all(|field| field.name != "notes"),
        "deleted page field must be hidden: {after:?}"
    );
    assert!(
        after.iter().any(|field| field.name == "full_name"),
        "a field on a kept page must stay listed: {after:?}"
    );

    let dir = TempDir::new("delete-existing-form");
    let output = dir.path().join("deleted.pdf");
    save(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(page_ids(&parsed).len(), 1);
    let saved_fields = form_field_names(&parsed);
    for removed in ["notes", "agree", "js_guard", "readonly_note"] {
        assert!(
            !saved_fields.contains(&removed.to_owned()),
            "a field whose page was deleted must not stay in /Fields: {saved_fields:?}"
        );
    }
    for kept in [
        "full_name",
        "email",
        "subscribe",
        "gender",
        "country",
        "department",
    ] {
        assert!(
            saved_fields.contains(&kept.to_owned()),
            "a field on a kept page must be preserved: {saved_fields:?}"
        );
    }

    let reopened = open(&output);
    let fields = form_fields(reopened.document_id.clone()).expect("list reopened fields");
    assert!(!fields.is_empty(), "kept fields must remain fillable");
    assert!(
        fields
            .iter()
            .all(|field| field.page_index == 0 && field.name != "notes"),
        "reopened fields must all belong to the kept page: {fields:?}"
    );
    close_document(reopened.document_id).expect("close reopened");

    // A field created on a kept page is appended to the pruned `/Fields`
    // array, and the deleted page's field still stays out of the output.
    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Text {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("kept_page_field".to_owned()),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: None,
        },
    )
    .expect("create a field on the kept page");
    assert!(created
        .fields
        .iter()
        .any(|field| field.name == "kept_page_field"));
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output with a created field");
    let saved_fields = form_field_names(&parsed);
    assert!(
        saved_fields.contains(&"kept_page_field".to_owned()),
        "a created field must be appended to the pruned array: {saved_fields:?}"
    );
    assert!(
        !saved_fields.contains(&"notes".to_owned()),
        "the pruned field must stay out: {saved_fields:?}"
    );
    assert!(saved_fields.contains(&"full_name".to_owned()));

    // Undo the created field and then the deletion: the next save writes the
    // deleted page's fields again.
    undo_text_edit(document_id.clone()).expect("undo created field");
    undo_text_edit(document_id.clone()).expect("undo delete");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse restored output");
    let saved_fields = form_field_names(&parsed);
    assert!(
        saved_fields.contains(&"notes".to_owned()),
        "undo must restore the deleted page's field: {saved_fields:?}"
    );
    assert_eq!(parsed.get_pages().len(), 2);

    close_document(document_id).expect("close document");
}

#[test]
fn deleting_a_link_target_leaves_untouched_destinations_alone() {
    let pdf = synthetic::page_operations_pdf();
    let source = PathBuf::from(pdf.path_string());
    let source_ids = {
        let parsed = lopdf::Document::load(&source).expect("parse source");
        page_ids(&parsed)
    };
    let document = open(&source);
    let document_id = document.document_id.clone();

    // Page 1's link targets page 3. Deleting page 3 must not rewrite the link
    // annotation on the untouched first page.
    pdfx_lib::page_ops::delete_page(document_id.clone(), 2).expect("delete page 3");
    let dir = TempDir::new("delete-link-target");
    let output = dir.path().join("deleted-target.pdf");
    save(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved_ids = page_ids(&parsed);
    assert_eq!(saved_ids, vec![source_ids[0], source_ids[1]]);
    let page_one = page_dict(&parsed, 1);
    let annots = page_one
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    let link = annots
        .iter()
        .map(|object| dict_of(&parsed, object))
        .find(|dictionary| {
            dictionary
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .is_ok_and(|subtype| subtype == b"Link")
        })
        .expect("the untouched page keeps its link annotation");
    let action = dict_of(&parsed, link.get(b"A").expect("/A"));
    let destination = action
        .get(b"D")
        .expect("/D")
        .as_array()
        .expect("destination array");
    assert_eq!(
        destination[0].as_reference().expect("destination page"),
        source_ids[2],
        "destinations keep pointing at the page object, deleted or not"
    );
    assert!(
        !saved_ids.contains(&source_ids[2]),
        "the destination target is out of the saved page tree"
    );
    // The bookmark points at the kept page 2 and is untouched.
    assert_eq!(outline_destination_page(&parsed), source_ids[1]);

    close_document(document_id).expect("close document");
}
