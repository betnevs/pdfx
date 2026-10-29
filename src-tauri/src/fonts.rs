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
const NOTO_SANS_SC_LABEL: &str = "Noto Sans SC";
const NOTO_SANS_SC_POSTSCRIPT: &str = "NotoSansSC";

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

/// A font program PDFX may embed, with the license recorded next to it.
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
        let Ok(face) = ttf_parser::Face::parse(self.bytes, 0) else {
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

    /// True when the font can render every character of `text`.
    pub(crate) fn covers(&self, text: &str) -> bool {
        self.missing_characters(text).is_empty()
    }

    /// Builds the subset that renders `text` at `weight`.
    ///
    /// `text` must already be covered; the caller reports missing characters
    /// before calling this so the error names them once.
    pub(crate) fn subset(&self, text: &str, weight: u16) -> Result<FontSubset, CommandError> {
        let face = ttf_parser::Face::parse(self.bytes, 0).map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!(
                    "the bundled font {} could not be parsed: {error}",
                    self.family
                ),
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
            .ok_or_else(|| missing_glyph_error(self.family, &character.to_string()))?;
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

        let variations: &[(subsetter::Tag, f32)] = if self.has_weight_axis {
            &[(subsetter::Tag::new(b"wght"), f32::from(weight))]
        } else {
            &[]
        };
        let bytes = subsetter::subset_with_variations(self.bytes, 0, variations, &remapper)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not subset the bundled font {}: {error}", self.family),
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
}

/// The one bundled font the Text Box editor may embed.
pub(crate) fn noto_sans_sc() -> &'static BundledFont {
    &NOTO_SANS_SC
}

/// True when `family` names a bundled font, including the PostScript-ish names
/// PDFium reports after a save/reopen (`NotoSansSC-Thin`, `NotoSansSC-Bold`).
pub(crate) fn is_bundled_family(family: &str) -> bool {
    let compact: String = family
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect();
    compact.starts_with("notosanssc")
}

/// Normalizes the family name PDFium reports for an embedded subset back to
/// the registry's family name, so a reopened Text Box keeps selecting
/// "Noto Sans SC" in the style panel.
pub(crate) fn normalize_family(family: &str) -> String {
    if is_bundled_family(family) {
        NOTO_SANS_SC_FAMILY.to_owned()
    } else {
        family.to_owned()
    }
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
    let missing: String = text
        .chars()
        .filter(|character| !matches!(character, '\n' | '\r'))
        .take(8)
        .collect();
    CommandError::new(
        CommandErrorCode::UnsupportedFont,
        format!(
            "the bundled font {family} has no glyphs for the replacement text \
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
    /// Returns a token for a subset covering `text`, loading it once per
    /// distinct character set and weight. Reusing the token means repeated
    /// edits cannot embed the same subset twice.
    pub(crate) fn token(
        &mut self,
        document: &mut PdfDocument<'static>,
        text: &str,
        weight: u16,
    ) -> Result<PdfFontToken, CommandError> {
        let key = Self::key(text, weight);
        if let Some(handle) = self.tokens.get(&key) {
            return Ok(PdfFontToken::from_usize(*handle));
        }

        let font = noto_sans_sc();
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

    fn key(text: &str, weight: u16) -> String {
        let unique: BTreeSet<char> = text
            .chars()
            .filter(|character| !matches!(character, '\n' | '\r'))
            .collect();
        let mut key = format!("{NOTO_SANS_SC_FAMILY}:{weight}:");
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
    if font.len() < 12 {
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
