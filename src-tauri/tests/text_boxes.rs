//! Command-boundary tests for creating and styling new Text Boxes.
//!
//! Every assertion goes through the Rust functions the Tauri IPC layer exposes.
//! Saved output is reopened through the same command boundary so "the Text Box
//! is really in the document" is checked against a fresh PDFium parse.

mod support;

use support::fixtures;
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

fn open_fixture(category: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, category).expect("fixture category");
    let path = fixtures::fixture_path(fixture);
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open fixture")
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
        strikeout: false,
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
        strikeout: false,
        color: "#c0392b".to_owned(),
        alignment: TextAlignment::Right,
    }
}

fn flattened(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn font_registry_lists_base14_and_the_bundled_cjk_families_with_metadata() {
    let fonts = text_box_fonts();
    assert!(
        fonts.len() >= 20,
        "three base-14 families with four variants each plus the eight bundled CJK fonts"
    );

    for family in ["Helvetica", "Times", "Courier"] {
        let variants: Vec<_> = fonts
            .iter()
            .filter(|font| font.source == "base14" && font.family == family)
            .collect();
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

    let base14: Vec<_> = fonts
        .iter()
        .filter(|font| font.source == "base14")
        .collect();
    assert_eq!(base14.len(), 12);
    for font in base14 {
        assert!(
            font.embeddable,
            "base-14 fonts need no embedding permission"
        );
        assert!(!font.embedded, "base-14 fonts are not embedded programs");
        assert_eq!(font.coverage, "win_ansi");
        assert!(!font.postscript_name.is_empty());
    }

    // The bundled fonts advertise what the embedding pipeline can rely on:
    // TrueType programs under freely embeddable OFL licences with CJK
    // coverage. Each has one upright face; the Noto faces honour bold through
    // their weight axis, the static faces keep their regular outlines, and
    // italic is refused for all of them.
    let bundled: Vec<_> = fonts
        .iter()
        .filter(|font| font.source == "embedded")
        .collect();
    assert_eq!(bundled.len(), 8);
    for font in &bundled {
        assert!(font.embedded && font.embeddable);
        assert_eq!(font.coverage, "cjk");
        assert!(!font.italic, "the bundled fonts have no italic face");
        assert!(
            !font.bold,
            "the registry lists upright faces; bold is a style flag"
        );
        assert!(!font.label.is_empty());
    }
    let noto_sans = bundled
        .iter()
        .find(|font| font.family == "Noto Sans SC")
        .expect("Noto Sans SC is bundled");
    assert_eq!(noto_sans.label, "思源黑体");
    assert_eq!(noto_sans.postscript_name, "NotoSansSC");
    for family in [
        "Noto Serif SC",
        "Zhuque Fangsong",
        "LXGW WenKai",
        "Smiley Sans",
        "Ma Shan Zheng",
        "ZCOOL XiaoWei",
        "ZCOOL QingKe HuangYou",
    ] {
        assert!(
            bundled.iter().any(|font| font.family == family),
            "{family} must be offered by the Text Box font registry"
        );
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

    // Installed fonts (including WPS Office's) are read from this machine, so
    // the list is machine-dependent. Every entry that is listed must be a
    // subsettable TrueType program the resolver can use, and must not shadow
    // a base-14 family.
    let installed: Vec<_> = fonts
        .iter()
        .filter(|font| font.source == "system" || font.source == "wps")
        .collect();
    for font in &installed {
        assert!(!font.embedded, "installed fonts are read from the machine");
        assert!(font.embeddable);
        assert_eq!(font.coverage, "system");
        assert!(!font.label.is_empty());
        assert!(!font.postscript_name.is_empty());
        assert!(!font.family.starts_with('.'));
    }
    for family in ["Helvetica", "Times", "Courier"] {
        assert!(
            installed.iter().all(|font| font.family != family),
            "{family} must stay the base-14 family"
        );
    }
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

    // Chinese is covered by the bundled font and now succeeds; a character the
    // bundled font lacks is still refused before any page object or history
    // entry changes.
    let error = apply_text_edit(document_id.clone(), 0, box_id, "😀".to_owned())
        .expect_err("a missing glyph must be rejected");
    assert_eq!(error.code, CommandErrorCode::UnsupportedFont);
    assert!(
        error.message.contains("Noto Sans SC") && error.message.contains("no glyphs"),
        "the error should name the bundled font and the missing coverage: {}",
        error.message
    );

    // A family that is neither bundled nor installed on this machine is
    // refused with the list of families the editor can use.
    let error = apply_text_box_style(
        document_id.clone(),
        0,
        box_id,
        styled("PDFX Missing Font", 18.0),
    )
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
    assert!(!layer.text.contains("😀"));

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
            strikeout: false,
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

#[test]
fn clicking_existing_text_selects_a_listed_box_with_its_original_geometry() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");

    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target on existing text");
    assert!(!target.created, "existing text must not become a new box");
    assert_eq!(target.text, "Original line.");
    assert_eq!(target.font_family, "Helvetica");
    assert!((target.font_size - 18.0).abs() < 0.01);
    assert_eq!(target.color, "#000000");
    assert!((target.x - 72.0).abs() < 0.01, "x was {}", target.x);
    assert!((target.y - 55.71).abs() < 0.05, "y was {}", target.y);
    assert!(
        (target.width - 100.026).abs() < 0.05,
        "width was {}",
        target.width
    );
    assert!(
        (target.height - 20.088).abs() < 0.05,
        "height was {}",
        target.height
    );
    assert!((target.rotation - 0.0).abs() < 0.01);
    assert!(
        target.auto_height,
        "an existing Text Object sizes itself to its own text"
    );

    // The resolved legacy Text Box is listed, not only created ones, so the
    // overlay survives a reload of the page.
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes.len(), 1, "the resolved legacy box must be listed");
    assert_eq!(boxes[0].box_id, target.box_id);
    assert_eq!(boxes[0], target);

    // Clicking the same Text Object again resolves that record instead of
    // registering a second Text Box.
    let again = text_edit_target(document_id.clone(), 0, char_index_of(&layer, "line") as u32)
        .expect("second target");
    assert_eq!(again.box_id, target.box_id);
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes again");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0], target);

    close_document(document_id).expect("close");
}

#[test]
fn moving_existing_text_leaves_one_copy_and_does_not_reflow_the_page() {
    let source = synthetic::independent_text_rows_pdf();
    let document_id = open_synthetic(&source);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_text = flattened(&layer.text);
    let before_origins: Vec<(f32, f32)> = ["Latin text", "The quick brown fox"]
        .iter()
        .map(|needle| {
            let character = &layer.chars[char_index_of(&layer, needle)];
            (character.origin_x, character.origin_y)
        })
        .collect();

    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "PDFX fixture") as u32,
    )
    .expect("target on existing text");
    let box_id = target.box_id;
    let old_region = (
        target.x - 4.0,
        target.y - 4.0,
        target.x + target.width + 4.0,
        target.y + target.height + 4.0,
    );
    let pixels = render_pixels(&document_id, 0, 2.0);
    let before_ink = ink_bounds(&pixels, 2.0, old_region);
    assert!(
        before_ink.0.is_finite(),
        "the original line must be painted before the move"
    );

    let moved = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(
            120.0,
            400.0,
            target.width,
            target.height,
            target.auto_height,
            0.0,
        ),
    )
    .expect("move existing text");
    assert!((moved.target.x - 120.0).abs() < 0.01);
    assert!((moved.target.y - 400.0).abs() < 0.01);
    assert!(moved.target.auto_height);

    let after = extract_text_layer(document_id.clone(), 0).expect("moved layer");
    assert_eq!(
        flattened(&after.text),
        before_text,
        "moving must not change the page text"
    );
    assert_eq!(
        after.text.matches("PDFX fixture").count(),
        1,
        "the moved line must exist exactly once: {:?}",
        after.text
    );
    let moved_char = &after.chars[char_index_of(&after, "PDFX fixture")];
    assert!(
        (moved_char.origin_x - 120.0).abs() < 0.5,
        "moved origin x was {}",
        moved_char.origin_x
    );
    assert!(
        (moved_char.origin_y
            - (400.0 + layer.chars[char_index_of(&layer, "PDFX fixture")].origin_y - target.y))
            .abs()
            < 0.6,
        "moved baseline was {}",
        moved_char.origin_y
    );

    // The untouched lines keep their exact origins, so removing and
    // re-inserting the moved Text Object did not reflow the paragraph.
    for (needle, before) in ["Latin text", "The quick brown fox"]
        .iter()
        .zip(before_origins)
    {
        let character = &after.chars[char_index_of(&after, needle)];
        assert!((character.origin_x - before.0).abs() < 0.01);
        assert!((character.origin_y - before.1).abs() < 0.01);
    }

    // The old position is blank and the new one is painted: the text was not
    // copied to both places.
    let pixels = render_pixels(&document_id, 0, 2.0);
    let old_ink = ink_bounds(&pixels, 2.0, old_region);
    assert!(
        !old_ink.0.is_finite(),
        "the old position still paints ink: {old_ink:?}"
    );
    let new_ink = ink_bounds(&pixels, 2.0, (112.0, 392.0, 260.0, 430.0));
    assert!(
        new_ink.0.is_finite(),
        "the moved line must be painted at its new position"
    );
    assert!(
        (new_ink.0 - 120.0).abs() < 2.0,
        "moved ink starts at {}",
        new_ink.0
    );

    // The listed overlay follows the moved record.
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after move");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].box_id, box_id);
    assert!((boxes[0].x - 120.0).abs() < 0.01);
    assert!((boxes[0].y - 400.0).abs() < 0.01);

    // The saved PDF reopens at the same visual position and style.
    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("moved-existing-text", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    assert_eq!(flattened(&reopened_layer.text), before_text);
    assert_eq!(reopened_layer.text.matches("PDFX fixture").count(), 1);
    let reopened_char = &reopened_layer.chars[char_index_of(&reopened_layer, "PDFX fixture")];
    assert!(
        (reopened_char.origin_x - 120.0).abs() < 0.6,
        "saved origin x was {}",
        reopened_char.origin_x
    );
    assert!(
        (reopened_char.origin_y - moved_char.origin_y).abs() < 0.02,
        "saved baseline was {}",
        reopened_char.origin_y
    );
    let reopened_target = text_edit_target(
        reopened.clone(),
        0,
        char_index_of(&reopened_layer, "PDFX fixture") as u32,
    )
    .expect("saved target");
    assert_eq!(reopened_target.text, "PDFX fixture: text layer.");
    assert_eq!(reopened_target.font_family, "Helvetica");
    assert!((reopened_target.font_size - 18.0).abs() < 0.01);

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn resizing_existing_text_rewraps_without_duplicating_it() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_text = flattened(&layer.text);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    let box_id = target.box_id;

    let resized = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(target.x, target.y, 70.0, 60.0, false, 0.0),
    )
    .expect("resize existing text");
    assert!((resized.target.width - 70.0).abs() < 0.01);
    assert!((resized.target.height - 60.0).abs() < 0.01);
    assert!(!resized.target.auto_height);
    assert_eq!(resized.target.text, "Original line.");

    let after = extract_text_layer(document_id.clone(), 0).expect("resized layer");
    assert_eq!(flattened(&after.text), before_text);
    assert_eq!(after.text.matches("Original").count(), 1);
    let rows: std::collections::BTreeSet<i32> = after
        .chars
        .iter()
        .filter(|character| character.width > 0.0)
        .map(|character| (character.origin_y * 10.0).round() as i32)
        .collect();
    assert!(
        rows.len() >= 2,
        "a 70pt wide box must wrap the line, got baselines {rows:?}"
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after resize");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].box_id, box_id);
    assert!((boxes[0].width - 70.0).abs() < 0.01);
    assert_eq!(boxes[0].text, target.text);

    // The old, wider copy is gone: nothing paints in the right-hand tail the
    // line used to occupy before it wrapped.
    let pixels = render_pixels(&document_id, 0, 2.0);
    let tail = ink_bounds(&pixels, 2.0, (148.0, 50.0, 180.0, 80.0));
    assert!(
        !tail.0.is_finite(),
        "the pre-resize tail is still painted: {tail:?}"
    );
    let resized_ink = ink_bounds(&pixels, 2.0, (65.0, 50.0, 145.0, 115.0));
    assert!(resized_ink.0.is_finite());
    assert!(
        resized_ink.2 <= target.x + 70.0 + 2.0,
        "resized text overflows its width: right edge {}",
        resized_ink.2
    );

    close_document(document_id).expect("close");
}

#[test]
fn rotating_existing_text_turns_it_and_survives_saving() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_text = flattened(&layer.text);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    let box_id = target.box_id;

    let rotated = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(
            target.x,
            target.y,
            target.width,
            target.height,
            target.auto_height,
            90.0,
        ),
    )
    .expect("rotate existing text");
    assert!((rotated.target.rotation - 90.0).abs() < 0.01);

    let after = extract_text_layer(document_id.clone(), 0).expect("rotated layer");
    assert_eq!(flattened(&after.text), before_text);
    assert_eq!(after.text.matches("Original").count(), 1);
    let first_index = char_index_of(&after, "Original");
    let first = &after.chars[first_index];
    assert!(
        (first.origin_x - (target.x - 18.0 * 0.8)).abs() < 0.8,
        "rotated origin x was {}",
        first.origin_x
    );
    assert!(
        (first.origin_y - target.y).abs() < 0.8,
        "rotated origin y was {}",
        first.origin_y
    );
    let last = &after.chars[first_index + "Original line.".chars().count() - 1];
    assert!(
        last.origin_y > first.origin_y + 20.0,
        "rotated text must run downward: first {first:?}, last {last:?}"
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after rotate");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0].box_id, box_id);
    assert!((boxes[0].rotation - 90.0).abs() < 0.01);

    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("rotated-existing-text", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    assert_eq!(flattened(&reopened_layer.text), before_text);
    assert_eq!(reopened_layer.text.matches("Original").count(), 1);
    let reopened_index = char_index_of(&reopened_layer, "Original");
    let reopened_target =
        text_edit_target(reopened.clone(), 0, reopened_index as u32).expect("saved target");
    assert!(
        (reopened_target.rotation - 90.0).abs() < 1.0,
        "saved rotation was {}",
        reopened_target.rotation
    );
    let reopened_last =
        &reopened_layer.chars[reopened_index + "Original line.".chars().count() - 1];
    assert!(
        reopened_last.origin_y > reopened_layer.chars[reopened_index].origin_y + 20.0,
        "saved text must still run downward"
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn restyling_existing_text_updates_the_whole_box_and_saves_it() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before_text = flattened(&layer.text);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    let box_id = target.box_id;

    let style = TextBoxStyleView {
        font_family: "Times".to_owned(),
        font_size: 16.0,
        bold: true,
        italic: false,
        underline: true,
        strikeout: false,
        color: "#c0392b".to_owned(),
        alignment: TextAlignment::Left,
    };
    let styled = apply_text_box_style(document_id.clone(), 0, box_id, style).expect("restyle");
    assert_eq!(styled.target.font_family, "Times");
    assert!((styled.target.font_size - 16.0).abs() < 0.01);
    assert!(styled.target.bold);
    assert!(!styled.target.italic);
    assert!(styled.target.underline);
    assert_eq!(styled.target.color, "#c0392b");
    assert_eq!(styled.target.text, "Original line.");
    // The style anchors the original baseline, so the text does not jump.
    assert!((styled.target.x - 72.0).abs() < 0.01);
    assert!((styled.target.y - (72.0 - 16.0 * 0.8)).abs() < 0.01);
    assert!(styled.target.auto_height);

    let after = extract_text_layer(document_id.clone(), 0).expect("styled layer");
    assert_eq!(flattened(&after.text), before_text);
    assert_eq!(after.text.matches("Original").count(), 1);
    let character = &after.chars[char_index_of(&after, "Original")];
    assert!(
        (character.font_size - 16.0).abs() < 0.01,
        "layer font size was {}",
        character.font_size
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after style");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0], styled.target);

    // The page paints the new size, colour and underline at the old baseline.
    let pixels = render_pixels(&document_id, 0, 2.0);
    let region = (65.0, 52.0, 220.0, 86.0);
    let (ink_left, _, _, ink_bottom) = ink_bounds(&pixels, 2.0, region);
    assert!((ink_left - 72.0).abs() < 2.0, "ink starts at {ink_left}");
    let baseline = 72.0;
    assert!(
        ink_bottom > baseline + 2.0 && ink_bottom < baseline + 6.0,
        "underline must paint below the baseline: {ink_bottom}"
    );
    let red_pixels = pixels
        .rgba
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 150 && pixel[1] < 110 && pixel[2] < 110)
        .count();
    assert!(
        red_pixels > 0,
        "the red Text Box colour must reach the rendered page"
    );

    // Saving keeps the whole-box style.
    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("styled-existing-text", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    assert_eq!(flattened(&reopened_layer.text), before_text);
    assert_eq!(reopened_layer.text.matches("Original").count(), 1);
    let reopened_index = char_index_of(&reopened_layer, "Original");
    let reopened_character = &reopened_layer.chars[reopened_index];
    assert!((reopened_character.font_size - 16.0).abs() < 0.01);
    let reopened_target =
        text_edit_target(reopened.clone(), 0, reopened_index as u32).expect("saved target");
    assert!(
        reopened_target.font_family.contains("Times"),
        "saved family was {:?}",
        reopened_target.font_family
    );
    assert!(reopened_target.bold);
    assert_eq!(reopened_target.color, "#c0392b");
    let reopened_pixels = render_pixels(&reopened, 0, 2.0);
    let (_, _, _, reopened_bottom) = ink_bounds(&reopened_pixels, 2.0, region);
    assert!(
        reopened_bottom > baseline + 2.0,
        "the saved underline must render: bottom {reopened_bottom}"
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn undo_and_redo_existing_text_edits_reach_past_a_save() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let before = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    let box_id = before.box_id;

    let moved = transform_text_box(
        document_id.clone(),
        0,
        box_id,
        transform(
            160.0,
            300.0,
            before.width,
            before.height,
            before.auto_height,
            0.0,
        ),
    )
    .expect("move")
    .target;
    let styled_target =
        apply_text_box_style(document_id.clone(), 0, box_id, styled("Courier", 15.0))
            .expect("style")
            .target;

    // Saving sets the clean baseline but keeps history usable.
    let bytes = save_document_bytes(&document_id).expect("save");
    assert!(!bytes.is_empty());
    let saved = mark_document_saved(document_id.clone()).expect("mark saved");
    assert!(!saved.dirty);
    assert!(saved.can_undo);

    undo_text_edit(document_id.clone()).expect("undo style");
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after style undo");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0], moved);

    undo_text_edit(document_id.clone()).expect("undo move");
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after move undo");
    assert_eq!(boxes.len(), 1);
    assert_eq!(boxes[0], before);
    let restored = extract_text_layer(document_id.clone(), 0).expect("restored layer");
    let restored_char = &restored.chars[char_index_of(&restored, "Original")];
    assert!((restored_char.origin_x - 72.0).abs() < 0.5);
    assert!((restored_char.origin_y - 72.0).abs() < 0.6);

    redo_text_edit(document_id.clone()).expect("redo move");
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after move redo");
    assert_eq!(boxes[0], moved);
    redo_text_edit(document_id.clone()).expect("redo style");
    let boxes = text_boxes(document_id.clone(), 0).expect("boxes after style redo");
    assert_eq!(boxes[0], styled_target);

    // The saved snapshot still contains the styled state; later undos do not
    // edit the file that was written.
    let reopened = open_synthetic(&synthetic::pdf_bytes_file("undo-existing-text", bytes));
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    assert_eq!(flattened(&reopened_layer.text), "Original line.");
    let reopened_char = &reopened_layer.chars[char_index_of(&reopened_layer, "Original")];
    assert!((reopened_char.font_size - 15.0).abs() < 0.01);
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn text_boxes_lists_resolved_strings_in_stable_order_and_hides_deleted_ones() {
    let source = synthetic::independent_text_rows_pdf();
    let document_id = open_synthetic(&source);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let third = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "The quick brown fox") as u32,
    )
    .expect("third line target");
    let first = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "PDFX fixture") as u32,
    )
    .expect("first line target");
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(320.0, 400.0, 150.0, 50.0),
    )
    .expect("create")
    .target;

    let expected = vec![third.box_id, first.box_id, created.box_id];
    let listed: Vec<u64> = text_boxes(document_id.clone(), 0)
        .expect("boxes")
        .iter()
        .map(|box_view| box_view.box_id)
        .collect();
    assert_eq!(
        listed, expected,
        "the list must follow registration order, not a hash order"
    );
    let again: Vec<u64> = text_boxes(document_id.clone(), 0)
        .expect("boxes again")
        .iter()
        .map(|box_view| box_view.box_id)
        .collect();
    assert_eq!(again, expected, "listing twice must not reorder the boxes");

    delete_text_box(document_id.clone(), 0, third.box_id).expect("delete third line");
    let listed: Vec<u64> = text_boxes(document_id.clone(), 0)
        .expect("boxes after delete")
        .iter()
        .map(|box_view| box_view.box_id)
        .collect();
    assert_eq!(
        listed,
        vec![first.box_id, created.box_id],
        "a deleted Text Box must not stay in the list as a tombstone"
    );
    let after_delete = extract_text_layer(document_id.clone(), 0).expect("layer after delete");
    assert!(!after_delete.text.contains("The quick brown fox"));
    assert!(after_delete.text.contains("PDFX fixture: text layer."));
    assert!(after_delete
        .text
        .contains("Latin text is extractable from this page."));

    undo_text_edit(document_id.clone()).expect("undo delete");
    let listed: Vec<u64> = text_boxes(document_id.clone(), 0)
        .expect("boxes after undo")
        .iter()
        .map(|box_view| box_view.box_id)
        .collect();
    assert_eq!(listed, expected, "undo restores the box in its old slot");
    let restored = extract_text_layer(document_id.clone(), 0).expect("restored layer");
    assert!(restored.text.contains("The quick brown fox"));

    close_document(document_id).expect("close");
}

#[test]
fn moving_existing_text_on_a_rotated_page_keeps_its_orientation() {
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("rotated layer");
    assert_eq!(layer.rotation, 90);
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Rotate 90") as u32,
    )
    .expect("rotated target");
    assert!(
        (target.rotation - 90.0).abs() < 0.01,
        "target rotation was {}",
        target.rotation
    );
    let before_char = &layer.chars[char_index_of(&layer, "Rotate 90")];

    let moved = transform_text_box(
        document_id.clone(),
        0,
        target.box_id,
        transform(
            target.x + 40.0,
            target.y + 30.0,
            target.width,
            target.height,
            target.auto_height,
            target.rotation,
        ),
    )
    .expect("move on rotated page");
    assert!((moved.target.rotation - 90.0).abs() < 0.01);

    let after = extract_text_layer(document_id.clone(), 0).expect("moved rotated layer");
    assert_eq!(flattened(&after.text), "Rotate 90");
    assert_eq!(after.text.matches("Rotate 90").count(), 1);
    let after_char = &after.chars[char_index_of(&after, "Rotate 90")];
    assert!(
        (after_char.origin_x - before_char.origin_x - 40.0).abs() < 2.0,
        "rotated origin x moved from {} to {}, not by 40pt",
        before_char.origin_x,
        after_char.origin_x
    );
    assert!(
        (after_char.origin_y - before_char.origin_y - 30.0).abs() < 2.0,
        "rotated origin y moved from {} to {}, not by 30pt",
        before_char.origin_y,
        after_char.origin_y
    );
    // The text still starts inside the requested box and runs downward.
    assert!(after_char.origin_x >= target.x + 40.0 - 16.0);
    assert!(after_char.origin_x <= target.x + 40.0 + 1.0);
    assert!(after_char.origin_y >= target.y + 30.0 - 1.0);
    assert!(after_char.origin_y <= target.y + 30.0 + 2.0);
    let last = &after.chars[after.chars.len() - 1];
    assert!(
        last.origin_y > after_char.origin_y + 20.0,
        "rotated text must keep running downward: first {after_char:?}, last {last:?}"
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes.len(), 1);
    assert!((boxes[0].rotation - 90.0).abs() < 0.01);

    let bytes = save_document_bytes(&document_id).expect("save");
    let saved = synthetic::pdf_bytes_file("moved-rotated-existing-text", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened rotated layer");
    assert_eq!(reopened_layer.rotation, 90);
    assert_eq!(flattened(&reopened_layer.text), "Rotate 90");
    let reopened_index = char_index_of(&reopened_layer, "Rotate 90");
    let reopened_target =
        text_edit_target(reopened.clone(), 0, reopened_index as u32).expect("saved target");
    assert!(
        (reopened_target.rotation - 90.0).abs() < 1.0,
        "saved rotation was {}",
        reopened_target.rotation
    );
    let reopened_char = &reopened_layer.chars[reopened_index];
    assert!((reopened_char.origin_x - after_char.origin_x).abs() < 1.0);
    assert!((reopened_char.origin_y - after_char.origin_y).abs() < 1.0);

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn strikeout_is_visible_saved_and_undoable() {
    let document_id = open_fixture("text");
    let created = create_text_box(
        document_id.clone(),
        0,
        create_request(72.0, 200.0, 300.0, 60.0),
    )
    .unwrap();
    let box_id = created.target.box_id;
    apply_text_edit(
        document_id.clone(),
        0,
        box_id,
        "Strikethrough verification".to_owned(),
    )
    .unwrap();
    let before = render_page(document_id.clone(), 0, 1.0).unwrap().png_base64;
    let mut style = default_style();
    style.strikeout = true;
    apply_text_box_style(document_id.clone(), 0, box_id, style).unwrap();
    assert!(text_boxes(document_id.clone(), 0)
        .unwrap()
        .iter()
        .any(|b| b.box_id == box_id && b.strikeout));
    let after = render_page(document_id.clone(), 0, 1.0).unwrap().png_base64;
    assert_ne!(before, after);
    let bytes = save_document_bytes(&document_id).unwrap();
    let saved = synthetic::pdf_bytes_file("strikeout-saved", bytes);
    let reopened = open_synthetic(&saved);
    assert_eq!(
        render_page(reopened.clone(), 0, 1.0).unwrap().png_base64,
        after
    );
    undo_text_edit(document_id.clone()).unwrap();
    assert_eq!(
        render_page(document_id.clone(), 0, 1.0).unwrap().png_base64,
        before
    );
    close_document(document_id).unwrap();
    close_document(reopened).unwrap();
}

#[test]
fn drag_preview_reflows_with_real_fonts_without_changing_document_or_history() {
    use pdfx_lib::text_edit::preview_text_box_transform;
    let id = open_fixture("text");
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let target = text_edit_target(id.clone(), 0, char_index_of(&layer, "PDFX") as u32).unwrap();
    let before = render_page(id.clone(), 0, 1.0).unwrap().png_base64;
    let history = edit_history(id.clone()).unwrap();
    let request = TextBoxTransformRequest {
        x: target.x + 40.0,
        y: target.y + 30.0,
        width: 65.0,
        height: target.font_size * 1.2,
        rotation: target.rotation,
        auto_height: true,
    };
    let preview =
        preview_text_box_transform(id.clone(), 0, target.box_id, request.clone(), 1.0).unwrap();
    assert_ne!(preview.image.png_base64, before);
    assert_eq!(preview.target.font_family, target.font_family);
    assert_eq!(preview.target.font_size, target.font_size);
    assert!(preview.target.height > target.height);
    assert_eq!(render_page(id.clone(), 0, 1.0).unwrap().png_base64, before);
    assert_eq!(edit_history(id.clone()).unwrap(), history);
    let committed = transform_text_box(id.clone(), 0, target.box_id, request.clone()).unwrap();
    assert_eq!(
        render_pixels(&id, 0, 1.0).rgba,
        decode_png(&preview.image.png_base64).rgba
    );
    undo_text_edit(id.clone()).unwrap();
    let undo_history = edit_history(id.clone()).unwrap();
    preview_text_box_transform(id.clone(), 0, target.box_id, request, 1.0).unwrap();
    assert_eq!(edit_history(id.clone()).unwrap(), undo_history);
    redo_text_edit(id.clone()).unwrap();
    assert_eq!(
        text_boxes(id.clone(), 0).unwrap()[0].width,
        committed.target.width
    );
    let saved = synthetic::pdf_bytes_file("drag-preview-reflow", save_document_bytes(&id).unwrap());
    let reopened = open_synthetic(&saved);
    assert_eq!(
        render_pixels(&reopened, 0, 1.0).rgba,
        decode_png(&preview.image.png_base64).rgba
    );
    close_document(id).unwrap();
    close_document(reopened).unwrap();
}

#[test]
fn resizing_original_cjk_and_embedded_text_preserves_font_and_reverses_wrapping() {
    for pdf in [
        synthetic::cjk_fangsong_pdf(),
        synthetic::embedded_true_type_pdf(),
    ] {
        let id = open_synthetic(&pdf);
        let target = text_edit_target(id.clone(), 0, 0).unwrap();
        let narrow = transform_text_box(
            id.clone(),
            0,
            target.box_id,
            TextBoxTransformRequest {
                x: target.x,
                y: target.y,
                width: 40.0,
                height: target.font_size * 1.2,
                rotation: target.rotation,
                auto_height: true,
            },
        )
        .unwrap()
        .target;
        assert_eq!(narrow.font_family, target.font_family);
        assert_eq!(narrow.font_size, target.font_size);
        assert!(narrow.height > target.font_size * 1.2);
        let wide = transform_text_box(
            id.clone(),
            0,
            target.box_id,
            TextBoxTransformRequest {
                x: target.x,
                y: target.y,
                width: 400.0,
                height: target.font_size * 1.2,
                rotation: target.rotation,
                auto_height: true,
            },
        )
        .unwrap()
        .target;
        assert_eq!(wide.font_family, target.font_family);
        assert_eq!(wide.font_size, target.font_size);
        assert!(wide.height < narrow.height);
        let text = extract_text(id.clone(), 0)
            .unwrap()
            .text
            .replace(['\r', '\n'], "");
        assert!(text.contains(&target.text));
        let pixels = render_page(id.clone(), 0, 1.0).unwrap().png_base64;
        let saved =
            synthetic::pdf_bytes_file("resized-original-font", save_document_bytes(&id).unwrap());
        let reopened = open_synthetic(&saved);
        assert_eq!(
            render_page(reopened.clone(), 0, 1.0).unwrap().png_base64,
            pixels
        );
        close_document(id).unwrap();
        close_document(reopened).unwrap();
    }
}

#[test]
fn resized_cjk_phone_stays_inside_the_requested_line_width() {
    let pdf = synthetic::cjk_phone_pdf();
    let id = open_synthetic(&pdf);
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    let narrow = transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x,
            y: target.y,
            width: 74.0,
            height: target.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap()
    .target;
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    for c in layer.chars.iter().filter(|c| !c.generated && c.width > 0.0) {
        assert!(
            c.x + c.width <= narrow.x + narrow.width + 1.0,
            "{} extends to {}, frame ends at {}",
            c.text,
            c.x + c.width,
            narrow.x + narrow.width
        );
    }
    close_document(id).unwrap();
}

#[test]
fn reopened_wrapped_original_text_can_be_selected_and_widened_as_one_box() {
    let pdf = synthetic::cjk_phone_pdf();
    let id = open_synthetic(&pdf);
    let original = text_edit_target(id.clone(), 0, 0).unwrap();
    transform_text_box(
        id.clone(),
        0,
        original.box_id,
        TextBoxTransformRequest {
            x: original.x,
            y: original.y,
            width: 74.0,
            height: original.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap();
    let saved =
        synthetic::pdf_bytes_file("reopen-wrapped-phone", save_document_bytes(&id).unwrap());
    let reopened = open_synthetic(&saved);
    let selected = text_edit_target(reopened.clone(), 0, 0).unwrap();
    assert_eq!(selected.text, "测试号码-0000");
    let wide = transform_text_box(
        reopened.clone(),
        0,
        selected.box_id,
        TextBoxTransformRequest {
            x: selected.x,
            y: selected.y,
            width: 400.0,
            height: selected.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap()
    .target;
    assert!(wide.height < selected.height);
    let text = extract_text(reopened.clone(), 0).unwrap().text;
    assert_eq!(text.matches("测试号码").count(), 1);
    close_document(id).unwrap();
    close_document(reopened).unwrap();
}

#[test]
fn adjacent_paragraph_lines_share_a_selection_but_table_rows_do_not() {
    let id = open_fixture("text");
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let first = text_edit_target(id.clone(), 0, 0).unwrap();
    let second =
        text_edit_target(id.clone(), 0, char_index_of(&layer, "Latin text") as u32).unwrap();
    assert_eq!(first.box_id, second.box_id);
    assert!(first.text.contains("PDFX fixture"));
    assert!(first.text.contains("Latin text"));
    assert!(!edit_history(id.clone()).unwrap().dirty);
    close_document(id).unwrap();
    let pdf = synthetic::independent_text_rows_pdf();
    let id = open_synthetic(&pdf);
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let first = text_edit_target(id.clone(), 0, 0).unwrap();
    let second =
        text_edit_target(id.clone(), 0, char_index_of(&layer, "Latin text") as u32).unwrap();
    assert_ne!(first.box_id, second.box_id);
    close_document(id).unwrap();
}

#[test]
fn moving_a_selected_paragraph_preserves_every_glyph_baseline_and_exact_undo() {
    let id = open_fixture("text");
    let before = extract_text_layer(id.clone(), 0).unwrap();
    let original_pixels = render_page(id.clone(), 0, 1.0).unwrap().png_base64;
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x + 35.0,
            y: target.y + 70.0,
            width: target.width,
            height: target.height,
            rotation: target.rotation,
            auto_height: target.auto_height,
        },
    )
    .unwrap();
    let moved = extract_text_layer(id.clone(), 0).unwrap();
    assert_eq!(moved.text, before.text);
    assert_eq!(moved.chars.len(), before.chars.len());
    for (old, new) in before
        .chars
        .iter()
        .zip(moved.chars.iter())
        .filter(|(old, _)| !old.generated)
    {
        assert!((new.origin_x - old.origin_x - 35.0).abs() < 0.02);
        assert!((new.origin_y - old.origin_y - 70.0).abs() < 0.02);
        assert_eq!(new.font_size, old.font_size);
    }
    let moved_pixels = render_page(id.clone(), 0, 1.0).unwrap().png_base64;
    undo_text_edit(id.clone()).unwrap();
    assert_eq!(
        render_page(id.clone(), 0, 1.0).unwrap().png_base64,
        original_pixels
    );
    redo_text_edit(id.clone()).unwrap();
    assert_eq!(
        render_page(id.clone(), 0, 1.0).unwrap().png_base64,
        moved_pixels
    );
    close_document(id).unwrap();
}

#[test]
fn vertical_frame_resize_keeps_paragraph_layout_and_never_clips_its_text() {
    let id = open_fixture("text");
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    let before = render_page(id.clone(), 0, 1.0).unwrap().png_base64;
    let tall = transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x,
            y: target.y,
            width: target.width,
            height: 250.0,
            auto_height: true,
            rotation: 0.0,
        },
    )
    .unwrap()
    .target;
    assert_eq!(tall.height, 250.0);
    assert_eq!(render_page(id.clone(), 0, 1.0).unwrap().png_base64, before);
    let short = transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x,
            y: target.y,
            width: target.width,
            height: 4.0,
            auto_height: true,
            rotation: 0.0,
        },
    )
    .unwrap()
    .target;
    assert!(short.height > 4.0);
    assert!(short.height < 250.0);
    assert_eq!(render_page(id.clone(), 0, 1.0).unwrap().png_base64, before);
    close_document(id).unwrap();
}

#[test]
fn paragraph_width_reflow_preserves_original_line_spacing() {
    let id = open_fixture("text");
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    assert_eq!(target.line_height, Some(28.0));
    let resized = transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x,
            y: target.y,
            width: 200.0,
            height: target.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap()
    .target;
    assert_eq!(resized.font_size, target.font_size);
    assert_eq!(resized.line_height, Some(28.0));
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let mut rows: Vec<_> = layer
        .chars
        .iter()
        .filter(|c| !c.generated && c.width > 0.0)
        .map(|c| (c.origin_y * 100.0).round() as i32)
        .collect();
    rows.sort();
    rows.dedup();
    assert!(rows.len() > 3);
    assert!(rows
        .windows(2)
        .all(|pair| (pair[1] - pair[0] - 2800).abs() <= 2));
    close_document(id).unwrap();
}

#[test]
fn line_ending_hyphens_survive_real_save_reopen_and_a_second_edit() {
    use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
    let pdf = synthetic::cjk_phone_pdf();
    let id = open_synthetic(&pdf);
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x,
            y: target.y,
            width: 90.0,
            height: target.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap();
    let dir = support::temp::TempDir::new("line-end-hyphen");
    let path = dir.path().join("saved.pdf");
    save_document_with_progress(
        &id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |_| {},
    )
    .unwrap();
    let reopened = open_document(path.to_string_lossy().into_owned(), None)
        .unwrap()
        .document_id;
    let layer = extract_text_layer(reopened.clone(), 0).unwrap();
    assert!(
        !layer.text.contains('\u{2}'),
        "internal hyphen marker leaked: {:?}",
        layer.text
    );
    let box_view = text_edit_target(reopened.clone(), 0, 0).unwrap();
    assert_eq!(box_view.text.trim_end(), "测试号码-0000");
    transform_text_box(
        reopened.clone(),
        0,
        box_view.box_id,
        TextBoxTransformRequest {
            x: box_view.x,
            y: box_view.y,
            width: 400.0,
            height: box_view.font_size * 1.2,
            rotation: 0.0,
            auto_height: true,
        },
    )
    .unwrap();
    assert!(extract_text(reopened.clone(), 0)
        .unwrap()
        .text
        .contains("测试号码-0000"));
    close_document(id).unwrap();
    close_document(reopened).unwrap();
}

#[test]
fn per_glyph_wrapped_lines_select_as_one_content_box() {
    let pdf = synthetic::glyph_by_glyph_wrapped_phone_pdf();
    let id = open_synthetic(&pdf);
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let first = text_edit_target(id.clone(), 0, 0).unwrap();
    assert_eq!(first.text.trim_end(), "Phone-0000");
    let next = text_edit_target(id.clone(), 0, char_index_of(&layer, "0000") as u32).unwrap();
    assert_eq!(first.box_id, next.box_id);
    close_document(id).unwrap();
    let id = open_synthetic(&pdf);
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let from_second =
        text_edit_target(id.clone(), 0, char_index_of(&layer, "0000") as u32).unwrap();
    assert_eq!(from_second.text.trim_end(), "Phone-0000");
    close_document(id).unwrap();
}

#[test]
fn appending_text_to_a_resized_original_box_grows_its_frame() {
    let pdf = synthetic::embedded_true_type_pdf();
    let id = open_synthetic(&pdf);
    let original = text_edit_target(id.clone(), 0, 0).unwrap();
    let resized = transform_text_box(
        id.clone(),
        0,
        original.box_id,
        TextBoxTransformRequest {
            x: original.x,
            y: original.y,
            width: 100.0,
            height: original.font_size * 1.2,
            auto_height: true,
            rotation: 0.0,
        },
    )
    .unwrap()
    .target;
    apply_text_edit(
        id.clone(),
        0,
        resized.box_id,
        format!("{} 哈哈哈哈哈哈哈哈哈哈哈哈", resized.text),
    )
    .unwrap();
    let current = text_boxes(id.clone(), 0)
        .unwrap()
        .into_iter()
        .find(|b| b.box_id == resized.box_id)
        .unwrap();
    let layer = extract_text_layer(id.clone(), 0).unwrap();
    let bottom = layer
        .chars
        .iter()
        .filter(|c| !c.generated && c.width > 0.0)
        .map(|c| c.y + c.height)
        .fold(0.0, f32::max);
    assert!(
        current.height > resized.height,
        "frame did not grow: {} -> {}",
        resized.height,
        current.height
    );
    assert!(
        current.y + current.height + 2.0 >= bottom,
        "new characters are outside the editable frame"
    );
    close_document(id).unwrap();
}

#[test]
fn cached_move_pixels_match_native_translation_and_do_not_mutate_history_or_save() {
    use pdfx_lib::text_edit::prepare_text_box_move_preview;
    let source = synthetic::embedded_true_type_pdf();
    let id = open_synthetic(&source);
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    let before = render_pixels(&id, 0, 2.0);
    let history = edit_history(id.clone()).unwrap();
    let saved_before = save_document_bytes(&id).unwrap();
    let prepared = prepare_text_box_move_preview(id.clone(), 0, target.box_id, 2.0)
        .unwrap()
        .expect("ordinary text uses cached pixels");
    assert_eq!(render_pixels(&id, 0, 2.0).rgba, before.rgba);
    assert_eq!(edit_history(id.clone()).unwrap(), history);
    // PDFium may regenerate trailer IDs on each serialization. Page objects,
    // font streams and content streams must stay byte-for-byte identical.
    let after_save = lopdf::Document::load_mem(&save_document_bytes(&id).unwrap()).unwrap();
    let before_save = lopdf::Document::load_mem(&saved_before).unwrap();
    assert!(
        after_save.objects == before_save.objects,
        "capture changed saved PDF objects"
    );
    let foreground = decode_png(&prepared.foreground.png_base64);
    let mut background = decode_png(&prepared.background.png_base64);
    assert!(foreground.rgba.chunks_exact(4).any(|p| p[3] == 0));
    assert!(foreground.rgba.chunks_exact(4).any(|p| p[3] > 0));
    let (dx, dy) = (24u32, 16u32);
    for y in 0..foreground.height {
        for x in 0..foreground.width {
            let src = ((y * foreground.width + x) * 4) as usize;
            let dest = (((y + prepared.foreground.y + dy) * background.width
                + x
                + prepared.foreground.x
                + dx)
                * 4) as usize;
            let alpha = u32::from(foreground.rgba[src + 3]);
            for c in 0..3 {
                background.rgba[dest + c] = ((u32::from(foreground.rgba[src + c]) * alpha
                    + u32::from(background.rgba[dest + c]) * (255 - alpha)
                    + 127)
                    / 255) as u8;
            }
        }
    }
    transform_text_box(
        id.clone(),
        0,
        target.box_id,
        TextBoxTransformRequest {
            x: target.x + dx as f32 / 2.0,
            y: target.y + dy as f32 / 2.0,
            width: target.width,
            height: target.height,
            rotation: target.rotation,
            auto_height: target.auto_height,
        },
    )
    .unwrap();
    let moved = render_pixels(&id, 0, 2.0);
    let max_difference = moved
        .rgba
        .iter()
        .zip(&background.rgba)
        .map(|(a, b)| (i16::from(*a) - i16::from(*b)).abs())
        .max()
        .unwrap();
    assert!(
        max_difference <= 4,
        "sprite/native pixels differ by {max_difference}"
    );
    undo_text_edit(id.clone()).unwrap();
    let before_redo = edit_history(id.clone()).unwrap();
    prepare_text_box_move_preview(id.clone(), 0, target.box_id, 2.0).unwrap();
    assert_eq!(edit_history(id.clone()).unwrap(), before_redo);
    redo_text_edit(id.clone()).unwrap();
    assert_eq!(render_pixels(&id, 0, 2.0).rgba, moved.rgba);
    close_document(id).unwrap();
}

#[test]
fn cached_move_falls_back_when_other_content_occludes_the_text() {
    use pdfx_lib::text_edit::prepare_text_box_move_preview;
    let source = synthetic::embedded_true_type_pdf();
    let mut pdf = lopdf::Document::load(source.path_string()).unwrap();
    let stream = pdf.get_object_mut((7, 0)).unwrap().as_stream_mut().unwrap();
    let mut content = stream.content.clone();
    content.extend_from_slice(b"q 1 0 0 rg 100 710 100 30 re f Q\n");
    stream.set_content(content);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let source = synthetic::pdf_bytes_file("occluded-drag", bytes);
    let id = open_synthetic(&source);
    let target = text_edit_target(id.clone(), 0, 0).unwrap();
    let before = render_pixels(&id, 0, 2.0);
    assert!(
        prepare_text_box_move_preview(id.clone(), 0, target.box_id, 2.0)
            .unwrap()
            .is_none()
    );
    assert_eq!(render_pixels(&id, 0, 2.0).rgba, before.rgba);
    assert!(!edit_history(id.clone()).unwrap().dirty);
    close_document(id).unwrap();
}
