//! Command-boundary tests for selection-level mixed text styling.
//!
//! A style applied to a character range of one Text Box becomes a run on the
//! box record and is rendered as extra text objects next to the untouched
//! text. Every assertion goes through the same Rust functions the Tauri IPC
//! layer exposes; saved output is reopened through the command boundary so the
//! runs are checked in a fresh PDFium parse.

mod support;

use support::fixtures;
use support::synthetic;

use pdfx_lib::commands::{
    extract_text, extract_text_layer, open_document, render_page, save_document_bytes,
    CommandErrorCode, TextLayerView,
};
use pdfx_lib::text_edit::{
    apply_text_box_selection_style, apply_text_edit, mark_document_saved, redo_text_edit,
    text_boxes, text_edit_target, undo_text_edit, CreateTextBoxRequest, TextAlignment,
    TextBoxStyleView, TextRangeView,
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

fn base_style() -> TextBoxStyleView {
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

fn selected_style() -> TextBoxStyleView {
    TextBoxStyleView {
        font_family: "Times".to_owned(),
        font_size: 24.0,
        bold: true,
        italic: false,
        underline: true,
        strikeout: false,
        color: "#c0392b".to_owned(),
        alignment: TextAlignment::Left,
    }
}

fn styled_text_box(document_id: &str, text: &str) -> u64 {
    let created = pdfx_lib::text_edit::create_text_box(
        document_id.to_owned(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 100.0,
            width: 320.0,
            height: 36.0,
            style: Some(base_style()),
        },
    )
    .expect("create Text Box");
    let box_id = created.target.box_id;
    apply_text_edit(document_id.to_owned(), 0, box_id, text.to_owned()).expect("type into box");
    box_id
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

fn count_pixels(
    pixels: &DecodedPng,
    scale: f32,
    region: (f32, f32, f32, f32),
    predicate: impl Fn([u8; 4]) -> bool,
) -> usize {
    let mut count = 0;
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        if x < region.0 || x > region.2 || y < region.1 || y > region.3 {
            continue;
        }
        if predicate([pixel[0], pixel[1], pixel[2], pixel[3]]) {
            count += 1;
        }
    }
    count
}

fn is_red(pixel: [u8; 4]) -> bool {
    let (red, green, blue) = (
        u16::from(pixel[0]),
        u16::from(pixel[1]),
        u16::from(pixel[2]),
    );
    red > 120 && red > green + 40 && red > blue + 40
}

fn is_dark(pixel: [u8; 4]) -> bool {
    pixel[0] < 100 && pixel[1] < 100 && pixel[2] < 100 && pixel[3] > 0
}

#[test]
fn styling_a_character_range_keeps_the_rest_of_the_box_unchanged() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");

    let before = render_pixels(&document_id, 0, 2.0);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("layer before");
    let before_index = char_index_of(&before_layer, "Selected line.");
    assert_eq!(before_layer.chars[before_index].font_size.round(), 16.0);

    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        selected_style(),
    )
    .expect("style the selection");

    // The logical text is untouched and only the selected range carries a run.
    assert_eq!(result.target.text, "Selected line.");
    assert_eq!(
        result.target.runs.len(),
        1,
        "runs: {:?}",
        result.target.runs
    );
    let run = &result.target.runs[0];
    assert_eq!((run.start, run.end), (9, 13));
    assert_eq!(run.font_family, "Times");
    assert!((run.font_size - 24.0).abs() < 0.01);
    assert!(run.bold && run.underline);
    assert_eq!(run.color, "#c0392b");
    // Everything outside the run keeps the box's base style.
    assert_eq!(result.target.font_family, "Helvetica");
    assert!((result.target.font_size - 16.0).abs() < 0.01);
    assert_eq!(result.target.color, "#000000");
    assert!(!result.target.bold && !result.target.underline);
    assert!(result.history.can_undo);
    assert!(result.history.dirty);

    // The page text still reads the same, without duplicated segments.
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after");
    assert_eq!(
        layer.text, before_layer.text,
        "extracted text must not change"
    );
    let index = char_index_of(&layer, "Selected line.");
    assert_eq!(index, before_index);

    // Unselected characters keep their size and sit on the line's shared
    // baseline (a mixed line uses the largest font for its line box).
    let line_y = layer.chars[index].origin_y;
    let original = &layer.chars[index..index + 8];
    for character in original {
        assert!(
            (character.font_size - 16.0).abs() < 0.01,
            "unselected text changed size: {character:?}"
        );
        assert!(
            (character.origin_y - line_y).abs() < 0.01,
            "unselected text must stay on the line baseline"
        );
    }

    let selected = &layer.chars[index + 9..index + 13];
    for character in selected {
        assert!(
            (character.font_size - 24.0).abs() < 0.01,
            "selected text did not change size: {character:?}"
        );
        assert!(
            (character.origin_y - line_y).abs() < 0.01,
            "mixed sizes must share one baseline"
        );
    }

    // Colour and underline are only visible in pixels: the selected glyph range
    // is red and has an underline, the untouched prefix stays black.
    let last_selected = &layer.chars[index + 12];
    let first_selected = &layer.chars[index + 9];
    let selected_region = (
        first_selected.origin_x - 1.0,
        first_selected.origin_y - 24.0,
        last_selected.origin_x + 1.0,
        first_selected.origin_y + 3.0,
    );
    assert!(
        count_pixels(&before, 2.0, selected_region, is_red) == 0,
        "nothing was red before the style was applied"
    );
    let after = render_pixels(&document_id, 0, 2.0);
    assert!(
        count_pixels(&after, 2.0, selected_region, is_red) > 10,
        "selected glyphs should be red"
    );
    let underline_region = (
        first_selected.origin_x - 1.0,
        first_selected.origin_y + 2.0,
        last_selected.origin_x + 1.0,
        first_selected.origin_y + 8.0,
    );
    assert!(
        count_pixels(&after, 2.0, underline_region, is_red) > 10,
        "selected glyphs should be underlined"
    );

    let base_region = (
        layer.chars[index].origin_x - 1.0,
        layer.chars[index].origin_y - 16.0,
        layer.chars[index + 7].origin_x,
        layer.chars[index].origin_y + 3.0,
    );
    assert!(
        count_pixels(&after, 2.0, base_region, is_dark) > 10,
        "unselected text should still be drawn in black"
    );
    assert!(
        count_pixels(&after, 2.0, base_region, is_red) == 0,
        "unselected text must not turn red"
    );
}

fn box_runs(document_id: &str, box_id: u64) -> Vec<pdfx_lib::text_edit::TextBoxRunView> {
    text_boxes(document_id.to_owned(), 0)
        .expect("list boxes")
        .into_iter()
        .find(|box_view| box_view.box_id == box_id)
        .expect("box is listed")
        .runs
}

#[test]
fn one_undo_restores_the_previous_runs_and_redo_reapplies_them() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");

    let before_style = render_pixels(&document_id, 0, 2.0);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("layer before");
    let before_history =
        pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history before");

    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        selected_style(),
    )
    .expect("style the selection");
    assert_eq!(
        result.history.revision,
        before_history.revision + 1,
        "one style operation is one revision"
    );

    let styled = render_pixels(&document_id, 0, 2.0);
    let styled_layer = extract_text_layer(document_id.clone(), 0).expect("layer styled");
    assert!(
        styled.rgba != before_style.rgba,
        "the style changed the page"
    );

    // One undo step reverts the whole style operation, not one page object.
    let undone = undo_text_edit(document_id.clone()).expect("undo");
    assert!(undone.history.can_redo);
    assert!(
        box_runs(&document_id, box_id).is_empty(),
        "undo must remove the run"
    );
    let undone_layer = extract_text_layer(document_id.clone(), 0).expect("layer undone");
    assert_eq!(undone_layer.text, before_layer.text);
    let index = char_index_of(&undone_layer, "Selected line.");
    for character in &undone_layer.chars[index..index + 14] {
        assert!(
            (character.font_size - 16.0).abs() < 0.01,
            "undo must restore the base size: {character:?}"
        );
    }
    let undone_pixels = render_pixels(&document_id, 0, 2.0);
    assert_eq!(
        undone_pixels.rgba, before_style.rgba,
        "undo restores the exact previous pixels"
    );

    let redone = redo_text_edit(document_id.clone()).expect("redo");
    assert!(redone.history.can_undo);
    let runs = box_runs(&document_id, box_id);
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].start, runs[0].end), (9, 13));
    let redone_layer = extract_text_layer(document_id.clone(), 0).expect("layer redone");
    assert_eq!(redone_layer.text, styled_layer.text);
    let index = char_index_of(&redone_layer, "Selected line.");
    for character in &redone_layer.chars[index + 9..index + 13] {
        assert!((character.font_size - 24.0).abs() < 0.01);
    }
    let redone_pixels = render_pixels(&document_id, 0, 2.0);
    assert_eq!(
        redone_pixels.rgba, styled.rgba,
        "redo reapplies the exact styled pixels"
    );
}

/// Counts xref sections. Each incremental save appends its own trailer, so the
/// delta proves how many revisions the save pipeline wrote.
fn startxref_count(bytes: &[u8]) -> usize {
    bytes
        .windows(b"startxref".len())
        .filter(|window| *window == b"startxref")
        .count()
}

fn page_font_names(parsed: &lopdf::Document, page_id: lopdf::ObjectId) -> Vec<String> {
    let (resources, _) = parsed
        .get_page_resources(page_id)
        .expect("page resources should resolve");
    let Some(resources) = resources else {
        return Vec::new();
    };
    let fonts = resources.get(b"Font").expect("font resource dictionary");
    let fonts = match fonts {
        lopdf::Object::Reference(id) => parsed.get_object(*id).expect("referenced font dictionary"),
        direct => direct,
    };
    let fonts = fonts.as_dict().expect("font dictionary");
    fonts
        .iter()
        .filter_map(|(_, value)| {
            let value = match value {
                lopdf::Object::Reference(id) => parsed.get_object(*id).ok()?,
                direct => direct,
            };
            let dict = value.as_dict().ok()?;
            match dict.get(b"BaseFont").ok()? {
                lopdf::Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn mixed_styles_survive_saving_and_reopening_with_one_lopdf_revision() {
    let pdf = synthetic::single_line_pdf();
    let source_bytes = std::fs::read(pdf.path_string()).expect("read the source PDF");
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");

    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        selected_style(),
    )
    .expect("style the selection");

    let bytes = save_document_bytes(&document_id).expect("serialize document");
    assert_eq!(
        std::fs::read(pdf.path_string()).expect("re-read source"),
        source_bytes,
        "saving must not modify the source file"
    );
    assert!(
        startxref_count(&bytes) <= startxref_count(&source_bytes) + 2,
        "saving appends at most PDFium's revision and one lopdf revision"
    );

    // The independent lopdf parse sees both fonts on the same page.
    let parsed = lopdf::Document::load_mem(&bytes).expect("lopdf parses the saved PDF");
    let page_id = *parsed.get_pages().get(&1).expect("page 1");
    let fonts = page_font_names(&parsed, page_id);
    assert!(
        fonts.iter().any(|name| name.contains("Helvetica")),
        "base style font is in the PDF: {fonts:?}"
    );
    assert!(
        fonts.iter().any(|name| name.contains("Times")),
        "run font is in the PDF: {fonts:?}"
    );

    // A fresh PDFium parse shows the same text, sizes and colours.
    let saved = synthetic::pdf_bytes_file("mixed-style", bytes);
    let reopened = open_synthetic(&saved);
    let text = extract_text(reopened.clone(), 0).expect("extract text");
    assert_eq!(
        text.text.split_whitespace().collect::<Vec<_>>().join(" "),
        "Original line. Selected line.",
        "the page text must not be lost or duplicated"
    );

    let opened_layer = extract_text_layer(document_id.clone(), 0).expect("original layer");
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let index = char_index_of(&reopened_layer, "Selected line.");
    assert_eq!(reopened_layer.text, opened_layer.text);

    for offset in 0..8 {
        assert!(
            (reopened_layer.chars[index + offset].font_size - 16.0).abs() < 0.01,
            "base characters keep 16pt"
        );
    }
    for offset in 9..13 {
        assert!(
            (reopened_layer.chars[index + offset].font_size - 24.0).abs() < 0.01,
            "run characters keep 24pt"
        );
    }

    let pixels = render_pixels(&reopened, 0, 2.0);
    let first_selected = &reopened_layer.chars[index + 9];
    let last_selected = &reopened_layer.chars[index + 12];
    let region = (
        first_selected.origin_x - 1.0,
        first_selected.origin_y - 24.0,
        last_selected.origin_x + 1.0,
        first_selected.origin_y + 8.0,
    );
    assert!(
        count_pixels(&pixels, 2.0, region, is_red) > 10,
        "the reopened run is still red with its underline"
    );

    // Clicking the run resolves the text object it was saved as.
    let base_target = text_edit_target(reopened.clone(), 0, index as u32).expect("base target");
    assert_eq!(base_target.font_family, "Helvetica");
    assert!((base_target.font_size - 16.0).abs() < 0.01);
    let run_target = text_edit_target(reopened.clone(), 0, (index + 9) as u32).expect("run target");
    assert_eq!(
        run_target.font_family, "Times-Bold",
        "the saved run keeps its real PostScript variant"
    );
    assert!((run_target.font_size - 24.0).abs() < 0.01);
    assert!(run_target.bold, "the saved run kept its bold variant");
}

#[test]
fn a_mixed_size_run_wraps_at_the_box_width_and_keeps_the_line_height_rule() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let text = "Mixed styles wrap across several lines.";
    let box_id = pdfx_lib::text_edit::create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 100.0,
            width: 120.0,
            height: 70.0,
            style: Some(base_style()),
        },
    )
    .expect("create Text Box")
    .target
    .box_id;
    apply_text_edit(document_id.clone(), 0, box_id, text.to_owned()).expect("type");
    let before = extract_text_layer(document_id.clone(), 0).expect("layer before");

    let start = text.find("across").expect("word") as u32;
    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView {
            start,
            end: start + 6,
        },
        TextBoxStyleView {
            font_family: "Times".to_owned(),
            font_size: 30.0,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            color: "#c0392b".to_owned(),
            alignment: TextAlignment::Left,
        },
    )
    .expect("style the wrapped word");
    assert_eq!(result.target.text, text);
    assert_eq!(result.target.runs.len(), 1);

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after");
    assert_eq!(
        layer.text.split_whitespace().collect::<Vec<_>>(),
        before.text.split_whitespace().collect::<Vec<_>>(),
        "wrapping must not lose or duplicate text"
    );

    // Wrapped lines add PDFium-generated break characters; drop them so the
    // box glyphs line up with the Text Box content.
    let visible: Vec<&pdfx_lib::commands::TextCharView> = layer
        .chars
        .iter()
        .filter(|character| !character.generated)
        .collect();
    let box_start = visible
        .iter()
        .position(|character| character.text == "M")
        .expect("box text start");
    let box_chars = &visible[box_start..];
    assert_eq!(
        box_chars
            .iter()
            .map(|character| character.text.as_str())
            .collect::<String>(),
        "Mixed styleswrap acrossseveral lines."
    );
    let word_start = box_chars
        .windows(6)
        .position(|window| {
            window
                .iter()
                .map(|character| character.text.as_str())
                .collect::<String>()
                == "across"
        })
        .expect("selected word in the layer");
    for character in &box_chars[word_start..word_start + 6] {
        assert!(
            (character.font_size - 30.0).abs() < 0.01,
            "selected word is 30pt: {character:?}"
        );
    }
    for (offset, character) in box_chars.iter().enumerate() {
        if !(word_start..word_start + 6).contains(&offset) {
            assert!(
                (character.font_size - 16.0).abs() < 0.01,
                "unselected text is 16pt: {character:?}"
            );
        }
        assert!(
            character.x + character.width <= 72.0 + 120.0 + 2.0,
            "every wrapped character stays inside the box width: {character:?}"
        );
    }

    // Group glyphs into visual lines and check the documented line rule:
    // line height 1.2 x the largest size on the line, baseline 0.8 x it below
    // the line top, lines stacking downward from the box origin.
    let mut lines: std::collections::BTreeMap<i32, (f32, f32)> = std::collections::BTreeMap::new();
    for character in box_chars {
        let key = (character.origin_y * 10.0).round() as i32;
        let entry = lines.entry(key).or_insert((f32::INFINITY, 0.0));
        entry.0 = entry.0.min(character.origin_y);
        entry.1 = entry.1.max(character.font_size);
    }
    assert!(lines.len() >= 3, "the text should wrap over several lines");
    let mut line_top = 100.0;
    for (_, (baseline, size)) in lines {
        let expected = line_top + size * 0.8;
        assert!(
            (baseline - expected).abs() < 0.6,
            "line baseline {baseline} should follow the 1.2/0.8 rule (expected {expected})"
        );
        line_top += size * 1.2;
    }

    let pixels = render_pixels(&document_id, 0, 2.0);
    let first = box_chars[word_start];
    let last = box_chars[word_start + 5];
    let region = (
        first.origin_x - 1.0,
        first.origin_y - 30.0,
        last.origin_x + last.width + 1.0,
        first.origin_y + 6.0,
    );
    assert!(
        count_pixels(&pixels, 2.0, region, is_red) > 10,
        "the larger run is painted in its own colour"
    );
}

fn char_index_of_last(layer: &TextLayerView, needle: &str) -> usize {
    let offset = layer.text.rfind(needle).expect("text in layer");
    layer.text[..offset].chars().count()
}

fn run_style(family: &str, size: f32, color: &str) -> TextBoxStyleView {
    TextBoxStyleView {
        font_family: family.to_owned(),
        font_size: size,
        bold: false,
        italic: false,
        underline: false,
        strikeout: false,
        color: color.to_owned(),
        alignment: TextAlignment::Left,
    }
}

#[test]
fn a_run_falls_back_to_the_bundled_font_with_the_same_notice_as_a_whole_box() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let text = "中文 Alpha 中文";
    let created = pdfx_lib::text_edit::create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 80.0,
            y: 160.0,
            width: 260.0,
            height: 40.0,
            style: Some(base_style()),
        },
    )
    .expect("create Text Box");
    let box_id = created.target.box_id;
    let typed = apply_text_edit(document_id.clone(), 0, box_id, text.to_owned()).expect("type");
    // Typing CJK text with Helvetica falls the whole box back to Noto, so the
    // base style is Noto before the selection styling starts.
    assert!(typed.font.expect("whole-box fallback").fallback);
    let before_layer = extract_text_layer(document_id.clone(), 0).expect("layer");

    // A Latin run in a base-14 font needs no fallback.
    let latin = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 3, end: 8 },
        run_style("Times", 20.0, "#1f6f3f"),
    )
    .expect("style the Latin word");
    assert!(latin.font.is_none(), "Times covers Latin text");
    assert_eq!(latin.target.runs.len(), 1);
    assert_eq!(latin.target.runs[0].font_family, "Times");

    // A CJK run asking for Times cannot be represented; its run resolves to
    // Noto and reports the same structured notice the whole-box path uses.
    let cjk = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 11 },
        run_style("Times", 22.0, "#c0392b"),
    )
    .expect("style the CJK word");
    let notice = cjk.font.clone().expect("run fallback notice");
    assert!(notice.fallback);
    assert_eq!(notice.original_family, "Times");
    assert_eq!(notice.family, "Noto Sans SC");
    assert!(
        notice.reason.as_deref().unwrap_or("").contains("Times"),
        "reason names the requested font: {notice:?}"
    );
    assert_eq!(cjk.target.font_family, "Noto Sans SC");
    assert!((cjk.target.font_size - 16.0).abs() < 0.01);
    assert_eq!(cjk.target.runs.len(), 2);
    let cjk_run = &cjk.target.runs[1];
    assert_eq!((cjk_run.start, cjk_run.end), (9, 11));
    assert_eq!(cjk_run.font_family, "Noto Sans SC");
    assert!((cjk_run.font_size - 22.0).abs() < 0.01);
    assert_eq!(cjk_run.color, "#c0392b");
    let latin_run = &cjk.target.runs[0];
    assert_eq!(latin_run.font_family, "Times");
    assert!((latin_run.font_size - 20.0).abs() < 0.01);

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after");
    assert_eq!(layer.text, before_layer.text, "fallback keeps the text");
    let last = char_index_of_last(&layer, "中文");
    for character in &layer.chars[last..last + 2] {
        assert!((character.font_size - 22.0).abs() < 0.01);
    }
    let pixels = render_pixels(&document_id, 0, 2.0);
    let first = &layer.chars[last];
    let second = &layer.chars[last + 1];
    let region = (
        first.origin_x - 1.0,
        first.origin_y - 24.0,
        second.origin_x + second.width + 1.0,
        first.origin_y + 6.0,
    );
    assert!(
        count_pixels(&pixels, 2.0, region, is_red) > 5,
        "the fallback run is painted, not missing glyphs"
    );

    // The whole-box style path reports the same fallback shape and clears the
    // runs: one look for the whole box means no per-range overrides remain.
    let whole = pdfx_lib::text_edit::apply_text_box_style(
        document_id.clone(),
        0,
        box_id,
        run_style("Times", 22.0, "#c0392b"),
    )
    .expect("style the whole box");
    let whole_notice = whole.font.expect("whole-box fallback notice");
    assert!(whole_notice.fallback);
    assert_eq!(whole_notice.original_family, "Times");
    assert_eq!(whole_notice.family, "Noto Sans SC");
    assert!(whole.target.runs.is_empty(), "whole-box style clears runs");
    assert_eq!(whole.target.font_family, "Noto Sans SC");
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

#[test]
fn a_legacy_text_object_can_style_a_selection_without_moving_the_rest() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let before = extract_text_layer(document_id.clone(), 0).expect("layer before");
    let before_pixels = render_pixels(&document_id, 0, 2.0);
    let index = char_index_of(&before, "Original line.");
    let target = text_edit_target(document_id.clone(), 0, (index + 9) as u32).expect("target");
    assert!((target.font_size - 18.0).abs() < 0.01);
    assert_eq!(target.font_family, "Helvetica");
    assert!(target.runs.is_empty(), "a legacy box has no runs yet");
    let box_id = target.box_id;

    let style = TextBoxStyleView {
        font_family: "Helvetica".to_owned(),
        font_size: 18.0,
        bold: false,
        italic: false,
        underline: true,
        strikeout: false,
        color: "#c0392b".to_owned(),
        alignment: TextAlignment::Left,
    };
    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        style,
    )
    .expect("style a legacy selection");
    assert_eq!(result.target.text, "Original line.");
    assert_eq!(result.target.runs.len(), 1);
    let run = &result.target.runs[0];
    assert_eq!((run.start, run.end), (9, 13));
    assert_eq!(run.font_family, "Helvetica");
    assert!((run.font_size - 18.0).abs() < 0.01);
    assert!(run.underline);
    assert_eq!(run.color, "#c0392b");
    assert_eq!(result.target.font_family, "Helvetica");
    assert!((result.target.font_size - 18.0).abs() < 0.01);
    assert_eq!(result.target.color, "#000000");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after");
    assert_eq!(layer.text, before.text, "legacy text is not duplicated");
    let index_after = char_index_of(&layer, "Original line.");
    for (offset, character) in layer.chars[index_after..index_after + 8].iter().enumerate() {
        let original = &before.chars[index + offset];
        assert!(
            (character.origin_x - original.origin_x).abs() < 0.01
                && (character.origin_y - original.origin_y).abs() < 0.01,
            "same-size selection styling must not move legacy text: {character:?}"
        );
    }

    let pixels = render_pixels(&document_id, 0, 2.0);
    let first = &layer.chars[index_after + 9];
    let last = &layer.chars[index_after + 12];
    let region = (
        first.origin_x - 1.0,
        first.origin_y - 20.0,
        last.origin_x + last.width + 1.0,
        first.origin_y + 8.0,
    );
    assert!(
        count_pixels(&before_pixels, 2.0, region, is_red) == 0,
        "the original text is not red"
    );
    assert!(
        count_pixels(&pixels, 2.0, region, is_red) > 10,
        "the legacy run becomes red with an underline"
    );

    // Undo restores the original Text Object, style and pixels exactly.
    undo_text_edit(document_id.clone()).expect("undo");
    let undone_pixels = render_pixels(&document_id, 0, 2.0);
    assert_eq!(undone_pixels.rgba, before_pixels.rgba);
    let after_undo = text_boxes(document_id.clone(), 0).expect("boxes after undo");
    let listed = after_undo
        .iter()
        .find(|box_view| box_view.box_id == box_id)
        .expect("box listed");
    assert!(listed.runs.is_empty());
    assert_eq!(listed.font_family, "Helvetica");
}

#[test]
fn legacy_embedded_fonts_refuse_selection_styling_without_mutating() {
    let pdf = synthetic::embedded_true_type_pdf();
    let document_id = open_synthetic(&pdf);
    let before = extract_text_layer(document_id.clone(), 0).expect("layer");
    let before_pixels = render_pixels(&document_id, 0, 2.0);
    let index = char_index_of(&before, "Embedded font sample.");
    let target = text_edit_target(document_id.clone(), 0, index as u32).expect("target");
    let box_id = target.box_id;

    let error = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 0, end: 8 },
        selected_style(),
    )
    .expect_err("an arbitrary embedded font has no built-in style");
    assert_eq!(error.code, CommandErrorCode::UnsupportedFont);
    assert!(
        error.message.contains("whole-box"),
        "the message should suggest styling the whole box first: {}",
        error.message
    );

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after refusal");
    assert_eq!(layer.text, before.text);
    let after_pixels = render_pixels(&document_id, 0, 2.0);
    assert_eq!(after_pixels.rgba, before_pixels.rgba);
    assert!(
        !pdfx_lib::text_edit::edit_history(document_id.clone())
            .expect("history")
            .can_undo,
        "a refused selection style creates no undo step"
    );
    let target = text_edit_target(document_id.clone(), 0, index as u32).expect("target again");
    assert!(target.runs.is_empty());
}

#[test]
fn invalid_ranges_and_styles_are_rejected_without_mutation() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");
    let before = extract_text_layer(document_id.clone(), 0).expect("layer");
    let before_pixels = render_pixels(&document_id, 0, 2.0);
    let before_history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");

    for range in [
        TextRangeView { start: 9, end: 9 },
        TextRangeView { start: 13, end: 9 },
        TextRangeView { start: 0, end: 999 },
    ] {
        let error =
            apply_text_box_selection_style(document_id.clone(), 0, box_id, range, selected_style())
                .expect_err("invalid selection");
        assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    }

    for style in [
        TextBoxStyleView {
            font_size: 500.0,
            ..base_style()
        },
        TextBoxStyleView {
            font_family: "Comic Sans".to_owned(),
            ..base_style()
        },
        TextBoxStyleView {
            font_family: "Noto Sans SC".to_owned(),
            italic: true,
            ..base_style()
        },
    ] {
        let error = apply_text_box_selection_style(
            document_id.clone(),
            0,
            box_id,
            TextRangeView { start: 0, end: 2 },
            style,
        )
        .expect_err("invalid style");
        assert!(
            matches!(
                error.code,
                CommandErrorCode::InvalidArgument | CommandErrorCode::UnsupportedFont
            ),
            "unexpected error code: {error:?}"
        );
    }

    let error = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id + 1000,
        TextRangeView { start: 0, end: 2 },
        selected_style(),
    )
    .expect_err("unknown box");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let after = extract_text_layer(document_id.clone(), 0).expect("layer after");
    assert_eq!(after.text, before.text);
    let after_pixels = render_pixels(&document_id, 0, 2.0);
    assert_eq!(after_pixels.rgba, before_pixels.rgba);
    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history after");
    assert_eq!(history.revision, before_history.revision);
    assert!(box_runs(&document_id, box_id).is_empty());
}

#[test]
fn permission_restricted_documents_reject_selection_styling() {
    let document_id = open_fixture_id("encrypted-owner-restrictions");
    let error = apply_text_box_selection_style(
        document_id.clone(),
        0,
        1,
        TextRangeView { start: 0, end: 1 },
        selected_style(),
    )
    .expect_err("read-only selection style");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert!(!history.dirty);
}

#[test]
fn retyping_the_same_text_keeps_runs_while_a_replacement_resets_them() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "First\nSecond");
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 6, end: 12 },
        selected_style(),
    )
    .expect("style the second line");
    assert_eq!(box_runs(&document_id, box_id).len(), 1);

    // The textarea sends CRLF; normalization happens before the unchanged
    // comparison, so the runs are remapped, not dropped.
    let same = apply_text_edit(document_id.clone(), 0, box_id, "First\r\nSecond".to_owned())
        .expect("retype the same text");
    assert_eq!(same.text, "First\nSecond");
    let runs = box_runs(&document_id, box_id);
    assert_eq!(runs.len(), 1, "unchanged text keeps its runs: {runs:?}");
    assert_eq!((runs[0].start, runs[0].end), (6, 12));
    assert_eq!(runs[0].font_family, "Times");
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let index = char_index_of(&layer, "Second");
    for character in &layer.chars[index..index + 6] {
        assert!(
            (character.font_size - 24.0).abs() < 0.01,
            "the remapped run is still styled: {character:?}"
        );
    }

    // Replacing the content invalidates character offsets, so the document is
    // reset to one base style instead of guessing where runs moved.
    let replaced = apply_text_edit(document_id.clone(), 0, box_id, "First\nThird".to_owned())
        .expect("replace the text");
    assert_eq!(replaced.text, "First\nThird");
    assert!(
        box_runs(&document_id, box_id).is_empty(),
        "changed text resets runs"
    );
    let listed = text_boxes(document_id.clone(), 0)
        .expect("boxes")
        .into_iter()
        .find(|box_view| box_view.box_id == box_id)
        .expect("box listed");
    assert_eq!(listed.font_family, "Helvetica");
    assert!((listed.font_size - 16.0).abs() < 0.01);
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after replacement");
    for character in &layer.chars[char_index_of(&layer, "Third")..][..5] {
        assert!((character.font_size - 16.0).abs() < 0.01);
    }
}

#[test]
fn transforming_a_styled_box_keeps_its_runs_and_undo_restores_them() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        selected_style(),
    )
    .expect("style the selection");

    let moved = pdfx_lib::text_edit::transform_text_box(
        document_id.clone(),
        0,
        box_id,
        pdfx_lib::text_edit::TextBoxTransformRequest {
            x: 120.0,
            y: 220.0,
            width: 200.0,
            height: 60.0,
            auto_height: true,
            rotation: 0.0,
        },
    )
    .expect("move and resize the box");
    assert_eq!(moved.target.text, "Selected line.");
    assert_eq!(moved.target.runs.len(), 1, "a transform keeps every run");
    let run = &moved.target.runs[0];
    assert_eq!((run.start, run.end), (9, 13));
    assert_eq!(run.font_family, "Times");
    assert!((run.font_size - 24.0).abs() < 0.01);

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after transform");
    let index = char_index_of(&layer, "Selected line.");
    assert!(
        (layer.chars[index].x - 120.0).abs() < 1.0,
        "the text moved with its box: {:?}",
        layer.chars[index]
    );
    for character in &layer.chars[index..index + 8] {
        assert!((character.font_size - 16.0).abs() < 0.01);
    }
    for character in &layer.chars[index + 9..index + 13] {
        assert!((character.font_size - 24.0).abs() < 0.01);
    }

    // One undo returns the frame and the styled ranges together.
    undo_text_edit(document_id.clone()).expect("undo the transform");
    let restored = text_boxes(document_id.clone(), 0)
        .expect("boxes")
        .into_iter()
        .find(|box_view| box_view.box_id == box_id)
        .expect("box listed");
    assert_eq!(restored.x, 72.0);
    assert_eq!(restored.y, 100.0);
    assert_eq!((restored.width, restored.height), (320.0, 36.0));
    assert_eq!(restored.runs.len(), 1);
    assert_eq!((restored.runs[0].start, restored.runs[0].end), (9, 13));
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer after undo");
    let index = char_index_of(&layer, "Selected line.");
    assert!((layer.chars[index].x - 72.0).abs() < 1.0);
}

#[test]
fn overlapping_selections_split_straddling_runs_and_merge_equal_neighbours() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "abcdefghij");
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 2, end: 8 },
        selected_style(),
    )
    .expect("style the middle");
    assert_eq!(
        box_runs(&document_id, box_id)
            .iter()
            .map(|run| (run.start, run.end))
            .collect::<Vec<_>>(),
        vec![(2, 8)]
    );

    // A nested selection splits the enclosing run on both sides.
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 4, end: 6 },
        base_style(),
    )
    .expect("reset the nested range");
    let runs = box_runs(&document_id, box_id);
    assert_eq!(
        runs.iter()
            .map(|run| (run.start, run.end, run.font_family.clone(), run.font_size))
            .collect::<Vec<_>>(),
        vec![
            (2, 4, "Times".to_owned(), 24.0),
            (4, 6, "Helvetica".to_owned(), 16.0),
            (6, 8, "Times".to_owned(), 24.0),
        ]
    );
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer split");
    let index = char_index_of(&layer, "abcdefghij");
    for offset in 2..4 {
        assert!((layer.chars[index + offset].font_size - 24.0).abs() < 0.01);
    }
    for offset in 4..6 {
        assert!((layer.chars[index + offset].font_size - 16.0).abs() < 0.01);
    }

    // Restyling the nested range with the surrounding style merges the three
    // pieces back into one run.
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 4, end: 6 },
        selected_style(),
    )
    .expect("restyle the nested range");
    let runs = box_runs(&document_id, box_id);
    assert_eq!(runs.len(), 1, "equal neighbours merge: {runs:?}");
    assert_eq!((runs[0].start, runs[0].end), (2, 8));
    let layer = extract_text_layer(document_id.clone(), 0).expect("layer merged");
    let index = char_index_of(&layer, "abcdefghij");
    for offset in 2..8 {
        assert!((layer.chars[index + offset].font_size - 24.0).abs() < 0.01);
    }
    for offset in (0..2).chain(8..10) {
        assert!((layer.chars[index + offset].font_size - 16.0).abs() < 0.01);
    }
}

/// The Noto subsets in a saved page's font resources, named by the BaseFont
/// PDFium wrote for each embedded subset.
fn noto_base_fonts(bytes: &[u8]) -> Vec<String> {
    let parsed = lopdf::Document::load_mem(bytes).expect("lopdf parses the saved PDF");
    let page_id = *parsed.get_pages().get(&1).expect("page 1");
    page_font_names(&parsed, page_id)
        .into_iter()
        .filter(|name| name.contains("NotoSansSC"))
        .collect()
}

/// PDFium's content generator deduplicates fonts by BaseFont name, so two
/// subsets on one page need distinct PostScript names or the later one's
/// characters are decoded through the first one's CID map.
fn assert_distinct_noto_subsets(bytes: &[u8], expected: usize) -> Vec<String> {
    let names = noto_base_fonts(bytes);
    let distinct: std::collections::BTreeSet<&String> = names.iter().collect();
    assert!(
        distinct.len() >= expected,
        "every Noto subset needs its own BaseFont: {names:?}"
    );
    for name in &distinct {
        let suffix = name
            .strip_prefix("NotoSansSC-Thin-")
            .or_else(|| name.strip_prefix("NotoSansSC-Bold-"))
            .unwrap_or_else(|| panic!("subset keeps the Noto PostScript stem: {name:?}"));
        assert_eq!(suffix.len(), 16, "subset tag is 16 hex digits: {name:?}");
        assert!(
            suffix
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
            "subset tag is hexadecimal: {name:?}"
        );
    }
    names
}

/// A Text Box whose base text falls back to Noto because Helvetica cannot
/// render the CJK (or mark-only) characters.
fn fallback_text_box(document_id: &str, x: f32, y: f32, text: &str) -> u64 {
    let box_id = pdfx_lib::text_edit::create_text_box(
        document_id.to_owned(),
        0,
        CreateTextBoxRequest {
            x,
            y,
            width: 220.0,
            height: 36.0,
            style: Some(base_style()),
        },
    )
    .expect("create Text Box")
    .target
    .box_id;
    apply_text_edit(document_id.to_owned(), 0, box_id, text.to_owned()).expect("type into box");
    box_id
}

#[test]
fn two_noto_subsets_in_one_box_keep_distinct_pdf_fonts() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = fallback_text_box(&document_id, 72.0, 100.0, "中文");

    // The second character at a larger size is rendered from a different
    // subset than the first; before the font names were made unique PDFium
    // collapsed both subsets into one resource and extraction returned the
    // first character twice.
    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 1, end: 2 },
        run_style("Noto Sans SC", 20.0, "#c0392b"),
    )
    .expect("style the second character");
    assert_eq!(result.target.text, "中文");
    assert_eq!(result.target.runs.len(), 1);
    assert_eq!(result.target.runs[0].font_family, "Noto Sans SC");

    let live_layer = extract_text_layer(document_id.clone(), 0).expect("live layer");
    let live_start = char_index_of(&live_layer, "中文");
    assert_eq!(live_layer.chars[live_start].text, "中");
    assert_eq!(live_layer.chars[live_start + 1].text, "文");
    assert!((live_layer.chars[live_start].font_size - 16.0).abs() < 0.01);
    assert!((live_layer.chars[live_start + 1].font_size - 20.0).abs() < 0.01);

    let bytes = save_document_bytes(&document_id).expect("serialize document");
    assert_distinct_noto_subsets(&bytes, 2);

    let saved = synthetic::pdf_bytes_file("mixed-noto-subsets", bytes);
    let reopened = open_synthetic(&saved);
    let plain = extract_text(reopened.clone(), 0).expect("plain text");
    assert_eq!(
        plain.text.split_whitespace().collect::<Vec<_>>().join(" "),
        "Original line. 中文",
        "both CJK glyphs survive the save/reopen round trip"
    );
    let layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let start = char_index_of(&layer, "中文");
    assert_eq!(layer.chars[start].text, "中");
    assert_eq!(layer.chars[start + 1].text, "文");
    assert!((layer.chars[start].font_size - 16.0).abs() < 0.01);
    assert!((layer.chars[start + 1].font_size - 20.0).abs() < 0.01);

    // The suffixed PostScript name still resolves to the bundled family, so
    // the style panel keeps offering "Noto Sans SC" after reopening.
    let target = text_edit_target(reopened.clone(), 0, (start + 1) as u32).expect("run target");
    assert_eq!(target.font_family, "Noto Sans SC");
    assert!((target.font_size - 20.0).abs() < 0.01);
}

#[test]
fn a_combining_mark_only_run_keeps_its_own_subset_and_reopens_intact() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let text = "e\u{0301}x";
    let box_id = fallback_text_box(&document_id, 72.0, 100.0, text);

    // Times cannot cover U+0301, so styling only the mark falls back to Noto;
    // the mark-run subset contains one character while the base subset holds
    // "ex", which is exactly the collision PDFium used to collapse.
    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 1, end: 2 },
        run_style("Times", 20.0, "#c0392b"),
    )
    .expect("style the combining mark");
    assert!(result.font.expect("mark fallback notice").fallback);
    assert_eq!(result.target.runs.len(), 1);
    assert_eq!(result.target.runs[0].font_family, "Noto Sans SC");
    assert!((result.target.runs[0].font_size - 20.0).abs() < 0.01);

    let live_layer = extract_text_layer(document_id.clone(), 0).expect("live layer");
    assert!(
        live_layer.text.ends_with(text),
        "live extraction keeps the combining mark: {:?}",
        live_layer.text
    );
    let live_start = char_index_of_last(&live_layer, text);
    assert_eq!(live_layer.chars[live_start].text, "e");
    assert_eq!(live_layer.chars[live_start + 1].text, "\u{0301}");
    assert_eq!(live_layer.chars[live_start + 2].text, "x");
    assert!((live_layer.chars[live_start + 1].font_size - 20.0).abs() < 0.01);

    let bytes = save_document_bytes(&document_id).expect("serialize document");
    assert_distinct_noto_subsets(&bytes, 2);

    let saved = synthetic::pdf_bytes_file("mark-only-noto-subsets", bytes);
    let reopened = open_synthetic(&saved);
    let plain = extract_text(reopened.clone(), 0).expect("plain text");
    assert!(
        plain.text.ends_with(text),
        "reopened extraction keeps the combining mark: {:?}",
        plain.text
    );
    let layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let start = char_index_of_last(&layer, text);
    assert_eq!(layer.chars[start].text, "e");
    assert_eq!(layer.chars[start + 1].text, "\u{0301}");
    assert_eq!(layer.chars[start + 2].text, "x");
    assert!((layer.chars[start + 1].font_size - 20.0).abs() < 0.01);

    let target = text_edit_target(reopened, 0, (start + 1) as u32).expect("mark target");
    assert_eq!(target.font_family, "Noto Sans SC");
    assert!((target.font_size - 20.0).abs() < 0.01);
}

#[test]
fn two_noto_boxes_on_one_page_do_not_share_a_subset_font() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    // Distinct positions: PDFium drops exactly-overlapping text objects from
    // extraction, and the point of this test is the font resources instead.
    fallback_text_box(&document_id, 72.0, 100.0, "中文");
    fallback_text_box(&document_id, 72.0, 200.0, "文字");

    let bytes = save_document_bytes(&document_id).expect("serialize document");
    assert_distinct_noto_subsets(&bytes, 2);

    let saved = synthetic::pdf_bytes_file("two-noto-boxes", bytes);
    let reopened = open_synthetic(&saved);
    let plain = extract_text(reopened, 0).expect("plain text");
    assert!(
        plain.text.contains("中文"),
        "first box survives: {:?}",
        plain.text
    );
    assert!(
        plain.text.contains("文字"),
        "second box survives instead of decoding through the first subset: {:?}",
        plain.text
    );
}

#[test]
fn mixed_styles_keep_the_box_alignment_on_the_line() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let mut base = base_style();
    base.alignment = TextAlignment::Center;
    let box_id = pdfx_lib::text_edit::create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 100.0,
            width: 200.0,
            height: 36.0,
            style: Some(base),
        },
    )
    .expect("create centred Text Box")
    .target
    .box_id;
    apply_text_edit(document_id.clone(), 0, box_id, "ab".to_owned()).expect("type");
    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 1, end: 2 },
        run_style("Times", 24.0, "#c0392b"),
    )
    .expect("style the second character");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let index = char_index_of(&layer, "ab");
    let first = &layer.chars[index];
    let second = &layer.chars[index + 1];
    assert!((first.font_size - 16.0).abs() < 0.01);
    assert!((second.font_size - 24.0).abs() < 0.01);
    // The mixed-size line is centred as a whole: its advance span is measured
    // from the first origin to the last origin plus advance.
    let left = first.origin_x;
    let right = second.origin_x + second.width;
    let center = (left + right) / 2.0;
    assert!(
        (center - (72.0 + 200.0 / 2.0)).abs() < 0.5,
        "mixed-style line centre {center} should match the box centre: {first:?} {second:?}"
    );
}

#[test]
fn a_styled_run_starts_after_the_trailing_space_of_the_previous_run() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");

    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        run_style("Times", 24.0, "#c0392b"),
    )
    .expect("style the second word");

    let layer = extract_text_layer(document_id.clone(), 0).expect("layer");
    let index = char_index_of(&layer, "Selected line.");
    let space = &layer.chars[index + 8];
    let run = &layer.chars[index + 9];
    assert_eq!(space.text, " ");
    assert!((run.font_size - 24.0).abs() < 0.01);

    // The styled run is placed at the space's advance, not at the ink edge of
    // the preceding word. Missing the space's advance makes the space have no
    // visual effect, so an external extractor such as pdftotext returns
    // "Selectedline." even though PDFium still reports the space.
    let space_end = space.origin_x + space.width;
    assert!(
        (run.origin_x - space_end).abs() < 0.5,
        "styled run origin {} should follow the space end {space_end}: {space:?} {run:?}",
        run.origin_x
    );
}

#[test]
fn selecting_the_whole_box_and_emptying_it_resets_runs_without_dangling_ranges() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let text = "Whole box";
    let box_id = styled_text_box(&document_id, text);

    // Selecting every character goes through the same range path as any other
    // selection; the untouched base text becomes empty, which is the edge the
    // renderer fills with a space-only subset.
    let result = apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView {
            start: 0,
            end: text.chars().count() as u32,
        },
        run_style("Times", 22.0, "#c0392b"),
    )
    .expect("style all characters");
    assert_eq!(result.target.text, text);
    assert_eq!(result.target.runs.len(), 1);
    assert_eq!(
        (result.target.runs[0].start, result.target.runs[0].end),
        (0, text.chars().count() as u32)
    );

    let layer = extract_text_layer(document_id.clone(), 0).expect("live layer");
    let index = char_index_of(&layer, text);
    for character in &layer.chars[index..index + text.chars().count()] {
        assert!(
            (character.font_size - 22.0).abs() < 0.01,
            "every selected character is styled: {character:?}"
        );
    }
    let bytes = save_document_bytes(&document_id).expect("serialize document");
    let saved = synthetic::pdf_bytes_file("all-text-run", bytes);
    let reopened = open_synthetic(&saved);
    let plain = extract_text(reopened.clone(), 0).expect("plain text");
    assert!(
        plain.text.ends_with(text),
        "an all-text run saves and reopens once: {:?}",
        plain.text
    );

    // Emptying the box replaces the text, so the run offsets no longer exist
    // and must be dropped rather than left dangling.
    apply_text_edit(document_id.clone(), 0, box_id, String::new()).expect("empty the box");
    assert!(
        box_runs(&document_id, box_id).is_empty(),
        "empty text has no runs"
    );
    let listed = text_boxes(document_id.clone(), 0)
        .expect("boxes")
        .into_iter()
        .find(|box_view| box_view.box_id == box_id)
        .expect("box listed");
    assert!(listed.text.is_empty());
    // The empty box stays renderable and editable after the runs were reset.
    let empty_bytes = save_document_bytes(&document_id).expect("serialize empty box");
    let empty_saved = synthetic::pdf_bytes_file("emptied-run-box", empty_bytes);
    let empty_reopened = open_synthetic(&empty_saved);
    assert!(
        extract_text(empty_reopened, 0)
            .expect("plain text")
            .text
            .ends_with("Original line."),
        "the empty box adds no text"
    );
}

#[test]
fn undo_and_redo_styled_ranges_reach_past_a_saved_revision() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let box_id = styled_text_box(&document_id, "Selected line.");
    let before = render_pixels(&document_id, 0, 2.0);
    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert!(history.dirty);

    apply_text_box_selection_style(
        document_id.clone(),
        0,
        box_id,
        TextRangeView { start: 9, end: 13 },
        selected_style(),
    )
    .expect("style the selection");
    let styled = render_pixels(&document_id, 0, 2.0);

    // Saving serializes the live document and records the clean baseline; the
    // undo stack keeps the styled ranges on both sides of that point.
    save_document_bytes(&document_id).expect("serialize the document");
    let saved = mark_document_saved(document_id.clone()).expect("mark saved");
    assert!(!saved.dirty, "the saved revision is clean");
    assert!(saved.can_undo, "saving must not clear the undo stack");
    assert_eq!(saved.revision, saved.saved_revision);

    let undone = undo_text_edit(document_id.clone()).expect("undo past the save");
    assert!(undone.history.dirty, "undo past the save is unsaved work");
    assert!(undone.history.can_redo);
    assert!(box_runs(&document_id, box_id).is_empty());
    assert_eq!(
        render_pixels(&document_id, 0, 2.0).rgba,
        before.rgba,
        "undo past the save restores the exact unstyled pixels"
    );

    redo_text_edit(document_id.clone()).expect("redo past the save");
    assert_eq!(box_runs(&document_id, box_id).len(), 1);
    assert_eq!(
        render_pixels(&document_id, 0, 2.0).rgba,
        styled.rgba,
        "redo past the save restores the exact styled pixels"
    );
}
