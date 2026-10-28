//! Command-boundary acceptance tests for signature images.
//!
//! A signature is inserted as the same kind of PDFium page image object as a
//! photo (ticket 14), but with a signature label/kind and a signature-sized
//! default rectangle. These tests drive the Rust command functions the Tauri
//! IPC layer exposes; saved output is reopened through the command boundary and
//! parsed independently with lopdf.

mod support;

use base64::Engine as _;
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::image_edit::{
    delete_image, insert_image, insert_signature_image, page_images, transform_image, ImageKind,
    ImageTransformRequest,
};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::{fixtures, synthetic, temp::TempDir};

const RED: [u8; 4] = [220, 30, 40, 255];
const BLUE: [u8; 4] = [20, 60, 200, 255];

fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        data.extend_from_slice(&color);
    }
    encode_png(width, height, &data)
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNG header");
        writer.write_image_data(rgba).expect("PNG data");
    }
    encoded
}

/// A transparent signature image: a red horizontal bar in the middle, alpha 0
/// everywhere else. The bar spans x 600..1000 and y 200..400 of 1600x600.
fn transparent_signature_png(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let left = width * 3 / 8;
    let right = width * 5 / 8;
    let top = height / 3;
    let bottom = height * 2 / 3;
    for y in top..bottom {
        for x in left..right {
            let offset = ((y * width + x) * 4) as usize;
            rgba[offset..offset + 4].copy_from_slice(&RED);
        }
    }
    encode_png(width, height, &rgba)
}

fn write_image(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write test image");
    path.to_string_lossy().into_owned()
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

fn is_reddish(colour: [u8; 4]) -> bool {
    colour[0] > 150 && colour[1] < 110 && colour[2] < 110
}

fn is_blueish(colour: [u8; 4]) -> bool {
    colour[2] > 120 && colour[0] < 120 && colour[1] < 120
}

fn is_page_white(colour: [u8; 4]) -> bool {
    colour[0] > 245 && colour[1] > 245 && colour[2] > 245
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
    decode_png_bytes(&bytes)
}

fn decode_png_bytes(bytes: &[u8]) -> DecodedPng {
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

fn image_xobject_ids(document: &lopdf::Document) -> Vec<lopdf::ObjectId> {
    document
        .objects
        .iter()
        .filter(|(_, object)| match object {
            lopdf::Object::Stream(stream) => {
                stream.dict.get(b"Subtype").ok() == Some(&lopdf::Object::Name(b"Image".to_vec()))
            }
            _ => false,
        })
        .map(|(id, _)| *id)
        .collect()
}

fn page_content(document: &lopdf::Document) -> String {
    let pages = document.get_pages();
    let id = *pages.values().next().expect("one page");
    String::from_utf8_lossy(&document.get_page_content(id)).into_owned()
}

#[test]
fn inserting_a_transparent_signature_uses_the_signature_kind_and_a_centered_layout() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let png =
        base64::engine::general_purpose::STANDARD.encode(transparent_signature_png(1600, 600));

    let result = insert_signature_image(document_id.clone(), 0, png, "手绘签名".to_owned())
        .expect("insert signature");

    assert_eq!(result.page_index, 0);
    let image = &result.image;
    assert_eq!(image.kind, ImageKind::Signature);
    assert_eq!(image.name, "手绘签名");
    assert_eq!(image.pixel_width, 1600);
    assert_eq!(image.pixel_height, 600);
    // Signatures use a smaller default rectangle than photos: 35% of the page
    // width, aspect-preserved, centered on the 612x792 page.
    assert_close(image.width, 214.2);
    assert_close(image.height, 80.325);
    assert_close(image.x, 198.9);
    assert_close(image.y, 355.8375);
    assert_close(image.rotation, 0.0);
    assert_eq!(result.history.revision, 1);
    assert!(result.history.can_undo);

    let list = page_images(document_id.clone(), 0).expect("list images");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, ImageKind::Signature);
    assert_eq!(list[0].name, "手绘签名");

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
    }
    matrix.expect("a saved image placement matrix")
}

fn signature_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(transparent_signature_png(1600, 600))
}

/// A valid 1x1 PNG whose IHDR declares dimensions above the pixel limit. The
/// header is read before any pixel data, so the limit check rejects it before
/// allocating a large buffer.
fn oversized_dimensions_png() -> Vec<u8> {
    let mut bytes = solid_png(1, 1, RED);
    bytes[16..20].copy_from_slice(&6_000u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&6_000u32.to_be_bytes());
    // Recompute the IHDR CRC so the header itself is valid.
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
fn page_image_exports_and_flattened_copies_include_the_signature() {
    use pdfx_lib::flatten_export::export_flattened_copy;
    use pdfx_lib::print_export::{export_page, PageImageFormat};

    let pdf = synthetic::single_line_pdf();
    let source_before = std::fs::read(pdf.path_string()).expect("read source");
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("signature-export");
    insert_signature_image(document_id.clone(), 0, signature_base64(), "签名".into())
        .expect("insert");

    // PNG and JPEG exports render through the page viewer pipeline.
    for (format, name) in [
        (PageImageFormat::Png, "signed-page.png"),
        (PageImageFormat::Jpg, "signed-page.jpg"),
    ] {
        let destination = dir.path().join(name);
        let exported = export_page(
            document_id.clone(),
            0,
            format,
            72,
            destination.to_string_lossy().into_owned(),
        )
        .expect("export page");
        assert_eq!(exported.width, 612);
        assert_eq!(exported.height, 792);
        let bytes = std::fs::read(&destination).expect("read export");
        assert_eq!(exported.bytes, bytes.len() as u64);
        let pixel = match format {
            PageImageFormat::Png => pixel_at(&decode_png_bytes(&bytes), 1.0, 306.0, 396.0),
            PageImageFormat::Jpg => {
                let decoded = image::load_from_memory(&bytes)
                    .expect("decode jpeg export")
                    .to_rgba8();
                let pixel = decoded.get_pixel(306, 396);
                [pixel[0], pixel[1], pixel[2], pixel[3]]
            }
        };
        assert!(
            is_reddish(pixel),
            "{name} should show the signature bar at the center, got {pixel:?}"
        );
    }

    // The flattened copy serializes the live document; the signature is
    // already page content, so it survives without being counted as an
    // annotation to flatten.
    let flattened_path = dir.path().join("signed-flat.pdf");
    let flattened = export_flattened_copy(
        document_id.clone(),
        flattened_path.to_string_lossy().into_owned(),
        false,
    )
    .expect("flatten");
    assert_eq!(flattened.flattened, 0);
    let reopened =
        open_document(flattened_path.to_string_lossy().into_owned(), None).expect("reopen flat");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&pixels, 1.0, 306.0, 396.0)),
        "the flattened copy should render the signature, got {:?}",
        pixel_at(&pixels, 1.0, 306.0, 396.0)
    );

    // Exporting never changes the edit history or the source document.
    let history = edit_history(document_id.clone()).expect("history");
    assert_eq!(history.revision, 1);
    assert_eq!(
        std::fs::read(pdf.path_string()).expect("source after export"),
        source_before
    );

    close_document(reopened.document_id).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn read_only_documents_reject_signature_insertion_and_transforms() {
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

    let error = insert_signature_image(document_id.clone(), 0, signature_base64(), "签名".into())
        .expect_err("read-only insert");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(
        error.message.to_lowercase().contains("does not allow"),
        "message should explain the restriction: {}",
        error.message
    );
    assert!(page_images(document_id.clone(), 0)
        .expect("read-only listing")
        .is_empty());

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

    let history = edit_history(document_id.clone()).expect("history");
    assert!(!history.can_undo && !history.can_redo && !history.dirty);

    close_document(document_id).expect("close");
}

#[test]
fn malformed_signature_payloads_are_rejected_before_the_document_changes() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid = signature_base64();
    let broken_png = {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 0]);
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let broken_jpeg = {
        let mut bytes = vec![0xff, 0xd8, 0xff];
        bytes.extend_from_slice(&[0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let oversized_bytes = "A".repeat(((25 * 1024 * 1024 + 2) / 3) * 4 + 1);
    let cases: Vec<(&str, String, String, CommandErrorCode)> = vec![
        (
            "empty payload",
            String::new(),
            "签名".into(),
            CommandErrorCode::InvalidArgument,
        ),
        (
            "not base64",
            "not base64!!".into(),
            "签名".into(),
            CommandErrorCode::ImageDecodeFailed,
        ),
        (
            "neither png nor jpeg",
            base64::engine::general_purpose::STANDARD.encode(b"hello"),
            "签名".into(),
            CommandErrorCode::UnsupportedImageFormat,
        ),
        (
            "truncated png",
            broken_png,
            "签名".into(),
            CommandErrorCode::ImageDecodeFailed,
        ),
        (
            "truncated jpeg",
            broken_jpeg,
            "签名".into(),
            CommandErrorCode::ImageDecodeFailed,
        ),
        (
            "too many pixels",
            base64::engine::general_purpose::STANDARD.encode(oversized_dimensions_png()),
            "签名".into(),
            CommandErrorCode::ImageTooLarge,
        ),
        (
            "payload too large",
            oversized_bytes,
            "签名".into(),
            CommandErrorCode::ImageTooLarge,
        ),
        (
            "blank name",
            valid.clone(),
            "   ".into(),
            CommandErrorCode::InvalidArgument,
        ),
        (
            "overlong name",
            valid.clone(),
            "名".repeat(81),
            CommandErrorCode::InvalidArgument,
        ),
        (
            "control character in name",
            valid.clone(),
            "签\n名".into(),
            CommandErrorCode::InvalidArgument,
        ),
    ];

    for (label, payload, name, expected) in cases {
        let error = insert_signature_image(document_id.clone(), 0, payload, name).expect_err(label);
        assert_eq!(error.code, expected, "{label}: {}", error.message);
        assert!(!error.message.is_empty(), "{label} needs a message");
        assert!(
            page_images(document_id.clone(), 0)
                .expect("page images")
                .is_empty(),
            "{label} must not add a signature"
        );
        let history = edit_history(document_id.clone()).expect("history");
        assert!(
            !history.can_undo && !history.can_redo && !history.dirty,
            "{label} must not change history"
        );
        let pixels = render_pixels(&document_id, 0, 1.0);
        assert!(is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)));
    }

    close_document(document_id).expect("close");
}

#[test]
fn a_signature_on_a_rotated_page_follows_the_display_geometry() {
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);

    let inserted = insert_signature_image(
        document_id.clone(),
        0,
        signature_base64(),
        "  手写签名  ".into(),
    )
    .expect("insert");
    assert_eq!(inserted.image.name, "手写签名");
    assert_eq!(inserted.image.kind, ImageKind::Signature);
    // Page 1 is rotated 90 degrees: its 400x600 CropBox displays as 600x400
    // points. 35% of 600 caps the width at 210 points, so the 1600x600 image
    // scales to 210x78.75 and is centered at (195, 160.625).
    assert_close(inserted.image.width, 210.0);
    assert_close(inserted.image.height, 78.75);
    assert_close(inserted.image.x, 195.0);
    assert_close(inserted.image.y, 160.625);

    for page_index in 0..3 {
        let pixels = render_pixels(&document_id, page_index, 1.0);
        let colour = pixel_at(&pixels, 1.0, 300.0, 200.0);
        if page_index == 0 {
            assert!(
                is_reddish(colour),
                "page {page_index} should show the signature, got {colour:?}"
            );
        } else {
            assert!(
                is_page_white(colour),
                "page {page_index} must stay unchanged, got {colour:?}"
            );
        }
    }

    close_document(document_id).expect("close");
}

#[test]
fn a_transformed_signature_saves_its_position_size_and_rotation() {
    use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};

    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("signature-transform-save");

    let inserted =
        insert_signature_image(document_id.clone(), 0, signature_base64(), "签名".into())
            .expect("insert");
    let transformed = transform_image(
        document_id.clone(),
        0,
        inserted.image.image_id,
        ImageTransformRequest {
            x: 100.0,
            y: 200.0,
            width: 150.0,
            height: 60.0,
            rotation: 20.0,
        },
    )
    .expect("transform");
    assert_eq!(transformed.image.kind, ImageKind::Signature);
    assert_close(transformed.image.x, 100.0);
    assert_close(transformed.image.rotation, 20.0);

    let output = dir.path().join("transformed-signature.pdf");
    save_document_with_progress(
        &document_id,
        SaveDocumentRequest {
            save_as_path: Some(output.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save");

    // The saved content places a 1600x600 image with the matrix implied by
    // the transformed layout, rotated 20 degrees clockwise around (175, 230).
    let bytes = std::fs::read(&output).expect("read output");
    let parsed = lopdf::Document::load_mem(&bytes).expect("parse output");
    let pages = parsed.get_pages();
    let page_id = pages[&1];
    let page_images = parsed.get_page_images(page_id).expect("page images");
    assert_eq!(page_images.len(), 1, "one signature colour image");
    assert_eq!((page_images[0].width, page_images[0].height), (1600, 600));

    let matrix = last_image_matrix(&parsed.get_page_content(page_id));
    let cos = 20f32.to_radians().cos();
    let sin = 20f32.to_radians().sin();
    let rotate = |x: f32, y: f32| {
        (
            175.0 + (x - 175.0) * cos - (y - 230.0) * sin,
            230.0 + (x - 175.0) * sin + (y - 230.0) * cos,
        )
    };
    let (bl_x, bl_y) = rotate(100.0, 260.0);
    let (br_x, br_y) = rotate(250.0, 260.0);
    let (tl_x, tl_y) = rotate(100.0, 200.0);
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

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&pixels, 1.0, 175.0, 230.0)),
        "the transformed signature should render at its center, got {:?}",
        pixel_at(&pixels, 1.0, 175.0, 230.0)
    );
    assert!(is_page_white(pixel_at(&pixels, 1.0, 500.0, 600.0)));

    close_document(reopened.document_id).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn signature_insert_transform_and_delete_share_one_undo_history() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let inserted =
        insert_signature_image(document_id.clone(), 0, signature_base64(), "签名".into())
            .expect("insert");
    let image_id = inserted.image.image_id;
    assert_eq!(inserted.history.revision, 1);
    assert!(inserted.history.can_undo);
    assert!(inserted.history.dirty);

    let transformed = transform_image(
        document_id.clone(),
        0,
        image_id,
        ImageTransformRequest {
            x: 100.0,
            y: 200.0,
            width: 150.0,
            height: 60.0,
            rotation: 10.0,
        },
    )
    .expect("transform");
    assert_eq!(transformed.history.revision, 2);

    let deleted = delete_image(document_id.clone(), 0, image_id).expect("delete");
    assert_eq!(deleted.history.revision, 3);
    assert!(page_images(document_id.clone(), 0)
        .expect("list after delete")
        .is_empty());
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_page_white(pixel_at(&pixels, 1.0, 175.0, 230.0)));

    // Undoing walks the same history backwards: the delete, then the
    // transform, then the insertion.
    let undone = undo_text_edit(document_id.clone()).expect("undo delete");
    assert_eq!(undone.history.revision, 2);
    let list = page_images(document_id.clone(), 0).expect("list after undo delete");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].image_id, image_id);
    assert_eq!(list[0].kind, ImageKind::Signature);
    assert_close(list[0].x, 100.0);
    assert_close(list[0].rotation, 10.0);

    let undone = undo_text_edit(document_id.clone()).expect("undo transform");
    assert_eq!(undone.history.revision, 1);
    let list = page_images(document_id.clone(), 0).expect("list after undo transform");
    assert_close(list[0].x, 198.9);
    assert_close(list[0].width, 214.2);
    assert_close(list[0].rotation, 0.0);

    let undone = undo_text_edit(document_id.clone()).expect("undo insert");
    assert_eq!(undone.history.revision, 0);
    assert!(!undone.history.can_undo);
    assert!(!undone.history.dirty);
    assert!(page_images(document_id.clone(), 0)
        .expect("list after undo insert")
        .is_empty());
    let pixels = render_pixels(&document_id, 0, 1.0);
    assert!(is_page_white(pixel_at(&pixels, 1.0, 306.0, 396.0)));

    // Redo replays all three as one unified history.
    for expected_revision in 1..=3 {
        let redone = redo_text_edit(document_id.clone()).expect("redo");
        assert_eq!(redone.history.revision, expected_revision);
    }
    assert!(page_images(document_id.clone(), 0)
        .expect("list after redo delete")
        .is_empty());

    close_document(document_id).expect("close");
}

#[test]
fn a_transparent_signature_keeps_its_alpha_through_save_and_reopen() {
    use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};

    let pdf = synthetic::single_line_pdf();
    let source_before = std::fs::read(pdf.path_string()).expect("read source");
    let source_mtime = std::fs::metadata(pdf.path_string())
        .and_then(|metadata| metadata.modified())
        .expect("source mtime");
    let document_id = open_synthetic(&pdf);
    let dir = TempDir::new("signature-alpha");

    // An opaque blue photo covers the page first. The signature is drawn on
    // top; where its alpha is zero the blue must show through.
    let blue_path = write_image(&dir, "background.png", &solid_png(612, 792, BLUE));
    insert_image(document_id.clone(), 0, blue_path).expect("insert background");
    let png =
        base64::engine::general_purpose::STANDARD.encode(transparent_signature_png(1600, 600));
    let signature = insert_signature_image(document_id.clone(), 0, png, "签名".to_owned())
        .expect("insert signature");
    assert_eq!(signature.image.kind, ImageKind::Signature);

    // Live rendering: the red bar is opaque, the rest of the signature box
    // shows the blue photo behind it.
    let live = render_pixels(&document_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&live, 1.0, 306.0, 396.0)),
        "the red stroke should be opaque, got {:?}",
        pixel_at(&live, 1.0, 306.0, 396.0)
    );
    assert!(
        is_blueish(pixel_at(&live, 1.0, 210.0, 396.0)),
        "transparent signature pixels must show the blue page content, got {:?}",
        pixel_at(&live, 1.0, 210.0, 396.0)
    );

    let output = dir.path().join("signed.pdf");
    let request = |path: Option<String>| SaveDocumentRequest {
        save_as_path: path,
        confirm_unencrypted_output: false,
        confirm_signature_invalidation: false,
        password: None,
    };
    save_document_with_progress(
        &document_id,
        request(Some(output.to_string_lossy().into_owned())),
        &mut |_| {},
    )
    .expect("save");

    // The source document is never touched, bytes or timestamp.
    assert_eq!(
        std::fs::read(pdf.path_string()).expect("read source after save"),
        source_before
    );
    assert_eq!(
        std::fs::metadata(pdf.path_string())
            .and_then(|metadata| metadata.modified())
            .expect("source mtime after save"),
        source_mtime
    );

    // Independently parse the saved structure: two page images (the photo and
    // the signature colour image), each with its own alpha mask. PDFium writes
    // a mask for any BGRA bitmap, including the opaque photo; what matters is
    // that there is no duplicate colour image.
    let bytes = std::fs::read(&output).expect("read output");
    let parsed = lopdf::Document::load_mem(&bytes).expect("parse output");
    let pages = parsed.get_pages();
    let page_images = parsed.get_page_images(pages[&1]).expect("page images");
    assert_eq!(page_images.len(), 2, "photo and signature expected");
    let image_ids = image_xobject_ids(&parsed);
    assert_eq!(
        image_ids.len(),
        4,
        "two colour images and their two masks: {image_ids:?}"
    );
    let masked: Vec<_> = image_ids
        .iter()
        .filter(|id| {
            matches!(
                parsed.get_object(**id),
                Ok(lopdf::Object::Stream(stream)) if stream.dict.get(b"SMask").is_ok()
            )
        })
        .collect();
    assert_eq!(masked.len(), 2, "each colour image carries its mask");
    let masked_widths: Vec<_> = masked
        .iter()
        .filter_map(|id| match parsed.get_object(**id) {
            Ok(lopdf::Object::Stream(stream)) => stream.dict.get(b"Width").ok().cloned(),
            _ => None,
        })
        .collect();
    assert!(
        masked_widths.contains(&lopdf::Object::Integer(1600)),
        "the 1600px signature colour image is masked: {masked_widths:?}"
    );

    // A repeated save stays stable: no duplicate image object or content.
    save_document_with_progress(&document_id, request(None), &mut |_| {}).expect("save changes");
    let repeated = lopdf::Document::load(&output).expect("load repeated save");
    assert_eq!(repeated.objects.len(), parsed.objects.len());
    assert_eq!(page_content(&repeated), page_content(&parsed));
    assert_eq!(image_xobject_ids(&repeated).len(), 4);

    // Reopening through PDFium reproduces the same visible pixels, which is
    // only possible if the saved stream carries transparency.
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let saved = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        is_reddish(pixel_at(&saved, 1.0, 306.0, 396.0)),
        "saved signature stroke, got {:?}",
        pixel_at(&saved, 1.0, 306.0, 396.0)
    );
    assert!(
        is_blueish(pixel_at(&saved, 1.0, 210.0, 396.0)),
        "saved transparent area shows the blue photo, got {:?}",
        pixel_at(&saved, 1.0, 210.0, 396.0)
    );

    close_document(reopened.document_id).expect("close reopened");
    close_document(document_id).expect("close");
}
