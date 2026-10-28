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

use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::form::{
    fill_form_field, form_fields, FormFieldKind, FormFieldValueInput, FormFieldView,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};

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
    resolved(
        document,
        catalog(document).get(b"AcroForm").expect("/AcroForm"),
    )
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

/// Content of the first widget's normal appearance for a field name, whether
/// the field is merged with its widget or keeps widgets in `/Kids`.
fn field_appearance_content(document: &lopdf::Document, field_name: &str) -> String {
    let field = find_field(document, field_name);
    let widget_id = widget_ids(document, &field, field_name)[0];
    appearance_content(document, widget(document, widget_id))
}

fn widget(document: &lopdf::Document, id: lopdf::ObjectId) -> &lopdf::Dictionary {
    document.get_dictionary(id).expect("widget dictionary")
}

fn name_of(object: &lopdf::Object) -> String {
    String::from_utf8_lossy(object.as_name().expect("name")).into_owned()
}

/// `/AP /N` of a button widget: a dictionary of appearance states.
fn state_appearances<'a>(
    document: &'a lopdf::Document,
    widget: &'a lopdf::Dictionary,
) -> &'a lopdf::Dictionary {
    let appearance = resolved(document, widget.get(b"AP").expect("/AP"))
        .as_dict()
        .expect("/AP dictionary");
    resolved(document, appearance.get(b"N").expect("/N"))
        .as_dict()
        .expect("/N state dictionary")
}

fn state_stream_content(document: &lopdf::Document, state: &lopdf::Object) -> String {
    String::from_utf8_lossy(
        &resolved(document, state)
            .as_stream()
            .expect("state appearance stream")
            .decompressed_content()
            .expect("state appearance content"),
    )
    .into_owned()
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

/// Pixels that are strongly red: the colour of every stale appearance in the
/// fixture. A saved page must contain none of them.
fn red_pixels(pixels: &DecodedPng) -> usize {
    pixels
        .rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 180 && pixel[1] < 110 && pixel[2] < 110)
        .count()
}

/// Non-white pixels inside a display-point region.
fn ink_in_region(pixels: &DecodedPng, scale: f32, region: (f32, f32, f32, f32)) -> usize {
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
    }
    count
}

fn fill_all_fields(document_id: &str) {
    let fields = form_fields(document_id.to_owned()).expect("fields");
    fill(
        document_id,
        field_named(&fields, "membership_id").field_id.clone(),
        FormFieldValueInput::Text {
            text: "MEM-2026-0001".to_owned(),
        },
    );
    fill(
        document_id,
        field_named(&fields, "person.name").field_id.clone(),
        FormFieldValueInput::Text {
            text: "Katherine Johnson".to_owned(),
        },
    );
    fill(
        document_id,
        field_named(&fields, "subscribe").field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );
    let gender: Vec<FormFieldView> = fields
        .iter()
        .filter(|field| field.name == "gender")
        .cloned()
        .collect();
    fill(
        document_id,
        gender[1].field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );
    fill(
        document_id,
        field_named(&fields, "country").field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Japan".to_owned(),
        },
    );
    fill(
        document_id,
        field_named(&fields, "comments").field_id.clone(),
        FormFieldValueInput::Text {
            text: "合同已审阅".to_owned(),
        },
    );
    fill(
        document_id,
        field_named(&fields, "department").field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Design".to_owned(),
        },
    );
}

#[test]
fn saved_pages_render_the_new_values_and_never_the_stale_appearances() {
    let dir = support::temp::TempDir::new("fill-appearance-pixels");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");

    // The fixture really does carry red stale appearances: rendering the
    // source proves the colour check below can fail.
    let source_document = open_at(&source);
    let source_pixels = render_pixels(&source_document, 0, 2.0);
    assert!(
        red_pixels(&source_pixels) > 0,
        "the source fixture's stale appearances are visible"
    );
    close_document(source_document).expect("close source preview");

    let document_id = open_at(&source);
    fill_all_fields(&document_id);
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    // Independent lopdf: the rotated page's appearance compensates /Rotate,
    // and the CJK value is a UTF-16BE string.
    let parsed = lopdf::Document::load(&output).expect("parse output");
    let comments = find_field(&parsed, "comments");
    assert_eq!(text_of(comments.get(b"V").expect("/V")), "合同已审阅");
    let comments_appearance = appearance_content(&parsed, &comments);
    assert!(
        comments_appearance.starts_with("q 0 1 -1 0 400 600 cm"),
        "the appearance compensates a 90-degree page rotation: {comments_appearance:?}"
    );
    assert!(
        comments_appearance.contains("Tj"),
        "the CJK value is drawn in the appearance: {comments_appearance:?}"
    );

    let reopened = open_at(&output);
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(
        field_named(&fields, "comments").value.as_deref(),
        Some("合同已审阅")
    );
    assert_eq!(
        field_named(&fields, "department").value.as_deref(),
        Some("Design")
    );

    // Fixed-DPI rendering: page 0 and the rotated page 1 draw the new values
    // and no stale red pixel survives anywhere.
    let page_one = render_pixels(&reopened, 0, 2.0);
    assert_eq!(
        red_pixels(&page_one),
        0,
        "page 1 must not keep any stale source appearance"
    );
    let page_two = render_pixels(&reopened, 1, 2.0);
    assert_eq!(
        page_two.width,
        (792.0 * 2.0) as u32,
        "the rotated page renders in display orientation"
    );
    assert_eq!(
        red_pixels(&page_two),
        0,
        "page 2 must not keep any stale source appearance"
    );
    // Display-space regions of the comments and department widgets on the
    // 90-degree page: user (x, y) maps to display (y, x).
    let comments_ink = ink_in_region(&page_two, 2.0, (600.0, 72.0, 680.0, 400.0));
    assert!(
        comments_ink > 200,
        "the filled comments widget draws its text, found {comments_ink} ink pixels"
    );
    let department_ink = ink_in_region(&page_two, 2.0, (500.0, 72.0, 524.0, 300.0));
    assert!(
        department_ink > 100,
        "the filled combo widget draws its label, found {department_ink} ink pixels"
    );
    close_document(reopened).expect("close reopened");
}

#[test]
fn fills_checkbox_radio_and_choice_with_matching_appearance_states() {
    let dir = support::temp::TempDir::new("fill-appearance-buttons");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");

    let document_id = open_at(&source);
    let fields = form_fields(document_id.clone()).expect("discover fields");

    let subscribe = field_named(&fields, "subscribe");
    assert!(!subscribe.checked);
    fill(
        &document_id,
        subscribe.field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );

    let gender: Vec<FormFieldView> = fields
        .iter()
        .filter(|field| field.name == "gender")
        .cloned()
        .collect();
    assert_eq!(gender.len(), 3);
    fill(
        &document_id,
        gender[1].field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );

    let country = field_named(&fields, "country");
    assert_eq!(country.value.as_deref(), Some("China"));
    fill(
        &document_id,
        country.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Japan".to_owned(),
        },
    );

    let department = field_named(&fields, "department");
    fill(
        &document_id,
        department.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Design".to_owned(),
        },
    );

    let after = form_fields(document_id.clone()).expect("fields after fill");
    assert!(field_named(&after, "subscribe").checked);
    assert!(after
        .iter()
        .filter(|field| field.name == "gender")
        .any(|field| field.checked));
    assert_eq!(
        field_named(&after, "country").value.as_deref(),
        Some("Japan")
    );
    assert_eq!(
        field_named(&after, "department").value.as_deref(),
        Some("Design")
    );

    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");

    // Checkbox: /V and /AS agree, and the appearance dictionary carries both
    // states with fresh streams.
    let subscribe = find_field(&parsed, "subscribe");
    assert_eq!(name_of(subscribe.get(b"V").expect("/V")), "Yes");
    assert_eq!(name_of(subscribe.get(b"AS").expect("/AS")), "Yes");
    let states = state_appearances(&parsed, &subscribe);
    assert!(states.has(b"Off"), "the Off appearance state survives");
    for state in [b"Off".as_slice(), b"Yes".as_slice()] {
        let content = state_stream_content(&parsed, states.get(state).expect("appearance state"));
        assert!(
            content.contains("1 1 1 rg"),
            "checkbox {state:?} appearance is regenerated: {content:?}"
        );
    }

    // Radio group: /V on the parent, /AS only on the selected kid.
    let gender = find_field(&parsed, "gender");
    assert_eq!(name_of(gender.get(b"V").expect("/V")), "female");
    let kids = gender
        .get(b"Kids")
        .expect("/Kids")
        .as_array()
        .expect("kid array");
    let mut appearance_states = Vec::new();
    for (index, kid) in kids.iter().enumerate() {
        let kid = resolved(&parsed, kid).as_dict().expect("kid dictionary");
        appearance_states.push(name_of(kid.get(b"AS").expect("/AS")));
        let states = state_appearances(&parsed, kid);
        let on_state = ["male", "female", "other"][index];
        assert!(
            states.has(on_state.as_bytes()),
            "kid {index} keeps its on-state {on_state}"
        );
        let content =
            state_stream_content(&parsed, states.get(on_state.as_bytes()).expect("on state"));
        assert!(
            content.contains("1 1 1 rg"),
            "radio kid {index} appearance is regenerated: {content:?}"
        );
        let content = state_stream_content(&parsed, states.get(b"Off".as_slice()).expect("Off"));
        assert!(
            content.contains("1 1 1 rg"),
            "radio kid {index} Off appearance is regenerated: {content:?}"
        );
    }
    assert_eq!(appearance_states, ["Off", "female", "Off"]);

    // List box: /V and /I select the second label; the selected row and the
    // other options are drawn, not the stale text.
    let country = find_field(&parsed, "country");
    assert_eq!(text_of(country.get(b"V").expect("/V")), "Japan");
    let indexes: Vec<i64> = country
        .get(b"I")
        .expect("/I")
        .as_array()
        .expect("index array")
        .iter()
        .map(|index| index.as_i64().expect("integer"))
        .collect();
    assert_eq!(indexes, [1]);
    let country_appearance = appearance_content(&parsed, &country);
    assert!(
        country_appearance.contains("0.20 0.45 0.85 rg"),
        "the selected list row is drawn: {country_appearance:?}"
    );
    assert!(
        !country_appearance.contains("STALE"),
        "the stale list appearance is replaced"
    );

    // Combo box: the selected label is drawn inside the field, with the
    // dropdown affordance and a fresh stream.
    let department = find_field(&parsed, "department");
    assert_eq!(text_of(department.get(b"V").expect("/V")), "Design");
    let department_appearance = appearance_content(&parsed, &department);
    assert!(
        department_appearance.contains("BT") && !department_appearance.contains("STALE"),
        "the combo appearance draws its label: {department_appearance:?}"
    );

    // PDFium reopen agrees with the saved values.
    let reopened = open_at(&output);
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert!(field_named(&fields, "subscribe").checked);
    assert_eq!(
        field_named(&fields, "country").value.as_deref(),
        Some("Japan")
    );
    assert_eq!(
        field_named(&fields, "department").value.as_deref(),
        Some("Design")
    );
    let selected: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "gender" && field.checked)
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].export_value.as_deref(), Some("female"));
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close source");
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
        noto.get(b"Subtype")
            .and_then(|object| object.as_name())
            .ok(),
        Some(b"Type0".as_slice())
    );
    let descendants = resolved(
        &parsed,
        noto.get(b"DescendantFonts").expect("/DescendantFonts"),
    )
    .as_array()
    .expect("descendant fonts");
    let descendant = resolved(&parsed, &descendants[0])
        .as_dict()
        .expect("descendant font");
    let descriptor = resolved(
        &parsed,
        descendant.get(b"FontDescriptor").expect("/FontDescriptor"),
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

#[test]
fn undo_then_save_omits_the_reverted_appearance_and_redo_restores_it() {
    let dir = support::temp::TempDir::new("fill-appearance-undo");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");
    let source_parsed = lopdf::Document::load(&source).expect("parse source");
    let stale_membership = field_appearance_content(&source_parsed, "membership_id");
    let stale_comments = field_appearance_content(&source_parsed, "comments");
    assert!(stale_membership.contains("STALE"));
    assert!(stale_comments.contains("STALE"));

    let document_id = open_at(&source);
    let fields = form_fields(document_id.clone()).expect("fields");
    let membership_id = field_named(&fields, "membership_id").field_id.clone();

    // Fill, undo, redo on a terminal field whose value lives on the parent.
    fill(
        &document_id,
        membership_id,
        FormFieldValueInput::Text {
            text: "MEM-1".to_owned(),
        },
    );
    let filled = dir.path().join("membership-filled.pdf");
    save_as(&document_id, &filled);
    let parsed = lopdf::Document::load(&filled).expect("parse filled");
    assert_eq!(
        text_of(find_field(&parsed, "membership_id").get(b"V").expect("/V")),
        "MEM-1"
    );
    let fresh_membership = field_appearance_content(&parsed, "membership_id");
    assert!(!fresh_membership.contains("STALE"));
    assert!(fresh_membership.contains("1 1 1 rg"));

    undo_text_edit(document_id.clone()).expect("undo membership fill");
    let live = form_fields(document_id.clone()).expect("fields after undo");
    assert_eq!(
        field_named(&live, "membership_id").value.as_deref(),
        Some("OLD-0001")
    );
    let undone = dir.path().join("membership-undone.pdf");
    save_as(&document_id, &undone);
    let parsed = lopdf::Document::load(&undone).expect("parse undone");
    assert_eq!(
        text_of(find_field(&parsed, "membership_id").get(b"V").expect("/V")),
        "OLD-0001"
    );
    assert_eq!(
        field_appearance_content(&parsed, "membership_id"),
        stale_membership,
        "a reverted fill must not emit the reverted appearance"
    );

    redo_text_edit(document_id.clone()).expect("redo membership fill");
    let redone = dir.path().join("membership-redone.pdf");
    save_as(&document_id, &redone);
    let parsed = lopdf::Document::load(&redone).expect("parse redone");
    assert_eq!(
        text_of(find_field(&parsed, "membership_id").get(b"V").expect("/V")),
        "MEM-1"
    );
    assert_eq!(
        field_appearance_content(&parsed, "membership_id"),
        fresh_membership,
        "redo restores the same appearance as the original fill"
    );

    // The same history contract holds for a field Pdfium writes directly.
    let comments_id = field_named(&fields, "comments").field_id.clone();
    fill(
        &document_id,
        comments_id,
        FormFieldValueInput::Text {
            text: "合同已审阅".to_owned(),
        },
    );
    let comments_filled = dir.path().join("comments-filled.pdf");
    save_as(&document_id, &comments_filled);
    let parsed = lopdf::Document::load(&comments_filled).expect("parse comments");
    assert_eq!(
        text_of(find_field(&parsed, "comments").get(b"V").expect("/V")),
        "合同已审阅"
    );
    let fresh_comments = field_appearance_content(&parsed, "comments");
    assert!(!fresh_comments.contains("STALE"));
    assert!(fresh_comments.contains("Tj"));

    undo_text_edit(document_id.clone()).expect("undo comments fill");
    let live = form_fields(document_id.clone()).expect("fields after undo");
    assert_eq!(
        field_named(&live, "comments").value.as_deref(),
        Some("Old comment")
    );
    let comments_undone = dir.path().join("comments-undone.pdf");
    save_as(&document_id, &comments_undone);
    let parsed = lopdf::Document::load(&comments_undone).expect("parse undone comments");
    assert_eq!(
        text_of(find_field(&parsed, "comments").get(b"V").expect("/V")),
        "Old comment"
    );
    assert_eq!(
        field_appearance_content(&parsed, "comments"),
        stale_comments,
        "the reverted CJK value is not drawn"
    );

    redo_text_edit(document_id.clone()).expect("redo comments fill");
    let comments_redone = dir.path().join("comments-redone.pdf");
    save_as(&document_id, &comments_redone);
    let parsed = lopdf::Document::load(&comments_redone).expect("parse redone comments");
    assert_eq!(
        text_of(find_field(&parsed, "comments").get(b"V").expect("/V")),
        "合同已审阅"
    );
    assert_eq!(
        field_appearance_content(&parsed, "comments"),
        fresh_comments
    );

    close_document(document_id).expect("close document");
}

#[test]
fn copied_pages_get_independent_field_names_annotation_names_and_appearances() {
    use pdfx_lib::page_ops::copy_page;

    let dir = support::temp::TempDir::new("fill-appearance-copy");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");

    let document_id = open_at(&source);
    let fields = form_fields(document_id.clone()).expect("fields");
    fill(
        &document_id,
        field_named(&fields, "membership_id").field_id.clone(),
        FormFieldValueInput::Text {
            text: "SOURCE-1".to_owned(),
        },
    );
    copy_page(document_id.clone(), 0).expect("copy page 1");

    let fields = form_fields(document_id.clone()).expect("fields after copy");
    let copied_name = "membership_id__copy_p2";
    let copied = fields
        .iter()
        .find(|field| field.name == copied_name)
        .unwrap_or_else(|| panic!("copied field {copied_name}: {fields:?}"));
    fill(
        &document_id,
        copied.field_id.clone(),
        FormFieldValueInput::Text {
            text: "COPY-1".to_owned(),
        },
    );
    // Copied buttons keep independent values too.
    fill(
        &document_id,
        field_named(&fields, "subscribe__copy_p2").field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );
    let copied_gender: Vec<FormFieldView> = fields
        .iter()
        .filter(|field| field.name == "gender__copy_p2")
        .cloned()
        .collect();
    assert_eq!(copied_gender.len(), 3);
    fill(
        &document_id,
        copied_gender[1].field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    );

    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let source_field = find_field(&parsed, "membership_id");
    let copy_field = find_field(&parsed, copied_name);
    assert_eq!(text_of(source_field.get(b"V").expect("/V")), "SOURCE-1");
    assert_eq!(text_of(copy_field.get(b"V").expect("/V")), "COPY-1");

    let source_widget_id = widget_ids(&parsed, &source_field, "membership_id")[0];
    let copy_widget_id = widget_ids(&parsed, &copy_field, copied_name)[0];
    assert_ne!(
        source_widget_id, copy_widget_id,
        "the copy owns its own widget object"
    );
    let source_widget = widget(&parsed, source_widget_id);
    let copy_widget = widget(&parsed, copy_widget_id);

    // Independent annotation names: the source keeps its /NM, the copy is
    // suffixed so no name-addressed lookup can reach across pages.
    assert_eq!(
        text_of(source_widget.get(b"NM").expect("/NM")),
        "w-membership"
    );
    assert_eq!(
        text_of(copy_widget.get(b"NM").expect("/NM")),
        "w-membership__copy_p2"
    );

    // Independent appearance objects with the matching values drawn.
    let appearance_of = |widget: &lopdf::Dictionary| {
        resolved(&parsed, widget.get(b"AP").expect("/AP"))
            .as_dict()
            .expect("/AP dictionary")
            .get(b"N")
            .expect("/N")
            .as_reference()
            .expect("/N reference")
    };
    let source_appearance_id = appearance_of(source_widget);
    let copy_appearance_id = appearance_of(copy_widget);
    assert_ne!(
        source_appearance_id, copy_appearance_id,
        "the copy's appearance is a new object, never the source's"
    );
    let source_content = field_appearance_content(&parsed, "membership_id");
    let copy_content = field_appearance_content(&parsed, copied_name);
    assert!(source_content.contains("1 1 1 rg"));
    assert!(copy_content.contains("1 1 1 rg"));
    assert_ne!(
        source_content, copy_content,
        "the two appearances draw different values"
    );

    // The copy's checkbox and radio group are independent of the source's.
    let source_subscribe = find_field(&parsed, "subscribe");
    assert_eq!(name_of(source_subscribe.get(b"V").expect("/V")), "Off");
    assert_eq!(name_of(source_subscribe.get(b"AS").expect("/AS")), "Off");
    let copy_subscribe = find_field(&parsed, "subscribe__copy_p2");
    assert_eq!(name_of(copy_subscribe.get(b"V").expect("/V")), "Yes");
    assert_eq!(name_of(copy_subscribe.get(b"AS").expect("/AS")), "Yes");

    assert_eq!(
        name_of(find_field(&parsed, "gender").get(b"V").expect("/V")),
        "Off"
    );
    let copy_gender = find_field(&parsed, "gender__copy_p2");
    assert_eq!(name_of(copy_gender.get(b"V").expect("/V")), "female");
    let states: Vec<String> = copy_gender
        .get(b"Kids")
        .expect("/Kids")
        .as_array()
        .expect("kid array")
        .iter()
        .map(|kid| {
            name_of(
                resolved(&parsed, kid)
                    .as_dict()
                    .expect("kid")
                    .get(b"AS")
                    .expect("/AS"),
            )
        })
        .collect();
    assert_eq!(states, ["Off", "female", "Off"]);

    // PDFium reopen sees both values, each on its own page.
    let reopened = open_at(&output);
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(
        field_named(&fields, "membership_id").value.as_deref(),
        Some("SOURCE-1")
    );
    assert_eq!(
        field_named(&fields, copied_name).value.as_deref(),
        Some("COPY-1")
    );
    assert!(field_named(&fields, "subscribe__copy_p2").checked);
    assert!(!field_named(&fields, "subscribe").checked);
    close_document(reopened).expect("close reopened");
}

fn byte_count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[derive(Debug, PartialEq)]
struct ObjectProfile {
    objects: usize,
    type0_fonts: usize,
    embedded_fonts: usize,
    form_xobjects: usize,
}

fn object_profile(document: &lopdf::Document) -> ObjectProfile {
    fn named(dictionary: &lopdf::Dictionary, key: &[u8], expected: &[u8]) -> bool {
        dictionary
            .get(key)
            .ok()
            .and_then(|value| value.as_name().ok())
            .is_some_and(|name| name == expected)
    }
    let mut profile = ObjectProfile {
        objects: document.objects.len(),
        type0_fonts: 0,
        embedded_fonts: 0,
        form_xobjects: 0,
    };
    for object in document.objects.values() {
        // Appearance streams are stream objects; their dictionaries hold the
        // `/Subtype /Form` and font descriptors we count here.
        let dictionary = match object {
            lopdf::Object::Dictionary(dictionary) => dictionary,
            lopdf::Object::Stream(stream) => &stream.dict,
            _ => continue,
        };
        if named(dictionary, b"Type", b"Font") && named(dictionary, b"Subtype", b"Type0") {
            profile.type0_fonts += 1;
        }
        if dictionary.get(b"FontFile2").is_ok() {
            profile.embedded_fonts += 1;
        }
        if named(dictionary, b"Subtype", b"Form") {
            profile.form_xobjects += 1;
        }
    }
    profile
}

fn default_font_names(document: &lopdf::Document) -> Vec<String> {
    let dr = acroform(document)
        .get(b"DR")
        .expect("/DR")
        .as_dict()
        .expect("/DR dictionary");
    let fonts = resolved(document, dr.get(b"Font").expect("/Font"))
        .as_dict()
        .expect("/Font dictionary");
    fonts
        .iter()
        .map(|(name, _)| String::from_utf8_lossy(name).into_owned())
        .collect()
}

#[test]
fn repeated_saves_append_one_revision_and_never_grow_fonts_or_appearances() {
    let dir = support::temp::TempDir::new("fill-appearance-repeat");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-fill-appearance"), &source).expect("copy fixture");
    let source_bytes = fs::read(&source).expect("source bytes");
    // The fixture is one revision with six stale appearance streams.
    assert_eq!(byte_count(&source_bytes, b"startxref"), 1);
    assert_eq!(byte_count(&source_bytes, b"%%EOF"), 1);

    let document_id = open_at(&source);
    fill_all_fields(&document_id);
    let first_path = dir.path().join("first.pdf");
    let second_path = dir.path().join("second.pdf");
    save_as(&document_id, &first_path);
    save_as(&document_id, &second_path);
    close_document(document_id).expect("close");

    assert_eq!(
        fs::read(&source).expect("source after saves"),
        source_bytes,
        "saving never rewrites the source document"
    );

    let first_bytes = fs::read(&first_path).expect("first bytes");
    let second_bytes = fs::read(&second_path).expect("second bytes");
    for (label, bytes) in [("first", &first_bytes), ("second", &second_bytes)] {
        // The source revision, PDFium's incremental one, and at most one
        // appended lopdf revision: a save never appends a second lopdf update.
        assert_eq!(byte_count(bytes, b"startxref"), 3, "{label} revision count");
        assert_eq!(byte_count(bytes, b"%%EOF"), 3, "{label} eof count");
        assert_eq!(
            byte_count(bytes, b"/NeedAppearances"),
            0,
            "{label} never asks the viewer to regenerate appearances"
        );
    }

    let first = lopdf::Document::load(&first_path).expect("parse first");
    let second = lopdf::Document::load(&second_path).expect("parse second");
    let first_profile = object_profile(&first);
    let second_profile = object_profile(&second);
    assert_eq!(
        first_profile, second_profile,
        "a second save must not add objects"
    );
    // One Noto subset: Type0 font, descendant CIDFontType2's FontFile2 stream.
    assert_eq!(first_profile.type0_fonts, 1);
    assert_eq!(first_profile.embedded_fonts, 1);
    // Thirteen fresh appearances (membership, person.name, checkbox's two
    // states, radio's three widgets x two states, country, comments,
    // department) replace the fixture's six stale streams, which survive as
    // unreferenced source objects: 19 in all, and no more after a second save.
    assert_eq!(first_profile.form_xobjects, 19);

    // The default resources hold the source Helvetica plus our single subset.
    let font_names = default_font_names(&first);
    assert_eq!(font_names.len(), 2, "fonts: {font_names:?}");
    assert!(font_names.iter().any(|name| name == "Helv"));
    assert_eq!(default_font_names(&second), font_names);
}
