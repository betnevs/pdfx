//! Command-boundary tests for the per-page text layer.
//!
//! These tests call the same Rust functions the Tauri IPC layer exposes. They
//! assert Unicode text plus display-space geometry; PDFium types never cross
//! the seam.

mod support;

use support::fixtures;
use support::synthetic;

use pdfx_lib::commands::TextLayerView;
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, render_page, CommandErrorCode,
};

fn open(category: &str) -> (String, String) {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, category).expect("fixture category");
    let path = fixtures::fixture_path(fixture);
    let view = open_document(path.to_string_lossy().into_owned(), None).expect("open fixture");
    (view.document_id, path.to_string_lossy().into_owned())
}

#[test]
fn text_page_returns_unicode_chars_with_display_space_boxes() {
    let (document_id, _) = open("text");

    let layer = extract_text_layer(document_id.clone(), 0).expect("extract text layer");

    assert_eq!(layer.page_index, 0);
    assert!(layer.has_text);
    assert!(layer.text.contains("PDFX fixture"), "got {:?}", layer.text);
    assert!(
        layer
            .text
            .contains("The quick brown fox jumps over the lazy dog."),
        "got {:?}",
        layer.text
    );
    assert_eq!(
        layer.text,
        layer
            .chars
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
    );

    let first = &layer.chars[0];
    assert_eq!(first.text, "P");
    // The fixture paints Helvetica 18pt at PDF user-space (72, 720) on a
    // 612x792 MediaBox. In display space (top-left origin) the loose glyph
    // box starts 56.29 points below the page top.
    assert!((first.x - 72.0).abs() < 0.01, "x was {}", first.x);
    assert!(
        (first.y - (792.0 - 736.29)).abs() < 0.01,
        "y was {}",
        first.y
    );
    assert!(
        (first.width - 12.006).abs() < 0.01,
        "width was {}",
        first.width
    );
    assert!(
        (first.font_size - 18.0).abs() < 0.01,
        "font size was {}",
        first.font_size
    );
    assert!(!first.generated);

    for character in &layer.chars {
        assert!(character.x >= -0.01 && character.x <= layer.page_width + 0.01);
        assert!(character.y >= -0.01 && character.y <= layer.page_height + 0.01);
        assert!(character.x + character.width <= layer.page_width + 0.01);
        assert!(character.y + character.height <= layer.page_height + 0.01);
    }

    close_document(document_id).expect("close document");
}

#[test]
fn unknown_document_and_out_of_range_pages_report_structured_errors() {
    let (document_id, _) = open("text");

    let error = extract_text_layer("doc-missing".to_owned(), 0).expect_err("unknown document");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);

    let error = extract_text_layer(document_id.clone(), 99).expect_err("page 100 does not exist");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    close_document(document_id).expect("close document");
}

/// Bounding box of all non-white pixels in a rendered page, in page points.
fn ink_bounds(document_id: &str, page_index: u32, scale: f32) -> (f32, f32, f32, f32) {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let pixels = decode_png(&image.png_base64);

    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        if pixel[0] >= 245 && pixel[1] >= 245 && pixel[2] >= 245 {
            continue;
        }
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    (left, top, right, bottom)
}

/// Bounding box of the layer's hit-testable character boxes, in page points.
fn char_bounds(layer: &TextLayerView) -> (f32, f32, f32, f32) {
    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for character in layer
        .chars
        .iter()
        .filter(|c| c.width > 0.0 && c.height > 0.0)
    {
        left = left.min(character.x);
        top = top.min(character.y);
        right = right.max(character.x + character.width);
        bottom = bottom.max(character.y + character.height);
    }
    (left, top, right, bottom)
}

fn assert_bounds_close(actual: (f32, f32, f32, f32), expected: (f32, f32, f32, f32)) {
    let tolerance = 6.0;
    for (name, actual, expected) in [
        ("left", actual.0, expected.0),
        ("top", actual.1, expected.1),
        ("right", actual.2, expected.2),
        ("bottom", actual.3, expected.3),
    ] {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{name} differed: text layer {actual} vs rendered ink {expected}"
        );
    }
}

#[test]
fn text_boxes_line_up_with_the_rendered_ink() {
    let (document_id, _) = open("text");

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let bar = char_bounds(&layer);
    let ink = ink_bounds(&document_id, 0, 1.0);
    assert_bounds_close(bar, ink);

    // The same must hold at other zoom levels; the layer is in page points, so
    // its box must not move when the bitmap does.
    let ink_at_two = ink_bounds(&document_id, 0, 2.0);
    assert_bounds_close(bar, ink_at_two);

    close_document(document_id).expect("close document");
}

#[test]
fn rotated_page_boxes_line_up_with_the_rendered_ink() {
    let (document_id, _) = open("rotated");

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    // Page 0 is a 612x792 MediaBox rotated 90 degrees, so it displays as
    // 792x612 with the text running down the right-hand side.
    assert_eq!(layer.rotation, 90);
    assert!((layer.page_width - 792.0).abs() < 0.01);
    assert!((layer.page_height - 612.0).abs() < 0.01);
    assert!(
        layer.text.contains("Rotated page 1 (rotate 90)"),
        "got {:?}",
        layer.text
    );

    let ink = ink_bounds(&document_id, 0, 1.0);
    assert_bounds_close(char_bounds(&layer), ink);

    close_document(document_id).expect("close document");
}

#[test]
fn cjk_and_latin_text_extracts_with_positions() {
    let pdf = synthetic::cjk_mixed_pdf();
    let document_id = open_synthetic(&pdf);

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    assert!(layer.has_text);
    assert!(layer.text.contains("中文"), "got {:?}", layer.text);
    assert!(layer.text.contains("PDF"), "got {:?}", layer.text);
    assert!(layer.text.contains("混合"), "got {:?}", layer.text);

    // Every layer character is a whole Unicode scalar, never half of a
    // surrogate pair.
    for character in &layer.chars {
        assert_eq!(
            character.text.chars().count(),
            1,
            "unexpected char {:?}",
            character.text
        );
    }

    let first = layer.chars.first().expect("first character");
    assert_eq!(first.text, "中");
    assert!(
        (first.origin_x - 72.0).abs() < 0.01,
        "origin_x {}",
        first.origin_x
    );
    assert!(
        (first.origin_y - 72.0).abs() < 0.01,
        "origin_y {}",
        first.origin_y
    );
    assert!(
        (first.font_size - 18.0).abs() < 0.01,
        "font size {}",
        first.font_size
    );
    assert!(first.width > 0.0 && first.height > 0.0);

    for character in &layer.chars {
        assert!(character.x >= -0.01 && character.x <= layer.page_width + 0.01);
        assert!(character.y >= -0.01 && character.y <= layer.page_height + 0.01);
    }

    assert_bounds_close(char_bounds(&layer), ink_bounds(&document_id, 0, 1.0));

    close_document(document_id).expect("close document");
}

#[test]
fn surrogate_pairs_stay_one_selectable_character() {
    let pdf = synthetic::surrogate_pair_pdf();
    let document_id = open_synthetic(&pdf);

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    assert!(layer.has_text);
    assert!(layer.text.starts_with('😀'), "got {:?}", layer.text);
    let character = layer.chars.first().expect("emoji character");
    assert_eq!(character.text, "😀");
    assert_eq!(character.text.chars().count(), 1);
    assert!((character.origin_x - 72.0).abs() < 0.01);
    assert!((character.origin_y - 72.0).abs() < 0.01);

    close_document(document_id).expect("close document");
}

#[test]
fn pages_without_a_text_layer_report_an_empty_layer() {
    let (document_id, _) = open("scanned");

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    assert!(!layer.has_text);
    assert!(layer.text.is_empty(), "scanned pages must not invent text");
    assert!(layer.chars.is_empty());
    assert!((layer.page_width - 612.0).abs() < 0.01);
    assert!((layer.page_height - 792.0).abs() < 0.01);

    close_document(document_id).expect("close document");
}

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

#[test]
fn offset_crop_box_maps_text_into_crop_relative_display_points() {
    let pdf = synthetic::crop_offset_pdf();
    let document_id = open_synthetic(&pdf);

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    assert_eq!(layer.rotation, 0);
    assert!(
        (layer.page_width - 400.0).abs() < 0.01,
        "width {}",
        layer.page_width
    );
    assert!(
        (layer.page_height - 600.0).abs() < 0.01,
        "height {}",
        layer.page_height
    );
    assert!((layer.crop_x - 100.0).abs() < 0.01);
    assert!((layer.crop_y - 100.0).abs() < 0.01);
    assert!(layer.text.contains("Crop offset"), "got {:?}", layer.text);

    // Text is drawn at user-space (150, 600). The crop box starts at (100,
    // 100) and is 600 points tall, so in display space the baseline sits at
    // (50, 100).
    let first = layer.chars.first().expect("first character");
    assert_eq!(first.text, "C");
    assert!(
        (first.origin_x - 50.0).abs() < 0.01,
        "origin_x {}",
        first.origin_x
    );
    assert!(
        (first.origin_y - 100.0).abs() < 0.01,
        "origin_y {}",
        first.origin_y
    );

    assert_bounds_close(char_bounds(&layer), ink_bounds(&document_id, 0, 1.0));

    close_document(document_id).expect("close document");
}

#[test]
fn rotated_crop_pages_map_origins_by_rotation() {
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);

    // User-space origin (100, 600) inside a [50 80 450 680] crop box gives
    // u=50, v=520 relative to the crop origin, and a 400x600 crop. Rotation
    // is applied to the crop-relative display image, so:
    //   90:  (v, u)          = (520, 50)
    //   180: (W-u, v)        = (350, 520)
    //   270: (H-v, W-u)      = (80, 350)
    let cases = [
        (90u16, (520.0, 50.0), (600.0, 400.0)),
        (180, (350.0, 520.0), (400.0, 600.0)),
        (270, (80.0, 350.0), (600.0, 400.0)),
    ];
    for (page_index, (rotation, (origin_x, origin_y), (width, height))) in
        cases.into_iter().enumerate()
    {
        let layer = extract_text_layer(document_id.clone(), page_index as u32).expect("text layer");

        assert_eq!(layer.rotation, rotation, "page {page_index}");
        assert!(
            (layer.page_width - width).abs() < 0.01,
            "page {page_index} width {}",
            layer.page_width
        );
        assert!(
            (layer.page_height - height).abs() < 0.01,
            "page {page_index} height {}",
            layer.page_height
        );

        let first = layer.chars.first().expect("first character");
        assert_eq!(first.text, "R");
        assert!(
            (first.origin_x - origin_x).abs() < 0.01,
            "page {page_index} origin_x {} expected {origin_x}",
            first.origin_x
        );
        assert!(
            (first.origin_y - origin_y).abs() < 0.01,
            "page {page_index} origin_y {} expected {origin_y}",
            first.origin_y
        );

        assert_bounds_close(
            char_bounds(&layer),
            ink_bounds(&document_id, page_index as u32, 1.0),
        );
    }

    close_document(document_id).expect("close document");
}

struct DecodedPng {
    width: u32,
    rgba: Vec<u8>,
}

fn decode_png(png_base64: &str) -> DecodedPng {
    use base64::Engine as _;

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
        rgba: buffer,
    }
}
