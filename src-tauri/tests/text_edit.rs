//! Command-boundary tests for editing existing text and the shared undo/redo
//! history.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes; PDFium and other engine types never cross the seam. Documents are
//! reopened from bytes returned by the save pipeline so "the edit was really
//! written" is checked against a fresh parse, not just in-memory state.

mod support;

use support::fixtures;
use support::synthetic;

use pdfx_lib::commands::{
    close_document, extract_text, extract_text_layer, open_document, render_page,
    save_document_bytes, CommandErrorCode, TextLayerView,
};
use pdfx_lib::text_edit::{
    apply_text_edit, delete_text_box, edit_history, mark_document_saved, redo_text_edit,
    text_edit_target, undo_text_edit,
};

fn open_fixture(category: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, category).expect("fixture category");
    let path = fixtures::fixture_path(fixture);
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open fixture")
        .document_id
}

fn open_fixture_id(id: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id:?} is missing from the manifest"));
    let path = fixtures::fixture_path(fixture);
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open fixture")
        .document_id
}

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn char_index_of(layer: &TextLayerView, needle: &str) -> usize {
    let offset = layer.text.find(needle).expect("text in layer");
    layer.text[..offset].chars().count()
}

#[test]
fn clicking_text_prefills_the_original_style_content_and_position() {
    let document_id = open_fixture("text");
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let char_index = char_index_of(&layer, "PDFX fixture");

    let target = text_edit_target(document_id.clone(), 0, char_index as u32).expect("target");

    // Content and style come from the original Text Object, not from defaults.
    assert_eq!(target.text, "PDFX fixture: text layer.");
    assert_eq!(target.font_family, "Helvetica");
    assert!(
        (target.font_size - 18.0).abs() < 0.01,
        "font size was {}",
        target.font_size
    );
    assert_eq!(target.color, "#000000");
    assert!(target.box_id > 0, "box ids are stable and non-zero");

    // The Text Box covers the clicked glyph and starts where the fixture
    // paints the line: user-space x=72 on an unrotated 612x792 page.
    let clicked = &layer.chars[char_index];
    assert!(
        target.x <= clicked.x + 0.01 && target.y <= clicked.y + 0.01,
        "target origin ({}, {}) must contain the glyph box at ({}, {})",
        target.x,
        target.y,
        clicked.x,
        clicked.y
    );
    assert!(
        target.x + target.width >= clicked.x + clicked.width - 0.01,
        "target right edge {} must cover glyph right edge {}",
        target.x + target.width,
        clicked.x + clicked.width
    );
    assert!(
        target.y + target.height >= clicked.y + clicked.height - 0.01,
        "target bottom edge {} must cover glyph bottom edge {}",
        target.y + target.height,
        clicked.y + clicked.height
    );
    assert!((target.x - 72.0).abs() < 0.01, "x was {}", target.x);

    close_document(document_id).expect("close document");
}

#[test]
fn characters_resolve_to_their_own_text_object() {
    let document_id = open_fixture("text");
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    let first = char_index_of(&layer, "PDFX fixture");
    let later_in_line = char_index_of(&layer, "text layer") + 2;
    let second_line = char_index_of(&layer, "Latin text");

    let first_target =
        text_edit_target(document_id.clone(), 0, first as u32).expect("first target");
    let same_target =
        text_edit_target(document_id.clone(), 0, later_in_line as u32).expect("same-line target");
    let second_target =
        text_edit_target(document_id.clone(), 0, second_line as u32).expect("second-line target");

    assert_eq!(
        first_target.box_id, same_target.box_id,
        "characters on one Text Object share a Text Box"
    );
    assert_eq!(first_target.text, same_target.text);
    assert_eq!(first_target.text, "PDFX fixture: text layer.");

    assert_ne!(
        second_target.box_id, first_target.box_id,
        "a different Text Object is a different Text Box"
    );
    assert_eq!(
        second_target.text,
        "Latin text is extractable from this page."
    );

    // The second line sits below the first: 18pt text with 28pt line spacing.
    assert!(
        second_target.y > first_target.y + first_target.height,
        "second Text Box must start below the first: {} vs {}",
        second_target.y,
        first_target.y + first_target.height
    );

    close_document(document_id).expect("close document");
}

#[test]
fn target_lookup_reports_structured_errors() {
    let document_id = open_fixture("text");

    let error = text_edit_target("doc-missing".to_owned(), 0, 0).expect_err("unknown document");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);

    let error = text_edit_target(document_id.clone(), 9, 0).expect_err("page out of range");
    assert_eq!(error.code, CommandErrorCode::PageOutOfRange);

    let error = text_edit_target(document_id.clone(), 0, 100_000).expect_err("char out of range");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    close_document(document_id).expect("close document");
}

#[test]
fn scanned_pages_have_no_editable_text() {
    let document_id = open_fixture("scanned");

    let error = text_edit_target(document_id.clone(), 0, 0).expect_err("scanned page");
    assert_eq!(error.code, CommandErrorCode::NoEditableText);
    assert!(
        error.message.to_lowercase().contains("no editable text"),
        "message should explain the reason: {}",
        error.message
    );

    close_document(document_id).expect("close document");
}

fn first_origin(layer: &TextLayerView, needle: &str) -> (f32, f32) {
    let index = char_index_of(layer, needle);
    (layer.chars[index].origin_x, layer.chars[index].origin_y)
}

#[test]
fn applying_an_edit_replaces_the_original_text_everywhere() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "PDFX fixture") as u32,
    )
    .expect("target");

    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Edited headline".to_owned(),
    )
    .expect("apply text edit");

    assert_eq!(result.text, "Edited headline");
    assert_eq!(result.box_id, target.box_id);
    assert!(
        result.font.is_none(),
        "editing base-14 Helvetica needs no fallback: {:?}",
        result.font
    );

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer after edit");
    assert!(
        layer.text.contains("Edited headline"),
        "got {:?}",
        layer.text
    );
    assert!(
        !layer.text.contains("PDFX fixture: text layer."),
        "the original Text Object must be removed, got {:?}",
        layer.text
    );
    assert_eq!(
        layer.text.matches("Edited headline").count(),
        1,
        "the replacement must not be duplicated: {:?}",
        layer.text
    );

    let extracted = extract_text(document_id.clone(), 0).expect("extract text after edit");
    assert!(extracted.text.contains("Edited headline"));
    assert!(!extracted.text.contains("PDFX fixture"));

    // No paragraph reflow: untouched Text Objects keep their exact origins.
    for needle in [
        "Latin text is extractable from this page.",
        "The quick brown fox jumps over the lazy dog. (1234567890)",
    ] {
        assert!(layer.text.contains(needle), "missing {needle:?}");
        let before_origin = first_origin(&before, needle);
        let after_origin = first_origin(&layer, needle);
        assert!(
            (before_origin.0 - after_origin.0).abs() < 0.01
                && (before_origin.1 - after_origin.1).abs() < 0.01,
            "{needle:?} moved from {before_origin:?} to {after_origin:?}"
        );
    }

    // The edit is really in the document: reopen the saved bytes through the
    // same command boundary and extract from the fresh parse.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("edited-text", bytes);
    let reopened = open_synthetic(&saved);
    let saved_text = extract_text(reopened.clone(), 0).expect("extract from saved bytes");
    assert!(saved_text.text.contains("Edited headline"));
    assert!(
        !saved_text.text.contains("PDFX fixture"),
        "saved document still contains the original line: {:?}",
        saved_text.text
    );
    assert_eq!(saved_text.text.matches("Edited headline").count(), 1);
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

#[test]
fn deleting_a_text_box_removes_the_original_text_everywhere() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "Latin text") as u32,
    )
    .expect("target");

    let result = delete_text_box(document_id.clone(), 0, target.box_id).expect("delete text box");
    assert!(result.text.is_empty());

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer after delete");
    assert!(!layer.text.contains("Latin text"), "got {:?}", layer.text);
    assert!(layer.text.contains("PDFX fixture: text layer."));
    assert!(layer.text.contains("The quick brown fox"));

    let extracted = extract_text(document_id.clone(), 0).expect("extract after delete");
    assert!(!extracted.text.contains("Latin text"));
    assert!(!extracted.text.contains("extractable"));

    // A fresh parse of the saved bytes confirms the text is gone rather than
    // merely hidden in memory.
    let bytes = save_document_bytes(&document_id).expect("serialize deleted document");
    let saved = synthetic::pdf_bytes_file("deleted-text", bytes);
    let reopened = open_synthetic(&saved);
    let saved_text = extract_text(reopened.clone(), 0).expect("extract from saved bytes");
    assert!(
        !saved_text.text.contains("Latin text"),
        "got {:?}",
        saved_text.text
    );
    assert!(!saved_text.text.contains("extractable"));
    assert!(saved_text.text.contains("PDFX fixture"));
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

struct DecodedPng {
    width: u32,
    height: u32,
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
        height: info.height,
        rgba: buffer,
    }
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    decode_png(&image.png_base64)
}

/// Bounding box of all non-white pixels in page points.
fn ink_bounds(pixels: &DecodedPng, scale: f32) -> (f32, f32, f32, f32) {
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

#[test]
fn the_canvas_reflects_the_edit_at_the_original_position() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");

    let before_pixels = render_pixels(&document_id, 0, 1.0);
    let before_ink = ink_bounds(&before_pixels, 1.0);

    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Replacement line.".to_owned(),
    )
    .expect("apply text edit");

    let after_pixels = render_pixels(&document_id, 0, 1.0);
    assert_ne!(
        before_pixels.rgba, after_pixels.rgba,
        "the rendered canvas must change immediately after an edit"
    );
    let after_ink = ink_bounds(&after_pixels, 1.0);

    // The replacement is painted at the original baseline and left edge; only
    // glyph shapes differ.
    assert!(
        (after_ink.0 - before_ink.0).abs() <= 1.0 && (after_ink.0 - 72.0).abs() <= 1.0,
        "left edge moved: before {before_ink:?}, after {after_ink:?}"
    );
    assert!(
        (after_ink.1 - before_ink.1).abs() <= 1.5,
        "top edge moved: before {before_ink:?}, after {after_ink:?}"
    );

    // The saved document renders the same replacement at the same place.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("edited-render", bytes);
    let reopened = open_synthetic(&saved);
    let saved_pixels = render_pixels(&reopened, 0, 1.0);
    let saved_ink = ink_bounds(&saved_pixels, 1.0);
    for (name, actual, expected) in [
        ("left", saved_ink.0, after_ink.0),
        ("top", saved_ink.1, after_ink.1),
        ("right", saved_ink.2, after_ink.2),
        ("bottom", saved_ink.3, after_ink.3),
    ] {
        assert!(
            (actual - expected).abs() <= 1.0,
            "{name} differed in the saved document: {actual} vs {expected}"
        );
    }
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

#[test]
fn long_text_wraps_at_the_text_box_width_and_expands_downward() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let clicked = char_index_of(&before, "The quick brown fox");
    let target = text_edit_target(document_id.clone(), 0, clicked as u32).expect("target");
    let box_width = target.width;
    let box_left = target.x;
    let box_top = target.y;

    let replacement = "The quick brown fox jumps over the lazy dog. This replacement is \
deliberately much longer than the original line and must wrap inside the Text Box without \
moving any other text.";
    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        replacement.to_owned(),
    )
    .expect("apply long text");
    assert_eq!(result.text, replacement);

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer after wrap");
    assert!(
        !layer.text.contains("(1234567890)"),
        "the original long line must be replaced: {:?}",
        layer.text
    );
    let flattened = layer.text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flattened.contains(&replacement.split_whitespace().collect::<Vec<_>>().join(" ")),
        "wrapped text must keep every word in order: {flattened:?}"
    );

    // Group the replacement's glyphs into visual rows.
    let baseline = before.chars[clicked].origin_y;
    let mut rows: std::collections::BTreeMap<i32, (f32, f32)> = std::collections::BTreeMap::new();
    for character in layer
        .chars
        .iter()
        .filter(|character| character.origin_y >= baseline - 0.5 && character.width > 0.0)
    {
        let row = (character.origin_y * 10.0).round() as i32;
        let entry = rows
            .entry(row)
            .or_insert((f32::INFINITY, f32::NEG_INFINITY));
        entry.0 = entry.0.min(character.x);
        entry.1 = entry.1.max(character.x + character.width);
    }
    assert!(
        rows.len() >= 2,
        "long text must wrap into multiple rows, got {rows:?}"
    );

    let row_entries: Vec<_> = rows.into_iter().collect();
    for (row, (left, right)) in &row_entries {
        assert!(
            (*left - box_left).abs() <= 1.0,
            "row {row} starts at {left}, expected the box left edge {box_left}"
        );
        assert!(
            *right <= box_left + box_width + 2.0,
            "row {row} is {} wide, beyond the {box_width}-point Text Box",
            right - left
        );
    }
    for pair in row_entries.windows(2) {
        let (first_row, _) = pair[0];
        let (second_row, _) = pair[1];
        let spacing = (second_row - first_row) as f32 / 10.0;
        assert!(
            (spacing - 21.6).abs() <= 0.6,
            "18pt text should use 21.6pt line spacing, got {spacing}"
        );
    }

    // The box expanded downward: its last row ends below the original line.
    let original_bottom = before.chars[clicked].y + before.chars[clicked].height;
    let last_row = row_entries.last().expect("last row").0 as f32 / 10.0;
    assert!(
        last_row > original_bottom,
        "wrapped text must expand below the original line: {last_row} vs {original_bottom}"
    );

    // No paragraph reflow: the two Text Objects above did not move.
    for needle in [
        "PDFX fixture: text layer.",
        "Latin text is extractable from this page.",
    ] {
        let before_origin = first_origin(&before, needle);
        let after_origin = first_origin(&layer, needle);
        assert!(
            (before_origin.0 - after_origin.0).abs() < 0.01
                && (before_origin.1 - after_origin.1).abs() < 0.01,
            "{needle:?} moved from {before_origin:?} to {after_origin:?}"
        );
    }

    // Clicking a wrapped continuation line re-opens the whole Text Box, not
    // just that one line, with the logical text the user typed.
    let second_row = row_entries[1].0 as f32 / 10.0;
    let continuation = layer
        .chars
        .iter()
        .position(|character| (character.origin_y - second_row).abs() < 0.5)
        .expect("continuation line character");
    let re_target =
        text_edit_target(document_id.clone(), 0, continuation as u32).expect("continuation target");
    assert_eq!(re_target.box_id, target.box_id);
    assert_eq!(re_target.text, replacement);
    assert!(
        (re_target.y - box_top).abs() <= 0.5,
        "the Text Box top must stay at the original position: {} vs {box_top}",
        re_target.y
    );

    close_document(document_id).expect("close document");
}

#[test]
fn invalid_edits_are_rejected_without_changing_the_document() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "PDFX fixture") as u32,
    )
    .expect("target");

    let error = apply_text_edit(document_id.clone(), 0, target.box_id, "   ".to_owned())
        .expect_err("blank replacement");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let error = apply_text_edit(document_id.clone(), 0, 999_999, "Text".to_owned())
        .expect_err("unknown Text Box");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let after = extract_text_layer(document_id.clone(), 0).expect("text layer");
    assert_eq!(
        after.text, before.text,
        "rejected edits must not mutate the page"
    );
    let history = edit_history(document_id.clone()).expect("history");
    assert!(
        !history.dirty && !history.can_undo,
        "a rejected edit must not enter the history: {history:?}"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn undo_and_redo_restore_the_exact_text_and_pixels() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before_layer, "Original") as u32,
    )
    .expect("target");
    let before_pixels = render_pixels(&document_id, 0, 1.0);

    let history = edit_history(document_id.clone()).expect("history before edit");
    assert!(!history.can_undo && !history.can_redo && !history.dirty);
    let error = undo_text_edit(document_id.clone()).expect_err("nothing to undo");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    let error = redo_text_edit(document_id.clone()).expect_err("nothing to redo");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    let error = edit_history("doc-missing".to_owned()).expect_err("unknown document");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);

    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Replacement line.".to_owned(),
    )
    .expect("apply text edit");
    let edited_layer = extract_text_layer(document_id.clone(), 0).expect("text layer after edit");
    let edited_pixels = render_pixels(&document_id, 0, 1.0);
    let edited_history = edit_history(document_id.clone()).expect("history after edit");
    assert!(edited_history.can_undo && !edited_history.can_redo && edited_history.dirty);

    let step = undo_text_edit(document_id.clone()).expect("undo");
    assert_eq!(step.page_index, 0);
    assert!(
        !step.history.can_undo && step.history.can_redo && !step.history.dirty,
        "undoing the only edit returns to the saved state: {:?}",
        step.history
    );
    assert_eq!(
        extract_text_layer(document_id.clone(), 0)
            .expect("text layer after undo")
            .text,
        before_layer.text,
        "undo restores the original text exactly"
    );
    let undone_pixels = render_pixels(&document_id, 0, 1.0);
    assert_eq!(
        undone_pixels.rgba, before_pixels.rgba,
        "undo restores the exact previous pixels"
    );

    let step = redo_text_edit(document_id.clone()).expect("redo");
    assert_eq!(step.page_index, 0);
    assert!(
        step.history.can_undo && !step.history.can_redo && step.history.dirty,
        "redo returns to the edited, unsaved state: {:?}",
        step.history
    );
    assert_eq!(step.history.revision, edited_history.revision);
    assert_eq!(
        extract_text_layer(document_id.clone(), 0)
            .expect("text layer after redo")
            .text,
        edited_layer.text
    );
    let redone_pixels = render_pixels(&document_id, 0, 1.0);
    assert_eq!(
        redone_pixels.rgba, edited_pixels.rgba,
        "redo restores the exact edited pixels"
    );

    // A new edit after undo replaces the redo branch.
    undo_text_edit(document_id.clone()).expect("undo before branching");
    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Branched.".to_owned(),
    )
    .expect("edit on a new branch");
    let history = edit_history(document_id.clone()).expect("history after branch");
    assert!(
        !history.can_redo,
        "a new edit must clear the redo branch: {history:?}"
    );
    assert!(history.dirty);
    let branched = extract_text_layer(document_id.clone(), 0).expect("branched text layer");
    assert!(
        branched.text.contains("Branched."),
        "the edit after undo must be on the page: {:?}",
        branched.text
    );
    assert!(!branched.text.contains("Replacement line."));

    close_document(document_id).expect("close document");
}

#[test]
fn history_survives_saving_and_dirty_tracks_the_saved_state() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before_layer, "Original") as u32,
    )
    .expect("target");
    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Saved edit.".to_owned(),
    )
    .expect("apply text edit");
    let edited_revision = edit_history(document_id.clone())
        .expect("history after edit")
        .revision;

    // Saving serializes the in-memory document; the session and its history
    // stay alive so the user can keep working.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("history-saved", bytes);
    let reopened = open_synthetic(&saved);
    assert!(extract_text(reopened.clone(), 0)
        .expect("saved text")
        .text
        .contains("Saved edit."));
    close_document(reopened).expect("close reopened document");

    let history = mark_document_saved(document_id.clone()).expect("mark saved");
    assert!(!history.dirty, "saving establishes a new clean baseline");
    assert_eq!(history.saved_revision, edited_revision);
    assert_eq!(history.revision, edited_revision);
    assert!(history.can_undo, "saving must not clear the undo stack");
    assert!(!history.can_redo);

    // The user can undo past the save point.
    undo_text_edit(document_id.clone()).expect("undo past the save point");
    assert_eq!(
        extract_text_layer(document_id.clone(), 0)
            .expect("text layer after undo")
            .text,
        before_layer.text
    );
    let history = edit_history(document_id.clone()).expect("history after undo");
    assert!(history.dirty, "the document differs from the saved version");
    assert!(
        !history.can_undo && history.can_redo,
        "the single edit is now only redoable: {history:?}"
    );

    // Redo returns to the saved state: clean again without another save.
    redo_text_edit(document_id.clone()).expect("redo to the saved state");
    let history = edit_history(document_id.clone()).expect("history after redo");
    assert_eq!(history.revision, edited_revision);
    assert!(!history.dirty);
    assert!(history.can_undo && !history.can_redo);

    // Editing after undoing past the save point is dirty and has no redo.
    undo_text_edit(document_id.clone()).expect("undo again");
    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Unsaved edit.".to_owned(),
    )
    .expect("edit after undo");
    let history = edit_history(document_id.clone()).expect("history after unsaved edit");
    assert!(!history.can_redo);
    assert!(history.dirty);
    assert_eq!(history.saved_revision, edited_revision);

    close_document(document_id).expect("close document");
}

/**
 * The close guard asks `edit_history` at the command boundary immediately
 * before deciding, so an edit whose frontend event has not arrived yet must
 * still be visible. It then relies on the same dirty transitions as the title
 * marker: a save is clean, undo past it is dirty, redo back is clean.
 */
#[test]
fn a_fresh_history_read_tracks_the_state_the_close_guard_asks_about() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let initial = edit_history(document_id.clone()).expect("initial history");
    assert!(
        !initial.dirty && !initial.can_undo,
        "a freshly opened document has nothing to save: {initial:?}"
    );

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Unsaved edit.".to_owned(),
    )
    .expect("apply text edit");

    // No command result is used here: this is exactly the fresh read the close
    // guard performs after the edit has been committed.
    let edited = edit_history(document_id.clone()).expect("fresh history after edit");
    assert!(edited.dirty, "an edit immediately before close is dirty");
    assert_eq!(edited.saved_revision, 0);

    mark_document_saved(document_id.clone()).expect("save baseline");
    let saved = edit_history(document_id.clone()).expect("fresh history after save");
    assert!(!saved.dirty, "a written state closes without asking");

    let undone = undo_text_edit(document_id.clone()).expect("undo past the save");
    assert!(undone.history.dirty, "undo past the baseline is dirty");
    let redone = redo_text_edit(document_id.clone()).expect("redo back to the save");
    assert!(!redone.history.dirty, "the saved revision is clean again");

    close_document(document_id).expect("close document");
}

#[test]
fn undo_restores_deleted_text_and_redo_removes_it_again() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_pixels = render_pixels(&document_id, 0, 1.0);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "Latin text") as u32,
    )
    .expect("target");
    let third = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "The quick brown fox") as u32,
    )
    .expect("third target");

    delete_text_box(document_id.clone(), 0, target.box_id).expect("delete text box");
    let deleted = extract_text_layer(document_id.clone(), 0).expect("text layer after delete");
    let deleted_pixels = render_pixels(&document_id, 0, 1.0);
    assert_ne!(before_pixels.rgba, deleted_pixels.rgba);

    undo_text_edit(document_id.clone()).expect("undo delete");
    let restored = extract_text_layer(document_id.clone(), 0).expect("text layer after undo");
    assert_eq!(restored.text, before.text, "undo restores the deleted line");
    assert_eq!(
        render_pixels(&document_id, 0, 1.0).rgba,
        before_pixels.rgba,
        "undo restores the deleted line's pixels"
    );

    // The Text Boxes after the deleted one shifted back with it: clicking the
    // third line still resolves to the same Text Box.
    let third_after = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&restored, "The quick brown fox") as u32,
    )
    .expect("third target after undo");
    assert_eq!(third_after.box_id, third.box_id);
    assert_eq!(
        third_after.text,
        "The quick brown fox jumps over the lazy dog. (1234567890)"
    );

    redo_text_edit(document_id.clone()).expect("redo delete");
    assert_eq!(
        extract_text_layer(document_id.clone(), 0)
            .expect("text layer after redo")
            .text,
        deleted.text
    );
    assert_eq!(
        render_pixels(&document_id, 0, 1.0).rgba,
        deleted_pixels.rgba
    );

    // Undoing a deletion reattaches the real page objects: the serialized
    // document contains the line again, not just the in-memory layer.
    undo_text_edit(document_id.clone()).expect("undo delete again");
    let bytes = save_document_bytes(&document_id).expect("serialize restored document");
    let saved = synthetic::pdf_bytes_file("delete-undone", bytes);
    let reopened = open_synthetic(&saved);
    let saved_text = extract_text(reopened.clone(), 0).expect("reopened text");
    assert!(
        saved_text
            .text
            .contains("Latin text is extractable from this page."),
        "the restored Text Object must be in the saved page content: {:?}",
        saved_text.text
    );
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

#[test]
fn undo_after_wrapping_restores_the_original_single_line() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_pixels = render_pixels(&document_id, 0, 1.0);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before_layer, "Original") as u32,
    )
    .expect("target");

    let replacement = "A much longer replacement line that has to wrap at the width of the \
original Text Box and therefore creates several text objects.";
    apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        replacement.to_owned(),
    )
    .expect("apply wrapped text");
    let wrapped_layer = extract_text_layer(document_id.clone(), 0).expect("wrapped layer");
    let wrapped_pixels = render_pixels(&document_id, 0, 1.0);
    let mut rows: Vec<i32> = wrapped_layer
        .chars
        .iter()
        .filter(|character| character.width > 0.0)
        .map(|character| (character.origin_y * 10.0).round() as i32)
        .collect();
    rows.sort_unstable();
    rows.dedup();
    assert!(rows.len() >= 2, "the replacement must wrap, got {rows:?}");

    undo_text_edit(document_id.clone()).expect("undo wrapped edit");
    let restored = extract_text_layer(document_id.clone(), 0).expect("restored layer");
    assert_eq!(
        restored.text, before_layer.text,
        "undo restores the single original line"
    );
    assert_eq!(render_pixels(&document_id, 0, 1.0).rgba, before_pixels.rgba);

    redo_text_edit(document_id.clone()).expect("redo wrapped edit");
    assert_eq!(
        extract_text_layer(document_id.clone(), 0)
            .expect("rewrapped layer")
            .text,
        wrapped_layer.text
    );
    assert_eq!(
        render_pixels(&document_id, 0, 1.0).rgba,
        wrapped_pixels.rgba
    );

    close_document(document_id).expect("close document");
}

#[test]
fn wrapping_an_earlier_box_keeps_later_boxes_addressable_after_undo() {
    let document_id = open_fixture("text");
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let first = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "PDFX fixture") as u32,
    )
    .expect("first target");
    let third = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "The quick brown fox") as u32,
    )
    .expect("third target");

    // Replacing the first line with wrapped text inserts several objects and
    // moves every later Text Box's object range.
    apply_text_edit(
        document_id.clone(),
        0,
        first.box_id,
        "A much longer replacement line that wraps at the first Text Box width and inserts \
several text objects before every later line."
            .to_owned(),
    )
    .expect("edit the first line");
    let wrapped = extract_text_layer(document_id.clone(), 0).expect("wrapped layer");
    let third_during = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&wrapped, "The quick brown fox") as u32,
    )
    .expect("third target while wrapped");
    assert_eq!(
        third_during.box_id, third.box_id,
        "later Text Boxes must stay addressable after an earlier box wraps"
    );

    undo_text_edit(document_id.clone()).expect("undo the wrapped edit");
    let restored = extract_text_layer(document_id.clone(), 0).expect("restored layer");
    assert_eq!(restored.text, before.text);
    let third_after = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&restored, "The quick brown fox") as u32,
    )
    .expect("third target after undo");
    assert_eq!(third_after.box_id, third.box_id);
    assert_eq!(
        third_after.text,
        "The quick brown fox jumps over the lazy dog. (1234567890)"
    );

    // Editing the later box after the undo still works and leaves the first
    // line untouched.
    apply_text_edit(
        document_id.clone(),
        0,
        third_after.box_id,
        "Edited third line.".to_owned(),
    )
    .expect("edit the third line after undo");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after third edit");
    assert!(layer.text.contains("PDFX fixture: text layer."));
    assert!(layer.text.contains("Edited third line."));
    assert!(!layer.text.contains("The quick brown fox"));

    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_reject_text_editing() {
    let document_id = open_fixture_id("encrypted-owner-restrictions");

    let error = text_edit_target(document_id.clone(), 0, 0).expect_err("read-only target");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(
        error.message.to_lowercase().contains("does not allow"),
        "message should explain the restriction: {}",
        error.message
    );

    let error = apply_text_edit(document_id.clone(), 0, 1, "Edited".to_owned())
        .expect_err("read-only edit");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = delete_text_box(document_id.clone(), 0, 1).expect_err("read-only delete");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);

    let history = edit_history(document_id.clone()).expect("history");
    assert!(!history.can_undo && !history.can_redo && !history.dirty);

    close_document(document_id).expect("close document");
}

#[test]
fn embedded_fonts_are_preserved_when_the_replacement_uses_covered_glyphs() {
    let pdf = synthetic::embedded_true_type_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Embedded") as u32,
    )
    .expect("target");
    assert!(
        target.font_family.contains("LiberationSans"),
        "the target must report the embedded font, got {:?}",
        target.font_family
    );

    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Embedded font edited.".to_owned(),
    )
    .expect("apply edit with covered glyphs");
    assert!(
        result.font.is_none(),
        "covered glyphs must keep the original embedded font: {:?}",
        result.font
    );

    let edited = extract_text_layer(document_id.clone(), 0).expect("edited layer");
    let edited_target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&edited, "Embedded font edited") as u32,
    )
    .expect("edited target");
    assert_eq!(
        edited_target.font_family, target.font_family,
        "the replacement must use the original embedded font"
    );

    // The font reference survives serialization.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("embedded-font-edited", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let reopened_target = text_edit_target(
        reopened.clone(),
        0,
        char_index_of(&reopened_layer, "Embedded font edited") as u32,
    )
    .expect("reopened target");
    assert_eq!(
        reopened_target.font_family, target.font_family,
        "the saved document must still reference the embedded font"
    );
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

#[test]
fn non_standard_fonts_fall_back_with_a_documented_reason() {
    let pdf = synthetic::non_standard_font_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Fallback") as u32,
    )
    .expect("target");
    assert_eq!(target.font_family, "PDFXMissingFont");

    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Fallback font edited.".to_owned(),
    )
    .expect("apply edit with fallback font");
    let font = result.font.expect("the fallback must be reported");
    assert!(font.fallback);
    assert_eq!(font.original_family, "PDFXMissingFont");
    assert_eq!(font.family, "Helvetica");
    let reason = font.reason.expect("the fallback needs a reason");
    assert!(
        reason.contains("PDFXMissingFont") && reason.contains("not embedded"),
        "the reason should name the font and why it was replaced: {reason}"
    );

    let edited = extract_text_layer(document_id.clone(), 0).expect("edited layer");
    assert!(
        edited.text.contains("Fallback font edited."),
        "the replacement must be extractable, got {:?}",
        edited.text
    );

    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("fallback-font-edited", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_text = extract_text(reopened.clone(), 0).expect("reopened text");
    assert!(reopened_text.text.contains("Fallback font edited."));
    close_document(reopened).expect("close reopened document");

    close_document(document_id).expect("close document");
}

#[test]
fn characters_the_bundled_font_lacks_are_rejected_without_mutating_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let before = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_pixels = render_pixels(&document_id, 0, 1.0);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&before, "Original") as u32,
    )
    .expect("target");

    // Latin, Chinese and other scripts covered by Noto Sans SC now succeed;
    // an emoji has no glyph in the bundled font and must still be refused
    // rather than rendered as a missing-glyph box.
    let error = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "😀 replacement".to_owned(),
    )
    .expect_err("a character without a bundled glyph must be rejected");
    assert_eq!(error.code, CommandErrorCode::UnsupportedFont);
    assert!(
        error.message.contains("Noto Sans SC") && error.message.contains("no glyphs"),
        "the error must name the bundled font and the missing coverage: {}",
        error.message
    );

    let after = extract_text_layer(document_id.clone(), 0).expect("text layer after rejection");
    assert_eq!(after.text, before.text, "the document must be unchanged");
    assert_eq!(
        render_pixels(&document_id, 0, 1.0).rgba,
        before_pixels.rgba,
        "a refused edit must not change the rendering"
    );
    let history = edit_history(document_id.clone()).expect("history");
    assert!(
        !history.dirty && !history.can_undo,
        "a refused edit must not enter the history: {history:?}"
    );

    close_document(document_id).expect("close document");
}
