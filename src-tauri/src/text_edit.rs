//! Editing existing text and the session-level edit history.
//!
//! A click on a character of the text layer resolves to the PDFium Text Object
//! that contains it. That object supplies the initial content, font, size,
//! colour and position of the Text Box. Text edits remove the original page
//! objects and write replacement objects at the same baseline, so extraction
//! and rendering observe the change rather than a stacked duplicate.
//!
//! Everything here runs behind the Tauri command boundary: [`PdfPageTextObject`]
//! and other engine types never cross it.

use std::collections::HashMap;

use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfFontBuiltin, PdfFontToken, PdfMatrix, PdfPageObject,
    PdfPageObjectCommon, PdfPageObjects, PdfPageObjectsCommon, PdfPagePathObject, PdfPageText,
    PdfPageTextObject, PdfPageTextRenderMode, PdfPoints, ToPdfFontToken,
};
use serde::{Deserialize, Serialize};

use crate::commands::{
    build_text_layer_with_raw_indices, load_page, with_open_document, CommandError,
    CommandErrorCode, DisplayGeometry, OpenDocument,
};

/// Horizontal alignment of the lines inside a Text Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextAlignment {
    #[default]
    Left,
    Center,
    Right,
}

/// Whole-box Text Box style as the frontend edits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxStyleView {
    /// One of the three base-14 families currently offered: Helvetica, Times
    /// or Courier. Ticket 11 can widen this list through [`text_box_fonts`].
    pub font_family: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Fill colour as `#rrggbb`.
    pub color: String,
    pub alignment: TextAlignment,
}

/// One font the Text Box editor may use.
///
/// The metadata is deliberately explicit so a later font pipeline (ticket 11)
/// can add embedded TrueType families without changing the command shape:
/// `source` distinguishes PDF base-14 fonts from embedded font programs,
/// `embedded` says whether a program must be written into the PDF, and
/// `coverage` states which characters the font can render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxFontView {
    pub family: String,
    pub label: String,
    pub bold: bool,
    pub italic: bool,
    pub postscript_name: String,
    /// `base14` for the built-in fonts. Ticket 11 adds `embedded`.
    pub source: String,
    /// True when the font program is embedded in the saved PDF.
    pub embedded: bool,
    /// True when the project may freely embed the font.
    pub embeddable: bool,
    /// `win_ansi` for base-14. Ticket 11 adds `cjk`/`full`.
    pub coverage: String,
}

struct Base14Font {
    family: &'static str,
    label: &'static str,
    bold: bool,
    italic: bool,
    postscript_name: &'static str,
    builtin: PdfFontBuiltin,
}

/// The only fonts offered for new Text Boxes until the subset-embedding
/// pipeline lands: the PDF base-14 Latin families and their four variants.
/// They need no font program in the document, so using them is always within
/// the project's embedding policy (ADR-0006).
const BASE14_FONTS: [Base14Font; 12] = [
    Base14Font {
        family: "Helvetica",
        label: "Helvetica",
        bold: false,
        italic: false,
        postscript_name: "Helvetica",
        builtin: PdfFontBuiltin::Helvetica,
    },
    Base14Font {
        family: "Helvetica",
        label: "Helvetica Bold",
        bold: true,
        italic: false,
        postscript_name: "Helvetica-Bold",
        builtin: PdfFontBuiltin::HelveticaBold,
    },
    Base14Font {
        family: "Helvetica",
        label: "Helvetica Italic",
        bold: false,
        italic: true,
        postscript_name: "Helvetica-Oblique",
        builtin: PdfFontBuiltin::HelveticaOblique,
    },
    Base14Font {
        family: "Helvetica",
        label: "Helvetica Bold Italic",
        bold: true,
        italic: true,
        postscript_name: "Helvetica-BoldOblique",
        builtin: PdfFontBuiltin::HelveticaBoldOblique,
    },
    Base14Font {
        family: "Times",
        label: "Times",
        bold: false,
        italic: false,
        postscript_name: "Times-Roman",
        builtin: PdfFontBuiltin::TimesRoman,
    },
    Base14Font {
        family: "Times",
        label: "Times Bold",
        bold: true,
        italic: false,
        postscript_name: "Times-Bold",
        builtin: PdfFontBuiltin::TimesBold,
    },
    Base14Font {
        family: "Times",
        label: "Times Italic",
        bold: false,
        italic: true,
        postscript_name: "Times-Italic",
        builtin: PdfFontBuiltin::TimesItalic,
    },
    Base14Font {
        family: "Times",
        label: "Times Bold Italic",
        bold: true,
        italic: true,
        postscript_name: "Times-BoldItalic",
        builtin: PdfFontBuiltin::TimesBoldItalic,
    },
    Base14Font {
        family: "Courier",
        label: "Courier",
        bold: false,
        italic: false,
        postscript_name: "Courier",
        builtin: PdfFontBuiltin::Courier,
    },
    Base14Font {
        family: "Courier",
        label: "Courier Bold",
        bold: true,
        italic: false,
        postscript_name: "Courier-Bold",
        builtin: PdfFontBuiltin::CourierBold,
    },
    Base14Font {
        family: "Courier",
        label: "Courier Italic",
        bold: false,
        italic: true,
        postscript_name: "Courier-Oblique",
        builtin: PdfFontBuiltin::CourierOblique,
    },
    Base14Font {
        family: "Courier",
        label: "Courier Bold Italic",
        bold: true,
        italic: true,
        postscript_name: "Courier-BoldOblique",
        builtin: PdfFontBuiltin::CourierBoldOblique,
    },
];

/// The `text_box_fonts` command: every font the Text Box editor may use.
///
/// These are base-14 fonts: the PDF viewer supplies the program, so nothing is
/// embedded and only WinAnsi text is safe. The registry is flat (one entry per
/// family/variant) so ticket 11 can append embedded families with their own
/// coverage without redesigning the UI contract.
#[tauri::command]
pub fn text_box_fonts() -> Vec<TextBoxFontView> {
    BASE14_FONTS
        .iter()
        .map(|font| TextBoxFontView {
            family: font.family.to_owned(),
            label: font.label.to_owned(),
            bold: font.bold,
            italic: font.italic,
            postscript_name: font.postscript_name.to_owned(),
            source: "base14".to_owned(),
            embedded: false,
            embeddable: true,
            coverage: "win_ansi".to_owned(),
        })
        .collect()
}

fn base14_font(family: &str, bold: bool, italic: bool) -> Option<&'static Base14Font> {
    BASE14_FONTS
        .iter()
        .find(|font| font.family == family && font.bold == bold && font.italic == italic)
}

fn builtin_for_style(family: &str, bold: bool, italic: bool) -> Option<PdfFontBuiltin> {
    base14_font(family, bold, italic).map(|font| font.builtin)
}

fn parse_color(color: &str) -> Result<(u8, u8, u8), CommandError> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("colour {color:?} is not a #rrggbb value"),
        ));
    }
    let channel = |offset: usize| {
        u8::from_str_radix(&hex[offset..offset + 2], 16).expect("validated hex digits")
    };
    Ok((channel(0), channel(2), channel(4)))
}

/// The validated internal form of [`TextBoxStyleView`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextBoxStyle {
    pub(crate) font_family: String,
    pub(crate) font_size: f32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) color: (u8, u8, u8),
    pub(crate) alignment: TextAlignment,
}

impl TextBoxStyle {
    fn default_box() -> Self {
        Self {
            font_family: "Helvetica".to_owned(),
            font_size: 16.0,
            bold: false,
            italic: false,
            underline: false,
            color: (0, 0, 0),
            alignment: TextAlignment::Left,
        }
    }

    fn from_view(view: &TextBoxStyleView) -> Result<Self, CommandError> {
        let family = view.font_family.trim();
        if builtin_for_style(family, view.bold, view.italic).is_none() {
            let available: Vec<&str> = BASE14_FONTS.iter().map(|font| font.label).collect();
            return Err(CommandError::new(
                CommandErrorCode::UnsupportedFont,
                format!(
                    "font {:?} is not available; use one of the built-in base-14 fonts: {}",
                    view.font_family,
                    available.join(", ")
                ),
            ));
        }
        if !view.font_size.is_finite() || !(4.0..=400.0).contains(&view.font_size) {
            return Err(CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("font size {} is outside 4..400 points", view.font_size),
            ));
        }
        Ok(Self {
            font_family: family.to_owned(),
            font_size: view.font_size,
            bold: view.bold,
            italic: view.italic,
            underline: view.underline,
            color: parse_color(&view.color)?,
            alignment: view.alignment,
        })
    }

    fn builtin(&self) -> PdfFontBuiltin {
        builtin_for_style(&self.font_family, self.bold, self.italic)
            .expect("TextBoxStyle::from_view validates the font")
    }

    fn pdf_color(&self) -> PdfColor {
        PdfColor::new(self.color.0, self.color.1, self.color.2, 255)
    }

    fn color_hex(&self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            self.color.0, self.color.1, self.color.2
        )
    }
}

/// A Text Box as the frontend needs it when the user starts editing.
///
/// Geometry is in display-space points (top-left origin, intrinsic page
/// rotation applied), the same coordinate system as the text layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxTargetView {
    /// Stable id of this Text Box for the life of the document session.
    pub box_id: u64,
    pub page_index: u32,
    /// Current full content of the Text Box, with one line per text object.
    pub text: String,
    /// Name of the font the original Text Object uses.
    pub font_family: String,
    pub font_size: f32,
    /// Fill colour as `#rrggbb`.
    pub color: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Style variant flags. A legacy Text Object can be bold/italic through its
    /// font name even when the variant was not chosen in the style panel.
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub alignment: TextAlignment,
    /// Clockwise rotation in degrees in display space.
    pub rotation: f32,
    /// True for boxes whose height follows their content. Manual resizing
    /// turns this off.
    pub auto_height: bool,
    /// True when this Text Box was created by the Text Box tool rather than
    /// being an original Text Object. Created boxes may be empty.
    pub created: bool,
}

/// A request to create an empty Text Box on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTextBoxRequest {
    /// Top-left corner in display-space points.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Omit for the default visible style: Helvetica 16, black, left aligned.
    #[serde(default)]
    pub style: Option<TextBoxStyleView>,
}

/// A request to move, resize or rotate a Text Box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxTransformRequest {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// False after the user resizes by hand: the height then clips content
    /// instead of growing.
    pub auto_height: bool,
    /// Clockwise rotation in degrees in display space.
    pub rotation: f32,
}

/// The outcome of creating or restyling a Text Box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxEditResultView {
    pub page_index: u32,
    pub target: TextBoxTargetView,
    /// Set when an existing font could not be preserved.
    pub font: Option<FontUsageView>,
    pub history: EditHistoryView,
}

/// The font used by a committed edit, and whether it differs from the
/// original Text Object's font.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontUsageView {
    pub original_family: String,
    pub family: String,
    pub fallback: bool,
    /// Human-readable explanation when `fallback` is true.
    pub reason: Option<String>,
}

/// Document-wide undo/redo state shared by every edit tool.
///
/// `revision` is the id of the document state currently on the page;
/// `savedRevision` is the state last written to disk. Undoing back to the saved
/// state clears `dirty` without another save. Saving never clears either stack,
/// so the user can undo past the save point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditHistoryView {
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
    pub saved_revision: u64,
    pub dirty: bool,
}

/// The outcome of committing a text edit or deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEditResultView {
    pub page_index: u32,
    pub box_id: u64,
    /// Logical Text Box content after the edit (empty after a deletion).
    pub text: String,
    /// Set when the replacement could not keep the original font.
    pub font: Option<FontUsageView>,
    pub history: EditHistoryView,
}

/// The outcome of an undo or redo step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditStepView {
    /// Page whose content changed, so the frontend can refresh just that page.
    pub page_index: u32,
    pub history: EditHistoryView,
}

/// Position and size of a box in display space.
///
/// `x`/`y` are the unrotated box's top-left corner; `rotation` is clockwise
/// around that corner. `height` is a floor while `auto_height` is true and a
/// hard clip limit otherwise.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoxLayout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) rotation: f32,
    pub(crate) auto_height: bool,
}

/// The complete record state an undo/redo step restores.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecordSnapshot {
    pub(crate) box_id: u64,
    pub(crate) start: usize,
    pub(crate) count: usize,
    pub(crate) width: f32,
    pub(crate) text: String,
    pub(crate) layout: Option<BoxLayout>,
    pub(crate) style: Option<TextBoxStyle>,
    pub(crate) created: bool,
    pub(crate) deleted: bool,
    /// Position in the page's record vector, so redo can re-insert a box that
    /// an undo removed.
    pub(crate) record_index: usize,
}

/// One committed Text Box change.
///
/// The detached page-object snapshots are what make undo restore the exact
/// previous appearance, including font resources and text-object metadata.
/// Record snapshots restore the logical box state (text, geometry, style and
/// deletion tombstones) alongside them.
///
/// Invariant: while the action sits on the undo stack, `before_objects` holds
/// the detached "before" objects and `after_objects` is `None` because the
/// "after" objects are on the page. On the redo stack the two are swapped.
pub(crate) struct BoxEditAction {
    pub(crate) page_index: u32,
    pub(crate) box_id: u64,
    pub(crate) start: usize,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before_record: Option<RecordSnapshot>,
    pub(crate) before_objects: Option<Vec<PdfPageObject<'static>>>,
    pub(crate) after_record: Option<RecordSnapshot>,
    pub(crate) after_objects: Option<Vec<PdfPageObject<'static>>>,
}

impl BoxEditAction {
    fn before_count(&self) -> usize {
        self.before_record
            .as_ref()
            .map(|record| record.count)
            .unwrap_or(0)
    }

    fn after_count(&self) -> usize {
        self.after_record
            .as_ref()
            .map(|record| record.count)
            .unwrap_or(0)
    }
}

pub(crate) enum EditAction {
    Box(BoxEditAction),
}

/// One editable Text Box on a page.
///
/// A box is a contiguous range of page objects emitted at one baseline block.
/// Editing a box replaces the whole range, so a wrapped replacement is edited
/// as one unit the next time it is clicked. Underlined boxes include path
/// objects (the underline strokes) in the range after their text object.
pub(crate) struct TextBoxRecord {
    pub(crate) box_id: u64,
    /// Index of the box's first page object.
    pub(crate) start: usize,
    /// Number of page objects the box currently occupies.
    pub(crate) count: usize,
    /// Wrap width in display-space points.
    pub(crate) width: f32,
    /// Current text, one line per text object.
    pub(crate) text: String,
    /// Some once the box has a canonical layout: a created box, or a legacy
    /// Text Object that was styled, transformed or already edited by a styled
    /// box. Legacy boxes without it derive their geometry from the text layer
    /// so opening the editor never moves existing text.
    pub(crate) layout: Option<BoxLayout>,
    /// Some when the box uses a chosen built-in style. `None` means a legacy
    /// Text Object keeps its original font token.
    pub(crate) style: Option<TextBoxStyle>,
    /// Created by the Text Box tool rather than resolved from existing text.
    pub(crate) created: bool,
    /// Deleted boxes stay in the record list as tombstones so undo can
    /// restore them; target lookups and the overlay hide them.
    pub(crate) deleted: bool,
}

impl TextBoxRecord {
    fn contains(&self, object_index: usize) -> bool {
        self.count > 0 && object_index >= self.start && object_index < self.start + self.count
    }

    fn snapshot(&self, record_index: usize) -> RecordSnapshot {
        RecordSnapshot {
            box_id: self.box_id,
            start: self.start,
            count: self.count,
            width: self.width,
            text: self.text.clone(),
            layout: self.layout.clone(),
            style: self.style.clone(),
            created: self.created,
            deleted: self.deleted,
            record_index,
        }
    }

    fn restore(&mut self, snapshot: &RecordSnapshot) {
        self.box_id = snapshot.box_id;
        self.start = snapshot.start;
        self.count = snapshot.count;
        self.width = snapshot.width;
        self.text = snapshot.text.clone();
        self.layout = snapshot.layout.clone();
        self.style = snapshot.style.clone();
        self.created = snapshot.created;
        self.deleted = snapshot.deleted;
    }

    fn from_snapshot(snapshot: &RecordSnapshot) -> Self {
        Self {
            box_id: snapshot.box_id,
            start: snapshot.start,
            count: snapshot.count,
            width: snapshot.width,
            text: snapshot.text.clone(),
            layout: snapshot.layout.clone(),
            style: snapshot.style.clone(),
            created: snapshot.created,
            deleted: snapshot.deleted,
        }
    }
}

/// Per-document editing session.
///
/// Lives next to the PDFium document for as long as the Document is open. The
/// undo/redo stacks store detached page-object snapshots, which is what makes
/// undo restore the exact previous rendering (including font resources).
#[derive(Default)]
pub struct EditSession {
    pub(crate) records: HashMap<u32, Vec<TextBoxRecord>>,
    pub(crate) next_box_id: u64,
    pub(crate) undo_stack: Vec<EditAction>,
    pub(crate) redo_stack: Vec<EditAction>,
    /// Id of the document state currently on the page.
    pub(crate) current_state: u64,
    /// Monotonic counter; every committed edit gets a fresh state id.
    pub(crate) next_state: u64,
    /// State id last written to disk by the save pipeline.
    pub(crate) saved_state: u64,
}

impl EditSession {
    pub(crate) fn history_view(&self) -> EditHistoryView {
        EditHistoryView {
            can_undo: !self.undo_stack.is_empty(),
            can_redo: !self.redo_stack.is_empty(),
            revision: self.current_state,
            saved_revision: self.saved_state,
            dirty: self.current_state != self.saved_state,
        }
    }
}

fn no_editable_text(page_index: u32) -> CommandError {
    CommandError::new(
        CommandErrorCode::NoEditableText,
        format!("page {page_index} has no editable text"),
    )
}

fn ensure_content_editable(document: &PdfDocument<'_>) -> Result<(), CommandError> {
    // A missing permission read is treated as denied, matching `open_document`.
    let allowed = document
        .permissions()
        .can_modify_document_content()
        .unwrap_or(false);
    if allowed {
        Ok(())
    } else {
        Err(CommandError::new(
            CommandErrorCode::PermissionDenied,
            "this document does not allow changing its content",
        ))
    }
}

/// Finds the page object containing a PDFium raw character index.
fn object_index_for_raw_char(
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    raw_index: usize,
) -> Option<usize> {
    for object_index in 0..objects.len() {
        let Ok(object) = objects.get(object_index) else {
            continue;
        };
        let Some(text_object) = object.as_text_object() else {
            continue;
        };
        let Ok(chars) = page_text.chars_for_object(text_object) else {
            continue;
        };
        if chars.iter().any(|character| character.index() == raw_index) {
            return Some(object_index);
        }
    }
    None
}

fn measure_text_width(
    document: &PdfDocument<'static>,
    object: &PdfPageTextObject<'_>,
    text: &str,
) -> Option<f32> {
    let font_size = object.unscaled_font_size().value;
    if !font_size.is_finite() || font_size <= 0.0 || text.is_empty() {
        return None;
    }
    let detached = PdfPageTextObject::new(
        document,
        text,
        object.font().token(),
        PdfPoints::new(font_size),
    )
    .ok()?;
    let bounds = detached.bounds().ok()?;
    let width = bounds.width().value;
    width.is_finite().then_some(width)
}

fn color_hex(text_object: &PdfPageTextObject<'_>) -> String {
    match text_object.fill_color() {
        Ok(color) => format!(
            "#{:02x}{:02x}{:02x}",
            color.red(),
            color.green(),
            color.blue()
        ),
        Err(_) => "#000000".to_owned(),
    }
}

/// Display-space union of the text-layer boxes for every character the box's
/// page objects contain.
///
/// Tight PDFium ink bounds start inside the first glyph's side bearing, which
/// would make the editor sit a few points right of the visible text. The text
/// layer boxes share the frontend's coordinate system and include the pen
/// origins, so the Text Box lines up with the text the user clicked.
fn box_display_rect(
    layer: &crate::commands::TextLayerView,
    layer_raw_indices: &[usize],
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    record: &TextBoxRecord,
) -> Option<(f32, f32, f32, f32)> {
    let mut raw_indices = Vec::new();
    for object_index in record.start..record.start + record.count {
        let object = objects.get(object_index).ok()?;
        let text_object = object.as_text_object()?;
        let chars = page_text.chars_for_object(text_object).ok()?;
        raw_indices.extend(chars.iter().map(|character| character.index()));
    }
    raw_indices.sort_unstable();
    raw_indices.dedup();

    let mut union: Option<(f32, f32, f32, f32)> = None;
    for (character, raw_index) in layer.chars.iter().zip(layer_raw_indices) {
        if character.width <= 0.0
            || character.height <= 0.0
            || raw_indices.binary_search(raw_index).is_err()
        {
            continue;
        }
        let (left, top) = (character.x, character.y);
        let (right, bottom) = (left + character.width, top + character.height);
        union = Some(match union {
            Some((union_left, union_top, union_right, union_bottom)) => (
                union_left.min(left),
                union_top.min(top),
                union_right.max(right),
                union_bottom.max(bottom),
            ),
            None => (left, top, right, bottom),
        });
    }
    union.map(|(left, top, right, bottom)| (left, top, right - left, bottom - top))
}

fn style_flags_from_font_name(name: &str) -> (bool, bool) {
    let name = base_font_name(name);
    (
        name.contains("Bold"),
        name.contains("Italic") || name.contains("Oblique"),
    )
}

/// Normalizes an angle to the half-open range `(-180, 180]`.
fn normalize_angle(angle: f32) -> f32 {
    let mut angle = angle % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    } else if angle > 180.0 {
        angle -= 360.0;
    }
    if angle == -0.0 {
        0.0
    } else {
        angle
    }
}

/// A PDF text matrix stores the baseline direction in `[a, b]` and no font
/// size, so the display-space rotation is the page's clockwise rotation minus
/// the user-space baseline angle.
fn display_rotation_from_matrix(matrix: [f32; 6], page_rotation: u16) -> f32 {
    let baseline_angle = matrix[1].atan2(matrix[0]).to_degrees();
    normalize_angle(page_rotation as f32 - baseline_angle)
}

fn first_text_metadata(
    objects: &PdfPageObjects<'_>,
    start: usize,
) -> Result<(String, f32, String), CommandError> {
    let object = objects.get(start).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read a Text Box object: {error:?}"),
        )
    })?;
    let text_object = object.as_text_object().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::Internal,
            "the Text Box does not start with a text object",
        )
    })?;
    let font_family = text_object.font().name();
    let font_size = text_object.unscaled_font_size().value;
    let color = color_hex(text_object);
    Ok((font_family, font_size, color))
}

fn target_from_record(
    layer: &crate::commands::TextLayerView,
    layer_raw_indices: &[usize],
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    record: &TextBoxRecord,
    page_index: u32,
) -> Result<TextBoxTargetView, CommandError> {
    if record.deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("Text Box {} has been deleted", record.box_id),
        ));
    }

    // Created and materialized boxes carry authoritative geometry. Word wrap
    // and rotation mean the text layer's axis-aligned character boxes are not
    // the box outline, so derive nothing from them.
    if let Some(layout) = &record.layout {
        let (font_family, font_size, color) = match &record.style {
            Some(style) => (
                style.font_family.clone(),
                style.font_size,
                style.color_hex(),
            ),
            None => first_text_metadata(objects, record.start)?,
        };
        let (bold, italic) = match &record.style {
            Some(style) => (style.bold, style.italic),
            None => style_flags_from_font_name(&font_family),
        };
        let underline = record
            .style
            .as_ref()
            .map(|style| style.underline)
            .unwrap_or(false);
        let alignment = record
            .style
            .as_ref()
            .map(|style| style.alignment)
            .unwrap_or_default();
        return Ok(TextBoxTargetView {
            box_id: record.box_id,
            page_index,
            text: record.text.clone(),
            font_family,
            font_size,
            color,
            x: layout.x,
            y: layout.y,
            width: layout.width,
            height: layout.height,
            bold,
            italic,
            underline,
            alignment,
            rotation: layout.rotation,
            auto_height: layout.auto_height,
            created: record.created,
        });
    }

    let first = objects.get(record.start).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read a Text Box object: {error:?}"),
        )
    })?;
    let text_object = first.as_text_object().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::Internal,
            "the Text Box does not start with a text object",
        )
    })?;
    let (x, y, width, height) =
        box_display_rect(layer, layer_raw_indices, page_text, objects, record).ok_or_else(
            || {
                CommandError::new(
                    CommandErrorCode::RenderFailed,
                    format!("could not read the bounds of the Text Box on page {page_index}"),
                )
            },
        )?;

    let font = text_object.font();
    let font_family = font.name();
    let (bold, italic) = style_flags_from_font_name(&font_family);
    let font_size = text_object.unscaled_font_size().value;
    let color = color_hex(text_object);
    let rotation = text_object
        .matrix()
        .map(|matrix| {
            display_rotation_from_matrix(
                [
                    matrix.a(),
                    matrix.b(),
                    matrix.c(),
                    matrix.d(),
                    matrix.e(),
                    matrix.f(),
                ],
                layer.rotation,
            )
        })
        .unwrap_or(0.0);

    Ok(TextBoxTargetView {
        box_id: record.box_id,
        page_index,
        text: record.text.clone(),
        font_family,
        font_size,
        color,
        x,
        y,
        width,
        height,
        bold,
        italic,
        underline: false,
        alignment: TextAlignment::Left,
        rotation,
        auto_height: false,
        created: record.created,
    })
}

fn target_for(
    entry: &mut OpenDocument,
    page_index: u32,
    char_index: u32,
) -> Result<TextBoxTargetView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let page = load_page(&entry.document, page_index)?;
    let (layer, raw_indices) = build_text_layer_with_raw_indices(&page, page_index)?;

    if !layer.has_text {
        return Err(no_editable_text(page_index));
    }

    let raw_index = *raw_indices.get(char_index as usize).ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!(
                "character {char_index} is outside the {} characters on page {page_index}",
                raw_indices.len()
            ),
        )
    })?;

    let page_text = page.text().map_err(|_| no_editable_text(page_index))?;
    let objects = page.objects();
    let object_index = object_index_for_raw_char(&page_text, objects, raw_index)
        .ok_or_else(|| no_editable_text(page_index))?;

    if let Some(record) = entry
        .edit
        .records
        .get(&page_index)
        .and_then(|records| records.iter().find(|record| record.contains(object_index)))
    {
        return target_from_record(
            &layer,
            &raw_indices,
            &page_text,
            objects,
            record,
            page_index,
        );
    }
    let object = objects.get(object_index).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read page object {object_index}: {error:?}"),
        )
    })?;
    let text_object = object
        .as_text_object()
        .ok_or_else(|| no_editable_text(page_index))?;
    let text = text_object.text();
    if text.trim().is_empty() {
        return Err(no_editable_text(page_index));
    }
    let width = measure_text_width(&entry.document, text_object, &text).unwrap_or_else(|| {
        object
            .bounds()
            .map(|bounds| bounds.width().value.abs())
            .unwrap_or(1.0)
            .max(1.0)
    });

    entry.edit.next_box_id += 1;
    let record = TextBoxRecord {
        box_id: entry.edit.next_box_id,
        start: object_index,
        count: 1,
        width,
        text,
        layout: None,
        style: None,
        created: false,
        deleted: false,
    };
    let view = target_from_record(
        &layer,
        &raw_indices,
        &page_text,
        objects,
        &record,
        page_index,
    )?;
    entry
        .edit
        .records
        .entry(page_index)
        .or_default()
        .push(record);
    Ok(view)
}

/// The `text_edit_target` command: resolves the Text Object under a clicked
/// text-layer character into the initial Text Box state.
#[tauri::command]
pub fn text_edit_target(
    document_id: String,
    page_index: u32,
    char_index: u32,
) -> Result<TextBoxTargetView, CommandError> {
    with_open_document(&document_id, |entry| {
        target_for(entry, page_index, char_index)
    })
}

/// Style read from the first Text Object of a box. The replacement keeps all
/// of it, so an edit does not invent a new look.
struct StyleSnapshot {
    font_family: String,
    font_size: f32,
    fill_color: Option<PdfColor>,
    render_mode: PdfPageTextRenderMode,
    matrix: [f32; 6],
    font_is_embedded: bool,
    font_data: Option<Vec<u8>>,
}

fn read_style(objects: &PdfPageObjects<'_>, start: usize) -> Result<StyleSnapshot, CommandError> {
    let object = objects.get(start).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read Text Box object {start}: {error:?}"),
        )
    })?;
    let text_object = object.as_text_object().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::Internal,
            "the Text Box does not start with a text object",
        )
    })?;

    let font = text_object.font();
    let font_family = font.name();
    let font_is_embedded = font.is_embedded().unwrap_or(false);
    let font_data = if font_is_embedded {
        font.data().ok()
    } else {
        None
    };
    let font_size = text_object.unscaled_font_size().value;
    if !font_size.is_finite() || font_size <= 0.0 {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedFont,
            format!("the Text Box font size {font_size} cannot be edited"),
        ));
    }
    let matrix = text_object.matrix().map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read the Text Box matrix: {error:?}"),
        )
    })?;

    Ok(StyleSnapshot {
        font_family,
        font_size,
        fill_color: text_object.fill_color().ok(),
        render_mode: text_object.render_mode(),
        matrix: [
            matrix.a(),
            matrix.b(),
            matrix.c(),
            matrix.d(),
            matrix.e(),
            matrix.f(),
        ],
        font_is_embedded,
        font_data,
    })
}

/// Strips the `ABCDEF+` subset prefix a PDF may put in front of a font name.
fn base_font_name(family: &str) -> &str {
    match family.split_once('+') {
        Some((prefix, name)) if prefix.len() == 6 => name,
        _ => family,
    }
}

fn built_in_for(family: &str) -> Option<PdfFontBuiltin> {
    Some(match base_font_name(family) {
        "Helvetica" => PdfFontBuiltin::Helvetica,
        "Helvetica-Bold" => PdfFontBuiltin::HelveticaBold,
        "Helvetica-Oblique" => PdfFontBuiltin::HelveticaOblique,
        "Helvetica-BoldOblique" => PdfFontBuiltin::HelveticaBoldOblique,
        "Times-Roman" => PdfFontBuiltin::TimesRoman,
        "Times-Bold" => PdfFontBuiltin::TimesBold,
        "Times-Italic" => PdfFontBuiltin::TimesItalic,
        "Times-BoldItalic" => PdfFontBuiltin::TimesBoldItalic,
        "Courier" => PdfFontBuiltin::Courier,
        "Courier-Bold" => PdfFontBuiltin::CourierBold,
        "Courier-Oblique" => PdfFontBuiltin::CourierOblique,
        "Courier-BoldOblique" => PdfFontBuiltin::CourierBoldOblique,
        _ => return None,
    })
}

/// True for characters WinAnsiEncoding can represent, plus line breaks.
///
/// The editor only falls back to a base-14 Latin font when every replacement
/// character is in this set; anything else is refused instead of emitting
/// missing glyphs (.notdef boxes).
fn is_win_ansi_char(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r'
            | '\t'
            | '\u{20}'..='\u{7E}'
            | '\u{A0}'..='\u{FF}'
            | '\u{20AC}'
            | '\u{201A}'
            | '\u{0192}'
            | '\u{201E}'
            | '\u{2026}'
            | '\u{2020}'
            | '\u{2021}'
            | '\u{02C6}'
            | '\u{2030}'
            | '\u{0160}'
            | '\u{2039}'
            | '\u{0152}'
            | '\u{017D}'
            | '\u{2018}'
            | '\u{2019}'
            | '\u{201C}'
            | '\u{201D}'
            | '\u{2022}'
            | '\u{2013}'
            | '\u{2014}'
            | '\u{02DC}'
            | '\u{2122}'
            | '\u{0161}'
            | '\u{203A}'
            | '\u{0153}'
            | '\u{017E}'
            | '\u{0178}'
    )
}

enum FontChoice {
    Original,
    BuiltIn(PdfFontBuiltin),
}

struct FontDecision {
    choice: FontChoice,
    fallback: Option<FontUsageView>,
}

fn unsupported_font(family: &str, text: &str) -> CommandError {
    let missing: String = text
        .chars()
        .filter(|character| !is_win_ansi_char(*character))
        .take(8)
        .collect();
    CommandError::new(
        CommandErrorCode::UnsupportedFont,
        format!(
            "font {family} cannot represent the replacement text without missing glyphs \
             (unsupported characters: {missing:?}); CJK text editing is not supported yet"
        ),
    )
}

/// Chooses the font for a replacement, preserving the original when provably
/// safe and otherwise falling back to a base-14 Latin font.
fn decide_font(style: &StyleSnapshot, text: &str) -> Result<FontDecision, CommandError> {
    let characters: Vec<char> = text
        .chars()
        .filter(|character| !matches!(character, '\n' | '\r'))
        .collect();
    let all_win_ansi = characters
        .iter()
        .all(|character| is_win_ansi_char(*character));
    let original = style.font_family.clone();

    if style.font_is_embedded {
        let covered = style
            .font_data
            .as_deref()
            .and_then(|data| ttf_parser::Face::parse(data, 0).ok())
            .map(|face| {
                characters
                    .iter()
                    .all(|character| face.glyph_index(*character).is_some())
            })
            .unwrap_or(false);

        if covered {
            return Ok(FontDecision {
                choice: FontChoice::Original,
                fallback: None,
            });
        }

        if !all_win_ansi {
            return Err(unsupported_font(&original, text));
        }
        let reason = if style.font_data.is_some() {
            format!(
                "the embedded font {original} is missing glyphs for the new text; \
                 Helvetica was used instead"
            )
        } else {
            format!(
                "the embedded font {original} could not be inspected; Helvetica was used instead"
            )
        };
        return Ok(FontDecision {
            choice: FontChoice::BuiltIn(PdfFontBuiltin::Helvetica),
            fallback: Some(FontUsageView {
                original_family: original,
                family: "Helvetica".to_owned(),
                fallback: true,
                reason: Some(reason),
            }),
        });
    }

    if let Some(built_in) = built_in_for(&original) {
        if !all_win_ansi {
            return Err(unsupported_font(&original, text));
        }
        return Ok(FontDecision {
            choice: FontChoice::BuiltIn(built_in),
            fallback: None,
        });
    }

    if !all_win_ansi {
        return Err(unsupported_font(&original, text));
    }
    let reason =
        format!("the original font {original} is not embedded; Helvetica was used instead");
    Ok(FontDecision {
        choice: FontChoice::BuiltIn(PdfFontBuiltin::Helvetica),
        fallback: Some(FontUsageView {
            original_family: original,
            family: "Helvetica".to_owned(),
            fallback: true,
            reason: Some(reason),
        }),
    })
}

fn measure_with_token(
    document: &PdfDocument<'static>,
    token: PdfFontToken,
    font_size: f32,
    text: &str,
) -> Option<f32> {
    let detached = PdfPageTextObject::new(document, text, token, PdfPoints::new(font_size)).ok()?;
    let width = detached.bounds().ok()?.width().value;
    width.is_finite().then_some(width)
}

fn fits(width: Option<f32>, limit: f32) -> bool {
    width.map(|width| width <= limit + 0.01).unwrap_or(true)
}

fn break_long_word<F: Fn(&str) -> Option<f32>>(
    lines: &mut Vec<String>,
    current: &mut String,
    word: &str,
    width: f32,
    measure: &F,
) {
    for character in word.chars() {
        let mut candidate = current.clone();
        candidate.push(character);
        if !current.is_empty() && !fits(measure(&candidate), width) {
            lines.push(std::mem::take(current));
        }
        current.push(character);
    }
}

/// Greedy word wrap at `width` text-space points, keeping explicit line
/// breaks. A word wider than the box is broken between characters so no text
/// is dropped.
fn wrap_text<F: Fn(&str) -> Option<f32>>(text: &str, width: f32, measure: &F) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            // PDFium text objects cannot hold an empty string; a single space
            // preserves the blank line.
            lines.push(" ".to_owned());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split(' ') {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if fits(measure(&candidate), width) || (current.is_empty() && word.is_empty()) {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if fits(measure(word), width) {
                current = word.to_owned();
            } else {
                break_long_word(&mut lines, &mut current, word, width, measure);
            }
        }
        lines.push(current);
    }
    while lines.len() > 1 && lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(" ".to_owned());
    }
    lines
}

/// Everything needed to emit the text objects of one box.
#[derive(Clone, Copy)]
struct BoxRenderStyle {
    token: PdfFontToken,
    font_size: f32,
    color: PdfColor,
    render_mode: PdfPageTextRenderMode,
    underline: bool,
    alignment: TextAlignment,
}

fn rotate_local(x: f32, y: f32, radians: f32) -> (f32, f32) {
    let (sin, cos) = radians.sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

fn check_layout(layout: &BoxLayout) -> Result<(), CommandError> {
    if !layout.x.is_finite()
        || !layout.y.is_finite()
        || !layout.width.is_finite()
        || !layout.height.is_finite()
        || !layout.rotation.is_finite()
    {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "Text Box position, size and rotation must be finite numbers",
        ));
    }
    if layout.width < 4.0 {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "Text Box width must be at least 4 points",
        ));
    }
    if layout.height <= 0.0 {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "Text Box height must be positive",
        ));
    }
    Ok(())
}

fn content_height(lines: usize, font_size: f32) -> f32 {
    lines as f32 * font_size * 1.2
}

/// The height text is laid out into. Auto-height boxes grow to fit their
/// content and never shrink below their current height; manual boxes keep
/// exactly the height the user dragged.
fn effective_height(layout: &BoxLayout, content_height: f32) -> f32 {
    if layout.auto_height {
        layout.height.max(content_height)
    } else {
        layout.height
    }
}

/// Reads the font, size, colour and render mode of a legacy Text Object so a
/// transform can rebuild it without changing its look.
fn read_legacy_font(
    objects: &PdfPageObjects<'_>,
    start: usize,
) -> Result<BoxRenderStyle, CommandError> {
    let object = objects.get(start).map_err(|error| {
        CommandError::new(
            CommandErrorCode::Internal,
            format!("could not read the Text Box: {error:?}"),
        )
    })?;
    let text_object = object.as_text_object().ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::Internal,
            "the Text Box does not start with a text object",
        )
    })?;
    let font_size = text_object.unscaled_font_size().value;
    if !font_size.is_finite() || font_size <= 0.0 {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedFont,
            format!("the Text Box font size {font_size} cannot be edited"),
        ));
    }
    let token = text_object.font().token();
    let color = text_object.fill_color().unwrap_or(PdfColor::BLACK);
    let render_mode = text_object.render_mode();
    Ok(BoxRenderStyle {
        token,
        font_size,
        color,
        render_mode,
        underline: false,
        alignment: TextAlignment::Left,
    })
}

/// Emits the page objects for one box.
///
/// Text objects are placed with a pure rotation + translation matrix; the font
/// size lives on the object, matching PDF text-matrix semantics. Lines whose
/// line box does not fit inside a manually sized box are emitted with the
/// invisible render mode: they are clipped (not painted) but stay in the
/// document and in text extraction, and reappear when the box is enlarged.
fn build_box_objects(
    document: &mut PdfDocument<'static>,
    geometry: &DisplayGeometry,
    layout: &BoxLayout,
    render: BoxRenderStyle,
    lines: &[String],
    height: f32,
) -> Result<Vec<PdfPageObject<'static>>, CommandError> {
    let line_height = render.font_size * 1.2;
    let theta = layout.rotation.to_radians();
    let alpha = (geometry.rotation() as f32 - layout.rotation).to_radians();
    let (sin_alpha, cos_alpha) = alpha.sin_cos();
    let mut objects = Vec::new();

    for (line_number, line) in lines.iter().enumerate() {
        let line_width = measure_with_token(document, render.token, render.font_size, line)
            .unwrap_or(0.0)
            .max(0.0);
        let offset = match render.alignment {
            TextAlignment::Left => 0.0,
            TextAlignment::Center => ((layout.width - line_width) / 2.0).max(0.0),
            TextAlignment::Right => (layout.width - line_width).max(0.0),
        };
        let local_y = line_number as f32 * line_height + render.font_size * 0.8;
        let (local_x, local_y_rotated) = rotate_local(offset, local_y, theta);
        let (user_x, user_y) =
            geometry.point_inverse(layout.x + local_x, layout.y + local_y_rotated);

        let visible = (line_number as f32 + 1.0) * line_height <= height + 0.01;
        let mut object = PdfPageTextObject::new(
            document,
            line,
            render.token,
            PdfPoints::new(render.font_size),
        )
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not create Text Box text: {error:?}"),
            )
        })?;
        object.set_fill_color(render.color).map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not set the Text Box colour: {error:?}"),
            )
        })?;
        let render_mode = if visible {
            render.render_mode
        } else {
            PdfPageTextRenderMode::Invisible
        };
        if render_mode != PdfPageTextRenderMode::Unknown {
            object.set_render_mode(render_mode).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not set the Text Box render mode: {error:?}"),
                )
            })?;
        }
        object
            .apply_matrix(PdfMatrix::new(
                cos_alpha, sin_alpha, -sin_alpha, cos_alpha, user_x, user_y,
            ))
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not place Text Box text: {error:?}"),
                )
            })?;
        objects.push(PdfPageObject::from(object));

        if visible && render.underline && line_width > 0.0 {
            let underline_y = local_y + render.font_size * 0.12;
            let (start_x, start_y) = rotate_local(offset, underline_y, theta);
            let (end_x, end_y) = rotate_local(offset + line_width, underline_y, theta);
            let (user_start_x, user_start_y) =
                geometry.point_inverse(layout.x + start_x, layout.y + start_y);
            let (user_end_x, user_end_y) =
                geometry.point_inverse(layout.x + end_x, layout.y + end_y);
            let path = PdfPagePathObject::new_line(
                document,
                PdfPoints::new(user_start_x),
                PdfPoints::new(user_start_y),
                PdfPoints::new(user_end_x),
                PdfPoints::new(user_end_y),
                render.color,
                PdfPoints::new((render.font_size * 0.06).max(0.4)),
            )
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not create the Text Box underline: {error:?}"),
                )
            })?;
            objects.push(PdfPageObject::from(path));
        }
    }

    Ok(objects)
}

/// Display-space baseline origin of the first character belonging to a box.
fn first_baseline_point(
    layer: &crate::commands::TextLayerView,
    layer_raw_indices: &[usize],
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    record: &TextBoxRecord,
) -> Option<(f32, f32)> {
    let mut raw_indices = Vec::new();
    for object_index in record.start..record.start + record.count {
        let object = objects.get(object_index).ok()?;
        let text_object = object.as_text_object()?;
        let chars = page_text.chars_for_object(text_object).ok()?;
        raw_indices.extend(chars.iter().map(|character| character.index()));
    }
    raw_indices.sort_unstable();
    raw_indices.dedup();

    for (character, raw_index) in layer.chars.iter().zip(layer_raw_indices) {
        if character.width <= 0.0 || character.height <= 0.0 {
            continue;
        }
        if raw_indices.binary_search(raw_index).is_ok() {
            return Some((character.origin_x, character.origin_y));
        }
    }
    None
}

/// Materializes a canonical layout for a legacy Text Object when a style
/// change needs one, anchoring the first baseline so changing the font size
/// does not make the text jump.
fn style_change_layout(
    entry: &OpenDocument,
    page_index: u32,
    record_index: usize,
    font_size: f32,
) -> Result<BoxLayout, CommandError> {
    let record = &entry.edit.records[&page_index][record_index];
    if let Some(layout) = &record.layout {
        return Ok(layout.clone());
    }

    let page = load_page(&entry.document, page_index)?;
    let (layer, raw_indices) = build_text_layer_with_raw_indices(&page, page_index)?;
    let page_text = page.text().map_err(|_| no_editable_text(page_index))?;
    let objects = page.objects();
    let target = target_from_record(
        &layer,
        &raw_indices,
        &page_text,
        objects,
        record,
        page_index,
    )?;
    let baseline = first_baseline_point(&layer, &raw_indices, &page_text, objects, record)
        .unwrap_or((target.x, target.y));
    let rotation = normalize_angle(target.rotation);
    let (offset_x, offset_y) = rotate_local(0.0, font_size * 0.8, rotation.to_radians());

    Ok(BoxLayout {
        x: baseline.0 - offset_x,
        y: baseline.1 - offset_y,
        width: record.width.max(4.0),
        height: target.height.max(font_size * 1.2).max(1.0),
        rotation,
        auto_height: true,
    })
}

fn validate_box_range(
    objects: &PdfPageObjects<'_>,
    start: usize,
    count: usize,
) -> Result<(), CommandError> {
    // An empty Text Box occupies no page objects.
    if count == 0 {
        return Ok(());
    }
    if start + count > objects.len() {
        return Err(CommandError::new(
            CommandErrorCode::Internal,
            format!(
                "Text Box range {start}..{} is outside the page objects",
                start + count
            ),
        ));
    }
    for index in start..start + count {
        let object = objects.get(index).map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not read Text Box object {index}: {error:?}"),
            )
        })?;
        if index == start {
            if object.as_text_object().is_none() {
                return Err(CommandError::new(
                    CommandErrorCode::Internal,
                    "the Text Box does not start with a text object",
                ));
            }
        } else if object.as_text_object().is_none() && object.as_path_object().is_none() {
            // Underline strokes are path objects interleaved with the text.
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                format!("Text Box object {index} is neither text nor an underline"),
            ));
        }
    }
    Ok(())
}

fn find_record(session: &EditSession, page_index: u32, box_id: u64) -> Result<usize, CommandError> {
    session
        .records
        .get(&page_index)
        .and_then(|records| records.iter().position(|record| record.box_id == box_id))
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("no Text Box {box_id} on page {page_index}"),
            )
        })
}

/// Updates the stored box ranges after a box at `start` changed its object
/// count. Boxes after it move by the difference; the edited box itself keeps
/// its start.
///
/// When objects are inserted (`delta > 0`), a later box may already sit at
/// exactly `start` (this happens when undoing a deletion, where the deleted
/// box had count 0 before the undo), so an insertion shifts boxes at or after
/// `start`. A removal (`delta < 0`) only moves boxes strictly after it.
fn shift_later_boxes(records: &mut [TextBoxRecord], box_id: u64, start: usize, delta: isize) {
    if delta == 0 {
        return;
    }
    for record in records.iter_mut() {
        if record.box_id == box_id {
            continue;
        }
        let moves = if delta > 0 {
            record.start >= start
        } else {
            record.start > start
        };
        if moves {
            record.start = (record.start as isize + delta).max(0) as usize;
        }
    }
}

/// Replaces `remove_count` page objects at `start` with `insert_objects` and
/// returns the detached originals.
///
/// Undo/redo use this both to take the current objects off the page and to put
/// snapshot objects back at the same index, which preserves object order and
/// therefore text extraction order.
fn detach_and_insert(
    document: &PdfDocument<'static>,
    page_index: u32,
    start: usize,
    remove_count: usize,
    insert_objects: Vec<PdfPageObject<'static>>,
) -> Result<Vec<PdfPageObject<'static>>, CommandError> {
    let mut page = load_page(document, page_index)?;
    let objects = page.objects_mut();
    if start > objects.len() || (remove_count > 0 && start + remove_count > objects.len()) {
        return Err(CommandError::new(
            CommandErrorCode::Internal,
            format!(
                "the edit history refers to page objects {start}..{} but page {page_index} \
                 has {} objects",
                start + remove_count,
                objects.len()
            ),
        ));
    }

    let mut removed = Vec::with_capacity(remove_count);
    for _ in 0..remove_count {
        removed.push(objects.remove_object_at_index(start).map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not detach Text Box objects: {error:?}"),
            )
        })?);
    }
    for (offset, object) in insert_objects.into_iter().enumerate() {
        objects
            .insert_object_at_index(start + offset, object)
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not restore Text Box objects: {error:?}"),
                )
            })?;
    }
    Ok(removed)
}

/// Appends a committed change to the undo stack and advances the revision.
fn push_action(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    start: usize,
    before_record: Option<RecordSnapshot>,
    before_objects: Vec<PdfPageObject<'static>>,
    after_record: RecordSnapshot,
) {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry.edit.undo_stack.push(EditAction::Box(BoxEditAction {
        page_index,
        box_id,
        start,
        before_state,
        after_state,
        before_record,
        before_objects: Some(before_objects),
        after_record: Some(after_record),
        after_objects: None,
    }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
}

fn undo_for(entry: &mut OpenDocument) -> Result<EditStepView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let Some(EditAction::Box(mut action)) = entry.edit.undo_stack.pop() else {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "there is nothing to undo",
        ));
    };

    let before_count = action.before_count();
    let after_count = action.after_count();
    let restore = action.before_objects.take().unwrap_or_default();
    let detached = detach_and_insert(
        &entry.document,
        action.page_index,
        action.start,
        after_count,
        restore,
    )?;
    action.after_objects = Some(detached);

    let page_index = action.page_index;
    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("an edit action implies a record page");
        match &action.before_record {
            Some(before) => {
                let index = records
                    .iter()
                    .position(|record| record.box_id == action.box_id)
                    .ok_or_else(|| {
                        CommandError::new(
                            CommandErrorCode::Internal,
                            format!(
                                "the edit history lost Text Box {} on page {page_index}",
                                action.box_id
                            ),
                        )
                    })?;
                records[index].restore(before);
            }
            None => {
                // The change created this box; undo removes it again.
                records.retain(|record| record.box_id != action.box_id);
            }
        }
        shift_later_boxes(
            records,
            action.box_id,
            action.start,
            before_count as isize - after_count as isize,
        );
    }

    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Box(action));
    Ok(EditStepView {
        page_index,
        history: entry.edit.history_view(),
    })
}

fn redo_for(entry: &mut OpenDocument) -> Result<EditStepView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let Some(EditAction::Box(mut action)) = entry.edit.redo_stack.pop() else {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "there is nothing to redo",
        ));
    };

    let before_count = action.before_count();
    let after_count = action.after_count();
    let restore = action.after_objects.take().unwrap_or_default();
    let detached = detach_and_insert(
        &entry.document,
        action.page_index,
        action.start,
        before_count,
        restore,
    )?;
    action.before_objects = Some(detached);

    let page_index = action.page_index;
    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("an edit action implies a record page");
        match &action.after_record {
            Some(after) => {
                if let Some(index) = records
                    .iter()
                    .position(|record| record.box_id == action.box_id)
                {
                    records[index].restore(after);
                } else {
                    // Redo of a creation re-inserts the box at its old place in
                    // the record list.
                    let index = after.record_index.min(records.len());
                    records.insert(index, TextBoxRecord::from_snapshot(after));
                }
            }
            None => {
                records.retain(|record| record.box_id != action.box_id);
            }
        }
        shift_later_boxes(
            records,
            action.box_id,
            action.start,
            after_count as isize - before_count as isize,
        );
    }

    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Box(action));
    Ok(EditStepView {
        page_index,
        history: entry.edit.history_view(),
    })
}

fn apply_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    text: String,
) -> Result<TextEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (deleted, uses_style) = {
        let record = &entry.edit.records[&page_index][record_index];
        (record.deleted, record.style.is_some())
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }
    if uses_style {
        // Created boxes and legacy boxes converted to a built-in style use the
        // canonical layout path, which also permits clearing the text.
        return apply_to_styled_box(entry, page_index, record_index, normalized);
    }
    if normalized.trim().is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "replacement text is empty; delete the Text Box instead",
        ));
    }

    // Inspect the box without holding a borrow of the document that would
    // block loading a fallback font from its font collection.
    let (start, old_count, box_width, style, decision) = {
        let page = load_page(&entry.document, page_index)?;
        let objects = page.objects();
        let record = &entry.edit.records[&page_index][record_index];
        if record.count == 0 {
            return Err(CommandError::new(
                CommandErrorCode::InvalidArgument,
                "this Text Box has been deleted",
            ));
        }
        validate_box_range(objects, record.start, record.count)?;
        let style = read_style(objects, record.start)?;
        let decision = decide_font(&style, &normalized)?;
        (record.start, record.count, record.width, style, decision)
    };

    let built_in_token = match decision.choice {
        FontChoice::BuiltIn(built_in) => Some(entry.document.fonts_mut().new_built_in(built_in)),
        FontChoice::Original => None,
    };

    let mut page = load_page(&entry.document, page_index)?;
    let token = match built_in_token {
        Some(token) => token,
        None => {
            let objects = page.objects();
            let object = objects.get(start).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not read the Text Box: {error:?}"),
                )
            })?;
            let text_object = object.as_text_object().ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    "the Text Box does not start with a text object",
                )
            })?;
            let font = text_object.font();
            font.token()
        }
    };

    let lines = wrap_text(&normalized, box_width, &|candidate| {
        measure_with_token(&entry.document, token, style.font_size, candidate)
    });
    let line_height = style.font_size * 1.2;
    let [a, b, c, d, e, f] = style.matrix;
    let mut replacements: Vec<PdfPageObject<'static>> = Vec::with_capacity(lines.len());
    for (line_number, line) in lines.iter().enumerate() {
        let mut object = PdfPageTextObject::new(
            &entry.document,
            line,
            token,
            PdfPoints::new(style.font_size),
        )
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("could not create replacement text: {error:?}"),
            )
        })?;
        if let Some(color) = style.fill_color {
            object.set_fill_color(color).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not copy the text colour: {error:?}"),
                )
            })?;
        }
        if style.render_mode != PdfPageTextRenderMode::Unknown {
            object.set_render_mode(style.render_mode).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not copy the text render mode: {error:?}"),
                )
            })?;
        }
        let downward = line_number as f32 * line_height;
        object
            .apply_matrix(PdfMatrix::new(
                a,
                b,
                c,
                d,
                e - downward * c,
                f - downward * d,
            ))
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not place replacement text: {error:?}"),
                )
            })?;
        replacements.push(PdfPageObject::from(object));
    }

    let new_count = replacements.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let mut original_objects: Vec<PdfPageObject<'static>> = Vec::with_capacity(old_count);
    {
        let objects = page.objects_mut();
        for _ in 0..old_count {
            let removed = objects.remove_object_at_index(start).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not remove the original text: {error:?}"),
                )
            })?;
            original_objects.push(removed);
        }
        for (offset, object) in replacements.into_iter().enumerate() {
            objects
                .insert_object_at_index(start + offset, object)
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not insert replacement text: {error:?}"),
                    )
                })?;
        }
    }
    drop(page);

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        {
            let record = &mut records[record_index];
            record.count = new_count;
            record.text = normalized.clone();
        }
        shift_later_boxes(
            records,
            box_id,
            start,
            new_count as isize - old_count as isize,
        );
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    push_action(
        entry,
        page_index,
        box_id,
        start,
        Some(before_snapshot),
        original_objects,
        after_snapshot,
    );

    Ok(TextEditResultView {
        page_index,
        box_id,
        text: normalized,
        font: decision.fallback,
        history: entry.edit.history_view(),
    })
}

fn delete_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
) -> Result<TextEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, count, deleted) = {
        let record = &entry.edit.records[&page_index][record_index];
        (record.start, record.count, record.deleted)
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has already been deleted",
        ));
    }

    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let mut removed_objects: Vec<PdfPageObject<'static>> = Vec::with_capacity(count);
    if count > 0 {
        let mut page = load_page(&entry.document, page_index)?;
        {
            let objects = page.objects_mut();
            validate_box_range(objects, start, count)?;
            for _ in 0..count {
                let removed = objects.remove_object_at_index(start).map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not remove the Text Box: {error:?}"),
                    )
                })?;
                removed_objects.push(removed);
            }
        }
        drop(page);
    }

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        {
            let record = &mut records[record_index];
            record.count = 0;
            record.text.clear();
            record.deleted = true;
        }
        shift_later_boxes(records, box_id, start, -(count as isize));
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    push_action(
        entry,
        page_index,
        box_id,
        start,
        Some(before_snapshot),
        removed_objects,
        after_snapshot,
    );

    Ok(TextEditResultView {
        page_index,
        box_id,
        text: String::new(),
        font: None,
        history: entry.edit.history_view(),
    })
}

/// Reads the current target view of a box after a command mutated it.
fn read_target(
    entry: &OpenDocument,
    page_index: u32,
    box_id: u64,
) -> Result<TextBoxTargetView, CommandError> {
    let record = entry
        .edit
        .records
        .get(&page_index)
        .and_then(|records| records.iter().find(|record| record.box_id == box_id))
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("Text Box {box_id} disappeared from page {page_index}"),
            )
        })?;
    let page = load_page(&entry.document, page_index)?;
    let (layer, raw_indices) = build_text_layer_with_raw_indices(&page, page_index)?;
    let page_text = page.text().map_err(|_| no_editable_text(page_index))?;
    target_from_record(
        &layer,
        &raw_indices,
        &page_text,
        page.objects(),
        record,
        page_index,
    )
}

/// Applies text to a box with a canonical layout and a chosen built-in style.
fn apply_to_styled_box(
    entry: &mut OpenDocument,
    page_index: u32,
    record_index: usize,
    normalized: String,
) -> Result<TextEditResultView, CommandError> {
    let (box_id, start, old_count, style, layout) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.box_id,
            record.start,
            record.count,
            record.style.clone().ok_or_else(|| {
                CommandError::new(CommandErrorCode::Internal, "the Text Box lost its style")
            })?,
            record.layout.clone().ok_or_else(|| {
                CommandError::new(CommandErrorCode::Internal, "the Text Box lost its layout")
            })?,
        )
    };

    for character in normalized.chars() {
        if !is_win_ansi_char(character) {
            return Err(unsupported_font(&style.font_family, &normalized));
        }
    }

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    drop(page);

    let token = entry.document.fonts_mut().new_built_in(style.builtin());
    let render = BoxRenderStyle {
        token,
        font_size: style.font_size,
        color: style.pdf_color(),
        render_mode: PdfPageTextRenderMode::FilledUnstroked,
        underline: style.underline,
        alignment: style.alignment,
    };
    let lines = if normalized.is_empty() {
        Vec::new()
    } else {
        wrap_text(&normalized, layout.width, &|candidate| {
            measure_with_token(&entry.document, token, style.font_size, candidate)
        })
    };
    let height = effective_height(&layout, content_height(lines.len(), style.font_size));
    let replacements = build_box_objects(
        &mut entry.document,
        &geometry,
        &layout,
        render,
        &lines,
        height,
    )?;
    let new_count = replacements.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, replacements)?;

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        let record = &mut records[record_index];
        record.text = normalized.clone();
        record.count = new_count;
        record.width = layout.width;
        if layout.auto_height {
            record.layout = Some(BoxLayout { height, ..layout });
        }
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        shift_later_boxes(
            records,
            box_id,
            start,
            new_count as isize - old_count as isize,
        );
    }
    push_action(
        entry,
        page_index,
        box_id,
        start,
        Some(before_snapshot),
        removed,
        after_snapshot,
    );

    Ok(TextEditResultView {
        page_index,
        box_id,
        text: normalized,
        font: None,
        history: entry.edit.history_view(),
    })
}

fn create_for(
    entry: &mut OpenDocument,
    page_index: u32,
    request: CreateTextBoxRequest,
) -> Result<TextBoxEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let style = match &request.style {
        Some(view) => TextBoxStyle::from_view(view)?,
        None => TextBoxStyle::default_box(),
    };
    let layout = BoxLayout {
        x: request.x,
        y: request.y,
        width: request.width,
        height: request.height,
        rotation: 0.0,
        auto_height: true,
    };
    check_layout(&layout)?;

    // A scanned page has no text layer and the app offers no text editing on
    // it (the same rule as editing existing text). Inserting objects on a page
    // whose text cannot be extracted or addressed would be a dead end.
    let start = {
        let page = load_page(&entry.document, page_index)?;
        let (layer, _) = build_text_layer_with_raw_indices(&page, page_index)?;
        if !layer.has_text {
            return Err(no_editable_text(page_index));
        }
        page.objects().len()
    };

    entry.edit.next_box_id += 1;
    let box_id = entry.edit.next_box_id;
    let record_index = entry
        .edit
        .records
        .get(&page_index)
        .map(Vec::len)
        .unwrap_or(0);
    let record = TextBoxRecord {
        box_id,
        start,
        count: 0,
        width: layout.width,
        text: String::new(),
        layout: Some(layout),
        style: Some(style),
        created: true,
        deleted: false,
    };
    let after_snapshot = record.snapshot(record_index);
    entry
        .edit
        .records
        .entry(page_index)
        .or_default()
        .push(record);
    push_action(
        entry,
        page_index,
        box_id,
        start,
        None,
        Vec::new(),
        after_snapshot,
    );

    let target = read_target(entry, page_index, box_id)?;
    Ok(TextBoxEditResultView {
        page_index,
        target,
        font: None,
        history: entry.edit.history_view(),
    })
}

fn boxes_for(
    entry: &OpenDocument,
    page_index: u32,
) -> Result<Vec<TextBoxTargetView>, CommandError> {
    load_page(&entry.document, page_index)?;
    let box_ids: Vec<u64> = entry
        .edit
        .records
        .get(&page_index)
        .map(|records| {
            records
                .iter()
                .filter(|record| record.created && !record.deleted)
                .map(|record| record.box_id)
                .collect()
        })
        .unwrap_or_default();
    box_ids
        .into_iter()
        .map(|box_id| read_target(entry, page_index, box_id))
        .collect()
}

fn style_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    style_view: TextBoxStyleView,
) -> Result<TextBoxEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let new_style = TextBoxStyle::from_view(&style_view)?;
    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, old_count, text, deleted) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.start,
            record.count,
            record.text.clone(),
            record.deleted,
        )
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }
    for character in text.chars() {
        if !is_win_ansi_char(character) {
            return Err(unsupported_font(&new_style.font_family, &text));
        }
    }

    let layout = style_change_layout(entry, page_index, record_index, new_style.font_size)?;
    check_layout(&layout)?;

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    drop(page);

    let token = entry.document.fonts_mut().new_built_in(new_style.builtin());
    let render = BoxRenderStyle {
        token,
        font_size: new_style.font_size,
        color: new_style.pdf_color(),
        render_mode: PdfPageTextRenderMode::FilledUnstroked,
        underline: new_style.underline,
        alignment: new_style.alignment,
    };
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        wrap_text(&text, layout.width, &|candidate| {
            measure_with_token(&entry.document, token, new_style.font_size, candidate)
        })
    };
    let height = effective_height(&layout, content_height(lines.len(), new_style.font_size));
    let replacements = build_box_objects(
        &mut entry.document,
        &geometry,
        &layout,
        render,
        &lines,
        height,
    )?;
    let new_count = replacements.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, replacements)?;

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        let record = &mut records[record_index];
        record.style = Some(new_style);
        record.layout = Some(BoxLayout { height, ..layout });
        record.width = layout.width;
        record.count = new_count;
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        shift_later_boxes(
            records,
            box_id,
            start,
            new_count as isize - old_count as isize,
        );
    }
    push_action(
        entry,
        page_index,
        box_id,
        start,
        Some(before_snapshot),
        removed,
        after_snapshot,
    );

    let target = read_target(entry, page_index, box_id)?;
    Ok(TextBoxEditResultView {
        page_index,
        target,
        font: None,
        history: entry.edit.history_view(),
    })
}

fn transform_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    request: TextBoxTransformRequest,
) -> Result<TextBoxEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let layout = BoxLayout {
        x: request.x,
        y: request.y,
        width: request.width,
        height: request.height,
        rotation: normalize_angle(request.rotation),
        auto_height: request.auto_height,
    };
    check_layout(&layout)?;

    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, old_count, text, deleted, style) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.start,
            record.count,
            record.text.clone(),
            record.deleted,
            record.style.clone(),
        )
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    let legacy_render = if style.is_none() && old_count > 0 {
        Some(read_legacy_font(page.objects(), start)?)
    } else {
        None
    };
    drop(page);

    let render = match &style {
        Some(style) => BoxRenderStyle {
            token: entry.document.fonts_mut().new_built_in(style.builtin()),
            font_size: style.font_size,
            color: style.pdf_color(),
            render_mode: PdfPageTextRenderMode::FilledUnstroked,
            underline: style.underline,
            alignment: style.alignment,
        },
        None => legacy_render.ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                "the Text Box lost its font while being transformed",
            )
        })?,
    };
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        wrap_text(&text, layout.width, &|candidate| {
            measure_with_token(&entry.document, render.token, render.font_size, candidate)
        })
    };
    let height = effective_height(&layout, content_height(lines.len(), render.font_size));
    let replacements = build_box_objects(
        &mut entry.document,
        &geometry,
        &layout,
        render,
        &lines,
        height,
    )?;
    let new_count = replacements.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, replacements)?;

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        let record = &mut records[record_index];
        record.layout = Some(BoxLayout { height, ..layout });
        record.width = layout.width;
        record.count = new_count;
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        shift_later_boxes(
            records,
            box_id,
            start,
            new_count as isize - old_count as isize,
        );
    }
    push_action(
        entry,
        page_index,
        box_id,
        start,
        Some(before_snapshot),
        removed,
        after_snapshot,
    );

    let target = read_target(entry, page_index, box_id)?;
    Ok(TextBoxEditResultView {
        page_index,
        target,
        font: None,
        history: entry.edit.history_view(),
    })
}

/// The `apply_text_edit` command: replaces a Text Box's content, wrapping at
/// its width and keeping the original baseline, font, size and colour when
/// possible.
#[tauri::command]
pub fn apply_text_edit(
    document_id: String,
    page_index: u32,
    box_id: u64,
    text: String,
) -> Result<TextEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        apply_for(entry, page_index, box_id, text)
    })
}

/// The `delete_text_box` command: removes a Text Box and its original page
/// objects from the page content.
#[tauri::command]
pub fn delete_text_box(
    document_id: String,
    page_index: u32,
    box_id: u64,
) -> Result<TextEditResultView, CommandError> {
    with_open_document(&document_id, |entry| delete_for(entry, page_index, box_id))
}

/// The `create_text_box` command: adds an empty, visible Text Box and records
/// its creation in the document-wide undo history.
#[tauri::command]
pub fn create_text_box(
    document_id: String,
    page_index: u32,
    request: CreateTextBoxRequest,
) -> Result<TextBoxEditResultView, CommandError> {
    with_open_document(&document_id, |entry| create_for(entry, page_index, request))
}

/// The `text_boxes` command: every Text Box this session created on the page,
/// including empty ones, in creation order. Deleted boxes are excluded.
#[tauri::command]
pub fn text_boxes(
    document_id: String,
    page_index: u32,
) -> Result<Vec<TextBoxTargetView>, CommandError> {
    with_open_document(&document_id, |entry| boxes_for(entry, page_index))
}

/// The `apply_text_box_style` command: replaces the whole-box style and keeps
/// the text and box content reflowing inside it.
#[tauri::command]
pub fn apply_text_box_style(
    document_id: String,
    page_index: u32,
    box_id: u64,
    style: TextBoxStyleView,
) -> Result<TextBoxEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        style_for(entry, page_index, box_id, style)
    })
}

/// The `transform_text_box` command: moves, resizes or rotates a Text Box.
#[tauri::command]
pub fn transform_text_box(
    document_id: String,
    page_index: u32,
    box_id: u64,
    transform: TextBoxTransformRequest,
) -> Result<TextBoxEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        transform_for(entry, page_index, box_id, transform)
    })
}

/// The `edit_history` command: document-wide undo/redo availability.
#[tauri::command]
pub fn edit_history(document_id: String) -> Result<EditHistoryView, CommandError> {
    with_open_document(&document_id, |entry| Ok(entry.edit.history_view()))
}

/// The `undo_text_edit` command: reverts the most recent text edit or deletion.
#[tauri::command]
pub fn undo_text_edit(document_id: String) -> Result<EditStepView, CommandError> {
    with_open_document(&document_id, undo_for)
}

/// The `redo_text_edit` command: re-applies the most recently undone edit.
#[tauri::command]
pub fn redo_text_edit(document_id: String) -> Result<EditStepView, CommandError> {
    with_open_document(&document_id, redo_for)
}

/// The `mark_document_saved` command: records that the current state is on
/// disk.
///
/// This is the contract the save pipeline (ticket 10) must call *after* a
/// successful write. It sets the clean baseline without clearing the undo/redo
/// stacks, so history survives Save Changes and the user can undo past the
/// save point. `save_document_bytes` serializes the live in-memory document
/// for that pipeline without reloading it, so the same history stays valid.
#[tauri::command]
pub fn mark_document_saved(document_id: String) -> Result<EditHistoryView, CommandError> {
    with_open_document(&document_id, |entry| {
        entry.edit.saved_state = entry.edit.current_state;
        Ok(entry.edit.history_view())
    })
}
