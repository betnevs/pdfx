//! Fonts installed on this machine.
//!
//! PDFX may not redistribute the proprietary typefaces a document names
//! (Microsoft YaHei, 宋体, the 汉仪 faces WPS Office bundles), but the user's
//! own machine usually has them already. This module catalogs the embeddable
//! TrueType programs in the OS font directories and in WPS Office's font
//! folders, and the Text Box editor reads them from their original location
//! when it needs to embed a subset. Nothing is copied into the repository,
//! the installer or the application bundle.
//!
//! Only programs that permit embedding are cataloged: PDFX writes a
//! `CIDFontType2`/`FontFile2` font, so a face needs TrueType `glyf` outlines
//! and an `fsType` that allows subsetting. CFF/OpenType faces (PingFang,
//! many Adobe fonts) are skipped until a `CIDFontType0` path exists.
//!
//! Scanning is header-only. A font collection can contain dozens of faces and
//! be tens of megabytes wide, so only the table directory, `name`, `OS/2` and
//! `fvar` slices are read while cataloging; the program itself is read when a
//! subset is actually needed.

use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::commands::{CommandError, CommandErrorCode};
use crate::fonts::{self, FontSubset};

/// Name-table slices larger than this are treated as corrupt. Real name
/// tables are a few kilobytes even for CJK collections.
const MAX_NAME_TABLE: usize = 4 * 1024 * 1024;
const MAX_FACES_PER_FILE: usize = 64;
const MAX_TABLES: usize = 512;
const MAX_FVAR_TABLE: usize = 4096;

/// One embeddable face of an installed font.
#[derive(Debug, PartialEq)]
pub(crate) struct SystemFont {
    pub(crate) family: String,
    /// Localized (preferably Simplified Chinese) family name for the panel.
    pub(crate) label: String,
    pub(crate) postscript_name: String,
    pub(crate) path: PathBuf,
    pub(crate) face_index: u32,
    /// True for a bold face (name or weight class); `weight` is authoritative
    /// when both regular and bold faces are installed.
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) weight: u16,
    /// True when the face has a `wght` variation axis; bold is then a real
    /// instance rather than a separate face.
    variable: bool,
    /// `wps` for fonts shipped by WPS Office, `system` for OS/user fonts.
    pub(crate) source: &'static str,
}

impl SystemFont {
    fn read_program(&self) -> Result<Vec<u8>, CommandError> {
        fs::read(&self.path).map_err(|error| {
            CommandError::new(
                CommandErrorCode::FileReadFailed,
                format!(
                    "the installed font {} could not be read from {}: {error}",
                    self.family,
                    self.path.display()
                ),
            )
        })
    }

    /// Characters of `text` this face has no glyph for. An unreadable or
    /// unparsable program reports every character as missing.
    pub(crate) fn missing_characters(&self, text: &str) -> Vec<char> {
        match self.read_program() {
            Ok(program) => fonts::missing_characters_in(&program, self.face_index, text),
            Err(_) => text
                .chars()
                .filter(|character| !matches!(character, '\n' | '\r'))
                .collect(),
        }
    }

    /// True when the face can render every character of `text`.
    pub(crate) fn covers(&self, text: &str) -> bool {
        self.missing_characters(text).is_empty()
    }

    /// Builds the subset embedded in the document for `text`.
    pub(crate) fn subset(&self, text: &str, weight: u16) -> Result<FontSubset, CommandError> {
        let program = self.read_program()?;
        if !fonts::missing_characters_in(&program, self.face_index, text).is_empty() {
            return Err(fonts::missing_system_glyph_error(&self.family, text));
        }
        fonts::subset_program(
            &program,
            self.face_index,
            &self.family,
            self.variable,
            text,
            weight,
        )
    }
}

/// One family offered in the style panel, with every installed face.
pub(crate) struct FamilyEntry {
    pub(crate) family: String,
    pub(crate) label: String,
    pub(crate) postscript_name: String,
    /// True when a bold face (or a weight axis) is installed.
    pub(crate) bold: bool,
    /// True when an italic face is installed.
    pub(crate) italic: bool,
    pub(crate) source: &'static str,
    faces: Vec<SystemFont>,
}

/// The installed fonts, grouped by family and sorted for the style panel.
pub(crate) struct SystemFontCatalog {
    families: Vec<FamilyEntry>,
    /// Compacted family name to index in `families`.
    by_family: HashMap<String, usize>,
    /// Compacted family and PostScript aliases, longest first, so a PDF font
    /// name or a saved subset name resolves to its family.
    aliases: Vec<(String, usize)>,
}

impl SystemFontCatalog {
    pub(crate) fn families(&self) -> impl Iterator<Item = &FamilyEntry> {
        self.families.iter()
    }

    /// Resolves a family name, PostScript name or saved subset name to the
    /// catalog's canonical family name.
    pub(crate) fn family_for_name(&self, name: &str) -> Option<&str> {
        self.family_index(name)
            .map(|index| self.families[index].family.as_str())
    }

    /// True when the panel's canonical family name is installed. Unlike
    /// [`Self::family_for_name`], this does not prefix-match: "Times" must
    /// stay the base-14 face even when "Times New Roman" is installed.
    pub(crate) fn has_family(&self, name: &str) -> bool {
        self.exact_family_index(name).is_some()
    }

    /// True when the family has an italic face.
    pub(crate) fn has_italic(&self, name: &str) -> bool {
        self.exact_family_index(name)
            .is_some_and(|index| self.families[index].italic)
    }

    /// The installed face to use for a style.
    ///
    /// Italic requires a real italic face. Bold picks the heaviest upright
    /// face (or the variable face, which is instantiated at the requested
    /// weight); a family without a bold face falls back to its regular face,
    /// matching the bundled fonts' static-bold behaviour.
    pub(crate) fn face_for_style(
        &self,
        family: &str,
        bold: bool,
        italic: bool,
    ) -> Option<&SystemFont> {
        let entry = &self.families[self.exact_family_index(family)?];
        let pool: Vec<&SystemFont> = entry
            .faces
            .iter()
            .filter(|face| face.italic == italic)
            .collect();
        if pool.is_empty() {
            return None;
        }
        if bold {
            pool.into_iter().max_by_key(|face| (face.bold, face.weight))
        } else {
            pool.into_iter()
                .min_by_key(|face| (face.weight.abs_diff(400), face.bold))
        }
    }

    fn exact_family_index(&self, name: &str) -> Option<usize> {
        let compact = compact_name(strip_subset_prefix(name.trim()));
        if compact.is_empty() {
            return None;
        }
        self.by_family.get(&compact).copied()
    }

    fn family_index(&self, name: &str) -> Option<usize> {
        let name = strip_subset_prefix(name.trim());
        if name.is_empty() {
            return None;
        }
        let compact = compact_name(name);
        if compact.is_empty() {
            return None;
        }
        if let Some(index) = self.by_family.get(&compact) {
            return Some(*index);
        }
        self.aliases
            .iter()
            .filter(|(alias, _)| compact.starts_with(alias))
            .max_by_key(|(alias, _)| alias.len())
            .map(|(_, index)| *index)
    }
}

/// The process-wide installed-font catalog.
pub(crate) fn catalog() -> &'static SystemFontCatalog {
    static CATALOG: OnceLock<SystemFontCatalog> = OnceLock::new();
    CATALOG.get_or_init(build_catalog)
}

fn build_catalog() -> SystemFontCatalog {
    let mut faces: Vec<SystemFont> = Vec::new();
    let mut seen_directories: Vec<PathBuf> = Vec::new();
    for (directory, source) in font_directories() {
        let directory = fs::canonicalize(&directory).unwrap_or(directory);
        if seen_directories.contains(&directory) {
            continue;
        }
        seen_directories.push(directory.clone());
        for path in font_files(&directory, 2) {
            for mut face in probe_font_file(&path, source) {
                face.family = face.family.trim().to_owned();
                face.label = if face.label.trim().is_empty() {
                    face.family.clone()
                } else {
                    face.label.trim().to_owned()
                };
                if face.family.is_empty() || ignored_family(&face.family) {
                    continue;
                }
                faces.push(face);
            }
        }
    }

    let mut grouped: HashMap<String, Vec<SystemFont>> = HashMap::new();
    for face in faces {
        grouped
            .entry(compact_name(&face.family))
            .or_default()
            .push(face);
    }

    let mut families: Vec<FamilyEntry> = Vec::new();
    for (_, mut faces) in grouped {
        // The same face can be installed in several directories. The first
        // directory wins (system fonts before user and WPS fonts), so a
        // duplicate PostScript name is dropped.
        let mut seen_postscript: Vec<String> = Vec::new();
        faces.retain(|face| {
            let key = compact_name(&face.postscript_name);
            if key.is_empty() || seen_postscript.contains(&key) {
                return false;
            }
            seen_postscript.push(key);
            true
        });
        if faces.is_empty() {
            continue;
        }
        // A family with no upright face cannot satisfy a regular style.
        if !faces.iter().any(|face| !face.italic) {
            continue;
        }
        let regular = faces
            .iter()
            .enumerate()
            .filter(|(_, face)| !face.italic)
            .min_by_key(|(_, face)| (face.weight.abs_diff(400), face.bold))
            .map(|(index, _)| index)
            .unwrap_or(0);
        let representative = &faces[regular];
        let bold = faces
            .iter()
            .any(|face| !face.italic && (face.bold || face.weight >= 600 || face.variable));
        let italic = faces.iter().any(|face| face.italic);
        let source = if representative.source == "wps" {
            "wps"
        } else {
            "system"
        };
        families.push(FamilyEntry {
            family: representative.family.clone(),
            label: representative.label.clone(),
            postscript_name: representative.postscript_name.clone(),
            bold,
            italic,
            source,
            faces,
        });
    }

    families.sort_by(|left, right| {
        source_rank(left.source)
            .cmp(&source_rank(right.source))
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
            .then_with(|| left.family.cmp(&right.family))
    });

    let mut by_family = HashMap::new();
    let mut aliases: Vec<(String, usize)> = Vec::new();
    for (index, entry) in families.iter().enumerate() {
        by_family.insert(compact_name(&entry.family), index);
        aliases.push((compact_name(&entry.family), index));
        for face in &entry.faces {
            aliases.push((compact_name(&face.postscript_name), index));
        }
    }
    aliases.retain(|(alias, _)| !alias.is_empty());
    aliases.sort_by_key(|(alias, _)| alias.len());

    SystemFontCatalog {
        families,
        by_family,
        aliases,
    }
}

fn source_rank(source: &str) -> u8 {
    if source == "wps" {
        0
    } else {
        1
    }
}

/// Fonts that are never useful as document text. Symbol and icon programs
/// would only lengthen the family list, private-use UI fonts start with a dot,
/// and WPS's Kingsoft Extra/Math/Sign faces are symbol/signature tools with
/// no alphabetic coverage.
fn ignored_family(family: &str) -> bool {
    if family.trim_start().starts_with('.') {
        return true;
    }
    let name = compact_name(family);
    const IGNORED: [&str; 11] = [
        "emoji",
        "symbol",
        "dingbat",
        "webdings",
        "wingdings",
        "braille",
        "icon",
        "pictogram",
        "awesome",
        "pua",
        "lastresort",
    ];
    if IGNORED.iter().any(|marker| name.contains(marker)) {
        return true;
    }
    if matches!(
        name.as_str(),
        "kingsoftextra" | "kingsoftmath" | "kingsoftsign"
    ) {
        return true;
    }
    matches!(family, "Helvetica" | "Times" | "Courier") || fonts::is_bundled_family(family)
}

fn compact_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// Strips the six-letter subset prefix PDF font names often carry
/// (`ABCDEF+MicrosoftYaHei`).
fn strip_subset_prefix(name: &str) -> &str {
    let bytes = name.as_bytes();
    if bytes.len() > 7
        && bytes[6] == b'+'
        && bytes[..6].iter().all(|byte| byte.is_ascii_uppercase())
    {
        &name[7..]
    } else {
        name
    }
}

// ---------------------------------------------------------------------------
// Directory discovery
// ---------------------------------------------------------------------------

fn font_directories() -> Vec<(PathBuf, &'static str)> {
    #[cfg(target_os = "macos")]
    {
        macos_font_directories()
    }
    #[cfg(target_os = "windows")]
    {
        windows_font_directories()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn macos_font_directories() -> Vec<(PathBuf, &'static str)> {
    let mut directories = vec![
        (PathBuf::from("/System/Library/Fonts"), "system"),
        (
            PathBuf::from("/System/Library/Fonts/Supplemental"),
            "system",
        ),
        (PathBuf::from("/Library/Fonts"), "system"),
    ];
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.push((home.join("Library/Fonts"), "system"));
        // Do not walk another app's private container. macOS can suspend
        // open()/read_dir() for a privacy prompt while the document registry
        // is locked, freezing all editing. WPS's installed bundle fonts and
        // fonts explicitly installed in ~/Library/Fonts remain available.
        collect_font_directories(
            &home.join("Library/Application Support/Kingsoft/office6"),
            5,
            &mut directories,
        );
    }
    for app in [
        "/Applications/wpsoffice.app/Contents/Resources/office6/fonts",
        "/Applications/WPS Office.app/Contents/Resources/office6/fonts",
    ] {
        collect_font_directories(Path::new(app), 5, &mut directories);
    }
    directories
}

#[cfg(target_os = "windows")]
fn windows_font_directories() -> Vec<(PathBuf, &'static str)> {
    let mut directories = Vec::new();
    if let Some(windir) = env::var_os("WINDIR") {
        directories.push((PathBuf::from(windir).join("Fonts"), "system"));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        directories.push((local.join("Microsoft/Windows/Fonts"), "system"));
        collect_font_directories(&local.join("Kingsoft"), 6, &mut directories);
    }
    for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
        let Some(base) = env::var_os(variable) else {
            continue;
        };
        let base = PathBuf::from(base);
        collect_font_directories(&base.join("Kingsoft"), 6, &mut directories);
        collect_font_directories(&base.join("WPS Office"), 6, &mut directories);
    }
    directories
}

/// Adds directories under `root` that directly contain font files. WPS keeps
/// its fonts in `office6/fonts` and downloaded faces in
/// `office6/data/fonts/extend_font`, several levels down, so a bounded walk is
/// cheaper and more robust than hard-coding every WPS version's layout.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn collect_font_directories(
    root: &Path,
    depth: usize,
    directories: &mut Vec<(PathBuf, &'static str)>,
) {
    if depth == 0 || !root.is_dir() {
        return;
    }
    if contains_font_file(root) {
        directories.push((root.to_path_buf(), "wps"));
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_directories(&path, depth - 1, directories);
        }
    }
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn contains_font_file(directory: &Path) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| is_font_file(&entry.path()))
}

fn font_files(directory: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_font_files(directory, depth, &mut files);
    files
}

fn collect_font_files(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, depth - 1, files);
        } else if is_font_file(&path) {
            files.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "ttc" | "otf" | "otc"
            )
        })
}

// ---------------------------------------------------------------------------
// Header-only probing
// ---------------------------------------------------------------------------

/// One face read from a font file, before it is grouped into a family.
struct ProbedFace {
    family: String,
    label: String,
    postscript_name: String,
    weight: u16,
    bold: bool,
    italic: bool,
    variable: bool,
}

/// Reads the embeddable TrueType faces of a font file. Returns an empty list
/// for files that are not sfnt programs, have no `glyf` outlines, forbid
/// embedding, or cannot be read.
fn probe_font_file(path: &Path, source: &'static str) -> Vec<SystemFont> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(metadata) = file.metadata() else {
        return Vec::new();
    };
    let file_len = metadata.len();
    let mut header = [0u8; 12];
    if file.read_exact(&mut header).is_err() {
        return Vec::new();
    }

    let mut offsets: Vec<(u64, u32)> = Vec::new();
    if &header[0..4] == b"ttcf" {
        let count = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
        if count == 0 || count > MAX_FACES_PER_FILE {
            return Vec::new();
        }
        let mut table = vec![0u8; count * 4];
        if file.read_exact(&mut table).is_err() {
            return Vec::new();
        }
        for (index, chunk) in table.chunks_exact(4).enumerate() {
            let offset = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as u64;
            offsets.push((offset, index as u32));
        }
    } else {
        offsets.push((0, 0));
    }

    let mut fonts = Vec::new();
    for (offset, face_index) in offsets {
        let Some(probed) = probe_face(&mut file, offset, file_len) else {
            continue;
        };
        fonts.push(SystemFont {
            family: probed.family,
            label: probed.label,
            postscript_name: probed.postscript_name,
            path: path.to_path_buf(),
            face_index,
            bold: probed.bold,
            italic: probed.italic,
            weight: probed.weight,
            variable: probed.variable,
            source,
        });
    }
    fonts
}

fn probe_face(file: &mut File, offset: u64, file_len: u64) -> Option<ProbedFace> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    let version = &header[0..4];
    if !matches!(version, b"\x00\x01\x00\x00" | b"true" | b"OTTO") {
        return None;
    }
    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    if num_tables == 0 || num_tables > MAX_TABLES {
        return None;
    }
    let mut directory = vec![0u8; num_tables * 16];
    file.read_exact(&mut directory).ok()?;

    let mut tables: HashMap<[u8; 4], (u64, usize)> = HashMap::new();
    for record in directory.chunks_exact(16) {
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&record[0..4]);
        let table_offset =
            u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as u64;
        let length = u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as usize;
        if table_offset > file_len || table_offset.saturating_add(length as u64) > file_len {
            continue;
        }
        tables.insert(tag, (table_offset, length));
    }
    // CFF outlines need a different PDF font type; the pipeline only writes
    // TrueType `glyf` programs.
    if !tables.contains_key(b"glyf") {
        return None;
    }

    let (name_offset, name_length) = *tables.get(b"name")?;
    if name_length > MAX_NAME_TABLE {
        return None;
    }
    let mut name_table = vec![0u8; name_length];
    file.seek(SeekFrom::Start(name_offset)).ok()?;
    file.read_exact(&mut name_table).ok()?;
    let names = parse_name_table(&name_table);

    let (weight, fs_type, fs_selection) = tables
        .get(b"OS/2")
        .map(|(os2_offset, os2_length)| {
            let mut os2 = vec![0u8; (*os2_length).min(64)];
            if os2.len() < 10 {
                return (400, 0, 0);
            }
            if file.seek(SeekFrom::Start(*os2_offset)).is_err()
                || file.read_exact(&mut os2).is_err()
            {
                return (400, 0, 0);
            }
            let weight = u16::from_be_bytes([os2[4], os2[5]]);
            let fs_type = u16::from_be_bytes([os2[8], os2[9]]);
            let selection = if os2.len() >= 64 {
                u16::from_be_bytes([os2[62], os2[63]])
            } else {
                0
            };
            (weight, fs_type, selection)
        })
        .unwrap_or((400, 0, 0));
    if !embedding_allowed(fs_type) {
        return None;
    }

    let variable = tables
        .get(b"fvar")
        .is_some_and(|(fvar_offset, fvar_length)| {
            has_weight_axis(file, *fvar_offset, *fvar_length)
        });

    let family = english_name(&names, &[16, 1])
        .or_else(|| first_name(&names, &[16, 1]))
        .unwrap_or_default();
    if family.is_empty() {
        return None;
    }
    let label = localized_name(&names, &[16, 1]).unwrap_or_else(|| family.clone());
    let postscript_name = english_name(&names, &[6])
        .or_else(|| first_name(&names, &[6]))
        .unwrap_or_default();
    if postscript_name.is_empty() {
        return None;
    }
    let subfamily = english_name(&names, &[17, 2])
        .or_else(|| first_name(&names, &[17, 2]))
        .unwrap_or_default()
        .to_lowercase();
    let italic =
        subfamily.contains("italic") || subfamily.contains("oblique") || fs_selection & 0x0201 != 0;
    let bold = weight >= 600
        || ["bold", "heavy", "black", "semibold", "demibold"]
            .iter()
            .any(|marker| subfamily.contains(marker));

    Some(ProbedFace {
        family,
        label,
        postscript_name,
        weight: if weight == 0 { 400 } else { weight },
        bold,
        italic,
        variable,
    })
}

/// True when `fsType` allows PDFX to embed a subset. Bit 0x0100 forbids
/// subsetting and 0x0200 forbids outline embedding; values 0 (Installable)
/// and 8 (Editable) allow the document to be edited.
fn embedding_allowed(fs_type: u16) -> bool {
    (fs_type & 0x000F == 0 || fs_type & 0x0008 != 0)
        && fs_type & 0x0100 == 0
        && fs_type & 0x0200 == 0
}

fn has_weight_axis(file: &mut File, offset: u64, length: usize) -> bool {
    if length < 16 || length > MAX_FVAR_TABLE {
        return false;
    }
    let mut table = vec![0u8; length];
    if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut table).is_err() {
        return false;
    }
    let axes_offset = u16::from_be_bytes([table[4], table[5]]) as usize;
    let axis_count = u16::from_be_bytes([table[8], table[9]]) as usize;
    let axis_size = u16::from_be_bytes([table[10], table[11]]) as usize;
    if axis_size < 20 {
        return false;
    }
    for index in 0..axis_count.min(64) {
        let start = axes_offset.saturating_add(index * axis_size);
        let Some(axis) = table.get(start..start + 4) else {
            return false;
        };
        if axis == b"wght" {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Name records
// ---------------------------------------------------------------------------

struct NameRecord {
    platform: u16,
    language: u16,
    name_id: u16,
    text: String,
}

const ENGLISH_LANGUAGES: [u16; 2] = [0x0409, 0x0009];
const CHINESE_LANGUAGES: [u16; 5] = [0x0804, 0x0404, 0x0C04, 0x1004, 0x1404];

fn parse_name_table(table: &[u8]) -> Vec<NameRecord> {
    if table.len() < 6 {
        return Vec::new();
    }
    let count = u16::from_be_bytes([table[2], table[3]]) as usize;
    let storage = u16::from_be_bytes([table[4], table[5]]) as usize;
    let Some(directory_end) = count.checked_mul(12).and_then(|size| size.checked_add(6)) else {
        return Vec::new();
    };
    if directory_end > table.len() || storage > table.len() {
        return Vec::new();
    }
    let mut records = Vec::with_capacity(count);
    for index in 0..count {
        let base = 6 + index * 12;
        let record = &table[base..base + 12];
        let platform = u16::from_be_bytes([record[0], record[1]]);
        let encoding = u16::from_be_bytes([record[2], record[3]]);
        let language = u16::from_be_bytes([record[4], record[5]]);
        let name_id = u16::from_be_bytes([record[6], record[7]]);
        let length = u16::from_be_bytes([record[8], record[9]]) as usize;
        let offset = u16::from_be_bytes([record[10], record[11]]) as usize;
        let Some(start) = storage.checked_add(offset) else {
            continue;
        };
        let Some(text) = table
            .get(start..start.saturating_add(length))
            .map(|bytes| decode_name(bytes, platform, encoding))
        else {
            continue;
        };
        if !text.is_empty() {
            records.push(NameRecord {
                platform,
                language,
                name_id,
                text,
            });
        }
    }
    records
}

fn decode_name(bytes: &[u8], platform: u16, encoding: u16) -> String {
    let text = match platform {
        0 => decode_utf16(bytes),
        3 if matches!(encoding, 0 | 1 | 10) => decode_utf16(bytes),
        1 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        _ => String::new(),
    };
    text.trim_matches('\0').trim().to_owned()
}

fn decode_utf16(bytes: &[u8]) -> String {
    if !bytes.len().is_multiple_of(2) {
        return String::new();
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16(&units).unwrap_or_default()
}

fn english_name(records: &[NameRecord], ids: &[u16]) -> Option<String> {
    for id in ids {
        if let Some(record) = records.iter().find(|record| {
            record.name_id == *id
                && record.platform == 3
                && ENGLISH_LANGUAGES.contains(&record.language)
        }) {
            return Some(record.text.clone());
        }
        if let Some(record) = records
            .iter()
            .find(|record| record.name_id == *id && record.platform == 0)
        {
            return Some(record.text.clone());
        }
    }
    None
}

fn localized_name(records: &[NameRecord], ids: &[u16]) -> Option<String> {
    for id in ids {
        if let Some(record) = records.iter().find(|record| {
            record.name_id == *id
                && record.platform == 3
                && CHINESE_LANGUAGES.contains(&record.language)
        }) {
            return Some(record.text.clone());
        }
    }
    None
}

fn first_name(records: &[NameRecord], ids: &[u16]) -> Option<String> {
    for id in ids {
        if let Some(record) = records.iter().find(|record| record.name_id == *id) {
            return Some(record.text.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn liberation_sans() -> SystemFont {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/LiberationSans-Regular.ttf");
        probe_font_file(&path, "system")
            .into_iter()
            .next()
            .expect("Liberation Sans is a probeable TrueType face")
    }

    #[test]
    fn probing_reads_name_weight_and_embedding_permission() {
        let font = liberation_sans();
        assert_eq!(font.family, "Liberation Sans");
        assert_eq!(font.postscript_name, "LiberationSans");
        assert!(!font.italic);
        assert!(!font.bold);
        assert_eq!(font.weight, 400);
        assert_eq!(font.face_index, 0);
        assert_eq!(font.source, "system");
    }

    #[test]
    fn probing_rejects_restricted_font_programs() {
        assert!(!embedding_allowed(0x0002));
        assert!(!embedding_allowed(0x0100));
        assert!(!embedding_allowed(0x0200));
        assert!(embedding_allowed(0x0000));
        assert!(embedding_allowed(0x0008));
        assert!(
            embedding_allowed(0x000A),
            "version<=2 treats bit 3 as editable even when bit 1 is also set"
        );
    }

    #[test]
    fn subset_names_resolve_back_to_the_family() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/LiberationSans-Regular.ttf");
        let fonts = probe_font_file(&path, "system");
        let mut catalog = SystemFontCatalog {
            families: Vec::new(),
            by_family: HashMap::new(),
            aliases: Vec::new(),
        };
        for font in fonts {
            let index = catalog.families.len();
            catalog.by_family.insert(compact_name(&font.family), index);
            catalog.aliases.push((compact_name(&font.family), index));
            catalog
                .aliases
                .push((compact_name(&font.postscript_name), index));
            catalog.families.push(FamilyEntry {
                family: font.family.clone(),
                label: font.label.clone(),
                postscript_name: font.postscript_name.clone(),
                bold: false,
                italic: false,
                source: "system",
                faces: vec![font],
            });
        }
        assert_eq!(
            catalog.family_for_name("LiberationSans-Regular-0123456789abcdef"),
            Some("Liberation Sans")
        );
        assert_eq!(
            catalog.family_for_name("ABCDEF+LiberationSans"),
            Some("Liberation Sans")
        );
        assert_eq!(catalog.family_for_name("Noto Sans SC"), None);
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn liberation_sans() -> SystemFont {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fonts/LiberationSans-Regular.ttf");
        probe_font_file(&path, "system")
            .into_iter()
            .next()
            .expect("Liberation Sans is a probeable TrueType face")
    }

    #[test]
    fn the_installed_catalog_is_self_consistent() {
        let catalog = catalog();
        for entry in catalog.families() {
            assert!(!entry.family.is_empty());
            assert!(!entry.label.is_empty());
            assert!(!entry.postscript_name.is_empty());
            assert!(
                !entry.family.starts_with('.'),
                "private UI fonts are filtered out"
            );
            assert!(
                catalog.has_family(&entry.family),
                "{} must resolve by its own name",
                entry.family
            );
            assert_eq!(
                catalog.family_for_name(&entry.family),
                Some(entry.family.as_str())
            );
            assert!(
                catalog
                    .face_for_style(&entry.family, false, false)
                    .is_some(),
                "{} must have an upright face",
                entry.family
            );
            if entry.italic {
                assert!(
                    catalog.face_for_style(&entry.family, false, true).is_some(),
                    "{} advertises italic and needs an italic face",
                    entry.family
                );
            }
        }
        assert_eq!(catalog.family_for_name("PDFXNoSuchInstalledFamily"), None);
    }

    #[test]
    fn installed_programs_subset_through_the_shared_pipeline() {
        let font = liberation_sans();
        let subset = font
            .subset("PDFX text", fonts::weight_for(false))
            .expect("an installed face must subset");
        let face = ttf_parser::Face::parse(&subset.bytes, 0).expect("the subset parses");
        for character in "PDFX text".chars() {
            assert!(
                face.glyph_index(character).is_some(),
                "the subset must cover {character:?}"
            );
        }
        assert!(
            !font.missing_characters("PDFX 文本").is_empty(),
            "Liberation Sans has no CJK glyphs, so the missing characters are reported"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod discovery_regression_tests {
    #[test]
    fn macos_font_discovery_does_not_enter_other_app_private_containers() {
        let started = std::time::Instant::now();
        let directories = super::macos_font_directories();
        assert!(directories
            .iter()
            .all(|(path, _)| !path.to_string_lossy().contains("/Library/Containers/")));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
