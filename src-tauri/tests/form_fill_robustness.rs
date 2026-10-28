//! Command-boundary regression tests for malformed or unusual AcroForms.
//!
//! Ticket 47's save pipeline looks fields up by full dotted name with lopdf.
//! These tests pin down what such a lookup must survive: a filled field whose
//! page is deleted before saving, a cyclic `/Kids` graph, duplicate names, a
//! literal dotted name colliding with a nested path, a widget without `/Rect`,
//! and `/Ff`/`/DA` inherited from ancestors. The synthetic PDFs are built raw
//! so the tests do not depend on a fixed malformed fixture.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::commands::{close_document, open_document};
use pdfx_lib::form::{
    fill_form_field, form_fields, FormFieldKind, FormFieldValueInput, FormFieldView,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};

fn fixture_path(id: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id}"));
    support::fixtures::fixture_path(fixture)
}

fn open_at(path: &Path) -> String {
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open document")
        .document_id
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save document");
}

fn fill_named(document_id: &str, name: &str, value: FormFieldValueInput) {
    let fields = form_fields(document_id.to_owned()).expect("fields");
    let field = fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("field {name}: {fields:?}"));
    fill_form_field(document_id.to_owned(), field.field_id.clone(), value).expect("fill field");
}

fn text_field_view<'a>(fields: &'a [FormFieldView], id: &str) -> &'a FormFieldView {
    fields
        .iter()
        .find(|field| field.field_id == id)
        .unwrap_or_else(|| panic!("field id {id}: {fields:?}"))
}

// ---------------------------------------------------------------------------
// Raw PDF builders.
// ---------------------------------------------------------------------------

fn build_raw_pdf(objects: Vec<(u32, Vec<u8>)>, root: u32) -> Vec<u8> {
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
        if offsets[number as usize] == 0 {
            out.extend_from_slice(b"0000000000 65535 f \n");
        } else {
            out.extend_from_slice(
                format!("{:010} 00000 n \n", offsets[number as usize]).as_bytes(),
            );
        }
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            last + 1
        )
        .as_bytes(),
    );
    out
}

fn stale_stream() -> Vec<u8> {
    let content = b"q 1 0 0 rg BT /Helv 12 Tf 2 6 Td (STALE) Tj ET Q\n";
    let mut body = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 248 24] /Resources << /Font << /Helv 3 0 R >> >> /Length {} >>\nstream\n",
        content.len()
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream");
    body
}

fn cyclic_form_pdf() -> Vec<u8> {
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 9 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [9 0 R] >>".to_vec()),
        (5, b"<< /T (cycleA) /Kids [6 0 R] >>".to_vec()),
        (6, b"<< /T (cycleB) /Parent 5 0 R /Kids [5 0 R] >>".to_vec()),
        (9, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (normal) /Rect [72 600 300 624] /P 4 0 R /DA (/Helv 12 Tf 0 g) /V (old) >>".to_vec()),
    ];
    build_raw_pdf(objects, 1)
}

fn duplicate_name_pdf() -> Vec<u8> {
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [7 0 R 6 0 R] >>".to_vec()),
        (5, b"<< /FT /Tx /T (dup) /V (old-a) /DA (/Helv 12 Tf 0 g) /Kids [7 0 R] >>".to_vec()),
        (6, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (dup) /Rect [72 560 300 584] /P 4 0 R /DA (/Helv 12 Tf 0 g) /V (old-b) /AP << /N 8 0 R >> >>".to_vec()),
        (7, b"<< /Type /Annot /Subtype /Widget /Parent 5 0 R /Rect [72 600 300 624] /P 4 0 R /AP << /N 8 0 R >> >>".to_vec()),
        (8, stale_stream()),
    ];
    build_raw_pdf(objects, 1)
}

fn dotted_collision_pdf() -> Vec<u8> {
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 8 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [7 0 R 8 0 R] >>".to_vec()),
        (5, b"<< /T (a) /FT /Tx /DA (/Helv 12 Tf 0 g) /Kids [6 0 R] >>".to_vec()),
        (6, b"<< /T (b) /Parent 5 0 R /V (nested-old) /Kids [7 0 R] >>".to_vec()),
        (7, b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [72 650 300 674] /P 4 0 R /AP << /N 9 0 R >> >>".to_vec()),
        (8, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (a.b) /Rect [72 600 300 624] /P 4 0 R /DA (/Helv 12 Tf 0 g) /V (literal-old) /AP << /N 9 0 R >> >>".to_vec()),
        (9, stale_stream()),
    ];
    build_raw_pdf(objects, 1)
}

fn rectless_pdf() -> Vec<u8> {
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R] >>".to_vec()),
        (5, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (rectless) /P 4 0 R /DA (/Helv 12 Tf 0 g) /V (old) >>".to_vec()),
        (6, b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (normal-rect) /Rect [72 600 300 624] /P 4 0 R /DA (/Helv 12 Tf 0 g) /V (old) >>".to_vec()),
    ];
    build_raw_pdf(objects, 1)
}

fn inherited_flags_pdf() -> Vec<u8> {
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [7 0 R] >>".to_vec()),
        (5, b"<< /T (person) /FT /Tx /Ff 4096 /DA (/Helv 18 Tf 0 g) /Kids [6 0 R] >>".to_vec()),
        (6, b"<< /T (name) /Parent 5 0 R /V (old) /Kids [7 0 R] >>".to_vec()),
        (7, b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [72 600 400 680] /P 4 0 R >>".to_vec()),
    ];
    build_raw_pdf(objects, 1)
}

// ---------------------------------------------------------------------------
// Independent lopdf readers.
// ---------------------------------------------------------------------------

fn resolved<'a>(document: &'a lopdf::Document, object: &'a lopdf::Object) -> &'a lopdf::Object {
    match object {
        lopdf::Object::Reference(id) => document.get_object(*id).expect("referenced object"),
        other => other,
    }
}

fn acroform(document: &lopdf::Document) -> &lopdf::Dictionary {
    let catalog = document
        .get_object(
            document
                .trailer
                .get(b"Root")
                .expect("/Root")
                .as_reference()
                .expect("/Root ref"),
        )
        .expect("catalog")
        .as_dict()
        .expect("catalog dict");
    resolved(document, catalog.get(b"AcroForm").expect("/AcroForm"))
        .as_dict()
        .expect("AcroForm")
}

fn field_entries(document: &lopdf::Document) -> &[lopdf::Object] {
    resolved(
        document,
        acroform(document).get(b"Fields").expect("/Fields"),
    )
    .as_array()
    .expect("/Fields array")
}

/// Field ids and names of every terminal field, in document order.
fn terminal_fields(document: &lopdf::Document) -> Vec<(lopdf::ObjectId, String)> {
    fn walk(
        document: &lopdf::Document,
        fields: &[lopdf::Object],
        found: &mut Vec<(lopdf::ObjectId, String)>,
    ) {
        for entry in fields {
            let Some(id) = entry.as_reference().ok() else {
                continue;
            };
            let Some(dictionary) = document.get_dictionary(id).ok() else {
                continue;
            };
            let mut child_fields = Vec::new();
            if let Ok(kids) = dictionary
                .get(b"Kids")
                .and_then(|kids| resolved(document, kids).as_array().cloned())
            {
                for kid in kids {
                    let Some(kid_id) = kid.as_reference().ok() else {
                        continue;
                    };
                    if document
                        .get_dictionary(kid_id)
                        .is_ok_and(|kid| kid.has(b"T"))
                    {
                        child_fields.push(kid);
                    }
                }
            }
            if child_fields.is_empty() {
                if let Ok(title) = dictionary.get(b"T").and_then(lopdf::Object::as_str) {
                    found.push((id, decode_pdf_text(title)));
                }
            } else {
                walk(document, &child_fields, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(document, field_entries(document), &mut found);
    found
}

fn find_field_id(document: &lopdf::Document, name: &str) -> Option<lopdf::ObjectId> {
    terminal_fields(document)
        .into_iter()
        .find(|(_, field_name)| field_name == name)
        .map(|(id, _)| id)
}

fn decode_pdf_text(bytes: &[u8]) -> String {
    if let Some(units) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = units
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn value_of(document: &lopdf::Document, id: lopdf::ObjectId) -> Option<String> {
    document
        .get_dictionary(id)
        .ok()?
        .get(b"V")
        .ok()?
        .as_str()
        .ok()
        .map(decode_pdf_text)
}

/// The `/AP /N` reference of a widget, if it has one.
fn normal_appearance_ref(
    document: &lopdf::Document,
    id: lopdf::ObjectId,
) -> Option<lopdf::ObjectId> {
    let widget = document.get_dictionary(id).ok()?;
    let appearance = resolved(document, widget.get(b"AP").ok()?).as_dict().ok()?;
    appearance.get(b"N").ok()?.as_reference().ok()
}

fn appearance_content(document: &lopdf::Document, id: lopdf::ObjectId) -> String {
    let widget = document.get_dictionary(id).expect("widget");
    let appearance = resolved(document, widget.get(b"AP").expect("/AP"))
        .as_dict()
        .expect("/AP dictionary");
    let stream = resolved(document, appearance.get(b"N").expect("/N"))
        .as_stream()
        .expect("/N stream");
    String::from_utf8_lossy(&stream.decompressed_content().expect("appearance content"))
        .into_owned()
}

fn assert_fresh_appearance(document: &lopdf::Document, id: lopdf::ObjectId, label: &str) {
    let content = appearance_content(document, id);
    assert!(
        !content.contains("STALE"),
        "{label} must not keep its stale source appearance: {content:?}"
    );
    assert!(
        content.contains("/PdfxNoto"),
        "{label} must use the shared embedded subset: {content:?}"
    );
}

fn byte_count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// A fill recorded in the history must not fail the save after its page is
/// deleted: `/Fields` prunes the field first, so the save edits must skip it.
#[test]
fn filling_a_field_then_deleting_its_page_still_saves() {
    let dir = support::temp::TempDir::new("fill-robust-delete");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");
    let document_id = open_at(&source);

    // One Pdfium-written text field and one overlay-only choice field, both on
    // the page that is about to be deleted.
    fill_named(
        &document_id,
        "comments",
        FormFieldValueInput::Text {
            text: "合同已审阅".to_owned(),
        },
    );
    fill_named(
        &document_id,
        "department",
        FormFieldValueInput::Choice {
            value: "Design".to_owned(),
        },
    );
    pdfx_lib::page_ops::delete_page(document_id.clone(), 1).expect("delete page 2");

    let output = dir.path().join("deleted.pdf");
    save_as(&document_id, &output);

    // Undo the deletion: the fills are still in history and must be written
    // again, with fresh appearances, on a save that now includes the page.
    let undone = pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo delete");
    assert_eq!(undone.page_count, 2);
    let restored = dir.path().join("restored.pdf");
    save_as(&document_id, &restored);
    close_document(document_id).expect("close");

    let parsed = lopdf::Document::load(&output).expect("parse deleted output");
    let names: Vec<String> = terminal_fields(&parsed)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(!names.iter().any(|name| name == "comments"));
    assert!(!names.iter().any(|name| name == "department"));

    let parsed = lopdf::Document::load(&restored).expect("parse restored output");
    let document_id = open_at(&restored);
    let fields = form_fields(document_id.clone()).expect("restored fields");
    let comments = fields
        .iter()
        .find(|field| field.name == "comments")
        .expect("comments restored");
    assert_eq!(comments.value.as_deref(), Some("合同已审阅"));
    let department = fields
        .iter()
        .find(|field| field.name == "department")
        .expect("department restored");
    assert_eq!(department.value.as_deref(), Some("Design"));
    close_document(document_id).expect("close restored");

    // The restored fields carry fresh appearances, not the fixture's stale ones.
    let comments_id = find_field_id(&parsed, "comments").expect("comments field object");
    assert_eq!(
        value_of(&parsed, comments_id).as_deref(),
        Some("合同已审阅")
    );
    assert_fresh_appearance(&parsed, comments_id, "restored comments");
    let department_id = find_field_id(&parsed, "department").expect("department field object");
    assert_fresh_appearance(&parsed, department_id, "restored department");
}

/// A cyclic `/Kids` graph must not overflow the stack while filling or saving
/// another, healthy field.
#[test]
fn a_cyclic_kids_graph_does_not_crash_fill_or_save() {
    let dir = support::temp::TempDir::new("fill-robust-cycle");
    let source = dir.path().join("cycle.pdf");
    fs::write(&source, cyclic_form_pdf()).expect("write cyclic pdf");
    let document_id = open_at(&source);

    let fields = form_fields(document_id.clone()).expect("fields");
    assert!(
        fields.iter().any(|field| field.name == "normal"),
        "PDFium exposes the healthy field next to the malformed cycle: {fields:?}"
    );
    fill_named(
        &document_id,
        "normal",
        FormFieldValueInput::Text {
            text: "new".to_owned(),
        },
    );
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");

    let document_id = open_at(&output);
    let fields = form_fields(document_id.clone()).expect("reopened fields");
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "normal")
            .and_then(|field| field.value.as_deref()),
        Some("new")
    );
    close_document(document_id).expect("close reopened");
}

/// Two field dictionaries sharing a name are one logical field to a viewer.
/// Filling either must update both and give both widgets a fresh appearance.
#[test]
fn duplicate_field_names_update_every_same_named_widget() {
    let dir = support::temp::TempDir::new("fill-robust-duplicate");
    let source = dir.path().join("duplicate.pdf");
    fs::write(&source, duplicate_name_pdf()).expect("write duplicate pdf");
    let document_id = open_at(&source);

    let fields = form_fields(document_id.clone()).expect("fields");
    let clicked = text_field_view(&fields, "0:1").clone();
    assert_eq!(clicked.name, "dup");
    fill_form_field(
        document_id.clone(),
        clicked.field_id.clone(),
        FormFieldValueInput::Text {
            text: "NEW".to_owned(),
        },
    )
    .expect("fill duplicate");
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");

    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(value_of(&parsed, (5, 0)).as_deref(), Some("NEW"));
    assert_eq!(value_of(&parsed, (6, 0)).as_deref(), Some("NEW"));
    // Widget 7 belongs to field 5's `/Kids`; widget 6 is the merged field both
    // sharing the name. Neither may keep the stale source stream.
    for widget_id in [(6, 0), (7, 0)] {
        assert_fresh_appearance(&parsed, widget_id, "duplicate-name widget");
        assert_ne!(
            normal_appearance_ref(&parsed, widget_id),
            Some((8, 0)),
            "a fresh appearance must not reuse the stale source stream"
        );
    }
}

/// A literal `/T (a.b)` and a nested `a` > `b` both resolve to `a.b`. The fill
/// clicked the literal widget; both must end up with the new value.
#[test]
fn a_literal_dotted_name_updates_the_nested_field_too() {
    let dir = support::temp::TempDir::new("fill-robust-dotted");
    let source = dir.path().join("dotted.pdf");
    fs::write(&source, dotted_collision_pdf()).expect("write dotted pdf");
    let document_id = open_at(&source);

    let fields = form_fields(document_id.clone()).expect("fields");
    let clicked = text_field_view(&fields, "0:1").clone();
    assert_eq!(clicked.name, "a.b");
    fill_form_field(
        document_id.clone(),
        clicked.field_id.clone(),
        FormFieldValueInput::Text {
            text: "NEW".to_owned(),
        },
    )
    .expect("fill literal dotted field");
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");

    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(value_of(&parsed, (6, 0)).as_deref(), Some("NEW"));
    assert_eq!(value_of(&parsed, (8, 0)).as_deref(), Some("NEW"));
    assert_fresh_appearance(&parsed, (7, 0), "nested a.b widget");
    assert_fresh_appearance(&parsed, (8, 0), "literal a.b widget");
}

/// A terminal widget with no `/Rect` is odd but must not panic or break the
/// save; its value is still written, it just gets no appearance.
#[test]
fn a_widget_without_a_rect_does_not_break_fill_or_save() {
    let dir = support::temp::TempDir::new("fill-robust-rectless");
    let source = dir.path().join("rectless.pdf");
    fs::write(&source, rectless_pdf()).expect("write rectless pdf");
    let document_id = open_at(&source);

    fill_named(
        &document_id,
        "rectless",
        FormFieldValueInput::Text {
            text: "NEW".to_owned(),
        },
    );
    fill_named(
        &document_id,
        "normal-rect",
        FormFieldValueInput::Text {
            text: "NEW".to_owned(),
        },
    );
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");

    let document_id = open_at(&output);
    let fields = form_fields(document_id.clone()).expect("reopened fields");
    for name in ["rectless", "normal-rect"] {
        assert_eq!(
            fields
                .iter()
                .find(|field| field.name == name)
                .and_then(|field| field.value.as_deref()),
            Some("NEW"),
            "{name} keeps its value"
        );
    }
    close_document(document_id).expect("close reopened");
}

/// `/Ff` and `/DA` inherited from the parent drive the appearance: 4096 is the
/// multiline flag, 18 is the inherited font size. For an 80pt-high widget the
/// multiline first baseline is 80 - 2 - 18 * 0.8 = 63.6.
#[test]
fn inherited_flags_and_da_drive_the_appearance() {
    let dir = support::temp::TempDir::new("fill-robust-inherited");
    let source = dir.path().join("inherited.pdf");
    fs::write(&source, inherited_flags_pdf()).expect("write inherited pdf");
    let document_id = open_at(&source);

    let fields = form_fields(document_id.clone()).expect("fields");
    let person = fields
        .iter()
        .find(|field| field.name == "person.name")
        .expect("person.name");
    assert!(person.multiline, "PDFium sees the inherited multiline flag");
    assert_eq!(
        person.kind,
        FormFieldKind::Text,
        "PDFium sees the inherited /FT"
    );
    fill_form_field(
        document_id.clone(),
        person.field_id.clone(),
        FormFieldValueInput::Text {
            text: "Katherine Johnson".to_owned(),
        },
    )
    .expect("fill person.name");
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");

    let parsed = lopdf::Document::load(&output).expect("parse output");
    assert_eq!(
        value_of(&parsed, (6, 0)).as_deref(),
        Some("Katherine Johnson")
    );
    let content = appearance_content(&parsed, (7, 0));
    assert!(content.contains("/PdfxNoto 18 Tf"), "{content:?}");
    assert!(content.contains("1 0 0 1 2 63.6 Tm"), "{content:?}");
    assert!(!content.contains("STALE"), "{content:?}");
}

/// Repeated saving of an untouched form keeps the byte-level guarantees.
#[test]
fn malformed_forms_still_save_without_need_appearances() {
    let dir = support::temp::TempDir::new("fill-robust-need-appearances");
    let source = dir.path().join("duplicate.pdf");
    fs::write(&source, duplicate_name_pdf()).expect("write duplicate pdf");
    let document_id = open_at(&source);
    fill_named(
        &document_id,
        "dup",
        FormFieldValueInput::Text {
            text: "NEW".to_owned(),
        },
    );
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close");
    let bytes = fs::read(&output).expect("output bytes");
    assert_eq!(byte_count(&bytes, b"/NeedAppearances"), 0);
}
