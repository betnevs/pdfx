//! Bundled fonts, glyph coverage and CID font subsetting.
//!
//! ADR-0006 allows only fonts the project may freely embed. Ticket 11 bundles
//! Noto Sans SC, a variable TrueType font (SIL Open Font License 1.1), and
//! embeds only the glyphs a Text Box actually uses. TrueType outlines keep the
//! PDF font a `CIDFontType2` with a `FontFile2` program, matching the pipeline
//! ADR-0010 describes; no CFF/OTF CIDFontType0 path is needed.
//!
//! Text written with a bundled font goes through PDFium's
//! `FPDFText_LoadCidType2Font`, which takes a caller-supplied ToUnicode CMap
//! and CIDToGIDMap instead of generating them. PDFX assigns one dense CID per
//! distinct (source glyph, character) pair in first-appearance order, subsets
//! the source font with `subsetter` (which renumbers glyphs), and writes the
//! CIDToGIDMap from that old-to-new mapping. PDFium then writes the Type0
//! font, `/W` widths from the subset's `hmtx`, and the ToUnicode CMap into its
//! incremental revision, so the text stays searchable and copyable.
//!
//! The subset's original `cmap` is dropped by `subsetter`, so one is injected
//! back after subsetting. PDFium does not use it (codes are CIDs), but it lets
//! `ttf-parser` prove glyph coverage when a saved document is reopened, which
//! is what allows a later edit to reuse the embedded subset instead of
//! embedding a second copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use pdfium_render::prelude::{PdfDocument, PdfFontToken};

use crate::commands::{CommandError, CommandErrorCode};

/// Family name shown in the Text Box style panel and stored in session styles.
pub(crate) const NOTO_SANS_SC_FAMILY: &str = "Noto Sans SC";
const NOTO_SANS_SC_LABEL: &str = "思源黑体";
const NOTO_SANS_SC_POSTSCRIPT: &str = "NotoSansSC";

/// Serif companion for editing text that came from a Song/Ming-style PDF
/// font. It also serves as a Text Box style choice for Song-style documents.
pub(crate) const NOTO_SERIF_SC_FAMILY: &str = "Noto Serif SC";
const NOTO_SERIF_SC_LABEL: &str = "思源宋体";
const NOTO_SERIF_SC_POSTSCRIPT: &str = "NotoSerifSC";

/// FangSong companion for edits to FangS/FangSong documents. Noto Sans SC and
/// Noto Serif SC both weigh far heavier than the hairline strokes of a
/// fangsong face, so a new character in a contract label stands out even when
/// the original characters are preserved. Zhuque Fangsong reproduces the
/// fangsong look and is offered as a style choice too.
pub(crate) const ZHUQUE_FANGSONG_FAMILY: &str = "Zhuque Fangsong";
const ZHUQUE_FANGSONG_LABEL: &str = "朱雀仿宋";
const ZHUQUE_FANGSONG_POSTSCRIPT: &str = "ZhuqueFangsong-Regular";

/// The remaining CJK style faces are free (OFL 1.1) counterparts of the
/// typefaces WPS and Word offer. The proprietary WPS/Windows faces (宋体,
/// 黑体, 楷体, 微软雅黑, 华文*, 方正*) cannot be bundled, so PDFX ships these
/// look-alikes instead; see `fonts/SOURCE.txt` for provenance.
const LXGW_WENKAI_LABEL: &str = "霞鹜文楷";
const LXGW_WENKAI_POSTSCRIPT: &str = "LXGWWenKai-Regular";
const SMILEY_SANS_LABEL: &str = "得意黑";
const SMILEY_SANS_POSTSCRIPT: &str = "SmileySans-Oblique";
const MA_SHAN_ZHENG_LABEL: &str = "马善政毛笔楷书";
const MA_SHAN_ZHENG_POSTSCRIPT: &str = "MaShanZheng-Regular";
const ZCOOL_XIAOWEI_LABEL: &str = "站酷小薇";
const ZCOOL_XIAOWEI_POSTSCRIPT: &str = "ZCOOLXiaoWei-Regular";
const ZCOOL_QINGKE_HUANGYOU_LABEL: &str = "站酷庆科黄油";
const ZCOOL_QINGKE_HUANGYOU_POSTSCRIPT: &str = "ZCOOLQingKeHuangYou-Regular";

const WEIGHT_REGULAR: u16 = 400;
const WEIGHT_BOLD: u16 = 700;

/// Noto Sans SC variable TrueType, Google Fonts commit
/// `ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b`. See `fonts/SOURCE.txt` for the
/// URL and SHA-256 and `fonts/NotoSansSC-OFL.txt` for the license.
static NOTO_SANS_SC: BundledFont = BundledFont {
    family: NOTO_SANS_SC_FAMILY,
    label: NOTO_SANS_SC_LABEL,
    postscript_name: NOTO_SANS_SC_POSTSCRIPT,
    bytes: include_bytes!("../fonts/NotoSansSC-VF.ttf"),
    has_weight_axis: true,
};

/// Noto Serif SC variable TrueType (`fonts/SOURCE.txt`, OFL 1.1). The PDFX
/// editor falls back to it when a Song/Ming original font cannot paint new
/// text, so an edit does not turn a Song document into a sans-serif one.
static NOTO_SERIF_SC: BundledFont = BundledFont {
    family: NOTO_SERIF_SC_FAMILY,
    label: NOTO_SERIF_SC_LABEL,
    postscript_name: NOTO_SERIF_SC_POSTSCRIPT,
    bytes: include_bytes!("../fonts/NotoSerifSC-VF.ttf"),
    has_weight_axis: true,
};

/// Zhuque Fangsong Regular, a static TrueType fangsong face (`fonts/SOURCE.txt`,
/// OFL 1.1). It has no weight axis, so bold text keeps the regular outlines.
static ZHUQUE_FANGSONG: BundledFont = BundledFont {
    family: ZHUQUE_FANGSONG_FAMILY,
    label: ZHUQUE_FANGSONG_LABEL,
    postscript_name: ZHUQUE_FANGSONG_POSTSCRIPT,
    bytes: include_bytes!("../fonts/ZhuqueFangsong-Regular.ttf"),
    has_weight_axis: false,
};

/// LXGW WenKai Regular, a Kai-style TrueType face (OFL 1.1).
static LXGW_WENKAI: BundledFont = BundledFont {
    family: "LXGW WenKai",
    label: LXGW_WENKAI_LABEL,
    postscript_name: LXGW_WENKAI_POSTSCRIPT,
    bytes: include_bytes!("../fonts/LXGWWenKai-Regular.ttf"),
    has_weight_axis: false,
};

/// Smiley Sans Oblique, a display sans TrueType face (OFL 1.1).
static SMILEY_SANS: BundledFont = BundledFont {
    family: "Smiley Sans",
    label: SMILEY_SANS_LABEL,
    postscript_name: SMILEY_SANS_POSTSCRIPT,
    bytes: include_bytes!("../fonts/SmileySans-Oblique.ttf"),
    has_weight_axis: false,
};

/// Ma Shan Zheng Regular, a brush-script TrueType face (OFL 1.1).
static MA_SHAN_ZHENG: BundledFont = BundledFont {
    family: "Ma Shan Zheng",
    label: MA_SHAN_ZHENG_LABEL,
    postscript_name: MA_SHAN_ZHENG_POSTSCRIPT,
    bytes: include_bytes!("../fonts/MaShanZheng-Regular.ttf"),
    has_weight_axis: false,
};

/// ZCOOL XiaoWei Regular, a Song-style display TrueType face (OFL 1.1).
static ZCOOL_XIAOWEI: BundledFont = BundledFont {
    family: "ZCOOL XiaoWei",
    label: ZCOOL_XIAOWEI_LABEL,
    postscript_name: ZCOOL_XIAOWEI_POSTSCRIPT,
    bytes: include_bytes!("../fonts/ZCOOLXiaoWei-Regular.ttf"),
    has_weight_axis: false,
};

/// ZCOOL QingKe HuangYou Regular, a display sans TrueType face (OFL 1.1).
static ZCOOL_QINGKE_HUANGYOU: BundledFont = BundledFont {
    family: "ZCOOL QingKe HuangYou",
    label: ZCOOL_QINGKE_HUANGYOU_LABEL,
    postscript_name: ZCOOL_QINGKE_HUANGYOU_POSTSCRIPT,
    bytes: include_bytes!("../fonts/ZCOOLQingKeHuangYou-Regular.ttf"),
    has_weight_axis: false,
};

/// A font program PDFX may embed, with the license recorded next to it.
#[derive(PartialEq, Eq)]
pub(crate) struct BundledFont {
    pub(crate) family: &'static str,
    pub(crate) label: &'static str,
    pub(crate) postscript_name: &'static str,
    bytes: &'static [u8],
    has_weight_axis: bool,
}

impl BundledFont {
    /// A parser over the bundled font program, for callers that need glyph
    /// advances and outlines rather than PDFium font handles.
    pub(crate) fn face(&self) -> Option<ttf_parser::Face<'static>> {
        let bytes: &'static [u8] = self.bytes;
        ttf_parser::Face::parse(bytes, 0).ok()
    }

    /// True when every character that will be written to a text object has a
    /// glyph. Line breaks never reach a text object (each line is one object);
    /// a tab falls back to the space glyph because CJK fonts rarely map the
    /// control character itself.
    pub(crate) fn missing_characters(&self, text: &str) -> Vec<char> {
        missing_characters_in(self.bytes, 0, text)
    }

    /// True when the font can render every character of `text`.
    pub(crate) fn covers(&self, text: &str) -> bool {
        self.missing_characters(text).is_empty()
    }

    /// Builds the subset that renders `text` at `weight`.
    ///
    /// `text` must already be covered; the caller reports missing characters
    /// before calling this so the error names them once.
    pub(crate) fn subset(&self, text: &str, weight: u16) -> Result<FontSubset, CommandError> {
        subset_program(
            self.bytes,
            0,
            self.family,
            self.has_weight_axis,
            text,
            weight,
        )
    }
}

/// Characters of `text` that `face_index` of the sfnt program has no glyph
/// for. A program that cannot be parsed reports every character as missing.
pub(crate) fn missing_characters_in(bytes: &[u8], face_index: u32, text: &str) -> Vec<char> {
    let Ok(face) = ttf_parser::Face::parse(bytes, face_index) else {
        return text
            .chars()
            .filter(|character| !matches!(character, '\n' | '\r'))
            .collect();
    };
    let mut missing = Vec::new();
    for character in text.chars() {
        if matches!(character, '\n' | '\r') || missing.contains(&character) {
            continue;
        }
        let covered = face.glyph_index(character).is_some()
            || (character == '\t' && face.glyph_index(' ').is_some());
        if !covered {
            missing.push(character);
        }
    }
    missing
}

/// Builds a subset of `face_index` of an sfnt program that renders `text` at
/// `weight`.
///
/// This is the embedding pipeline shared by the bundled and installed fonts:
/// CIDs are assigned in first-appearance order, the program is subset with
/// `subsetter`, a cmap is injected so coverage survives a save/reopen, and the
/// PostScript name gets a per-subset suffix so PDFium's BaseFont-keyed font
/// cache cannot confuse two subsets on one page.
pub(crate) fn subset_program(
    program: &[u8],
    face_index: u32,
    family: &str,
    has_weight_axis: bool,
    text: &str,
    weight: u16,
) -> Result<FontSubset, CommandError> {
    // Rebuild unsorted embedded sfnt directories before subsetter's binary
    // table lookup. Collections retain their original offsets and face index.
    let mut compatible_program = match table_records(program) {
        Some(records) if !records.windows(2).all(|w| w[0].tag <= w[1].tag) => {
            std::borrow::Cow::Owned(rebuild_font(program, &[]).unwrap_or_else(|| program.to_vec()))
        }
        _ => std::borrow::Cow::Borrowed(program),
    };
    if compatible_program.starts_with(b"true") {
        compatible_program.to_mut()[..4].copy_from_slice(&[0, 1, 0, 0]);
    }
    let program = compatible_program.as_ref();
    let face = ttf_parser::Face::parse(program, face_index).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("the font {family} could not be parsed: {error}"),
        )
    })?;

    // CID = 1-based first-appearance index of a distinct (glyph, character)
    // pair. Two characters that share a glyph (for example a compatibility
    // character) still need separate CIDs so ToUnicode can map each CID
    // back to its own character.
    let mut cid_by_key: BTreeMap<(u16, char), u16> = BTreeMap::new();
    let mut unicode_by_cid: BTreeMap<u16, char> = BTreeMap::new();
    let mut old_gid_by_cid: BTreeMap<u16, u16> = BTreeMap::new();
    let mut next_cid: u16 = 1;
    let mut space_assigned = false;

    let mut assign = |character: char, old_gid: u16| -> u16 {
        let cid = *cid_by_key.entry((old_gid, character)).or_insert_with(|| {
            let cid = next_cid;
            next_cid += 1;
            unicode_by_cid.insert(cid, character);
            old_gid_by_cid.insert(cid, old_gid);
            cid
        });
        cid
    };

    for character in text.chars() {
        if matches!(character, '\n' | '\r') {
            continue;
        }
        if character == ' ' {
            space_assigned = true;
        }
        let old_gid = match character {
            '\t' => face.glyph_index(' ').map(|glyph| glyph.0),
            _ => face.glyph_index(character).map(|glyph| glyph.0),
        }
        .ok_or_else(|| missing_glyph_error(family, &character.to_string()))?;
        assign(character, old_gid);
    }

    // PDFium cannot create an empty text object, but a style change on an
    // empty box still needs a token. A subset containing one space is
    // valid and tiny; adding it unconditionally also means a later edit
    // that only adds spaces never needs another font.
    if !space_assigned {
        if let Some(glyph) = face.glyph_index(' ') {
            assign(' ', glyph.0);
        }
    }

    let mut unique_gids: Vec<u16> = old_gid_by_cid.values().copied().collect();
    unique_gids.sort_unstable();
    unique_gids.dedup();

    // `subsetter` assigns new glyph IDs in remap order. Remapping the old
    // IDs in ascending order keeps the CIDToGIDMap deterministic while
    // staying non-identity wherever CID order differs from glyph order.
    let mut remapper = subsetter::GlyphRemapper::new();
    for old_gid in &unique_gids {
        remapper.remap(*old_gid);
    }
    let mut cid_to_gid = vec![0u16; usize::from(next_cid)];
    for (cid, old_gid) in &old_gid_by_cid {
        let new_gid = remapper.get(*old_gid).ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("glyph {old_gid} was not added to the subset"),
            )
        })?;
        cid_to_gid[usize::from(*cid)] = new_gid;
    }

    let variations: &[(subsetter::Tag, f32)] = if has_weight_axis {
        &[(subsetter::Tag::new(b"wght"), f32::from(weight))]
    } else {
        &[]
    };
    let bytes = subsetter::subset_with_variations(program, face_index, variations, &remapper)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not subset the font {family}: {error}"),
            )
        })?;

    // Re-add a cmap so the embedded program remains self-describing: a
    // reopened document can prove coverage with ttf-parser and reuse the
    // font instead of embedding another subset.
    let mut char_to_gid: BTreeMap<char, u16> = BTreeMap::new();
    for (cid, character) in &unicode_by_cid {
        if let Some(gid) = cid_to_gid.get(usize::from(*cid)) {
            char_to_gid.insert(*character, *gid);
        }
    }
    let bytes = inject_cmap(&bytes, &char_to_gid);
    // PDFium's content generator deduplicates fonts by BaseFont name, so
    // every subset needs its own name or the second subset on a page would
    // be rendered through the first subset's CID map.
    let tag = subset_tag(weight, &bytes, &cid_to_gid, &unicode_by_cid);
    let bytes = rename_font(&bytes, &tag);

    Ok(FontSubset {
        bytes,
        cid_to_gid,
        unicode_by_cid,
    })
}

/// The default bundled font for Text Boxes.
pub(crate) fn noto_sans_sc() -> &'static BundledFont {
    &NOTO_SANS_SC
}

/// The serif bundled font used when the original PDF font is a Song-style
/// face and cannot paint the replacement text.
pub(crate) fn noto_serif_sc() -> &'static BundledFont {
    &NOTO_SERIF_SC
}

/// The fangsong bundled font used when the original PDF font is a FangSong
/// face and cannot paint the replacement text.
pub(crate) fn zhuque_fangsong() -> &'static BundledFont {
    &ZHUQUE_FANGSONG
}

/// Every font PDFX bundles, in the order the style panel lists them.
///
/// All of them may be chosen for new text; the Noto and Zhuque faces are also
/// used as fallbacks when an original PDF font cannot paint a character.
static BUNDLED_FONTS: [&BundledFont; 8] = [
    &NOTO_SANS_SC,
    &NOTO_SERIF_SC,
    &ZHUQUE_FANGSONG,
    &LXGW_WENKAI,
    &SMILEY_SANS,
    &MA_SHAN_ZHENG,
    &ZCOOL_XIAOWEI,
    &ZCOOL_QINGKE_HUANGYOU,
];

pub(crate) fn bundled_fonts() -> &'static [&'static BundledFont] {
    &BUNDLED_FONTS
}

fn compact_family(family: &str) -> String {
    family
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// True when `family` names a bundled font, including the PostScript-ish names
/// PDFium reports after a save/reopen (`NotoSansSC-Thin`, `NotoSerifSC-Bold`,
/// `ZhuqueFangsong-Regular-...`).
pub(crate) fn is_bundled_family(family: &str) -> bool {
    bundled_family(family).is_some()
}

/// Resolves a bundled family name to its font program.
///
/// Matching is by compacted prefix so the PostScript-flavoured names PDFium
/// reports after a save/reopen (`NotoSansSC-Thin`, `ZhuqueFangsong-Regular-…`)
/// still resolve to the registry entry.
pub(crate) fn bundled_family(family: &str) -> Option<&'static BundledFont> {
    let compact = compact_family(family);
    if compact.is_empty() {
        return None;
    }
    bundled_fonts().iter().copied().find(|font| {
        compact.starts_with(&compact_family(font.family))
            || compact.starts_with(&compact_family(font.postscript_name))
    })
}

/// True when a font name looks like a FangSong face (FangS-SC, STFangsong,
/// FangSong_GB2312, 仿宋). The name must contain the full "fangsong" stem or
/// "FangS-SC"'s compact form, so 方正 (FangZheng) Song faces are not mistaken
/// for fangsong.
pub(crate) fn is_fangsong_family(family: &str) -> bool {
    if family.contains("仿宋") {
        return true;
    }
    let name = compact_family(family);
    name.contains("fangsong") || name.contains("fangssc")
}

/// True when a font name looks like a serif (Song, Ming, FangSong or Kai)
/// face. Word and WPS exports use names such as SimSun, FangS-SC and
/// HYShuSongErKW; Latin serif names are recognized as well.
pub(crate) fn is_serif_family(family: &str) -> bool {
    let name = compact_family(family);
    // "PingFang" and "Hei" are sans faces even though "fang" and "hei" also
    // appear inside serif names such as FangSong or "Hei Song"; check these
    // before the serif markers so PingFang SC never becomes a serif fallback.
    const SANS_MARKERS: [&str; 3] = ["sans", "pingfang", "hei"];
    if SANS_MARKERS.iter().any(|marker| name.contains(marker)) {
        return false;
    }
    const SERIF_MARKERS: [&str; 15] = [
        "song",
        "sun",
        "fangsong",
        "fang",
        "ming",
        "serif",
        "kaiti",
        "kai",
        "times",
        "georgia",
        "garamond",
        "cambria",
        "roman",
        "xiaowei",
        "shanzheng",
    ];
    SERIF_MARKERS.iter().any(|marker| name.contains(marker))
}

/// Normalizes the family name PDFium reports for an embedded subset back to
/// the registry's family name, so a reopened Text Box keeps selecting
/// "Noto Sans SC" (or the serif companion) in the style panel.
///
/// Installed fonts are resolved the same way: a saved subset of Microsoft
/// YaHei carries a suffixed PostScript name, and a reopened Text Box should
/// still show the family the user picked instead of the private subset name.
pub(crate) fn normalize_family(family: &str) -> String {
    if let Some(font) = bundled_family(family) {
        return font.family.to_owned();
    }
    crate::system_fonts::catalog()
        .family_for_name(family)
        .unwrap_or(family)
        .to_owned()
}

/// The weight a bundled-font style uses. Noto Sans SC is a variable font with
/// a `wght` axis, so bold text is a real instance rather than a synthetic
/// stroke.
pub(crate) fn weight_for(bold: bool) -> u16 {
    if bold {
        WEIGHT_BOLD
    } else {
        WEIGHT_REGULAR
    }
}

/// Reports the first few characters a bundled font cannot render.
pub(crate) fn missing_glyph_error(family: &str, text: &str) -> CommandError {
    missing_glyph_error_for("bundled font", family, text)
}

/// Reports the first few characters an installed font cannot render.
pub(crate) fn missing_system_glyph_error(family: &str, text: &str) -> CommandError {
    missing_glyph_error_for("installed font", family, text)
}

fn missing_glyph_error_for(kind: &str, family: &str, text: &str) -> CommandError {
    let missing: String = text
        .chars()
        .filter(|character| !matches!(character, '\n' | '\r'))
        .take(8)
        .collect();
    CommandError::new(
        CommandErrorCode::UnsupportedFont,
        format!(
            "the {kind} {family} has no glyphs for the replacement text \
             (unsupported characters: {missing:?})"
        ),
    )
}

/// A subset font program plus the CID mapping PDFium needs.
pub(crate) struct FontSubset {
    pub(crate) bytes: Vec<u8>,
    /// `cid_to_gid[cid]` is the subset glyph ID for PDF CID `cid`.
    pub(crate) cid_to_gid: Vec<u16>,
    pub(crate) unicode_by_cid: BTreeMap<u16, char>,
}

impl FontSubset {
    /// PDF permits omitting OS/2 and post, but browser font sanitizers do not.
    /// Supply metrics-only tables; outlines, cmap, and advances remain identical.
    pub(crate) fn web_font_bytes(&self) -> Result<Vec<u8>, CommandError> {
        let face = ttf_parser::Face::parse(&self.bytes, 0).map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not inspect editor font: {error:?}"),
            )
        })?;
        let units = face.units_per_em();
        let mut os2 = vec![0u8; 78];
        let mut write = |offset: usize, value: u16| {
            os2[offset..offset + 2].copy_from_slice(&value.to_be_bytes())
        };
        write(2, units / 2); // average width
        write(4, face.weight().to_number());
        write(6, 5); // normal width
        for offset in [10, 12, 18, 20] {
            write(offset, units / 2);
        }
        write(26, (units / 20).max(1));
        write(28, units / 3);
        write(62, 0x40); // regular
        write(
            64,
            self.unicode_by_cid
                .values()
                .map(|c| (*c as u32).min(65535) as u16)
                .min()
                .unwrap_or(32),
        );
        write(
            66,
            self.unicode_by_cid
                .values()
                .map(|c| (*c as u32).min(65535) as u16)
                .max()
                .unwrap_or(32),
        );
        write(68, face.ascender() as u16);
        write(70, face.descender() as u16);
        write(72, face.line_gap() as u16);
        write(74, face.global_bounding_box().y_max.max(0) as u16);
        write(
            76,
            face.global_bounding_box().y_min.saturating_neg().max(0) as u16,
        );
        os2[58..62].copy_from_slice(b"PDFX");
        let mut replacements = vec![(b"OS/2", os2)];
        if !table_records(&self.bytes)
            .is_some_and(|records| records.iter().any(|r| r.tag == *b"post"))
        {
            let mut post = vec![0u8; 32];
            post[..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
            post[8..10].copy_from_slice(&(-(units as i16) / 10).to_be_bytes());
            post[10..12].copy_from_slice(&(units / 20).to_be_bytes());
            replacements.push((b"post", post));
        }
        rebuild_font(&self.bytes, &replacements).ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                "could not prepare editor font tables",
            )
        })
    }

    /// The ToUnicode CMap PDFium writes into the PDF: every CID maps to the
    /// UTF-16BE encoding of the character it renders.
    pub(crate) fn to_unicode_cmap(&self) -> String {
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
        let entries: Vec<(&u16, &char)> = self.unicode_by_cid.iter().collect();
        for chunk in entries.chunks(100) {
            let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
            for (cid, character) in chunk {
                let mut buffer = [0u16; 2];
                let utf16: String = character
                    .encode_utf16(&mut buffer)
                    .iter()
                    .map(|unit| format!("{unit:04X}"))
                    .collect();
                let _ = writeln!(cmap, "<{cid:04X}> <{utf16}>");
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        cmap
    }

    /// The CIDToGIDMap stream: one big-endian glyph ID per CID, starting at 0.
    pub(crate) fn cid_to_gid_map(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.cid_to_gid.len() * 2);
        for gid in &self.cid_to_gid {
            data.extend_from_slice(&gid.to_be_bytes());
        }
        data
    }
}

/// Caches the PDFium font handles this session loaded for bundled subsets.
///
/// A handle is an opaque, non-owning pointer into the owning document's font
/// collection, so it is stored as an integer to keep the edit session `Send`
/// (the document registry is the synchronization point for PDFium access).
/// Every document has its own session, and the session is declared before the
/// document in `OpenDocument`, so a cached handle never outlives its document.
#[derive(Default)]
pub(crate) struct BundledFontCache {
    tokens: std::collections::HashMap<String, usize>,
}

impl BundledFontCache {
    /// Re-encodes an existing embedded program when its PDF space mapping
    /// disagrees with the program's metrics. Keep the original outlines, but
    /// build an explicit Unicode/CID/width map for every edited character.
    pub(crate) fn repaired_embedded_token(
        &mut self,
        document: &mut PdfDocument<'static>,
        program: &[u8],
        family: &str,
        text: &str,
    ) -> Result<PdfFontToken, CommandError> {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        program.hash(&mut hash);
        let key = Self::key(
            &format!("embedded:{:x}", hash.finish()),
            text,
            WEIGHT_REGULAR,
        );
        if let Some(handle) = self.tokens.get(&key) {
            return Ok(PdfFontToken::from_usize(*handle));
        }
        let subset = subset_program(program, 0, family, false, text, WEIGHT_REGULAR)?;
        let token = document
            .fonts_mut()
            .load_cid_type2_from_bytes(
                &subset.bytes,
                &subset.to_unicode_cmap(),
                &subset.cid_to_gid_map(),
            )
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not repair embedded font encoding: {error:?}"),
                )
            })?;
        self.tokens.insert(key, token.to_usize());
        Ok(token)
    }

    /// Returns a token for a subset of `font` covering `text`, loading it once
    /// per distinct family, character set and weight. Reusing the token means
    /// repeated edits cannot embed the same subset twice.
    pub(crate) fn token(
        &mut self,
        document: &mut PdfDocument<'static>,
        font: &BundledFont,
        text: &str,
        weight: u16,
    ) -> Result<PdfFontToken, CommandError> {
        let key = Self::key(font.family, text, weight);
        if let Some(handle) = self.tokens.get(&key) {
            return Ok(PdfFontToken::from_usize(*handle));
        }

        if !font.covers(text) {
            return Err(missing_glyph_error(font.family, text));
        }
        let subset = font.subset(text, weight)?;
        let cmap = subset.to_unicode_cmap();
        let gid_map = subset.cid_to_gid_map();
        let token = document
            .fonts_mut()
            .load_cid_type2_from_bytes(&subset.bytes, &cmap, &gid_map)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not load the bundled font subset: {error:?}"),
                )
            })?;
        self.tokens.insert(key, token.to_usize());
        Ok(token)
    }

    /// Returns a token for a subset of an installed font covering `text`,
    /// loading it once per path, face, character set and weight. The subset is
    /// embedded in the edited document; the installed font itself is only
    /// read, never copied or modified.
    pub(crate) fn system_token(
        &mut self,
        document: &mut PdfDocument<'static>,
        font: &crate::system_fonts::SystemFont,
        text: &str,
        weight: u16,
    ) -> Result<PdfFontToken, CommandError> {
        let key = Self::system_key(font, text, weight);
        if let Some(handle) = self.tokens.get(&key) {
            return Ok(PdfFontToken::from_usize(*handle));
        }

        let subset = font.subset(text, weight)?;
        let cmap = subset.to_unicode_cmap();
        let gid_map = subset.cid_to_gid_map();
        let token = document
            .fonts_mut()
            .load_cid_type2_from_bytes(&subset.bytes, &cmap, &gid_map)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not load the installed font subset: {error:?}"),
                )
            })?;
        self.tokens.insert(key, token.to_usize());
        Ok(token)
    }

    fn key(family: &str, text: &str, weight: u16) -> String {
        let unique: BTreeSet<char> = text
            .chars()
            .filter(|character| !matches!(character, '\n' | '\r'))
            .collect();
        let mut key = format!("{family}:{weight}:");
        key.extend(unique);
        key
    }

    fn system_key(font: &crate::system_fonts::SystemFont, text: &str, weight: u16) -> String {
        let unique: BTreeSet<char> = text
            .chars()
            .filter(|character| !matches!(character, '\n' | '\r'))
            .collect();
        let mut key = format!(
            "system\0{}\0{}\0{weight}\0",
            font.path.display(),
            font.face_index
        );
        key.extend(unique);
        key
    }
}

/// Injects a `cmap` table into a subset font program.
///
/// `subsetter` drops the original `cmap`; without one the embedded program is
/// less portable and PDFX cannot check coverage after reopening the document.
/// Both a Windows BMP format 4 subtable and a Windows UCS-4 format 12 subtable
/// are written so supplementary-plane characters stay reachable too.
fn inject_cmap(font: &[u8], mapping: &BTreeMap<char, u16>) -> Vec<u8> {
    if mapping.is_empty() {
        return font.to_vec();
    }
    let Some(records) = table_records(font) else {
        return font.to_vec();
    };
    if records.iter().any(|record| record.tag == *b"cmap") {
        return font.to_vec();
    }
    let cmap = build_cmap(mapping);
    rebuild_font(font, &[(b"cmap", cmap)]).unwrap_or_else(|| font.to_vec())
}

/// One table directory entry of an sfnt program.
struct TableRecord {
    tag: [u8; 4],
    /// Read by the checksum regression test; the rewriter computes fresh
    /// checksums for the tables it writes.
    #[allow(dead_code)]
    checksum: u32,
    offset: usize,
    length: usize,
}

/// The table directory of an sfnt program.
fn table_records(font: &[u8]) -> Option<Vec<TableRecord>> {
    if font.len() < 12
        || !matches!(
            &font[..4],
            b"\x00\x01\x00\x00" | b"OTTO" | b"true" | b"typ1"
        )
    {
        // A TTC has a collection directory, not an sfnt table directory.
        // Interpreting its version/face offsets as tables corrupts installed
        // Windows collections such as SimSun before face_index is applied.
        return None;
    }
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    if font.len() < 12 + num_tables * 16 {
        return None;
    }
    let mut records = Vec::with_capacity(num_tables);
    for index in 0..num_tables {
        let base = 12 + index * 16;
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&font[base..base + 4]);
        let checksum = u32::from_be_bytes(font[base + 4..base + 8].try_into().expect("4 bytes"));
        let offset =
            u32::from_be_bytes(font[base + 8..base + 12].try_into().expect("4 bytes")) as usize;
        let length =
            u32::from_be_bytes(font[base + 12..base + 16].try_into().expect("4 bytes")) as usize;
        if offset.checked_add(length)? > font.len() {
            return None;
        }
        records.push(TableRecord {
            tag,
            checksum,
            offset,
            length,
        });
    }
    Some(records)
}

/// Rewrites an sfnt program with the given tables replaced or added.
///
/// Existing table data is copied byte for byte; the directory checksums and
/// `head.checkSumAdjustment` are recomputed so the program stays internally
/// consistent. Returns `None` if the input is not a well-formed sfnt program.
fn rebuild_font(font: &[u8], replacements: &[(&[u8; 4], Vec<u8>)]) -> Option<Vec<u8>> {
    let records = table_records(font)?;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = records
        .into_iter()
        .map(|record| {
            (
                record.tag,
                font[record.offset..record.offset + record.length].to_vec(),
            )
        })
        .collect();
    for (tag, data) in replacements {
        match tables.iter_mut().find(|(existing, _)| existing == *tag) {
            Some((_, slot)) => *slot = data.clone(),
            None => tables.push((**tag, data.clone())),
        }
    }
    tables.sort_by_key(|table| table.0);

    let count = tables.len();
    let count_u16 = u16::try_from(count).ok()?;
    let power = (count as f64).log2().floor() as u16;
    let search_range = 16 * (1u16 << power);
    let header_len = 12 + 16 * count;

    let mut out = Vec::new();
    out.extend_from_slice(&font[0..4]);
    out.extend_from_slice(&count_u16.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&power.to_be_bytes());
    out.extend_from_slice(&(16 * count_u16 - search_range).to_be_bytes());

    let mut layout: Vec<(&[u8; 4], usize, usize, u32)> = Vec::with_capacity(count);
    let mut data = Vec::new();
    for (tag, table) in &tables {
        let start = header_len + data.len();
        let checksum = if tag == b"head" && table.len() >= 12 {
            // The head directory checksum is defined with this field zeroed.
            let mut head = table.clone();
            head[8..12].fill(0);
            table_checksum(&head)
        } else {
            table_checksum(table)
        };
        data.extend_from_slice(table);
        layout.push((tag, start, table.len(), checksum));
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    let mut head_offset = None;
    for (tag, offset, length, checksum) in &layout {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&(*offset as u32).to_be_bytes());
        out.extend_from_slice(&(*length as u32).to_be_bytes());
        if *tag == b"head" {
            head_offset = Some(*offset);
        }
    }
    out.extend_from_slice(&data);

    // Changing a table changes the whole-font checksum, so update
    // head.checkSumAdjustment (the directory checksum for `head` is defined
    // with that field zeroed, so it stays valid).
    if let Some(head) = head_offset {
        if head + 12 <= out.len() {
            out[head + 8..head + 12].fill(0);
            let adjustment = 0xB1B0_AFBAu32.wrapping_sub(table_checksum(&out));
            out[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
        }
    }
    Some(out)
}

/// Appends a per-subset suffix to the font's PostScript name.
///
/// PDFium's page content generator deduplicates fonts by `(BaseFont, type)`,
/// not by font handle (`CPDF_PageObjectHolder::FontData`). Every subset PDFX
/// embeds from Noto Sans SC would otherwise share `/BaseFont
/// (NotoSansSC-Thin)`, so the second subset on a page would be written with the
/// first subset's resource name and its characters would be read through the
/// wrong CID map. A unique PostScript name keeps each subset its own resource.
fn rename_font(font: &[u8], suffix: &str) -> Vec<u8> {
    let Some(records) = table_records(font) else {
        return font.to_vec();
    };
    let Some(record) = records.iter().find(|record| record.tag == *b"name") else {
        return font.to_vec();
    };
    let Some(table) =
        rewrite_name_table(&font[record.offset..record.offset + record.length], suffix)
    else {
        return font.to_vec();
    };
    rebuild_font(font, &[(b"name", table)]).unwrap_or_else(|| font.to_vec())
}

/// Rewrites every PostScript-name (name ID 6) record with `suffix` appended.
fn rewrite_name_table(table: &[u8], suffix: &str) -> Option<Vec<u8>> {
    if table.len() < 6 {
        return None;
    }
    let count = u16::from_be_bytes([table[2], table[3]]) as usize;
    let storage = u16::from_be_bytes([table[4], table[5]]) as usize;
    if 6 + count.checked_mul(12)? > table.len() || storage > table.len() {
        return None;
    }

    struct Record {
        platform: u16,
        encoding: u16,
        language: u16,
        name_id: u16,
        text: Vec<u8>,
    }
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let base = 6 + index * 12;
        let platform = u16::from_be_bytes([table[base], table[base + 1]]);
        let encoding = u16::from_be_bytes([table[base + 2], table[base + 3]]);
        let language = u16::from_be_bytes([table[base + 4], table[base + 5]]);
        let name_id = u16::from_be_bytes([table[base + 6], table[base + 7]]);
        let length = u16::from_be_bytes([table[base + 8], table[base + 9]]) as usize;
        let offset = u16::from_be_bytes([table[base + 10], table[base + 11]]) as usize;
        let start = storage.checked_add(offset)?;
        let text = table.get(start..start.checked_add(length)?)?.to_vec();
        let text = if name_id == 6 {
            append_name_suffix(&text, platform, suffix)?
        } else {
            text
        };
        records.push(Record {
            platform,
            encoding,
            language,
            name_id,
            text,
        });
    }

    let string_offset = 6 + records.len() * 12;
    let mut directory = Vec::with_capacity(records.len() * 12);
    let mut strings: Vec<u8> = Vec::new();
    let mut offsets: BTreeMap<Vec<u8>, u16> = BTreeMap::new();
    for record in &records {
        let offset = match offsets.get(&record.text) {
            Some(offset) => *offset,
            None => {
                let offset = u16::try_from(strings.len()).ok()?;
                strings.extend_from_slice(&record.text);
                offsets.insert(record.text.clone(), offset);
                offset
            }
        };
        let length = u16::try_from(record.text.len()).ok()?;
        directory.extend_from_slice(&record.platform.to_be_bytes());
        directory.extend_from_slice(&record.encoding.to_be_bytes());
        directory.extend_from_slice(&record.language.to_be_bytes());
        directory.extend_from_slice(&record.name_id.to_be_bytes());
        directory.extend_from_slice(&length.to_be_bytes());
        directory.extend_from_slice(&offset.to_be_bytes());
    }

    let mut out = Vec::with_capacity(string_offset + strings.len());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&u16::try_from(records.len()).ok()?.to_be_bytes());
    out.extend_from_slice(&u16::try_from(string_offset).ok()?.to_be_bytes());
    out.extend_from_slice(&directory);
    out.extend_from_slice(&strings);
    Some(out)
}

/// Returns `text` with `-{suffix}` appended in the record's own encoding.
fn append_name_suffix(text: &[u8], platform: u16, suffix: &str) -> Option<Vec<u8>> {
    let suffix = format!("-{suffix}");
    if platform == 0 || platform == 3 {
        if !text.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = text
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        let mut name = String::from_utf16(&units).ok()?;
        name.push_str(&suffix);
        Some(
            name.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<u8>>(),
        )
    } else {
        let mut name = text.to_vec();
        name.extend_from_slice(suffix.as_bytes());
        Some(name)
    }
}

/// A stable tag that differs whenever the subset's font data or CID mapping
/// differs, so distinct subsets never share a BaseFont name.
fn subset_tag(
    weight: u16,
    font: &[u8],
    cid_to_gid: &[u16],
    unicode_by_cid: &BTreeMap<u16, char>,
) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&weight.to_be_bytes());
    eat(font);
    for gid in cid_to_gid {
        eat(&gid.to_be_bytes());
    }
    for (cid, character) in unicode_by_cid {
        eat(&cid.to_be_bytes());
        eat(&u32::from(*character).to_be_bytes());
    }
    format!("{hash:016x}")
}

fn build_cmap(mapping: &BTreeMap<char, u16>) -> Vec<u8> {
    let mut bmp: Vec<(u16, u16)> = mapping
        .iter()
        .filter(|(character, _)| u32::from(**character) <= 0xFFFF)
        .map(|(character, gid)| (*character as u16, *gid))
        .collect();
    bmp.sort_unstable();

    let seg_count = bmp.len() + 1;
    let length = 16 + seg_count * 8;
    let mut format4 = Vec::with_capacity(length);
    format4.extend_from_slice(&4u16.to_be_bytes());
    format4.extend_from_slice(&(length as u16).to_be_bytes());
    format4.extend_from_slice(&0u16.to_be_bytes());
    let seg_count_x2 = (seg_count * 2) as u16;
    format4.extend_from_slice(&seg_count_x2.to_be_bytes());
    let power = (seg_count as f64).log2().floor() as u16;
    let search_range = 2 * (1u16 << power);
    format4.extend_from_slice(&search_range.to_be_bytes());
    format4.extend_from_slice(&power.to_be_bytes());
    format4.extend_from_slice(&(seg_count_x2 - search_range).to_be_bytes());
    for (code, _) in &bmp {
        format4.extend_from_slice(&code.to_be_bytes());
    }
    format4.extend_from_slice(&0xFFFFu16.to_be_bytes());
    format4.extend_from_slice(&0u16.to_be_bytes());
    for (code, _) in &bmp {
        format4.extend_from_slice(&code.to_be_bytes());
    }
    format4.extend_from_slice(&0xFFFFu16.to_be_bytes());
    for (code, gid) in &bmp {
        format4.extend_from_slice(&gid.wrapping_sub(*code).to_be_bytes());
    }
    format4.extend_from_slice(&1u16.to_be_bytes());
    for _ in 0..seg_count {
        format4.extend_from_slice(&0u16.to_be_bytes());
    }

    let mut entries: Vec<(u32, u32)> = mapping
        .iter()
        .map(|(character, gid)| (u32::from(*character), u32::from(*gid)))
        .collect();
    entries.sort_unstable();
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (code, gid) in entries {
        if let Some((start_code, end_code, start_gid)) = groups.last_mut() {
            if end_code.checked_add(1) == Some(code) && *start_gid + (code - *start_code) == gid {
                *end_code = code;
                continue;
            }
        }
        groups.push((code, code, gid));
    }
    let mut format12 = Vec::with_capacity(16 + groups.len() * 12);
    format12.extend_from_slice(&12u16.to_be_bytes());
    format12.extend_from_slice(&0u16.to_be_bytes());
    format12.extend_from_slice(&((16 + groups.len() * 12) as u32).to_be_bytes());
    format12.extend_from_slice(&0u32.to_be_bytes());
    format12.extend_from_slice(&(groups.len() as u32).to_be_bytes());
    for (start_code, end_code, start_gid) in &groups {
        format12.extend_from_slice(&start_code.to_be_bytes());
        format12.extend_from_slice(&end_code.to_be_bytes());
        format12.extend_from_slice(&start_gid.to_be_bytes());
    }

    let mut cmap = Vec::with_capacity(20 + format4.len() + format12.len());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    cmap.extend_from_slice(&2u16.to_be_bytes());
    cmap.extend_from_slice(&3u16.to_be_bytes());
    cmap.extend_from_slice(&1u16.to_be_bytes());
    cmap.extend_from_slice(&20u32.to_be_bytes());
    cmap.extend_from_slice(&3u16.to_be_bytes());
    cmap.extend_from_slice(&10u16.to_be_bytes());
    cmap.extend_from_slice(&((20 + format4.len()) as u32).to_be_bytes());
    cmap.extend_from_slice(&format4);
    cmap.extend_from_slice(&format12);
    cmap
}

fn table_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_ttc_is_not_rewritten_as_a_single_sfnt() {
        let source = include_bytes!("../tests/fonts/LiberationSans-Regular.ttf");
        let mut face = source.to_vec();
        let count = u16::from_be_bytes([face[4], face[5]]) as usize;
        for index in 0..count {
            let base = 12 + index * 16 + 8;
            let offset = u32::from_be_bytes(face[base..base + 4].try_into().unwrap());
            face[base..base + 4].copy_from_slice(&(offset + 32).to_be_bytes());
        }
        let mut collection = b"ttcf".to_vec();
        for value in [0x0002_0000u32, 2, 32, 32, 0, 0, 0] {
            collection.extend_from_slice(&value.to_be_bytes());
        }
        collection.extend_from_slice(&face);
        // Large collections made the old sfnt directory parser accidentally
        // accept TTC offsets as table records, then corrupt the font.
        collection.resize(8 * 1024 * 1024, 0);
        assert!(ttf_parser::Face::parse(&collection, 1).is_ok());
        assert!(table_records(&collection).is_none());
        let subset = subset_program(&collection, 1, "Collection", false, "PDFX", 400)
            .expect("a selected collection face can be subset without rewriting its header");
        assert!(missing_characters_in(&subset.bytes, 0, "PDFX").is_empty());
    }

    #[test]
    fn serif_detection_keeps_hei_and_pingfang_faces_sans() {
        assert!(is_serif_family("SimSun"));
        assert!(is_serif_family("FangS-SC"));
        assert!(is_serif_family("HYShuSongErKW"));
        assert!(is_serif_family("Times New Roman"));
        assert!(!is_serif_family("Microsoft YaHei"));
        assert!(!is_serif_family("SimHei"));
        assert!(
            !is_serif_family("PingFang SC"),
            "PingFang is sans even though the name contains \"fang\""
        );
        assert!(!is_serif_family("Noto Sans SC"));
    }

    #[test]
    fn fangsong_detection_keeps_sans_and_song_faces_out() {
        assert!(is_fangsong_family("FangS-SC"));
        assert!(is_fangsong_family("PFTTWF+FangS-SC"));
        assert!(is_fangsong_family("STFangsong"));
        assert!(is_fangsong_family("FangSong_GB2312"));
        assert!(is_fangsong_family("仿宋_GB2312"));
        assert!(is_fangsong_family("Zhuque Fangsong"));
        assert!(!is_fangsong_family("PingFang SC"));
        assert!(!is_fangsong_family("SimSun"));
        assert!(!is_fangsong_family("Songti SC"));
        assert!(!is_fangsong_family("Microsoft YaHei"));
        assert!(!is_fangsong_family("FZLanTingHeiS-R-GB"));
    }

    #[test]
    fn reopened_zhuque_subsets_resolve_to_the_bundled_family() {
        let font = bundled_family("ZhuqueFangsong-Regular-0123456789abcdef")
            .expect("a reopened Zhuque subset must resolve");
        assert_eq!(font.family, "Zhuque Fangsong");
        assert_eq!(font.postscript_name, "ZhuqueFangsong-Regular");
        assert_eq!(
            normalize_family("ZhuqueFangsong-Regular-0123456789abcdef"),
            "Zhuque Fangsong"
        );
    }

    #[test]
    fn every_bundled_font_resolves_from_its_family_and_postscript_name() {
        assert_eq!(bundled_fonts().len(), 8);
        for font in bundled_fonts() {
            let from_family = bundled_family(font.family)
                .unwrap_or_else(|| panic!("{} must resolve by family", font.family));
            assert_eq!(from_family.family, font.family);
            let from_postscript = bundled_family(font.postscript_name).unwrap_or_else(|| {
                panic!("{} must resolve by PostScript name", font.postscript_name)
            });
            assert_eq!(from_postscript.family, font.family);
            assert!(!font.label.is_empty());
        }
        assert_eq!(
            normalize_family("LXGWWenKai-Regular-0123456789abcdef"),
            "LXGW WenKai"
        );
        assert_eq!(
            normalize_family("ZCOOLQingKeHuangYou-Regular-0123456789abcdef"),
            "ZCOOL QingKe HuangYou"
        );
    }

    #[test]
    fn bundled_display_faces_keep_their_closest_serif_or_sans_fallback() {
        assert!(is_serif_family("LXGW WenKai"));
        assert!(is_serif_family("ZCOOL XiaoWei"));
        assert!(is_serif_family("Ma Shan Zheng"));
        assert!(!is_serif_family("ZCOOL QingKe HuangYou"));
        assert!(!is_serif_family("Smiley Sans"));
    }

    #[test]
    fn new_bundled_subsets_cover_their_own_text() {
        for font in bundled_fonts() {
            let subset = font
                .subset("编辑文本 PDFX", WEIGHT_REGULAR)
                .unwrap_or_else(|error| panic!("{} must subset: {error:?}", font.family));
            let face = ttf_parser::Face::parse(&subset.bytes, 0).expect("subset parses");
            for character in "编辑文本".chars() {
                assert!(
                    face.glyph_index(character).is_some(),
                    "{} subset must cover {character:?}",
                    font.family
                );
            }
        }
    }

    fn postscript_name(bytes: &[u8]) -> String {
        let face = ttf_parser::Face::parse(bytes, 0).expect("subset parses");
        face.names()
            .into_iter()
            .find(|name| name.name_id == 6 && name.is_unicode())
            .and_then(|name| name.to_string())
            .expect("subset has a Unicode PostScript name")
    }

    #[test]
    fn bundled_subsets_get_unique_stable_postscript_names() {
        let font = noto_sans_sc();
        let first = font.subset("中文", WEIGHT_REGULAR).expect("subset first");
        let second = font.subset("文字", WEIGHT_REGULAR).expect("subset second");
        let first_name = postscript_name(&first.bytes);
        let second_name = postscript_name(&second.bytes);
        assert!(
            first_name.starts_with("NotoSansSC-Thin-"),
            "name keeps the stem: {first_name}"
        );
        assert_ne!(
            first_name, second_name,
            "distinct subsets must not share a BaseFont"
        );
        assert_eq!(
            first_name,
            postscript_name(&font.subset("中文", WEIGHT_REGULAR).expect("repeat").bytes),
            "the same subset always gets the same name"
        );
    }

    #[test]
    fn zhuque_subsets_keep_stable_unique_names_and_coverage() {
        let font = zhuque_fangsong();
        let subset = font.subset("实操人啊", WEIGHT_REGULAR).expect("subset");
        let name = postscript_name(&subset.bytes);
        assert!(
            name.starts_with("ZhuqueFangsong-Regular-"),
            "name keeps the stem: {name}"
        );
        assert_eq!(
            name,
            postscript_name(
                &font
                    .subset("实操人啊", WEIGHT_REGULAR)
                    .expect("repeat")
                    .bytes
            ),
            "the same subset always gets the same name"
        );
        assert_ne!(
            name,
            postscript_name(&font.subset("实操人", WEIGHT_REGULAR).expect("other").bytes),
            "distinct subsets must not share a BaseFont"
        );
        let face = ttf_parser::Face::parse(&subset.bytes, 0).expect("subset parses");
        for character in "实操人啊".chars() {
            assert!(
                face.glyph_index(character).is_some(),
                "the injected cmap must cover {character:?}"
            );
        }
        // The font has no weight axis; bold text still subsets the regular
        // outlines instead of failing.
        font.subset("实操人", WEIGHT_BOLD)
            .expect("a static font subsets at any requested weight");
    }

    #[test]
    fn renamed_subsets_keep_a_valid_sfnt_checksum() {
        let subset = noto_sans_sc()
            .subset("中文", WEIGHT_REGULAR)
            .expect("subset");
        let records = table_records(&subset.bytes).expect("subset is an sfnt program");
        for record in records {
            let data = &subset.bytes[record.offset..record.offset + record.length];
            let computed = if record.tag == *b"head" {
                let mut head = data.to_vec();
                head[8..12].fill(0);
                table_checksum(&head)
            } else {
                table_checksum(data)
            };
            assert_eq!(
                computed, record.checksum,
                "checksum for table {:?}",
                record.tag
            );
        }
        assert_eq!(
            table_checksum(&subset.bytes),
            0xB1B0_AFBA,
            "whole-font checksum matches head.checkSumAdjustment"
        );
    }
}
