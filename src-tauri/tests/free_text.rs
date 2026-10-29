//! Command-boundary tests for free text and callout annotations.
//!
//! Free text is a native `/FreeText` annotation with `/Contents`, `/DA`, `/Q`,
//! `/IT` and a real `/AP`. The callout box shares that native representation
//! and is deliberately not a comment thread: no `/Popup`, `/Parent` or `/IRT`.
//! Every assertion goes through the same functions the Tauri IPC layer exposes,
//! and saved output is parsed with lopdf and reopened through the command
//! boundary so the checks apply to a fresh parse, not the live document.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_free_text_from_settings, create_free_text_with_author, create_note_with_author,
    delete_annotation, list_annotations, transform_free_text_annotation, update_free_text,
    AnnotationKind, CreateFreeTextRequest, CreateNoteRequest, FreeTextAlign, FreeTextKind,
    FreeTextView, TextMarkupRect,
};
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::settings::{default_annotation_author, Settings, SettingsStore};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn fixture_path(id: &str) -> String {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture '{id}' is missing from the manifest"));
    support::fixtures::fixture_path(fixture)
        .to_string_lossy()
        .into_owned()
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> TextMarkupRect {
    TextMarkupRect {
        x,
        y,
        width,
        height,
    }
}

fn request(
    kind: FreeTextKind,
    rect: TextMarkupRect,
    color: &str,
    opacity: u8,
    text: &str,
    font_size: f32,
    align: FreeTextAlign,
) -> CreateFreeTextRequest {
    CreateFreeTextRequest {
        page_index: 0,
        kind,
        rect,
        color: color.to_owned(),
        opacity,
        text: text.to_owned(),
        font_size,
        align,
    }
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
    .expect("save output");
}

/// A page carrying a foreign `/FreeText` annotation written by another
/// producer: no PDFX private keys, no `/Q` and no `/IT`.
fn foreign_free_text_pdf() -> synthetic::TempPdf {
    let mut document = lopdf::Document::with_version("1.7");
    let font_id = document.add_object(lopdf::dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding"
    });
    let content_id = document.add_object(lopdf::Stream::new(
        lopdf::dictionary! {},
        b"BT /F1 12 Tf 72 720 Td (Foreign) Tj ET".to_vec(),
    ));
    let annotation_id = document.add_object(lopdf::dictionary! {
        "Type" => "Annot",
        "Subtype" => "FreeText",
        "Rect" => Object::Array(vec![10.0.into(), 10.0.into(), 110.0.into(), 40.0.into()]),
        "Contents" => Object::string_literal("Foreign annotation"),
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"),
        "C" => Object::Array(vec![0.0.into(), 0.0.into(), 1.0.into()]),
        "CA" => 1.0,
        "F" => 4
    });
    let pages_id = document.new_object_id();
    let page_id = document.add_object(lopdf::dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => Object::Array(vec![0.0.into(), 0.0.into(), 612.0.into(), 792.0.into()]),
        "Resources" => lopdf::dictionary! {
            "Font" => lopdf::dictionary! { "F1" => font_id }
        },
        "Contents" => content_id,
        "Annots" => Object::Array(vec![annotation_id.into()])
    });
    document.set_object(
        pages_id,
        lopdf::dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(vec![page_id.into()]),
            "Count" => 1
        },
    );
    let catalog_id = document.add_object(lopdf::dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id
    });
    document.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("write the foreign annotation fixture");
    synthetic::pdf_bytes_file("foreign-freetext.pdf", bytes)
}

fn page_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    let page_id = *document.get_pages().get(&page_number).expect("page number");
    let page = document
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    let annots = page.get(b"Annots").expect("page has /Annots");
    let array = match annots {
        Object::Reference(id) => document
            .get_object(*id)
            .expect("annots object")
            .as_array()
            .expect("/Annots array"),
        Object::Array(array) => array,
        other => panic!("/Annots is not an array: {other:?}"),
    };
    array
        .iter()
        .map(|object| match object {
            Object::Reference(id) => document
                .get_object(*id)
                .expect("annotation object")
                .as_dict()
                .expect("annotation dictionary")
                .clone(),
            Object::Dictionary(dictionary) => dictionary.clone(),
            other => panic!("annotation is not a dictionary: {other:?}"),
        })
        .collect()
}

fn free_text_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    page_annotations(document, page_number)
        .into_iter()
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .ok()
                .and_then(|object| object.as_name().ok())
                .is_some_and(|name| name == b"FreeText")
        })
        .collect()
}

fn find_free_text(document: &lopdf::Document, page_number: u32) -> lopdf::Dictionary {
    let mut annotations = free_text_annotations(document, page_number);
    assert_eq!(
        annotations.len(),
        1,
        "expected exactly one /FreeText annotation, got {}",
        annotations.len()
    );
    annotations.remove(0)
}

fn text_of(object: &Object) -> String {
    match object {
        Object::String(bytes, _) if bytes.starts_with(&[0xFE, 0xFF]) => {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
        other => panic!("not a string: {other:?}"),
    }
}

fn string_of(annotation: &lopdf::Dictionary, key: &[u8]) -> String {
    text_of(
        annotation
            .get(key)
            .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key))),
    )
}

fn number(object: &Object) -> f32 {
    object
        .as_float()
        .or_else(|_| object.as_i64().map(|value| value as f32))
        .expect("numeric object")
}

fn components(annotation: &lopdf::Dictionary, key: &[u8]) -> Vec<f32> {
    annotation
        .get(key)
        .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key)))
        .as_array()
        .unwrap_or_else(|_| panic!("{:?} is not an array", String::from_utf8_lossy(key)))
        .iter()
        .map(number)
        .collect()
}

fn scalar(annotation: &lopdf::Dictionary, key: &[u8]) -> f32 {
    number(
        annotation
            .get(key)
            .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key))),
    )
}

fn integer(annotation: &lopdf::Dictionary, key: &[u8]) -> i64 {
    annotation
        .get(key)
        .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key)))
        .as_i64()
        .unwrap_or_else(|_| panic!("{:?} is not an integer", String::from_utf8_lossy(key)))
}

fn name_of(annotation: &lopdf::Dictionary, key: &[u8]) -> Vec<u8> {
    annotation
        .get(key)
        .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key)))
        .as_name()
        .unwrap_or_else(|_| panic!("{:?} is not a name", String::from_utf8_lossy(key)))
        .to_vec()
}

fn normal_appearance<'a>(
    document: &'a lopdf::Document,
    annotation: &'a lopdf::Dictionary,
) -> &'a lopdf::Stream {
    let appearance = annotation
        .get(b"AP")
        .expect("/AP appearance dictionary")
        .as_dict()
        .expect("appearance dictionary");
    let normal = match appearance.get(b"N").expect("normal appearance") {
        Object::Reference(id) => document.get_object(*id).expect("appearance stream"),
        object => object,
    };
    normal
        .as_stream()
        .expect("the normal appearance is a stream")
}

fn appearance_content(document: &lopdf::Document, annotation: &lopdf::Dictionary) -> String {
    String::from_utf8_lossy(
        &normal_appearance(document, annotation)
            .decompressed_content()
            .expect("appearance stream content"),
    )
    .into_owned()
}

fn assert_no_need_appearances(document: &lopdf::Document) {
    let catalog = document.catalog().expect("document catalog");
    assert!(
        catalog.get(b"NeedAppearances").is_err(),
        "the catalog must not request /NeedAppearances"
    );
    if let Ok(acro_form) = catalog.get_deref(b"AcroForm", document) {
        if let Ok(dictionary) = acro_form.as_dict() {
            assert!(
                dictionary.get(b"NeedAppearances").is_err(),
                "the AcroForm must not request /NeedAppearances"
            );
        }
    }
}

fn assert_no_comment_thread(annotation: &lopdf::Dictionary) {
    for key in [b"Popup".as_slice(), b"Parent", b"IRT", b"CL"] {
        assert!(
            annotation.get(key).is_err(),
            "a callout box must not carry /{}: {annotation:?}",
            String::from_utf8_lossy(key)
        );
    }
}

struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl DecodedPng {
    fn pixel(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let offset = ((y * self.width + x) * 4) as usize;
        (
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
        )
    }

    fn count_matching(
        &self,
        region: (u32, u32, u32, u32),
        predicate: impl Fn((u8, u8, u8)) -> bool,
    ) -> usize {
        let (left, top, right, bottom) = region;
        let mut count = 0;
        for y in top..bottom.min(self.height) {
            for x in left..right.min(self.width) {
                if predicate(self.pixel(x, y)) {
                    count += 1;
                }
            }
        }
        count
    }

    fn bounds_matching(
        &self,
        region: (u32, u32, u32, u32),
        predicate: impl Fn((u8, u8, u8)) -> bool,
    ) -> Option<(u32, u32, u32, u32)> {
        let (left, top, right, bottom) = region;
        let mut bounds: Option<(u32, u32, u32, u32)> = None;
        for y in top..bottom.min(self.height) {
            for x in left..right.min(self.width) {
                if predicate(self.pixel(x, y)) {
                    bounds = Some(match bounds {
                        None => (x, y, x + 1, y + 1),
                        Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(x + 1), b.max(y + 1)),
                    });
                }
            }
        }
        bounds
    }
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload is base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buffer).expect("PNG frame");
    buffer.truncate(info.buffer_size());
    DecodedPng {
        width: info.width,
        height: info.height,
        rgba: buffer,
    }
}

fn region(rect: TextMarkupRect, scale: f32) -> (u32, u32, u32, u32) {
    (
        (rect.x * scale) as u32,
        (rect.y * scale) as u32,
        ((rect.x + rect.width) * scale) as u32,
        ((rect.y + rect.height) * scale) as u32,
    )
}

fn assert_close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 0.01,
        "{label}: expected {expected}, got {actual}"
    );
}

fn assert_rect_close(actual: TextMarkupRect, expected: TextMarkupRect) {
    assert_close(actual.x, expected.x, "rect.x");
    assert_close(actual.y, expected.y, "rect.y");
    assert_close(actual.width, expected.width, "rect.width");
    assert_close(actual.height, expected.height, "rect.height");
}

#[test]
fn free_text_saves_as_a_native_freetext_annotation_with_contents_da_q_ap_and_pixels() {
    let dir = TempDir::new("free-text-native");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_rect = rect(100.0, 200.0, 240.0, 80.0);

    let mutation = create_free_text_with_author(
        document_id.clone(),
        "注释作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            box_rect,
            "#c62828",
            70,
            "Hello 世界",
            16.0,
            FreeTextAlign::Right,
        ),
    )
    .expect("create free text");
    let created = mutation.annotation.expect("created annotation view");
    assert_eq!(created.kind, AnnotationKind::FreeText);
    assert_eq!(created.text.as_deref(), Some("Hello 世界"));
    assert_eq!(created.color, "#c62828");
    assert_eq!(created.opacity, 70);
    assert_eq!(
        created.free_text,
        Some(FreeTextView {
            kind: FreeTextKind::FreeText,
            font_size: 16.0,
            align: FreeTextAlign::Right,
        })
    );
    assert_eq!(created.rects, vec![box_rect]);
    assert_eq!(created.author.as_deref(), Some("注释作者"));
    assert!(mutation.history.dirty);

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, AnnotationKind::FreeText);
    assert_eq!(listed[0].free_text, created.free_text);

    let output = dir.path().join("free-text.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    assert_eq!(string_of(&annotation, b"Contents"), "Hello 世界");
    assert_eq!(string_of(&annotation, b"T"), "注释作者");
    assert_eq!(string_of(&annotation, b"NM"), created.name);
    assert_eq!(integer(&annotation, b"Q"), 2, "/Q right");
    assert_eq!(name_of(&annotation, b"IT"), b"FreeText".to_vec());
    let da = string_of(&annotation, b"DA");
    assert!(
        da.contains("NotoSansSC"),
        "/DA names the bundled font: {da}"
    );
    assert!(da.contains("16.0000 Tf"), "/DA carries the font size: {da}");
    let colour = components(&annotation, b"C");
    assert!((colour[0] - 198.0 / 255.0).abs() < 0.01, "C {colour:?}");
    assert!((colour[1] - 40.0 / 255.0).abs() < 0.01, "C {colour:?}");
    assert!((colour[2] - 40.0 / 255.0).abs() < 0.01, "C {colour:?}");
    assert!((scalar(&annotation, b"CA") - 0.7).abs() < 0.01);
    assert_eq!(string_of(&annotation, b"PDFXFreeTextKind"), "freetext");
    assert_eq!(string_of(&annotation, b"PDFXFreeTextFontSize"), "16.0000");
    assert_eq!(string_of(&annotation, b"PDFXFreeTextAlign"), "right");
    let appearance = appearance_content(&parsed, &annotation);
    assert!(
        appearance.ends_with("f\nQ\n"),
        "the appearance fills glyph outlines: {appearance}"
    );
    assert!(
        !appearance.contains("Tj") && !appearance.contains("Tf"),
        "the appearance uses vector outlines, not text operators: {appearance}"
    );
    assert_no_need_appearances(&parsed);

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views.len(), 1);
    let view = &views[0];
    assert_eq!(view.kind, AnnotationKind::FreeText);
    assert_eq!(view.text.as_deref(), Some("Hello 世界"));
    assert_eq!(view.color, "#c62828");
    assert_eq!(view.opacity, 70);
    assert_eq!(view.author.as_deref(), Some("注释作者"));
    assert_eq!(view.created_at, created.created_at);
    assert_eq!(
        view.free_text,
        Some(FreeTextView {
            kind: FreeTextKind::FreeText,
            font_size: 16.0,
            align: FreeTextAlign::Right,
        })
    );
    assert_rect_close(view.rects[0], box_rect);

    let pixels = render_pixels(&reopened.document_id, 0, 2.0);
    let red = |pixel: (u8, u8, u8)| {
        pixel.0 > 150
            && i32::from(pixel.0) - i32::from(pixel.1) > 60
            && i32::from(pixel.0) - i32::from(pixel.2) > 60
    };
    let painted = pixels.count_matching(region(box_rect, 2.0), red);
    assert!(
        painted > 40,
        "the red text should paint inside its box, got {painted} red pixels"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn callout_is_a_native_freetext_annotation_but_not_a_comment_thread() {
    let dir = TempDir::new("free-text-callout");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_rect = rect(120.0, 220.0, 200.0, 80.0);

    let created = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::Callout,
            box_rect,
            "#cc0000",
            100,
            "注释框",
            18.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create callout")
    .annotation
    .expect("created annotation view");
    assert_eq!(created.kind, AnnotationKind::Callout);
    assert_eq!(
        created.free_text.expect("free text view").kind,
        FreeTextKind::Callout
    );

    let output = dir.path().join("callout.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    assert_eq!(name_of(&annotation, b"IT"), b"FreeTextCallout".to_vec());
    assert_eq!(string_of(&annotation, b"PDFXFreeTextKind"), "callout");
    let border = components(&annotation, b"Border");
    assert!((border[2] - 1.5).abs() < 0.01, "Border {border:?}");
    assert_no_comment_thread(&annotation);
    let appearance = appearance_content(&parsed, &annotation);
    assert!(
        appearance.contains(" re f"),
        "the callout paints its box: {appearance}"
    );
    assert!(
        appearance.contains(" re S"),
        "the callout draws a border: {appearance}"
    );
    assert_no_need_appearances(&parsed);

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].kind, AnnotationKind::Callout);
    assert_eq!(views[0].text.as_deref(), Some("注释框"));

    let pixels = render_pixels(&reopened.document_id, 0, 2.0);
    // The bottom-right of the box is away from the top-left text and from the
    // 1.5pt border, so it shows the callout tint (#cc0000 at 8%).
    let tinted =
        |pixel: (u8, u8, u8)| pixel.0 > 235 && i32::from(pixel.0) - i32::from(pixel.1).abs() > 8;
    let corner = (
        ((box_rect.x + box_rect.width - 8.0) * 2.0) as u32,
        ((box_rect.y + box_rect.height - 8.0) * 2.0) as u32,
    );
    let corner_pixel = pixels.pixel(corner.0, corner.1);
    assert!(
        tinted(corner_pixel),
        "the callout box should be tinted, got {corner_pixel:?}"
    );
    let text_pixels = pixels.count_matching(region(box_rect, 2.0), |pixel| {
        pixel.0 > 140 && pixel.1 < 90 && pixel.2 < 90
    });
    assert!(
        text_pixels > 20,
        "the callout text should paint, got {text_pixels}"
    );

    // Deleting and undoing a callout is one annotation, not a thread.
    delete_annotation(reopened.document_id.clone(), views[0].name.clone())
        .expect("delete the callout");
    assert!(list_annotations(reopened.document_id.clone())
        .expect("list after delete")
        .is_empty());
    undo_text_edit(reopened.document_id.clone()).expect("undo the callout deletion");
    let restored = list_annotations(reopened.document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, views[0].name);
    assert_eq!(restored[0].kind, AnnotationKind::Callout);
    assert_eq!(restored[0].free_text, views[0].free_text);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn editing_free_text_text_and_style_is_undoable_and_saves_the_new_alignment() {
    let dir = TempDir::new("free-text-update");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_rect = rect(100.0, 200.0, 260.0, 90.0);

    let created = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            box_rect,
            "#111827",
            100,
            "First",
            12.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create free text")
    .annotation
    .expect("created view");

    let updated = update_free_text(
        document_id.clone(),
        pdfx_lib::annotations::UpdateFreeTextRequest {
            name: created.name.clone(),
            text: "Second 行".to_owned(),
            font_size: 24.0,
            align: FreeTextAlign::Center,
            color: "#0055aa".to_owned(),
            opacity: 60,
        },
    )
    .expect("edit free text")
    .annotation
    .expect("updated view");
    assert_eq!(updated.name, created.name);
    assert_eq!(updated.text.as_deref(), Some("Second 行"));
    assert_eq!(updated.color, "#0055aa");
    assert_eq!(updated.opacity, 60);
    assert_eq!(
        updated.free_text,
        Some(FreeTextView {
            kind: FreeTextKind::FreeText,
            font_size: 24.0,
            align: FreeTextAlign::Center,
        })
    );
    assert_eq!(updated.author, created.author);
    assert_eq!(updated.created_at, created.created_at);

    undo_text_edit(document_id.clone()).expect("undo the edit");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, created.name);
    assert_eq!(restored[0].text.as_deref(), Some("First"));
    assert_eq!(restored[0].color, "#111827");
    assert_eq!(restored[0].opacity, 100);
    assert_eq!(
        restored[0].free_text,
        Some(FreeTextView {
            kind: FreeTextKind::FreeText,
            font_size: 12.0,
            align: FreeTextAlign::Left,
        })
    );

    redo_text_edit(document_id.clone()).expect("redo the edit");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone[0].text.as_deref(), Some("Second 行"));
    assert_eq!(
        redone[0].free_text.expect("free text").align,
        FreeTextAlign::Center
    );

    let output = dir.path().join("updated.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    assert_eq!(string_of(&annotation, b"Contents"), "Second 行");
    assert_eq!(integer(&annotation, b"Q"), 1, "/Q centre");
    let da = string_of(&annotation, b"DA");
    assert!(da.contains("24.0000 Tf"), "/DA follows the new size: {da}");
    assert_eq!(string_of(&annotation, b"PDFXFreeTextAlign"), "center");
    assert_no_need_appearances(&parsed);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views[0].text.as_deref(), Some("Second 行"));
    assert_eq!(views[0].color, "#0055aa");
    assert_eq!(views[0].opacity, 60);
    assert_eq!(
        views[0].free_text,
        Some(FreeTextView {
            kind: FreeTextKind::FreeText,
            font_size: 24.0,
            align: FreeTextAlign::Center,
        })
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn moving_and_resizing_free_text_rewraps_and_is_undoable() {
    let dir = TempDir::new("free-text-transform");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let original = rect(100.0, 200.0, 220.0, 70.0);
    let moved = rect(150.0, 320.0, 140.0, 110.0);

    let created = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            original,
            "#111827",
            100,
            "移动后重新换行的一段文字",
            18.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create free text")
    .annotation
    .expect("created view");

    let transformed =
        transform_free_text_annotation(document_id.clone(), created.name.clone(), moved)
            .expect("move and resize")
            .annotation
            .expect("transformed view");
    assert_eq!(transformed.name, created.name);
    assert_eq!(transformed.text, created.text);
    assert_eq!(transformed.free_text, created.free_text);
    assert_rect_close(transformed.rects[0], moved);

    undo_text_edit(document_id.clone()).expect("undo the transform");
    let after_undo = list_annotations(document_id.clone()).expect("list after undo");
    assert_rect_close(after_undo[0].rects[0], original);
    assert_eq!(after_undo[0].text, created.text);

    redo_text_edit(document_id.clone()).expect("redo the transform");
    let after_redo = list_annotations(document_id.clone()).expect("list after redo");
    assert_rect_close(after_redo[0].rects[0], moved);

    let output = dir.path().join("moved.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views.len(), 1);
    assert_rect_close(views[0].rects[0], moved);
    assert_eq!(views[0].text, created.text);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn cjk_free_text_renders_vector_glyphs_and_wraps_without_font_resources() {
    let dir = TempDir::new("free-text-cjk");
    let pdf = synthetic::cjk_mixed_pdf();
    let document_id = open_synthetic(&pdf);
    let box_rect = rect(100.0, 200.0, 180.0, 140.0);
    let text = "中文注释框自由文本自动换行验证";

    let created = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            box_rect,
            "#202020",
            100,
            text,
            20.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create CJK free text")
    .annotation
    .expect("created view");
    assert_eq!(created.text.as_deref(), Some(text));

    let output = dir.path().join("cjk.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    assert_eq!(string_of(&annotation, b"Contents"), text);
    let appearance = appearance_content(&parsed, &annotation);
    assert!(
        !appearance.contains("Tf") && !appearance.contains("Tj"),
        "CJK glyphs must be vector outlines with no font resource: {appearance}"
    );
    assert_no_need_appearances(&parsed);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views[0].text.as_deref(), Some(text));

    // 15 characters at 20pt in a 172pt text width wrap into at least two
    // lines. Count horizontal bands of dark pixels: a single unwrapped line
    // would produce one band, clipped text none.
    let pixels = render_pixels(&reopened.document_id, 0, 2.0);
    let dark = |pixel: (u8, u8, u8)| pixel.0 < 130 && pixel.1 < 130 && pixel.2 < 130;
    let (left, top, right, bottom) = region(box_rect, 2.0);
    let mut ink_rows = Vec::new();
    for y in top..bottom.min(pixels.height) {
        let mut painted = false;
        for x in left..right.min(pixels.width) {
            if dark(pixels.pixel(x, y)) {
                painted = true;
                break;
            }
        }
        ink_rows.push(painted);
    }
    let mut bands = 0;
    let mut in_band = false;
    let mut gap = 0;
    for painted in ink_rows {
        if painted {
            if !in_band {
                bands += 1;
                in_band = true;
            }
            gap = 0;
        } else if in_band {
            gap += 1;
            if gap >= 3 {
                in_band = false;
            }
        }
    }
    assert!(
        bands >= 2,
        "wrapped CJK text should produce at least two line bands, got {bands}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn free_text_on_a_rotated_cropped_page_stays_upright_and_round_trips_geometry() {
    let dir = TempDir::new("free-text-rotated");
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);
    // Page 0 is /Rotate 90 with CropBox [50 80 450 680], so the display page
    // is 600x400: the box below fits in display space.
    let box_rect = rect(100.0, 80.0, 240.0, 50.0);

    let created = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            box_rect,
            "#0033cc",
            100,
            "ROTATED",
            18.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create on a rotated page")
    .annotation
    .expect("created view");
    assert_rect_close(created.rects[0], box_rect);

    let output = dir.path().join("rotated.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    assert_eq!(string_of(&annotation, b"Contents"), "ROTATED");
    assert_no_need_appearances(&parsed);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views.len(), 1);
    assert_rect_close(views[0].rects[0], box_rect);

    // The renderer shows the rotated page. If the appearance were drawn in
    // unrotated user space the text would appear on its side, so assert the
    // painted ink is wider than it is tall.
    let pixels = render_pixels(&reopened.document_id, 0, 2.0);
    let blue = |pixel: (u8, u8, u8)| pixel.2 > 130 && pixel.0 < 120;
    let bounds = pixels
        .bounds_matching(region(box_rect, 2.0), blue)
        .expect("the rotated free text paints inside its display box");
    let width = bounds.2 - bounds.0;
    let height = bounds.3 - bounds.1;
    assert!(
        width > height * 2,
        "text on a rotated page should stay upright and horizontal, got {width}x{height}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn free_text_on_a_cropped_page_maps_the_display_box_into_pdf_space() {
    let dir = TempDir::new("free-text-crop");
    let pdf = synthetic::crop_offset_pdf();
    let document_id = open_synthetic(&pdf);
    // CropBox [100 100 500 700] makes the display page 400x600.
    let box_rect = rect(60.0, 80.0, 180.0, 60.0);

    create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            box_rect,
            "#006600",
            100,
            "CROP",
            18.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create on a cropped page");

    let output = dir.path().join("crop.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotation = find_free_text(&parsed, 1);
    // Display (60, 80) on a 400x600 cropped page maps to PDF user space
    // [160 560 340 620]: crop offset + (x, crop top - y - box height).
    let rect_values = components(&annotation, b"Rect");
    assert!(
        (rect_values[0] - 160.0).abs() < 0.51 && (rect_values[1] - 560.0).abs() < 0.51,
        "Rect {rect_values:?}"
    );
    assert!(
        (rect_values[2] - 340.0).abs() < 0.51 && (rect_values[3] - 620.0).abs() < 0.51,
        "Rect {rect_values:?}"
    );
    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_rect_close(views[0].rects[0], box_rect);

    let pixels = render_pixels(&reopened.document_id, 0, 2.0);
    let green = |pixel: (u8, u8, u8)| pixel.1 > 80 && pixel.0 < 100 && pixel.2 < 100;
    let painted = pixels.count_matching(region(box_rect, 2.0), green);
    assert!(
        painted > 20,
        "the cropped-page text should paint inside its display box, got {painted}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn repeat_saves_do_not_duplicate_freetext_annotations_or_require_appearances() {
    let dir = TempDir::new("free-text-repeat");
    let pdf = synthetic::single_line_pdf();
    let source_before = std::fs::read(pdf.path_string()).expect("read source bytes");
    let document_id = open_synthetic(&pdf);

    create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            rect(100.0, 200.0, 200.0, 60.0),
            "#111827",
            100,
            "One",
            14.0,
            FreeTextAlign::Left,
        ),
    )
    .expect("create free text");
    create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::Callout,
            rect(300.0, 300.0, 180.0, 70.0),
            "#cc0000",
            100,
            "Two",
            14.0,
            FreeTextAlign::Right,
        ),
    )
    .expect("create callout");

    let output = dir.path().join("repeat.pdf");
    save_as(&document_id, &output);
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 2, "two annotations, not four");
    let free_text = free_text_annotations(&parsed, 1);
    assert_eq!(free_text.len(), 2);
    let plain = free_text
        .iter()
        .find(|annotation| string_of(annotation, b"PDFXFreeTextKind") == "freetext")
        .expect("plain free text");
    let callout = free_text
        .iter()
        .find(|annotation| string_of(annotation, b"PDFXFreeTextKind") == "callout")
        .expect("callout");
    assert_eq!(integer(plain, b"Q"), 0);
    assert_eq!(name_of(plain, b"IT"), b"FreeText".to_vec());
    assert_eq!(integer(callout, b"Q"), 2);
    assert_eq!(name_of(callout, b"IT"), b"FreeTextCallout".to_vec());
    assert_no_need_appearances(&parsed);
    assert_no_comment_thread(callout);

    let source_after = std::fs::read(pdf.path_string()).expect("read source bytes again");
    assert_eq!(
        source_before, source_after,
        "the Source Document is untouched"
    );
    close_document(document_id).expect("close document");
}

#[test]
fn foreign_freetext_annotations_survive_saving_without_pdfx_keys_injected() {
    let dir = TempDir::new("free-text-foreign");
    let pdf = foreign_free_text_pdf();
    let document_id = open_synthetic(&pdf);

    create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            rect(200.0, 200.0, 180.0, 60.0),
            "#111827",
            100,
            "Own text",
            14.0,
            FreeTextAlign::Center,
        ),
    )
    .expect("create PDFX free text alongside the foreign annotation");

    let output = dir.path().join("foreign.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = free_text_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 2, "the foreign annotation is preserved");

    let foreign = annotations
        .iter()
        .find(|annotation| string_of(annotation, b"Contents") == "Foreign annotation")
        .expect("foreign annotation");
    assert!(foreign.get(b"Q").is_err(), "no /Q is invented for it");
    assert!(foreign.get(b"IT").is_err(), "no /IT is invented for it");
    assert!(
        foreign.get(b"PDFXFreeTextKind").is_err(),
        "no private keys are injected"
    );
    assert_eq!(string_of(foreign, b"DA"), "/Helv 10 Tf 0 g");

    let ours = annotations
        .iter()
        .find(|annotation| annotation.get(b"PDFXFreeTextKind").is_ok())
        .expect("PDFX annotation");
    assert_eq!(integer(ours, b"Q"), 1);
    assert_eq!(name_of(ours, b"IT"), b"FreeText".to_vec());

    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_free_text_changes_but_still_list_them() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;

    let create_error = create_free_text_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            FreeTextKind::FreeText,
            rect(10.0, 10.0, 100.0, 40.0),
            "#111827",
            100,
            "Denied",
            12.0,
            FreeTextAlign::Left,
        ),
    )
    .expect_err("free text creation is denied");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);

    let update_error = update_free_text(
        document_id.clone(),
        pdfx_lib::annotations::UpdateFreeTextRequest {
            name: "missing".to_owned(),
            text: "Denied".to_owned(),
            font_size: 12.0,
            align: FreeTextAlign::Left,
            color: "#111827".to_owned(),
            opacity: 100,
        },
    )
    .expect_err("free text updates are denied");
    assert_eq!(update_error.code, CommandErrorCode::PermissionDenied);

    let transform_error = transform_free_text_annotation(
        document_id.clone(),
        "missing".to_owned(),
        rect(10.0, 10.0, 100.0, 40.0),
    )
    .expect_err("free text transforms are denied");
    assert_eq!(transform_error.code, CommandErrorCode::PermissionDenied);

    let delete_error = delete_annotation(document_id.clone(), "missing".to_owned())
        .expect_err("free text deletion is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);

    assert!(
        list_annotations(document_id.clone())
            .expect("listing is allowed")
            .is_empty(),
        "listing stays available on restricted documents"
    );
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_free_text_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid = || {
        request(
            FreeTextKind::FreeText,
            rect(10.0, 10.0, 100.0, 40.0),
            "#111827",
            100,
            "Valid",
            12.0,
            FreeTextAlign::Left,
        )
    };
    let invalid = |label: &str, request: CreateFreeTextRequest| {
        let error = create_free_text_with_author(document_id.clone(), "作者".to_owned(), request)
            .expect_err(label);
        assert_eq!(error.code, CommandErrorCode::InvalidArgument, "{label}");
    };

    invalid("too many characters", {
        let mut request = valid();
        request.text = "a".repeat(5_001);
        request
    });
    invalid("control character", {
        let mut request = valid();
        request.text = "bad\u{0007}text".to_owned();
        request
    });
    let missing_glyph = create_free_text_with_author(document_id.clone(), "作者".to_owned(), {
        let mut request = valid();
        request.text = "\u{10FFFF}".to_owned();
        request
    })
    .expect_err("the bundled font cannot draw the text");
    assert_eq!(missing_glyph.code, CommandErrorCode::UnsupportedFont);
    invalid("font size below the minimum", {
        let mut request = valid();
        request.font_size = 5.0;
        request
    });
    invalid("font size above the maximum", {
        let mut request = valid();
        request.font_size = 73.0;
        request
    });
    invalid("NaN font size", {
        let mut request = valid();
        request.font_size = f32::NAN;
        request
    });
    invalid("box too narrow", {
        let mut request = valid();
        request.rect.width = 11.0;
        request
    });
    invalid("box too short", {
        let mut request = valid();
        request.rect.height = 11.0;
        request
    });
    invalid("invalid colour", {
        let mut request = valid();
        request.color = "blue".to_owned();
        request
    });
    invalid("opacity above 100", {
        let mut request = valid();
        request.opacity = 101;
        request
    });

    let page_error = create_free_text_with_author(document_id.clone(), "作者".to_owned(), {
        let mut request = valid();
        request.page_index = 7;
        request
    })
    .expect_err("out-of-range page must fail");
    assert_eq!(page_error.code, CommandErrorCode::PageOutOfRange);

    let author_error = create_free_text_with_author(document_id.clone(), "   ".to_owned(), valid())
        .expect_err("empty author must fail");
    assert_eq!(author_error.code, CommandErrorCode::InvalidArgument);

    let missing_update = update_free_text(
        document_id.clone(),
        pdfx_lib::annotations::UpdateFreeTextRequest {
            name: "missing".to_owned(),
            text: "Text".to_owned(),
            font_size: 12.0,
            align: FreeTextAlign::Left,
            color: "#111827".to_owned(),
            opacity: 100,
        },
    )
    .expect_err("unknown annotation");
    assert_eq!(missing_update.code, CommandErrorCode::InvalidArgument);

    let missing_transform =
        transform_free_text_annotation(document_id.clone(), "missing".to_owned(), valid().rect)
            .expect_err("unknown annotation");
    assert_eq!(missing_transform.code, CommandErrorCode::InvalidArgument);

    assert!(list_annotations(document_id.clone())
        .expect("list after rejected requests")
        .is_empty());
    let history = edit_history(document_id.clone()).expect("history after rejected requests");
    assert!(!history.dirty, "rejected requests must not create history");
    assert!(!history.can_undo);
    close_document(document_id).expect("close document");
}

#[test]
fn free_text_takes_its_author_from_settings_at_creation_time() {
    let dir = TempDir::new("free-text-author");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let store = SettingsStore::new(dir.path());
    let request = || {
        request(
            FreeTextKind::FreeText,
            rect(10.0, 10.0, 120.0, 40.0),
            "#111827",
            100,
            "Author",
            12.0,
            FreeTextAlign::Left,
        )
    };

    let mut settings = Settings {
        annotation_author: "Alice".to_owned(),
        ..Settings::default()
    };
    store.save(&settings).expect("persist settings");
    let alice = create_free_text_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Alice")
        .annotation
        .expect("Alice's view");
    assert_eq!(alice.author.as_deref(), Some("Alice"));

    settings.annotation_author = "Bob".to_owned();
    store.save(&settings).expect("persist changed author");
    let bob = create_free_text_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Bob")
        .annotation
        .expect("Bob's view");
    assert_eq!(bob.author.as_deref(), Some("Bob"));

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == alice.name)
            .and_then(|view| view.author.as_deref()),
        Some("Alice"),
        "changing the setting must not rewrite existing annotations"
    );
    let default_author =
        create_free_text_from_settings(document_id.clone(), &Settings::default(), request())
            .expect("create with the default author")
            .annotation
            .expect("default-author view")
            .author;
    assert_eq!(default_author, Some(default_annotation_author()));
    close_document(document_id).expect("close document");
}

#[test]
fn free_text_commands_reject_annotations_of_other_kinds() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let note = create_note_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateNoteRequest {
            page_index: 0,
            rect: rect(40.0, 40.0, 30.0, 30.0),
            color: "#ffd400".to_owned(),
            opacity: 100,
            text: "便签".to_owned(),
        },
    )
    .expect("create a note")
    .annotation
    .expect("created note");
    assert_eq!(note.kind, AnnotationKind::Note);

    let update_error = update_free_text(
        document_id.clone(),
        pdfx_lib::annotations::UpdateFreeTextRequest {
            name: note.name.clone(),
            text: "Not free text".to_owned(),
            font_size: 12.0,
            align: FreeTextAlign::Left,
            color: "#111827".to_owned(),
            opacity: 100,
        },
    )
    .expect_err("a note is not free text");
    assert_eq!(update_error.code, CommandErrorCode::InvalidArgument);

    let transform_error = transform_free_text_annotation(
        document_id.clone(),
        note.name.clone(),
        rect(50.0, 50.0, 60.0, 40.0),
    )
    .expect_err("a note cannot be transformed as free text");
    assert_eq!(transform_error.code, CommandErrorCode::InvalidArgument);

    // The failed commands left the note untouched and created no history.
    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, note.name);
    assert_eq!(listed[0].text.as_deref(), Some("便签"));
    let history = edit_history(document_id.clone()).expect("history");
    assert!(history.can_undo, "the note creation is undoable");
    undo_text_edit(document_id.clone()).expect("undo the note creation");
    assert!(
        !edit_history(document_id.clone())
            .expect("history after undo")
            .can_undo
    );
    close_document(document_id).expect("close document");
}
