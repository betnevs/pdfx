//! Acceptance tests for the PDFX M0→M1 technical spike (ticket 03).
//!
//! Each test maps to one acceptance criterion in
//! `docs/planning/issues/03-technical-spike.md`. The tests drive the spike's public
//! probe API; they fail loudly (rather than skipping) when the native assets are
//! missing, because a skipped probe is not evidence for a go/no-go decision.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use pdfx_pdf_engine_spike::annotations::{
    add_appearance_streams_and_circle, add_whitelisted_annotations,
    whitelist_high_level, WhitelistedAnnotation,
};
use pdfx_pdf_engine_spike::cjk_font::embed_cjk_subset;
use pdfx_pdf_engine_spike::engine::{cjk_font_path, SpikeEngine};
use pdfx_pdf_engine_spike::save_pipeline::pdfium_then_lopdf;

fn fixtures_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PDFX_FIXTURES_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .canonicalize()
        .expect("fixtures/ directory")
}

fn fixture(name: &str) -> PathBuf {
    fixtures_dir().join("pdfs").join(name)
}

/// AC1: open and render text, scanned, encrypted, and structure PDFs; extract
/// per-character text with font, size, color, and position.
#[test]
fn ac1_opens_renders_and_extracts_fixture_kinds() {
    let engine = SpikeEngine::shared().expect("PDFium must be set up (scripts/setup-native.sh)");

    // --- text PDF ---------------------------------------------------------
    let doc = engine
        .open_file(&fixture("text-latin.pdf"), None)
        .expect("open text fixture");
    let page = doc.pages().get(0).expect("page 0");
    assert!((page.width().value - 612.0).abs() < 0.5);
    assert!((page.height().value - 792.0).abs() < 0.5);

    let render = engine.render_stats(&page, 1.0).expect("render text page");
    assert_eq!(render.width, 612);
    assert_eq!(render.height, 792);
    assert!(
        render.non_white_pixels > 1_000,
        "text page must render visible glyphs, got {} non-white pixels",
        render.non_white_pixels
    );

    let text = page.text().expect("text layer").all();
    assert!(text.contains("The quick brown fox"), "extracted: {text:?}");

    let chars = engine.extract_chars(&page).expect("per-char extraction");
    assert!(chars.len() >= 100, "got only {} chars", chars.len());
    let first = &chars[0];
    assert_eq!(first.unicode, Some('P'));
    assert_eq!(first.font_name, "Helvetica");
    assert!(
        (first.font_size - 18.0).abs() < 0.5,
        "font size {}, expected 18",
        first.font_size
    );
    assert_eq!(
        first.fill_color,
        [0, 0, 0],
        "text fills black, got {:?}",
        first.fill_color
    );
    assert!(
        (first.origin.0 - 72.0).abs() < 3.0 && (first.origin.1 - 720.0).abs() < 3.0,
        "first char origin {:?}, expected near (72, 720)",
        first.origin
    );
    assert!(first.bounds.2 > first.bounds.0, "bounds must have width");

    // --- scanned PDF ------------------------------------------------------
    let doc = engine
        .open_file(&fixture("scanned-image-only.pdf"), None)
        .expect("open scanned fixture");
    let page = doc.pages().get(0).expect("page 0");
    let scanned_text = page.text().expect("text layer").all();
    assert!(
        scanned_text.trim().is_empty(),
        "scanned fixture must not expose a text layer, got {scanned_text:?}"
    );
    let render = engine.render_stats(&page, 1.0).expect("render scanned page");
    assert!(
        render.non_white_pixels > (render.width as usize * render.height as usize) / 2,
        "scanned page should be mostly image pixels, got {}",
        render.non_white_pixels
    );

    // --- encrypted PDF with a user password -------------------------------
    assert!(
        engine
            .open_file(&fixture("encrypted-user-password.pdf"), Some("wrong"))
            .is_err(),
        "a wrong user password must not open the document"
    );
    let doc = engine
        .open_file(&fixture("encrypted-user-password.pdf"), Some("pdfx-user"))
        .expect("open with user password");
    let text = doc.pages().get(0).unwrap().text().unwrap().all();
    assert!(
        text.contains("PDFX encrypted fixture."),
        "encrypted text: {text:?}"
    );

    // --- owner-password restrictions --------------------------------------
    let doc = engine
        .open_file(&fixture("encrypted-owner-restrictions.pdf"), Some(""))
        .expect("open with empty user password");
    let text = doc.pages().get(0).unwrap().text().unwrap().all();
    assert!(!text.is_empty());
    let permissions = doc.permissions();
    assert!(
        !permissions.can_modify_document_content().unwrap_or(true),
        "owner restrictions must forbid content modification"
    );
    assert!(
        !permissions
            .can_add_or_modify_text_annotations()
            .unwrap_or(true),
        "owner restrictions must forbid annotation changes"
    );
    assert!(
        !permissions.can_extract_text_and_graphics().unwrap_or(true),
        "owner restrictions must forbid text extraction"
    );
    assert!(
        permissions.can_print_high_quality().unwrap_or(false),
        "owner restrictions allow printing (R2 has no quality split)"
    );

    // --- structure PDF ----------------------------------------------------
    let doc = engine
        .open_file(&fixture("structure-metadata.pdf"), None)
        .expect("open structure fixture");
    let page = doc.pages().get(0).expect("page 0");
    let link_count = page.links().len();
    assert_eq!(link_count, 1, "structure fixture has one URI link");
    let link = page.links().first().expect("first link");
    match link.action() {
        Some(pdfium_render::prelude::PdfAction::Uri(uri)) => {
            assert_eq!(uri.uri().unwrap(), "https://example.com/pdfx");
        }
        other => panic!("expected URI link, got {:?}", other.is_some()),
    }

    let bookmark_titles: Vec<String> = doc
        .bookmarks()
        .iter()
        .filter_map(|bookmark| bookmark.title())
        .collect();
    assert!(
        bookmark_titles.iter().any(|t| t == "Introduction"),
        "bookmark titles: {bookmark_titles:?}"
    );
    assert!(
        bookmark_titles.iter().any(|t| t == "Details"),
        "bookmark titles: {bookmark_titles:?}"
    );

    let title = doc
        .metadata()
        .get(pdfium_render::prelude::PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().to_string());
    assert_eq!(title.as_deref(), Some("PDFX structure fixture"));
    assert_eq!(doc.attachments().len(), 1, "one embedded attachment");
}

/// AC2: create highlight, underline, strikeout, note, ink, square, and circle
/// native annotations; save; reopen with PDFium and see them.
#[test]
fn ac2_creates_whitelisted_native_annotations_and_reopens_them() {
    let engine = SpikeEngine::shared().expect("PDFium must be set up");
    let source = std::fs::read(fixture("text-latin.pdf")).expect("read text fixture");

    let doc = engine.open_bytes(&source, None).expect("open fixture");
    let before = {
        let page = doc.pages().get(0).expect("page 0");
        engine.render_stats(&page, 1.0).expect("render before")
    };

    let created = {
        let mut page = doc.pages().get(0).expect("page 0");
        add_whitelisted_annotations(&mut page).expect("create annotations")
    };
    assert_eq!(
        created.len(),
        whitelist_high_level().len(),
        "high-level creators cover every whitelisted type except circle"
    );
    assert!(!created.contains(&WhitelistedAnnotation::Circle));

    let rewritten = doc.save_to_bytes().expect("save annotated document");
    drop(doc);
    assert!(
        rewritten.len() > source.len(),
        "annotation save must grow the file"
    );

    // The hybrid pass completes the whitelist and installs the `/AP` streams
    // PDFium needs to paint each annotation, then appends an incremental
    // revision on top of the high-level full rewrite.
    let saved = add_appearance_streams_and_circle(engine, &rewritten)
        .expect("appearance streams + circle + incremental save");
    assert!(
        saved.starts_with(&rewritten),
        "raw save must append to the rewritten document, not rewrite it \
         (rewritten={} bytes, saved={} bytes)",
        rewritten.len(),
        saved.len()
    );
    assert!(saved.len() > rewritten.len(), "incremental revision adds bytes");

    let reopened = engine.open_bytes(&saved, None).expect("reopen saved PDF");
    let page = reopened.pages().get(0).expect("page 0");
    let annotations: Vec<_> = page.annotations().iter().collect();
    assert_eq!(annotations.len(), 7, "seven native annotations survive");

    let types: Vec<PdfPageAnnotationType> =
        annotations.iter().map(|a| a.annotation_type()).collect();
    for expected in WhitelistedAnnotation::ALL {
        assert!(
            types.contains(&expected.annotation_type()),
            "missing {:?}; reopened types: {types:?}",
            expected
        );
    }

    let highlight = annotations
        .iter()
        .find(|a| a.annotation_type() == PdfPageAnnotationType::Highlight)
        .expect("highlight");
    assert_eq!(highlight.contents().as_deref(), Some("PDFX highlight"));
    assert_eq!(highlight.creator().as_deref(), Some("PDFX spike"));

    let after = engine.render_stats(&page, 1.0).expect("render after");
    assert!(
        after.non_white_pixels > before.non_white_pixels + 500,
        "annotations must be visible after reopen: before={} after={}",
        before.non_white_pixels,
        after.non_white_pixels
    );
}

/// AC3: complete one sequential save — PDFium incremental save, then a lopdf
/// appended revision — and verify bookmarks, links, and metadata survive.
#[test]
fn ac3_pdfium_incremental_then_lopdf_append_preserves_structure() {
    let engine = SpikeEngine::shared().expect("PDFium must be set up");
    let source = std::fs::read(fixture("structure-metadata.pdf")).expect("read fixture");
    let before = {
        let doc = engine.open_bytes(&source, None).expect("open source");
        let page = doc.pages().get(0).expect("page 0");
        engine.render_stats(&page, 1.0).expect("render source")
    };

    let saved = pdfium_then_lopdf(engine, &source).expect("sequential save");

    assert!(
        saved.pdfium_bytes.starts_with(&source),
        "PDFium stage must append an incremental revision"
    );
    assert!(
        saved.final_bytes.starts_with(&saved.pdfium_bytes),
        "lopdf stage must append to the PDFium revision"
    );
    assert!(saved.final_bytes.len() > saved.pdfium_bytes.len());

    // The PDFium edit (a painted square) survives the lopdf revision.
    let doc = engine
        .open_bytes(&saved.final_bytes, None)
        .expect("reopen final bytes with PDFium");
    let page = doc.pages().get(0).expect("page 0");
    let annotations: Vec<_> = page.annotations().iter().collect();
    let squares: Vec<_> = annotations
        .iter()
        .filter(|annotation| annotation.annotation_type() == PdfPageAnnotationType::Square)
        .collect();
    assert_eq!(squares.len(), 1, "one PDFium-added square annotation");
    assert_eq!(
        squares[0].contents().as_deref(),
        Some("PDFium incremental square")
    );

    // The lopdf edit (an Info dictionary entry) lands in the appended revision.
    let parsed = lopdf::Document::load_mem(&saved.final_bytes).expect("lopdf reread");
    let info_id = parsed
        .trailer
        .get(b"Info")
        .and_then(|object| object.as_reference())
        .expect("/Info reference");
    let info = parsed
        .get_object(info_id)
        .and_then(|object| object.as_dict())
        .expect("/Info dictionary");
    assert_eq!(
        info.get(b"PDFXSpike").and_then(|object| object.as_str()).ok(),
        Some(&b"lopdf-append"[..]),
        "lopdf marker missing from appended revision"
    );

    // Structure from the original revision is intact.
    let titles: Vec<String> = doc
        .bookmarks()
        .iter()
        .filter_map(|bookmark| bookmark.title())
        .collect();
    for expected in ["Introduction", "Details"] {
        assert!(
            titles.iter().any(|title| title == expected),
            "bookmark {expected:?} lost; titles: {titles:?}"
        );
    }

    let link = page.links().first().expect("URI link survived");
    match link.action() {
        Some(pdfium_render::prelude::PdfAction::Uri(uri)) => {
            assert_eq!(uri.uri().unwrap(), "https://example.com/pdfx");
        }
        other => panic!("expected URI link, got {:?}", other.is_some()),
    }

    let title = doc
        .metadata()
        .get(pdfium_render::prelude::PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().to_string());
    assert_eq!(title.as_deref(), Some("PDFX structure fixture"));
    assert_eq!(doc.attachments().len(), 1, "attachment survived");

    let xmp = String::from_utf8_lossy(&saved.final_bytes);
    assert!(
        xmp.contains("<x:xmpmeta") && xmp.contains("PDFX structure fixture"),
        "XMP metadata stream lost"
    );

    let after = engine.render_stats(&page, 1.0).expect("render final");
    assert!(
        after.non_white_pixels > before.non_white_pixels + 500,
        "PDFium-added square must paint: before={} after={}",
        before.non_white_pixels,
        after.non_white_pixels
    );
}

/// AC4: embed a subset of the OFL CJK font with a non-identity CIDToGIDMap,
/// `/W` widths and a ToUnicode CMap; reopen with PDFium and extract the text.
#[test]
fn ac4_embeds_cjk_font_subset_and_pdfium_extracts_text() {
    let engine = SpikeEngine::shared().expect("PDFium must be set up");
    let font_path = cjk_font_path().expect("CJK font must be set up (scripts/setup-native.sh)");
    let text = "PDFX 中文字体测试 你好世界你好";

    let embedding = embed_cjk_subset(&font_path, text).expect("embed CJK subset");
    assert_eq!(embedding.text, text);

    // Subsetting must shrink the 17 MB source dramatically and only keep the
    // glyphs used by this string.
    assert!(
        embedding.subset_font_bytes.len() < embedding.source_font_bytes / 10,
        "subset is {} bytes of {} source bytes",
        embedding.subset_font_bytes.len(),
        embedding.source_font_bytes
    );

    // GID remapping: every used CID maps into the compact subset glyph range,
    // and at least one CID is not a fixed point (otherwise no remapping
    // happened at all).
    assert!(!embedding.cid_to_gid.is_empty());
    for (cid, new_gid) in &embedding.cid_to_gid {
        assert_ne!(*new_gid, 0, "CID {cid} must not map to .notdef");
        assert!(
            (1..=embedding.subset_glyph_count).contains(new_gid),
            "CID {cid} -> GID {new_gid} outside subset glyph count {}",
            embedding.subset_glyph_count
        );
    }
    assert!(
        embedding
            .cid_to_gid
            .iter()
            .any(|(cid, gid)| u16::from(*cid) != *gid),
        "expected a non-identity GID remapping, got {:?}",
        embedding.cid_to_gid
    );

    // Independent typographic expectation: CJK ideographs are full-width in
    // Noto Sans SC, so their PDF width must be 1000/1000 em.
    let cjk_cid = embedding
        .unicode_by_cid
        .iter()
        .find(|(_, value)| value.as_str() == "中")
        .map(|(cid, _)| *cid)
        .expect("CID for 中");
    assert_eq!(embedding.widths[&cjk_cid], 1000);

    // The serialized PDF must carry the CIDToGIDMap stream, /W widths and
    // ToUnicode entries.
    let parsed = lopdf::Document::load_mem(&embedding.pdf_bytes).expect("lopdf parse");
    let page_id = parsed.get_pages().get(&1).copied().expect("page 1");
    let page = parsed
        .get_object(page_id)
        .and_then(|object| object.as_dict())
        .expect("page dict");
    let resources = page
        .get(b"Resources")
        .and_then(|object| object.as_dict())
        .expect("resources");
    let fonts = resources
        .get(b"Font")
        .and_then(|object| object.as_dict())
        .expect("fonts");
    let font_id = fonts
        .get(b"F1")
        .and_then(|object| object.as_reference())
        .expect("/F1 reference");
    let font = parsed
        .get_object(font_id)
        .and_then(|object| object.as_dict())
        .expect("Type0 font dict");
    assert_eq!(
        font.get(b"Subtype").and_then(|object| object.as_name()).unwrap(),
        b"Type0"
    );
    assert_eq!(
        font.get(b"Encoding").and_then(|object| object.as_name()).unwrap(),
        b"Identity-H"
    );
    let descendants = font
        .get(b"DescendantFonts")
        .and_then(|object| object.as_array())
        .expect("DescendantFonts");
    let cid_font_id = descendants[0]
        .as_reference()
        .expect("descendant reference");
    let cid_font = parsed
        .get_object(cid_font_id)
        .and_then(|object| object.as_dict())
        .expect("CIDFont dict");
    assert_eq!(
        cid_font
            .get(b"Subtype")
            .and_then(|object| object.as_name())
            .unwrap(),
        b"CIDFontType2"
    );

    let map_id = cid_font
        .get(b"CIDToGIDMap")
        .and_then(|object| object.as_reference())
        .expect("CIDToGIDMap stream reference");
    let map_stream = parsed
        .get_object(map_id)
        .and_then(|object| object.as_stream())
        .expect("CIDToGIDMap stream");
    let map_bytes = &map_stream.content;
    for (cid, new_gid) in &embedding.cid_to_gid {
        let offset = usize::from(*cid) * 2;
        let stored = u16::from_be_bytes([map_bytes[offset], map_bytes[offset + 1]]);
        assert_eq!(stored, *new_gid, "CIDToGIDMap entry for CID {cid}");
    }

    let widths = cid_font
        .get(b"W")
        .and_then(|object| object.as_array())
        .expect("/W array");
    let mut parsed_widths = std::collections::BTreeMap::new();
    for pair in widths.chunks(2) {
        let cid = pair[0].as_i64().expect("W cid") as u16;
        let width = pair[1].as_array().expect("W width array")[0]
            .as_i64()
            .expect("W width") as u16;
        parsed_widths.insert(cid, width);
    }
    for (cid, width) in &embedding.widths {
        assert_eq!(parsed_widths.get(cid), Some(width), "/W entry for CID {cid}");
    }

    let tounicode_id = font
        .get(b"ToUnicode")
        .and_then(|object| object.as_reference())
        .expect("/ToUnicode reference");
    let tounicode = parsed
        .get_object(tounicode_id)
        .and_then(|object| object.as_stream())
        .expect("ToUnicode stream");
    let tounicode_text = String::from_utf8_lossy(&tounicode.content);
    for (cid, value) in &embedding.unicode_by_cid {
        let unicode_hex: String = value
            .encode_utf16()
            .map(|unit| format!("{unit:04X}"))
            .collect();
        assert!(
            tounicode_text.contains(&format!("<{cid:04X}> <{unicode_hex}>")),
            "ToUnicode entry for CID {cid} ({value:?}) missing"
        );
    }

    // PDFium reopens the embedded font and extracts the original string. This
    // is the independent proof that CIDToGIDMap, /W and ToUnicode agree.
    let reopened = engine
        .open_bytes(&embedding.pdf_bytes, None)
        .expect("PDFium reopen embedded PDF");
    let page = reopened.pages().get(0).expect("page 0");
    let extracted = page.text().expect("text layer").all();
    assert!(
        extracted.contains("中文字体测试") && extracted.contains("你好世界"),
        "PDFium extracted {extracted:?}"
    );
    assert!(
        !extracted.contains('\u{FFFD}'),
        "replacement characters in extracted text: {extracted:?}"
    );

    let chars = engine.extract_chars(&page).expect("per-char extraction");
    let per_char: String = chars.iter().filter_map(|char| char.unicode).collect();
    assert!(
        per_char.contains("中文字体测试") && per_char.contains("你好世界"),
        "per-char extraction {per_char:?}"
    );
    assert!(chars.iter().any(|char| (char.font_size - 24.0).abs() < 0.5));
    assert!(
        chars
            .iter()
            .all(|char| char.font_name.is_empty() || char.font_name.contains("PDFX")),
        "unexpected font names: {:?}",
        chars
            .iter()
            .map(|char| &char.font_name)
            .collect::<std::collections::BTreeSet<_>>()
    );

    let render = engine.render_stats(&page, 1.0).expect("render CJK page");
    assert!(
        render.non_white_pixels > 500,
        "subset glyphs must render, got {} non-white pixels",
        render.non_white_pixels
    );
}
