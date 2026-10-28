//! Command-boundary tests for creating text Form Fields.
//!
//! The tests use the same Rust functions the Tauri IPC layer exposes. Saved
//! output is reopened through the command boundary with a fresh PDFium parse
//! and parsed independently with lopdf, so the structure other readers see is
//! checked rather than PDFX's in-memory session state.

mod support;

use support::synthetic;

use pdfx_lib::commands::{close_document, open_document, CommandErrorCode};
use pdfx_lib::form::{
    fill_form_field, form_fields, missing_required_fields, FormFieldKind, FormFieldRect,
    FormFieldValueInput,
};
use pdfx_lib::form_create::{create_form_text_field, CreateFormTextFieldRequest};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn create_request(rect: FormFieldRect) -> CreateFormTextFieldRequest {
    CreateFormTextFieldRequest {
        rect,
        name: None,
        tooltip: Some("请输入姓名".to_owned()),
        required: true,
        multiline: false,
        font_size: Some(12.0),
    }
}

#[test]
fn creates_a_text_field_that_discovery_reports_and_undo_removes() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };

    let created = create_form_text_field(document_id.clone(), 0, create_request(rect))
        .expect("create a text field");
    assert!(
        created.history.dirty,
        "creating a field dirties the document"
    );
    assert!(created.history.can_undo);
    assert_eq!(created.history.revision, created.history.saved_revision + 1);

    let fields = form_fields(document_id.clone()).expect("discover fields");
    let field = fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("the created field is discovered");
    assert_eq!(field.kind, FormFieldKind::Text);
    assert_eq!(field.name, "text_field_1");
    assert_eq!(field.page_index, 0);
    assert_eq!(field.rect, rect);
    assert_eq!(field.tooltip.as_deref(), Some("请输入姓名"));
    assert!(field.required);
    assert!(!field.multiline);
    assert!(!field.read_only);
    assert_eq!(field.font_size, Some(12.0));
    assert!(
        field.created,
        "the property panel needs to identify new fields"
    );
    assert_eq!(field.value, None);

    let undo = undo_text_edit(document_id.clone()).expect("undo the creation");
    assert_eq!(undo.page_index, 0);
    assert!(
        form_fields(document_id.clone())
            .expect("fields after undo")
            .iter()
            .all(|field| field.field_id != created.field_id),
        "undo removes the created field from discovery and the tab order"
    );
    assert!(!undo.history.can_undo && undo.history.can_redo);

    let redo = redo_text_edit(document_id.clone()).expect("redo the creation");
    assert_eq!(redo.page_index, 0);
    let restored = form_fields(document_id.clone()).expect("fields after redo");
    let field = restored
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("redo re-inserts the same field id");
    assert_eq!(field.rect, rect);
    assert_eq!(field.name, "text_field_1");
    assert!(field.required);
    assert_eq!(field.tooltip.as_deref(), Some("请输入姓名"));
    assert!(!redo.history.can_redo && redo.history.dirty);

    close_document(document_id).expect("close document");
}

#[test]
fn rejects_invalid_bounds_and_duplicate_names_without_creating_a_field() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };
    let mut with_name = create_request(rect);
    with_name.name = Some("customer_name".to_owned());
    create_form_text_field(document_id.clone(), 0, with_name)
        .expect("create the first named field");

    let mut duplicate = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 24.0,
    });
    duplicate.name = Some("customer_name".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, duplicate)
        .expect_err("duplicate names are rejected");
    assert_eq!(error.code, CommandErrorCode::FormFieldNameDuplicate);

    let fields = form_fields(document_id.clone()).expect("fields");
    assert_eq!(
        fields
            .iter()
            .filter(|field| field.name == "customer_name")
            .count(),
        1,
        "a rejected duplicate does not create a second field"
    );

    let mut empty_name = create_request(FormFieldRect {
        x: 72.0,
        y: 180.0,
        width: 180.0,
        height: 24.0,
    });
    empty_name.name = Some("  ".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, empty_name)
        .expect_err("blank names are rejected");
    assert_eq!(error.code, CommandErrorCode::FormFieldNameInvalid);

    let mut outside = create_request(FormFieldRect {
        x: 500.0,
        y: 700.0,
        width: 200.0,
        height: 100.0,
    });
    outside.name = Some("outside".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, outside)
        .expect_err("a widget must fit on its page");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert_eq!(
        history.revision,
        created_revision(&document_id),
        "rejected creations leave the history untouched"
    );

    close_document(document_id).expect("close document");
}

fn created_revision(document_id: &str) -> u64 {
    pdfx_lib::text_edit::edit_history(document_id.to_owned())
        .expect("history")
        .revision
}

#[test]
fn fills_a_created_field_and_the_required_gate_follows_the_value() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("full_name".to_owned());
    request.tooltip = None;
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create a field");

    let missing = missing_required_fields(document_id.clone()).expect("required fields");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].field_id, created.field_id);

    let filled = pdfx_lib::form::fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Text {
            text: "张三".to_owned(),
        },
    )
    .expect("fill the created field without saving");
    let field = filled
        .fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("still discovered");
    assert_eq!(field.value.as_deref(), Some("张三"));
    assert!(field.created);
    assert!(filled.history.dirty);
    assert!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .is_empty(),
        "a filled required field no longer blocks saving"
    );

    undo_text_edit(document_id.clone()).expect("undo the value");
    let after_undo = form_fields(document_id.clone()).expect("fields after undo");
    let field = after_undo
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("undo restores the field, not its record");
    assert_eq!(field.value, None, "undo restores the empty value");
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1
    );

    redo_text_edit(document_id.clone()).expect("redo the value");
    let after_redo = form_fields(document_id.clone()).expect("fields after redo");
    assert_eq!(
        after_redo
            .iter()
            .find(|field| field.field_id == created.field_id)
            .and_then(|field| field.value.as_deref()),
        Some("张三")
    );

    close_document(document_id).expect("close document");
}

#[test]
fn updates_and_deletes_created_fields_through_the_history() {
    use pdfx_lib::form_create::{
        delete_form_text_field, update_form_text_field, UpdateFormTextFieldRequest,
    };

    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let mut first = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    first.name = Some("first".to_owned());
    let first = create_form_text_field(document_id.clone(), 0, first).expect("first field");
    let mut second = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 24.0,
    });
    second.name = Some("second".to_owned());
    second.required = false;
    second.tooltip = None;
    let second = create_form_text_field(document_id.clone(), 0, second).expect("second field");

    let duplicate = update_form_text_field(
        document_id.clone(),
        second.field_id.clone(),
        UpdateFormTextFieldRequest {
            name: "first".to_owned(),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: 12.0,
        },
    )
    .expect_err("renaming onto another field is rejected");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);
    assert_eq!(
        form_fields(document_id.clone())
            .expect("fields")
            .iter()
            .find(|field| field.field_id == second.field_id)
            .map(|field| field.name.as_str()),
        Some("second"),
        "the rejected rename changes nothing"
    );

    let updated = update_form_text_field(
        document_id.clone(),
        second.field_id.clone(),
        UpdateFormTextFieldRequest {
            name: "  renamed  ".to_owned(),
            tooltip: Some("请输入备注".to_owned()),
            required: true,
            multiline: true,
            font_size: 16.0,
        },
    )
    .expect("update the properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("still discovered");
    assert_eq!(field.name, "renamed");
    assert_eq!(field.tooltip.as_deref(), Some("请输入备注"));
    assert!(field.required);
    assert!(field.multiline);
    assert_eq!(field.font_size, Some(16.0));
    assert_eq!(field.rect.width, 180.0, "property edits keep the geometry");
    assert!(updated.history.dirty);

    undo_text_edit(document_id.clone()).expect("undo the update");
    let restored = form_fields(document_id.clone()).expect("fields after undo");
    let field = restored
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("field");
    assert_eq!(field.name, "second");
    assert_eq!(field.tooltip, None);
    assert!(!field.required && !field.multiline);
    assert_eq!(field.font_size, Some(12.0));

    redo_text_edit(document_id.clone()).expect("redo the update");
    let field = form_fields(document_id.clone())
        .expect("fields after redo")
        .into_iter()
        .find(|field| field.field_id == second.field_id)
        .expect("field");
    assert_eq!(field.name, "renamed");
    assert!(field.required);

    let deleted = delete_form_text_field(document_id.clone(), second.field_id.clone())
        .expect("delete the created field");
    assert!(!deleted.history.can_redo);
    assert!(
        deleted
            .fields
            .iter()
            .all(|field| field.field_id != second.field_id),
        "deletion removes the field and its tab stop"
    );
    assert_eq!(deleted.fields.len(), 1, "the other created field survives");

    undo_text_edit(document_id.clone()).expect("undo the deletion");
    let restored = form_fields(document_id.clone()).expect("fields after undoing deletion");
    let field = restored
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("undo restores the deleted field");
    assert_eq!(field.name, "renamed");
    assert_eq!(field.rect.y, 140.0);
    assert!(field.multiline && field.required);

    redo_text_edit(document_id.clone()).expect("redo the deletion");
    assert!(form_fields(document_id.clone())
        .expect("fields after redo deletion")
        .iter()
        .all(|field| field.field_id != second.field_id));
    assert!(form_fields(document_id.clone())
        .expect("first field")
        .iter()
        .any(|field| field.field_id == first.field_id));

    close_document(document_id).expect("close document");
}

// ---------------------------------------------------------------------------
// Save pipeline and independent-reader assertions.
// ---------------------------------------------------------------------------

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::commands::render_page;
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveResultView};

fn save_as(document_id: &str, path: &Path) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save the document")
}

fn save_changes(document_id: &str) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save the document changes")
}

fn lopdf_catalog(document: &lopdf::Document) -> &lopdf::Dictionary {
    let root = document
        .trailer
        .get(b"Root")
        .expect("trailer Root")
        .as_reference()
        .expect("Root reference");
    document
        .get_object(root)
        .expect("catalog")
        .as_dict()
        .expect("catalog dictionary")
}

fn lopdf_dict<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> &'a lopdf::Dictionary {
    match object {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("referenced object")
            .as_dict()
            .expect("referenced dictionary"),
        other => other.as_dict().expect("direct dictionary"),
    }
}

fn decode_pdf_text(object: &lopdf::Object) -> String {
    let bytes = object.as_str().expect("text string");
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

fn field_named<'a>(document: &'a lopdf::Document, name: &str) -> &'a lopdf::Dictionary {
    let catalog = lopdf_catalog(document);
    let acro_form = lopdf_dict(document, catalog.get(b"AcroForm").expect("/AcroForm"));
    acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .iter()
        .map(|field| lopdf_dict(document, field))
        .find(|field| field.get(b"T").ok().map(decode_pdf_text).as_deref() == Some(name))
        .unwrap_or_else(|| panic!("field {name} in the saved document"))
}

fn stream_content(document: &lopdf::Document, object: &lopdf::Object) -> Vec<u8> {
    let stream = match object {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("referenced stream")
            .as_stream()
            .expect("stream"),
        other => other.as_stream().expect("stream"),
    };
    stream.get_plain_content().expect("plain stream content")
}

fn numbers(array: &lopdf::Object) -> Vec<f32> {
    array
        .as_array()
        .expect("number array")
        .iter()
        .map(|value| match value {
            lopdf::Object::Integer(value) => *value as f32,
            lopdf::Object::Real(value) => *value,
            other => panic!("not a number: {other:?}"),
        })
        .collect()
}

fn assert_rect_close(actual: &[f32], expected: [f32; 4]) {
    assert_eq!(actual.len(), 4, "rectangle has four numbers");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 0.05,
            "rect {actual:?} differs from {expected:?}"
        );
    }
}

struct DecodedPng {
    width: u32,
    rgba: Vec<u8>,
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    use base64::Engine as _;

    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload should be base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header should parse");
    let mut buffer = vec![0; reader.output_buffer_size().expect("output buffer size")];
    let info = reader
        .next_frame(&mut buffer)
        .expect("PNG frame should decode");
    buffer.truncate(info.buffer_size());
    DecodedPng {
        width: info.width,
        rgba: buffer,
    }
}

/// Non-white pixels inside a page-point region, and their bounding box.
fn ink_in_region(
    pixels: &DecodedPng,
    scale: f32,
    region: (f32, f32, f32, f32),
) -> (usize, (f32, f32, f32, f32)) {
    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut count = 0;
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        if pixel[0] >= 245 && pixel[1] >= 245 && pixel[2] >= 245 {
            continue;
        }
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        if x < region.0 || x > region.2 || y < region.1 || y > region.3 {
            continue;
        }
        count += 1;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    (count, (left, top, right, bottom))
}

fn fixture_path(id: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id}"));
    support::fixtures::fixture_path(fixture)
}

#[test]
fn saves_a_created_field_with_ap_da_dr_and_a_cjk_appearance() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read synthetic source");
    let dir = support::temp::TempDir::new("create-text-save");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };
    let mut request = create_request(rect);
    request.name = Some("full_name".to_owned());
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create field");
    fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Text {
            text: "张三".to_owned(),
        },
    )
    .expect("fill the created field");

    save_as(&document_id, &output);
    assert_eq!(
        fs::read(pdf.path_string()).expect("source after save"),
        source,
        "the Source Document is never modified"
    );
    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source),
        "created fields are appended after the unmodified source bytes"
    );

    // PDFium reopens the output: the field is discovered, fillable and its
    // required flag and value survived.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1, "exactly one widget was written");
    let field = &fields[0];
    assert_eq!(field.name, "full_name");
    assert_eq!(field.kind, FormFieldKind::Text);
    assert_eq!(field.value.as_deref(), Some("张三"));
    assert!(field.required);
    assert!(!field.multiline);
    assert!(!field.created, "saved fields are ordinary widgets");
    assert!((field.rect.x - rect.x).abs() < 0.01);
    assert!((field.rect.y - rect.y).abs() < 0.01);
    assert!((field.rect.width - rect.width).abs() < 0.01);
    assert!((field.rect.height - rect.height).abs() < 0.01);
    assert!(
        missing_required_fields(reopened.clone())
            .expect("required fields")
            .is_empty(),
        "the saved required field is already answered"
    );

    let pixels = render_pixels(&reopened, 0, 2.0);
    let (ink, bbox) = ink_in_region(&pixels, 2.0, (76.0, 104.0, 248.0, 120.0));
    assert!(ink > 20, "the CJK value is visible: {ink} dark pixels");
    assert!(
        bbox.2 - bbox.0 > 12.0,
        "the value is painted horizontally: {bbox:?}"
    );
    close_document(reopened).expect("close reopened");

    // Independently parse the AcroForm structure with lopdf.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert!(
        !acro_form.has(b"NeedAppearances"),
        "generated forms never set /NeedAppearances"
    );
    let saved = field_named(&parsed, "full_name");
    assert_eq!(saved.get(b"FT").expect("/FT").as_name().unwrap(), b"Tx");
    assert_eq!(
        saved.get(b"Subtype").expect("/Subtype").as_name().unwrap(),
        b"Widget"
    );
    assert_eq!(
        decode_pdf_text(saved.get(b"TU").expect("/TU")),
        "请输入姓名"
    );
    let flags = saved.get(b"Ff").expect("/Ff").as_i64().expect("flags");
    assert_eq!(flags & (1 << 1), 1 << 1, "Required bit");
    assert_eq!(flags & (1 << 12), 0, "single-line field");
    assert_eq!(
        decode_pdf_text(saved.get(b"V").expect("/V")),
        "张三",
        "non-ASCII /V is UTF-16BE"
    );
    assert_rect_close(
        &numbers(saved.get(b"Rect").expect("/Rect")),
        [72.0, 668.0, 252.0, 692.0],
    );

    // /DA names the embedded font and carries the chosen size.
    let da = decode_pdf_text(saved.get(b"DA").expect("/DA"));
    assert!(da.contains("PdfxNoto"), "got /DA {da:?}");
    assert!(da.contains("12 Tf"), "got /DA {da:?}");

    // The widget has an explicit /AP /N appearance in the same user rect.
    let appearance = lopdf_dict(&parsed, saved.get(b"AP").expect("/AP"));
    let normal = appearance.get(b"N").expect("/AP /N");
    let normal_stream = parsed
        .get_object(normal.as_reference().expect("appearance reference"))
        .expect("appearance object");
    let appearance_dict = normal_stream
        .as_stream()
        .expect("appearance stream")
        .dict
        .clone();
    assert_rect_close(
        &numbers(appearance_dict.get(b"BBox").expect("/BBox")),
        [72.0, 668.0, 252.0, 692.0],
    );
    let content = String::from_utf8_lossy(&stream_content(&parsed, normal)).into_owned();
    assert!(content.contains("BT /PdfxNoto 12 Tf"), "got {content:?}");
    assert!(
        content.contains(" Tj"),
        "the value is drawn in the appearance"
    );
    assert!(
        content.contains("1 0 0 1 72 668 cm"),
        "the unrotated appearance matrix: {content:?}"
    );

    // The font is a real Type0/CIDFontType2 with ToUnicode and a subset.
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    let type0 = lopdf_dict(&parsed, fonts.get(b"PdfxNoto").expect("PdfxNoto font"));
    assert_eq!(
        type0.get(b"Subtype").expect("/Subtype").as_name().unwrap(),
        b"Type0"
    );
    assert_eq!(
        type0
            .get(b"Encoding")
            .expect("/Encoding")
            .as_name()
            .unwrap(),
        b"Identity-H"
    );
    let descendants = type0
        .get(b"DescendantFonts")
        .expect("/DescendantFonts")
        .as_array()
        .expect("descendant array");
    let cid_font = lopdf_dict(&parsed, &descendants[0]);
    assert_eq!(
        cid_font
            .get(b"Subtype")
            .expect("/Subtype")
            .as_name()
            .unwrap(),
        b"CIDFontType2"
    );
    assert!(cid_font.get(b"CIDToGIDMap").is_ok());
    assert!(cid_font.get(b"W").is_ok());
    let descriptor = lopdf_dict(
        &parsed,
        cid_font.get(b"FontDescriptor").expect("/FontDescriptor"),
    );
    assert!(descriptor.get(b"FontFile2").is_ok());
    let cmap = String::from_utf8_lossy(&stream_content(
        &parsed,
        type0.get(b"ToUnicode").expect("/ToUnicode"),
    ))
    .into_owned();
    assert!(
        cmap.contains("<5F20>"),
        "张 is in the ToUnicode CMap: {cmap}"
    );
    assert!(
        cmap.contains("<4E09>"),
        "三 is in the ToUnicode CMap: {cmap}"
    );
    let program = stream_content(&parsed, descriptor.get(b"FontFile2").expect("/FontFile2"));
    assert!(
        program.starts_with(&[0x00, 0x01, 0x00, 0x00]),
        "FontFile2 is a TrueType program"
    );

    // The widget is wired into the page's /Annots and the form's /Fields.
    let page_id = parsed.get_pages().get(&1).copied().expect("page 1");
    let page = parsed.get_object(page_id).expect("page").as_dict().unwrap();
    let annots = page.get(b"Annots").expect("/Annots").as_array().unwrap();
    assert_eq!(annots.len(), 1);
    assert_eq!(
        saved.get(b"P").expect("/P").as_reference().unwrap(),
        page_id
    );
    assert_eq!(
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .unwrap()
            .len(),
        1
    );

    close_document(document_id).expect("close document");
}

#[test]
fn repeated_saves_never_duplicate_created_fields() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read source");
    let dir = support::temp::TempDir::new("create-text-repeat");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let mut first = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    first.name = Some("first".to_owned());
    first.required = false;
    let first = create_form_text_field(document_id.clone(), 0, first).expect("first field");
    save_as(&document_id, &output);

    // Fill after the first save, then create a multiline field and Save Changes.
    fill_form_field(
        document_id.clone(),
        first.field_id.clone(),
        FormFieldValueInput::Text {
            text: "第一版".to_owned(),
        },
    )
    .expect("fill after save");
    let mut second = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 48.0,
    });
    second.name = Some("second".to_owned());
    second.required = false;
    second.multiline = true;
    let second = create_form_text_field(document_id.clone(), 0, second).expect("second field");
    fill_form_field(
        document_id.clone(),
        second.field_id.clone(),
        FormFieldValueInput::Text {
            text: "第一行\n第二行".to_owned(),
        },
    )
    .expect("fill the multiline field");
    save_changes(&document_id);
    assert!(fs::read(&output).expect("output").starts_with(&source));

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let fields = parsed
        .catalog()
        .and_then(|catalog| catalog.get_deref(b"AcroForm", &parsed))
        .and_then(lopdf::Object::as_dict)
        .and_then(|acro_form| acro_form.get_deref(b"Fields", &parsed))
        .and_then(lopdf::Object::as_array)
        .expect("/Fields");
    assert_eq!(fields.len(), 2, "one /Fields entry per created field");
    let second_dict = field_named(&parsed, "second");
    let flags = second_dict.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(flags & (1 << 12), 1 << 12, "Multiline bit");
    let appearance = lopdf_dict(&parsed, second_dict.get(b"AP").unwrap());
    let content = String::from_utf8_lossy(&stream_content(&parsed, appearance.get(b"N").unwrap()))
        .into_owned();
    assert_eq!(
        content.matches(" Tm ").count(),
        2,
        "multiline values get one Tm per line: {content}"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "first")
            .and_then(|field| field.value.as_deref()),
        Some("第一版")
    );
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "second")
            .and_then(|field| field.value.as_deref()),
        Some("第一行\n第二行")
    );
    assert_eq!(
        fields.iter().filter(|field| field.name == "first").count(),
        1,
        "repeated saves do not duplicate the field"
    );
    close_document(reopened).expect("close reopened");

    // Undo the second creation, save again: the output loses exactly that
    // field and keeps the first one's saved value.
    undo_text_edit(document_id.clone()).expect("undo the multiline fill");
    undo_text_edit(document_id.clone()).expect("undo the second creation");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after undo");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .unwrap()
            .len(),
        1,
        "the undone field is not written"
    );
    assert_eq!(
        decode_pdf_text(field_named(&parsed, "first").get(b"V").expect("/V")),
        "第一版"
    );
    let page_id = parsed.get_pages().get(&1).copied().unwrap();
    let page = parsed.get_object(page_id).unwrap().as_dict().unwrap();
    assert_eq!(
        page.get(b"Annots").unwrap().as_array().unwrap().len(),
        1,
        "the undone widget leaves the page /Annots"
    );

    // Redo the creation and save: it comes back once.
    redo_text_edit(document_id.clone()).expect("redo the second creation");
    redo_text_edit(document_id.clone()).expect("redo the multiline fill");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after redo");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        2
    );

    close_document(document_id).expect("close document");
}

#[test]
fn rotated_pages_get_upright_compensated_appearances() {
    let pdf = synthetic::rotated_crop_pdf();
    let dir = support::temp::TempDir::new("create-text-rotated");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    for page_index in 0..3u32 {
        let mut request = create_request(FormFieldRect {
            x: 40.0,
            y: 200.0,
            width: 180.0,
            height: 30.0,
        });
        request.name = Some(format!("rotated_{page_index}"));
        request.tooltip = None;
        request.required = false;
        let created = create_form_text_field(document_id.clone(), page_index, request)
            .expect("create a field on a rotated page");
        fill_form_field(
            document_id.clone(),
            created.field_id,
            FormFieldValueInput::Text {
                text: "MMMM".to_owned(),
            },
        )
        .expect("fill the rotated field");
    }
    save_as(&document_id, &output);

    // Display rect (40 200 180 30) back to user space on each crop+rotation.
    let expected_rects = [
        [250.0, 120.0, 280.0, 300.0], // /Rotate 90
        [230.0, 280.0, 410.0, 310.0], // /Rotate 180
        [220.0, 460.0, 250.0, 640.0], // /Rotate 270
    ];
    let expected_matrices = [
        "0 1 -1 0 280 120 cm",
        "-1 0 0 -1 410 310 cm",
        "0 -1 1 0 220 640 cm",
    ];
    let parsed = lopdf::Document::load(&output).expect("parse output");
    for (page_index, (expected_rect, expected_matrix)) in expected_rects
        .iter()
        .zip(expected_matrices.iter())
        .enumerate()
    {
        let field = field_named(&parsed, &format!("rotated_{page_index}"));
        assert_rect_close(&numbers(field.get(b"Rect").expect("/Rect")), *expected_rect);
        let appearance = lopdf_dict(&parsed, field.get(b"AP").expect("/AP"));
        let normal = appearance.get(b"N").expect("/AP /N");
        let stream = match normal {
            lopdf::Object::Reference(id) => parsed.get_object(*id).unwrap().as_stream().unwrap(),
            other => other.as_stream().unwrap(),
        };
        assert_rect_close(
            &numbers(stream.dict.get(b"BBox").expect("/BBox")),
            *expected_rect,
        );
        let content = String::from_utf8_lossy(
            &stream
                .get_plain_content()
                .expect("plain appearance content"),
        )
        .into_owned();
        assert!(
            content.contains(expected_matrix),
            "page {page_index} appearance matrix: {content}"
        );
    }

    // The reopened document paints each value upright: the ink box of a wide
    // four-glyph word is wider than it is tall inside a 180x30 widget.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    for page_index in 0..3u32 {
        let pixels = render_pixels(&reopened, page_index, 2.0);
        let (ink, bbox) = ink_in_region(&pixels, 2.0, (42.0, 202.0, 218.0, 228.0));
        assert!(
            ink > 40,
            "page {page_index} paints the appearance: {ink} dark pixels"
        );
        assert!(
            bbox.2 - bbox.0 > 2.0 * (bbox.3 - bbox.1),
            "page {page_index} text is upright, not sideways: {bbox:?}"
        );
    }
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn merges_created_fields_into_an_existing_acroform() {
    let dir = support::temp::TempDir::new("create-text-merge");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-acroform"), &source).expect("copy fixture");
    let output = dir.path().join("output.pdf");
    let source_fields = {
        let parsed = lopdf::Document::load(&source).expect("parse fixture");
        let catalog = lopdf_catalog(&parsed);
        let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .expect("field array")
            .len()
    };

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the form fixture")
        .document_id;
    let before = form_fields(document_id.clone()).expect("existing fields");
    assert!(before.len() >= 10, "the fixture is a populated form");
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 520.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("created_note".to_owned());
    request.required = false;
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create field");
    fill_form_field(
        document_id.clone(),
        created.field_id,
        FormFieldValueInput::Text {
            text: "新增字段".to_owned(),
        },
    )
    .expect("fill the new field");
    save_as(&document_id, &output);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen merged output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(fields.len(), before.len() + 1);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "full_name")
            .and_then(|field| field.value.as_deref()),
        Some("Ada Lovelace"),
        "existing values survive the merge"
    );
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "created_note")
            .and_then(|field| field.value.as_deref()),
        Some("新增字段")
    );
    let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
    for name in [
        "email",
        "subscribe",
        "gender",
        "country",
        "department",
        "notes",
    ] {
        assert!(names.contains(&name), "existing field {name} survives");
    }
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&output).expect("parse merged output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    assert!(fonts.get(b"PdfxNoto").is_ok(), "the new font is merged");
    assert!(
        fonts.iter().count() > 1,
        "existing /DR fonts are preserved: {:?}",
        fonts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        source_fields + 1,
        "the existing /Fields entries are preserved and one is added"
    );
    assert!(field_named(&parsed, "full_name").get(b"V").is_ok());

    close_document(document_id).expect("close document");
}

#[test]
fn permission_restricted_documents_refuse_creation_with_a_chinese_reason() {
    use pdfx_lib::text_edit::edit_history;

    let path = fixture_path("encrypted-owner-restrictions");
    let opened = open_document(path.to_string_lossy().into_owned(), None)
        .expect("open the restricted document");
    let error = create_form_text_field(
        opened.document_id.clone(),
        0,
        create_request(FormFieldRect {
            x: 72.0,
            y: 100.0,
            width: 180.0,
            height: 24.0,
        }),
    )
    .expect_err("permission restrictions reject field creation");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(
        error.message.contains("权限"),
        "the refusal explains the permission restriction in Chinese: {}",
        error.message
    );
    assert!(form_fields(opened.document_id.clone())
        .expect("fields")
        .is_empty());
    let history = edit_history(opened.document_id.clone()).expect("history");
    assert_eq!(history.revision, 0);
    assert!(!history.dirty && !history.can_undo);
    close_document(opened.document_id).expect("close restricted document");
}

#[test]
fn scanned_pages_accept_created_fields() {
    use pdfx_lib::commands::page_info;

    let dir = support::temp::TempDir::new("create-text-scanned");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("scanned-image-only"), &source).expect("copy scanned fixture");
    let output = dir.path().join("output.pdf");

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the scanned document")
        .document_id;
    let info = page_info(document_id.clone(), 0).expect("page info");
    let rect = FormFieldRect {
        x: 72.0,
        y: 72.0,
        width: 180.0,
        height: 24.0,
    };
    assert!(
        rect.x + rect.width <= info.width && rect.y + rect.height <= info.height,
        "the test rectangle fits the scanned page"
    );
    let mut request = create_request(rect);
    request.name = Some("scan_note".to_owned());
    request.required = false;
    let created = create_form_text_field(document_id.clone(), 0, request)
        .expect("a page without a text layer still accepts a form field");
    fill_form_field(
        document_id.clone(),
        created.field_id,
        FormFieldValueInput::Text {
            text: "扫描页字段".to_owned(),
        },
    )
    .expect("fill the scanned-page field");
    save_as(&document_id, &output);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "scan_note");
    assert_eq!(fields[0].value.as_deref(), Some("扫描页字段"));
    assert_eq!(fields[0].kind, FormFieldKind::Text);
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn saves_an_empty_created_field_with_a_static_appearance() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-text-empty");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("empty_note".to_owned());
    request.required = false;
    request.tooltip = None;
    create_form_text_field(document_id.clone(), 0, request).expect("create an empty field");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved = field_named(&parsed, "empty_note");
    assert!(saved.get(b"V").is_err(), "an empty field has no /V");
    let appearance = lopdf_dict(&parsed, saved.get(b"AP").expect("/AP"));
    assert!(
        appearance.get(b"N").is_ok(),
        "the empty field still has /AP /N"
    );
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    assert!(
        fonts.has(b"PdfxNoto"),
        "empty values still embed a usable font subset"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "empty_note");
    assert_eq!(fields[0].value, None);
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}
