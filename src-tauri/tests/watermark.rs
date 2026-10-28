//! Command-boundary acceptance tests for text and image watermarks.
//!
//! Watermarks are page *content* objects, not annotations: every assertion goes
//! through the same Rust functions the Tauri IPC layer exposes. Placement,
//! opacity and rotation are checked against pixels rendered by PDFium at a
//! fixed scale, while saved output is parsed with lopdf and reopened through
//! PDFium so "the mark is really in the document" is checked against a fresh
//! parse rather than the session's own state.

mod support;

use std::path::Path;

use base64::Engine as _;
use pdfx_lib::annotations::list_annotations;
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, page_info, render_page, CommandErrorCode,
};
use pdfx_lib::flatten_export::export_flattened_copy;
use pdfx_lib::page_ops::copy_page;
use pdfx_lib::print_export::{export_page, PageImageFormat};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::{
    create_text_box, delete_text_box, edit_history, redo_text_edit, text_edit_target,
    undo_text_edit, CreateTextBoxRequest,
};
use pdfx_lib::watermark::{
    apply_watermark, delete_watermark, watermark_image_preview, watermark_state,
    WatermarkEditResult, WatermarkKind, WatermarkRequest, WatermarkScope,
};
use support::synthetic;
use support::temp::TempDir;

const PAGE_WIDTH: f32 = 612.0;
const PAGE_HEIGHT: f32 = 792.0;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

/// Colours used by generated watermark images, chosen to stand out from the
/// page background and each other in rendered pixels.
const RED: [u8; 4] = [220, 30, 40, 255];

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

fn write_image(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write test image");
    path.to_string_lossy().into_owned()
}

fn text_request(scope: WatermarkScope, text: &str) -> WatermarkRequest {
    WatermarkRequest {
        scope,
        kind: WatermarkKind::Text,
        text: Some(text.to_owned()),
        image_path: None,
        opacity: 100,
        rotation: 0.0,
        scale: 100.0,
        position_x: 0.5,
        position_y: 0.5,
    }
}

fn image_request(scope: WatermarkScope, image_path: &str, opacity: u8) -> WatermarkRequest {
    WatermarkRequest {
        scope,
        kind: WatermarkKind::Image,
        text: None,
        image_path: Some(image_path.to_owned()),
        opacity,
        rotation: 0.0,
        scale: 100.0,
        position_x: 0.5,
        position_y: 0.5,
    }
}

/// Colour of one point in a 1:1 render.
fn page_pixel(pixels: &DecodedPng, x: f32, y: f32) -> (u8, u8, u8) {
    pixels.pixel(x.round() as u32, y.round() as u32)
}

fn image_xobject_count(document: &lopdf::Document) -> usize {
    image_xobject_ids(document).len()
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

/// Pixels that differ noticeably from a baseline render, used to isolate a
/// watermark from a page's original content.
struct PixelDiff {
    count: u32,
    min_luminance: u8,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl PixelDiff {
    fn center(&self) -> (f32, f32) {
        (
            (self.min_x as f32 + self.max_x as f32) / 2.0,
            (self.min_y as f32 + self.max_y as f32) / 2.0,
        )
    }

    fn width(&self) -> u32 {
        self.max_x - self.min_x
    }

    fn height(&self) -> u32 {
        self.max_y - self.min_y
    }
}

fn diff_from(baseline: &DecodedPng, changed: &DecodedPng) -> PixelDiff {
    assert_eq!(
        (baseline.width, baseline.height),
        (changed.width, changed.height),
        "renders must share dimensions"
    );
    let mut diff = PixelDiff {
        count: 0,
        min_luminance: 255,
        min_x: u32::MAX,
        min_y: u32::MAX,
        max_x: 0,
        max_y: 0,
    };
    for y in 0..changed.height {
        for x in 0..changed.width {
            let before = baseline.pixel(x, y);
            let after = changed.pixel(x, y);
            let delta = (i32::from(before.0) - i32::from(after.0))
                .abs()
                .max((i32::from(before.1) - i32::from(after.1)).abs())
                .max((i32::from(before.2) - i32::from(after.2)).abs());
            if delta <= 24 {
                continue;
            }
            diff.count += 1;
            diff.min_luminance = diff.min_luminance.min(after.0.min(after.1).min(after.2));
            diff.min_x = diff.min_x.min(x);
            diff.min_y = diff.min_y.min(y);
            diff.max_x = diff.max_x.max(x);
            diff.max_y = diff.max_y.max(y);
        }
    }
    diff
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            password: None,
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
        },
        &mut |_| {},
    )
    .expect("save output");
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: expected {expected} +/- {tolerance}, got {actual}"
    );
}

#[test]
fn text_watermark_is_real_page_content_and_survives_save() {
    let pdf = synthetic::single_line_pdf();
    let source_path = Path::new(&pdf.path_string()).to_path_buf();
    let source_bytes = std::fs::read(&source_path).expect("read source");
    let source_mtime = std::fs::metadata(&source_path)
        .expect("source metadata")
        .modified()
        .expect("source mtime");
    let document_id = open_synthetic(&pdf);

    let before = render_pixels(&document_id, 0, 1.0);
    let applied = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "CONFIDENTIAL"),
    )
    .expect("apply text watermark");

    assert_eq!(applied.history.revision, 1);
    assert!(applied.history.can_undo);
    assert!(applied.history.dirty);
    let watermark = applied.watermark.expect("a watermark is active");
    assert_eq!(watermark.kind, WatermarkKind::Text);
    assert_eq!(watermark.scope, WatermarkScope::Current);
    assert_eq!(watermark.text.as_deref(), Some("CONFIDENTIAL"));
    assert_eq!(watermark.page_indices, vec![0]);

    let state = watermark_state(document_id.clone())
        .expect("read watermark state")
        .expect("state is present");
    assert_eq!(state.opacity, 100);
    assert_eq!(state.rotation, 0.0);

    assert!(
        list_annotations(document_id.clone())
            .expect("list annotations")
            .is_empty(),
        "a watermark must not become a native annotation"
    );
    let layer = extract_text_layer(document_id.clone(), 0).expect("extract text");
    assert!(
        layer.text.contains("CONFIDENTIAL"),
        "the watermark text stays searchable page content: {:?}",
        layer.text
    );

    let after = render_pixels(&document_id, 0, 1.0);
    let diff = diff_from(&before, &after);
    assert!(
        diff.count > 200,
        "the watermark must paint visible pixels, saw {}",
        diff.count
    );
    let (center_x, center_y) = diff.center();
    assert_close(center_x, PAGE_WIDTH / 2.0, 6.0, "watermark center x");
    assert_close(center_y, PAGE_HEIGHT / 2.0, 6.0, "watermark center y");

    let dir = TempDir::new("watermark-text-save");
    let output = dir.path().join("watermarked.pdf");
    save_as(&document_id, &output);

    assert_eq!(
        std::fs::read(&source_path).expect("source still readable"),
        source_bytes,
        "saving must never rewrite the Source Document"
    );
    assert_eq!(
        std::fs::metadata(&source_path)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_mtime,
        "saving must never touch the Source Document mtime"
    );

    // The saved file is a real PDF with a content stream, not an annotation.
    let saved = lopdf::Document::load(&output).expect("load saved PDF");
    let saved_pages = saved.get_pages();
    assert_eq!(saved_pages.len(), 1);
    let content = saved.get_page_content(*saved_pages.values().next().expect("one page"));
    assert!(!content.is_empty(), "saved page keeps page content");

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    let reopened_id = reopened.document_id;
    assert!(list_annotations(reopened_id.clone())
        .expect("list saved annotations")
        .is_empty());
    let saved_layer = extract_text_layer(reopened_id.clone(), 0).expect("extract saved text");
    assert!(
        saved_layer.text.contains("CONFIDENTIAL"),
        "the saved watermark is still text: {:?}",
        saved_layer.text
    );
    let reopened_pixels = render_pixels(&reopened_id, 0, 1.0);
    let reopened_diff = diff_from(&before, &reopened_pixels);
    assert!(
        reopened_diff.count > 200,
        "the saved watermark renders after reopening, saw {} pixels",
        reopened_diff.count
    );
    let (reopened_x, reopened_y) = reopened_diff.center();
    assert_close(reopened_x, center_x, 3.0, "saved watermark center x");
    assert_close(reopened_y, center_y, 3.0, "saved watermark center y");
    assert!(
        (i32::from(reopened_diff.min_luminance) - i32::from(diff.min_luminance)).abs() <= 24,
        "saved opacity matches the live render: live {}, saved {}",
        diff.min_luminance,
        reopened_diff.min_luminance
    );
    close_document(reopened_id).expect("close reopened document");
}

#[test]
fn history_is_per_document() {
    let first = synthetic::single_line_pdf();
    let second = synthetic::single_line_pdf();
    let first_id = open_synthetic(&first);
    let second_id = open_synthetic(&second);

    apply_watermark(
        first_id.clone(),
        0,
        text_request(WatermarkScope::Current, "FIRST"),
    )
    .expect("apply to first document");

    assert!(
        edit_history(first_id.clone())
            .expect("first history")
            .can_undo
    );
    assert!(
        !edit_history(second_id.clone())
            .expect("second history")
            .can_undo,
        "an edit in one Document must not dirty another"
    );

    close_document(first_id).expect("close first");
    close_document(second_id).expect("close second");
}

/// `page_info` is a command-boundary smoke check that keeps the page used by
/// the geometry assertions honest about its display size.
#[test]
fn page_size_is_fixture_size() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let info = page_info(document_id.clone(), 0).expect("page info");
    assert_eq!((info.width, info.height), (PAGE_WIDTH, PAGE_HEIGHT));
    close_document(document_id).expect("close document");
}

fn page_content(document: &lopdf::Document) -> String {
    let pages = document.get_pages();
    let id = *pages.values().next().expect("one page");
    String::from_utf8_lossy(&document.get_page_content(id)).into_owned()
}

fn font_dictionaries(document: &lopdf::Document) -> Vec<String> {
    document
        .objects
        .values()
        .filter_map(|object| {
            let lopdf::Object::Dictionary(dictionary) = object else {
                return None;
            };
            if dictionary.get(b"Type").ok() != Some(&lopdf::Object::Name(b"Font".to_vec())) {
                return None;
            }
            dictionary
                .get(b"BaseFont")
                .ok()
                .and_then(|base| match base {
                    lopdf::Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
                    _ => None,
                })
        })
        .collect()
}

#[test]
fn chinese_watermark_replaces_without_stacking_and_repeat_saves_stay_stable() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);

    let first = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "机密文件"),
    )
    .expect("apply a Chinese watermark");
    assert_eq!(first.history.revision, 1);
    assert_eq!(
        first
            .watermark
            .as_ref()
            .and_then(|watermark| watermark.text.as_deref()),
        Some("机密文件")
    );
    let first_layer = extract_text_layer(document_id.clone(), 0).expect("extract text");
    assert!(
        first_layer.text.contains("机密文件"),
        "the Chinese watermark stays searchable: {:?}",
        first_layer.text
    );

    // Applying a different watermark replaces the first one; it must not stack
    // a second text object over the old one.
    let second = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "内部资料"),
    )
    .expect("replace the watermark");
    assert_eq!(second.history.revision, 2);
    let replaced_layer = extract_text_layer(document_id.clone(), 0).expect("extract text");
    assert!(
        replaced_layer.text.contains("内部资料"),
        "the replacement is visible: {:?}",
        replaced_layer.text
    );
    assert!(
        !replaced_layer.text.contains("机密文件"),
        "re-applying must not leave the previous watermark behind: {:?}",
        replaced_layer.text
    );
    let replaced_pixels = render_pixels(&document_id, 0, 1.0);
    let replaced_diff = diff_from(&baseline, &replaced_pixels);
    assert!(
        replaced_diff.count > 200,
        "the replacement paints visible pixels"
    );

    let dir = TempDir::new("watermark-chinese-repeat");
    let output = dir.path().join("repeat.pdf");
    save_as(&document_id, &output);
    let first_save = lopdf::Document::load(&output).expect("parse first save");
    let first_fonts = font_dictionaries(&first_save);
    let first_content = page_content(&first_save);
    let first_objects = first_save.objects.len();
    assert!(
        first_fonts
            .iter()
            .any(|font| font.to_ascii_lowercase().contains("notosans")),
        "the Chinese watermark embeds the bundled font: {first_fonts:?}"
    );

    save_as(&document_id, &output);
    let second_save = lopdf::Document::load(&output).expect("parse second save");
    let second_fonts = font_dictionaries(&second_save);
    let second_content = page_content(&second_save);
    assert_eq!(
        second_save.objects.len(),
        first_objects,
        "a repeated save must not append objects: {second_fonts:?}"
    );
    assert_eq!(
        second_fonts.len(),
        first_fonts.len(),
        "a repeated save must not embed a second copy of the font: \
         first {first_fonts:?}, second {second_fonts:?}"
    );
    assert_eq!(
        second_content, first_content,
        "a repeated save must not stack another copy of the watermark content"
    );

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    assert!(
        list_annotations(reopened.document_id.clone())
            .expect("list annotations")
            .is_empty(),
        "the Chinese watermark is page content, not an annotation"
    );
    let reopened_layer =
        extract_text_layer(reopened.document_id.clone(), 0).expect("extract saved text");
    assert!(
        reopened_layer.text.contains("内部资料"),
        "the saved Chinese watermark is still searchable: {:?}",
        reopened_layer.text
    );
    assert!(
        !reopened_layer.text.contains("机密文件"),
        "the saved file contains only the replacement: {:?}",
        reopened_layer.text
    );
    let reopened_pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let reopened_diff = diff_from(&baseline, &reopened_pixels);
    let (live_x, live_y) = replaced_diff.center();
    let (saved_x, saved_y) = reopened_diff.center();
    assert_close(saved_x, live_x, 3.0, "saved Chinese watermark center x");
    assert_close(saved_y, live_y, 3.0, "saved Chinese watermark center y");
    close_document(reopened.document_id).expect("close reopened document");
}

fn active_text(entry: &WatermarkEditResult) -> Option<&str> {
    entry
        .watermark
        .as_ref()
        .and_then(|watermark| watermark.text.as_deref())
}

#[test]
fn watermark_apply_replace_and_delete_are_undoable_and_redoable() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "ONE"),
    )
    .expect("apply first watermark");
    let replaced = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "TWO"),
    )
    .expect("replace watermark");
    assert_eq!(active_text(&replaced), Some("TWO"));
    assert_eq!(replaced.history.revision, 2);

    let undo_replace = undo_text_edit(document_id.clone()).expect("undo replacement");
    assert_eq!(undo_replace.history.revision, 1);
    assert!(undo_replace.history.can_undo);
    assert!(undo_replace.history.can_redo);
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state after undo")
            .as_ref()
            .and_then(|watermark| watermark.text.as_deref()),
        Some("ONE"),
        "undo restores the previous watermark"
    );
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("text after undo")
        .text;
    assert!(text.contains("ONE") && !text.contains("TWO"), "{text:?}");

    let undo_add = undo_text_edit(document_id.clone()).expect("undo first watermark");
    assert_eq!(undo_add.history.revision, 0);
    assert!(!undo_add.history.can_undo);
    assert!(!undo_add.history.dirty);
    assert!(
        watermark_state(document_id.clone())
            .expect("state after undoing add")
            .is_none(),
        "undo of the first apply removes the watermark"
    );
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("text after undoing add")
        .text;
    assert!(!text.contains("ONE") && !text.contains("TWO"), "{text:?}");

    let redo_add = redo_text_edit(document_id.clone()).expect("redo first watermark");
    assert_eq!(redo_add.history.revision, 1);
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state after redo add")
            .as_ref()
            .and_then(|watermark| watermark.text.as_deref()),
        Some("ONE")
    );
    let redo_replace = redo_text_edit(document_id.clone()).expect("redo replacement");
    assert_eq!(redo_replace.history.revision, 2);
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state after redo replace")
            .as_ref()
            .and_then(|watermark| watermark.text.as_deref()),
        Some("TWO")
    );
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("text after redo")
        .text;
    assert!(text.contains("TWO") && !text.contains("ONE"), "{text:?}");

    let deleted = delete_watermark(document_id.clone()).expect("delete watermark");
    assert!(deleted.watermark.is_none());
    assert_eq!(deleted.history.revision, 3);
    assert!(deleted.history.dirty);
    assert!(watermark_state(document_id.clone())
        .expect("state after delete")
        .is_none());
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("text after delete")
        .text;
    assert!(!text.contains("TWO"), "{text:?}");

    let undo_delete = undo_text_edit(document_id.clone()).expect("undo delete");
    assert_eq!(undo_delete.history.revision, 2);
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state after undo delete")
            .as_ref()
            .and_then(|watermark| watermark.text.as_deref()),
        Some("TWO"),
        "undoing delete restores the deleted watermark"
    );

    // Deleting again after a new edit clears the redo branch.
    delete_watermark(document_id.clone()).expect("delete again");
    assert!(
        !edit_history(document_id.clone()).expect("history").can_redo,
        "a new action clears redo"
    );
    close_document(document_id).expect("close document");
}

#[test]
fn reapplying_the_identical_watermark_is_a_noop_not_a_second_layer() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let applied = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "SAME"),
    )
    .expect("apply watermark");
    assert_eq!(applied.history.revision, 1);
    let painted = render_pixels(&document_id, 0, 1.0);

    let repeated = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "SAME"),
    )
    .expect("re-apply the same watermark");
    assert_eq!(
        repeated.history.revision, 1,
        "an identical re-application must not add a history action"
    );
    let after = render_pixels(&document_id, 0, 1.0);
    assert_eq!(
        diff_from(&painted, &after).count,
        0,
        "an identical re-application must not stack a second mark"
    );
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert_eq!(
        text.matches("SAME").count(),
        1,
        "the mark appears once: {text:?}"
    );
    close_document(document_id).expect("close document");
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

#[test]
fn all_pages_scope_uses_visible_logical_pages_only() {
    let pdf = synthetic::page_operations_pdf();
    let document_id = open_synthetic(&pdf);

    // Hide the middle page; "all pages" must skip its physical page.
    pdfx_lib::page_ops::delete_page(document_id.clone(), 1).expect("delete middle page");
    let applied = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "SCOPE"),
    )
    .expect("apply to all visible pages");
    assert_eq!(
        applied.watermark.as_ref().expect("active").page_indices,
        vec![0, 1],
        "only visible logical slots carry the watermark"
    );
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract visible page")
            .text;
        assert!(text.contains("SCOPE"), "page {logical}: {text:?}");
    }

    // Undoing both actions and redoing them keeps the hidden page out of the
    // watermark: when it is briefly visible again after undoing the deletion,
    // it carries no mark, and the redone watermark again targets only the two
    // visible slots.
    undo_text_edit(document_id.clone()).expect("undo watermark");
    undo_text_edit(document_id.clone()).expect("undo page deletion");
    let restored_text = extract_text_layer(document_id.clone(), 1)
        .expect("extract restored page")
        .text;
    assert!(
        !restored_text.contains("SCOPE"),
        "a deleted logical page must not receive a watermark: {restored_text:?}"
    );
    redo_text_edit(document_id.clone()).expect("redo page deletion");
    redo_text_edit(document_id.clone()).expect("redo watermark");
    let redone = watermark_state(document_id.clone())
        .expect("state after redo")
        .expect("watermark is active again");
    assert_eq!(redone.page_indices, vec![0, 1]);

    // Re-applying to all replaces the old mark on every visible page instead
    // of stacking a second copy.
    let replaced = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "SECOND"),
    )
    .expect("apply to all pages again");
    assert_eq!(
        replaced.watermark.as_ref().expect("active").page_indices,
        vec![0, 1]
    );
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert!(
            text.contains("SECOND") && !text.contains("SCOPE"),
            "page {logical} must carry only the replacement: {text:?}"
        );
    }
    close_document(document_id).expect("close document");
}

fn fixture_document(id: &str) -> String {
    open_document(fixture_path(id), None)
        .expect("open fixture")
        .document_id
}

#[test]
fn all_pages_watermark_is_upright_and_centered_on_mixed_sizes_and_rotations() {
    let document_id = fixture_document("rotated-mixed-size");
    let infos: Vec<_> = (0..3)
        .map(|page_index| page_info(document_id.clone(), page_index).expect("page info"))
        .collect();
    let baselines: Vec<_> = (0..3)
        .map(|page_index| render_pixels(&document_id, page_index, 1.0))
        .collect();

    let applied = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "MARK"),
    )
    .expect("apply to every page");
    assert_eq!(
        applied.watermark.as_ref().expect("active").page_indices,
        vec![0, 1, 2]
    );
    assert_eq!(
        applied.watermark.as_ref().expect("active").scope,
        WatermarkScope::All
    );

    let mut live_diffs = Vec::new();
    for (page_index, (info, baseline)) in infos.iter().zip(&baselines).enumerate() {
        let pixels = render_pixels(&document_id, page_index as u32, 1.0);
        let diff = diff_from(baseline, &pixels);
        assert!(
            diff.count > 60,
            "page {page_index} must paint a watermark, saw {}",
            diff.count
        );
        let (center_x, center_y) = diff.center();
        assert_close(
            center_x,
            info.width / 2.0,
            info.width * 0.03,
            &format!("page {page_index} watermark center x"),
        );
        assert_close(
            center_y,
            info.height / 2.0,
            info.height * 0.03,
            &format!("page {page_index} watermark center y"),
        );
        assert!(
            diff.width() > diff.height(),
            "page {page_index} (rotation {}) must show upright horizontal text: {}x{}",
            info.rotation,
            diff.width(),
            diff.height()
        );
        live_diffs.push(diff);
    }
    let ratios: Vec<f32> = live_diffs
        .iter()
        .enumerate()
        .map(|(page_index, diff)| {
            diff.height() as f32 / infos[page_index].width.min(infos[page_index].height)
        })
        .collect();
    let min_ratio = ratios.iter().copied().fold(f32::INFINITY, f32::min);
    let max_ratio = ratios.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_ratio - min_ratio <= 0.02,
        "the mark keeps its relative size across page sizes and rotations: {ratios:?}"
    );

    let dir = TempDir::new("watermark-rotated-save");
    let output = dir.path().join("rotated.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    for (page_index, (info, baseline)) in infos.iter().zip(&baselines).enumerate() {
        let saved_info =
            page_info(reopened.document_id.clone(), page_index as u32).expect("saved page info");
        assert_eq!(
            (saved_info.width, saved_info.height),
            (info.width, info.height)
        );
        assert_eq!(saved_info.rotation, info.rotation);
        let pixels = render_pixels(&reopened.document_id, page_index as u32, 1.0);
        let diff = diff_from(baseline, &pixels);
        assert!(
            diff.count > 60,
            "saved page {page_index} still paints the watermark"
        );
        let live = &live_diffs[page_index];
        let (saved_x, saved_y) = diff.center();
        let (live_x, live_y) = live.center();
        assert_close(saved_x, live_x, 3.0, "saved center x");
        assert_close(saved_y, live_y, 3.0, "saved center y");
        assert_close(
            diff.height() as f32,
            live.height() as f32,
            2.0,
            "saved mark height",
        );
        assert!(
            (i32::from(diff.min_luminance) - i32::from(live.min_luminance)).abs() <= 24,
            "saved opacity matches the live render on page {page_index}"
        );
    }
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn rotation_position_opacity_and_scale_change_the_rendered_mark() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);

    let mut first = text_request(WatermarkScope::Current, "MARK");
    first.rotation = 0.0;
    first.scale = 50.0;
    first.opacity = 40;
    first.position_x = 0.25;
    first.position_y = 0.25;
    apply_watermark(document_id.clone(), 0, first).expect("apply first configuration");
    let first_pixels = render_pixels(&document_id, 0, 1.0);
    let first_diff = diff_from(&baseline, &first_pixels);
    assert!(first_diff.count > 60);
    let (first_x, first_y) = first_diff.center();
    assert_close(first_x, PAGE_WIDTH * 0.25, 6.0, "first center x");
    assert_close(first_y, PAGE_HEIGHT * 0.25, 6.0, "first center y");
    assert!(
        first_diff.width() > first_diff.height(),
        "unrotated text is horizontal"
    );

    let mut second = text_request(WatermarkScope::Current, "MARK");
    second.rotation = 90.0;
    second.scale = 100.0;
    second.opacity = 100;
    second.position_x = 0.75;
    second.position_y = 0.75;
    apply_watermark(document_id.clone(), 0, second).expect("replace with second configuration");
    let second_pixels = render_pixels(&document_id, 0, 1.0);
    let second_diff = diff_from(&baseline, &second_pixels);
    let (second_x, second_y) = second_diff.center();
    assert_close(second_x, PAGE_WIDTH * 0.75, 6.0, "second center x");
    assert_close(second_y, PAGE_HEIGHT * 0.75, 6.0, "second center y");
    assert!(
        second_diff.height() > second_diff.width(),
        "90-degree text is vertical: {}x{}",
        second_diff.width(),
        second_diff.height()
    );
    assert_close(
        second_diff.height() as f32,
        first_diff.width() as f32 * 2.0,
        12.0,
        "doubling the scale doubles the mark length",
    );
    assert!(
        second_diff.min_luminance + 50 < first_diff.min_luminance,
        "full opacity is darker than 40%: {} vs {}",
        second_diff.min_luminance,
        first_diff.min_luminance
    );

    let dir = TempDir::new("watermark-adjust-save");
    let output = dir.path().join("adjusted.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    let saved_pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let saved_diff = diff_from(&baseline, &saved_pixels);
    let (saved_x, saved_y) = saved_diff.center();
    assert_close(saved_x, second_x, 3.0, "saved rotated center x");
    assert_close(saved_y, second_y, 3.0, "saved rotated center y");
    assert_close(
        saved_diff.height() as f32,
        second_diff.height() as f32,
        2.0,
        "saved rotated length",
    );
    assert!(
        (i32::from(saved_diff.min_luminance) - i32::from(second_diff.min_luminance)).abs() <= 24,
        "saved opacity matches: live {}, saved {}",
        second_diff.min_luminance,
        saved_diff.min_luminance
    );
    close_document(reopened.document_id).expect("close reopened document");
}

/// A solid image watermark must be real page content: the saved PDF carries
/// an image XObject, the live and saved renders show the same semi-transparent
/// rectangle, and no annotation is involved.
#[test]
fn image_watermark_is_real_page_content_with_opacity_and_survives_save() {
    let pdf = synthetic::single_line_pdf();
    let source_path = Path::new(&pdf.path_string()).to_path_buf();
    let source_bytes = std::fs::read(&source_path).expect("read source");
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);
    let dir = TempDir::new("watermark-image");
    let path = write_image(&dir, "logo.png", &solid_png(200, 100, RED));

    let applied = apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &path, 50),
    )
    .expect("apply image watermark");
    assert_eq!(applied.history.revision, 1);
    assert!(applied.history.dirty);
    let watermark = applied.watermark.expect("a watermark is active");
    assert_eq!(watermark.kind, WatermarkKind::Image);
    assert_eq!(watermark.image_name.as_deref(), Some("logo.png"));
    assert_eq!(watermark.image_path.as_deref(), Some(path.as_str()));
    assert_eq!(watermark.opacity, 50);
    assert_eq!(watermark.page_indices, vec![0]);
    assert!(
        list_annotations(document_id.clone())
            .expect("list annotations")
            .is_empty(),
        "an image watermark must not become a native annotation"
    );

    // A 2:1 image at 100% fills 60% of the page width and keeps its aspect.
    let pixels = render_pixels(&document_id, 0, 1.0);
    let diff = diff_from(&baseline, &pixels);
    assert!(
        diff.count > 2000,
        "the image watermark paints a large visible area, saw {} pixels",
        diff.count
    );
    assert_close(diff.width() as f32, PAGE_WIDTH * 0.6, 4.0, "image width");
    assert_close(
        diff.height() as f32,
        PAGE_WIDTH * 0.6 / 2.0,
        4.0,
        "image height",
    );
    let (center_x, center_y) = diff.center();
    assert_close(center_x, PAGE_WIDTH / 2.0, 3.0, "image center x");
    assert_close(center_y, PAGE_HEIGHT / 2.0, 3.0, "image center y");

    // 50% opacity over white: each channel is halfway to the source colour.
    let (red, green, blue) = page_pixel(&pixels, PAGE_WIDTH / 2.0, PAGE_HEIGHT / 2.0);
    for (channel, expected, name) in [
        (red, 237.5, "red"),
        (green, 142.5, "green"),
        (blue, 147.5, "blue"),
    ] {
        assert!(
            (f32::from(channel) - expected).abs() <= 14.0,
            "{name} channel should be blended at 50% opacity, got ({red}, {green}, {blue})"
        );
    }
    // Far outside the image box the page is untouched.
    assert_eq!(
        page_pixel(&pixels, 20.0, PAGE_HEIGHT - 20.0),
        page_pixel(&baseline, 20.0, PAGE_HEIGHT - 20.0),
        "the watermark must not tint the whole page"
    );

    let dir = TempDir::new("watermark-image-save");
    let output = dir.path().join("watermarked-image.pdf");
    save_as(&document_id, &output);
    assert_eq!(
        std::fs::read(&source_path).expect("source still readable"),
        source_bytes,
        "saving must never rewrite the Source Document"
    );
    let saved = lopdf::Document::load(&output).expect("load saved PDF");
    let image_ids = image_xobject_ids(&saved);
    assert_eq!(
        image_ids.len(),
        2,
        "the saved file contains the image and its alpha mask, not a duplicate: {image_ids:?}"
    );
    let masked = image_ids
        .iter()
        .filter(|id| {
            matches!(
                saved.get_object(**id),
                Ok(lopdf::Object::Stream(stream)) if stream.dict.get(b"SMask").is_ok()
            )
        })
        .count();
    assert_eq!(
        masked, 1,
        "exactly one colour image carries the opacity mask"
    );
    assert!(
        !page_content(&saved).is_empty(),
        "the image watermark is part of the page content stream"
    );

    // A repeated save stays stable: no duplicate image object or content.
    let first_objects = saved.objects.len();
    let first_content = page_content(&saved);
    save_as(&document_id, &output);
    let repeated = lopdf::Document::load(&output).expect("load repeated save");
    assert_eq!(repeated.objects.len(), first_objects);
    assert_eq!(page_content(&repeated), first_content);
    assert_eq!(image_xobject_count(&repeated), image_ids.len());

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    assert!(list_annotations(reopened.document_id.clone())
        .expect("list saved annotations")
        .is_empty());
    let saved_pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let saved_diff = diff_from(&baseline, &saved_pixels);
    assert!(
        saved_diff.count > 2000,
        "the saved image watermark renders after reopening, saw {}",
        saved_diff.count
    );
    let (saved_x, saved_y) = saved_diff.center();
    assert_close(saved_x, center_x, 3.0, "saved image center x");
    assert_close(saved_y, center_y, 3.0, "saved image center y");
    let (saved_red, saved_green, saved_blue) =
        page_pixel(&saved_pixels, PAGE_WIDTH / 2.0, PAGE_HEIGHT / 2.0);
    for (live, saved_channel, name) in [
        (red, saved_red, "red"),
        (green, saved_green, "green"),
        (blue, saved_blue, "blue"),
    ] {
        assert!(
            i32::from(live).abs_diff(i32::from(saved_channel)) <= 24,
            "{name} channel survives save/reopen: live {live}, saved {saved_channel}"
        );
    }
    close_document(reopened.document_id).expect("close reopened document");
}

#[test]
fn image_watermark_apply_delete_and_undo_redo_restore_the_mark() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);
    let dir = TempDir::new("watermark-image-history");
    let path = write_image(&dir, "stamp.png", &solid_png(180, 120, RED));

    let mut request = image_request(WatermarkScope::Current, &path, 70);
    request.rotation = 30.0;
    request.scale = 60.0;
    request.position_x = 0.3;
    request.position_y = 0.7;
    apply_watermark(document_id.clone(), 0, request).expect("apply image watermark");
    let painted = render_pixels(&document_id, 0, 1.0);
    let painted_diff = diff_from(&baseline, &painted);
    assert!(painted_diff.count > 500);
    let (painted_x, painted_y) = painted_diff.center();
    assert_close(painted_x, PAGE_WIDTH * 0.3, 6.0, "rotated image center x");
    assert_close(painted_y, PAGE_HEIGHT * 0.7, 6.0, "rotated image center y");
    assert!(!watermark_state(document_id.clone())
        .expect("state")
        .expect("active")
        .page_indices
        .is_empty());

    let undone = undo_text_edit(document_id.clone()).expect("undo image watermark");
    assert_eq!(undone.history.revision, 0);
    assert!(watermark_state(document_id.clone())
        .expect("state after undo")
        .is_none());
    let after_undo = render_pixels(&document_id, 0, 1.0);
    assert_eq!(
        diff_from(&baseline, &after_undo).count,
        0,
        "undo removes the image pixels entirely"
    );

    let redone = redo_text_edit(document_id.clone()).expect("redo image watermark");
    assert_eq!(redone.history.revision, 1);
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state after redo")
            .and_then(|watermark| watermark.image_name),
        Some("stamp.png".to_owned())
    );
    let after_redo = render_pixels(&document_id, 0, 1.0);
    let redone_diff = diff_from(&baseline, &after_redo);
    assert_close(
        redone_diff.center().0,
        painted_x,
        2.0,
        "redo restores the same position x",
    );
    assert_close(
        redone_diff.center().1,
        painted_y,
        2.0,
        "redo restores the same position y",
    );

    let deleted = delete_watermark(document_id.clone()).expect("delete image watermark");
    assert!(deleted.watermark.is_none());
    assert_eq!(deleted.history.revision, 2);
    assert_eq!(
        diff_from(&baseline, &render_pixels(&document_id, 0, 1.0)).count,
        0
    );

    undo_text_edit(document_id.clone()).expect("undo image delete");
    let restored = watermark_state(document_id.clone())
        .expect("state after undo delete")
        .expect("image watermark restored");
    assert_eq!(restored.image_name.as_deref(), Some("stamp.png"));
    assert_eq!(restored.rotation, 30.0);
    assert_eq!(restored.scale, 60.0);
    assert_eq!(restored.opacity, 70);
    assert!(diff_from(&baseline, &render_pixels(&document_id, 0, 1.0)).count > 500);
    close_document(document_id).expect("close document");
}

#[test]
fn image_preview_returns_the_chosen_file_and_bad_files_are_rejected() {
    let dir = TempDir::new("watermark-image-preview");
    let png = solid_png(64, 32, RED);
    let path = write_image(&dir, "stamp.png", &png);

    let preview = watermark_image_preview(path.clone()).expect("preview a PNG");
    assert_eq!(preview.name, "stamp.png");
    assert_eq!(preview.mime_type, "image/png");
    assert_eq!((preview.width, preview.height), (64, 32));
    let encoded = preview
        .data_url
        .strip_prefix("data:image/png;base64,")
        .expect("the preview is a PNG data URL");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("preview payload is base64");
    assert_eq!(decoded, png, "the preview shows the chosen file's bytes");

    let text_path = write_image(&dir, "notes.txt", b"not an image");
    let unsupported = watermark_image_preview(text_path).expect_err("reject other extensions");
    assert_eq!(unsupported.code, CommandErrorCode::UnsupportedImageFormat);

    let missing = watermark_image_preview(
        dir.path()
            .join("missing.png")
            .to_string_lossy()
            .into_owned(),
    )
    .expect_err("reject a missing file");
    assert_eq!(missing.code, CommandErrorCode::FileNotFound);

    // Applying a bad image fails before the document or history is touched.
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let bad_path = write_image(&dir, "broken.png", b"not a PNG");
    let error = apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &bad_path, 100),
    )
    .expect_err("a corrupt image must be rejected");
    assert_eq!(error.code, CommandErrorCode::UnsupportedImageFormat);
    assert!(watermark_state(document_id.clone())
        .expect("state after rejection")
        .is_none());
    let history = edit_history(document_id.clone()).expect("history after rejection");
    assert_eq!(history.revision, 0);
    assert!(!history.dirty);
    close_document(document_id).expect("close document");
}

/// The live canvas, thumbnails, print surface, page image export and flattened
/// copy all render the same page content, so one render-based assertion covers
/// them all. The flattened copy is reopened to prove the mark is in the file.
#[test]
fn watermark_survives_page_image_and_flattened_exports() {
    let pdf = synthetic::single_line_pdf();
    let source_path = Path::new(&pdf.path_string()).to_path_buf();
    let source_bytes = std::fs::read(&source_path).expect("read source");
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);
    let dir = TempDir::new("watermark-export");
    let image_path = write_image(&dir, "export.png", &solid_png(200, 100, RED));
    apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &image_path, 100),
    )
    .expect("apply image watermark");
    let canvas = render_pixels(&document_id, 0, 1.0);
    assert!(diff_from(&baseline, &canvas).count > 2000);

    // The print surface renders these same canvases, so a PNG export at the
    // canvas scale must be byte-identical to the rendered page.
    let png_path = dir.path().join("page.png");
    export_page(
        document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        png_path.to_string_lossy().into_owned(),
    )
    .expect("export PNG");
    let canvas_png = base64::engine::general_purpose::STANDARD
        .decode(
            render_page(document_id.clone(), 0, 1.0)
                .expect("render canvas")
                .png_base64,
        )
        .expect("canvas base64");
    assert_eq!(
        std::fs::read(&png_path).expect("read exported PNG"),
        canvas_png,
        "the PNG export matches the canvas including the watermark"
    );

    let jpg_path = dir.path().join("page.jpg");
    export_page(
        document_id.clone(),
        0,
        PageImageFormat::Jpg,
        150,
        jpg_path.to_string_lossy().into_owned(),
    )
    .expect("export JPEG");
    let jpg = image::load_from_memory(&std::fs::read(&jpg_path).expect("read exported JPEG"))
        .expect("decode exported JPEG")
        .to_rgba8();
    let (width, height) = jpg.dimensions();
    let center = jpg.get_pixel(width / 2, height / 2);
    assert!(
        center[0] > 170 && center[1] < 110 && center[2] < 110,
        "the JPEG export shows the red watermark at the page center: {center:?}"
    );

    let flatten_path = dir.path().join("flattened.pdf");
    export_flattened_copy(
        document_id.clone(),
        flatten_path.to_string_lossy().into_owned(),
        false,
    )
    .expect("export a flattened copy");
    assert_eq!(
        std::fs::read(&source_path).expect("re-read source"),
        source_bytes,
        "exporting must never modify the Source Document"
    );

    close_document(document_id).expect("close live document");
    let reopened = open_document(flatten_path.to_string_lossy().into_owned(), None)
        .expect("reopen the flattened copy");
    assert!(
        list_annotations(reopened.document_id.clone())
            .expect("list flattened annotations")
            .is_empty(),
        "the watermark stays page content in the flattened copy"
    );
    let flattened_pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let flattened_diff = diff_from(&baseline, &flattened_pixels);
    assert!(
        flattened_diff.count > 2000,
        "the flattened copy renders the watermark, saw {}",
        flattened_diff.count
    );
    let (flattened_x, flattened_y) = flattened_diff.center();
    let (canvas_x, canvas_y) = diff_from(&baseline, &canvas).center();
    assert_close(flattened_x, canvas_x, 3.0, "flattened watermark center x");
    assert_close(flattened_y, canvas_y, 3.0, "flattened watermark center y");
    close_document(reopened.document_id).expect("close flattened copy");
}

#[test]
fn image_watermark_scale_changes_the_rendered_size() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);
    let dir = TempDir::new("watermark-image-scale");
    let path = write_image(&dir, "scale.png", &solid_png(200, 100, RED));

    apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &path, 100),
    )
    .expect("apply at full scale");
    let full = diff_from(&baseline, &render_pixels(&document_id, 0, 1.0));
    assert_close(full.width() as f32, PAGE_WIDTH * 0.6, 4.0, "100% width");

    let mut half = image_request(WatermarkScope::Current, &path, 100);
    half.scale = 50.0;
    apply_watermark(document_id.clone(), 0, half).expect("replace at half scale");
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state")
            .expect("active")
            .scale,
        50.0
    );
    let reduced = diff_from(&baseline, &render_pixels(&document_id, 0, 1.0));
    assert_close(reduced.width() as f32, PAGE_WIDTH * 0.3, 4.0, "50% width");
    assert_close(
        reduced.height() as f32,
        PAGE_WIDTH * 0.3 / 2.0,
        4.0,
        "50% height",
    );
    assert!(reduced.count > 500);
    close_document(document_id).expect("close document");
}

#[test]
fn image_watermark_keeps_its_shape_upright_on_rotated_mixed_size_pages() {
    let document_id = fixture_document("rotated-mixed-size");
    let infos: Vec<_> = (0..3)
        .map(|page_index| page_info(document_id.clone(), page_index).expect("page info"))
        .collect();
    let baselines: Vec<_> = (0..3)
        .map(|page_index| render_pixels(&document_id, page_index, 1.0))
        .collect();
    let dir = TempDir::new("watermark-image-rotated");
    let path = write_image(&dir, "wide.png", &solid_png(200, 100, RED));

    let applied = apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::All, &path, 100),
    )
    .expect("apply image to every page");
    assert_eq!(
        applied.watermark.as_ref().expect("active").page_indices,
        vec![0, 1, 2]
    );

    for (page_index, (info, baseline)) in infos.iter().zip(&baselines).enumerate() {
        let pixels = render_pixels(&document_id, page_index as u32, 1.0);
        let diff = diff_from(baseline, &pixels);
        assert!(
            diff.count > 1200,
            "page {page_index} paints the image watermark, saw {}",
            diff.count
        );
        let expected_width = 0.6 * info.width.min(info.height * 2.0);
        assert_close(
            diff.width() as f32,
            expected_width,
            4.0,
            &format!("page {page_index} image width"),
        );
        assert_close(
            diff.height() as f32,
            expected_width / 2.0,
            4.0,
            &format!("page {page_index} image height"),
        );
        assert!(
            diff.width() > diff.height(),
            "page {page_index} (rotation {}) shows the landscape image upright",
            info.rotation
        );
        let (center_x, center_y) = diff.center();
        assert_close(
            center_x,
            info.width / 2.0,
            3.0,
            &format!("page {page_index} image center x"),
        );
        assert_close(
            center_y,
            info.height / 2.0,
            3.0,
            &format!("page {page_index} image center y"),
        );
    }
    close_document(document_id).expect("close document");
}

#[test]
fn all_pages_watermark_covers_a_large_document_and_undoes_as_one_action() {
    let document_id = fixture_document("large-500-pages");
    let applied = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "BULK"),
    )
    .expect("watermark every page");
    let watermark = applied.watermark.expect("active watermark");
    assert_eq!(watermark.page_indices.len(), 500);
    assert_eq!(watermark.page_indices.first(), Some(&0));
    assert_eq!(watermark.page_indices.last(), Some(&499));
    for page_index in [0u32, 250, 499] {
        let text = extract_text_layer(document_id.clone(), page_index)
            .expect("extract a page")
            .text;
        assert!(text.contains("BULK"), "page {page_index}: {text:?}");
    }
    assert_eq!(applied.history.revision, 1);

    let undone = undo_text_edit(document_id.clone()).expect("undo the bulk watermark");
    assert_eq!(undone.history.revision, 0);
    assert!(watermark_state(document_id.clone())
        .expect("state after undo")
        .is_none());
    let text = extract_text_layer(document_id.clone(), 499)
        .expect("extract last page")
        .text;
    assert!(!text.contains("BULK"), "undo removes every page: {text:?}");
    close_document(document_id).expect("close document");
}

#[test]
fn saving_the_watermark_clears_dirty_and_undoing_makes_it_dirty_again() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "SAVED"),
    )
    .expect("apply watermark");
    assert!(edit_history(document_id.clone()).expect("history").dirty);

    let dir = TempDir::new("watermark-dirty");
    let output = dir.path().join("dirty.pdf");
    save_as(&document_id, &output);
    let saved = edit_history(document_id.clone()).expect("history after save");
    assert!(!saved.dirty, "saving establishes a clean baseline");
    assert!(saved.can_undo, "the saved watermark remains undoable");

    undo_text_edit(document_id.clone()).expect("undo past the save point");
    assert!(
        edit_history(document_id.clone())
            .expect("history after undo")
            .dirty,
        "undoing past the saved state makes the document dirty"
    );
    redo_text_edit(document_id.clone()).expect("redo to the saved state");
    assert!(
        !edit_history(document_id.clone())
            .expect("history after redo")
            .dirty,
        "redoing back to the saved state is clean again"
    );
    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_watermark_changes_without_history() {
    let document_id = fixture_document("encrypted-owner-restrictions");
    let before = edit_history(document_id.clone()).expect("initial history");
    assert!(!before.can_undo);
    assert_eq!(before.revision, 0);

    let error = apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "DENIED"),
    )
    .expect_err("a read-only document must refuse the watermark");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(watermark_state(document_id.clone())
        .expect("state after refusal")
        .is_none());
    let after = edit_history(document_id.clone()).expect("history after refusal");
    assert!(!after.can_undo);
    assert!(!after.can_redo);
    assert!(!after.dirty);
    assert_eq!(after.revision, 0, "a refusal must not advance history");

    let dir = TempDir::new("watermark-read-only-image");
    let image_path = write_image(&dir, "denied.png", &solid_png(32, 16, RED));
    let image_error = apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &image_path, 100),
    )
    .expect_err("a read-only document must refuse an image watermark too");
    assert_eq!(image_error.code, CommandErrorCode::PermissionDenied);
    assert!(watermark_state(document_id.clone())
        .expect("state after image refusal")
        .is_none());
    assert_eq!(
        edit_history(document_id.clone())
            .expect("history after image refusal")
            .revision,
        0
    );

    let delete_error = delete_watermark(document_id.clone()).expect_err("delete is refused too");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);
    assert_eq!(
        edit_history(document_id.clone())
            .expect("history after delete refusal")
            .revision,
        0
    );
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_watermark_requests_are_rejected_without_history() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let mut cases: Vec<(&str, WatermarkRequest)> = Vec::new();
    let mut empty = text_request(WatermarkScope::Current, "   ");
    cases.push(("empty text", empty.clone()));
    empty.text = Some("字".repeat(201));
    cases.push(("text above the character cap", empty.clone()));
    let mut newline = text_request(WatermarkScope::Current, "line");
    newline.text = Some("two\nlines".to_owned());
    cases.push(("text with a line break", newline));
    for (name, mutate) in [
        (
            "opacity zero",
            Box::new(|r: &mut WatermarkRequest| r.opacity = 0)
                as Box<dyn Fn(&mut WatermarkRequest)>,
        ),
        (
            "opacity above 100",
            Box::new(|r: &mut WatermarkRequest| r.opacity = 101),
        ),
        (
            "rotation above 180",
            Box::new(|r: &mut WatermarkRequest| r.rotation = 181.0),
        ),
        (
            "rotation below -180",
            Box::new(|r: &mut WatermarkRequest| r.rotation = -181.0),
        ),
        (
            "scale below 10",
            Box::new(|r: &mut WatermarkRequest| r.scale = 9.0),
        ),
        (
            "scale above 400",
            Box::new(|r: &mut WatermarkRequest| r.scale = 401.0),
        ),
        (
            "position x below 0",
            Box::new(|r: &mut WatermarkRequest| r.position_x = -0.01),
        ),
        (
            "position y above 1",
            Box::new(|r: &mut WatermarkRequest| r.position_y = 1.01),
        ),
    ] {
        let mut request = text_request(WatermarkScope::Current, "VALID");
        mutate(&mut request);
        cases.push((name, request));
    }
    let mut missing_image = text_request(WatermarkScope::Current, "");
    missing_image.kind = WatermarkKind::Image;
    missing_image.text = None;
    missing_image.image_path = None;
    cases.push(("image without a path", missing_image));

    for (name, request) in cases {
        let error = apply_watermark(document_id.clone(), 0, request)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected a rejection"));
        assert_eq!(
            error.code,
            CommandErrorCode::InvalidArgument,
            "{name}: {error:?}"
        );
        assert!(
            watermark_state(document_id.clone())
                .expect("state after rejection")
                .is_none(),
            "{name}: a rejected request must not create a watermark"
        );
        let history = edit_history(document_id.clone()).expect("history after rejection");
        assert_eq!(history.revision, 0, "{name}: history must not advance");
        assert!(
            !history.dirty,
            "{name}: a rejection must not dirty the document"
        );
    }
    close_document(document_id).expect("close document");
}

/// Page duplication copies the mark as page content. The active watermark must
/// follow the copy so a later replace or delete removes it there too, and
/// undo/redo of both actions must stay consistent.
#[test]
fn copying_a_watermarked_page_keeps_the_copy_in_the_watermark() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);

    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "MARK"),
    )
    .expect("apply a watermark");
    let copied = copy_page(document_id.clone(), 0).expect("copy the marked page");
    assert_eq!((copied.page_count, copied.page_index), (2, 1));

    let copy_text = extract_text_layer(document_id.clone(), 1)
        .expect("extract the copy")
        .text;
    assert!(
        copy_text.contains("MARK"),
        "the copy must keep the page's watermark content: {copy_text:?}"
    );
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("read state")
            .expect("watermark is active")
            .page_indices,
        vec![0, 1],
        "the copied page is part of the one active watermark"
    );

    // A replace cleans the copy as well: no old mark may survive underneath.
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "SECOND"),
    )
    .expect("replace on every page");
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert!(
            text.contains("SECOND") && !text.contains("MARK"),
            "page {logical} must carry only the replacement: {text:?}"
        );
    }

    // Delete removes the mark from both pages, including the copy.
    delete_watermark(document_id.clone()).expect("delete the watermark");
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert!(!text.contains("SECOND"), "page {logical}: {text:?}");
        let diff = diff_from(&baseline, &render_pixels(&document_id, logical, 1.0));
        assert_eq!(
            diff.count, 0,
            "page {logical} still paints the deleted watermark"
        );
    }

    // Undo restores the mark on both pages; undo the copy and the mark again,
    // then redo in order.
    undo_text_edit(document_id.clone()).expect("undo delete");
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert!(text.contains("SECOND"), "page {logical}: {text:?}");
    }
    undo_text_edit(document_id.clone()).expect("undo replace");
    undo_text_edit(document_id.clone()).expect("undo first apply");
    undo_text_edit(document_id.clone()).expect("undo copy");
    assert!(watermark_state(document_id.clone())
        .expect("state after undo")
        .is_none());
    redo_text_edit(document_id.clone()).expect("redo copy");
    redo_text_edit(document_id.clone()).expect("redo first apply");
    let redone = watermark_state(document_id.clone())
        .expect("state after redo")
        .expect("watermark is active again");
    assert_eq!(redone.text.as_deref(), Some("MARK"));
    assert_eq!(redone.page_indices, vec![0, 1]);
    let copy_text = extract_text_layer(document_id.clone(), 1)
        .expect("extract copy")
        .text;
    assert!(copy_text.contains("MARK"), "{copy_text:?}");

    // The copy and its tracked mark survive saving and reopening.
    let dir = TempDir::new("watermark-copy-save");
    let output = dir.path().join("copied.pdf");
    save_as(&document_id, &output);
    let saved = lopdf::Document::load(&output).expect("parse saved copy");
    assert_eq!(saved.get_pages().len(), 2);
    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved copy");
    for logical in [0u32, 1] {
        let text = extract_text_layer(reopened.document_id.clone(), logical)
            .expect("extract saved page")
            .text;
        assert!(text.contains("MARK"), "saved page {logical}: {text:?}");
    }
    close_document(reopened.document_id).expect("close reopened document");
}

/// Text Box creation, deletion and undo move page objects around the mark. The
/// watermark's recorded object range must follow those shifts so delete and
/// undo/redo detach the mark itself, never a neighbouring object.
#[test]
fn watermark_object_ranges_follow_text_box_edits() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);

    let text_box = create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 40.0,
            y: 50.0,
            width: 220.0,
            height: 90.0,
            style: None,
        },
    )
    .expect("create a Text Box");
    let box_id = text_box.target.box_id;
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "MARK"),
    )
    .expect("apply the watermark after the Text Box");

    // Deleting the Text Box removes an object before the mark: the range must
    // shift down by one.
    delete_text_box(document_id.clone(), 0, box_id).expect("delete the Text Box");
    let deleted = delete_watermark(document_id.clone()).expect("delete the watermark");
    assert_eq!(deleted.history.revision, 4);
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(!text.contains("MARK"), "{text:?}");
    assert_eq!(
        diff_from(&baseline, &render_pixels(&document_id, 0, 1.0)).count,
        0,
        "no watermark pixels may remain after the shifted delete"
    );

    // Undo the delete, then the Text Box deletion: the restored Text Box
    // re-inserts at the front and shifts the mark back up. Deleting again must
    // still address the mark.
    undo_text_edit(document_id.clone()).expect("undo watermark delete");
    undo_text_edit(document_id.clone()).expect("undo Text Box deletion");
    let restored = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(restored.contains("MARK"), "{restored:?}");
    delete_watermark(document_id.clone()).expect("delete the shifted-in watermark");
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(!text.contains("MARK"), "{text:?}");

    close_document(document_id).expect("close document");
}

fn char_index_of(layer: &pdfx_lib::commands::TextLayerView, needle: &str) -> usize {
    let offset = layer.text.find(needle).expect("text in layer");
    layer.text[..offset].chars().count()
}

/// The watermark is page content, but it is not a Text Box: clicking its glyphs
/// in the text layer must not offer a text edit whose object range overlaps the
/// active watermark.
#[test]
fn watermark_text_is_not_editable_as_a_text_box() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "MARK"),
    )
    .expect("apply a watermark");

    let layer = extract_text_layer(document_id.clone(), 0).expect("extract text");
    let index = char_index_of(&layer, "MARK");
    let error = text_edit_target(document_id.clone(), 0, index as u32)
        .err()
        .expect("the watermark text must not become an editable Text Box");
    assert_eq!(error.code, CommandErrorCode::NoEditableText);

    // Even a direct text edit of the recorded box would be refused, and the
    // watermark remains deletable afterwards.
    let history = edit_history(document_id.clone()).expect("history");
    assert_eq!(history.revision, 1, "a refused target changes no history");
    delete_watermark(document_id.clone()).expect("delete the untouched watermark");
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(!text.contains("MARK"), "{text:?}");
    close_document(document_id).expect("close document");
}

/// A save rewrites the live PDFium page objects in place. Replacing and
/// deleting the watermark afterwards must still detach exactly the mark, and a
/// second save must not leave the old layer in the output.
#[test]
fn watermark_replace_and_delete_after_save_stay_consistent() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "BEFORE"),
    )
    .expect("apply before saving");

    let dir = TempDir::new("watermark-after-save");
    let output = dir.path().join("after-save.pdf");
    save_as(&document_id, &output);
    assert!(!edit_history(document_id.clone()).expect("history").dirty);

    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::Current, "AFTER"),
    )
    .expect("replace after saving");
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(
        text.contains("AFTER") && !text.contains("BEFORE"),
        "the post-save replacement must not stack: {text:?}"
    );

    save_as(&document_id, &output);

    // The watermark is still the live session's active mark after the save, so
    // it can be deleted and the deletion written by one more save.
    delete_watermark(document_id.clone()).expect("delete after save");
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(
        !text.contains("AFTER") && !text.contains("BEFORE"),
        "{text:?}"
    );
    let output_two = dir.path().join("after-delete.pdf");
    save_as(&document_id, &output_two);
    close_document(document_id).expect("close live document");

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    let saved_text = extract_text_layer(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text;
    assert!(
        saved_text.contains("AFTER") && !saved_text.contains("BEFORE"),
        "the saved file carries only the replacement: {saved_text:?}"
    );
    assert_eq!(
        saved_text.matches("AFTER").count(),
        1,
        "the replacement appears once: {saved_text:?}"
    );
    close_document(reopened.document_id).expect("close reopened document");

    let reopened_two =
        open_document(output_two.to_string_lossy().into_owned(), None).expect("reopen final save");
    let final_text = extract_text_layer(reopened_two.document_id.clone(), 0)
        .expect("extract final text")
        .text;
    assert!(
        !final_text.contains("AFTER") && !final_text.contains("BEFORE"),
        "the deleted watermark must not be restored by a later save: {final_text:?}"
    );
    close_document(reopened_two.document_id).expect("close final document");
}

/// Image watermarks copy with the page as an image page object. Deleting the
/// watermark must remove the image from the copy too.
#[test]
fn copying_an_image_watermarked_page_tracks_the_copy() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let baseline = render_pixels(&document_id, 0, 1.0);
    let dir = TempDir::new("watermark-image-copy");
    let path = write_image(&dir, "copy.png", &solid_png(120, 60, RED));

    apply_watermark(
        document_id.clone(),
        0,
        image_request(WatermarkScope::Current, &path, 100),
    )
    .expect("apply an image watermark");
    copy_page(document_id.clone(), 0).expect("copy the marked page");
    assert_eq!(
        watermark_state(document_id.clone())
            .expect("state")
            .expect("active")
            .page_indices,
        vec![0, 1]
    );
    delete_watermark(document_id.clone()).expect("delete the image watermark");
    for logical in [0u32, 1] {
        let diff = diff_from(&baseline, &render_pixels(&document_id, logical, 1.0));
        assert_eq!(
            diff.count, 0,
            "page {logical} still paints the deleted image watermark"
        );
    }
    close_document(document_id).expect("close document");
}

/// Page operations after a watermark move, hide and rotate physical pages.
/// The active watermark addresses physical pages, so delete/undo must still hit
/// the mark on the pages the user currently sees, and saving the rearranged
/// document must keep it on the same content.
#[test]
fn page_operations_after_a_watermark_keep_the_mark_targeted() {
    use pdfx_lib::page_ops::{delete_page, move_page, rotate_page};

    let pdf = synthetic::page_operations_pdf();
    let document_id = open_synthetic(&pdf);
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "MARK"),
    )
    .expect("apply to all pages");

    rotate_page(document_id.clone(), 0, 90).expect("rotate a marked page");
    move_page(document_id.clone(), 2, 0).expect("move a marked page");
    delete_page(document_id.clone(), 1).expect("hide a marked page");

    let state = watermark_state(document_id.clone())
        .expect("read state")
        .expect("watermark is active");
    assert_eq!(
        state.page_indices,
        vec![0, 1],
        "only the visible slots are reported after reorder and deletion"
    );

    delete_watermark(document_id.clone()).expect("delete after the page operations");
    for logical in [0u32, 1] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract visible page")
            .text;
        assert!(!text.contains("MARK"), "page {logical}: {text:?}");
    }

    // Undo in order: the mark returns before the hidden page becomes visible.
    undo_text_edit(document_id.clone()).expect("undo watermark delete");
    undo_text_edit(document_id.clone()).expect("undo page deletion");
    for logical in [0u32, 1, 2] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract restored page")
            .text;
        assert!(text.contains("MARK"), "restored page {logical}: {text:?}");
    }
    undo_text_edit(document_id.clone()).expect("undo move");
    undo_text_edit(document_id.clone()).expect("undo rotation");
    assert_eq!(
        page_info(document_id.clone(), 0)
            .expect("page info after undo")
            .rotation,
        0
    );

    // Redo the page operations and the rotate still carry the mark; saving the
    // rearranged document keeps it on the two visible pages.
    redo_text_edit(document_id.clone()).expect("redo rotation");
    redo_text_edit(document_id.clone()).expect("redo move");
    redo_text_edit(document_id.clone()).expect("redo page deletion");
    let state = watermark_state(document_id.clone())
        .expect("state after redo")
        .expect("active again");
    assert_eq!(state.page_indices, vec![0, 1]);

    let dir = TempDir::new("watermark-page-ops-save");
    let output = dir.path().join("page-ops.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let saved = lopdf::Document::load(&output).expect("parse saved rearranged document");
    assert_eq!(saved.get_pages().len(), 2);
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    for logical in [0u32, 1] {
        let text = extract_text_layer(reopened.document_id.clone(), logical)
            .expect("extract saved page")
            .text;
        assert!(text.contains("MARK"), "saved page {logical}: {text:?}");
    }
    close_document(reopened.document_id).expect("close reopened document");
}

/// Narrowing an all-pages watermark to the current page must remove the mark
/// from every other page, and undo must restore the earlier all-pages state.
#[test]
fn narrowing_the_scope_replaces_instead_of_leaving_old_marks() {
    let pdf = synthetic::page_operations_pdf();
    let document_id = open_synthetic(&pdf);
    apply_watermark(
        document_id.clone(),
        0,
        text_request(WatermarkScope::All, "ALL"),
    )
    .expect("apply to all pages");

    let narrowed = apply_watermark(
        document_id.clone(),
        1,
        text_request(WatermarkScope::Current, "ONE"),
    )
    .expect("narrow to the current page");
    assert_eq!(
        narrowed.watermark.as_ref().expect("active").page_indices,
        vec![1]
    );
    for (logical, expected) in [(0u32, false), (1, true), (2, false)] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert_eq!(
            text.contains("ONE"),
            expected,
            "page {logical} after narrowing: {text:?}"
        );
        assert!(
            !text.contains("ALL"),
            "page {logical} must not keep the all-pages mark: {text:?}"
        );
    }

    let undone = undo_text_edit(document_id.clone()).expect("undo narrowing");
    assert_eq!(undone.history.revision, 1);
    let restored = watermark_state(document_id.clone())
        .expect("state after undo")
        .expect("all-pages watermark restored");
    assert_eq!(restored.page_indices, vec![0, 1, 2]);
    for logical in [0u32, 1, 2] {
        let text = extract_text_layer(document_id.clone(), logical)
            .expect("extract page")
            .text;
        assert!(text.contains("ALL"), "page {logical}: {text:?}");
    }
    redo_text_edit(document_id.clone()).expect("redo narrowing");
    let text = extract_text_layer(document_id.clone(), 0)
        .expect("extract page")
        .text;
    assert!(!text.contains("ALL"), "{text:?}");
    close_document(document_id).expect("close document");
}
