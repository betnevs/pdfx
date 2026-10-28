//! Command-boundary tests for creating and styling new Text Boxes.
//!
//! Every assertion goes through the Rust functions the Tauri IPC layer exposes.
//! Saved output is reopened through the same command boundary so "the Text Box
//! is really in the document" is checked against a fresh PDFium parse.

mod support;

use support::synthetic;

use pdfx_lib::commands::{
    close_document, extract_text, extract_text_layer, open_document, render_page,
    save_document_bytes, CommandErrorCode, TextLayerView,
};
use pdfx_lib::text_edit::{
    apply_text_box_style, apply_text_edit, create_text_box, delete_text_box, edit_history,
    mark_document_saved, redo_text_edit, text_box_fonts, text_boxes, text_edit_target,
    transform_text_box, undo_text_edit, CreateTextBoxRequest, TextAlignment, TextBoxStyleView,
    TextBoxTransformRequest,
};

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn char_index_of(layer: &TextLayerView, needle: &str) -> usize {
    let offset = layer.text.find(needle).expect("text in layer");
    layer.text[..offset].chars().count()
}

fn default_style() -> TextBoxStyleView {
    TextBoxStyleView {
        font_family: "Helvetica".to_owned(),
        font_size: 16.0,
        bold: false,
        italic: false,
        underline: false,
        color: "#000000".to_owned(),
        alignment: TextAlignment::Left,
    }
}

fn create_request(x: f32, y: f32, width: f32, height: f32) -> CreateTextBoxRequest {
    CreateTextBoxRequest {
        x,
        y,
        width,
        height,
        style: None,
    }
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

/// Bounding box of all non-white pixels inside a page-point region.
fn ink_bounds(
    pixels: &DecodedPng,
    scale: f32,
    region: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        if pixel[0] >= 245 && pixel[1] >= 245 && pixel[2] >= 245 {
            continue;
        }
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        if x < region.0 || x > region.2 || y < region.1 || y > region.3 {
            continue;
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    (left, top, right, bottom)
}

fn styled(font_family: &str, size: f32) -> TextBoxStyleView {
    TextBoxStyleView {
        font_family: font_family.to_owned(),
        font_size: size,
        bold: true,
        italic: true,
        underline: true,
        color: "#c0392b".to_owned(),
        alignment: TextAlignment::Right,
    }
}

fn flattened(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn font_registry_lists_only_base14_families_with_metadata() {
    let fonts = text_box_fonts();
    assert_eq!(fonts.len(), 12, "three families with four variants each");

    for family in ["Helvetica", "Times", "Courier"] {
        let variants: Vec<_> = fonts.iter().filter(|font| font.family == family).collect();
        assert_eq!(
            variants.len(),
            4,
            "{family} needs regular, bold, italic, bold-italic"
        );
        assert!(variants.iter().any(|font| !font.bold && !font.italic));
        assert!(variants.iter().any(|font| font.bold && !font.italic));
        assert!(variants.iter().any(|font| !font.bold && font.italic));
        assert!(variants.iter().any(|font| font.bold && font.italic));
    }

    for font in &fonts {
        // The registry tells ticket 11 how to extend the list without claiming
        // these base-14 fonts are embedded TrueType programs.
        assert_eq!(font.source, "base14");
        assert!(
            font.embeddable,
            "base-14 fonts need no embedding permission"
        );
        assert!(!font.embedded, "base-14 fonts are not embedded programs");
        assert_eq!(font.coverage, "win_ansi");
        assert!(!font.postscript_name.is_empty());
    }

    assert!(fonts
        .iter()
        .any(|font| font.postscript_name == "Helvetica-BoldOblique"));
    assert!(fonts
        .iter()
        .any(|font| font.postscript_name == "Times-BoldItalic"));
    assert!(fonts
        .iter()
        .any(|font| font.postscript_name == "Courier-Oblique"));
}

#[test]
fn creating_a_text_box_and_typing_survives_saving_and_reopening() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(120.0, 180.0, 200.0, 60.0),
    )
    .expect("create Text Box");
    assert_eq!(created.target.text, "");
    assert_eq!(created.target.font_family, "Helvetica");
    assert!((created.target.font_size - 16.0).abs() < 0.01);
    assert_eq!(created.target.color, "#000000");
    assert_eq!(created.target.alignment, TextAlignment::Left);
    assert!(!created.target.bold && !created.target.italic && !created.target.underline);
    assert!(created.target.auto_height);
    assert!(created.target.created, "new boxes are marked as created");
    assert!(created.target.box_id > 0);
    assert!(created.history.can_undo, "creating a box is undoable");
    assert!(created.history.dirty);

    // The empty box is listed so the overlay can select it before it has text.
    let boxes = text_boxes(document_id.clone(), 0).expect("list boxes");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].box_id, created.target.box_id);
    assert_eq!(boxes[0].text, "");

    let applied = apply_text_edit(
        document_id.clone(),
        0,
        created.target.box_id,
        "Hello text box".to_owned(),
    )
    .expect("type into the box");
    assert_eq!(applied.text, "Hello text box");
    assert!(applied.font.is_none(), "a built-in font needs no fallback");

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    assert!(
        layer.text.contains("Hello text box"),
        "the new text must be on the page: {:?}",
        layer.text
    );
    assert!(
        layer.text.contains("Original line."),
        "the original page text must be untouched: {:?}",
        layer.text
    );

    // The first line sits at the Text Box origin with the documented 0.8em
    // baseline offset.
    let index = char_index_of(&layer, "Hello");
    let first = &layer.chars[index];
    assert!(
        (first.origin_x - 120.0).abs() < 0.5,
        "origin x was {}",
        first.origin_x
    );
    assert!(
        (first.origin_y - (180.0 + 16.0 * 0.8)).abs() < 0.6,
        "origin y was {}",
        first.origin_y
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("list boxes after typing");
    assert_eq!(boxes[0].text, "Hello text box");
    assert!(boxes[0].height >= 16.0 * 1.2 - 0.01);

    // Saving writes real text objects: reopen and extract them.
    let bytes = save_document_bytes(&document_id).expect("serialize document");
    let saved = synthetic::pdf_bytes_file("created-text-box", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_text = extract_text(reopened.clone(), 0).expect("extract after reopen");
    assert!(
        reopened_text.text.contains("Hello text box"),
        "saved text: {:?}",
        reopened_text.text
    );
    assert!(reopened_text.text.contains("Original line."));

    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let reopened_index = char_index_of(&reopened_layer, "Hello");
    let reopened_first = &reopened_layer.chars[reopened_index];
    assert!(
        (reopened_first.origin_x - 120.0).abs() < 0.5,
        "saved origin x was {}",
        reopened_first.origin_x
    );
    assert!(
        (reopened_first.origin_y - (180.0 + 16.0 * 0.8)).abs() < 0.6,
        "saved origin y was {}",
        reopened_first.origin_y
    );

    // The saved font, size and colour are inspectable through the same seam.
    let target = text_edit_target(reopened.clone(), 0, reopened_index as u32)
        .expect("target from saved text");
    assert_eq!(target.font_family, "Helvetica");
    assert!(
        (target.font_size - 16.0).abs() < 0.01,
        "saved font size was {}",
        target.font_size
    );
    assert_eq!(target.color, "#000000");

    close_document(reopened).expect("close reopened document");
    close_document(document_id).expect("close document");
}

#[test]
fn whole_box_style_changes_apply_immediately_and_survive_saving() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(100.0, 200.0, 260.0, 80.0),
    )
    .expect("create Text Box");
    let box_id = created.target.box_id;
    apply_text_edit(document_id.clone(), 0, box_id, "Styled".to_owned()).expect("type");

    let styled = apply_text_box_style(document_id.clone(), 0, box_id, styled("Times", 22.0))
        .expect("restyle");
    assert_eq!(styled.target.font_family, "Times");
    assert!((styled.target.font_size - 22.0).abs() < 0.01);
    assert!(styled.target.bold);
    assert!(styled.target.italic);
    assert!(styled.target.underline);
    assert_eq!(styled.target.color, "#c0392b");
    assert_eq!(styled.target.alignment, TextAlignment::Right);
    assert_eq!(styled.target.text, "Styled");

    // The page now paints the new style. The underline is the lowest ink below
    // the baseline for a word without descenders.
    let pixels = render_pixels(&document_id, 0, 2.0);
    let box_region = (100.0, 180.0, 360.0, 320.0);
    let (ink_left, ink_top, ink_right, ink_bottom) = ink_bounds(&pixels, 2.0, box_region);
    let baseline = 200.0 + 22.0 * 0.8;
    assert!(
        ink_bottom > baseline + 1.0,
        "underline must paint below the baseline: ink bottom {ink_bottom}, baseline {baseline}"
    );
    assert!(
        ink_bottom < baseline + 6.0,
        "underline must sit close to the baseline: ink bottom {ink_bottom}"
    );
    // Right aligned inside a 260pt box.
    assert!(
        ink_left > 100.0 + 260.0 * 0.35,
        "right aligned text started at {ink_left}"
    );
    assert!(ink_top >= 200.0 - 1.0);
    assert!(ink_right <= 100.0 + 260.0 + 2.0);

    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let index = char_index_of(&layer, "Styled");
    assert!(
        (layer.chars[index].font_size - 22.0).abs() < 0.01,
        "layer font size was {}",
        layer.chars[index].font_size
    );

    // Save fidelity: the style is in the serialized text and path objects.
    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("styled-text-box", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let reopened_index = char_index_of(&reopened_layer, "Styled");
    assert!(
        (reopened_layer.chars[reopened_index].font_size - 22.0).abs() < 0.01,
        "saved font size was {}",
        reopened_layer.chars[reopened_index].font_size
    );
    let target =
        text_edit_target(reopened.clone(), 0, reopened_index as u32).expect("saved target");
    assert_eq!(target.font_family, "Times-BoldItalic");
    assert_eq!(target.color, "#c0392b");
    assert!(target.bold && target.italic);
    let reopened_pixels = render_pixels(&reopened, 0, 2.0);
    let (_, _, _, reopened_bottom) = ink_bounds(&reopened_pixels, 2.0, box_region);
    assert!(
        reopened_bottom > baseline + 1.0,
        "the saved underline must render: bottom {reopened_bottom}"
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn style_and_text_rejections_do_not_mutate_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(90.0, 160.0, 220.0, 60.0),
    )
    .expect("create Text Box");
    let box_id = created.target.box_id;
    apply_text_edit(document_id.clone(), 0, box_id, "Plain".to_owned()).expect("type");
    let revision = edit_history(document_id.clone()).expect("history").revision;

    // CJK cannot be written with a base-14 font and is refused before any
    // page object or history entry changes.
    let error = apply_text_edit(document_id.clone(), 0, box_id, "你好".to_owned())
        .expect_err("CJK must be rejected");
    assert_eq!(error.code, CommandErrorCode::UnsupportedFont);
    assert!(
        error.message.contains("CJK"),
        "the error should explain the limitation: {}",
        error.message
    );

    let error = apply_text_box_style(document_id.clone(), 0, box_id, styled("Noto Sans SC", 18.0))
        .expect_err("unknown font must be rejected");
    assert_eq!(error.code, CommandErrorCode::UnsupportedFont);
    assert!(
        error.message.contains("Helvetica"),
        "the error should list the available fonts: {}",
        error.message
    );

    let mut invalid_color = styled("Helvetica", 18.0);
    invalid_color.bold = false;
    invalid_color.italic = false;
    invalid_color.underline = false;
    invalid_color.alignment = TextAlignment::Left;
    invalid_color.color = "red".to_owned();
    let error = apply_text_box_style(document_id.clone(), 0, box_id, invalid_color)
        .expect_err("invalid colour must be rejected");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let mut invalid_size = styled("Helvetica", 18.0);
    invalid_size.font_size = 0.0;
    let error = apply_text_box_style(document_id.clone(), 0, box_id, invalid_size)
        .expect_err("invalid size must be rejected");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    assert_eq!(
        edit_history(document_id.clone()).expect("history").revision,
        revision,
        "rejected changes must not create a revision"
    );
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    assert!(layer.text.contains("Plain"));
    assert!(!layer.text.contains("你好"));

    let target = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(target.text, "Plain");
    assert_eq!(target.font_family, "Helvetica");
    assert_eq!(target.color, "#000000");
    assert!(!target.underline);

    close_document(document_id).expect("close");
}

#[test]
fn empty_and_filled_boxes_can_be_deleted_and_undone() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let empty = create_text_box(
        document_id.clone(),
        0,
        create_request(80.0, 600.0, 150.0, 40.0),
    )
    .expect("create empty");
    assert_eq!(empty.target.text, "");
    assert_eq!(text_boxes(document_id.clone(), 0).expect("boxes").len(), 1);

    let deleted = delete_text_box(document_id.clone(), 0, empty.target.box_id).expect("delete");
    assert_eq!(deleted.text, "");
    assert!(deleted.history.can_undo);
    assert!(
        text_boxes(document_id.clone(), 0)
            .expect("boxes")
            .is_empty(),
        "a deleted box disappears from the overlay"
    );

    undo_text_edit(document_id.clone()).expect("undo delete");
    let restored = text_boxes(document_id.clone(), 0).expect("boxes after undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].box_id, empty.target.box_id);
    assert_eq!(restored[0].text, "");
    assert!((restored[0].width - 150.0).abs() < 0.01);

    redo_text_edit(document_id.clone()).expect("redo delete");
    assert!(text_boxes(document_id.clone(), 0)
        .expect("boxes after redo")
        .is_empty());

    // A filled box deletes its real text objects, not a white cover.
    let filled = create_text_box(
        document_id.clone(),
        0,
        create_request(80.0, 500.0, 180.0, 50.0),
    )
    .expect("create filled");
    apply_text_edit(
        document_id.clone(),
        0,
        filled.target.box_id,
        "Delete this".to_owned(),
    )
    .expect("type");
    delete_text_box(document_id.clone(), 0, filled.target.box_id).expect("delete filled");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    assert!(!layer.text.contains("Delete this"));
    assert!(layer.text.contains("Original line."));
    undo_text_edit(document_id.clone()).expect("undo filled delete");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after undo");
    assert!(layer.text.contains("Delete this"));
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert!(boxes
        .iter()
        .any(|b| b.box_id == filled.target.box_id && b.text == "Delete this"));
    redo_text_edit(document_id.clone()).expect("redo filled delete");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after redo");
    assert!(!layer.text.contains("Delete this"));

    close_document(document_id).expect("close");
}

#[test]
fn multiple_boxes_keep_independent_content_and_offsets() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let first = create_text_box(
        document_id.clone(),
        0,
        create_request(80.0, 300.0, 180.0, 50.0),
    )
    .expect("create first");
    let second = create_text_box(
        document_id.clone(),
        0,
        create_request(80.0, 420.0, 180.0, 50.0),
    )
    .expect("create second");
    apply_text_edit(
        document_id.clone(),
        0,
        first.target.box_id,
        "First box".to_owned(),
    )
    .expect("type first");
    apply_text_edit(
        document_id.clone(),
        0,
        second.target.box_id,
        "Second box".to_owned(),
    )
    .expect("type second");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let first_index = char_index_of(&layer, "First box");
    let second_index = char_index_of(&layer, "Second box");
    assert!((layer.chars[first_index].origin_y - 312.8).abs() < 0.6);
    assert!((layer.chars[second_index].origin_y - 432.8).abs() < 0.6);
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[0].text, "First box");
    assert_eq!(boxes[1].text, "Second box");

    // Deleting the first must not disturb the second box's objects.
    delete_text_box(document_id.clone(), 0, first.target.box_id).expect("delete first");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after delete");
    assert!(!layer.text.contains("First box"));
    assert!(layer.text.contains("Second box"));
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].box_id, second.target.box_id);

    undo_text_edit(document_id.clone()).expect("undo delete");
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after undo");
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[0].text, "First box");
    assert_eq!(boxes[1].text, "Second box");

    close_document(document_id).expect("close");
}

#[test]
fn undo_and_redo_restore_exact_box_state_after_saving() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(110.0, 340.0, 210.0, 70.0),
    )
    .expect("create");
    let box_id = created.target.box_id;
    apply_text_edit(document_id.clone(), 0, box_id, "History".to_owned()).expect("type");
    let after_type = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();

    let mut style = styled("Courier", 15.0);
    style.underline = false;
    style.alignment = TextAlignment::Center;
    style.color = "#224466".to_owned();
    let after_style = apply_text_box_style(document_id.clone(), 0, box_id, style).expect("style");
    let after_style_target = after_style.target.clone();

    let after_transform = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(130.0, 360.0, 170.0, 90.0, false, -20.0),
    )
    .expect("transform")
    .target;

    // Saving sets the clean baseline but keeps history usable.
    let bytes = save_document_bytes(&document_id).expect("save");
    assert!(!bytes.is_empty());
    let saved = mark_document_saved(document_id.clone()).expect("mark saved");
    assert!(!saved.dirty);
    assert!(saved.can_undo);

    undo_text_edit(document_id.clone()).expect("undo transform");
    let restored_style = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(restored_style, after_style_target);

    undo_text_edit(document_id.clone()).expect("undo style");
    let restored_text = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(restored_text, after_type);

    undo_text_edit(document_id.clone()).expect("undo text");
    let empty = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(empty.text, "");
    undo_text_edit(document_id.clone()).expect("undo create");
    assert!(text_boxes(document_id.clone(), 0)
        .expect("boxes")
        .is_empty());

    // Redo brings each exact state back, including the creation.
    redo_text_edit(document_id.clone()).expect("redo create");
    let redone_create = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(redone_create.text, "");
    redo_text_edit(document_id.clone()).expect("redo text");
    let redone_text = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(redone_text.text, "History");
    redo_text_edit(document_id.clone()).expect("redo style");
    assert_eq!(
        text_boxes(document_id.clone(), 0).expect("boxes")[0].clone(),
        after_style_target
    );
    redo_text_edit(document_id.clone()).expect("redo transform");
    let redone_transform = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert_eq!(redone_transform, after_transform);

    close_document(document_id).expect("close");
}

#[test]
fn every_base14_variant_saves_with_its_real_font_name() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let variants = [
        ("Helvetica", false, false, "Helvetica"),
        ("Helvetica", true, false, "Helvetica-Bold"),
        ("Helvetica", false, true, "Helvetica-Oblique"),
        ("Helvetica", true, true, "Helvetica-BoldOblique"),
    ];
    let mut expected = Vec::new();
    for (index, (family, bold, italic, postscript)) in variants.into_iter().enumerate() {
        let y = 100.0 + index as f32 * 70.0;
        let created = create_text_box(document_id.clone(), 0, create_request(80.0, y, 220.0, 40.0))
            .expect("create variant");
        let text = format!("Variant{index}");
        apply_text_edit(document_id.clone(), 0, created.target.box_id, text.clone())
            .expect("type variant");
        let style = TextBoxStyleView {
            font_family: family.to_owned(),
            font_size: 18.0,
            bold,
            italic,
            underline: false,
            color: "#112233".to_owned(),
            alignment: TextAlignment::Left,
        };
        apply_text_box_style(document_id.clone(), 0, created.target.box_id, style)
            .expect("style variant");
        expected.push((text, postscript));
    }

    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("base14-variants", bytes);
    let reopened = open_synthetic(&saved);
    let layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    for (text, postscript) in expected {
        let index = char_index_of(&layer, &text);
        let target = text_edit_target(reopened.clone(), 0, index as u32).expect("target");
        assert_eq!(target.font_family, postscript, "saved font for {text}");
        assert!((target.font_size - 18.0).abs() < 0.01);
        assert_eq!(target.color, "#112233");
        assert_eq!(target.bold, postscript.contains("Bold"));
        assert_eq!(
            target.italic,
            postscript.contains("Italic") || postscript.contains("Oblique")
        );
    }

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn explicit_newlines_start_new_lines_at_the_line_height() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(90.0, 220.0, 320.0, 100.0),
    )
    .expect("create");
    apply_text_edit(
        document_id.clone(),
        0,
        created.target.box_id,
        "One\nTwo".to_owned(),
    )
    .expect("type with newline");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let one = char_index_of(&layer, "One");
    let two = char_index_of(&layer, "Two");
    assert!((layer.chars[one].origin_x - 90.0).abs() < 0.5);
    assert!((layer.chars[two].origin_x - 90.0).abs() < 0.5);
    assert!(
        (layer.chars[two].origin_y - layer.chars[one].origin_y - 19.2).abs() < 0.6,
        "lines were {} and {}",
        layer.chars[one].origin_y,
        layer.chars[two].origin_y
    );
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes[0].text, "One\nTwo");

    close_document(document_id).expect("close");
}

fn transform(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    auto_height: bool,
    rotation: f32,
) -> TextBoxTransformRequest {
    TextBoxTransformRequest {
        x,
        y,
        width,
        height,
        auto_height,
        rotation,
    }
}

#[test]
fn move_resize_and_rotate_are_reversible_and_survive_saving() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(100.0, 120.0, 200.0, 60.0),
    )
    .expect("create");
    let box_id = created.target.box_id;
    apply_text_edit(document_id.clone(), 0, box_id, "Move me".to_owned()).expect("type");

    let moved = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(160.0, 260.0, 200.0, 60.0, true, 0.0),
    )
    .expect("move");
    assert!((moved.target.x - 160.0).abs() < 0.01);
    assert!((moved.target.y - 260.0).abs() < 0.01);
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let index = char_index_of(&layer, "Move");
    assert!((layer.chars[index].origin_x - 160.0).abs() < 0.5);
    assert!((layer.chars[index].origin_y - 272.8).abs() < 0.6);

    let rotated = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(160.0, 260.0, 200.0, 60.0, true, 90.0),
    )
    .expect("rotate");
    assert!((rotated.target.rotation - 90.0).abs() < 0.01);
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after rotate");
    let first_index = char_index_of(&layer, "Move");
    let first = &layer.chars[first_index];
    assert!(
        (first.origin_x - 147.2).abs() < 0.8,
        "rotated origin x was {}",
        first.origin_x
    );
    assert!(
        (first.origin_y - 260.0).abs() < 0.8,
        "rotated origin y was {}",
        first.origin_y
    );
    let last = &layer.chars[first_index + "Move me".chars().count() - 1];
    assert!(
        last.origin_y > first.origin_y + 20.0,
        "rotated text must run downward: first {first:?}, last {last:?}"
    );

    // Rotation and position are reversible.
    undo_text_edit(document_id.clone()).expect("undo rotation");
    let after_undo = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert!((after_undo.rotation - 0.0).abs() < 0.01);
    assert!((after_undo.x - 160.0).abs() < 0.01);
    undo_text_edit(document_id.clone()).expect("undo move");
    let original = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert!((original.x - 100.0).abs() < 0.01);
    assert!((original.y - 120.0).abs() < 0.01);
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after undo");
    let index = char_index_of(&layer, "Move");
    assert!((layer.chars[index].origin_x - 100.0).abs() < 0.5);

    redo_text_edit(document_id.clone()).expect("redo move");
    redo_text_edit(document_id.clone()).expect("redo rotation");
    let redone = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert!((redone.rotation - 90.0).abs() < 0.01);

    // Save the rotated box, reopen it and check the matrix survived.
    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("rotated-text-box", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let reopened_index = char_index_of(&reopened_layer, "Move");
    let reopened_first = &reopened_layer.chars[reopened_index];
    assert!(
        (reopened_first.origin_x - 147.2).abs() < 1.0,
        "saved rotated x was {}",
        reopened_first.origin_x
    );
    assert!(
        (reopened_first.origin_y - 260.0).abs() < 1.0,
        "saved rotated y was {}",
        reopened_first.origin_y
    );
    let reopened_last = &reopened_layer.chars[reopened_index + "Move me".chars().count() - 1];
    assert!(reopened_last.origin_y > reopened_first.origin_y + 20.0);
    let target =
        text_edit_target(reopened.clone(), 0, reopened_index as u32).expect("saved target");
    assert!(
        (target.rotation - 90.0).abs() < 1.0,
        "saved rotation was {}",
        target.rotation
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn auto_height_grows_for_overflow_and_never_shrinks_automatically() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(100.0, 140.0, 120.0, 30.0),
    )
    .expect("create");
    let box_id = created.target.box_id;
    let long_text = "Wrap this sentence into several lines of text right here";
    apply_text_edit(document_id.clone(), 0, box_id, long_text.to_owned()).expect("type long");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let page_text = flattened(&layer.text);
    assert!(
        page_text.contains(&flattened(long_text)),
        "all wrapped words must remain: {page_text:?}"
    );
    let rows: std::collections::BTreeSet<i32> = layer
        .chars
        .iter()
        .filter(|character| {
            character.width > 0.0 && character.origin_x >= 99.0 && character.origin_x <= 221.0
        })
        .map(|character| (character.origin_y * 10.0).round() as i32)
        .collect();
    assert!(
        rows.len() >= 3,
        "120pt wide box should wrap into at least 3 lines, got {rows:?}"
    );

    let grown = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert!(grown.auto_height);
    assert!(
        grown.height >= 3.0 * 16.0 * 1.2 - 0.01,
        "auto height must fit every line: {}",
        grown.height
    );
    assert!(grown.height > 30.0);

    // A shorter replacement does not shrink the box.
    apply_text_edit(document_id.clone(), 0, box_id, "Short".to_owned()).expect("shorten");
    let after_short = text_boxes(document_id.clone(), 0).expect("boxes")[0].clone();
    assert!(
        (after_short.height - grown.height).abs() < 0.01,
        "auto height must not shrink: {} vs {}",
        after_short.height,
        grown.height
    );

    // Manual resizing turns auto-height off; content beyond the height is
    // clipped from the canvas but not deleted.
    apply_text_edit(document_id.clone(), 0, box_id, long_text.to_owned()).expect("long again");
    let clipped = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(100.0, 140.0, 120.0, 20.0, false, 0.0),
    )
    .expect("manual shrink");
    assert!(!clipped.target.auto_height);
    assert!((clipped.target.height - 20.0).abs() < 0.01);

    let region = (95.0, 135.0, 230.0, 300.0);
    let pixels = render_pixels(&document_id, 0, 2.0);
    let (_, ink_top, _, ink_bottom) = ink_bounds(&pixels, 2.0, region);
    assert!(
        ink_top >= 139.0,
        "clipped box should start at its top: {ink_top}"
    );
    assert!(
        ink_bottom <= 140.0 + 20.0 + 1.5,
        "only the first line may paint inside a 20pt tall box: {ink_bottom}"
    );

    let layer = extract_text_layer(document_id.clone(), 0).expect("clipped layer");
    let page_text = flattened(&layer.text);
    assert!(
        page_text.contains(&flattened(long_text)),
        "clipped text stays in the document and extraction: {page_text:?}"
    );

    // Enlarging the same manual box shows the clipped lines again.
    transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(100.0, 140.0, 120.0, 200.0, false, 0.0),
    )
    .expect("enlarge");
    let pixels = render_pixels(&document_id, 0, 2.0);
    let (_, _, _, ink_bottom) = ink_bounds(&pixels, 2.0, region);
    assert!(
        ink_bottom > 140.0 + 20.0 + 10.0,
        "enlarging must reveal the clipped lines: {ink_bottom}"
    );

    close_document(document_id).expect("close");
}
