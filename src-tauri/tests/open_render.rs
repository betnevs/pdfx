//! Command-boundary integration tests for opening documents and rendering pages.
//!
//! These tests call the same Rust functions the Tauri IPC layer exposes; the
//! frontend and the tests therefore share one seam. They require the pinned
//! PDFium library (`npm run setup:native`).

mod support;

use support::fixtures;

use base64::Engine as _;
use pdfx_lib::commands::{
    close_document, extract_text, open_document, render_page, CommandErrorCode,
};

#[test]
fn text_pdf_opens_with_page_count_and_first_page_dimensions() {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, "text").expect("text fixture");
    let path = fixtures::fixture_path(fixture);

    let view = open_document(path.to_string_lossy().into_owned(), None)
        .expect("the text fixture should open");

    assert_eq!(view.name, "text-latin.pdf");
    assert_eq!(view.page_count, 1);
    assert_eq!(view.page_width, 612.0);
    assert_eq!(view.page_height, 792.0);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn render_page_returns_a_png_with_the_page_aspect_ratio_and_visible_content() {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, "text").expect("text fixture");
    let path = fixtures::fixture_path(fixture);
    let view = open_document(path.to_string_lossy().into_owned(), None).expect("open text fixture");

    let image = render_page(view.document_id.clone(), 0, 1.0).expect("render page 1");

    assert_eq!(image.page_index, 0);
    assert_eq!(image.scale, 1.0);
    assert_eq!(image.width, 612);
    assert_eq!(image.height, 792);

    let pixels = decode_png(&image.png_base64);
    assert_eq!((pixels.width, pixels.height), (612, 792));

    let non_white = pixels
        .rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[0] < 245 || pixel[1] < 245 || pixel[2] < 245)
        .count();
    assert!(
        non_white > 1_000,
        "expected visible page content, found {non_white} non-white pixels"
    );

    close_document(view.document_id).expect("document should close");
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
fn encrypted_pdf_without_a_password_reports_password_required() {
    let error = open_document(fixture_path("encrypted"), None)
        .expect_err("encrypted file must ask for a password");

    assert_eq!(error.code, CommandErrorCode::PasswordRequired);
    assert!(!error.message.is_empty());
}

#[test]
fn encrypted_pdf_opens_with_the_user_password() {
    let view = open_document(fixture_path("encrypted"), Some("pdfx-user".to_owned()))
        .expect("the user password should open the encrypted fixture");

    assert_eq!(view.page_count, 1);
    close_document(view.document_id).expect("document should close");
}

#[test]
fn encrypted_pdf_with_a_wrong_password_reports_invalid_password() {
    let error = open_document(fixture_path("encrypted"), Some("wrong-password".to_owned()))
        .expect_err("a wrong password must not open the document");

    assert_eq!(error.code, CommandErrorCode::InvalidPassword);
}

#[test]
fn truncated_pdf_returns_a_structured_error_instead_of_panicking() {
    let error = open_document(fixture_path("malformed"), None)
        .expect_err("the truncated fixture must not open");

    assert_eq!(error.code, CommandErrorCode::CorruptDocument);
    assert!(!error.message.is_empty());
}

#[test]
fn bad_xref_pdf_recovers_or_returns_a_structured_error() {
    let path = {
        let manifest = fixtures::load_manifest();
        manifest
            .fixtures
            .iter()
            .find(|fixture| fixture.id == "malformed-bad-xref")
            .map(fixtures::fixture_path)
            .expect("bad-xref fixture")
            .to_string_lossy()
            .into_owned()
    };

    match open_document(path, None) {
        Ok(view) => {
            assert_eq!(view.page_count, 1);
            close_document(view.document_id).expect("document should close");
        }
        Err(error) => assert_eq!(error.code, CommandErrorCode::CorruptDocument),
    }
}

#[test]
fn render_page_rejects_out_of_range_pages_unknown_documents_and_invalid_scale() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    let error = render_page(view.document_id.clone(), 1, 1.0).expect_err("page 2 does not exist");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    let error = render_page(view.document_id.clone(), 0, 0.0).expect_err("scale 0 is invalid");
    assert_eq!(error.code, CommandErrorCode::InvalidScale);

    close_document(view.document_id.clone()).expect("document should close");

    let error = render_page(view.document_id, 0, 1.0).expect_err("the document was closed");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);
}

#[test]
fn missing_file_reports_file_not_found() {
    let error = open_document("/definitely/not/a/pdfx-document.pdf".to_owned(), None)
        .expect_err("a missing file cannot open");

    assert_eq!(error.code, CommandErrorCode::FileNotFound);
}

#[test]
fn text_pdf_exposes_its_text_layer() {
    let view = open_document(fixture_path("text"), None).expect("open text fixture");

    let text = extract_text(view.document_id.clone(), 0).expect("extract page text");

    assert!(text.text.contains("PDFX fixture"), "got {:?}", text.text);
    assert_eq!(text.char_count, text.text.chars().count() as u32);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn scanned_pdf_opens_without_a_text_layer() {
    let view = open_document(fixture_path("scanned"), None).expect("open scanned fixture");

    let text = extract_text(view.document_id.clone(), 0).expect("extract page text");

    assert_eq!(text.char_count, 0);
    assert!(text.text.is_empty(), "got {:?}", text.text);

    close_document(view.document_id).expect("document should close");
}

#[test]
fn large_document_reports_every_page_and_renders_only_the_first() {
    let view = open_document(fixture_path("large"), None).expect("open 500-page fixture");

    assert_eq!(view.page_count, 500);
    assert_eq!(view.page_width, 612.0);
    assert_eq!(view.page_height, 792.0);

    let image = render_page(view.document_id.clone(), 0, 1.0).expect("render first page");
    assert!(!image.png_base64.is_empty());

    close_document(view.document_id).expect("document should close");
}

#[test]
fn rotated_first_page_uses_the_rotated_page_dimensions() {
    let view = open_document(fixture_path("rotated"), None).expect("open rotated fixture");

    // Page 1 is a 612x792 media box with /Rotate 90, so it displays landscape.
    assert_eq!(view.page_width, 792.0);
    assert_eq!(view.page_height, 612.0);

    let image = render_page(view.document_id.clone(), 0, 1.0).expect("render rotated page");
    assert_eq!((image.width, image.height), (792, 612));

    close_document(view.document_id).expect("document should close");
}

/// Manual latency measurement, excluded from the normal suite because timings
/// vary by machine. Run with:
///
/// ```text
/// cargo test --test open_render measure_large_document -- --ignored --nocapture
/// ```
///
/// Set `PDFX_LATENCY_PDF` and `PDFX_LATENCY_PAGES` to measure an out-of-repo
/// fixture (for example the 2000-page stress file).
#[test]
#[ignore = "manual latency measurement"]
fn measure_large_document_first_page_open_and_render_latency() {
    let path = std::env::var("PDFX_LATENCY_PDF").unwrap_or_else(|_| fixture_path("large"));
    let expected_pages: u32 = std::env::var("PDFX_LATENCY_PAGES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500);

    // The first open in a process also binds and initializes PDFium. The app
    // does that during startup, so the warm measurement matches what a user
    // sees when opening the first document.
    let open_started = std::time::Instant::now();
    let view = open_document(path.clone(), None).expect("open large fixture");
    let cold_open_elapsed = open_started.elapsed();

    let render_started = std::time::Instant::now();
    let image =
        render_page(view.document_id.clone(), 0, 1.0).expect("render first page of large fixture");
    let cold_render_elapsed = render_started.elapsed();

    assert_eq!(view.page_count, expected_pages);
    let png_bytes = image.png_base64.len();
    close_document(view.document_id).expect("document should close");

    let warm_open_started = std::time::Instant::now();
    let view = open_document(path, None).expect("reopen large fixture");
    let warm_open_elapsed = warm_open_started.elapsed();

    let warm_render_started = std::time::Instant::now();
    let warm_image = render_page(view.document_id.clone(), 0, 1.0).expect("re-render first page");
    let warm_render_elapsed = warm_render_started.elapsed();

    assert_eq!(view.page_count, expected_pages);
    assert_eq!(warm_image.png_base64.len(), png_bytes);
    close_document(view.document_id).expect("document should close");

    eprintln!(
        "first-page latency (pages={expected_pages}, png={png_bytes} bytes): \
         cold open={cold_open_elapsed:?} render={cold_render_elapsed:?} total={:?}; \
         warm open={warm_open_elapsed:?} render={warm_render_elapsed:?} total={:?}",
        cold_open_elapsed + cold_render_elapsed,
        warm_open_elapsed + warm_render_elapsed,
    );
}
