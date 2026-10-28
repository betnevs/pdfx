//! Command-boundary acceptance tests for image insertion on a Page.
//!
//! Every assertion goes through the Rust functions the Tauri IPC layer exposes.
//! Saved output is reopened through the same command boundary so "the image is
//! really in the document" is checked against a fresh PDFium parse.

mod support;

use base64::Engine as _;
use support::{fixtures, synthetic, temp::TempDir};

use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::image_edit::{
    delete_image, insert_image, page_images, transform_image, ImageTransformRequest,
};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};

/// Colours used by the generated images, chosen to be easy to tell apart in a
/// rendered page.
const RED: [u8; 4] = [220, 30, 40, 255];
const BLUE: [u8; 4] = [20, 60, 200, 255];

fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        data.extend_from_slice(&color);
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

fn solid_jpeg(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..width * height {
        data.extend_from_slice(&color);
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90);
        encoder
            .encode(&data, width, height, image::ExtendedColorType::Rgb8)
            .expect("JPEG data");
    }
    encoded
}

/// Writes image bytes into the test's temporary directory and returns a path
/// the command boundary accepts.
fn write_image(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write test image");
    path.to_string_lossy().into_owned()
}

struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_png(png_base64: &str) -> DecodedPng {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
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
        height: info.height,
        rgba: buffer,
    }
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    decode_png(&image.png_base64)
}

/// The colour of one page-space point in a rendered page.
fn pixel_at(pixels: &DecodedPng, scale: f32, x: f32, y: f32) -> [u8; 4] {
    let px = (x * scale).round() as u32;
    let py = (y * scale).round() as u32;
    let px = px.min(pixels.width - 1);
    let py = py.min(pixels.height - 1);
    let offset = ((py * pixels.width + px) * 4) as usize;
    [
        pixels.rgba[offset],
        pixels.rgba[offset + 1],
        pixels.rgba[offset + 2],
        pixels.rgba[offset + 3],
    ]
}

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.05,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn inserting_a_png_places_a_centered_aspect_fitted_image_on_the_current_page() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-insert");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));

    let result = insert_image(document_id.clone(), 0, path).expect("insert image");

    assert_eq!(result.page_index, 0);
    let image = &result.image;
    assert_eq!(image.name, "photo.png");
    assert_eq!(image.pixel_width, 200);
    assert_eq!(image.pixel_height, 100);
    // 200x100 pt fits inside 80% of the 612x792 pt page, so it is inserted at
    // its natural pixel size, centered.
    assert_close(image.width, 200.0);
    assert_close(image.height, 100.0);
    assert_close(image.x, 206.0);
    assert_close(image.y, 346.0);
    assert_close(image.rotation, 0.0);

    let list = page_images(document_id.clone(), 0).expect("list images");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].image_id, image.image_id);

    let history = edit_history(document_id.clone()).expect("history");
    assert!(history.can_undo);
    assert!(!history.can_redo);
    assert!(history.dirty);

    let pixels = render_pixels(&document_id, 0, 1.0);
    let colour = pixel_at(&pixels, 1.0, 306.0, 396.0);
    assert!(
        colour[0] > 180 && colour[1] < 90 && colour[2] < 90,
        "the inserted image should render at its default rectangle, got {colour:?}"
    );

    close_document(document_id).expect("close");
}

#[test]
fn a_large_image_is_scaled_down_to_fit_inside_the_page() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-large");
    let path = write_image(&dir, "wide.png", &solid_png(2000, 500, BLUE));

    let result = insert_image(document_id.clone(), 0, path).expect("insert image");

    let image = &result.image;
    // scale = min(0.8 * 612 / 2000, 0.8 * 792 / 500) = 0.2448.
    assert_close(image.width, 489.6);
    assert_close(image.height, 122.4);
    assert!(image.x >= 0.0 && image.y >= 0.0);
    assert!(image.x + image.width <= 612.0);
    assert!(image.y + image.height <= 792.0);

    close_document(document_id).expect("close");
}

#[test]
fn a_jpeg_is_accepted_and_placed_on_the_page() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-jpeg");
    let path = write_image(&dir, "picture.jpg", &solid_jpeg(160, 80, [20, 60, 200]));

    let result = insert_image(document_id.clone(), 0, path).expect("insert JPEG");

    assert_eq!(result.image.pixel_width, 160);
    assert_eq!(result.image.pixel_height, 80);
    let pixels = render_pixels(&document_id, 0, 1.0);
    let colour = pixel_at(&pixels, 1.0, 306.0, 396.0);
    assert!(
        colour[2] > 120 && colour[0] < 120,
        "the JPEG should render blue at its center, got {colour:?}"
    );

    close_document(document_id).expect("close");
}

fn is_reddish(colour: [u8; 4]) -> bool {
    colour[0] > 150 && colour[1] < 110 && colour[2] < 110
}

fn is_page_white(colour: [u8; 4]) -> bool {
    colour[0] > 245 && colour[1] > 245 && colour[2] > 245
}

#[test]
fn moving_resizing_and_rotating_an_image_is_one_undoable_action() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-transform");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    let image_id = inserted.image.image_id;

    let transformed = transform_image(
        document_id.clone(),
        0,
        image_id,
        ImageTransformRequest {
            x: 50.0,
            y: 60.0,
            width: 120.0,
            height: 80.0,
            rotation: 30.0,
        },
    )
    .expect("transform");

    assert_close(transformed.image.x, 50.0);
    assert_close(transformed.image.y, 60.0);
    assert_close(transformed.image.width, 120.0);
    assert_close(transformed.image.height, 80.0);
    assert_close(transformed.image.rotation, 30.0);
    assert_eq!(transformed.history.revision, 2);

    // Rotation is around the center, which stays at (110, 100).
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&pixels, 1.0, 110.0, 100.0)),
        "the transformed image should render at its new center"
    );

    let undone = undo_text_edit(document_id.clone()).expect("undo transform");
    assert!(undone.history.can_redo);
    assert_eq!(undone.history.revision, 1);
    let list = page_images(document_id.clone(), 0).expect("list after undo");
    assert_eq!(list.len(), 1);
    assert_close(list[0].x, 206.0);
    assert_close(list[0].y, 346.0);
    assert_close(list[0].width, 200.0);
    assert_close(list[0].height, 100.0);
    assert_close(list[0].rotation, 0.0);
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_reddish(pixel_at(&pixels, 1.0, 306.0, 396.0)));

    let redone = redo_text_edit(document_id.clone()).expect("redo transform");
    assert_eq!(redone.history.revision, 2);
    let list = page_images(document_id.clone(), 0).expect("list after redo");
    assert_close(list[0].x, 50.0);
    assert_close(list[0].rotation, 30.0);

    close_document(document_id).expect("close");
}

#[test]
fn undo_removes_a_new_image_and_redo_restores_it() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-undo-insert");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    let image_id = inserted.image.image_id;

    let undone = undo_text_edit(document_id.clone()).expect("undo insert");
    assert_eq!(undone.page_index, 0);
    assert!(!undone.history.can_undo);
    assert!(undone.history.can_redo);
    assert!(!undone.history.dirty);
    assert!(page_images(document_id.clone(), 0)
        .expect("list")
        .is_empty());
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(
        is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)),
        "undo should remove the image from the page"
    );

    let redone = redo_text_edit(document_id.clone()).expect("redo insert");
    assert!(redone.history.can_undo);
    assert!(!redone.history.can_redo);
    assert_eq!(redone.history.revision, 1);
    let list = page_images(document_id.clone(), 0).expect("list after redo");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].image_id, image_id);
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_reddish(pixel_at(&pixels, 1.0, 306.0, 396.0)));

    close_document(document_id).expect("close");
}

#[test]
fn deleting_an_image_removes_it_and_undo_restores_the_page() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-delete");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    let image_id = inserted.image.image_id;

    let deleted = delete_image(document_id.clone(), 0, image_id).expect("delete");
    assert_eq!(deleted.page_index, 0);
    assert!(deleted.history.can_undo);
    assert!(page_images(document_id.clone(), 0)
        .expect("list")
        .is_empty());
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)));

    let undone = undo_text_edit(document_id.clone()).expect("undo delete");
    assert!(undone.history.can_redo);
    let list = page_images(document_id.clone(), 0).expect("list after undo");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].image_id, image_id);
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_reddish(pixel_at(&pixels, 1.0, 306.0, 396.0)));

    let redone = redo_text_edit(document_id.clone()).expect("redo delete");
    assert!(redone.history.can_undo);
    assert!(page_images(document_id.clone(), 0)
        .expect("list")
        .is_empty());

    close_document(document_id).expect("close");
}

#[test]
fn image_actions_keep_text_box_object_ranges_in_sync() {
    use pdfx_lib::text_edit::{apply_text_edit, create_text_box, CreateTextBoxRequest};

    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-text-sync");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    let image_id = inserted.image.image_id;

    // The Text Box is appended after the image: its objects now sit after the
    // image's page object.
    let created = create_text_box(
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
    .expect("create box");
    apply_text_edit(
        document_id.clone(),
        0,
        created.target.box_id,
        "After the image".to_owned(),
    )
    .expect("apply text");

    // Deleting the image removes an object before the box, so the box's
    // stored start index must move down; undo moves it back. A stale index
    // would make the next text edit remove the wrong page objects.
    delete_image(document_id.clone(), 0, image_id).expect("delete image");
    undo_text_edit(document_id.clone()).expect("undo delete");
    apply_text_edit(
        document_id.clone(),
        0,
        created.target.box_id,
        "Still editable".to_owned(),
    )
    .expect("edit the box after the image moved");

    let text = pdfx_lib::commands::extract_text(document_id.clone(), 0)
        .expect("extract")
        .text;
    assert!(text.contains("Still editable"), "text: {text:?}");
    assert!(text.contains("Original line."), "text: {text:?}");
    let list = page_images(document_id.clone(), 0).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].image_id, image_id);

    close_document(document_id).expect("close");
}

/// Extracts the six operands of the `cm` operator that places the last image
/// drawn on the page. This reads the saved file, not the in-memory document.
fn last_image_matrix(content: &[u8]) -> [f32; 6] {
    let text = String::from_utf8_lossy(content);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut matrix = None;
    for (index, token) in tokens.iter().enumerate() {
        if *token == "cm" && index >= 6 {
            if let Ok(values) = tokens[index - 6..index]
                .iter()
                .map(|value| value.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
            {
                matrix = Some([
                    values[0], values[1], values[2], values[3], values[4], values[5],
                ]);
            }
        }
        if *token == "Do" && matrix.is_some() {
            // Keep the most recent `cm` before the image draw.
        }
    }
    matrix.expect("a saved image placement matrix")
}

#[test]
fn a_saved_image_reopens_with_its_position_size_and_rotation() {
    use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveStage};

    let pdf = synthetic::single_line_pdf();
    let source_before = std::fs::read(pdf.path_string()).expect("read source");
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-save");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    transform_image(
        document_id.clone(),
        0,
        inserted.image.image_id,
        ImageTransformRequest {
            x: 50.0,
            y: 60.0,
            width: 120.0,
            height: 80.0,
            rotation: 30.0,
        },
    )
    .expect("transform");

    let output = dir.path().join("output.pdf");
    let mut stages = Vec::new();
    let saved = save_document_with_progress(
        &document_id,
        SaveDocumentRequest {
            save_as_path: Some(output.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
        },
        &mut |progress| stages.push(progress.stage),
    )
    .expect("save");
    assert_eq!(stages.last(), Some(&SaveStage::Done));
    assert!(!saved.history.dirty);
    assert!(output.is_file());

    // The Source Document is never modified (ADR-0004).
    assert_eq!(
        std::fs::read(pdf.path_string()).expect("read source after save"),
        source_before
    );

    // An independent lopdf parse sees one image XObject with the PNG's pixel
    // dimensions on page 1, and the content stream places it with the matrix
    // the display-space layout implies.
    let bytes = std::fs::read(&output).expect("read output");
    let parsed = lopdf::Document::load_mem(&bytes).expect("parse output");
    let pages = parsed.get_pages();
    let page_id = pages[&1];
    let images = parsed.get_page_images(page_id).expect("page images");
    assert_eq!(images.len(), 1, "one image XObject expected");
    assert_eq!(images[0].width, 200);
    assert_eq!(images[0].height, 100);

    let matrix = last_image_matrix(&parsed.get_page_content(page_id));
    // Layout: rect (50, 60) size 120x80 rotated 30 degrees clockwise around
    // its center (110, 100) on a 612x792 page. Display-space corners are
    // rotated, then mapped to PDF user space with y = 792 - display_y.
    let cos = 30f32.to_radians().cos();
    let sin = 30f32.to_radians().sin();
    let rotate = |x: f32, y: f32| {
        (
            110.0 + (x - 110.0) * cos - (y - 100.0) * sin,
            100.0 + (x - 110.0) * sin + (y - 100.0) * cos,
        )
    };
    let (bl_x, bl_y) = rotate(50.0, 140.0);
    let (br_x, br_y) = rotate(170.0, 140.0);
    let (tl_x, tl_y) = rotate(50.0, 60.0);
    let expected = [
        br_x - bl_x,
        -(br_y - bl_y),
        tl_x - bl_x,
        -(tl_y - bl_y),
        bl_x,
        792.0 - bl_y,
    ];
    for (index, (actual, expected)) in matrix.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 0.2,
            "matrix[{index}] was {actual}, expected {expected} (matrix {matrix:?})"
        );
    }

    // Reopening through PDFium renders the image at the transformed center.
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    assert_eq!(reopened.page_count, 1);
    assert_eq!(reopened.page_width, 612.0);
    let pixels = render_pixels(&reopened_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&pixels, 1.0, 110.0, 100.0)),
        "the saved image should render at its transformed center"
    );
    // The image covers about 120x80 pt around the center; a point well outside
    // the rotated rectangle stays white.
    assert!(is_page_white(pixel_at(&pixels, 1.0, 30.0, 180.0)));

    close_document(reopened_id).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn a_saved_image_stays_on_its_page_and_rotated_pages_use_display_geometry() {
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-rotated");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));

    // Page 1 is rotated 90 degrees: its 400x600 CropBox displays as 600x400
    // points, and overlay placement must follow the display geometry.
    let inserted = insert_image(document_id.clone(), 0, path).expect("insert");
    assert_close(inserted.image.x, 200.0);
    assert_close(inserted.image.y, 150.0);
    assert_close(inserted.image.width, 200.0);
    assert_close(inserted.image.height, 100.0);
    for page_index in 0..3 {
        let pixels = render_pixels(&document_id, page_index, 1.0);
        let colour = pixel_at(&pixels, 1.0, 300.0, 200.0);
        if page_index == 0 {
            assert!(
                is_reddish(colour),
                "page {page_index} should show the inserted image, got {colour:?}"
            );
        } else {
            assert!(
                is_page_white(colour),
                "page {page_index} must stay unchanged, got {colour:?}"
            );
        }
    }

    // A Save As and an independent lopdf parse confirm the image XObject is
    // attached to page 1 only.
    let output = dir.path().join("rotated-output.pdf");
    pdfx_lib::save::save_document_with_progress(
        &document_id,
        pdfx_lib::save::SaveDocumentRequest {
            save_as_path: Some(output.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save");
    let bytes = std::fs::read(&output).expect("read output");
    let parsed = lopdf::Document::load_mem(&bytes).expect("parse output");
    let pages = parsed.get_pages();
    assert_eq!(parsed.get_page_images(pages[&1]).expect("page 1").len(), 1);
    assert!(parsed
        .get_page_images(pages[&2])
        .expect("page 2")
        .is_empty());
    assert!(parsed
        .get_page_images(pages[&3])
        .expect("page 3")
        .is_empty());

    close_document(document_id).expect("close");
}

#[test]
fn undo_after_save_removes_the_image_from_a_later_save() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-undo-save");
    let path = write_image(&dir, "photo.png", &solid_png(200, 100, RED));
    insert_image(document_id.clone(), 0, path).expect("insert");

    let output = dir.path().join("output.pdf");
    let request = |path: Option<String>| pdfx_lib::save::SaveDocumentRequest {
        save_as_path: path,
        confirm_unencrypted_output: false,
    };
    let first = pdfx_lib::save::save_document_with_progress(
        &document_id,
        request(Some(output.to_string_lossy().into_owned())),
        &mut |_| {},
    )
    .expect("first save");
    assert!(!first.history.dirty);

    let undone = undo_text_edit(document_id.clone()).expect("undo after save");
    assert!(undone.history.dirty);
    assert_eq!(undone.history.saved_revision, 1);
    assert_eq!(undone.history.revision, 0);

    let second =
        pdfx_lib::save::save_document_with_progress(&document_id, request(None), &mut |_| {})
            .expect("save changes after undo");
    assert!(!second.history.dirty);
    assert_eq!(second.history.revision, 0);

    let bytes = std::fs::read(&output).expect("read second output");
    let parsed = lopdf::Document::load_mem(&bytes).expect("parse second output");
    let pages = parsed.get_pages();
    assert!(
        parsed
            .get_page_images(pages[&1])
            .expect("page images")
            .is_empty(),
        "the undone image must not remain in the saved file"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None).expect("reopen");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)));
    close_document(reopened.document_id).expect("close reopened");

    close_document(document_id).expect("close");
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// A valid 1x1 PNG whose IHDR declares dimensions above the pixel limit. The
/// decoder reads the header (and its CRC) before decoding any pixel data, so
/// the limit check rejects it before allocating a buffer.
fn oversized_dimensions_png() -> Vec<u8> {
    let mut bytes = solid_png(1, 1, RED);
    // Signature (8) + IHDR length (4) + "IHDR" (4) + width (4) + height (4)
    // + bit depth/colour/compression/filter/interlace (5) + CRC (4).
    bytes[16..20].copy_from_slice(&6_000u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&6_000u32.to_be_bytes());
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in &bytes[12..29] {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    bytes[29..33].copy_from_slice(&(crc ^ 0xFFFF_FFFF).to_be_bytes());
    bytes
}

#[test]
fn invalid_image_files_are_rejected_without_changing_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("image-invalid");

    let mut cases: Vec<(&str, Vec<u8>, CommandErrorCode)> = vec![
        (
            "notes.txt",
            solid_png(10, 10, RED),
            CommandErrorCode::UnsupportedImageFormat,
        ),
        (
            "renamed.png",
            solid_jpeg(10, 10, [220, 30, 40]),
            CommandErrorCode::UnsupportedImageFormat,
        ),
        (
            "broken.png",
            {
                let mut bytes = PNG_MAGIC.to_vec();
                bytes.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 0]);
                bytes
            },
            CommandErrorCode::ImageDecodeFailed,
        ),
        (
            "broken.jpg",
            {
                let mut bytes = JPEG_MAGIC.to_vec();
                bytes.extend_from_slice(&[0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
                bytes
            },
            CommandErrorCode::ImageDecodeFailed,
        ),
        (
            "huge-dimensions.png",
            oversized_dimensions_png(),
            CommandErrorCode::ImageTooLarge,
        ),
    ];

    for (name, bytes, expected) in cases.drain(..) {
        let path = write_image(&dir, name, &bytes);
        let error = insert_image(document_id.clone(), 0, path).expect_err(name);
        assert_eq!(error.code, expected, "{name}: {}", error.message);
        assert!(!error.message.is_empty(), "{name} needs a message");
        assert!(
            page_images(document_id.clone(), 0)
                .expect("page images")
                .is_empty(),
            "{name} must not add an image"
        );
        let history = edit_history(document_id.clone()).expect("history");
        assert!(!history.can_undo && !history.can_redo && !history.dirty);
        let pixels = render_pixels(&document_id, 0, 1.0);
        assert!(is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)));
    }

    // A file above the byte limit is rejected from its metadata alone; a
    // sparse file keeps the test fixture tiny.
    let oversized = dir.path().join("oversized.png");
    let file = std::fs::File::create(&oversized).expect("create sparse file");
    file.set_len(25 * 1024 * 1024 + 1).expect("sparse length");
    drop(file);
    let error = insert_image(
        document_id.clone(),
        0,
        oversized.to_string_lossy().into_owned(),
    )
    .expect_err("oversized file");
    assert_eq!(error.code, CommandErrorCode::ImageTooLarge);
    assert!(page_images(document_id.clone(), 0)
        .expect("page images")
        .is_empty());
    let history = edit_history(document_id.clone()).expect("history");
    assert!(!history.dirty);

    close_document(document_id).expect("close");
}

#[test]
fn read_only_documents_reject_image_editing() {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == "encrypted-owner-restrictions")
        .expect("encrypted-owner-restrictions fixture");
    let path = fixtures::fixture_path(fixture);
    let document_id = open_document(path.to_string_lossy().into_owned(), None)
        .expect("open read-only fixture")
        .document_id;

    let dir = TempDir::new("image-read-only");
    let image = write_image(&dir, "photo.png", &solid_png(20, 10, RED));

    let error = insert_image(document_id.clone(), 0, image).expect_err("read-only insert");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(
        error.message.to_lowercase().contains("does not allow"),
        "message should explain the restriction: {}",
        error.message
    );

    let transform = ImageTransformRequest {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 10.0,
        rotation: 0.0,
    };
    let error =
        transform_image(document_id.clone(), 0, 1, transform).expect_err("read-only transform");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = delete_image(document_id.clone(), 0, 1).expect_err("read-only delete");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);

    // Listing images is a read and stays available.
    page_images(document_id.clone(), 0).expect("read-only listing");

    let history = edit_history(document_id.clone()).expect("history");
    assert!(!history.can_undo && !history.can_redo && !history.dirty);

    close_document(document_id).expect("close");
}
