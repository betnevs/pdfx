//! Command-boundary integration tests for printing and page-image export.
//!
//! These tests call the same Rust functions the Tauri IPC layer exposes, so
//! the frontend and the tests share the single command seam. They require the
//! pinned PDFium library (`npm run setup:native`).

mod support;

use std::fs;

use base64::Engine as _;
use support::fixtures;
use support::temp::TempDir;

use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, render_page, CommandErrorCode,
};
use pdfx_lib::print_export::{ensure_print_allowed, export_page, PageImageFormat};
use pdfx_lib::text_edit::{apply_text_edit, edit_history, text_edit_target};

fn open_text_document() -> (pdfx_lib::commands::DocumentView, String) {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, "text").expect("text fixture");
    let path = fixtures::fixture_path(fixture);
    let view = open_document(path.to_string_lossy().into_owned(), None)
        .expect("the text fixture should open");
    (view, path.to_string_lossy().into_owned())
}

fn open_fixture_document(id: &str) -> pdfx_lib::commands::DocumentView {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id:?} is missing from the manifest"));
    let path = fixtures::fixture_path(fixture);
    open_document(path.to_string_lossy().into_owned(), None)
        .unwrap_or_else(|error| panic!("fixture {id:?} should open: {error:?}"))
}

#[test]
fn exporting_the_page_as_png_at_72_dpi_writes_exactly_the_canvas_bitmap() {
    let (view, source_path) = open_text_document();
    let source_before = fs::read(&source_path).expect("read the Source Document");
    let canvas = render_page(view.document_id.clone(), 0, 1.0).expect("render the canvas page");

    let temp = TempDir::new("export-png");
    let destination = temp.path().join("page-1.png");
    let exported = export_page(
        view.document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        destination.to_string_lossy().into_owned(),
    )
    .expect("export page 1 as PNG");

    assert_eq!(exported.format, PageImageFormat::Png);
    assert_eq!(exported.dpi, 72);
    assert_eq!(
        (exported.width, exported.height),
        (canvas.width, canvas.height),
        "exported image dimensions match the canvas render"
    );

    let expected = base64::engine::general_purpose::STANDARD
        .decode(&canvas.png_base64)
        .expect("canvas payload is base64");
    let written = fs::read(&destination).expect("the exported PNG should exist");
    assert_eq!(
        written, expected,
        "a 72 DPI export is byte-identical to the canvas bitmap"
    );
    assert_eq!(exported.bytes, written.len() as u64);
    assert_eq!(exported.path, destination.to_string_lossy());

    assert_eq!(
        fs::read(&source_path).expect("re-read the Source Document"),
        source_before,
        "export must never modify the Source Document"
    );

    close_document(view.document_id).expect("document should close");
}

#[test]
fn exporting_the_page_as_jpeg_at_150_dpi_uses_the_requested_resolution_and_page_content() {
    let (view, _source_path) = open_text_document();

    let temp = TempDir::new("export-jpeg");
    let destination = temp.path().join("page-1.jpg");
    let exported = export_page(
        view.document_id.clone(),
        0,
        PageImageFormat::Jpg,
        150,
        destination.to_string_lossy().into_owned(),
    )
    .expect("export page 1 as JPEG");

    assert_eq!(exported.format, PageImageFormat::Jpg);
    assert_eq!(exported.dpi, 150);
    // 612x792 points at 150 DPI is exactly 1275x1650 pixels.
    assert_eq!((exported.width, exported.height), (1275, 1650));

    let written = fs::read(&destination).expect("the exported JPEG should exist");
    assert_eq!(written.len() as u64, exported.bytes);
    assert_eq!(
        &written[..2],
        [0xff, 0xd8],
        "the exported file starts with the JPEG SOI marker"
    );

    let canvas = render_page(view.document_id.clone(), 0, 150.0 / 72.0)
        .expect("render the canvas page at the export resolution");
    let expected = decode_png(&canvas.png_base64);
    let actual = decode_jpeg(&written);
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "the JPEG has the same dimensions as the canvas render"
    );
    let error = mean_absolute_error(&actual.rgb, &expected.rgba);
    assert!(
        error < 6.0,
        "the JPEG export should match the canvas render, mean channel error {error:.2}"
    );

    close_document(view.document_id).expect("document should close");
}

#[test]
fn exporting_does_not_change_the_edit_history_or_the_dirty_state() {
    let (view, _source_path) = open_text_document();
    let layer = extract_text_layer(view.document_id.clone(), 0).expect("text layer");
    assert!(layer.has_text, "the text fixture has editable text");
    let target =
        text_edit_target(view.document_id.clone(), 0, 0).expect("text edit target at char 0");
    let edited = apply_text_edit(
        view.document_id.clone(),
        0,
        target.box_id,
        "Read-only export.".to_owned(),
    )
    .expect("apply an edit so the history is dirty");
    assert!(edited.history.dirty, "the edit marks the Document unsaved");
    let history_before = edit_history(view.document_id.clone()).expect("history before export");

    let temp = TempDir::new("export-history");
    export_page(
        view.document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        temp.path()
            .join("edited.png")
            .to_string_lossy()
            .into_owned(),
    )
    .expect("export the edited page");

    let history_after = edit_history(view.document_id.clone()).expect("history after export");
    assert_eq!(
        history_after, history_before,
        "export must not record history or move the saved baseline"
    );
    assert!(
        history_after.dirty,
        "export must not clear the unsaved marker left by the edit"
    );

    close_document(view.document_id).expect("document should close");
}

#[test]
fn printing_is_denied_when_permission_restrictions_forbid_it() {
    let view = open_fixture_document("encrypted-print-denied");

    assert!(
        !view.permissions.can_print,
        "the fixture's Permission Restrictions deny printing"
    );
    assert!(
        !view.read_only,
        "the fixture still allows editing, so print gating is separate from read-only"
    );

    let error = ensure_print_allowed(&view.document_id)
        .expect_err("a document that forbids printing must not reach the print dialog");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(!error.message.is_empty());

    close_document(view.document_id).expect("document should close");
}

#[test]
fn printing_is_allowed_for_an_unrestricted_document() {
    let (view, _source_path) = open_text_document();

    ensure_print_allowed(&view.document_id).expect("unrestricted documents may print");

    close_document(view.document_id).expect("document should close");
}

struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode_png(png_base64: &str) -> DecodedImage {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .expect("render payload is base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header should parse");
    let mut buffer = vec![0; reader.output_buffer_size().expect("output buffer size")];
    let info = reader
        .next_frame(&mut buffer)
        .expect("PNG frame should decode");
    buffer.truncate(info.buffer_size());
    DecodedImage {
        width: info.width,
        height: info.height,
        rgba: buffer,
    }
}

struct DecodedJpeg {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

fn decode_jpeg(bytes: &[u8]) -> DecodedJpeg {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .expect("the exported file should decode as JPEG")
        .to_rgb8();
    DecodedJpeg {
        width: image.width(),
        height: image.height(),
        rgb: image.into_raw(),
    }
}

/// Mean absolute difference between JPEG RGB values and the canvas RGBA.
fn mean_absolute_error(rgb: &[u8], rgba: &[u8]) -> f64 {
    assert_eq!(
        rgb.len() * 4,
        rgba.len() * 3,
        "both images have the same size"
    );
    let mut total = 0u64;
    let mut samples = 0u64;
    for (pixel, reference) in rgb.chunks_exact(3).zip(rgba.chunks_exact(4)) {
        // The page background is opaque white, so alpha does not need blending.
        for (value, expected) in pixel.iter().zip(reference.iter()) {
            total += value.abs_diff(*expected) as u64;
            samples += 1;
        }
    }
    total as f64 / samples as f64
}

#[test]
fn export_rejects_invalid_resolutions_and_pages_without_writing_a_file() {
    let (view, _source_path) = open_text_document();
    let temp = TempDir::new("export-validation");
    let destination = temp.path().join("invalid.png");
    let destination_arg = destination.to_string_lossy().into_owned();

    let dpi_error = export_page(
        view.document_id.clone(),
        0,
        PageImageFormat::Png,
        0,
        destination_arg.clone(),
    )
    .expect_err("0 DPI is outside the export bounds");
    assert_eq!(dpi_error.code, CommandErrorCode::InvalidArgument);

    let page_error = export_page(
        view.document_id.clone(),
        99,
        PageImageFormat::Png,
        72,
        destination_arg,
    )
    .expect_err("the text fixture has only one page");
    assert_eq!(page_error.code, CommandErrorCode::PageOutOfRange);

    assert!(
        !destination.exists(),
        "a rejected export must not write a file"
    );

    close_document(view.document_id).expect("document should close");
}

#[test]
fn exporting_over_the_source_document_is_refused_and_leaves_the_source_untouched() {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, "text").expect("text fixture");
    let temp = TempDir::new("export-source-guard");
    let source = temp.path().join("source.pdf");
    fs::copy(fixtures::fixture_path(fixture), &source).expect("copy the fixture");
    let source_before = fs::read(&source).expect("read the copied Source Document");

    let view = open_document(source.to_string_lossy().into_owned(), None)
        .expect("the copied fixture should open");

    let error = export_page(
        view.document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        source.to_string_lossy().into_owned(),
    )
    .expect_err("exporting over the Source Document must be refused");

    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    assert_eq!(
        fs::read(&source).expect("re-read the Source Document"),
        source_before,
        "a refused export must not touch the Source Document"
    );

    close_document(view.document_id).expect("document should close");
}
