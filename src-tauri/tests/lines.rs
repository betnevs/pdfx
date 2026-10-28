//! Command-boundary acceptance tests for straight lines and arrows.
//!
//! Lines and arrows are page *content* objects (path objects inside the page
//! content stream), not annotations. Every assertion goes through the same
//! Rust functions the Tauri IPC layer exposes; saved output is parsed with
//! lopdf and reopened through PDFium so "the line is real page content" and
//! "the line is not in the annotation list" are checked against a fresh parse.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::list_annotations;
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::line_edit::{
    create_line_object, delete_line_object, page_line_objects, transform_line_object,
    update_line_style, CreateLineRequest, LineGeometry, LineKind, LineStyle, LineTransformRequest,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::edit_history;
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn line_geometry(x1: f32, y1: f32, x2: f32, y2: f32) -> LineGeometry {
    LineGeometry { x1, y1, x2, y2 }
}

fn line_style(stroke: &str, opacity: u8, line_width: f32) -> LineStyle {
    LineStyle {
        stroke_color: stroke.to_owned(),
        opacity,
        line_width,
    }
}

fn line_request(
    page_index: u32,
    kind: LineKind,
    geometry: LineGeometry,
    style: LineStyle,
) -> CreateLineRequest {
    CreateLineRequest {
        page_index,
        kind,
        geometry,
        style,
    }
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

struct DecodedPng {
    width: u32,
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
        rgba: buffer,
    }
}

fn is_red(pixel: (u8, u8, u8)) -> bool {
    pixel.0 > 150 && pixel.1 < 110 && pixel.2 < 110
}

fn is_white(pixel: (u8, u8, u8)) -> bool {
    pixel.0 > 245 && pixel.1 > 245 && pixel.2 > 245
}

/// Every annotation dictionary on a page, whether /Annots is inline or a
/// reference.
fn page_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    let page_id = *document.get_pages().get(&page_number).expect("page number");
    let page = document
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
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

fn page_content(document: &lopdf::Document, page_number: u32) -> String {
    let page_id = *document.get_pages().get(&page_number).expect("page number");
    String::from_utf8_lossy(&document.get_page_content(page_id)).into_owned()
}

fn assert_not_a_line_annotation(document: &lopdf::Document) {
    for annotation in page_annotations(document, 1) {
        let subtype = annotation
            .get(b"Subtype")
            .ok()
            .and_then(|object| object.as_name().ok());
        assert_ne!(
            subtype,
            Some(b"Line".as_slice()),
            "a PDFX line must be page content, never a /Line annotation: {annotation:?}"
        );
    }
}

#[test]
fn a_straight_line_is_page_content_and_never_an_annotation() {
    let dir = TempDir::new("lines-content");
    let pdf = synthetic::single_line_pdf();
    let source_before = std::fs::read(pdf.path_string()).expect("read source bytes");
    let document_id = open_synthetic(&pdf);

    let request = line_request(
        0,
        LineKind::Line,
        line_geometry(100.0, 300.0, 400.0, 300.0),
        line_style("#cc0000", 100, 6.0),
    );
    let mutation = create_line_object(document_id.clone(), request.clone()).expect("create line");
    assert_eq!(mutation.page_index, 0);
    assert_eq!(mutation.line.kind, LineKind::Line);
    assert_eq!(mutation.line.geometry, request.geometry);
    assert_eq!(mutation.line.style, request.style);
    assert!(mutation.history.dirty);
    assert_eq!(mutation.history.revision, 1);

    let listed = page_line_objects(document_id.clone(), 0).expect("list lines");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].line_id, mutation.line.line_id);

    assert!(
        list_annotations(document_id.clone())
            .expect("list annotations")
            .is_empty(),
        "a line is page content, so the Annotation list must stay empty"
    );

    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(
        is_red(pixels.pixel(250, 300)),
        "the line should paint at its midpoint, got {:?}",
        pixels.pixel(250, 300)
    );
    assert!(
        is_white(pixels.pixel(250, 360)),
        "off the line the page stays white, got {:?}",
        pixels.pixel(250, 360)
    );

    let output = dir.path().join("line.pdf");
    save_as(&document_id, &output);

    // The Source Document is never modified (ADR-0004).
    assert_eq!(
        std::fs::read(pdf.path_string()).expect("read source after save"),
        source_before
    );

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_not_a_line_annotation(&parsed);
    let content = page_content(&parsed, 1);
    assert!(
        content.contains(" m") && content.contains(" l") && content.contains(" S"),
        "the saved content stream must stroke a real path, got {content:?}"
    );
    // Display (100, 300) -> (400, 300) on a 792pt-tall page is PDF user space
    // (100, 492) -> (400, 492).
    assert!(
        content.contains("100 492") && content.contains("400 492"),
        "the saved path must keep the display-space endpoints, got {content:?}"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    assert!(
        list_annotations(reopened.document_id.clone())
            .expect("list reopened annotations")
            .is_empty(),
        "reopening must not turn the line into an annotation"
    );
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        is_red(pixels.pixel(250, 300)),
        "the saved line should render at its midpoint after reopen, got {:?}",
        pixels.pixel(250, 300)
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn an_arrow_saves_the_shaft_and_a_filled_head_in_the_content_stream() {
    let dir = TempDir::new("lines-arrow");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let created = create_line_object(
        document_id.clone(),
        line_request(
            0,
            LineKind::Arrow,
            line_geometry(100.0, 300.0, 400.0, 300.0),
            line_style("#0000cc", 100, 4.0),
        ),
    )
    .expect("create an arrow")
    .line;
    assert_eq!(created.kind, LineKind::Arrow);

    // The head is a filled triangle at the end. With a 4pt stroke its length
    // is 16pt and its half-width 5.6pt, so (392, 303) is inside the head and
    // (392, 312) is outside it.
    let pixels = render_pixels(&document_id, 0, 1.0);
    let is_blue = |pixel: (u8, u8, u8)| pixel.2 > 130 && pixel.0 < 110;
    assert!(is_blue(pixels.pixel(250, 300)), "the shaft is stroked");
    assert!(
        is_blue(pixels.pixel(392, 303)),
        "the arrow head is filled, got {:?}",
        pixels.pixel(392, 303)
    );
    assert!(
        is_white(pixels.pixel(392, 312)),
        "the head stays narrow, got {:?}",
        pixels.pixel(392, 312)
    );

    let output = dir.path().join("arrow.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_not_a_line_annotation(&parsed);
    let content = page_content(&parsed, 1);
    // A line-only object emits `S`; an arrow also fills its head, which shows
    // up as a fill+stroke painting operator in the content stream.
    assert!(
        content.contains(" B") || content.contains(" S\n") && content.contains(" f"),
        "the arrow must fill its head as well as stroke the shaft, got {content:?}"
    );
    let move_count = content.matches(" m").count();
    assert!(
        move_count >= 2,
        "the shaft and the head are separate sub-paths, got {content:?}"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(is_blue(pixels.pixel(250, 300)));
    assert!(
        is_blue(pixels.pixel(392, 303)),
        "the saved arrow head survives reopen, got {:?}",
        pixels.pixel(392, 303)
    );
    assert!(is_white(pixels.pixel(392, 312)));
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn rotating_restyling_and_deleting_a_line_are_undoable() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let original_geometry = line_geometry(100.0, 300.0, 300.0, 300.0);
    let original_style = line_style("#cc0000", 100, 6.0);
    let created = create_line_object(
        document_id.clone(),
        line_request(0, LineKind::Line, original_geometry, original_style.clone()),
    )
    .expect("create a line")
    .line;
    assert_eq!(
        edit_history(document_id.clone()).expect("history").revision,
        1
    );

    // A 90-degree rotation around the midpoint (200, 300) is stored as real
    // endpoint geometry, not as overlay chrome.
    let rotated_geometry = line_geometry(200.0, 200.0, 200.0, 400.0);
    let rotated = transform_line_object(
        document_id.clone(),
        0,
        created.line_id,
        LineTransformRequest {
            geometry: rotated_geometry,
        },
    )
    .expect("rotate the line");
    assert_eq!(rotated.line.geometry, rotated_geometry);
    assert_eq!(rotated.line.kind, LineKind::Line);
    assert_eq!(rotated.history.revision, 2);

    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_red(pixels.pixel(200, 220)), "the rotated line paints");
    assert!(
        is_white(pixels.pixel(150, 300)),
        "the old horizontal footprint must be gone after the real rotation"
    );

    let restyled = pdfx_lib::line_edit::update_line_style(
        document_id.clone(),
        0,
        created.line_id,
        line_style("#00aa00", 50, 10.0),
    )
    .expect("restyle the line")
    .line;
    assert_eq!(restyled.style, line_style("#00aa00", 50, 10.0));
    assert_eq!(restyled.geometry, rotated_geometry);
    let pixels = render_pixels(&document_id, 0, 1.0);
    let green = pixels.pixel(200, 220);
    assert!(
        green.1 > green.0 + 40 && green.1 > green.2 + 40,
        "a 50% green stroke blends to green, got {green:?}"
    );

    // Undo restyle: colour returns, the rotation stays.
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo restyle");
    let listed = page_line_objects(document_id.clone(), 0).expect("list after style undo");
    assert_eq!(listed[0].style, original_style);
    assert_eq!(listed[0].geometry, rotated_geometry);
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_red(pixels.pixel(200, 220)));

    // Undo transform: the original endpoints return.
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo transform");
    let listed = page_line_objects(document_id.clone(), 0).expect("list after transform undo");
    assert_eq!(listed[0].geometry, original_geometry);
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_red(pixels.pixel(150, 300)));
    assert!(is_white(pixels.pixel(200, 220)));

    // Undo creation: the page object is gone and the revision is back to 0.
    let undone = pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo creation");
    assert_eq!(undone.history.revision, 0);
    assert!(!undone.history.dirty);
    assert!(page_line_objects(document_id.clone(), 0)
        .expect("list after undo creation")
        .is_empty());
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_white(pixels.pixel(150, 300)));

    // Redo creation, transform and restyle in order.
    pdfx_lib::text_edit::redo_text_edit(document_id.clone()).expect("redo creation");
    let redone = page_line_objects(document_id.clone(), 0).expect("list after redo creation");
    assert_eq!(redone[0].line_id, created.line_id, "redo restores the id");
    assert_eq!(redone[0].geometry, original_geometry);
    pdfx_lib::text_edit::redo_text_edit(document_id.clone()).expect("redo transform");
    let redone = page_line_objects(document_id.clone(), 0).expect("list after redo transform");
    assert_eq!(redone[0].geometry, rotated_geometry);
    pdfx_lib::text_edit::redo_text_edit(document_id.clone()).expect("redo restyle");
    let redone = page_line_objects(document_id.clone(), 0).expect("list after redo restyle");
    assert_eq!(redone[0].style, line_style("#00aa00", 50, 10.0));

    // Deletion is a content edit with undo, not an annotation delete.
    pdfx_lib::line_edit::delete_line_object(document_id.clone(), 0, created.line_id)
        .expect("delete the line");
    assert!(page_line_objects(document_id.clone(), 0)
        .expect("list after delete")
        .is_empty());
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo delete");
    let restored = page_line_objects(document_id.clone(), 0).expect("list after undo delete");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].line_id, created.line_id);
    assert_eq!(restored[0].geometry, rotated_geometry);
    assert_eq!(restored[0].style, line_style("#00aa00", 50, 10.0));
    pdfx_lib::text_edit::redo_text_edit(document_id.clone()).expect("redo delete");
    assert!(page_line_objects(document_id.clone(), 0)
        .expect("list after redo delete")
        .is_empty());

    close_document(document_id).expect("close document");
}

/// A solid 40x20 red PNG, enough for an image page object that shares the
/// page-object list with lines and Text Boxes.
fn solid_png(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        data.extend_from_slice(&[220, 30, 40, 255]);
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNG header");
        writer.write_image_data(&data).expect("PNG data");
    }
    encoded
}

#[test]
fn line_actions_keep_text_box_and_image_object_ranges_in_sync() {
    use pdfx_lib::image_edit::{delete_image, insert_image};
    use pdfx_lib::text_edit::{apply_text_edit, create_text_box, CreateTextBoxRequest};

    let dir = TempDir::new("lines-object-sync");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let image_path = dir.path().join("photo.png");
    std::fs::write(&image_path, solid_png(40, 20)).expect("write PNG");

    let image = insert_image(
        document_id.clone(),
        0,
        image_path.to_string_lossy().into_owned(),
    )
    .expect("insert an image");
    let line = create_line_object(
        document_id.clone(),
        line_request(
            0,
            LineKind::Line,
            line_geometry(100.0, 100.0, 300.0, 100.0),
            line_style("#cc0000", 100, 4.0),
        ),
    )
    .expect("create a line")
    .line;
    let box_view = create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 100.0,
            y: 600.0,
            width: 200.0,
            height: 40.0,
            style: None,
        },
    )
    .expect("create a text box");
    apply_text_edit(
        document_id.clone(),
        0,
        box_view.target.box_id,
        "Before the shifts".to_owned(),
    )
    .expect("type");

    // Deleting the image removes a page object before both the line and the
    // Text Box, so their stored object indices must move down.
    delete_image(document_id.clone(), 0, image.image.image_id).expect("delete the image");
    transform_line_object(
        document_id.clone(),
        0,
        line.line_id,
        LineTransformRequest {
            geometry: line_geometry(120.0, 120.0, 320.0, 160.0),
        },
    )
    .expect("the line still resolves after the index shift");
    apply_text_edit(
        document_id.clone(),
        0,
        box_view.target.box_id,
        "After the shifts".to_owned(),
    )
    .expect("the box still resolves after the index shift");

    // Undoing the later edits inserts the image object back and shifts every
    // later range up again; both tools must still address their own objects.
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo text edit");
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo line transform");
    pdfx_lib::text_edit::undo_text_edit(document_id.clone()).expect("undo image deletion");
    transform_line_object(
        document_id.clone(),
        0,
        line.line_id,
        LineTransformRequest {
            geometry: line_geometry(140.0, 140.0, 340.0, 180.0),
        },
    )
    .expect("the line resolves after the image is restored");
    apply_text_edit(
        document_id.clone(),
        0,
        box_view.target.box_id,
        "Restored".to_owned(),
    )
    .expect("the box resolves after the image is restored");

    let text = pdfx_lib::commands::extract_text(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("Restored"), "text: {text:?}");
    assert!(text.contains("Original line."), "text: {text:?}");
    let lines = page_line_objects(document_id.clone(), 0).expect("list lines");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].line_id, line.line_id);
    let images = pdfx_lib::image_edit::page_images(document_id.clone(), 0).expect("list images");
    assert_eq!(images.len(), 1);

    close_document(document_id).expect("close document");
}

#[test]
fn saved_lines_use_display_geometry_on_rotated_and_cropped_pages() {
    let dir = TempDir::new("lines-rotated");
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);

    // The three pages share a 400x600 CropBox and are rotated 90, 180 and 270
    // degrees, so their display sizes are 600x400, 400x600 and 600x400.
    let lines = [
        (line_geometry(100.0, 100.0, 300.0, 100.0), 200u32, 100u32),
        (line_geometry(100.0, 150.0, 300.0, 150.0), 200u32, 150u32),
        (line_geometry(100.0, 200.0, 300.0, 200.0), 200u32, 200u32),
    ];
    let mut ids = Vec::new();
    for (page_index, (geometry, _, _)) in lines.iter().enumerate() {
        let created = create_line_object(
            document_id.clone(),
            line_request(
                page_index as u32,
                LineKind::Line,
                *geometry,
                line_style("#cc0000", 100, 6.0),
            ),
        )
        .expect("create a line on a rotated page");
        ids.push(created.line.line_id);
        let pixels = render_pixels(&document_id, page_index as u32, 1.0);
        assert!(
            is_red(pixels.pixel(lines[page_index].1, lines[page_index].2)),
            "page {page_index} must show the line at its display-space midpoint"
        );
    }

    let output = dir.path().join("rotated-lines.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_not_a_line_annotation(&parsed);
    for page_number in 1..=3 {
        let content = page_content(&parsed, page_number);
        assert!(
            content.contains(" m") && content.contains(" l") && content.contains(" S"),
            "page {page_number} must contain the stroked path, got {content:?}"
        );
    }

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    for (page_index, (_, x, y)) in lines.iter().enumerate() {
        let pixels = render_pixels(&reopened.document_id, page_index as u32, 1.0);
        assert!(
            is_red(pixels.pixel(*x, *y)),
            "the saved line must survive reopen on page {page_index} at its display position"
        );
        assert!(
            is_white(pixels.pixel(*x, y + 100)),
            "off the line page {page_index} stays white"
        );
    }
    assert!(
        list_annotations(reopened.document_id.clone())
            .expect("list reopened annotations")
            .is_empty(),
        "rotated pages must not turn lines into annotations"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn repeat_saves_do_not_duplicate_the_line_content() {
    let dir = TempDir::new("lines-repeat-save");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_line_object(
        document_id.clone(),
        line_request(
            0,
            LineKind::Line,
            line_geometry(100.0, 300.0, 300.0, 300.0),
            line_style("#cc0000", 100, 6.0),
        ),
    )
    .expect("create a line")
    .line;

    let output = dir.path().join("repeat.pdf");
    save_as(&document_id, &output);
    let first = std::fs::read(&output).expect("read first output");
    let first = lopdf::Document::load_mem(&first).expect("parse first output");
    let first_content = page_content(&first, 1);
    assert_eq!(
        first_content.matches("300 492 l").count(),
        1,
        "the first save writes exactly one line segment: {first_content:?}"
    );
    assert_eq!(
        first_content.matches(" S").count(),
        1,
        "the first save writes exactly one stroked path: {first_content:?}"
    );

    // Save Changes on the same open document must rewrite the page content
    // without appending a second copy of the line.
    save_as(&document_id, &output);
    let second = std::fs::read(&output).expect("read second output");
    let second = lopdf::Document::load_mem(&second).expect("parse second output");
    let second_content = page_content(&second, 1);
    assert_eq!(
        second_content.matches("300 492 l").count(),
        1,
        "a repeated save must not duplicate the line: {second_content:?}"
    );

    // Moving the line then saving leaves exactly one path, at the new place.
    transform_line_object(
        document_id.clone(),
        0,
        created.line_id,
        LineTransformRequest {
            geometry: line_geometry(100.0, 400.0, 300.0, 400.0),
        },
    )
    .expect("move the line");
    save_as(&document_id, &output);
    let third = std::fs::read(&output).expect("read third output");
    let third = lopdf::Document::load_mem(&third).expect("parse third output");
    let third_content = page_content(&third, 1);
    assert_eq!(
        third_content.matches("300 392 l").count(),
        1,
        "exactly one segment at the new endpoints: {third_content:?}"
    );
    assert!(
        !third_content.contains("300 492 l"),
        "the old segment must not linger after the move: {third_content:?}"
    );
    assert_eq!(
        third_content.matches(" S").count(),
        1,
        "the moved line is still a single stroked path: {third_content:?}"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_line_changes_but_still_list_them() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;

    let create_error = create_line_object(
        document_id.clone(),
        line_request(
            0,
            LineKind::Arrow,
            line_geometry(10.0, 10.0, 100.0, 100.0),
            line_style("#cc0000", 100, 4.0),
        ),
    )
    .expect_err("content edits are denied on restricted documents");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);

    let transform_error = transform_line_object(
        document_id.clone(),
        0,
        1,
        LineTransformRequest {
            geometry: line_geometry(10.0, 10.0, 200.0, 200.0),
        },
    )
    .expect_err("transforms are denied");
    assert_eq!(transform_error.code, CommandErrorCode::PermissionDenied);

    let style_error = update_line_style(document_id.clone(), 0, 1, line_style("#00aa00", 50, 8.0))
        .expect_err("restyling is denied");
    assert_eq!(style_error.code, CommandErrorCode::PermissionDenied);

    let delete_error =
        delete_line_object(document_id.clone(), 0, 1).expect_err("deleting is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);

    assert!(
        page_line_objects(document_id.clone(), 0)
            .expect("listing page content stays available")
            .is_empty(),
        "no line objects exist on the untouched restricted document"
    );
    assert!(
        list_annotations(document_id.clone())
            .expect("listing annotations stays available")
            .is_empty(),
        "a line is never exposed through the annotation list"
    );
    let history = edit_history(document_id.clone()).expect("history on restricted document");
    assert!(
        !history.dirty,
        "denied requests must not dirty the document"
    );
    assert!(!history.can_undo);
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_line_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid = || {
        line_request(
            0,
            LineKind::Line,
            line_geometry(10.0, 10.0, 100.0, 100.0),
            line_style("#cc0000", 100, 4.0),
        )
    };
    let invalid = |label: &str, request: CreateLineRequest| {
        let error = create_line_object(document_id.clone(), request).expect_err(label);
        assert_eq!(error.code, CommandErrorCode::InvalidArgument, "{label}");
    };

    invalid("zero length", {
        let mut request = valid();
        request.geometry.x2 = request.geometry.x1;
        request.geometry.y2 = request.geometry.y1;
        request
    });
    invalid("NaN endpoint", {
        let mut request = valid();
        request.geometry.x1 = f32::NAN;
        request
    });
    invalid("infinite endpoint", {
        let mut request = valid();
        request.geometry.y2 = f32::INFINITY;
        request
    });
    invalid("coordinate too large", {
        let mut request = valid();
        request.geometry.x1 = 100_001.0;
        request
    });
    invalid("invalid colour", {
        let mut request = valid();
        request.style.stroke_color = "red".to_owned();
        request
    });
    invalid("opacity above 100", {
        let mut request = valid();
        request.style.opacity = 101;
        request
    });
    invalid("zero line width", {
        let mut request = valid();
        request.style.line_width = 0.0;
        request
    });
    invalid("line width above the maximum", {
        let mut request = valid();
        request.style.line_width = 101.0;
        request
    });

    let page_error = create_line_object(document_id.clone(), {
        let mut request = valid();
        request.page_index = 7;
        request
    })
    .expect_err("out-of-range page must fail");
    assert_eq!(page_error.code, CommandErrorCode::PageOutOfRange);

    let missing_transform = transform_line_object(
        document_id.clone(),
        0,
        999,
        LineTransformRequest {
            geometry: line_geometry(10.0, 10.0, 100.0, 100.0),
        },
    )
    .expect_err("unknown line");
    assert_eq!(missing_transform.code, CommandErrorCode::InvalidArgument);

    let missing_style =
        update_line_style(document_id.clone(), 0, 999, line_style("#00aa00", 50, 4.0))
            .expect_err("unknown line");
    assert_eq!(missing_style.code, CommandErrorCode::InvalidArgument);

    let missing_delete = delete_line_object(document_id.clone(), 0, 999).expect_err("unknown line");
    assert_eq!(missing_delete.code, CommandErrorCode::InvalidArgument);

    assert!(page_line_objects(document_id.clone(), 0)
        .expect("list after rejected requests")
        .is_empty());
    let history = edit_history(document_id.clone()).expect("history after rejected requests");
    assert!(
        !history.dirty,
        "rejected requests must not dirty the document"
    );
    assert!(!history.can_undo);
    assert!(!history.can_redo);
    close_document(document_id).expect("close document");
}
