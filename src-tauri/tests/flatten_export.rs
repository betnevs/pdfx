//! Command-boundary tests for flattened-copy export (ticket 35).
//!
//! The command is exercised exactly as the Tauri IPC layer invokes it, and
//! saved output is inspected with an independent `lopdf` parse and a fresh
//! PDFium open. Each test proves one part of the ticket's contract:
//!
//! * every visual PDFX annotation kind becomes page content,
//! * links, form fields and signature widgets stay native and interactive,
//! * annotations with no usable appearance follow an explicit, reported policy,
//! * the Source Document, the Output Document and the live document (including
//!   its undo history) are never modified,
//! * rotated pages and untouched pages keep their rendering,
//! * the destination is written atomically and repeatable, and
//! * encrypted sources follow the ADR-0007 confirmation rule.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use lopdf::dictionary;
use lopdf::Object;

use pdfx_lib::annotations::{
    create_free_text_with_author, create_ink_with_author, create_note_with_author,
    create_shape_with_author, create_stamp_with_author, create_text_markup_with_author,
    list_annotations, CreateFreeTextRequest, CreateInkRequest, CreateNoteRequest,
    CreateShapeRequest, CreateStampRequest, CreateTextMarkupRequest, FreeTextAlign, FreeTextKind,
    InkPoint, ShapeGeometry, ShapeKind, ShapeStyle, TextMarkupKind, TextMarkupRect,
};
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, render_page, CommandErrorCode, DocumentView,
    TextLayerView,
};
use pdfx_lib::flatten_export::{export_flattened_copy, FlattenSkipReason};
use pdfx_lib::form::{
    fill_form_field, form_fields, FormFieldKind, FormFieldRect, FormFieldValueInput,
};
use pdfx_lib::form_create::{create_form_field, CreateFormFieldRequest};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::stamps::{StampFrameView, StampKind};
use pdfx_lib::text_edit::{edit_history, undo_text_edit};
use support::fixtures;
use support::synthetic;
use support::temp::TempDir;

// ---------------------------------------------------------------------------
// Fixtures and command helpers
// ---------------------------------------------------------------------------

fn fixture_path(id: &str) -> PathBuf {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id:?} is missing from the manifest"));
    fixtures::fixture_path(fixture)
}

fn open_path(path: &Path, password: Option<&str>) -> DocumentView {
    open_document(
        path.to_string_lossy().into_owned(),
        password.map(str::to_owned),
    )
    .unwrap_or_else(|error| panic!("open {}: {error:?}", path.display()))
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
    .expect("save the Output Document");
}

fn export(document_id: &str, path: &Path) -> pdfx_lib::flatten_export::FlattenExportView {
    export_flattened_copy(
        document_id.to_owned(),
        path.to_string_lossy().into_owned(),
        false,
    )
    .expect("export the flattened copy")
}

fn author() -> String {
    "flatten test".to_owned()
}

// ---------------------------------------------------------------------------
// Annotation creation helpers, all through the command surface
// ---------------------------------------------------------------------------

fn selection_rect(layer: &TextLayerView, needle: &str) -> TextMarkupRect {
    let byte_offset = layer.text.find(needle).expect("needle in text layer");
    let start = layer.text[..byte_offset].chars().count();
    let count = needle.chars().count();
    let chars = &layer.chars[start..start + count];
    let left = chars
        .iter()
        .map(|character| character.x)
        .fold(f32::INFINITY, f32::min);
    let top = chars
        .iter()
        .map(|character| character.y)
        .fold(f32::INFINITY, f32::min);
    let right = chars
        .iter()
        .map(|character| character.x + character.width)
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = chars
        .iter()
        .map(|character| character.y + character.height)
        .fold(f32::NEG_INFINITY, f32::max);
    TextMarkupRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

fn add_markup(document_id: &str, kind: TextMarkupKind, rect: TextMarkupRect) {
    create_text_markup_with_author(
        document_id.to_owned(),
        author(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind,
            color: "#ffd400".to_owned(),
            opacity: 70,
            rects: vec![rect],
        },
    )
    .expect("create the text markup");
}

fn add_note(document_id: &str, rect: TextMarkupRect) {
    create_note_with_author(
        document_id.to_owned(),
        author(),
        CreateNoteRequest {
            page_index: 0,
            color: "#f9a825".to_owned(),
            opacity: 100,
            text: "flatten me".to_owned(),
            rect,
        },
    )
    .expect("create the note");
}

fn add_ink(document_id: &str, page_index: u32) {
    create_ink_with_author(
        document_id.to_owned(),
        author(),
        CreateInkRequest {
            page_index,
            color: "#c62828".to_owned(),
            opacity: 100,
            line_width: 4.0,
            strokes: vec![vec![
                InkPoint { x: 120.0, y: 120.0 },
                InkPoint { x: 200.0, y: 170.0 },
            ]],
        },
    )
    .expect("create the ink annotation");
}

fn add_shape(document_id: &str, kind: ShapeKind, x: f32, y: f32) {
    create_shape_with_author(
        document_id.to_owned(),
        author(),
        CreateShapeRequest {
            page_index: 0,
            kind,
            geometry: ShapeGeometry {
                x,
                y,
                width: 120.0,
                height: 80.0,
                rotation: 0.0,
            },
            style: ShapeStyle {
                fill_color: Some("#1e88e5".to_owned()),
                stroke_color: Some("#0d47a1".to_owned()),
                opacity: 90,
                line_width: 2.0,
            },
        },
    )
    .expect("create the shape");
}

fn add_stamp(document_id: &str) {
    create_stamp_with_author(
        document_id.to_owned(),
        author(),
        CreateStampRequest {
            page_index: 0,
            kind: StampKind::Approved,
            opacity: 100,
            frame: StampFrameView {
                x: 300.0,
                y: 480.0,
                width: 180.0,
                height: 60.0,
                rotation: 0.0,
            },
        },
    )
    .expect("create the stamp");
}

fn add_free_text(document_id: &str, kind: FreeTextKind, y: f32) {
    create_free_text_with_author(
        document_id.to_owned(),
        author(),
        CreateFreeTextRequest {
            page_index: 0,
            kind,
            rect: TextMarkupRect {
                x: 60.0,
                y,
                width: 220.0,
                height: 70.0,
            },
            color: "#2e7d32".to_owned(),
            opacity: 100,
            text: "Flattened free text".to_owned(),
            font_size: 12.0,
            align: FreeTextAlign::Left,
        },
    )
    .expect("create the free text annotation");
}

// ---------------------------------------------------------------------------
// Independent parsing and rendering helpers
// ---------------------------------------------------------------------------

fn page_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    let Some(page_id) = document.get_pages().get(&page_number).copied() else {
        return Vec::new();
    };
    let Ok(page) = document.get_dictionary(page_id) else {
        return Vec::new();
    };
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    let array = match annots {
        Object::Reference(id) => document
            .get_object(*id)
            .expect("/Annots reference")
            .as_array()
            .expect("/Annots array"),
        Object::Array(array) => array,
        other => panic!("/Annots is not an array: {other:?}"),
    };
    array
        .iter()
        .map(|object| match object {
            Object::Reference(id) => document
                .get_dictionary(*id)
                .expect("annotation dictionary")
                .clone(),
            Object::Dictionary(dictionary) => dictionary.clone(),
            other => panic!("annotation is not a dictionary: {other:?}"),
        })
        .collect()
}

fn subtype(annotation: &lopdf::Dictionary) -> String {
    String::from_utf8_lossy(
        annotation
            .get(b"Subtype")
            .and_then(Object::as_name)
            .expect("annotation /Subtype"),
    )
    .into_owned()
}

struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn render(document_id: &str, page_index: u32, scale: f32) -> Pixels {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render the page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload is base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("PNG buffer size")];
    let info = reader.next_frame(&mut buffer).expect("PNG frame");
    buffer.truncate(info.buffer_size());
    Pixels {
        width: info.width,
        height: info.height,
        rgba: buffer,
    }
}

/// Mean absolute channel difference between two renders of the same page.
fn mean_absolute_error(left: &Pixels, right: &Pixels) -> f64 {
    assert_eq!(
        (left.width, left.height, left.rgba.len()),
        (right.width, right.height, right.rgba.len()),
        "both renders have the same dimensions and channels"
    );
    let total: u64 = left
        .rgba
        .iter()
        .zip(&right.rgba)
        .map(|(left, right)| left.abs_diff(*right) as u64)
        .sum();
    total as f64 / left.rgba.len() as f64
}

fn page_content(document: &lopdf::Document, page_number: u32) -> Vec<u8> {
    let page_id = *document.get_pages().get(&page_number).expect("page number");
    document.get_page_content(page_id)
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

// ---------------------------------------------------------------------------
// 1. Every visual annotation kind becomes page content
// ---------------------------------------------------------------------------

#[test]
fn every_pdfx_annotation_kind_becomes_page_content_and_the_copy_reopens_in_pdfium() {
    let path = fixture_path("text-latin");
    let view = open_path(&path, None);
    let id = view.document_id.clone();
    let source_before = fs::read(&path).expect("read the Source Document");

    let layer = extract_text_layer(id.clone(), 0).expect("text layer");
    let selection = selection_rect(&layer, "PDFX");
    add_markup(&id, TextMarkupKind::Highlight, selection);
    add_markup(&id, TextMarkupKind::Underline, selection);
    add_markup(&id, TextMarkupKind::Strikeout, selection);
    add_markup(&id, TextMarkupKind::Squiggly, selection);
    add_note(
        &id,
        TextMarkupRect {
            x: 80.0,
            y: 80.0,
            width: 28.0,
            height: 28.0,
        },
    );
    add_ink(&id, 0);
    add_shape(&id, ShapeKind::Square, 60.0, 200.0);
    add_shape(&id, ShapeKind::Circle, 220.0, 200.0);
    add_stamp(&id);
    add_free_text(&id, FreeTextKind::FreeText, 340.0);
    add_free_text(&id, FreeTextKind::Callout, 430.0);
    assert_eq!(
        list_annotations(id.clone())
            .expect("list annotations")
            .len(),
        11,
        "the fixture carries every visual annotation kind"
    );

    let before = render(&id, 0, 1.5);
    let temp = TempDir::new("flatten-all-kinds");
    let destination = temp.path().join("flattened.pdf");
    let exported = export(&id, &destination);

    assert_eq!(
        exported.flattened, 11,
        "every visual annotation is flattened"
    );
    assert_eq!(exported.preserved, 0, "no links or widgets on this page");
    assert!(
        exported.skipped.is_empty(),
        "nothing was skipped: {exported:?}"
    );
    assert_eq!(
        exported.bytes,
        fs::metadata(&destination).expect("output metadata").len()
    );
    assert_eq!(exported.path, destination.to_string_lossy());

    let parsed = lopdf::Document::load(&destination).expect("lopdf parses the flattened copy");
    let page_id = *parsed.get_pages().get(&1).expect("first page");
    let page = parsed.get_dictionary(page_id).expect("page dictionary");
    assert!(
        !page.has(b"Annots"),
        "no visual annotation is left in the copied page's /Annots"
    );
    let content = parsed.get_page_content(page_id);
    assert_eq!(
        count_occurrences(&content, b"PDFXFlat"),
        11,
        "the page content paints one form XObject per annotation"
    );
    assert!(
        content.windows(2).any(|window| window == b"Do"),
        "the content uses the Do operator to paint appearances"
    );

    // A fresh PDFium open proves the copy is a valid, readable PDF and that
    // the flattened page renders like the live annotated page.
    let reopened = open_path(&destination, None);
    assert_eq!(
        list_annotations(reopened.document_id.clone())
            .expect("list annotations on the copy")
            .len(),
        0,
        "the copy reports no managed annotations"
    );
    let after = render(&reopened.document_id, 0, 1.5);
    let error = mean_absolute_error(&before, &after);
    assert!(
        error < 2.0,
        "flattened page renders like the annotated live page (MAE {error})"
    );
    close_document(reopened.document_id).expect("close the copy");

    assert_eq!(
        fs::read(&path).expect("re-read the Source Document"),
        source_before,
        "exporting a flattened copy never modifies the Source Document"
    );
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 2. Links and form widgets stay native; filling still works
// ---------------------------------------------------------------------------

/// The form fixture with a URI link annotation added on page 1. The link is
/// authored with lopdf, not PDFX, so it represents a foreign interactive
/// annotation the flatten pass must leave alone.
fn form_fixture_with_uri_link() -> synthetic::TempPdf {
    let mut document =
        lopdf::Document::load(fixture_path("form-acroform")).expect("load the form fixture");
    let page_id = *document.get_pages().get(&1).expect("page 1");
    let link_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => Object::Array(vec![
            Object::Real(440.0),
            Object::Real(700.0),
            Object::Real(560.0),
            Object::Real(730.0),
        ]),
        "Border" => Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
        "A" => lopdf::dictionary! {
            "S" => "URI",
            "URI" => Object::string_literal("https://example.com/pdfx"),
        },
    });

    let annots_is_direct = document
        .get_dictionary(page_id)
        .and_then(|page| page.get(b"Annots"))
        .map(|annots| matches!(annots, Object::Array(_)))
        .unwrap_or(false);
    if annots_is_direct {
        document
            .get_dictionary_mut(page_id)
            .expect("page dictionary")
            .get_mut(b"Annots")
            .expect("/Annots")
            .as_array_mut()
            .expect("/Annots array")
            .push(Object::Reference(link_id));
    } else {
        let annots_id = document
            .get_dictionary(page_id)
            .expect("page dictionary")
            .get(b"Annots")
            .expect("/Annots")
            .as_reference()
            .expect("/Annots reference");
        document
            .get_object_mut(annots_id)
            .expect("/Annots object")
            .as_array_mut()
            .expect("/Annots array")
            .push(Object::Reference(link_id));
    }

    let mut bytes = Vec::new();
    document.save_to(&mut bytes).expect("serialize the fixture");
    synthetic::pdf_bytes_file("form-with-link", bytes)
}

#[test]
fn links_and_form_widgets_stay_interactive_and_form_fill_still_works() {
    let pdf = form_fixture_with_uri_link();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    let fields = form_fields(id.clone()).expect("discover form fields");
    let widget_count = fields.len();
    assert_eq!(widget_count, 12, "the fixture has twelve widgets");

    let text_field = fields
        .iter()
        .find(|field| field.kind == FormFieldKind::Text && field.page_index == 0)
        .expect("a text field on page 1");
    fill_form_field(
        id.clone(),
        text_field.field_id.clone(),
        FormFieldValueInput::Text {
            text: "Ada Lovelace".to_owned(),
        },
    )
    .expect("fill a form field before flattening");

    let temp = TempDir::new("flatten-form");
    let destination = temp.path().join("form-flattened.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 0, "widgets are not visual annotations");
    assert_eq!(
        exported.preserved,
        widget_count as u32 + 1,
        "every widget and the URI link are preserved"
    );
    assert!(exported.skipped.is_empty());

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let mut widget_total = 0;
    for page_number in 1..=parsed.get_pages().len() as u32 {
        widget_total += page_annotations(&parsed, page_number)
            .iter()
            .filter(|annotation| subtype(annotation) == "Widget")
            .count();
    }
    assert_eq!(
        widget_total, widget_count,
        "all widgets remain across pages"
    );
    let annotations = page_annotations(&parsed, 1);
    let link = annotations
        .iter()
        .find(|annotation| subtype(annotation) == "Link")
        .expect("the URI link remains");
    let action = link.get(b"A").expect("/A").as_dict().expect("/A dict");
    assert_eq!(action.get(b"S").expect("/S").as_name().expect("/S"), b"URI");
    assert_eq!(
        action.get(b"URI").expect("/URI").as_str().expect("/URI"),
        b"https://example.com/pdfx"
    );

    // AcroForm machinery is untouched: fields, default appearance and the
    // resource dictionary survive; no /NeedAppearances is introduced.
    let catalog_id = parsed
        .trailer
        .get(b"Root")
        .expect("/Root")
        .as_reference()
        .expect("/Root reference");
    let catalog = parsed.get_dictionary(catalog_id).expect("catalog");
    let acro_form = catalog
        .get(b"AcroForm")
        .expect("/AcroForm")
        .as_dict()
        .expect("/AcroForm dict");
    assert!(acro_form.has(b"Fields"), "/Fields is preserved");
    assert!(acro_form.has(b"DR"), "/DR is preserved");
    assert!(acro_form.has(b"DA"), "/DA is preserved");
    assert_eq!(
        acro_form
            .get(b"NeedAppearances")
            .ok()
            .and_then(|flag| flag.as_bool().ok()),
        Some(true),
        "the fixture's /NeedAppearances flag is preserved, neither dropped nor invented"
    );

    // Reopen the copy through PDFium: the filled value is visible, and the
    // surviving widgets accept a new value.
    let reopened = open_path(&destination, None);
    let reopened_fields = form_fields(reopened.document_id.clone()).expect("fields in the copy");
    assert_eq!(reopened_fields.len(), widget_count);
    let name_field = reopened_fields
        .iter()
        .find(|field| field.name == text_field.name)
        .expect("the filled field survives");
    assert_eq!(name_field.value.as_deref(), Some("Ada Lovelace"));
    let email = reopened_fields
        .iter()
        .find(|field| field.name == "email")
        .expect("the email field survives");
    fill_form_field(
        reopened.document_id.clone(),
        email.field_id.clone(),
        FormFieldValueInput::Text {
            text: "ada@example.com".to_owned(),
        },
    )
    .expect("fill a field in the flattened copy");
    let after_fill = form_fields(reopened.document_id.clone()).expect("fields after filling");
    assert_eq!(
        after_fill
            .iter()
            .find(|field| field.name == "email")
            .and_then(|field| field.value.as_deref()),
        Some("ada@example.com"),
        "a flattened copy keeps fillable form fields"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 3. Signature widgets are preserved without a value
// ---------------------------------------------------------------------------

#[test]
fn signature_widgets_are_preserved_and_never_given_a_value() {
    let pdf = synthetic::single_line_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    create_form_field(
        id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                x: 72.0,
                y: 500.0,
                width: 240.0,
                height: 80.0,
            },
            name: Some("sign_here".to_owned()),
            tooltip: Some("请在此签名".to_owned()),
        },
    )
    .expect("create the signature placeholder");

    let temp = TempDir::new("flatten-signature");
    let destination = temp.path().join("signature.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 0);
    assert_eq!(exported.preserved, 1, "the signature widget is preserved");

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let annotations = page_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 1);
    let widget = &annotations[0];
    assert_eq!(subtype(widget), "Widget");
    assert_eq!(
        widget.get(b"FT").expect("/FT").as_name().expect("/FT"),
        b"Sig"
    );
    assert_eq!(
        widget.get(b"T").expect("/T").as_str().expect("/T"),
        b"sign_here"
    );
    assert!(
        !widget.has(b"V"),
        "a signature placeholder carries no value in the flattened copy"
    );
    let catalog_id = parsed
        .trailer
        .get(b"Root")
        .expect("/Root")
        .as_reference()
        .expect("/Root reference");
    let acro_form = parsed
        .get_dictionary(catalog_id)
        .expect("catalog")
        .get(b"AcroForm")
        .expect("/AcroForm")
        .as_dict()
        .expect("/AcroForm dict");
    assert!(
        !acro_form.has(b"NeedAppearances"),
        "flattening a PDFX form must not request appearance regeneration"
    );

    let reopened = open_path(&destination, None);
    let fields = form_fields(reopened.document_id.clone()).expect("fields in the copy");
    let signature = fields
        .iter()
        .find(|field| field.name == "sign_here")
        .expect("the signature widget is rediscovered");
    assert_eq!(signature.kind, FormFieldKind::Signature);
    assert_eq!(signature.value, None);
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 4. Explicit policy for annotations with no usable appearance
// ---------------------------------------------------------------------------

/// One page carrying a foreign note without appearance, its popup, squares
/// with no / unusable / hidden appearances, and a URI link.
fn appearance_policy_pdf() -> synthetic::TempPdf {
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();

    let font_id = document.add_object(lopdf::dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let content_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"BT /F1 12 Tf 72 720 Td (Appearance policy) Tj ET".to_vec(),
    ));

    let valid_appearance_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => Object::Array(vec![
                Object::Real(100.0),
                Object::Real(400.0),
                Object::Real(200.0),
                Object::Real(450.0),
            ]),
        },
        b"q 1 0 0 rg 100 400 100 50 re f Q".to_vec(),
    ));
    let image_appearance_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => Object::Integer(1),
            "Height" => Object::Integer(1),
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => Object::Integer(8),
        },
        vec![0, 0, 0],
    ));
    let invalid_bbox_appearance_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => Object::Array(vec![Object::Real(0.0), Object::Real(0.0)]),
        },
        b"q 0 0 1 rg 0 0 10 10 re f Q".to_vec(),
    ));

    let rect = |x: f32| {
        Object::Array(vec![
            Object::Real(x),
            Object::Real(0.0),
            Object::Real(x + 100.0),
            Object::Real(50.0),
        ])
    };
    let text_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => Object::Array(vec![
            Object::Real(72.0),
            Object::Real(650.0),
            Object::Real(96.0),
            Object::Real(674.0),
        ]),
        "Contents" => Object::string_literal("foreign note without appearance"),
    });
    let popup_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Popup",
        "Rect" => Object::Array(vec![
            Object::Real(100.0),
            Object::Real(650.0),
            Object::Real(300.0),
            Object::Real(700.0),
        ]),
        "Parent" => Object::Reference(text_id),
    });
    let square_without_appearance_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Square",
        "Rect" => rect(100.0),
    });
    let hidden_square_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Square",
        "Rect" => rect(250.0),
        "F" => Object::Integer(2),
        "AP" => lopdf::dictionary! { "N" => Object::Reference(valid_appearance_id) },
    });
    let image_square_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Square",
        "Rect" => rect(400.0),
        "AP" => lopdf::dictionary! { "N" => Object::Reference(image_appearance_id) },
    });
    let invalid_bbox_square_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Square",
        "Rect" => rect(550.0),
        "AP" => lopdf::dictionary! { "N" => Object::Reference(invalid_bbox_appearance_id) },
    });
    let link_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => Object::Array(vec![
            Object::Real(72.0),
            Object::Real(600.0),
            Object::Real(272.0),
            Object::Real(620.0),
        ]),
        "A" => lopdf::dictionary! {
            "S" => "URI",
            "URI" => Object::string_literal("https://example.com/policy"),
        },
    });

    let page_id = document.add_object(lopdf::dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "MediaBox" => Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
        "Resources" => lopdf::dictionary! {
            "Font" => lopdf::dictionary! { "F1" => Object::Reference(font_id) },
        },
        "Contents" => Object::Reference(content_id),
        "Annots" => Object::Array(vec![
            Object::Reference(text_id),
            Object::Reference(popup_id),
            Object::Reference(square_without_appearance_id),
            Object::Reference(hidden_square_id),
            Object::Reference(image_square_id),
            Object::Reference(invalid_bbox_square_id),
            Object::Reference(link_id),
        ]),
    });
    document.set_object(
        pages_id,
        lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(vec![Object::Reference(page_id)]),
            "Count" => Object::Integer(1),
        },
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    document.trailer.set("Root", Object::Reference(catalog_id));

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize the policy PDF");
    synthetic::pdf_bytes_file("appearance-policy", bytes)
}

#[test]
fn annotations_without_usable_appearances_are_omitted_with_explicit_reasons() {
    let pdf = appearance_policy_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    let before = render(&id, 0, 1.0);

    let temp = TempDir::new("flatten-policy");
    let destination = temp.path().join("policy-flattened.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 0, "nothing has a drawable appearance");
    assert_eq!(exported.preserved, 1, "the URI link is preserved");

    let reasons: Vec<(String, FlattenSkipReason)> = exported
        .skipped
        .iter()
        .map(|skip| (skip.subtype.clone(), skip.reason))
        .collect();
    assert_eq!(
        reasons,
        vec![
            ("Text".to_owned(), FlattenSkipReason::NoAppearance),
            ("Popup".to_owned(), FlattenSkipReason::Popup),
            ("Square".to_owned(), FlattenSkipReason::NoAppearance),
            ("Square".to_owned(), FlattenSkipReason::Hidden),
            ("Square".to_owned(), FlattenSkipReason::NoAppearance),
            (
                "Square".to_owned(),
                FlattenSkipReason::UnsupportedAppearance
            ),
        ],
        "every omitted annotation is reported with its reason, in page order"
    );
    assert!(
        exported.skipped.iter().all(|skip| skip.page_index == 0),
        "skip reports carry the page index"
    );

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let annotations = page_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 1, "only the link remains");
    assert_eq!(subtype(&annotations[0]), "Link");
    assert_eq!(
        count_occurrences(
            &parsed.get_page_content(*parsed.get_pages().get(&1).expect("page")),
            b"PDFXFlat"
        ),
        0,
        "no appearance was drawn into the page"
    );

    let reopened = open_path(&destination, None);
    let after = render(&reopened.document_id, 0, 1.0);
    assert!(
        mean_absolute_error(&before, &after) < 0.5,
        "omitting invisible annotations does not change the rendered page"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 5. Rotated pages
// ---------------------------------------------------------------------------

#[test]
fn rotated_pages_keep_their_rendering_after_flattening() {
    let pdf = synthetic::rotated_crop_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    for page_index in 0..3 {
        add_ink(&id, page_index);
    }
    let before: Vec<Pixels> = (0..3).map(|page| render(&id, page, 1.5)).collect();

    let temp = TempDir::new("flatten-rotation");
    let destination = temp.path().join("rotated-flattened.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 3);

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    for page_number in 1..=3 {
        assert!(
            page_annotations(&parsed, page_number)
                .iter()
                .all(|annotation| subtype(annotation) != "Ink"),
            "page {page_number} keeps no ink annotation"
        );
        assert_eq!(
            count_occurrences(
                &parsed.get_page_content(*parsed.get_pages().get(&page_number).expect("page")),
                b"PDFXFlat"
            ),
            1,
            "page {page_number} paints its flattened ink"
        );
    }

    let reopened = open_path(&destination, None);
    for page_index in 0..3_usize {
        let after = render(&reopened.document_id, page_index as u32, 1.5);
        let error = mean_absolute_error(&before[page_index], &after);
        assert!(
            error < 2.0,
            "rotated page {page_index} renders like the annotated page (MAE {error})"
        );
    }
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 6. Pages without visual annotations stay byte-identical
// ---------------------------------------------------------------------------

#[test]
fn pages_without_visual_annotations_keep_their_content_streams() {
    let pdf = synthetic::rotated_crop_pdf();
    let source_bytes = fs::read(pdf.path_string()).expect("read the synthetic source");
    let source_document = lopdf::Document::load_mem(&source_bytes).expect("parse the source");
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    add_ink(&id, 0);

    let before: Vec<Pixels> = (1..3).map(|page| render(&id, page, 1.2)).collect();

    let temp = TempDir::new("flatten-untouched-pages");
    let destination = temp.path().join("untouched-pages.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 1, "only page 1 was annotated");

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    for page_number in 2..=3 {
        assert_eq!(
            page_content(&parsed, page_number),
            page_content(&source_document, page_number),
            "page {page_number} content is byte-identical to the Source Document"
        );
    }

    let reopened = open_path(&destination, None);
    for (offset, page_index) in (1..3).enumerate() {
        let after = render(&reopened.document_id, page_index, 1.2);
        assert!(
            mean_absolute_error(&before[offset], &after) < 0.5,
            "untouched page {page_index} renders identically"
        );
    }
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 7. Source, Output Document and live document protection
// ---------------------------------------------------------------------------

#[test]
fn flattening_never_touches_the_source_output_or_live_document() {
    let path = fixture_path("text-latin");
    let view = open_path(&path, None);
    let id = view.document_id.clone();
    let layer = extract_text_layer(id.clone(), 0).expect("text layer");
    add_markup(
        &id,
        TextMarkupKind::Highlight,
        selection_rect(&layer, "PDFX"),
    );

    let source_bytes = fs::read(&path).expect("read the Source Document");
    let source_modified = fs::metadata(&path)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let temp = TempDir::new("flatten-protection");
    let output = temp.path().join("output.pdf");
    save_as(&id, &output);
    let output_bytes = fs::read(&output).expect("read the Output Document");

    let history_before = edit_history(id.clone()).expect("edit history");
    let annotations_before = list_annotations(id.clone()).expect("annotations");
    let live_before = render(&id, 0, 1.0);

    // The Source Document is refused byte-for-byte.
    let error = export_flattened_copy(id.clone(), path.to_string_lossy().into_owned(), false)
        .expect_err("the Source Document cannot be the flattened destination");
    assert_eq!(error.code, CommandErrorCode::ExportTargetIsSource);
    assert_eq!(
        fs::read(&path).expect("re-read the Source Document"),
        source_bytes
    );
    assert_eq!(
        fs::metadata(&path)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_modified,
        "a refused export must not touch the Source Document's mtime"
    );

    // The current Output Document is refused too.
    let error = export_flattened_copy(id.clone(), output.to_string_lossy().into_owned(), false)
        .expect_err("the Output Document cannot be the flattened destination");
    assert_eq!(error.code, CommandErrorCode::ExportTargetIsOutput);
    assert_eq!(
        fs::read(&output).expect("re-read the Output Document"),
        output_bytes
    );

    // Aliases of the Source Document through a symlink are refused by
    // identity, not by path spelling.
    #[cfg(unix)]
    {
        let symlink = temp.path().join("sym-link.pdf");
        std::os::unix::fs::symlink(&path, &symlink)
            .expect("create a symlink to the Source Document");
        let error =
            export_flattened_copy(id.clone(), symlink.to_string_lossy().into_owned(), false)
                .expect_err("a symlink to the Source Document is refused");
        assert_eq!(error.code, CommandErrorCode::ExportTargetIsSource);
    }

    // A legitimate export succeeds and changes none of the protected state.
    let destination = temp.path().join("copy.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 1);
    assert_eq!(
        fs::read(&path).expect("re-read the Source Document"),
        source_bytes
    );
    assert_eq!(
        fs::read(&output).expect("re-read the Output Document"),
        output_bytes,
        "exporting does not change the Output Document"
    );
    assert_eq!(
        edit_history(id.clone()).expect("edit history after export"),
        history_before,
        "exporting does not move the saved baseline or the undo history"
    );
    assert_eq!(
        list_annotations(id.clone()).expect("annotations after export"),
        annotations_before,
        "the live document still carries its annotations"
    );
    let live_after = render(&id, 0, 1.0);
    assert_eq!(
        mean_absolute_error(&live_before, &live_after),
        0.0,
        "the live document renders unchanged after export"
    );

    // The undo history is still usable, which proves the export never
    // reopened or replaced the live PDFium document.
    undo_text_edit(id.clone()).expect("undo the annotation creation after export");
    assert!(
        list_annotations(id.clone())
            .expect("annotations after undo")
            .is_empty(),
        "undo still works on the live document after exporting"
    );
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 8. Atomic, repeatable destination writes
// ---------------------------------------------------------------------------

#[test]
fn repeat_exports_overwrite_atomically_and_failures_leave_no_file() {
    let path = fixture_path("text-latin");
    let view = open_path(&path, None);
    let id = view.document_id.clone();
    let layer = extract_text_layer(id.clone(), 0).expect("text layer");
    add_markup(
        &id,
        TextMarkupKind::Underline,
        selection_rect(&layer, "PDFX"),
    );

    let temp = TempDir::new("flatten-repeat");
    let destination = temp.path().join("repeat.pdf");
    fs::write(&destination, b"stale bytes").expect("write a previous file");

    let first = export(&id, &destination);
    let first_bytes = fs::read(&destination).expect("read the first export");
    assert_eq!(first.flattened, 1);
    assert!(
        first_bytes.starts_with(b"%PDF-"),
        "the target becomes a PDF"
    );

    let second = export(&id, &destination);
    assert_eq!(second.flattened, 1);
    let second_bytes = fs::read(&destination).expect("read the second export");
    assert!(
        second_bytes.starts_with(b"%PDF-"),
        "the second export replaces the first with another valid PDF"
    );
    let parsed_second =
        lopdf::Document::load_mem(&second_bytes).expect("the overwritten file parses as a PDF");
    let second_page = *parsed_second.get_pages().get(&1).expect("first page");
    assert!(
        !parsed_second
            .get_dictionary(second_page)
            .expect("page dictionary")
            .has(b"Annots"),
        "the overwritten file is the same flattened result"
    );
    assert_eq!(
        count_occurrences(&parsed_second.get_page_content(second_page), b"PDFXFlat"),
        1,
        "the overwritten file paints the annotation once"
    );
    assert!(
        !fs::read_dir(temp.path())
            .expect("list the export directory")
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.contains("pdfx-backup") || name.contains(".pdfx-export-")
            }),
        "image-export semantics apply: atomic overwrite without backup or temporary leftovers"
    );

    // A missing parent directory fails cleanly with no partial file.
    let missing = temp.path().join("missing").join("copy.pdf");
    let error = export_flattened_copy(id.clone(), missing.to_string_lossy().into_owned(), false)
        .expect_err("a missing parent directory fails the write");
    assert_eq!(error.code, CommandErrorCode::FileWriteFailed);
    assert!(!missing.exists(), "no partial file is created");

    // Renaming over an existing directory fails and removes the temp file.
    let directory_target = temp.path().join("a-directory.pdf");
    fs::create_dir(&directory_target).expect("create a target directory");
    let error = export_flattened_copy(
        id.clone(),
        directory_target.to_string_lossy().into_owned(),
        false,
    )
    .expect_err("a directory cannot be replaced by the export");
    assert_eq!(error.code, CommandErrorCode::FileWriteFailed);
    assert!(
        directory_target.is_dir(),
        "the directory target is untouched"
    );
    assert!(
        !fs::read_dir(temp.path())
            .expect("list the export directory")
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains(".pdfx-export-")),
        "the failed rename leaves no temporary file"
    );

    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 9. Encrypted Source Documents (ADR-0007)
// ---------------------------------------------------------------------------

#[test]
fn encrypted_sources_require_confirmation_and_write_an_unencrypted_copy() {
    let path = fixture_path("encrypted-user-password");
    let source_bytes = fs::read(&path).expect("read the encrypted Source Document");
    let view = open_path(&path, Some("pdfx-user"));
    assert!(view.encrypted);
    let id = view.document_id.clone();
    add_note(
        &id,
        TextMarkupRect {
            x: 100.0,
            y: 100.0,
            width: 28.0,
            height: 28.0,
        },
    );

    let temp = TempDir::new("flatten-encrypted");
    let destination = temp.path().join("plain-copy.pdf");

    let error = export_flattened_copy(
        id.clone(),
        destination.to_string_lossy().into_owned(),
        false,
    )
    .expect_err("an encrypted source needs the unencrypted-output confirmation");
    assert_eq!(
        error.code,
        CommandErrorCode::UnencryptedOutputConfirmationRequired
    );
    assert!(
        !destination.exists(),
        "no file is written before the user confirms"
    );

    let exported =
        export_flattened_copy(id.clone(), destination.to_string_lossy().into_owned(), true)
            .expect("a confirmed export writes an unencrypted flattened copy");
    assert_eq!(exported.flattened, 1);
    assert_eq!(
        fs::read(&path).expect("re-read the encrypted Source Document"),
        source_bytes,
        "the encrypted Source Document is never modified"
    );

    let reopened = open_path(&destination, None);
    assert!(
        !reopened.encrypted,
        "the flattened copy is written unencrypted, as confirmed"
    );
    assert_eq!(
        list_annotations(reopened.document_id.clone())
            .expect("annotations in the copy")
            .len(),
        0,
        "the note was flattened into page content"
    );
    let parsed = lopdf::Document::load(&destination).expect("parse the copy");
    assert!(
        count_occurrences(&page_content(&parsed, 1), b"PDFXFlat") >= 1,
        "the flattened note is painted on the page"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// 10. Bookmarks, metadata and existing links survive
// ---------------------------------------------------------------------------

#[test]
fn bookmarks_metadata_and_existing_links_survive_flattening() {
    let path = fixture_path("structure-metadata");
    let source_document = lopdf::Document::load(&path).expect("parse the structure fixture");
    let source_title = source_document
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|info| info.as_reference().ok())
        .and_then(|id| source_document.get_dictionary(id).ok())
        .and_then(|info| info.get(b"Title").ok())
        .and_then(|title| title.as_str().ok())
        .map(<[u8]>::to_vec);
    assert!(source_title.is_some(), "the fixture has document metadata");

    let view = open_path(&path, None);
    let id = view.document_id.clone();
    add_ink(&id, 0);
    let before = render(&id, 0, 1.0);

    let temp = TempDir::new("flatten-structure");
    let destination = temp.path().join("structure-flattened.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 1);
    assert!(
        exported.preserved >= 1,
        "the existing URI link is preserved"
    );

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let output_title = parsed
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|info| info.as_reference().ok())
        .and_then(|id| parsed.get_dictionary(id).ok())
        .and_then(|info| info.get(b"Title").ok())
        .and_then(|title| title.as_str().ok())
        .map(<[u8]>::to_vec);
    assert_eq!(output_title, source_title, "document metadata is preserved");

    let catalog_id = parsed
        .trailer
        .get(b"Root")
        .expect("/Root")
        .as_reference()
        .expect("/Root reference");
    let catalog = parsed.get_dictionary(catalog_id).expect("catalog");
    assert!(
        source_document
            .get_dictionary(
                source_document
                    .trailer
                    .get(b"Root")
                    .expect("/Root")
                    .as_reference()
                    .expect("/Root reference")
            )
            .expect("source catalog")
            .has(b"Outlines"),
        "the fixture has bookmarks"
    );
    assert!(
        catalog.has(b"Outlines"),
        "bookmarks survive the flattened copy"
    );

    let links: Vec<_> = page_annotations(&parsed, 1)
        .into_iter()
        .filter(|annotation| subtype(annotation) == "Link")
        .collect();
    assert!(!links.is_empty(), "the fixture's URI link survives");
    assert!(
        links.iter().any(|link| {
            link.get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"S").ok())
                .and_then(|subtype| subtype.as_name().ok())
                == Some(b"URI".as_slice())
        }),
        "the URI action is intact"
    );

    let reopened = open_path(&destination, None);
    let after = render(&reopened.document_id, 0, 1.0);
    assert!(
        mean_absolute_error(&before, &after) < 2.0,
        "the page still renders with its flattened ink"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

// ---------------------------------------------------------------------------
// Independent audit: appearance placement, content-stream shapes, inherited
// resources, save parity, mixed widget pages and popup removal.
// ---------------------------------------------------------------------------

/// Inclusive pixel bounds of every pixel matching `predicate`.
fn color_bounds(
    pixels: &Pixels,
    predicate: impl Fn(&[u8]) -> bool,
) -> Option<(u32, u32, u32, u32)> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..pixels.height {
        for x in 0..pixels.width {
            let offset = ((y * pixels.width + x) * 4) as usize;
            if predicate(&pixels.rgba[offset..offset + 4]) {
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((left, top, right, bottom)) => {
                        (left.min(x), top.min(y), right.max(x), bottom.max(y))
                    }
                });
            }
        }
    }
    bounds
}

/// Inclusive pixel bounds of a filled PDF rectangle in a 1x render whose
/// page height is `page_height` (PDF y grows up, bitmap rows grow down).
fn expected_pixel_bbox(
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
    page_height: u32,
) -> (i32, i32, i32, i32) {
    (
        left as i32,
        (page_height as f32 - top) as i32,
        right as i32 - 1,
        (page_height as f32 - bottom) as i32 - 1,
    )
}

fn assert_bbox_close(label: &str, actual: (u32, u32, u32, u32), expected: (i32, i32, i32, i32)) {
    let actual_values = [
        actual.0 as i32,
        actual.1 as i32,
        actual.2 as i32,
        actual.3 as i32,
    ];
    let expected_values = [expected.0, expected.1, expected.2, expected.3];
    for (index, (actual_value, expected_value)) in
        actual_values.into_iter().zip(expected_values).enumerate()
    {
        assert!(
            (actual_value - expected_value).abs() <= 3,
            "{label}: bounding-box component {index} is {actual_value}, expected \
             {expected_value} (actual {actual:?}, expected {expected:?})"
        );
    }
}

/// Adds a form XObject with an optional `/Matrix` and `/Resources`.
fn synthetic_form(
    document: &mut lopdf::Document,
    bbox: [f32; 4],
    matrix: Option<[f32; 6]>,
    resources: Option<lopdf::Dictionary>,
    content: &[u8],
) -> lopdf::ObjectId {
    let mut dictionary = lopdf::dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => Object::Array(bbox.iter().map(|value| Object::Real(*value)).collect()),
    };
    if let Some(matrix) = matrix {
        dictionary.set(
            "Matrix",
            Object::Array(matrix.iter().map(|value| Object::Real(*value)).collect()),
        );
    }
    if let Some(resources) = resources {
        dictionary.set("Resources", resources);
    }
    document.add_object(lopdf::Stream::new(dictionary, content.to_vec()))
}

fn synthetic_square_annotation(
    document: &mut lopdf::Document,
    rect: [f32; 4],
    appearance: lopdf::ObjectId,
) -> lopdf::ObjectId {
    document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Square",
        "Rect" => Object::Array(rect.iter().map(|value| Object::Real(*value)).collect()),
        "AP" => lopdf::dictionary! { "N" => Object::Reference(appearance) },
    })
}

fn synthetic_page(
    document: &mut lopdf::Document,
    pages_id: lopdf::ObjectId,
    contents: Option<Object>,
    annotation: lopdf::ObjectId,
) -> lopdf::ObjectId {
    let mut dictionary = lopdf::dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "MediaBox" => Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
        "Annots" => Object::Array(vec![Object::Reference(annotation)]),
    };
    if let Some(contents) = contents {
        dictionary.set("Contents", contents);
    }
    document.add_object(dictionary)
}

/// One page with four foreign square annotations:
/// a plain form, a form whose `/BBox` is half its `/Rect`, a form with a
/// rotating `/Matrix`, and a form that draws text using its own `/Resources`.
fn foreign_appearance_placement_pdf() -> synthetic::TempPdf {
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(lopdf::dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let content_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"BT /F1 12 Tf 72 720 Td (Base text) Tj ET".to_vec(),
    ));

    let plain = synthetic_form(
        &mut document,
        [0.0, 0.0, 100.0, 50.0],
        None,
        None,
        b"1 0 0 rg 0 0 100 50 re f",
    );
    let scaled = synthetic_form(
        &mut document,
        [0.0, 0.0, 50.0, 25.0],
        None,
        None,
        b"0 0 1 rg 0 0 50 25 re f",
    );
    let rotated = synthetic_form(
        &mut document,
        [0.0, 0.0, 100.0, 50.0],
        Some([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
        None,
        b"0 0.6 0 rg 0 0 100 50 re f",
    );
    let font_resources = lopdf::dictionary! {
        "Font" => lopdf::dictionary! { "F1" => Object::Reference(font_id) },
    };
    let text_form = synthetic_form(
        &mut document,
        [0.0, 0.0, 100.0, 50.0],
        None,
        Some(font_resources),
        b"BT /F1 12 Tf 5 20 Td (AP text) Tj ET",
    );

    let annotations = [
        synthetic_square_annotation(&mut document, [72.0, 100.0, 172.0, 150.0], plain),
        synthetic_square_annotation(&mut document, [300.0, 100.0, 400.0, 150.0], scaled),
        synthetic_square_annotation(&mut document, [72.0, 500.0, 172.0, 550.0], rotated),
        synthetic_square_annotation(&mut document, [300.0, 500.0, 400.0, 550.0], text_form),
    ];

    let page_id = synthetic_page(
        &mut document,
        pages_id,
        Some(Object::Reference(content_id)),
        annotations[0],
    );
    document
        .get_dictionary_mut(page_id)
        .expect("page dictionary")
        .set(
            "Annots",
            Object::Array(annotations.into_iter().map(Object::Reference).collect()),
        );
    document.set_object(
        pages_id,
        lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(vec![Object::Reference(page_id)]),
            "Count" => Object::Integer(1),
        },
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    document.trailer.set("Root", Object::Reference(catalog_id));

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize the placement PDF");
    synthetic::pdf_bytes_file("foreign-appearance-placement", bytes)
}

#[test]
fn foreign_appearance_bbox_and_matrix_placement_matches_pdfium_rendering() {
    let pdf = foreign_appearance_placement_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    let before = render(&id, 0, 1.0);

    let temp = TempDir::new("flatten-foreign-placement");
    let destination = temp.path().join("placement.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 4, "{exported:?}");
    assert_eq!(exported.preserved, 0, "{exported:?}");
    assert!(exported.skipped.is_empty(), "{exported:?}");

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let page_id = *parsed.get_pages().get(&1).expect("first page");
    assert!(
        !parsed
            .get_dictionary(page_id)
            .expect("page dictionary")
            .has(b"Annots"),
        "every visual annotation is removed from the copy"
    );
    assert_eq!(
        count_occurrences(&parsed.get_page_content(page_id), b"PDFXFlat"),
        4,
        "each appearance is painted once"
    );

    let reopened = open_path(&destination, None);
    let after = render(&reopened.document_id, 0, 1.0);
    let error = mean_absolute_error(&before, &after);

    let red: fn(&[u8]) -> bool = |pixel| pixel[0] > 180 && pixel[1] < 90 && pixel[2] < 90;
    let blue: fn(&[u8]) -> bool = |pixel| pixel[2] > 180 && pixel[0] < 90 && pixel[1] < 90;
    let green: fn(&[u8]) -> bool = |pixel| pixel[1] > 110 && pixel[0] < 90 && pixel[2] < 90;
    let height = before.height;
    for (label, predicate, expected) in [
        (
            "plain form",
            red,
            expected_pixel_bbox(72.0, 100.0, 172.0, 150.0, height),
        ),
        (
            "scaled bbox form",
            blue,
            expected_pixel_bbox(300.0, 100.0, 400.0, 150.0, height),
        ),
        (
            "rotated matrix form",
            green,
            expected_pixel_bbox(72.0, 500.0, 172.0, 550.0, height),
        ),
    ] {
        let before_bounds = color_bounds(&before, predicate)
            .unwrap_or_else(|| panic!("{label} is rendered before flattening"));
        let after_bounds = color_bounds(&after, predicate)
            .unwrap_or_else(|| panic!("{label} is rendered after flattening"));
        assert_bbox_close(&format!("{label} before"), before_bounds, expected);
        assert_bbox_close(&format!("{label} after"), after_bounds, expected);
    }
    assert!(
        error < 2.0,
        "the flattened page renders like the annotated page (MAE {error})"
    );

    // The appearance stream's own font must stay usable when the form is
    // painted from the page content stream.
    let dark = |pixel: &[u8]| pixel[0] < 120 && pixel[1] < 120 && pixel[2] < 120;
    let text_region = |pixels: &Pixels| {
        let mut bounds: Option<(u32, u32, u32, u32)> = None;
        let (x_from, x_to) = (296u32, 404u32);
        let (y_from, y_to) = (
            (height as f32 - 555.0) as u32,
            (height as f32 - 495.0) as u32,
        );
        for y in y_from..y_to {
            for x in x_from..x_to {
                let offset = ((y * pixels.width + x) * 4) as usize;
                if dark(&pixels.rgba[offset..offset + 4]) {
                    bounds = Some(match bounds {
                        None => (x, y, x, y),
                        Some((left, top, right, bottom)) => {
                            (left.min(x), top.min(y), right.max(x), bottom.max(y))
                        }
                    });
                }
            }
        }
        bounds
    };
    let before_text =
        text_region(&before).expect("the appearance's own font draws text before flattening");
    let after_text =
        text_region(&after).expect("the appearance's own font draws text after flattening");
    for (index, (before_value, after_value)) in [
        (before_text.0, after_text.0),
        (before_text.1, after_text.1),
        (before_text.2, after_text.2),
        (before_text.3, after_text.3),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            (before_value as i32 - after_value as i32).abs() <= 2,
            "appearance text component {index} moved: {before_text:?} vs {after_text:?}"
        );
    }

    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

/// A four-page document with inherited `/Resources` on the page tree and every
/// page carrying one visual annotation. The content streams exercise a
/// referenced stream array, a missing entry, a null entry and a direct array.
fn inherited_resources_contents_pdf() -> synthetic::TempPdf {
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(lopdf::dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let first_content = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"BT /F1 12 Tf 72 700 Td (Inherited text) Tj ET\n".to_vec(),
    ));
    let gray_content = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"0.5 0.5 0.5 rg 10 10 30 20 re f\n".to_vec(),
    ));
    let contents_array_id = document.add_object(Object::Array(vec![
        Object::Reference(first_content),
        Object::Reference(gray_content),
    ]));
    let existing_form = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(10),
                Object::Integer(10),
            ]),
        },
        b"0 0 0 rg 0 0 10 10 re f".to_vec(),
    ));
    let appearance = synthetic_form(
        &mut document,
        [0.0, 0.0, 20.0, 20.0],
        None,
        None,
        b"0 0.4 0.8 rg 0 0 20 20 re f",
    );

    let first =
        synthetic_square_annotation(&mut document, [100.0, 100.0, 120.0, 120.0], appearance);
    let second =
        synthetic_square_annotation(&mut document, [150.0, 150.0, 170.0, 170.0], appearance);
    let third =
        synthetic_square_annotation(&mut document, [200.0, 200.0, 220.0, 220.0], appearance);
    let fourth =
        synthetic_square_annotation(&mut document, [250.0, 250.0, 270.0, 270.0], appearance);

    let page_one = synthetic_page(
        &mut document,
        pages_id,
        Some(Object::Reference(contents_array_id)),
        first,
    );
    let page_two = synthetic_page(&mut document, pages_id, None, second);
    let page_three = synthetic_page(&mut document, pages_id, Some(Object::Null), third);
    let page_four = synthetic_page(
        &mut document,
        pages_id,
        Some(Object::Array(vec![
            Object::Reference(first_content),
            Object::Reference(gray_content),
        ])),
        fourth,
    );

    document.set_object(
        pages_id,
        lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(vec![
                Object::Reference(page_one),
                Object::Reference(page_two),
                Object::Reference(page_three),
                Object::Reference(page_four),
            ]),
            "Count" => Object::Integer(4),
            "Resources" => lopdf::dictionary! {
                "Font" => lopdf::dictionary! { "F1" => Object::Reference(font_id) },
                "XObject" => lopdf::dictionary! {
                    "PDFXFlat1" => Object::Reference(existing_form),
                },
            },
        },
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    document.trailer.set("Root", Object::Reference(catalog_id));

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize the inherited-resources PDF");
    synthetic::pdf_bytes_file("inherited-resources-contents", bytes)
}

#[test]
fn content_stream_shapes_and_inherited_resources_survive_flattening() {
    let pdf = inherited_resources_contents_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();

    let temp = TempDir::new("flatten-content-shapes");
    let destination = temp.path().join("content-shapes.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 4, "{exported:?}");
    assert!(exported.skipped.is_empty(), "{exported:?}");

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    for page_number in 1..=4 {
        let page_id = *parsed.get_pages().get(&page_number).expect("page");
        let page = parsed.get_dictionary(page_id).expect("page dictionary");
        assert!(
            !page.has(b"Annots"),
            "page {page_number} keeps no visual annotation"
        );
        assert_eq!(
            count_occurrences(&parsed.get_page_content(page_id), b"PDFXFlat"),
            1,
            "page {page_number} paints its flattened form"
        );
    }

    let first = page_content(&parsed, 1);
    for expected in [
        b"Inherited text".as_slice(),
        b"0.5 0.5 0.5 rg".as_slice(),
        b"/PDFXFlat2 Do".as_slice(),
    ] {
        assert!(
            first
                .windows(expected.len())
                .any(|window| window == expected),
            "page 1 content keeps {expected:?}"
        );
    }
    let fourth = page_content(&parsed, 4);
    assert!(
        fourth
            .windows(b"Inherited text".len())
            .any(|window| window == b"Inherited text"),
        "a direct contents array keeps its existing streams"
    );

    let page_id = *parsed.get_pages().get(&1).expect("first page");
    let page = parsed.get_dictionary(page_id).expect("page dictionary");
    let resources = page
        .get(b"Resources")
        .expect("/Resources")
        .as_dict()
        .expect("resources dictionary");
    assert!(
        resources
            .get(b"Font")
            .expect("/Font")
            .as_dict()
            .expect("font dictionary")
            .has(b"F1"),
        "the inherited font resource is preserved on the flattened page"
    );
    let xobjects = resources
        .get(b"XObject")
        .expect("/XObject")
        .as_dict()
        .expect("xobject dictionary");
    assert!(
        xobjects.has(b"PDFXFlat1"),
        "an existing form resource name is never overwritten"
    );
    assert!(
        xobjects.has(b"PDFXFlat2"),
        "the flattened form gets a fresh unique name"
    );

    let reopened = open_path(&destination, None);
    let layer =
        extract_text_layer(reopened.document_id.clone(), 0).expect("text layer in the copy");
    assert!(
        layer.text.contains("Inherited text"),
        "inherited /Resources still resolve the font: {:?}",
        layer.text
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

/// Replaces the trailer `/ID` hex strings with zeros. Pdfium generates a fresh
/// random document ID on every save, so two saves of the same state otherwise
/// differ only there.
fn mask_document_ids(bytes: &[u8]) -> Vec<u8> {
    let mut masked = bytes.to_vec();
    let Some(position) = masked.windows(4).position(|window| window == b"/ID[") else {
        return masked;
    };
    let mut position = position + 4;
    for _ in 0..2 {
        let Some(open) = masked[position..].iter().position(|byte| *byte == b'<') else {
            break;
        };
        let open = position + open + 1;
        let Some(close) = masked[open..].iter().position(|byte| *byte == b'>') else {
            break;
        };
        let close = open + close;
        for byte in &mut masked[open..close] {
            *byte = b'0';
        }
        position = close + 1;
    }
    masked
}

#[test]
fn flatten_export_leaves_save_output_and_save_state_unchanged() {
    let path = fixture_path("text-latin");
    let view = open_path(&path, None);
    let id = view.document_id.clone();
    let layer = extract_text_layer(id.clone(), 0).expect("text layer");
    add_markup(
        &id,
        TextMarkupKind::Highlight,
        selection_rect(&layer, "PDFX"),
    );

    let temp = TempDir::new("flatten-save-parity");
    let first_output = temp.path().join("first.pdf");
    save_as(&id, &first_output);
    let first_bytes = fs::read(&first_output).expect("read the first save");
    let history = edit_history(id.clone()).expect("edit history after saving");

    let flattened = temp.path().join("flattened.pdf");
    let exported = export(&id, &flattened);
    assert_eq!(exported.flattened, 1);
    assert_eq!(
        edit_history(id.clone()).expect("edit history after exporting"),
        history,
        "the flatten export must not move the saved baseline"
    );

    let second_output = temp.path().join("second.pdf");
    save_as(&id, &second_output);
    let second_bytes = fs::read(&second_output).expect("read the second save");
    assert_eq!(
        mask_document_ids(&second_bytes),
        mask_document_ids(&first_bytes),
        "the flatten export must not change what Save writes (the random trailer /ID aside)"
    );

    let parsed = lopdf::Document::load_mem(&second_bytes).expect("parse the second save");
    let annotations = page_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 1);
    assert_eq!(
        subtype(&annotations[0]),
        "Highlight",
        "the saved document still carries its annotation"
    );
    let page_id = *parsed.get_pages().get(&1).expect("first page");
    assert_eq!(
        count_occurrences(&parsed.get_page_content(page_id), b"PDFXFlat"),
        0,
        "saving after a flatten export never writes flattened content"
    );
    close_document(id).expect("close the live document");
}

#[test]
fn visual_annotations_flatten_on_a_page_that_also_keeps_form_widgets() {
    let pdf = form_fixture_with_uri_link();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    let widget_count = form_fields(id.clone()).expect("form fields").len();
    assert_eq!(widget_count, 12, "the fixture has twelve widgets");
    add_shape(&id, ShapeKind::Square, 60.0, 200.0);

    let temp = TempDir::new("flatten-mixed-page");
    let destination = temp.path().join("mixed.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 1, "{exported:?}");
    assert_eq!(
        exported.preserved,
        widget_count as u32 + 1,
        "every widget and the link stay interactive: {exported:?}"
    );
    assert!(exported.skipped.is_empty(), "{exported:?}");

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let mut widgets = 0;
    let mut links = 0;
    for page_number in 1..=parsed.get_pages().len() as u32 {
        for annotation in page_annotations(&parsed, page_number) {
            match subtype(&annotation).as_str() {
                "Widget" => widgets += 1,
                "Link" => links += 1,
                other => panic!("visual annotation {other} was not flattened"),
            }
        }
    }
    assert_eq!(widgets, widget_count, "all widgets remain on the pages");
    assert_eq!(links, 1, "the URI link remains");
    let page_id = *parsed.get_pages().get(&1).expect("first page");
    assert_eq!(
        count_occurrences(&parsed.get_page_content(page_id), b"PDFXFlat"),
        1,
        "the shape was drawn into the page that keeps its widgets"
    );

    let reopened = open_path(&destination, None);
    assert_eq!(
        form_fields(reopened.document_id.clone())
            .expect("fields in the copy")
            .len(),
        widget_count,
        "the widgets remain fillable fields in the copy"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}

/// A sticky-note-style text annotation with its own icon appearance and a
/// popup in `/Annots`, exactly the shape real producers write.
fn note_with_popup_pdf() -> synthetic::TempPdf {
    let mut document = lopdf::Document::with_version("1.7");
    let pages_id = document.new_object_id();
    let content_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"BT /F1 12 Tf 72 720 Td (Base) Tj ET".to_vec(),
    ));
    let icon = synthetic_form(
        &mut document,
        [0.0, 0.0, 24.0, 24.0],
        None,
        None,
        b"1 0.8 0 rg 0 0 24 24 re f 0 0 0 RG 1 w 0.5 0.5 23 23 re S",
    );

    let note_id = document.new_object_id();
    let popup_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "Popup",
        "Rect" => Object::Array(vec![
            Object::Real(100.0),
            Object::Real(650.0),
            Object::Real(300.0),
            Object::Real(700.0),
        ]),
        "Parent" => Object::Reference(note_id),
    });
    document.set_object(
        note_id,
        lopdf::dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => Object::Array(vec![
                Object::Real(72.0),
                Object::Real(650.0),
                Object::Real(96.0),
                Object::Real(674.0),
            ]),
            "Contents" => Object::string_literal("note with an icon"),
            "AP" => lopdf::dictionary! { "N" => Object::Reference(icon) },
            "Popup" => Object::Reference(popup_id),
        },
    );

    let page_id = document.add_object(lopdf::dictionary! {
        "Type" => "Page",
        "Parent" => Object::Reference(pages_id),
        "MediaBox" => Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
        "Resources" => lopdf::dictionary! {},
        "Contents" => Object::Reference(content_id),
        "Annots" => Object::Array(vec![
            Object::Reference(note_id),
            Object::Reference(popup_id),
        ]),
    });
    document.set_object(
        pages_id,
        lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(vec![Object::Reference(page_id)]),
            "Count" => Object::Integer(1),
        },
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    document.trailer.set("Root", Object::Reference(catalog_id));

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize the note-with-popup PDF");
    synthetic::pdf_bytes_file("note-with-popup", bytes)
}

#[test]
fn flattening_a_note_draws_its_icon_and_leaves_no_orphan_popup() {
    let pdf = note_with_popup_pdf();
    let view = open_path(Path::new(&pdf.path_string()), None);
    let id = view.document_id.clone();
    let before = render(&id, 0, 1.0);

    let temp = TempDir::new("flatten-note-popup");
    let destination = temp.path().join("note-popup.pdf");
    let exported = export(&id, &destination);
    assert_eq!(exported.flattened, 1, "{exported:?}");
    assert_eq!(exported.preserved, 0, "{exported:?}");
    assert_eq!(exported.skipped.len(), 1, "{exported:?}");
    assert_eq!(exported.skipped[0].subtype, "Popup");
    assert_eq!(exported.skipped[0].reason, FlattenSkipReason::Popup);

    let parsed = lopdf::Document::load(&destination).expect("parse the flattened copy");
    let page_id = *parsed.get_pages().get(&1).expect("first page");
    assert!(
        page_annotations(&parsed, 1).is_empty(),
        "the note and its popup are both gone, so no popup is orphaned"
    );
    assert_eq!(
        count_occurrences(&parsed.get_page_content(page_id), b"PDFXFlat"),
        1,
        "the note's icon appearance is drawn into the page"
    );

    let reopened = open_path(&destination, None);
    let after = render(&reopened.document_id, 0, 1.0);
    assert!(
        mean_absolute_error(&before, &after) < 2.0,
        "the flattened note icon renders like the annotation"
    );
    close_document(reopened.document_id).expect("close the copy");
    close_document(id).expect("close the live document");
}
