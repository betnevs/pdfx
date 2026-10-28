//! CJK Font Subset embedding probe.
//!
//! Pipeline: `rustybuzz` shapes the string and reports the old glyph IDs and
//! advances; `subsetter` produces a TrueType subset whose glyph IDs are the
//! remapped IDs; this module writes a Type0 / Identity-H font dictionary with a
//! **non-identity** `/CIDToGIDMap` stream (CID = old glyph ID), a `/W` width
//! array, and a ToUnicode CMap.
//!
//! Using the old glyph ID as the CID (instead of the subsetter's new ID) is
//! deliberate: it exercises PDFium's CIDToGIDMap handling and makes the GID
//! remapping observable in the output file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use lopdf::{Document, Object, Stream, dictionary};
use rustybuzz::{Direction, UnicodeBuffer};

/// One embedded CJK subset and the bookkeeping needed to validate it.
#[derive(Debug, Clone)]
pub struct CjkEmbedding {
    /// The shaped source text.
    pub text: String,
    /// A complete, one-page PDF containing the embedded subset.
    pub pdf_bytes: Vec<u8>,
    /// The TrueType subset produced by `subsetter`.
    pub subset_font_bytes: Vec<u8>,
    /// Size of the source font file, for subset size comparisons.
    pub source_font_bytes: usize,
    /// Glyph count of the subset font (`maxp.numGlyphs`).
    pub subset_glyph_count: u16,
    /// Used CID (= old glyph ID) -> new glyph ID in the subset font.
    pub cid_to_gid: BTreeMap<u16, u16>,
    /// Used CID -> advance width in 1/1000 em units.
    pub widths: BTreeMap<u16, u16>,
    /// Used CID -> Unicode text for the ToUnicode CMap.
    pub unicode_by_cid: BTreeMap<u16, String>,
}

const FONT_SIZE: f32 = 24.0;
const PAGE_WIDTH: f32 = 612.0;
const PAGE_HEIGHT: f32 = 792.0;
const TEXT_X: f32 = 72.0;
const TEXT_Y: f32 = 600.0;
const BASE_FONT: &str = "PDFXNotoSansSC";

/// Shapes `text` with the font at `font_path`, subsets it, and returns a
/// one-page PDF that embeds the subset as a CID font.
pub fn embed_cjk_subset(font_path: &Path, text: &str) -> Result<CjkEmbedding> {
    let font_data = fs::read(font_path)
        .with_context(|| format!("read CJK font {}", font_path.display()))?;
    let face = rustybuzz::Face::from_slice(&font_data, 0)
        .ok_or_else(|| anyhow!("rustybuzz cannot parse {}", font_path.display()))?;
    let metrics_face = ttf_parser::Face::parse(&font_data, 0)
        .map_err(|error| anyhow!("ttf-parser cannot parse {}: {error}", font_path.display()))?;
    let units_per_em = f32::from(metrics_face.units_per_em());

    let shaped = shape(&face, text)?;
    let glyph_order: Vec<u16> = shaped.iter().map(|glyph| glyph.glyph_id).collect();
    let mut cid_to_gid = BTreeMap::new();
    let mut widths = BTreeMap::new();
    let mut unicode_by_cid = BTreeMap::new();
    let mut used = Vec::new();
    let mut seen = BTreeSet::new();

    for glyph in &shaped {
        let old_gid = glyph.glyph_id;
        if old_gid == 0 {
            bail!("font has no glyph for cluster {:?}", glyph.text);
        }
        let width = (glyph.x_advance as f32 * 1000.0 / units_per_em).round();
        if !(0.0..=u16::MAX as f32).contains(&width) {
            bail!("advance out of range for glyph {old_gid}: {width}");
        }
        let width = width as u16;

        match widths.get(&old_gid) {
            Some(previous) if *previous != width => bail!(
                "glyph {old_gid} has inconsistent advances {previous} and {width} in the same run"
            ),
            Some(_) => {}
            None => {
                widths.insert(old_gid, width);
            }
        }

        if seen.insert(old_gid) {
            used.push(old_gid);
            unicode_by_cid.insert(old_gid, glyph.text.clone());
        } else if let Some(existing) = unicode_by_cid.get(&old_gid) {
            if existing != &glyph.text {
                bail!(
                    "CID {old_gid} would need two Unicode mappings: {existing:?} and {:?}",
                    glyph.text
                );
            }
        }
    }

    let mut remapper = subsetter::GlyphRemapper::new();
    for gid in &used {
        remapper.remap(*gid);
    }
    for gid in &used {
        let new_gid = remapper
            .get(*gid)
            .ok_or_else(|| anyhow!("no subset glyph for old GID {gid}"))?;
        cid_to_gid.insert(*gid, new_gid);
    }

    let subset_font_bytes =
        subsetter::subset(&font_data, 0, &remapper).context("subset CJK font")?;
    let subset_face = ttf_parser::Face::parse(&subset_font_bytes, 0)
        .map_err(|error| anyhow!("ttf-parser cannot parse subset: {error}"))?;
    let subset_glyph_count = subset_face.number_of_glyphs();

    let pdf_bytes = write_pdf(
        &subset_font_bytes,
        &metrics_face,
        &glyph_order,
        &cid_to_gid,
        &widths,
        &unicode_by_cid,
    )?;

    Ok(CjkEmbedding {
        text: text.to_string(),
        pdf_bytes,
        subset_font_bytes,
        source_font_bytes: font_data.len(),
        subset_glyph_count,
        cid_to_gid,
        widths,
        unicode_by_cid,
    })
}

struct ShapedGlyph {
    glyph_id: u16,
    x_advance: i32,
    /// Source text covered by this glyph (usually one character; several for a
    /// ligature) as UTF-8.
    text: String,
}

fn shape(face: &rustybuzz::Face<'_>, text: &str) -> Result<Vec<ShapedGlyph>> {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(Direction::LeftToRight);
    buffer.set_script(rustybuzz::script::HAN);
    let glyphs = rustybuzz::shape(face, &[], buffer);

    let infos = glyphs.glyph_infos();
    let positions = glyphs.glyph_positions();
    if infos.len() != positions.len() {
        bail!(
            "rustybuzz returned {} infos for {} positions",
            infos.len(),
            positions.len()
        );
    }

    let clusters: Vec<usize> = infos.iter().map(|info| info.cluster as usize).collect();
    let mut shaped = Vec::with_capacity(infos.len());
    for (index, (info, position)) in infos.iter().zip(positions).enumerate() {
        let start = clusters[index];
        let end = clusters[index + 1..]
            .iter()
            .copied()
            .find(|cluster| *cluster > start)
            .unwrap_or(text.len());
        let cluster_text = text
            .get(start..end)
            .ok_or_else(|| anyhow!("cluster range {start}..{end} is not a char boundary"))?;
        shaped.push(ShapedGlyph {
            glyph_id: u16::try_from(info.glyph_id)
                .map_err(|_| anyhow!("glyph ID {} does not fit in u16", info.glyph_id))?,
            x_advance: position.x_advance,
            text: cluster_text.to_string(),
        });
    }
    Ok(shaped)
}

#[allow(clippy::too_many_arguments)]
fn write_pdf(
    subset_font: &[u8],
    metrics: &ttf_parser::Face<'_>,
    glyph_order: &[u16],
    cid_to_gid: &BTreeMap<u16, u16>,
    widths: &BTreeMap<u16, u16>,
    unicode_by_cid: &BTreeMap<u16, String>,
) -> Result<Vec<u8>> {
    let scale = 1000.0 / f32::from(metrics.units_per_em());
    let scale_i = |value: i16| (f32::from(value) * scale).round() as i64;

    let bbox = metrics.global_bounding_box();
    let ascender = scale_i(metrics.ascender());
    let descender = scale_i(metrics.descender());
    let cap_height = metrics
        .capital_height()
        .map(scale_i)
        .unwrap_or(ascender);

    let max_cid = cid_to_gid.keys().copied().max().unwrap_or(0);
    let mut gid_map = vec![0u8; (usize::from(max_cid) + 1) * 2];
    for (cid, new_gid) in cid_to_gid {
        let offset = usize::from(*cid) * 2;
        gid_map[offset..offset + 2].copy_from_slice(&new_gid.to_be_bytes());
    }

    let mut widths_array = Vec::with_capacity(widths.len() * 2);
    for (cid, width) in widths {
        widths_array.push(Object::Integer(i64::from(*cid)));
        widths_array.push(Object::Array(vec![Object::Integer(i64::from(*width))]));
    }

    let mut content_codes = String::new();
    for cid in glyph_order {
        content_codes.push_str(&format!("{cid:04X}"));
    }
    let content = format!(
        "BT\n/F1 {FONT_SIZE} Tf\n{TEXT_X} {TEXT_Y} Td\n<{content_codes}> Tj\nET\n"
    );

    let mut document = Document::with_version("1.7");

    let content_id = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let font_file_id = document.add_object(Stream::new(
        dictionary! {
            "Length1" => i64::try_from(subset_font.len())?,
        },
        subset_font.to_vec(),
    ));
    let tounicode_id = document.add_object(Stream::new(
        dictionary! {},
        to_unicode_cmap(unicode_by_cid).into_bytes(),
    ));
    let gid_map_id = document.add_object(Stream::new(dictionary! {}, gid_map));
    let descriptor_id = document.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(BASE_FONT.as_bytes().to_vec()),
        "Flags" => 4,
        "FontBBox" => vec![
            Object::Integer(scale_i(bbox.x_min)),
            Object::Integer(scale_i(bbox.y_min)),
            Object::Integer(scale_i(bbox.x_max)),
            Object::Integer(scale_i(bbox.y_max)),
        ],
        "ItalicAngle" => 0,
        "Ascent" => ascender,
        "Descent" => descender,
        "CapHeight" => cap_height,
        "StemV" => 80,
        "FontFile2" => Object::Reference(font_file_id),
    });
    let cid_font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => Object::Name(BASE_FONT.as_bytes().to_vec()),
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::Reference(descriptor_id),
        "CIDToGIDMap" => Object::Reference(gid_map_id),
        "DW" => 1000,
        "W" => Object::Array(widths_array),
    });
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(BASE_FONT.as_bytes().to_vec()),
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid_font_id)],
        "ToUnicode" => Object::Reference(tounicode_id),
    });
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(PAGE_WIDTH as i64),
            Object::Integer(PAGE_HEIGHT as i64),
        ],
        "Resources" => dictionary! {
            "Font" => dictionary! {
                "F1" => Object::Reference(font_id),
            },
        },
        "Contents" => Object::Reference(content_id),
    });
    let pages_id = document.add_object(dictionary! {
        "Type" => "Pages",
        "Kids" => vec![Object::Reference(page_id)],
        "Count" => 1,
    });
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    document.trailer.set("Root", Object::Reference(catalog_id));

    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .context("write CJK PDF with lopdf")?;
    Ok(output)
}

fn to_unicode_cmap(unicode_by_cid: &BTreeMap<u16, String>) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <0000> <FFFF>\n\
         endcodespacerange\n",
    );

    let entries: Vec<(&u16, &String)> = unicode_by_cid.iter().collect();
    for chunk in entries.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (cid, value) in chunk {
            let unicode_hex: String = value
                .encode_utf16()
                .map(|unit| format!("{unit:04X}"))
                .collect();
            cmap.push_str(&format!("<{cid:04X}> <{unicode_hex}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }

    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}
