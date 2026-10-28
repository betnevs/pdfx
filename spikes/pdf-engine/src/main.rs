//! Runnable probe for the PDFX M0→M1 technical spike.
//!
//! Usage (from this directory):
//!
//! ```text
//! scripts/setup-native.sh
//! cargo run --release
//! ```
//!
//! Outputs are written to `spikes/pdf-engine/out/` (gitignored). The binary
//! fails loudly when the native assets or a fixture is missing; it never skips
//! a probe silently.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use pdfium_render::prelude::PdfDocumentMetadataTagType;
use pdfx_pdf_engine_spike::annotations::{
    add_appearance_streams_and_circle, add_whitelisted_annotations,
};
use pdfx_pdf_engine_spike::cjk_font::embed_cjk_subset;
use pdfx_pdf_engine_spike::engine::{
    cjk_font_path, CharInfo, RenderedPage, SpikeEngine,
};
use pdfx_pdf_engine_spike::save_pipeline::pdfium_then_lopdf;

const FIXTURES: &[(&str, Option<&str>)] = &[
    ("text-latin.pdf", None),
    ("scanned-image-only.pdf", None),
    ("encrypted-user-password.pdf", Some("pdfx-user")),
    ("structure-metadata.pdf", None),
];

fn main() -> Result<()> {
    let engine = SpikeEngine::shared()?;
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    fs::create_dir_all(&out)?;
    println!("PDFX spike outputs: {}", out.display());

    probe_open_render_extract(engine, &out)?;
    probe_native_annotations(engine, &out)?;
    probe_sequential_save(engine, &out)?;
    probe_cjk_subset(engine, &out)?;

    println!("SPIKE OK — all probes completed; see {}", out.display());
    Ok(())
}

fn probe_open_render_extract(engine: &SpikeEngine, out: &Path) -> Result<()> {
    println!("\n[AC1] open / render / extract");
    let dir = out.join("ac1");
    fs::create_dir_all(&dir)?;
    let mut summary = String::new();

    for (name, password) in FIXTURES {
        let path = fixture(name)?;
        let document = engine.open_file(&path, *password)?;
        let pages = document.pages().len();
        for page_index in 0..pages {
            let page = document.pages().get(page_index).context("page")?;
            let rendered = engine.render(&page, 1.0)?;
            let stats = rendered.stats();
            let stem = format!("{name}.page-{page_index}");
            let ppm = dir.join(format!("{stem}.ppm"));
            write_ppm(&ppm, &rendered)?;

            let text = page.text()?.all();
            let chars = engine.extract_chars(&page)?;
            let tsv = dir.join(format!("{stem}.chars.tsv"));
            write_chars_tsv(&tsv, &chars)?;

            let line = format!(
                "{name} page {page_index}/{pages}: text_chars={} extraction_chars={} render={}x{} non_white={} dark={} -> {}\n",
                text.chars().count(),
                chars.len(),
                stats.width,
                stats.height,
                stats.non_white_pixels,
                stats.dark_pixels,
                ppm.display()
            );
            print!("{line}");
            summary.push_str(&line);
        }
    }

    fs::write(dir.join("summary.txt"), summary)?;
    Ok(())
}

fn probe_native_annotations(engine: &SpikeEngine, out: &Path) -> Result<()> {
    println!("\n[AC2] native annotations");
    let dir = out.join("ac2");
    fs::create_dir_all(&dir)?;
    let source = fs::read(fixture("text-latin.pdf")?)?;

    let document = engine.open_bytes(&source, None)?;
    let before = {
        let page = document.pages().get(0)?;
        engine.render_stats(&page, 1.0)?
    };
    let created = {
        let mut page = document.pages().get(0)?;
        add_whitelisted_annotations(&mut page)?
    };
    let rewritten = document.save_to_bytes()?;
    drop(document);
    let final_bytes = add_appearance_streams_and_circle(engine, &rewritten)?;

    let pdf_path = dir.join("annotated.pdf");
    fs::write(&pdf_path, &final_bytes)?;

    let reopened = engine.open_bytes(&final_bytes, None)?;
    let page = reopened.pages().get(0)?;
    let kinds: Vec<String> = page
        .annotations()
        .iter()
        .map(|annotation| format!("{:?}", annotation.annotation_type()))
        .collect();
    let after = engine.render_stats(&page, 1.0)?;
    let ppm = dir.join("annotated.ppm");
    write_ppm(&ppm, &engine.render(&page, 1.0)?)?;

    let summary = format!(
        "high-level creators: {created:?}\n\
         reopened annotations: {kinds:?}\n\
         render non-white before={} after={} (delta={})\n\
         file: {} ({} bytes; source {} bytes; high-level rewrite {} bytes)\n\
         pdfium incremental prefix preserved: {}\n",
        before.non_white_pixels,
        after.non_white_pixels,
        after.non_white_pixels as i64 - before.non_white_pixels as i64,
        pdf_path.display(),
        final_bytes.len(),
        source.len(),
        rewritten.len(),
        final_bytes.starts_with(&rewritten)
    );
    print!("{summary}");
    fs::write(dir.join("summary.txt"), summary)?;
    Ok(())
}

fn probe_sequential_save(engine: &SpikeEngine, out: &Path) -> Result<()> {
    println!("\n[AC3] PDFium incremental save -> lopdf append");
    let dir = out.join("ac3");
    fs::create_dir_all(&dir)?;
    let source = fs::read(fixture("structure-metadata.pdf")?)?;
    let saved = pdfium_then_lopdf(engine, &source)?;

    let pdf_path = dir.join("sequential-save.pdf");
    fs::write(&pdf_path, &saved.final_bytes)?;

    let document = engine.open_bytes(&saved.final_bytes, None)?;
    let page = document.pages().get(0)?;
    let annotations: Vec<String> = page
        .annotations()
        .iter()
        .map(|annotation| format!("{:?}", annotation.annotation_type()))
        .collect();
    let bookmarks: Vec<String> = document
        .bookmarks()
        .iter()
        .filter_map(|bookmark| bookmark.title())
        .collect();
    let links = page.links().len();
    let title = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().to_string());
    let summary = format!(
        "source={} bytes\n\
         after PDFium incremental={} bytes (prefix preserved: {})\n\
         after lopdf append={} bytes (prefix preserved: {})\n\
         annotations after reopen: {annotations:?}\n\
         bookmarks after reopen: {bookmarks:?}\n\
         links: {links}; title: {title:?}; attachments: {}\n\
         file: {}\n",
        source.len(),
        saved.pdfium_bytes.len(),
        saved.pdfium_bytes.starts_with(&source),
        saved.final_bytes.len(),
        saved.final_bytes.starts_with(&saved.pdfium_bytes),
        document.attachments().len(),
        pdf_path.display()
    );
    print!("{summary}");
    fs::write(dir.join("summary.txt"), summary)?;
    Ok(())
}

fn probe_cjk_subset(engine: &SpikeEngine, out: &Path) -> Result<()> {
    println!("\n[AC4] CJK font subset");
    let dir = out.join("ac4");
    fs::create_dir_all(&dir)?;
    let text = "PDFX 中文字体测试 你好世界你好";
    let embedding = embed_cjk_subset(&cjk_font_path()?, text)?;

    let pdf_path = dir.join("cjk-subset.pdf");
    let font_path = dir.join("NotoSansSC-subset.ttf");
    fs::write(&pdf_path, &embedding.pdf_bytes)?;
    fs::write(&font_path, &embedding.subset_font_bytes)?;

    let reopened = engine.open_bytes(&embedding.pdf_bytes, None)?;
    let page = reopened.pages().get(0)?;
    let extracted = page.text()?.all();
    let stats = engine.render_stats(&page, 1.0)?;
    let ppm = dir.join("cjk-subset.ppm");
    write_ppm(&ppm, &engine.render(&page, 1.0)?)?;

    let mut mapping = String::new();
    for (cid, new_gid) in &embedding.cid_to_gid {
        let width = embedding.widths.get(cid).copied().unwrap_or(0);
        let unicode = embedding
            .unicode_by_cid
            .get(cid)
            .cloned()
            .unwrap_or_default();
        mapping.push_str(&format!(
            "cid={cid:5} ({cid:#06x}) -> gid={new_gid:5} width={width:4} unicode={unicode:?}\n"
        ));
    }

    let summary = format!(
        "source font: {} bytes\nsubset font: {} bytes ({} glyphs)\n\
         cid entries: {}\nextracted by PDFium: {extracted:?}\n\
         render non-white: {}\nfile: {}\nfont: {}\nppm: {}\n\n{mapping}",
        embedding.source_font_bytes,
        embedding.subset_font_bytes.len(),
        embedding.subset_glyph_count,
        embedding.cid_to_gid.len(),
        stats.non_white_pixels,
        pdf_path.display(),
        font_path.display(),
        ppm.display()
    );
    print!("{summary}");
    fs::write(dir.join("summary.txt"), summary)?;
    Ok(())
}

fn fixture(name: &str) -> Result<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pdfs")
        .join(name);
    if !path.exists() {
        bail!("fixture not found: {}", path.display());
    }
    Ok(path)
}

fn write_ppm(path: &Path, rendered: &RenderedPage) -> Result<()> {
    let expected = rendered.width as usize * rendered.height as usize * 4;
    if rendered.rgba.len() != expected {
        return Err(anyhow!(
            "bitmap is {} bytes, expected {expected} RGBA bytes",
            rendered.rgba.len()
        ));
    }
    let mut ppm = format!("P6\n{} {}\n255\n", rendered.width, rendered.height).into_bytes();
    ppm.reserve(rendered.width as usize * rendered.height as usize * 3);
    for pixel in rendered.rgba.chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, ppm).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_chars_tsv(path: &Path, chars: &[CharInfo]) -> Result<()> {
    let mut tsv = String::from(
        "unicode\tfont\tsize\tcolor_r\tcolor_g\tcolor_b\torigin_x\torigin_y\tleft\tbottom\tright\ttop\n",
    );
    for character in chars {
        let unicode = match character.unicode {
            Some(' ') => "space".to_string(),
            Some('\t') | Some('\n') => format!("{:?}", character.unicode.unwrap()),
            Some(value) => value.to_string(),
            None => String::new(),
        };
        tsv.push_str(&format!(
            "{unicode}\t{}\t{:.3}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\n",
            character.font_name,
            character.font_size,
            character.fill_color[0],
            character.fill_color[1],
            character.fill_color[2],
            character.origin.0,
            character.origin.1,
            character.bounds.0,
            character.bounds.1,
            character.bounds.2,
            character.bounds.3
        ));
    }
    fs::write(path, tsv).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
