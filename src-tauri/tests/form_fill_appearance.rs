//! Command-boundary tests for appearance fidelity when filling the AcroForm
//! fields a PDF already has.
//!
//! The fixture's widgets all carry deliberately stale red appearances, and its
//! field shapes include the ones Pdfium's annotation API cannot write directly:
//! a terminal field whose value lives on the parent with the widget in `/Kids`,
//! and a nested `/Parent`-inherited field. Saved output is parsed independently
//! with lopdf and rendered with PDFium at a fixed DPI.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::commands::{close_document, open_document, CommandErrorCode};
use pdfx_lib::form::{fill_form_field, form_fields, FormFieldKind, FormFieldView, FormFieldValueInput};
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

fn field_named<'a>(fields: &'a [FormFieldView], name: &str) -> &'a FormFieldView {
    fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("field {name}: {fields:?}"))
}

fn fill(document_id: &str, field_id: String, value: FormFieldValueInput) {
    fill_form_field(document_id.to_owned(), field_id, value).expect("fill field");
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save document");
}

// ---------------------------------------------------------------------------
// Independent lopdf readers.
// ---------------------------------------------------------------------------

fn catalog(document: &lopdf::Document) -> &lopdf::Dictionary {
    let root = document
        .trailer
        .get(b"Root")
        .expect("/Root")
        .as_reference()
        .expect("/Root reference");
    document
        .get_object(root)
        .expect("catalog")
        .as_dict()
        .expect("catalog dictionary")
}

fn resolved<'a>(document: &'a lopdf::Document, object: &'a lopdf::Object) -> &'a lopdf::Object {
    match object {
        lopdf::Object::Reference(id) => document.get_object(*id).expect("referenced object"),
        other => other,
    }
}

fn acroform(document: &lopdf::Document) -> &lopdf::Dictionary {
    resolved(document, catalog(document).get(b"AcroForm").expect("/AcroForm"))
        .as_dict()
        .expect("AcroForm dictionary")
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

fn text_of(object: &lopdf::Object) -> String {
    decode_pdf_text(object.as_str().expect("text string"))
}

/// Finds a field by its full dotted name, walking `/Fields` and `/Kids`.
///
/// Returns a clone so the recursive walk can build a child list without
/// borrowing it from the input array.
fn find_field(document: &lopdf::Document, name: &str) -> lopdf::Dictionary {
    fn walk(
        document: &lopdf::Document,
        fields: &[lopdf::Object],
        prefix: &str,
        target: &str,
    ) -> Option<lopdf::Dictionary> {
        for entry in fields {
            let Some(id) = entry.as_reference().ok() else {
                continue;
            };
            let Some(dictionary) = document.get_dictionary(id).ok() else {
                continue;
            };
            let title = dictionary
                .get(b"T")
                .ok()
                .and_then(|object| object.as_str().ok())
                .map(decode_pdf_text);
            let full = match &title {
                Some(title) if prefix.is_empty() => title.clone(),
                Some(title) => format!("{prefix}.{title}"),
                None => prefix.to_owned(),
            };
            let children: Vec<lopdf::Object> = dictionary
                .get(b"Kids")
                .ok()
                .and_then(|kids| resolved(document, kids).as_array().ok())
                .map(|kids| {
                    kids.iter()
                        .filter(|kid| {
                            kid.as_reference()
                                .ok()
                                .and_then(|id| document.get_dictionary(id).ok())
                                .map(|kid| kid.has(b"T"))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if title.is_some() && full == target && children.is_empty() {
                return Some(dictionary.clone());
            }
            if let Some(found) = walk(document, &children, &full, target) {
                return Some(found);
            }
        }
        None
    }

    let fields = acroform(document)
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array");
    walk(document, fields, "", name).unwrap_or_else(|| panic!("field {name} in saved document"))
}

/// `/Kids` object ids of a field, or the field's own id when it is a widget.
fn widget_ids(
    document: &lopdf::Document,
    field: &lopdf::Dictionary,
    field_name: &str,
) -> Vec<lopdf::ObjectId> {
    if let Some(kids) = field
        .get(b"Kids")
        .ok()
        .and_then(|kids| resolved(document, kids).as_array().ok())
    {
        return kids
            .iter()
            .filter_map(|kid| kid.as_reference().ok())
            .collect();
    }
    // Without /Kids the terminal field object must itself be a widget.
    acroform(document)
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .iter()
        .find_map(|entry| {
            let id = entry.as_reference().ok()?;
            let dictionary = document.get_dictionary(id).ok()?;
            let title = dictionary
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(decode_pdf_text)?;
            (title == field_name).then_some(vec![id])
        })
        .unwrap_or_else(|| panic!("widget ids for {field_name}"))
}

/// Normal appearance stream for a text or choice widget (`/AP /N` stream).
fn normal_appearance<'a>(
    document: &'a lopdf::Document,
    widget: &'a lopdf::Dictionary,
) -> &'a lopdf::Stream {
    let appearance = resolved(document, widget.get(b"AP").expect("/AP"))
        .as_dict()
        .expect("/AP dictionary");
    resolved(document, appearance.get(b"N").expect("/N"))
        .as_stream()
        .expect("/N stream")
}

fn appearance_content(document: &lopdf::Document, widget: &lopdf::Dictionary) -> String {
    String::from_utf8_lossy(
        &normal_appearance(document, widget)
            .decompressed_content()
            .expect("appearance stream content"),
    )
    .into_owned()
}

fn widget(document: &lopdf::Document, id: lopdf::ObjectId) -> &lopdf::Dictionary {
    document
        .get_dictionary(id)
        .expect("widget dictionary")
}

#[test]
fn fills_a_kids_terminal_and_a_nested_field_with_fresh_appearances() {
    let dir = support::temp::TempDir::new("fill-appearance-core");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");
    let source_bytes = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let document_id = open_at(&source);
    let fields = form_fields(document_id.clone()).expect("discover fields");
    let membership = field_named(&fields, "membership_id");
    assert_eq!(membership.kind, FormFieldKind::Text);
    assert_eq!(membership.value.as_deref(), Some("OLD-0001"));
    let person = field_named(&fields, "person.name");
    assert_eq!(person.kind, FormFieldKind::Text);
    assert_eq!(person.value.as_deref(), Some("Grace Hopper"));

    fill(
        &document_id,
        membership.field_id.clone(),
        FormFieldValueInput::Text {
            text: "MEM-2026-0001".to_owned(),
        },
    );
    fill(
        &document_id,
        person.field_id.clone(),
        FormFieldValueInput::Text {
            text: "Katherine Johnson".to_owned(),
        },
    );

    // The live session reflects both fills, including the field whose value
    // cannot be written through Pdfium's annotation API.
    let after = form_fields(document_id.clone()).expect("fields after fill");
    assert_eq!(
        field_named(&after, "membership_id").value.as_deref(),
        Some("MEM-2026-0001")
    );
    assert_eq!(
        field_named(&after, "person.name").value.as_deref(),
        Some("Katherine Johnson")
    );

    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    assert_eq!(fs::read(&source).expect("read source"), source_bytes);
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_mtime
    );
    assert!(
        fs::read(&output)
            .expect("read output")
            .starts_with(&source_bytes),
        "saving appends to the source bytes"
    );

    let parsed = lopdf::Document::load(&output).expect("parse output");

    // Values live on the terminal field objects, not on the widget annotations.
    let membership_field = find_field(&parsed, "membership_id");
    assert_eq!(
        text_of(membership_field.get(b"V").expect("membership /V")),
        "MEM-2026-0001"
    );
    assert!(membership_field.has(b"Kids"));
    let person_field = find_field(&parsed, "person.name");
    assert_eq!(
        text_of(person_field.get(b"V").expect("person.name /V")),
        "Katherine Johnson"
    );

    // Every filled widget has an explicit /AP /N that is a fresh stream, not
    // the stale red source appearance.
    for (field_name, field) in [
        ("membership_id", membership_field),
        ("person.name", person_field),
    ] {
        for id in widget_ids(&parsed, &field, field_name) {
            let widget = widget(&parsed, id);
            assert!(
                widget.has(b"AP"),
                "{field_name} widget {id:?} has no explicit appearance"
            );
            let content = appearance_content(&parsed, widget);
            assert!(
                content.contains("1 1 1 rg"),
                "{field_name} widget {id:?} appearance is not regenerated: {content:?}"
            );
            assert!(
                !content.contains("STALE"),
                "{field_name} widget {id:?} kept the stale appearance: {content:?}"
            );
        }
    }

    // The new appearance uses the embedded CJK-capable font, not the fixture's
    // Helvetica.
    let dr = resolved(&parsed, acroform(&parsed).get(b"DR").expect("/DR"))
        .as_dict()
        .expect("/DR dictionary");
    let fonts = resolved(&parsed, dr.get(b"Font").expect("/DR /Font"))
        .as_dict()
        .expect("/DR /Font dictionary");
    let noto = fonts
        .iter()
        .find(|(name, _)| name.starts_with(b"PdfxNoto"))
        .map(|(_, font)| font)
        .expect("embedded Noto font in /DR");
    let noto = resolved(&parsed, noto).as_dict().expect("font dictionary");
    assert_eq!(
        noto.get(b"Subtype").and_then(|object| object.as_name()).ok(),
        Some(b"Type0".as_slice())
    );
    let descendants = resolved(
        &parsed,
        noto.get(b"DescendantFonts")
            .expect("/DescendantFonts"),
    )
    .as_array()
    .expect("descendant fonts");
    let descendant = resolved(&parsed, &descendants[0])
        .as_dict()
        .expect("descendant font");
    let descriptor = resolved(
        &parsed,
        descendant
            .get(b"FontDescriptor")
            .expect("/FontDescriptor"),
    )
    .as_dict()
    .expect("font descriptor");
    let program = resolved(&parsed, descriptor.get(b"FontFile2").expect("/FontFile2"))
        .as_stream()
        .expect("embedded font program");
    assert!(
        !program.content.is_empty(),
        "the embedded font program is not empty"
    );

    // The AcroForm never asks readers to synthesize appearances.
    assert!(
        !acroform(&parsed).has(b"NeedAppearances"),
        "the saved AcroForm must not set /NeedAppearances"
    );

    // PDFium reopen sees the new values.
    let reopened = open_at(&output);
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(
        field_named(&fields, "membership_id").value.as_deref(),
        Some("MEM-2026-0001")
    );
    assert_eq!(
        field_named(&fields, "person.name").value.as_deref(),
        Some("Katherine Johnson")
    );
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close source");
}

#[test]
fn read_only_fields_still_refuse_filling() {
    let document_id = open_at(&fixture_path("form-acroform"));
    let fields = form_fields(document_id.clone()).expect("fields");
    let readonly = field_named(&fields, "readonly_note");
    let error = fill_form_field(
        document_id.clone(),
        readonly.field_id.clone(),
        FormFieldValueInput::Text {
            text: "changed".to_owned(),
        },
    )
    .expect_err("read-only fields cannot be filled");
    assert_eq!(error.code, CommandErrorCode::FormFieldReadOnly);
    close_document(document_id).expect("close document");
}
