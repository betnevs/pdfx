//! Integration tests for the PDFium edit/save adapter.
//!
//! These tests drive the adapter the same way the save pipeline will, then reopen
//! the produced bytes with PDFium to prove the output is structurally intact and
//! that annotations actually paint.
//!
//! They require the pinned PDFium library (`npm run setup:native`).

mod support;

use support::fixtures;

use pdfium_render::prelude::{
    PdfDocument, PdfDocumentMetadataTagType, PdfPageAnnotationCommon, PdfPageAnnotationType,
    PdfPoints, PdfRect, PdfSecurityHandlerRevision,
};
use pdfx_lib::engine::Engine;
use pdfx_lib::pdfium_adapter::{
    AdapterError, AnnotationRect, AnnotationRef, PdfiumAdapter, SaveMode,
};

fn fixture_bytes(category: &str) -> (Vec<u8>, Option<String>) {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, category).expect("fixture category");
    let path = fixtures::fixture_path(fixture);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    (bytes, fixture.password.clone())
}

fn open_adapter(bytes: Vec<u8>, password: Option<&str>) -> PdfiumAdapter<'static> {
    let engine = Engine::shared().expect("PDF engine should initialize");
    let document = engine
        .open_bytes(bytes, password)
        .expect("document should open with PDFium");
    PdfiumAdapter::new(document)
}

fn rect(bottom: f32, left: f32, top: f32, right: f32) -> PdfRect {
    PdfRect::new(
        PdfPoints::new(bottom),
        PdfPoints::new(left),
        PdfPoints::new(top),
        PdfPoints::new(right),
    )
}

/// Opens the structure fixture and makes one high-level `pdfium-render` edit on
/// the same document the adapter will later save.
fn open_structure_with_square_annotation(source: Vec<u8>) -> PdfDocument<'static> {
    let engine = Engine::shared().expect("PDF engine should initialize");
    let document = engine.open_bytes(source, None).expect("open fixture");
    let pages = document.pages();
    let mut page = pages.get(0).expect("page 0");
    let annotations = page.annotations_mut();
    let mut square = annotations
        .create_square_annotation()
        .expect("create square");
    square
        .set_bounds(rect(72.0, 72.0, 144.0, 144.0))
        .expect("set bounds");
    document
}

/// Asserts every structural feature of the structure fixture, plus the
/// high-level square annotation, survived into `document`.
fn assert_structure_fixture_intact(document: &PdfDocument<'_>) {
    assert_eq!(document.pages().len(), 1);

    let bookmarks: Vec<String> = document
        .bookmarks()
        .iter()
        .filter_map(|bookmark| bookmark.title())
        .collect();
    assert_eq!(bookmarks, vec!["Introduction", "Details"]);

    assert_eq!(
        document
            .metadata()
            .get(PdfDocumentMetadataTagType::Title)
            .expect("Info title")
            .value(),
        "PDFX structure fixture"
    );

    let page = document.pages().get(0).expect("page 0");
    let link = page.links().first().expect("the URI link should survive");
    assert_eq!(
        link.action()
            .expect("link action")
            .as_uri_action()
            .expect("URI action")
            .uri()
            .expect("URI value"),
        "https://example.com/pdfx"
    );

    let attachments: Vec<String> = document
        .attachments()
        .iter()
        .map(|attachment| attachment.name())
        .collect();
    assert_eq!(attachments, vec!["notes.txt"]);

    let text = page.text().expect("text layer").all();
    assert!(text.contains("PDFX structure fixture"), "got {text:?}");

    let annotation_types: Vec<PdfPageAnnotationType> = page
        .annotations()
        .iter()
        .map(|annotation| annotation.annotation_type())
        .collect();
    assert!(annotation_types.contains(&PdfPageAnnotationType::Square));
    assert!(annotation_types.contains(&PdfPageAnnotationType::Link));
}

/// Renders page `page_index` at 1:1 and returns (non-white pixels, blue-fill pixels).
fn render_pixels(document: &PdfDocument<'_>, page_index: i32) -> (usize, usize) {
    let page = document.pages().get(page_index).expect("page to render");
    let width = page.width().value.round() as i32;
    let height = page.height().value.round() as i32;
    let bitmap = page
        .render(width, height, None)
        .expect("PDFium should render the page");
    let rgba = bitmap.as_rgba_bytes();

    let mut non_white = 0usize;
    let mut blue = 0usize;
    for pixel in rgba.chunks_exact(4) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        if r < 245 || g < 245 || b < 245 {
            non_white += 1;
        }
        if b > 150 && r < 100 && g < 150 {
            blue += 1;
        }
    }
    (non_white, blue)
}

#[test]
fn circle_annotation_with_border_and_custom_appearance_is_visible_after_reopen() {
    let (source, _) = fixture_bytes("text");
    let adapter = open_adapter(source, None);

    let (baseline_non_white, baseline_blue) = render_pixels(adapter.document(), 0);
    assert!(
        baseline_blue < 100,
        "the text fixture should not already contain blue fill, found {baseline_blue} pixels"
    );

    let circle = adapter
        .create_circle_annotation(0, AnnotationRect::new(72.0, 72.0, 272.0, 272.0))
        .expect("create a circle annotation");
    adapter
        .set_border_width(circle, 4.0)
        .expect("set the annotation border width");
    adapter
        .set_appearance(circle, "0.12 0.35 0.86 rg 72 72 200 200 re f\n")
        .expect("install a custom appearance stream");

    let saved = adapter
        .save(SaveMode::Incremental)
        .expect("save incrementally");

    let reopened = open_adapter(saved, None);
    let document = reopened.document();
    assert_eq!(
        reopened.annotation_count(0).expect("count annotations"),
        1,
        "the saved file should contain exactly the circle annotation"
    );

    let page = document.pages().get(0).expect("page 0");
    let annotation = page.annotations().get(0).expect("circle annotation");
    assert_eq!(annotation.annotation_type(), PdfPageAnnotationType::Circle);

    let target = AnnotationRef::new(0, 0);
    let border_width = reopened.border_width(target).expect("read border width");
    assert!(
        (border_width - 4.0).abs() < 0.01,
        "border width should round-trip through save, got {border_width}"
    );

    let (non_white, blue) = render_pixels(document, 0);
    assert!(
        non_white > baseline_non_white + 30_000,
        "the appearance stream should paint: {baseline_non_white} -> {non_white} non-white pixels"
    );
    assert!(
        blue > 20_000,
        "the custom appearance fill should be visible, found {blue} blue pixels"
    );
}

#[test]
fn ink_annotation_strokes_and_appearance_survive_save_and_reopen() {
    let (source, _) = fixture_bytes("text");
    let adapter = open_adapter(source, None);

    let (baseline_non_white, baseline_blue) = render_pixels(adapter.document(), 0);
    assert!(baseline_blue < 100);

    let ink = adapter
        .create_ink_annotation(0, AnnotationRect::new(72.0, 72.0, 272.0, 272.0))
        .expect("create an ink annotation");

    let first = adapter
        .add_ink_stroke(ink, &[(80.0, 80.0), (120.0, 180.0), (200.0, 120.0)])
        .expect("add the first stroke");
    assert_eq!(first, 0);
    let second = adapter
        .add_ink_stroke(ink, &[(90.0, 200.0), (240.0, 240.0)])
        .expect("add the second stroke");
    assert_eq!(second, 1);
    assert_eq!(
        adapter.ink_stroke_count(ink).expect("count strokes"),
        2,
        "both strokes should be present before saving"
    );

    adapter
        .set_appearance(ink, "0.12 0.35 0.86 rg 72 72 200 200 re f\n")
        .expect("install a custom appearance stream");

    let saved = adapter
        .save(SaveMode::Incremental)
        .expect("save incrementally");
    let reopened = open_adapter(saved, None);

    assert_eq!(reopened.annotation_count(0).expect("count annotations"), 1);
    let page = reopened.document().pages().get(0).expect("page 0");
    let annotation = page.annotations().get(0).expect("ink annotation");
    assert_eq!(annotation.annotation_type(), PdfPageAnnotationType::Ink);

    let target = AnnotationRef::new(0, 0);
    assert_eq!(
        reopened
            .ink_stroke_count(target)
            .expect("count strokes after reopen"),
        2,
        "the InkList should survive an incremental save"
    );

    let (non_white, blue) = render_pixels(reopened.document(), 0);
    assert!(
        non_white > baseline_non_white + 30_000,
        "the ink appearance should paint: {baseline_non_white} -> {non_white} non-white pixels"
    );
    assert!(
        blue > 20_000,
        "the custom ink appearance should be visible, found {blue} blue pixels"
    );
}

#[test]
fn encrypted_sources_refuse_encrypted_output_and_remove_security_decrypts() {
    let manifest = fixtures::load_manifest();
    let encrypted: Vec<_> = manifest
        .fixtures
        .iter()
        .filter(|fixture| fixture.category == "encrypted")
        .collect();
    assert_eq!(encrypted.len(), 3, "expected every encrypted fixture");

    for fixture in encrypted {
        let path = fixtures::fixture_path(fixture);
        let source =
            std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let password = fixture.password.as_deref();

        let adapter = open_adapter(source.clone(), password);
        assert!(
            adapter.is_encrypted().expect("read encryption state"),
            "{} should report itself as encrypted",
            fixture.id
        );

        assert!(
            matches!(
                adapter.save_incremental(),
                Err(AdapterError::EncryptedDocument)
            ),
            "{} must not silently emit an encrypted incremental revision",
            fixture.id
        );
        assert!(
            matches!(
                adapter.save_full_rewrite(),
                Err(AdapterError::EncryptedDocument)
            ),
            "{} must not silently emit an encrypted full rewrite",
            fixture.id
        );

        let cleared = adapter
            .save_full_rewrite_removing_security()
            .unwrap_or_else(|error| panic!("remove security from {}: {error}", fixture.id));

        assert!(
            !cleared.starts_with(&source),
            "removing security is a full rewrite, not an append"
        );

        // Reopen without any password and with the plain `pdfium-render` API,
        // independent of the adapter that produced the bytes.
        let engine = Engine::shared().expect("PDF engine should initialize");
        let reopened = engine.open_bytes(cleared, None).unwrap_or_else(|error| {
            panic!("{} should reopen without a password: {error}", fixture.id)
        });

        assert_eq!(
            reopened
                .permissions()
                .security_handler_revision()
                .expect("security handler revision"),
            PdfSecurityHandlerRevision::Unprotected,
            "{} should be unencrypted after a remove-security rewrite",
            fixture.id
        );
        assert_eq!(reopened.pages().len(), fixture.page_count as i32);
    }
}

/// Renders page `page_index` at 1:1 and returns its raw RGBA bytes.
fn render_page_bytes(document: &PdfDocument<'_>, page_index: i32) -> Vec<u8> {
    let page = document.pages().get(page_index).expect("page to render");
    let width = page.width().value.round() as i32;
    let height = page.height().value.round() as i32;
    let bitmap = page
        .render(width, height, None)
        .expect("PDFium should render the page");
    bitmap.as_rgba_bytes()
}

#[test]
fn full_rewrite_annotates_only_the_target_page_and_preserves_untouched_content() {
    let (source, _) = fixture_bytes("large");
    let adapter = open_adapter(source.clone(), None);

    assert_eq!(adapter.document().pages().len(), 500);
    let (baseline_non_white, _) = render_pixels(adapter.document(), 0);
    let untouched_before = render_page_bytes(adapter.document(), 1);

    let circle = adapter
        .create_circle_annotation(0, AnnotationRect::new(72.0, 72.0, 272.0, 272.0))
        .expect("create a circle annotation");
    adapter
        .set_appearance(circle, "0.12 0.35 0.86 rg 72 72 200 200 re f\n")
        .expect("install a custom appearance stream");

    let saved = adapter.save_full_rewrite().expect("full rewrite");

    assert!(
        !saved.starts_with(&source),
        "a full rewrite must produce a new file, not append a revision"
    );

    let reopened = open_adapter(saved, None);
    assert_eq!(reopened.document().pages().len(), 500);
    assert_eq!(reopened.annotation_count(0).expect("count annotations"), 1);

    let (non_white, blue) = render_pixels(reopened.document(), 0);
    assert!(
        non_white > baseline_non_white + 30_000,
        "the annotation should paint on page 0 after the rewrite: \
         {baseline_non_white} -> {non_white} non-white pixels"
    );
    assert!(blue > 20_000, "found {blue} blue pixels");

    assert_eq!(
        render_page_bytes(reopened.document(), 1),
        untouched_before,
        "page 1 was never edited, so its rendering must not change"
    );
}

#[test]
fn incremental_save_keeps_original_bytes_as_prefix_and_preserves_structure() {
    let (source, _) = fixture_bytes("structure");
    let document = open_structure_with_square_annotation(source.clone());

    let adapter = PdfiumAdapter::new(document);
    let saved = adapter.save_incremental().expect("incremental save");

    assert!(
        saved.starts_with(&source),
        "an incremental save must append to the source bytes, not rewrite them"
    );
    assert!(
        saved.len() > source.len(),
        "the incremental revision should add bytes"
    );

    let reopened = open_adapter(saved, None);
    assert_structure_fixture_intact(reopened.document());
}

#[test]
fn full_rewrite_preserves_document_structure_and_high_level_edits() {
    let (source, _) = fixture_bytes("structure");
    let document = open_structure_with_square_annotation(source.clone());

    let adapter = PdfiumAdapter::new(document);
    let saved = adapter.save_full_rewrite().expect("full rewrite");

    assert!(
        !saved.starts_with(&source),
        "a full rewrite must produce a new file, not append a revision"
    );

    let reopened = open_adapter(saved, None);
    assert_structure_fixture_intact(reopened.document());
}
