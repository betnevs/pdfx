//! Command-boundary tests for navigation, zoom and thumbnail support.
//!
//! These call the same Rust functions the Tauri IPC layer exposes. They require
//! the pinned PDFium library (`npm run setup:native`).

mod support;

use support::fixtures;

use base64::Engine as _;
use pdfx_lib::commands::{
    close_document, open_document, page_info, render_page, render_page_region, render_thumbnail,
    CommandErrorCode,
};

struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_png(png_base64: &str) -> DecodedPng {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .expect("render payload should be base64");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "payload should be a PNG");

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

fn fixture_path(category: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, category).expect("fixture category");
    fixtures::fixture_path(fixture)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn page_info_reports_rotation_and_display_dimensions_for_mixed_pages() {
    let view = open_document(fixture_path("rotated"), None).expect("open rotated fixture");

    let expected = [
        (0, 90, 792.0, 612.0, 0.0, 0.0),
        (1, 0, 792.0, 612.0, 0.0, 0.0),
        (2, 270, 432.0, 288.0, 0.0, 0.0),
    ];

    for (index, rotation, width, height, crop_x, crop_y) in expected {
        let info = page_info(view.document_id.clone(), index).expect("read page info");
        assert_eq!(info.page_index, index);
        assert_eq!(info.rotation, rotation, "page {index} rotation");
        assert_eq!(info.width, width, "page {index} display width");
        assert_eq!(info.height, height, "page {index} display height");
        assert_eq!(info.crop_x, crop_x, "page {index} crop origin x");
        assert_eq!(info.crop_y, crop_y, "page {index} crop origin y");
    }

    close_document(view.document_id).expect("document should close");
}

#[test]
fn page_info_rejects_out_of_range_pages_and_unknown_documents() {
    let view = open_document(fixture_path("rotated"), None).expect("open rotated fixture");

    let error = page_info(view.document_id.clone(), 3).expect_err("page 4 does not exist");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    close_document(view.document_id.clone()).expect("document should close");

    let error = page_info(view.document_id, 0).expect_err("the document was closed");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);
}

#[test]
fn render_thumbnail_caps_the_long_edge_and_preserves_the_display_aspect() {
    let view = open_document(fixture_path("rotated"), None).expect("open rotated fixture");

    // Page 0 displays landscape (792x612) because of its 90 degree rotation.
    let thumbnail = render_thumbnail(view.document_id.clone(), 0, 120).expect("thumbnail page 1");
    assert_eq!(thumbnail.page_index, 0);
    assert_eq!(thumbnail.width, 120);
    assert_eq!(thumbnail.height, 93);

    let pixels = decode_png(&thumbnail.png_base64);
    assert_eq!((pixels.width, pixels.height), (120, 93));

    // Page 2 is a 288x432 media box rotated 270 degrees, so it displays 432x288.
    let thumbnail = render_thumbnail(view.document_id.clone(), 2, 120).expect("thumbnail page 3");
    assert_eq!((thumbnail.width, thumbnail.height), (120, 80));

    close_document(view.document_id).expect("document should close");
}

#[test]
fn render_thumbnail_rejects_invalid_sizes_and_out_of_range_pages() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    let error = render_thumbnail(view.document_id.clone(), 0, 0).expect_err("zero edge");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let error =
        render_thumbnail(view.document_id.clone(), 0, 4096).expect_err("edge above the cap");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let error = render_thumbnail(view.document_id.clone(), 9, 120).expect_err("missing page");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn render_page_region_matches_the_corresponding_crop_of_the_full_page() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    let full = render_page(view.document_id.clone(), 0, 1.0).expect("render full page");
    let full_pixels = decode_png(&full.png_base64);

    let region = render_page_region(view.document_id.clone(), 0, 1.0, 100, 120, 150, 100)
        .expect("render region");
    assert_eq!((region.x, region.y), (100, 120));
    assert_eq!((region.width, region.height), (150, 100));

    let region_pixels = decode_png(&region.png_base64);
    assert_eq!((region_pixels.width, region_pixels.height), (150, 100));

    assert_region_matches(&full_pixels, &region_pixels, 100, 120);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn render_page_region_uses_display_coordinates_for_rotated_pages() {
    let view = open_document(fixture_path("rotated"), None).expect("open rotated fixture");

    let full = render_page(view.document_id.clone(), 0, 1.0).expect("render full page");
    let full_pixels = decode_png(&full.png_base64);
    assert_eq!((full_pixels.width, full_pixels.height), (792, 612));

    // Page 1 is rotated 90 degrees; its text occupies display pixels around
    // x=716..731, y=73..258, so this window must capture visible content.
    let region = render_page_region(view.document_id.clone(), 0, 1.0, 700, 50, 140, 120)
        .expect("render region on a rotated page");
    let region_pixels = decode_png(&region.png_base64);

    assert_region_matches(&full_pixels, &region_pixels, 700, 50);

    let visible = region_pixels
        .rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[0] < 240 || pixel[1] < 240 || pixel[2] < 240)
        .count();
    assert!(
        visible > 50,
        "expected the rotated page's text in the region, found {visible} non-white pixels"
    );

    close_document(view.document_id).expect("document should close");
}

/// Compares a region against a crop of the full-page render.
///
/// PDFium re-rasterizes glyph edges slightly differently when a clipped render
/// offsets the device origin, so a handful of anti-aliased edge pixels may
/// differ. Alignment errors (a shifted region) move thousands of text pixels.
fn assert_region_matches(
    full: &DecodedPng,
    region: &DecodedPng,
    offset_x: u32,
    offset_y: u32,
) {
    let mut differing = 0usize;
    let mut total = 0usize;
    let mut sum_diff = 0i64;

    for row in 0..region.height {
        for column in 0..region.width {
            let source = (((row + offset_y) * full.width + (column + offset_x)) * 4) as usize;
            let target = ((row * region.width + column) * 4) as usize;

            if region.rgba[target..target + 4] != full.rgba[source..source + 4] {
                differing += 1;
                for channel in 0..3 {
                    sum_diff += (region.rgba[target + channel] as i32
                        - full.rgba[source + channel] as i32)
                        .unsigned_abs() as i64;
                }
            }

            total += 1;
        }
    }

    let differing_fraction = differing as f64 / total as f64;
    let mean_difference = sum_diff as f64 / (total * 3) as f64;
    assert!(
        differing_fraction <= 0.05,
        "{differing}/{total} pixels differ from the full-page crop"
    );
    assert!(
        mean_difference <= 4.0,
        "mean pixel difference {mean_difference} suggests a misaligned region"
    );
}

#[test]
fn render_page_region_clamps_to_the_page_and_accepts_high_zoom() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    // The backend renders only the requested viewport-sized window, so a scale
    // far above the whole-page bitmap limit still works.
    let region = render_page_region(view.document_id.clone(), 0, 20.0, 0, 0, 320, 240)
        .expect("render a 20x viewport-sized region");
    assert_eq!((region.width, region.height), (320, 240));
    let pixels = decode_png(&region.png_base64);
    assert_eq!((pixels.width, pixels.height), (320, 240));

    // A request that starts inside the page but extends past its edge is clamped.
    let full_width = (612.0 * 20.0) as u32;
    let region = render_page_region(
        view.document_id.clone(),
        0,
        20.0,
        full_width - 10,
        0,
        100,
        100,
    )
    .expect("clamped region");
    assert_eq!(region.width, 10);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn render_page_region_validates_scale_and_size() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    let error =
        render_page_region(view.document_id.clone(), 0, 0.0, 0, 0, 100, 100).expect_err("scale 0");
    assert_eq!(error.code, CommandErrorCode::InvalidScale);

    let error = render_page_region(view.document_id.clone(), 0, 1.0, 0, 0, 0, 100)
        .expect_err("zero width");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let error = render_page_region(view.document_id.clone(), 1, 1.0, 0, 0, 100, 100)
        .expect_err("page 2 does not exist");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    close_document(view.document_id).expect("document should close");
}

/// Manual latency measurement for the lazy thumbnail window, excluded from the
/// normal suite because timings vary by machine. Run with:
///
/// ```text
/// cargo test --test navigation measure_thumbnail -- --ignored --nocapture
/// ```
///
/// Set `PDFX_LATENCY_PDF` and `PDFX_LATENCY_PAGES` to measure an out-of-repo
/// fixture (for example the generated 2000-page stress file).
#[test]
#[ignore = "manual latency measurement"]
fn measure_thumbnail_and_region_latency_on_a_long_document() {
    let path = std::env::var("PDFX_LATENCY_PDF").unwrap_or_else(|_| fixture_path("large"));
    let expected_pages: u32 = std::env::var("PDFX_LATENCY_PAGES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);

    let opened = std::time::Instant::now();
    let view = open_document(path, None).expect("open large fixture");
    let open_elapsed = opened.elapsed();
    assert_eq!(view.page_count, expected_pages);

    // The first thumbnail in a process also initializes PDFium's render path.
    let _ = render_thumbnail(view.document_id.clone(), 0, 144).expect("warm-up thumbnail");

    // The rail mounts one viewport of thumbnails plus overscan.
    let window_started = std::time::Instant::now();
    let mut sizes = Vec::new();
    for page_index in 0..6 {
        let thumbnail = render_thumbnail(view.document_id.clone(), page_index, 144)
            .expect("visible thumbnail");
        sizes.push((thumbnail.width, thumbnail.height));
    }
    let window_elapsed = window_started.elapsed();

    let far_started = std::time::Instant::now();
    let far_index = expected_pages - 1;
    let far = render_thumbnail(view.document_id.clone(), far_index, 144)
        .expect("thumbnail after scrolling to the last page");
    let far_elapsed = far_started.elapsed();

    // 4000% on the 500-page fixture's first page: only the viewport is rendered.
    let region_started = std::time::Instant::now();
    let region = render_page_region(view.document_id.clone(), 0, 40.0, 0, 0, 1400, 1000)
        .expect("viewport region at 4000%");
    let region_elapsed = region_started.elapsed();

    let info_started = std::time::Instant::now();
    let info = page_info(view.document_id.clone(), far_index).expect("page info");
    let info_elapsed = info_started.elapsed();

    eprintln!(
        "navigation latency (pages={expected_pages}, open={open_elapsed:?}): \
         first 6 thumbnails={window_elapsed:?} (avg {:?}); \
         last-page thumbnail={far_elapsed:?}; \
         page_info={info_elapsed:?}; \
         1400x1000 region at 4000%={region_elapsed:?}",
        window_elapsed / 6,
    );

    assert_eq!(info.page_index, far_index);
    assert_eq!(sizes.len(), 6);
    assert!(sizes.iter().all(|(width, height)| *width > 0 && *height > 0));
    // The 612x792 page caps its long edge at 144 px.
    assert_eq!((far.width, far.height), (111, 144));
    assert_eq!((region.width, region.height), (1400, 1000));

    close_document(view.document_id).expect("document should close");
}

