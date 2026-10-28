//! Command-boundary tests for CJK text, font fallback and font subsetting.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is reopened through the command boundary with a fresh
//! PDFium parse, so searchable text and the embedded font are checked where a
//! PDF viewer would read them.

mod support;

use support::synthetic;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use lopdf::{Dictionary, Document, Object};

use pdfx_lib::commands::{
    close_document, extract_text, extract_text_layer, open_document, render_page,
    save_document_bytes, TextLayerView,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveKind, SaveProgress};
use pdfx_lib::text_edit::{
    apply_text_edit, create_text_box, text_box_fonts, text_boxes, text_edit_target,
    CreateTextBoxRequest, TextAlignment, TextBoxStyleView,
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

fn bundled_style(size: f32) -> TextBoxStyleView {
    TextBoxStyleView {
        font_family: "Noto Sans SC".to_owned(),
        font_size: size,
        bold: false,
        italic: false,
        underline: false,
        color: "#000000".to_owned(),
        alignment: TextAlignment::Left,
    }
}

struct DecodedPng {
    width: u32,
    rgba: Vec<u8>,
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    use base64::Engine as _;

    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
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

/// Non-white pixels inside a page-point region, and their bounding box.
fn ink_in_region(
    pixels: &DecodedPng,
    scale: f32,
    region: (f32, f32, f32, f32),
) -> (usize, (f32, f32, f32, f32)) {
    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut count = 0;
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        if pixel[0] >= 245 && pixel[1] >= 245 && pixel[2] >= 245 {
            continue;
        }
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        if x < region.0 || x > region.2 || y < region.1 || y > region.3 {
            continue;
        }
        count += 1;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    (count, (left, top, right, bottom))
}

fn resolved<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    match object {
        Object::Reference(id) => doc.get_object(*id).expect("referenced object"),
        other => other,
    }
}

fn entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get(key).ok().map(|object| resolved(doc, object))
}

fn dict_entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    entry(doc, dict, key).and_then(|object| object.as_dict().ok())
}

fn number(object: &Object) -> f32 {
    match object {
        Object::Integer(value) => *value as f32,
        Object::Real(value) => *value,
        other => panic!("expected a number, got {other:?}"),
    }
}

/// Widths from a `/W` array, either `[cid [w...] ...]` or `[first last w ...]`.
fn parse_widths(doc: &Document, array: &[Object]) -> BTreeMap<u16, f32> {
    let mut widths = BTreeMap::new();
    let mut index = 0;
    while index + 1 < array.len() {
        let first = number(resolved(doc, &array[index])) as u16;
        match resolved(doc, &array[index + 1]) {
            Object::Array(values) => {
                for (offset, value) in values.iter().enumerate() {
                    widths.insert(first + offset as u16, number(resolved(doc, value)));
                }
                index += 2;
            }
            _ => {
                let last = number(resolved(doc, &array[index + 1])) as u16;
                let width = number(resolved(doc, &array[index + 2]));
                for cid in first..=last {
                    widths.insert(cid, width);
                }
                index += 3;
            }
        }
    }
    widths
}

/// CID-to-Unicode pairs from the `beginbfchar` sections of a ToUnicode CMap.
fn parse_bfchars(cmap: &str) -> BTreeMap<u16, String> {
    let mut pairs = BTreeMap::new();
    let mut in_bfchar = false;
    for line in cmap.lines() {
        let line = line.trim();
        if line.ends_with("beginbfchar") {
            in_bfchar = true;
            continue;
        }
        if line == "endbfchar" {
            in_bfchar = false;
            continue;
        }
        if !in_bfchar || !line.starts_with('<') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let cid = parts.next().expect("CID").trim_matches(['<', '>']);
        let unicode = parts.next().expect("Unicode").trim_matches(['<', '>']);
        assert_eq!(cid.len(), 4, "CID must be 16-bit: {line}");
        assert_eq!(unicode.len() % 4, 0, "UTF-16BE must be whole units: {line}");
        let cid = u16::from_str_radix(cid, 16).expect("hex CID");
        let units: Vec<u16> = unicode
            .as_bytes()
            .chunks(4)
            .map(|chunk| {
                u16::from_str_radix(std::str::from_utf8(chunk).expect("ASCII"), 16)
                    .expect("hex UTF-16 unit")
            })
            .collect();
        pairs.insert(cid, String::from_utf16(&units).expect("valid UTF-16"));
    }
    pairs
}

struct CidSubsetFacts {
    encoding: String,
    widths: BTreeMap<u16, f32>,
    cid_to_gid: Vec<u16>,
    to_unicode: String,
    font_file: Vec<u8>,
}

/// Reads the Type0/CIDFontType2 font PDFium wrote into the document.
fn read_cid_subset(doc: &Document) -> CidSubsetFacts {
    for object in doc.objects.values() {
        let Object::Dictionary(type0) = object else {
            continue;
        };
        if entry(doc, type0, b"Subtype").and_then(|object| object.as_name().ok())
            != Some(b"Type0".as_slice())
        {
            continue;
        }
        let encoding = entry(doc, type0, b"Encoding")
            .and_then(|object| object.as_name().ok())
            .expect("Type0 /Encoding")
            .to_vec();
        let descendants = entry(doc, type0, b"DescendantFonts")
            .and_then(|object| object.as_array().ok())
            .expect("DescendantFonts array");
        let cid_font = resolved(doc, &descendants[0])
            .as_dict()
            .expect("descendant font dictionary");
        assert_eq!(
            entry(doc, cid_font, b"Subtype").and_then(|object| object.as_name().ok()),
            Some(b"CIDFontType2".as_slice()),
            "CJK text must embed a CIDFontType2 font"
        );

        let widths = entry(doc, cid_font, b"W")
            .and_then(|object| object.as_array().ok())
            .map(|array| parse_widths(doc, array))
            .expect("CIDFontType2 /W array");
        let cid_map = entry(doc, cid_font, b"CIDToGIDMap")
            .and_then(|object| object.as_stream().ok())
            .expect("CIDToGIDMap must be a stream, not /Identity");
        let cid_to_gid: Vec<u16> = cid_map
            .content
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        let to_unicode = entry(doc, type0, b"ToUnicode")
            .and_then(|object| object.as_stream().ok())
            .map(|stream| String::from_utf8_lossy(&stream.content).into_owned())
            .expect("Type0 /ToUnicode stream");
        let descriptor =
            dict_entry(doc, cid_font, b"FontDescriptor").expect("CIDFontType2 /FontDescriptor");
        let font_file = entry(doc, descriptor, b"FontFile2")
            .and_then(|object| object.as_stream().ok())
            .expect("FontDescriptor /FontFile2")
            .content
            .clone();
        return CidSubsetFacts {
            encoding: String::from_utf8_lossy(&encoding).into_owned(),
            widths,
            cid_to_gid,
            to_unicode,
            font_file,
        };
    }
    panic!("the saved document has no Type0 font");
}

fn save_to(document_id: &str, path: Option<&Path>) -> pdfx_lib::save::SaveResultView {
    let mut progress: Vec<SaveProgress> = Vec::new();
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: path.map(|path| path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            password: None,
        },
        &mut |update| progress.push(update),
    )
    .expect("save should succeed")
}

fn font_program_count(bytes: &[u8]) -> usize {
    let document = Document::load_mem(bytes).expect("count font programs");
    document
        .objects
        .values()
        .filter(|object| matches!(object, Object::Dictionary(dict) if dict.has(b"FontFile2")))
        .count()
}

#[test]
fn repeated_saves_do_not_embed_duplicate_subsets() {
    let dir = support::temp::TempDir::new("cjk-repeat-save");
    let output = dir.path().join("output.pdf");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
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
        "中文 PDF 混合".to_owned(),
    )
    .expect("edit with Chinese");

    let first = save_to(&document_id, Some(&output));
    assert_eq!(first.kind, SaveKind::SaveAs);
    let first_bytes = fs::read(&output).expect("read first save");
    assert_eq!(font_program_count(&first_bytes), 1);

    let second = save_to(&document_id, None);
    assert_eq!(second.kind, SaveKind::SaveChanges);
    let second_bytes = fs::read(&output).expect("read second save");
    assert_eq!(
        font_program_count(&second_bytes),
        1,
        "a repeated save must not embed a second font program"
    );
    assert!(
        second_bytes.len() <= first_bytes.len() + 1024,
        "saving without edits must not grow the document: {} -> {} bytes",
        first_bytes.len(),
        second_bytes.len()
    );

    // A second Text Box with the same characters and weight reuses the cached
    // subset token instead of loading another copy of the font.
    let created = create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 500.0,
            width: 320.0,
            height: 60.0,
            style: Some(bundled_style(18.0)),
        },
    )
    .expect("create a second bundle-font box");
    apply_text_edit(
        document_id.clone(),
        0,
        created.target.box_id,
        "中文 PDF 混合".to_owned(),
    )
    .expect("type the same characters again");

    let third = save_to(&document_id, None);
    assert_eq!(third.kind, SaveKind::SaveChanges);
    let third_bytes = fs::read(&output).expect("read third save");
    assert_eq!(
        font_program_count(&third_bytes),
        1,
        "the same subset must not be embedded twice"
    );
    assert!(
        third.bytes_written < first.bytes_written + 50_000,
        "the second Text Box must add content only, not a font: {} -> {} bytes",
        first.bytes_written,
        third.bytes_written
    );
    assert_eq!(font_program_count(&fs::read(&output).expect("reread")), 1);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen saved output")
        .document_id;
    let reopened_text = extract_text(reopened.clone(), 0).expect("reopened text");
    let compact: String = reopened_text
        .text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(
        compact.matches("中文PDF混合").count(),
        2,
        "both Text Boxes must remain searchable: {:?}",
        reopened_text.text
    );
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn the_saved_subset_has_correct_cids_widths_and_unicode() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");
    let text = "中文 PDF 混合";
    apply_text_edit(document_id.clone(), 0, target.box_id, text.to_owned())
        .expect("edit with Chinese");
    let bytes = save_document_bytes(&document_id).expect("save");
    assert!(
        bytes.len() < 300_000,
        "a few glyphs must not embed the whole bundled font: {} bytes",
        bytes.len()
    );

    let mut doc = Document::load_mem(&bytes).expect("lopdf opens the saved document");
    doc.decompress();
    let facts = read_cid_subset(&doc);

    assert_eq!(facts.encoding, "Identity-H");
    let face = ttf_parser::Face::parse(&facts.font_file, 0).expect("subset font parses");
    assert!(
        facts.font_file.len() < 100_000,
        "only the used glyphs may be embedded, got {} bytes",
        facts.font_file.len()
    );
    assert!(
        face.number_of_glyphs() <= 16,
        "the subset must contain only the used glyphs, got {}",
        face.number_of_glyphs()
    );

    let unicode = parse_bfchars(&facts.to_unicode);
    let expected: BTreeSet<char> = text.chars().collect();
    let mapped: BTreeSet<char> = unicode
        .values()
        .map(|value| value.chars().next().expect("one character"))
        .collect();
    assert_eq!(
        mapped, expected,
        "ToUnicode must map every used character: {unicode:?}"
    );

    // Every mapped CID points at a real glyph in the embedded program, and the
    // mapping is not the identity: CIDs are dense first-appearance indices
    // while glyph IDs keep the source font's order.
    let mut non_identity = false;
    for (cid, value) in &unicode {
        let character = value.chars().next().expect("one character");
        let gid = facts.cid_to_gid[*cid as usize];
        assert_ne!(gid, 0, "CID {cid} ({character:?}) maps to .notdef");
        assert!(
            gid < face.number_of_glyphs(),
            "CID {cid} maps to glyph {gid}, outside the subset"
        );
        assert!(
            face.glyph_index(character).is_some(),
            "the injected cmap must cover {character:?}"
        );
        if gid != *cid {
            non_identity = true;
        }
    }
    assert!(
        non_identity,
        "CIDToGIDMap must record the old-to-new glyph remapping, not /Identity"
    );

    // /W carries the subset's own advances. Noto Sans SC is a 1000-unit font,
    // and full-width CJK ideographs advance the full em.
    assert!(!facts.widths.is_empty(), "the CID font needs /W widths");
    for cid in unicode.keys() {
        let width = facts.widths.get(cid).expect("a width for every CID");
        assert!(
            (100.0..=1500.0).contains(width),
            "CID {cid} has implausible width {width}"
        );
    }
    let width_of = |character: char| {
        let cid = unicode
            .iter()
            .find(|(_, value)| value.chars().eq(std::iter::once(character)))
            .map(|(cid, _)| *cid)
            .expect("character in ToUnicode");
        *facts.widths.get(&cid).expect("width")
    };
    assert!(
        width_of('中') >= 900.0,
        "a full-width ideograph must advance about one em, got {}",
        width_of('中')
    );
    assert!(
        width_of('P') < width_of('中'),
        "Latin glyphs must keep their narrower advances"
    );

    close_document(document_id).expect("close");
}

#[test]
fn a_non_embedded_original_font_falls_back_and_reports_the_change() {
    let pdf = synthetic::cjk_mixed_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(document_id.clone(), 0, char_index_of(&layer, "中文") as u32)
        .expect("target");
    assert_eq!(target.font_family, "SimSun");

    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "中文排版测试 PDF".to_owned(),
    )
    .expect("edit a non-embedded CJK font");
    let font = result.font.expect("the fallback must be reported");
    assert!(font.fallback);
    assert_eq!(font.original_family, "SimSun");
    assert_eq!(font.family, "Noto Sans SC");
    let reason = font.reason.expect("the fallback needs a reason");
    assert!(
        reason.contains("SimSun") && reason.contains("not embedded"),
        "the reason should name the font and why it was replaced: {reason}"
    );

    let edited = extract_text_layer(document_id.clone(), 0).expect("edited layer");
    let compact: String = edited.text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("中文排版测试PDF"),
        "the replacement must be searchable: {:?}",
        edited.text
    );

    // The object now carries the bundled family, so another edit keeps using
    // it rather than falling back again.
    let second = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&edited, "中文") as u32,
    )
    .expect("target after fallback");
    assert_eq!(second.font_family, "Noto Sans SC");
    let second_result = apply_text_edit(
        document_id.clone(),
        0,
        second.box_id,
        "中文排版测试 PDF 二次".to_owned(),
    )
    .expect("edit the bundled fallback again");
    assert!(
        second_result.font.is_none(),
        "an embedded bundled subset that covers the text is kept, not a fallback: {:?}",
        second_result.font
    );

    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("cjk-simsun-fallback", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_text = extract_text(reopened.clone(), 0).expect("reopened text");
    let reopened_compact: String = reopened_text
        .text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        reopened_compact.contains("中文排版测试PDF二次"),
        "the saved fallback text must be extractable: {:?}",
        reopened_text.text
    );
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn new_text_boxes_can_use_the_bundled_font_for_chinese() {
    let font = text_box_fonts()
        .into_iter()
        .find(|font| font.family == "Noto Sans SC")
        .expect("the bundled font must be in the registry");
    assert_eq!(font.source, "embedded");
    assert!(font.embedded);
    assert!(font.embeddable);
    assert_eq!(font.coverage, "cjk");
    assert_eq!(font.postscript_name, "NotoSansSC");

    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_box(
        document_id.clone(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 600.0,
            width: 320.0,
            height: 80.0,
            style: Some(bundled_style(24.0)),
        },
    )
    .expect("create a Text Box with the bundled font");
    let box_id = created.target.box_id;
    assert_eq!(created.target.font_family, "Noto Sans SC");
    assert!(
        created.font.is_none(),
        "the chosen font is not a fallback: {:?}",
        created.font
    );

    let applied = apply_text_edit(document_id.clone(), 0, box_id, "中文 PDF 混合".to_owned())
        .expect("type Chinese into the new Text Box");
    assert!(
        applied.font.is_none(),
        "typing with the chosen font is not a fallback: {:?}",
        applied.font
    );

    let boxes = text_boxes(document_id.clone(), 0).expect("boxes");
    assert_eq!(boxes[0].font_family, "Noto Sans SC");
    assert_eq!(boxes[0].text, "中文 PDF 混合");

    // The glyphs are painted, not merely recorded in the content stream.
    let pixels = render_pixels(&document_id, 0, 2.0);
    let (ink, (left, top, right, _)) = ink_in_region(&pixels, 2.0, (72.0, 560.0, 392.0, 690.0));
    assert!(ink > 50, "the Chinese text must render, ink pixels: {ink}");
    assert!(left >= 71.0 && top >= 560.0 && right <= 392.0);

    // A fresh parse reports the same family, so the style panel still selects
    // the bundled font after the document is saved and reopened.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("cjk-text-box", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_layer = extract_text_layer(reopened.clone(), 0).expect("reopened layer");
    let reopened_compact: String = reopened_layer
        .text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        reopened_compact.contains("中文PDF混合"),
        "the saved Text Box must extract its Chinese text: {:?}",
        reopened_layer.text
    );
    let reopened_target = text_edit_target(
        reopened.clone(),
        0,
        char_index_of(&reopened_layer, "中") as u32,
    )
    .expect("reopened target");
    assert_eq!(reopened_target.font_family, "Noto Sans SC");
    assert!((reopened_target.font_size - 24.0).abs() < 0.01);
    assert!(!reopened_target.bold && !reopened_target.italic);

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn editing_existing_text_with_chinese_uses_the_bundled_font_and_survives_save() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let target = text_edit_target(
        document_id.clone(),
        0,
        char_index_of(&layer, "Original") as u32,
    )
    .expect("target");

    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "中文 PDF 混合".to_owned(),
    )
    .expect("Chinese replacement should be accepted");
    let font = result.font.expect("the fallback must be reported");
    assert!(font.fallback, "the font must be reported as a fallback");
    assert_eq!(font.original_family, "Helvetica");
    assert_eq!(font.family, "Noto Sans SC");
    let reason = font.reason.expect("the fallback needs a reason");
    assert!(
        reason.contains("Helvetica") && reason.contains("Noto Sans SC"),
        "the reason should name both fonts: {reason}"
    );

    // The replacement is searchable immediately and the original text is gone,
    // not stacked underneath the new text. Wrapping may put a line break
    // between characters, which extraction reports like a PDF viewer does.
    let edited = extract_text_layer(document_id.clone(), 0).expect("edited layer");
    let compact: String = edited.text.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        compact.matches("中文PDF混合").count(),
        1,
        "extracted text must contain the replacement exactly once: {:?}",
        edited.text
    );
    assert!(
        !edited.text.contains("Original line."),
        "the original text must be replaced: {:?}",
        edited.text
    );

    // The same holds after serialization and a fresh PDFium parse.
    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("cjk-edit", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_text = extract_text(reopened.clone(), 0).expect("reopened text");
    let reopened_compact: String = reopened_text
        .text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(
        reopened_compact.matches("中文PDF混合").count(),
        1,
        "the saved document must extract the Chinese replacement once: {:?}",
        reopened_text.text
    );
    assert!(
        !reopened_text.text.contains("Original line."),
        "the saved document must not keep the original text: {:?}",
        reopened_text.text
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}

#[test]
fn an_embedded_font_without_cjk_glyphs_falls_back_to_the_bundled_font() {
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

    // ttf-parser must prove coverage before the original program is reused.
    // Liberation Sans has no CJK glyphs, so the edit falls back rather than
    // writing .notdef boxes with the embedded font.
    let result = apply_text_edit(
        document_id.clone(),
        0,
        target.box_id,
        "Embedded 中文 sample.".to_owned(),
    )
    .expect("Chinese replacement should fall back, not fail");
    let font = result.font.expect("the fallback must be reported");
    assert!(font.fallback);
    assert!(
        font.original_family.contains("LiberationSans"),
        "the fallback must name the embedded original: {font:?}"
    );
    assert_eq!(font.family, "Noto Sans SC");
    let reason = font.reason.expect("the fallback needs a reason");
    assert!(
        reason.contains("missing glyphs") && reason.contains("Noto Sans SC"),
        "the reason must explain the missing coverage: {reason}"
    );

    let bytes = save_document_bytes(&document_id).expect("serialize edited document");
    let saved = synthetic::pdf_bytes_file("embedded-font-cjk-fallback", bytes);
    let reopened = open_synthetic(&saved);
    let reopened_text = extract_text(reopened.clone(), 0).expect("reopened text");
    let compact: String = reopened_text
        .text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        compact.contains("Embedded中文sample."),
        "the saved fallback text must be extractable: {:?}",
        reopened_text.text
    );

    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close");
}
