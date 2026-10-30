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

use crate::annotations::{
    ensure_annotation_editable, remove_annotation_named, restore_annotation_snapshot,
    AnnotationSnapshot,
};
use crate::commands::{
    build_text_layer_with_raw_indices, load_page, with_open_document, CommandError,
    CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::fonts;
use crate::system_fonts;

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
    /// A family advertised by [`text_box_fonts`]: one of the base-14
    /// families, a bundled OFL face, or a font installed on this machine.
    pub font_family: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    #[serde(default)]
    pub strikeout: bool,
    /// Fill colour as `#rrggbb`.
    pub color: String,
    pub alignment: TextAlignment,
}

/// One font the Text Box editor may use.
///
/// The metadata is deliberately explicit so the style panel can describe each
/// family honestly: `source` distinguishes PDF base-14 fonts, bundled font
/// programs and fonts installed on this machine, `embedded` says whether a
/// program must be written into the PDF, and `coverage` states which
/// characters the font can render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxFontView {
    pub family: String,
    pub label: String,
    /// For base-14 entries, this variant's weight; for bundled and installed
    /// families, whether a bold face or weight axis is available.
    pub bold: bool,
    /// For base-14 entries, this variant's slant; for installed families,
    /// whether an italic face is available. Bundled families have none.
    pub italic: bool,
    pub postscript_name: String,
    /// `base14` for the built-in fonts, `embedded` for the bundled font
    /// programs, `wps` for fonts shipped by WPS Office and `system` for other
    /// fonts installed on this machine.
    pub source: String,
    /// True when the font program is embedded in the saved PDF.
    pub embedded: bool,
    /// True when the project may freely embed the font. Installed fonts are
    /// only offered when their `fsType` permits subsetting.
    pub embeddable: bool,
    /// `win_ansi` for base-14, `cjk` for the bundled family, `system` for
    /// installed fonts.
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

/// The PDF base-14 Latin families and their four variants. They need no font
/// program in the document, so using them is always within the project's
/// embedding policy (ADR-0006). The bundled Noto Sans SC entry is added
/// separately by [`text_box_fonts`].
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
/// The base-14 fonts need no program in the document and cover WinAnsi. The
/// bundled CJK faces are SIL OFL 1.1 fonts embedded as per-Text-Box subsets,
/// which is what makes Chinese and mixed Chinese/Latin text editable. Fonts
/// installed on this machine (including WPS Office's 汉仪 and 书宋/仿宋 faces)
/// are listed last when their embedding permission allows a subset, so a
/// document that names 微软雅黑 or 宋体 can keep its intended typeface without
/// PDFX redistributing the font. `coverage` tells the panel what each font can
/// render.
#[tauri::command]
pub fn text_box_fonts() -> Vec<TextBoxFontView> {
    let mut fonts: Vec<TextBoxFontView> = BASE14_FONTS
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
        .collect();
    for bundled in crate::fonts::bundled_fonts() {
        fonts.push(TextBoxFontView {
            family: bundled.family.to_owned(),
            label: bundled.label.to_owned(),
            // The bundled fonts have no italic face; the Noto faces are
            // variable and honour bold, while the static faces keep their
            // regular outlines for bold text. Refusing italic in
            // `TextBoxStyle::from_view` is clearer than dropping it.
            bold: false,
            italic: false,
            postscript_name: bundled.postscript_name.to_owned(),
            source: "embedded".to_owned(),
            embedded: true,
            embeddable: true,
            coverage: "cjk".to_owned(),
        });
    }
    // Fonts already installed on this machine (including the faces WPS Office
    // ships) are read from their original location and only their subset is
    // written into the document. This is what lets a document that names
    // 微软雅黑, 宋体 or a 汉仪 face keep its intended typeface without PDFX
    // redistributing any proprietary font.
    for installed in system_fonts::catalog().families() {
        fonts.push(TextBoxFontView {
            family: installed.family.clone(),
            label: installed.label.clone(),
            bold: installed.bold,
            italic: installed.italic,
            postscript_name: installed.postscript_name.clone(),
            source: installed.source.to_owned(),
            embedded: false,
            embeddable: true,
            coverage: "system".to_owned(),
        });
    }
    fonts
}

fn base14_font(family: &str, bold: bool, italic: bool) -> Option<&'static Base14Font> {
    BASE14_FONTS
        .iter()
        .find(|font| font.family == family && font.bold == bold && font.italic == italic)
}

fn builtin_for_style(family: &str, bold: bool, italic: bool) -> Option<PdfFontBuiltin> {
    base14_font(family, bold, italic).map(|font| font.builtin)
}

/// True when PDFX can reproduce `family` from its own registry: a base-14
/// family or one of the bundled CJK faces. Installed fonts are deliberately
/// not advertised here: a Text Box painted with a document's embedded program
/// keeps offering that exact program ("原字体"), even when a macOS or WPS copy
/// of the same family is installed and could be used for new text. The
/// installed copy takes over only when the original cannot paint a character,
/// and the canvas reports that as a fallback.
fn font_is_advertised(family: &str) -> bool {
    fonts::is_bundled_family(family) || built_in_for(base_font_name(family)).is_some()
}

fn validate_font_size(size: f32) -> Result<(), CommandError> {
    if !size.is_finite() || !(4.0..=400.0).contains(&size) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("font size {size} is outside 4..400 points"),
        ));
    }
    Ok(())
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
    pub(crate) strikeout: bool,
    pub(crate) color: (u8, u8, u8),
    pub(crate) alignment: TextAlignment,
}

/// One styled character range inside a Text Box.
///
/// Offsets are Unicode scalar values into the box text, the same unit the
/// frontend converts its textarea selection to. Runs are kept sorted,
/// non-overlapping and merged when adjacent ranges share a style; text not
/// covered by a run uses the box's base style.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextBoxRun {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) style: TextBoxStyle,
}

/// A character range in a Text Box, as the selection style command receives it.
///
/// Offsets count Unicode scalar values, not UTF-16 code units: a selection that
/// spans an emoji or other astral character counts it once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRangeView {
    pub start: u32,
    pub end: u32,
}

/// A styled range as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxRunView {
    pub start: u32,
    pub end: u32,
    pub font_family: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    #[serde(default)]
    pub strikeout: bool,
    /// Fill colour as `#rrggbb`.
    pub color: String,
}

impl TextBoxStyle {
    fn default_box() -> Self {
        Self {
            font_family: "Helvetica".to_owned(),
            font_size: 16.0,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            color: (0, 0, 0),
            alignment: TextAlignment::Left,
        }
    }

    fn from_view(view: &TextBoxStyleView) -> Result<Self, CommandError> {
        let family = view.font_family.trim();
        let bundled = fonts::is_bundled_family(family);
        let base14 = builtin_for_style(family, view.bold, view.italic).is_some();
        let installed = system_fonts::catalog();
        if bundled {
            // The bundled families have no italic face. The Noto companions
            // have a weight axis; Zhuque Fangsong is regular-only, so bold
            // text keeps its regular outlines. Refusing italic is clearer
            // than dropping it.
            if view.italic {
                return Err(CommandError::new(
                    CommandErrorCode::UnsupportedFont,
                    format!(
                        "font {} has no italic variant; clear italic or choose a base-14 italic font",
                        fonts::normalize_family(family)
                    ),
                ));
            }
        } else if !base14 && installed.has_family(family) {
            // Installed families are used exactly as the machine provides
            // them. Italic needs a real italic face; bold may fall back to the
            // regular outlines when the family has no bold face.
            if view.italic && !installed.has_italic(family) {
                return Err(CommandError::new(
                    CommandErrorCode::UnsupportedFont,
                    format!(
                        "font {} has no italic variant installed; clear italic or choose \
                         another font",
                        installed.family_for_name(family).unwrap_or(family)
                    ),
                ));
            }
        } else if !base14 {
            let mut available: Vec<&str> = BASE14_FONTS.iter().map(|font| font.label).collect();
            available.extend(crate::fonts::bundled_fonts().iter().map(|font| font.label));
            return Err(CommandError::new(
                CommandErrorCode::UnsupportedFont,
                format!(
                    "font {:?} is not available; use one of the built-in fonts: {}",
                    view.font_family,
                    available.join(", ")
                ),
            ));
        }
        validate_font_size(view.font_size)?;
        Ok(Self {
            font_family: if bundled || installed.has_family(family) {
                fonts::normalize_family(family)
            } else {
                family.to_owned()
            },
            font_size: view.font_size,
            bold: view.bold,
            italic: view.italic,
            underline: view.underline,
            strikeout: view.strikeout,
            color: parse_color(&view.color)?,
            alignment: view.alignment,
        })
    }

    fn is_bundled(&self) -> bool {
        fonts::is_bundled_family(&self.font_family)
    }

    fn is_installed(&self) -> bool {
        system_fonts::catalog().has_family(&self.font_family)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
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
    #[serde(default)]
    pub strikeout: bool,
    pub alignment: TextAlignment,
    /// Clockwise rotation in degrees in display space.
    pub rotation: f32,
    /// True for boxes whose height follows their content. Manual resizing
    /// turns this off.
    pub auto_height: bool,
    /// True when this Text Box was created by the Text Box tool rather than
    /// being an original Text Object. Created boxes may be empty.
    pub created: bool,
    /// True when the box still paints with the PDF's original embedded font,
    /// which PDFX can preserve while only the size or colour changes. The
    /// style panel then offers the original font as a choice and disables the
    /// controls that need a replacement font.
    #[serde(default)]
    pub keep_original_font: bool,
    /// Styled character ranges inside `text`, sorted by start offset. Empty
    /// when the whole box uses the base style above.
    #[serde(default)]
    pub runs: Vec<TextBoxRunView>,
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
    /// Visible page count after the step. Page deletion, insertion and undo/
    /// redo of them can change it without a separate document reload.
    pub page_count: u32,
    /// True when page order, count or rotation changed, so caches keyed by
    /// page index (page metadata, thumbnails) must be refreshed.
    pub page_structure_changed: bool,
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
    pub(crate) line_height: Option<f32>,
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
    pub(crate) runs: Vec<TextBoxRun>,
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

/// One committed image change (insertion, transform or deletion).
///
/// The page-object snapshots work exactly like [`BoxEditAction`]: the detached
/// "before" objects live in the action while it is on the undo stack, and the
/// two snapshots are swapped when the action moves to the other stack.
pub(crate) struct ImageEditAction {
    pub(crate) page_index: u32,
    pub(crate) image_id: u64,
    pub(crate) start: usize,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before_record: Option<crate::image_edit::ImageSnapshot>,
    pub(crate) before_objects: Option<Vec<PdfPageObject<'static>>>,
    pub(crate) after_record: Option<crate::image_edit::ImageSnapshot>,
    pub(crate) after_objects: Option<Vec<PdfPageObject<'static>>>,
}

/// One committed line or arrow change (creation, transform, restyle or
/// deletion).
///
/// Lines are page content objects, so they use the same detached page-object
/// snapshot model as [`ImageEditAction`]: the snapshots hold the path objects
/// that were on the page before and after the change, which is what lets undo
/// restore the exact stroke without re-deriving it.
pub(crate) struct LineEditAction {
    pub(crate) page_index: u32,
    pub(crate) line_id: u64,
    pub(crate) start: usize,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before_record: Option<crate::line_edit::LineSnapshot>,
    pub(crate) before_objects: Option<Vec<PdfPageObject<'static>>>,
    pub(crate) after_record: Option<crate::line_edit::LineSnapshot>,
    pub(crate) after_objects: Option<Vec<PdfPageObject<'static>>>,
}

/// A translation keeps the original text objects, glyph spacing, and fonts.
/// Exact matrices (rather than inverse deltas) make undo pixel-stable.
pub(crate) struct TextMoveAction {
    page_index: u32,
    box_id: u64,
    before_state: u64,
    after_state: u64,
    before_matrices: Vec<PdfMatrix>,
    after_matrices: Vec<PdfMatrix>,
    before_layout: Option<BoxLayout>,
    after_layout: Option<BoxLayout>,
}

pub(crate) enum EditAction {
    Box(BoxEditAction),
    TextMove(TextMoveAction),
    Image(ImageEditAction),
    Line(LineEditAction),
    Field(FieldEditAction),
    FormStructure(FormStructureEditAction),
    Annotation(Box<AnnotationEditAction>),
    Link(Box<LinkEditAction>),
    /// Rotation, deletion, reordering or blank-page insertion.
    Page(crate::page_ops::PageEditAction),
    /// Insertion or deletion of an embedded-file attachment.
    Attachment(crate::attachments::AttachmentEditAction),
    /// Apply, replace or delete the session's watermark.
    Watermark(crate::watermark::WatermarkEditAction),
    /// Change one optional-content layer's visibility or name, or delete it.
    Layer(crate::layers::LayerEditAction),
}

/// The value a field edit restores during undo or redo.
///
/// Field edits do not snapshot page objects; PDFium stores a widget's value in
/// the annotation dictionary, so restoring the recorded value is enough.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldValueSnapshot {
    Text(String),
    Checked(bool),
    Choice(String),
    /// The radio group's selected on-state name, or `None` when unselected.
    Radio(Option<String>),
}

pub(crate) struct FieldEditAction {
    pub(crate) field_id: String,
    pub(crate) field_name: String,
    /// Set for radio widgets: the group whose value the action changes.
    pub(crate) radio_group: Option<String>,
    pub(crate) page_index: u32,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: FieldValueSnapshot,
    pub(crate) after: FieldValueSnapshot,
    /// True when the value was written into the PDFium document rather than
    /// recorded only in the history overlay. Undo/redo must apply the snapshot
    /// the same way the original fill did.
    pub(crate) pdfium_write: bool,
}

/// One committed change to a form field this session created.
///
/// Creation, property updates and deletion all use the same shape: `before`
/// is `None` when the action created the field, `after` is `None` when it
/// removed the field. Created fields do not exist in the PDFium document until
/// the next save, so the action snapshots the whole session record (geometry,
/// properties, value) and undo/redo restores it directly.
pub(crate) struct FormStructureEditAction {
    pub(crate) uid: u64,
    pub(crate) page_index: u32,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: Option<crate::form_create::CreatedFieldSnapshot>,
    pub(crate) after: Option<crate::form_create::CreatedFieldSnapshot>,
}

/// One committed annotation change.
///
/// Unlike a Text Box action, both snapshots stay on the action while it moves
/// between the undo and redo stacks: applying a snapshot always removes the
/// current annotation with the same name and recreates it from the snapshot.
pub(crate) struct AnnotationEditAction {
    pub(crate) page_index: u32,
    pub(crate) name: String,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    /// State before the change; `None` when the change created the annotation.
    pub(crate) before: Option<AnnotationSnapshot>,
    /// State after the change; `None` when the change deleted the annotation.
    pub(crate) after: Option<AnnotationSnapshot>,
}

/// One committed link change (creation, target edit or deletion).
///
/// Links use the same remove-and-recreate model as annotations: PDFium cannot
/// change a link action in place, so both snapshots stay on the action and
/// applying one always replaces the link with the recorded state.
pub(crate) struct LinkEditAction {
    pub(crate) page_index: u32,
    pub(crate) name: String,
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    /// State before the change; `None` when the change created the link.
    pub(crate) before: Option<crate::links::LinkSnapshot>,
    /// State after the change; `None` when the change deleted the link.
    pub(crate) after: Option<crate::links::LinkSnapshot>,
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
    /// Styled character ranges on top of the base style. Stored sorted and
    /// non-overlapping; only styled boxes (with `layout` and `style`) use them.
    pub(crate) runs: Vec<TextBoxRun>,
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
            runs: self.runs.clone(),
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
        self.runs = snapshot.runs.clone();
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
            runs: snapshot.runs.clone(),
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
    /// Images inserted by this session, per page.
    pub(crate) images: HashMap<u32, Vec<crate::image_edit::ImageRecord>>,
    pub(crate) next_image_id: u64,
    /// Lines and arrows drawn by this session, per page. They are page
    /// content objects, not annotations, so they live beside images and Text
    /// Boxes rather than in the annotation dictionary.
    pub(crate) lines: HashMap<u32, Vec<crate::line_edit::LineRecord>>,
    pub(crate) next_line_id: u64,
    /// Text fields created by this session; materialized into the saved output
    /// by the save pipeline and merged into discovery until then.
    pub(crate) created_fields: crate::form_create::CreatedFieldStore,
    pub(crate) undo_stack: Vec<EditAction>,
    pub(crate) redo_stack: Vec<EditAction>,
    /// Id of the document state currently on the page.
    pub(crate) current_state: u64,
    /// Monotonic counter; every committed edit gets a fresh state id.
    pub(crate) next_state: u64,
    /// State id last written to disk by the save pipeline.
    pub(crate) saved_state: u64,
    /// Bundled-font subsets loaded into this Document, keyed by weight and
    /// character set. Reusing a token prevents a repeated edit from embedding
    /// the same subset twice.
    pub(crate) bundled_fonts: fonts::BundledFontCache,
    /// The one watermark this session has applied, if any. Its page-object
    /// ranges are updated by [`shift_object_indexes`] when other edits move
    /// page objects.
    pub(crate) watermark: Option<crate::watermark::WatermarkState>,
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

    /// Records that the current state is on disk. The undo/redo stacks are
    /// deliberately untouched, so history survives a save and the user can
    /// undo past the new baseline.
    pub(crate) fn mark_document_saved(&mut self) {
        self.saved_state = self.current_state;
    }
}

fn no_editable_text(page_index: u32) -> CommandError {
    CommandError::new(
        CommandErrorCode::NoEditableText,
        format!("page {page_index} has no editable text"),
    )
}

pub(crate) fn ensure_content_editable(document: &PdfDocument<'_>) -> Result<(), CommandError> {
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
) -> Result<(String, f32, String, bool), CommandError> {
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
    let font = text_object.font();
    let font_family = fonts::normalize_family(&font.name());
    let font_is_embedded = font.is_embedded().unwrap_or(false);
    let font_size = display_font_size(
        text_object.unscaled_font_size().value,
        text_object
            .matrix()
            .map(|matrix| {
                [
                    matrix.a(),
                    matrix.b(),
                    matrix.c(),
                    matrix.d(),
                    matrix.e(),
                    matrix.f(),
                ]
            })
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    );
    let color = color_hex(text_object);
    Ok((font_family, font_size, color, font_is_embedded))
}

fn run_views(record: &TextBoxRecord) -> Vec<TextBoxRunView> {
    record
        .runs
        .iter()
        .map(|run| TextBoxRunView {
            start: run.start as u32,
            end: run.end as u32,
            font_family: run.style.font_family.clone(),
            font_size: run.style.font_size,
            bold: run.style.bold,
            italic: run.style.italic,
            underline: run.style.underline,
            strikeout: run.style.strikeout,
            color: run.style.color_hex(),
        })
        .collect()
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
        let (font_family, font_size, color, font_is_embedded) = match &record.style {
            Some(style) => (
                style.font_family.clone(),
                style.font_size,
                style.color_hex(),
                false,
            ),
            None => first_text_metadata(objects, record.start)?,
        };
        let keep_original_font =
            record.style.is_none() && font_is_embedded && !font_is_advertised(&font_family);
        let (bold, italic) = match &record.style {
            Some(style) => (style.bold, style.italic),
            None => style_flags_from_font_name(&font_family),
        };
        let underline = record
            .style
            .as_ref()
            .map(|style| style.underline)
            .unwrap_or(false);
        let strikeout = record
            .style
            .as_ref()
            .map(|style| style.strikeout)
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
            line_height: layout.line_height.map(|height| height.max(font_size * 1.2)),
            color,
            x: layout.x,
            y: layout.y,
            width: layout.width,
            height: layout.height,
            bold,
            italic,
            underline,
            strikeout,
            alignment,
            rotation: layout.rotation,
            auto_height: layout.auto_height,
            created: record.created,
            keep_original_font,
            runs: run_views(record),
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
    let font_name = font.name();
    let font_family = fonts::normalize_family(&font_name);
    let keep_original_font =
        font.is_embedded().unwrap_or(false) && !font_is_advertised(&font_family);
    let (bold, italic) = style_flags_from_font_name(&font_name);
    let matrix = text_object
        .matrix()
        .map(|matrix| {
            [
                matrix.a(),
                matrix.b(),
                matrix.c(),
                matrix.d(),
                matrix.e(),
                matrix.f(),
            ]
        })
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let font_size = display_font_size(text_object.unscaled_font_size().value, matrix);
    let color = color_hex(text_object);
    let rotation = display_rotation_from_matrix(matrix, layer.rotation);
    let (x, y, _ink_width, height) = legacy_box_rect(x, y, width, height, rotation);

    Ok(TextBoxTargetView {
        box_id: record.box_id,
        page_index,
        text: record.text.clone(),
        font_family,
        font_size,
        line_height: None,
        color,
        x,
        y,
        // The editor must use the same fixed width as native word wrapping.
        // Ink bounds shrink when text is shortened; the editing frame must not.
        width: record.width,
        height,
        bold,
        italic,
        underline: false,
        strikeout: false,
        alignment: TextAlignment::Left,
        rotation,
        // A legacy Text Object is sized by its own glyphs; moving or rotating
        // it must keep every line painted instead of clipping at the loose
        // text-layer height.
        auto_height: true,
        created: record.created,
        keep_original_font,
        runs: run_views(record),
    })
}

/// A legacy Text Object is measured as an axis-aligned display rect, while a
/// Text Box stores its geometry in its own rotated frame. For quarter turns
/// the two are the same rectangle expressed around a different corner, so
/// convert the rect: its width and height swap at 90/270 and the origin moves
/// to the corner the model rotates about. Other angles keep the axis-aligned
/// approximation the text editor overlay already used.
fn legacy_box_rect(x: f32, y: f32, width: f32, height: f32, rotation: f32) -> (f32, f32, f32, f32) {
    let angle = normalize_angle(rotation);
    if (angle - 90.0).abs() < 0.01 {
        (x + width, y, height, width)
    } else if (angle + 90.0).abs() < 0.01 {
        (x, y + height, height, width)
    } else if (angle.abs() - 180.0).abs() < 0.01 {
        (x + width, y + height, width, height)
    } else {
        (x, y, width, height)
    }
}

/// One page object's contribution to an editable Text Block, in display space.
#[derive(Clone)]
struct BlockObject {
    text: String,
    font_name: String,
    font_size: f32,
    color: String,
    rotation: f32,
    baseline: f32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

/// PDFium collapses repeated spaces in its reading-order text. Reconstruct
/// exact integral space advances from this object's own glyph origins, so
/// reopening an edited box does not turn two saved spaces into one.
fn object_text_with_spaces(object: &PdfPageTextObject<'_>, page_text: &PdfPageText<'_>) -> String {
    let text = object.text();
    if !text.contains(' ') {
        return text;
    }
    let Ok(chars) = page_text.chars_for_object(object) else {
        return text;
    };
    let chars: Vec<_> = chars.iter().collect();
    let Ok(matrix) = object.matrix() else {
        return text;
    };
    let norm = matrix.a() * matrix.a() + matrix.b() * matrix.b();
    let Some(advance) = object
        .font()
        .character_advance(' ', object.unscaled_font_size())
    else {
        return text;
    };
    if norm <= 0.0 || advance.value <= 0.0 {
        return text;
    }
    let mut counts = Vec::new();
    for (index, character) in chars.iter().enumerate() {
        if character.unicode_char() != Some(' ') {
            continue;
        }
        let count = chars
            .get(index + 1)
            .and_then(|next| {
                let (x, y) = character.origin().ok()?;
                let (nx, ny) = next.origin().ok()?;
                let dx = nx.value - x.value;
                let dy = ny.value - y.value;
                let along = (dx * matrix.a() + dy * matrix.b()) / norm;
                let across = (-dx * matrix.b() + dy * matrix.a()) / norm;
                let spaces = along / advance.value;
                (across.abs() < 0.05
                    && (spaces - spaces.round()).abs() < 0.01
                    && (1.0..=4096.0).contains(&spaces))
                .then_some(spaces.round() as usize)
            })
            .unwrap_or(1);
        counts.push(count);
    }
    if counts.len() != text.chars().filter(|c| *c == ' ').count() {
        return text;
    }
    let mut counts = counts.into_iter();
    text.chars()
        .flat_map(|c| {
            std::iter::repeat_n(
                c,
                if c == ' ' {
                    counts.next().unwrap_or(1)
                } else {
                    1
                },
            )
        })
        .collect()
}

fn block_font_identity(name: &str) -> &str {
    let mut name = base_font_name(name);
    while let Some((base, suffix)) = name.rsplit_once('-') {
        if suffix.len() != 16 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            break;
        }
        name = base;
    }
    name
}

/// Reads one page object as a Text Block member, or `None` when it is not a
/// visible text object. Geometry comes from the text layer so it shares the
/// coordinate system the frontend draws its dashed edit guides in.
fn block_object(
    layer: &crate::commands::TextLayerView,
    layer_raw_indices: &[usize],
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    object_index: usize,
) -> Option<BlockObject> {
    let object = objects.get(object_index).ok()?;
    let text_object = object.as_text_object()?;
    let mut text = object_text_with_spaces(text_object, page_text);
    let has_marker = text.contains('\u{2}') || text.contains('\u{fffe}');
    let mut has_hyphen = false;
    if text.trim().is_empty() {
        return None;
    }
    let font_name = block_font_identity(&text_object.font().name()).to_owned();
    let matrix = text_object
        .matrix()
        .map(|matrix| {
            [
                matrix.a(),
                matrix.b(),
                matrix.c(),
                matrix.d(),
                matrix.e(),
                matrix.f(),
            ]
        })
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let font_size = display_font_size(text_object.unscaled_font_size().value, matrix);
    let chars = page_text.chars_for_object(text_object).ok()?;

    let mut left = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    let mut baseline: Option<f32> = None;
    for character in chars.iter() {
        has_hyphen |= has_marker && character.is_hyphen().unwrap_or(false);
        let raw_index = character.index();
        let Some(position) = layer_raw_indices
            .iter()
            .position(|index| *index == raw_index)
        else {
            continue;
        };
        let view = layer.chars.get(position)?;
        if view.width <= 0.0 || view.height <= 0.0 {
            continue;
        }
        left = left.min(view.x);
        right = right.max(view.x + view.width);
        top = top.min(view.y);
        bottom = bottom.max(view.y + view.height);
        baseline = Some(baseline.map_or(view.origin_y, |current: f32| current.min(view.origin_y)));
    }
    if !left.is_finite() || !right.is_finite() {
        return None;
    }

    if has_hyphen {
        text = text.replace(['\u{2}', '\u{fffe}'], "-");
    }
    Some(BlockObject {
        text,
        font_name,
        font_size,
        color: color_hex(text_object),
        rotation: display_rotation_from_matrix(matrix, layer.rotation),
        baseline: baseline?,
        left,
        right,
        top,
        bottom,
    })
}

/// Whether two page objects are parts of the same editable line.
///
/// Word and WPS exports paint one object per glyph, so a Text Box built from a
/// single object would edit a single character. Objects join only when they
/// share the original look, sit on the same horizontal baseline, and are
/// close enough to be one run of text; a gap wider than three quarters of the
/// glyph height is a table cell or a column break.
fn blocks_join(left: &BlockObject, right: &BlockObject, line_start: Option<f32>) -> bool {
    if left.rotation.abs() > 0.01 || right.rotation.abs() > 0.01 {
        return false;
    }
    if left.font_name != right.font_name {
        return false;
    }
    if (left.font_size - right.font_size).abs() > 0.05 {
        return false;
    }
    if left.color != right.color {
        return false;
    }
    let height = (left.bottom - left.top)
        .max(right.bottom - right.top)
        .max(1.0);
    if (left.baseline - right.baseline).abs() > 0.5 {
        // Saved wrapped lines are separate PDF text objects. Rejoin adjacent
        // aligned lines so reopening does not turn a resized paragraph into
        // independently editable fragments. Never cross other page objects,
        // font/style changes, columns, or a paragraph-sized vertical gap.
        let line_step = right.baseline - left.baseline;
        return line_step >= left.font_size * 0.85
            && line_step <= left.font_size * 1.6
            && (right.left - line_start.unwrap_or(left.left)).abs()
                <= (left.font_size * 0.8).max(1.0);
    }
    if right.left < left.left - 0.25 * height {
        return false;
    }
    let gap = right.left - left.right;
    gap >= -0.5 * height && gap <= 0.75 * height
}

/// True when a boundary between two joined objects stood for a space glyph.
fn blocks_need_space(left: &BlockObject, right: &BlockObject) -> bool {
    if right.baseline - left.baseline > 0.5 {
        if left.text.chars().last().is_some_and(|c| c.is_ascii_digit())
            && right
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        {
            return false;
        }
        return left
            .text
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii() && !c.is_whitespace() && c != '-')
            && right
                .text
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
    }
    let height = (left.bottom - left.top)
        .max(right.bottom - right.top)
        .max(1.0);
    let gap = right.left - left.right;
    gap >= (0.15 * height).max(0.6)
        && !left.text.chars().last().is_some_and(char::is_whitespace)
        && !right.text.chars().next().is_some_and(char::is_whitespace)
}

struct TextBlock {
    start: usize,
    count: usize,
    text: String,
    width: f32,
    layout: Option<BoxLayout>,
}

/// Groups the clicked page object with the adjacent objects that form the same
/// visible line. The block is one editable unit: its text, font, baseline and
/// width come from the original objects, so an edit replaces the whole line
/// instead of a single glyph.
fn text_block_around(
    entry: &OpenDocument,
    page_index: u32,
    page_text: &PdfPageText<'_>,
    objects: &PdfPageObjects<'_>,
    layer: &crate::commands::TextLayerView,
    layer_raw_indices: &[usize],
    object_index: usize,
) -> Result<TextBlock, CommandError> {
    let clicked = block_object(layer, layer_raw_indices, page_text, objects, object_index)
        .ok_or_else(|| no_editable_text(page_index))?;
    let mut members = vec![clicked];
    let mut start = object_index;
    while start > 0 {
        if crate::watermark::is_watermark_object(entry, page_index, start - 1)
            || entry
                .edit
                .records
                .get(&page_index)
                .is_some_and(|records| records.iter().any(|record| record.contains(start - 1)))
        {
            break;
        }
        let Some(previous) = block_object(layer, layer_raw_indices, page_text, objects, start - 1)
        else {
            break;
        };
        // A Word/WPS line can contain one PDF object per glyph. Compare
        // paragraph alignment with the line's start, not its last glyph.
        let mut line_start = previous.left;
        if (previous.baseline - members[0].baseline).abs() > 0.5 {
            let mut probe = previous.clone();
            let mut probe_index = start - 1;
            while probe_index > 0 {
                let candidate_index = probe_index - 1;
                if crate::watermark::is_watermark_object(entry, page_index, candidate_index)
                    || entry.edit.records.get(&page_index).is_some_and(|records| {
                        records
                            .iter()
                            .any(|record| record.contains(candidate_index))
                    })
                {
                    break;
                }
                let Some(candidate) = block_object(
                    layer,
                    layer_raw_indices,
                    page_text,
                    objects,
                    candidate_index,
                ) else {
                    break;
                };
                if (candidate.baseline - previous.baseline).abs() > 0.5
                    || !blocks_join(&candidate, &probe, None)
                {
                    break;
                }
                line_start = line_start.min(candidate.left);
                probe = candidate;
                probe_index = candidate_index;
            }
        }
        if !blocks_join(&previous, &members[0], Some(line_start)) {
            break;
        }
        members.insert(0, previous);
        start -= 1;
    }

    let mut end = object_index;
    let object_count = objects.len();
    while end + 1 < object_count {
        if crate::watermark::is_watermark_object(entry, page_index, end + 1)
            || entry
                .edit
                .records
                .get(&page_index)
                .is_some_and(|records| records.iter().any(|record| record.contains(end + 1)))
        {
            break;
        }
        let Some(next) = block_object(layer, layer_raw_indices, page_text, objects, end + 1) else {
            break;
        };
        let last = members.last().expect("the block has its clicked object");
        let line_start = members
            .iter()
            .rev()
            .take_while(|member| (member.baseline - last.baseline).abs() <= 0.5)
            .map(|member| member.left)
            .fold(f32::INFINITY, f32::min);
        if !blocks_join(last, &next, Some(line_start)) {
            break;
        }
        members.push(next);
        end += 1;
    }

    let mut text = String::new();
    let mut width_left = f32::INFINITY;
    let mut width_right = f32::NEG_INFINITY;
    for (index, member) in members.iter().enumerate() {
        if index > 0 && blocks_need_space(&members[index - 1], member) {
            text.push(' ');
        }
        text.push_str(&member.text);
        width_left = width_left.min(member.left);
        width_right = width_right.max(member.right);
    }
    if text.trim().is_empty() {
        return Err(no_editable_text(page_index));
    }

    let mut leading: Vec<f32> = members
        .windows(2)
        .map(|pair| pair[1].baseline - pair[0].baseline)
        .filter(|step| *step > 0.5)
        .collect();
    leading.sort_by(f32::total_cmp);
    let layout = leading.get(leading.len() / 2).map(|spacing| {
        let top = members.iter().map(|m| m.top).fold(f32::INFINITY, f32::min);
        let bottom = members
            .iter()
            .map(|m| m.bottom)
            .fold(f32::NEG_INFINITY, f32::max);
        BoxLayout {
            x: width_left,
            y: top,
            width: (width_right - width_left).max(4.0),
            height: (bottom - top).max(1.0),
            rotation: 0.0,
            auto_height: true,
            line_height: Some(*spacing),
        }
    });
    Ok(TextBlock {
        start,
        count: end - start + 1,
        width: if (members[0].rotation.abs() - 90.0).abs() < 0.01 {
            (members[0].bottom - members[0].top).max(1.0)
        } else {
            (width_right - width_left).max(1.0)
        },
        text,
        layout,
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
    if crate::watermark::is_watermark_object(entry, page_index, object_index) {
        // The watermark is page content but not a Text Box; editing it would
        // move page objects underneath the watermark's tracked range.
        return Err(CommandError::new(
            CommandErrorCode::NoEditableText,
            format!("page {page_index} watermark text is not editable"),
        ));
    }

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
    let block = text_block_around(
        entry,
        page_index,
        &page_text,
        objects,
        &layer,
        &raw_indices,
        object_index,
    )?;

    entry.edit.next_box_id += 1;
    let record = TextBoxRecord {
        box_id: entry.edit.next_box_id,
        start: block.start,
        count: block.count,
        width: block.width,
        text: block.text,
        layout: block.layout,
        style: None,
        runs: Vec::new(),
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
        let physical = entry.physical_page(page_index)?;
        let mut view = target_for(entry, physical, char_index)?;
        view.page_index = entry.logical_page(physical).unwrap_or(0);
        Ok(view)
    })
}

/// Web-font subsets for the original-font textarea. These use the same
/// coverage plan/programs as a commit; nothing is attached to the PDF.
#[tauri::command]
pub fn text_box_editor_fonts(
    document_id: String,
    page_index: u32,
    box_id: u64,
    text: String,
) -> Result<Vec<String>, CommandError> {
    use base64::Engine;
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let index = find_record(&entry.edit, physical, box_id)?;
        let record = &entry.edit.records[&physical][index];
        if record.deleted || record.count == 0 || record.style.is_some() {
            return Ok(Vec::new());
        }
        let page = load_page(&entry.document, physical)?;
        let style = read_style(page.objects(), record.start)?;
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let plan = plan_fonts(&style, &normalized)?;
        let mut programs = Vec::new();
        if plan.uses_original() {
            if let Some(program) = style.font_data.as_deref() {
                let text: String = normalized
                    .chars()
                    .zip(&plan.fonts)
                    .filter(|(c, font)| !is_line_break(*c) && matches!(font, GlyphFont::Original))
                    .map(|(c, _)| c)
                    .collect();
                programs.push(
                    fonts::subset_program(
                        program,
                        0,
                        &style.font_family,
                        false,
                        &text,
                        fonts::weight_for(style.font_is_bold),
                    )?
                    .web_font_bytes()?,
                );
            }
        }
        let fallback_text = plan.fallback_text(&normalized);
        match plan.fallback_font() {
            Some(GlyphFont::Bundled(font)) => programs.push(
                font.subset(&fallback_text, fonts::weight_for(style.font_is_bold))?
                    .web_font_bytes()?,
            ),
            Some(GlyphFont::System(font)) => programs.push(
                font.subset(&fallback_text, fonts::weight_for(style.font_is_bold))?
                    .web_font_bytes()?,
            ),
            _ => {}
        }
        Ok(programs
            .into_iter()
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
            .collect())
    })
}

/// The linear scale a text matrix applies to its font size.
///
/// PDF text objects store the font size before the text matrix is applied.
/// Word and WPS exports commonly store a size in the hundreds with a matrix
/// that scales it down (for example 360 with a 0.05 scale paints 18 pt), so the
/// size a person sees — and the size the editor must show — is the stored size
/// times this scale.
fn matrix_font_scale(matrix: [f32; 6]) -> f32 {
    let scale = (matrix[0] * matrix[0] + matrix[1] * matrix[1]).sqrt();
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Converts a stored font size into the size the text paints at.
fn display_font_size(unscaled: f32, matrix: [f32; 6]) -> f32 {
    let size = unscaled * matrix_font_scale(matrix);
    if size.is_finite() && size > 0.0 {
        size
    } else {
        unscaled
    }
}

/// Removes the matrix's scale so `font_size` alone carries the size.
fn unscaled_matrix(matrix: [f32; 6]) -> [f32; 6] {
    let scale = matrix_font_scale(matrix);
    [
        matrix[0] / scale,
        matrix[1] / scale,
        matrix[2] / scale,
        matrix[3] / scale,
        matrix[4],
        matrix[5],
    ]
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
    /// Whether the embedded program is a bold instance. PDFX only needs this
    /// to rebuild a bundled-font subset at the same weight.
    font_is_bold: bool,
    font_is_italic: bool,
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
    let font_name = font.name();
    let font_family = fonts::normalize_family(&font_name);
    let (font_is_bold, font_is_italic) = style_flags_from_font_name(&font_name);
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
    let matrix = [
        matrix.a(),
        matrix.b(),
        matrix.c(),
        matrix.d(),
        matrix.e(),
        matrix.f(),
    ];
    let font_size = display_font_size(font_size, matrix);

    Ok(StyleSnapshot {
        font_family,
        font_size,
        fill_color: text_object.fill_color().ok(),
        render_mode: text_object.render_mode(),
        matrix: unscaled_matrix(matrix),
        font_is_embedded,
        font_is_bold,
        font_is_italic,
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

fn text_is_win_ansi(text: &str) -> bool {
    text.chars()
        .filter(|character| !matches!(character, '\n' | '\r'))
        .all(is_win_ansi_char)
}

/// Which font paints one character of the replacement text.
#[derive(Clone, Copy, PartialEq)]
enum GlyphFont {
    /// The box's own font program, which still has this glyph.
    Original,
    /// A fresh subset of a bundled family.
    Bundled(&'static fonts::BundledFont),
    /// A fresh subset of a face installed on this machine.
    System(&'static system_fonts::SystemFont),
    /// A base-14 font.
    BuiltIn(PdfFontBuiltin),
}

/// Per-character font plan for one replacement.
///
/// An edit keeps the original font for every character it can still paint, so
/// untouched Chinese text stays in the document's own typeface instead of
/// turning into the bundled font. Only characters the original program lacks
/// are painted by a fallback.
struct FontPlan {
    /// One entry per character of the replacement text. Line breaks are never
    /// painted; they keep their neighbour's choice.
    fonts: Vec<GlyphFont>,
    /// Set when at least one character needs a font other than the original.
    fallback: Option<FontUsageView>,
}

impl FontPlan {
    fn uniform(font: GlyphFont, characters: usize) -> Self {
        Self {
            fonts: vec![font; characters],
            fallback: None,
        }
    }

    fn fallback_font(&self) -> Option<GlyphFont> {
        self.fonts
            .iter()
            .copied()
            .find(|font| !matches!(font, GlyphFont::Original))
    }

    fn uses_original(&self) -> bool {
        self.fonts
            .iter()
            .any(|font| matches!(font, GlyphFont::Original))
    }

    /// The characters a fallback font must render.
    fn fallback_text(&self, text: &str) -> String {
        text.chars()
            .zip(&self.fonts)
            .filter(|(character, font)| {
                !is_line_break(*character) && !matches!(font, GlyphFont::Original)
            })
            .map(|(character, _)| character)
            .collect()
    }
}

fn is_line_break(character: char) -> bool {
    matches!(character, '\n' | '\r')
}

/// The base-14 face that matches the original font's weight and serif style.
fn latin_builtin(style: &StyleSnapshot) -> PdfFontBuiltin {
    if fonts::is_serif_family(&style.font_family) {
        match (style.font_is_bold, style.font_is_italic) {
            (false, false) => PdfFontBuiltin::TimesRoman,
            (true, false) => PdfFontBuiltin::TimesBold,
            (false, true) => PdfFontBuiltin::TimesItalic,
            (true, true) => PdfFontBuiltin::TimesBoldItalic,
        }
    } else {
        match (style.font_is_bold, style.font_is_italic) {
            (false, false) => PdfFontBuiltin::Helvetica,
            (true, false) => PdfFontBuiltin::HelveticaBold,
            (false, true) => PdfFontBuiltin::HelveticaOblique,
            (true, true) => PdfFontBuiltin::HelveticaBoldOblique,
        }
    }
}

fn glyph_font_family(font: GlyphFont) -> String {
    match font {
        GlyphFont::Original => String::new(),
        GlyphFont::Bundled(bundled) => bundled.family.to_owned(),
        GlyphFont::System(installed) => installed.family.clone(),
        GlyphFont::BuiltIn(built_in) => built_in.to_pdf_font_name().to_owned(),
    }
}

/// The installed face that matches the original family and covers `text`.
///
/// A document names 微软雅黑 or 宋体 but may not embed the program (or embed a
/// subset that lacks the characters being typed). When the machine has that
/// family — from the OS or from WPS Office — using it keeps the document's
/// intended typeface instead of switching to a bundled look-alike.
fn installed_face_for(
    original: &str,
    text: &str,
    bold: bool,
    italic: bool,
) -> Option<&'static system_fonts::SystemFont> {
    // A base-14 name is reproduced without embedding anything; never let a
    // similarly named installed family (Times New Roman for Times) win.
    if built_in_for(base_font_name(original)).is_some() {
        return None;
    }
    let catalog = system_fonts::catalog();
    let family = catalog.family_for_name(original)?;
    let face = catalog.face_for_style(family, bold, italic)?;
    face.covers(text).then_some(face)
}

/// Chooses a fallback for characters the original font cannot paint.
///
/// The installed face of the original family comes first, so a missing
/// character continues in the same typeface the document uses. A WinAnsi
/// string can also use a base-14 font; otherwise PDFX falls back to the
/// bundled family whose style matches the original.
fn pick_fallback_font(
    style: &StyleSnapshot,
    text: &str,
    all_win_ansi: bool,
) -> Result<GlyphFont, CommandError> {
    if let Some(face) = installed_face_for(
        &style.font_family,
        text,
        style.font_is_bold,
        style.font_is_italic,
    ) {
        return Ok(GlyphFont::System(face));
    }
    if all_win_ansi {
        return Ok(GlyphFont::BuiltIn(latin_builtin(style)));
    }
    Ok(GlyphFont::Bundled(pick_bundled_font(
        &style.font_family,
        text,
    )?))
}

/// Chooses the bundled family whose style matches the original and that
/// covers every missing character.
///
/// FangSong originals (FangS-SC, STFangsong, 仿宋_GB2312) get Zhuque Fangsong,
/// so a new character keeps the thin strokes of the surrounding text.
/// Song/Ming/Kai originals (SimSun, HYShuSongErKW) get Noto Serif SC, so
/// editing a contract does not turn it into a sans-serif document; other
/// originals get Noto Sans SC. Later candidates are tried before rejecting the
/// edit, because a family may lack a rare character the missing text needs.
fn pick_bundled_font(
    original: &str,
    missing: &str,
) -> Result<&'static fonts::BundledFont, CommandError> {
    let candidates: &[&'static fonts::BundledFont] = if fonts::is_fangsong_family(original) {
        &[
            fonts::zhuque_fangsong(),
            fonts::noto_serif_sc(),
            fonts::noto_sans_sc(),
        ]
    } else if fonts::is_serif_family(original) {
        &[fonts::noto_serif_sc(), fonts::noto_sans_sc()]
    } else {
        &[fonts::noto_sans_sc(), fonts::noto_serif_sc()]
    };
    for candidate in candidates {
        if candidate.covers(missing) {
            return Ok(candidate);
        }
    }
    Err(fonts::missing_glyph_error(candidates[0].family, missing))
}

/// A plan that paints every character with a fallback font.
fn full_fallback_plan(
    style: &StyleSnapshot,
    text: &str,
    characters: usize,
    all_win_ansi: bool,
    reason: &str,
) -> Result<FontPlan, CommandError> {
    let original = style.font_family.as_str();
    let fallback = pick_fallback_font(style, text, all_win_ansi)?;
    let family = glyph_font_family(fallback);
    // Using the installed copy of the original family is not a visible
    // fallback: the document keeps the typeface it named, PDFX only supplies
    // the program the document did not embed.
    let fallback_notice = if matches!(fallback, GlyphFont::System(_)) {
        None
    } else {
        Some(FontUsageView {
            original_family: original.to_owned(),
            family: family.clone(),
            fallback: true,
            reason: Some(format!("{reason}; {family} was used instead")),
        })
    };
    Ok(FontPlan {
        fonts: vec![fallback; characters],
        fallback: fallback_notice,
    })
}

/// Chooses a font for every character of a replacement.
///
/// The original font is kept wherever its program provably paints the
/// character. Subset fonts keep advances for glyphs they dropped but no
/// outline, so [`face_paints`] is the proof. Characters the original cannot
/// paint use a bundled font that matches the original's style (Zhuque
/// Fangsong for FangSong names, Noto Serif SC for other serif names, Noto
/// Sans SC otherwise), or a base-14 font when the missing characters are
/// Latin.
fn plan_fonts(style: &StyleSnapshot, text: &str) -> Result<FontPlan, CommandError> {
    let original = style.font_family.as_str();
    let characters: Vec<char> = text.chars().collect();
    let all_win_ansi = text_is_win_ansi(text);

    if style.font_is_embedded {
        // The original program is one of our own subsets. A larger subset of
        // the same family is not a user-visible fallback.
        if let Some(bundled) = fonts::bundled_family(original) {
            if !bundled.covers(text) {
                return Err(fonts::missing_glyph_error(bundled.family, text));
            }
            return Ok(FontPlan::uniform(
                GlyphFont::Bundled(bundled),
                characters.len(),
            ));
        }

        if let Some(face) = style
            .font_data
            .as_deref()
            .and_then(|data| ttf_parser::Face::parse(data, 0).ok())
        {
            let mut painted: Vec<bool> = Vec::with_capacity(characters.len());
            let mut missing = String::new();
            for character in &characters {
                let paints = is_line_break(*character) || face_paints(&face, *character);
                if !paints {
                    missing.push(*character);
                }
                painted.push(paints);
            }
            if missing.is_empty() {
                return Ok(FontPlan::uniform(GlyphFont::Original, characters.len()));
            }

            let fallback = pick_fallback_font(style, &missing, text_is_win_ansi(&missing))?;
            let family = glyph_font_family(fallback);
            // The installed copy of the same family is the document's own
            // typeface, not a user-visible substitution.
            let fallback_notice = if matches!(fallback, GlyphFont::System(_)) {
                None
            } else {
                Some(FontUsageView {
                    original_family: original.to_owned(),
                    family: family.clone(),
                    fallback: true,
                    reason: Some(format!(
                        "the embedded font {original} is missing glyphs for the new text; \
                         {family} was used instead"
                    )),
                })
            };
            return Ok(FontPlan {
                fonts: characters
                    .iter()
                    .zip(&painted)
                    .map(|(_, paints)| {
                        if *paints {
                            GlyphFont::Original
                        } else {
                            fallback
                        }
                    })
                    .collect(),
                fallback: fallback_notice,
            });
        }

        return full_fallback_plan(
            style,
            text,
            characters.len(),
            all_win_ansi,
            &format!("the embedded font {original} could not be inspected"),
        );
    }

    if let Some(built_in) = built_in_for(original) {
        if all_win_ansi {
            return Ok(FontPlan::uniform(
                GlyphFont::BuiltIn(built_in),
                characters.len(),
            ));
        }
        let bundled = pick_bundled_font(original, text)?;
        let family = glyph_font_family(GlyphFont::Bundled(bundled));
        return Ok(FontPlan {
            fonts: vec![GlyphFont::Bundled(bundled); characters.len()],
            fallback: Some(FontUsageView {
                original_family: original.to_owned(),
                family: family.clone(),
                fallback: true,
                reason: Some(format!(
                    "the font {original} cannot represent the replacement text; \
                     {family} was used instead"
                )),
            }),
        });
    }

    full_fallback_plan(
        style,
        text,
        characters.len(),
        all_win_ansi,
        &format!("the original font {original} is not embedded"),
    )
}

#[derive(Default)]
struct OutlinePointCounter {
    points: usize,
}

impl ttf_parser::OutlineBuilder for OutlinePointCounter {
    fn move_to(&mut self, _x: f32, _y: f32) {
        self.points += 1;
    }
    fn line_to(&mut self, _x: f32, _y: f32) {
        self.points += 1;
    }
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {
        self.points += 1;
    }
    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {
        self.points += 1;
    }
    fn close(&mut self) {}
}

/// True when `character` produces a visible glyph in `face`.
///
/// Subset fonts keep the original cmap and advance widths for characters they
/// dropped; only the outline is gone. PDFium would lay such a character out
/// and paint nothing, so coverage must require an outline. Whitespace is the
/// exception: it is meant to paint nothing and only needs an advance.
fn face_paints(face: &ttf_parser::Face<'_>, character: char) -> bool {
    let Some(glyph) = face.glyph_index(character) else {
        return false;
    };
    if glyph.0 == 0 || face.glyph_hor_advance(glyph).unwrap_or(0) == 0 {
        return false;
    }
    if character.is_whitespace() {
        return true;
    }
    let mut outline = OutlinePointCounter::default();
    face.outline_glyph(glyph, &mut outline).is_some() && outline.points > 0
}

/// Loads (or reuses) the session's subset token for `text`.
fn bundled_token(
    entry: &mut OpenDocument,
    font: &fonts::BundledFont,
    text: &str,
    bold: bool,
) -> Result<PdfFontToken, CommandError> {
    entry
        .edit
        .bundled_fonts
        .token(&mut entry.document, font, text, fonts::weight_for(bold))
}

/// Loads (or reuses) the session's subset token for an installed font.
fn system_token(
    entry: &mut OpenDocument,
    font: &system_fonts::SystemFont,
    text: &str,
    bold: bool,
) -> Result<PdfFontToken, CommandError> {
    entry
        .edit
        .bundled_fonts
        .system_token(&mut entry.document, font, text, fonts::weight_for(bold))
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

/// The advance width of `text` in `token`, including leading and trailing
/// whitespace.
///
/// A text object's `bounds()` is ink bounds: it leaves out the advance of a
/// trailing space and the glyphs' side bearings. Using those bounds to place
/// the next styled segment puts it on top of the previous segment's trailing
/// space, so the space has no effect and an external extractor sees the two
/// words joined. Segment placement therefore sums the font's per-character
/// advances, which are exactly the advances PDFium uses to lay the text out.
/// Fonts that cannot report advances fall back to ink bounds.
fn measure_segment_advance(
    document: &PdfDocument<'static>,
    token: PdfFontToken,
    font_size: f32,
    text: &str,
) -> Option<f32> {
    // The document font registry contains newly loaded fonts only. A token
    // obtained from an original page object is not necessarily registered.
    // Resolve it through a detached object instead of falling back to ink bounds.
    let probe = PdfPageTextObject::new(document, text, token, PdfPoints::new(font_size)).ok()?;
    let font = probe.font();
    let advance = {
        let size = PdfPoints::new(font_size);
        text.chars()
            .map(|character| {
                font.character_advance(character, size)
                    .map(|width| width.value)
            })
            .sum::<Option<f32>>()
    };
    match advance {
        Some(width) if width.is_finite() && width >= 0.0 => Some(width),
        _ => measure_with_token(document, token, font_size, text),
    }
}

fn fits(width: Option<f32>, limit: f32) -> bool {
    // Imported boxes use rounded ink bounds. Match the textarea's quarter-
    // point allowance so a subpixel difference cannot wrap the last glyph.
    width.map(|width| width <= limit + 0.25).unwrap_or(true)
}

/// The concrete rendering attributes of one style segment.
#[derive(Clone, Copy)]
struct SegmentStyle {
    token: PdfFontToken,
    font_size: f32,
    color: PdfColor,
    underline: bool,
    strikeout: bool,
}

/// One wrapped line as (character, style index) pairs, where index 0 is the
/// box's base style and `index + 1` is the corresponding run.
type StyledLine = Vec<(char, usize)>;

/// Width of a span measured piece by piece with each character's own font.
fn span_width(
    document: &PdfDocument<'static>,
    span: &[(char, usize)],
    styles: &[SegmentStyle],
) -> Option<f32> {
    let mut total = 0.0;
    let mut index = 0;
    while index < span.len() {
        let style_index = span[index].1;
        let mut text = String::new();
        while index < span.len() && span[index].1 == style_index {
            text.push(span[index].0);
            index += 1;
        }
        total += measure_segment_advance(
            document,
            styles[style_index].token,
            styles[style_index].font_size,
            &text,
        )?;
    }
    Some(total)
}

fn span_fits(
    document: &PdfDocument<'static>,
    span: &[(char, usize)],
    width: f32,
    styles: &[SegmentStyle],
) -> bool {
    fits(span_width(document, span, styles), width)
}

fn break_long_styled_word(
    document: &PdfDocument<'static>,
    lines: &mut Vec<StyledLine>,
    current: &mut StyledLine,
    word: &[(char, usize)],
    width: f32,
    styles: &[SegmentStyle],
) {
    for character in word {
        let mut candidate = current.clone();
        candidate.push(*character);
        if !current.is_empty() && !span_fits(document, &candidate, width, styles) {
            lines.push(std::mem::take(current));
        }
        current.push(*character);
    }
}

fn styled_line_or_space(line: StyledLine) -> StyledLine {
    // PDFium text objects cannot hold an empty string; a single space
    // preserves a blank line.
    if line.is_empty() {
        vec![(' ', 0)]
    } else {
        line
    }
}

/// Greedy word wrap over styled characters.
///
/// Every width is measured with the actual font and size, so a run
/// with a larger font occupies its real advance and the line still wraps at the
/// box width.
fn wrap_styled(
    document: &PdfDocument<'static>,
    text: &str,
    char_styles: &[usize],
    width: f32,
    styles: &[SegmentStyle],
) -> Vec<StyledLine> {
    let mut paragraphs: Vec<StyledLine> = Vec::new();
    let mut paragraph: StyledLine = Vec::new();
    for (index, character) in text.chars().enumerate() {
        if character == '\n' {
            paragraphs.push(std::mem::take(&mut paragraph));
        } else {
            paragraph.push((character, char_styles[index]));
        }
    }
    paragraphs.push(paragraph);

    let mut lines: Vec<StyledLine> = Vec::new();
    for paragraph in paragraphs {
        if paragraph.is_empty() {
            lines.push(vec![(' ', 0)]);
            continue;
        }

        // Split on spaces exactly like `str::split(' ')`: empty words between
        // consecutive spaces are kept, and the separator keeps its own style.
        let mut words: Vec<StyledLine> = Vec::new();
        let mut separators: Vec<(char, usize)> = Vec::new();
        let mut word: StyledLine = Vec::new();
        for (character, style_index) in paragraph {
            if character == ' ' {
                words.push(std::mem::take(&mut word));
                separators.push((character, style_index));
            } else {
                word.push((character, style_index));
            }
        }
        words.push(word);

        let mut current: StyledLine = Vec::new();
        for (word_index, word) in words.iter().enumerate() {
            let candidate = if current.is_empty() {
                word.clone()
            } else {
                let mut candidate = current.clone();
                candidate.push(separators[word_index - 1]);
                candidate.extend(word.iter().copied());
                candidate
            };
            if span_fits(document, &candidate, width, styles)
                || (current.is_empty() && word.is_empty())
            {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if span_fits(document, word, width, styles) {
                current = word.clone();
            } else {
                break_long_styled_word(document, &mut lines, &mut current, word, width, styles);
            }
        }
        lines.push(styled_line_or_space(current));
    }

    while lines.len() > 1
        && lines
            .last()
            .is_some_and(|line| line.iter().all(|(character, _)| character.is_whitespace()))
    {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(vec![(' ', 0)]);
    }
    lines
}

/// Splits one wrapped line into `(style index, text)` runs, folding
/// whitespace-only runs into a neighbour.
///
/// PDFium drops whitespace-only text objects from text extraction. A space
/// can become its own run when two differently styled ranges meet, so such
/// runs travel with the next text object (or the previous one at end of
/// line) to keep copy/extract output identical to the box content.
fn line_segments(line: &StyledLine) -> Vec<(usize, String)> {
    let mut segments: Vec<(usize, String)> = Vec::new();
    for (character, style_index) in line {
        match segments.last_mut() {
            Some((index, text)) if index == style_index => text.push(*character),
            _ => segments.push((*style_index, character.to_string())),
        }
    }

    let mut merged: Vec<(usize, String)> = Vec::with_capacity(segments.len());
    let mut pending = String::new();
    for (style_index, text) in segments {
        if text.chars().all(char::is_whitespace) {
            pending.push_str(&text);
            continue;
        }
        let text = if pending.is_empty() {
            text
        } else {
            let mut combined = std::mem::take(&mut pending);
            combined.push_str(&text);
            combined
        };
        merged.push((style_index, text));
    }
    if !pending.is_empty() {
        match merged.last_mut() {
            Some((_, text)) => text.push_str(&pending),
            None => merged.push((0, pending)),
        }
    }
    merged
}

/// Emits the page objects for one box whose lines contain styled segments.
///
/// The line rules extend the uniform renderer: a line's height is 1.2 times
/// its largest font size and its baseline sits 0.8 times that size below the
/// line top, so mixed sizes never overlap the next line. Each segment is its
/// own text object (plus its own underline path) placed at its measured
/// advance; text extraction therefore returns the original character order.
fn build_styled_box_objects(
    document: &mut PdfDocument<'static>,
    geometry: &DisplayGeometry,
    layout: &BoxLayout,
    alignment: TextAlignment,
    styles: &[SegmentStyle],
    lines: &[StyledLine],
    height: f32,
    original_mode: Option<PdfPageTextRenderMode>,
) -> Result<Vec<PdfPageObject<'static>>, CommandError> {
    let theta = layout.rotation.to_radians();
    let alpha = (geometry.rotation() as f32 - layout.rotation).to_radians();
    let (sin_alpha, cos_alpha) = alpha.sin_cos();
    let mut objects = Vec::new();
    let mut line_top = 0.0;

    for line in lines {
        let segments = line_segments(line);
        if segments.is_empty() {
            continue;
        }

        let widths: Vec<f32> = segments
            .iter()
            .map(|(index, text)| {
                measure_segment_advance(
                    document,
                    styles[*index].token,
                    styles[*index].font_size,
                    text,
                )
                .unwrap_or(0.0)
                .max(0.0)
            })
            .collect();
        let line_width: f32 = widths.iter().sum();
        let line_size = segments
            .iter()
            .map(|(index, _)| styles[*index].font_size)
            .fold(0.0, f32::max);
        let line_height = layout.line_height.unwrap_or(0.0).max(line_size * 1.2);
        let offset = match alignment {
            TextAlignment::Left => 0.0,
            TextAlignment::Center => ((layout.width - line_width) / 2.0).max(0.0),
            TextAlignment::Right => (layout.width - line_width).max(0.0),
        };
        let local_y = line_top + line_size * 0.8;
        let visible = line_top + line_height <= height + 0.01;
        let mut cursor = 0.0;

        for ((style_index, text), width) in segments.iter().zip(widths.iter()) {
            let style = styles[*style_index];
            let (local_x, local_y_rotated) = rotate_local(offset + cursor, local_y, theta);
            let (user_x, user_y) =
                geometry.point_inverse(layout.x + local_x, layout.y + local_y_rotated);

            let mut object = PdfPageTextObject::new(
                document,
                text,
                style.token,
                PdfPoints::new(style.font_size),
            )
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not create Text Box text: {error:?}"),
                )
            })?;
            object.set_fill_color(style.color).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not set the Text Box colour: {error:?}"),
                )
            })?;
            let render_mode = if visible {
                original_mode.unwrap_or(PdfPageTextRenderMode::FilledUnstroked)
            } else {
                PdfPageTextRenderMode::Invisible
            };
            object.set_render_mode(render_mode).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not set the Text Box render mode: {error:?}"),
                )
            })?;
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

            if visible && style.underline && *width > 0.0 {
                let underline_y = local_y + style.font_size * 0.12;
                let (start_x, start_y) = rotate_local(offset + cursor, underline_y, theta);
                let (end_x, end_y) = rotate_local(offset + cursor + width, underline_y, theta);
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
                    style.color,
                    PdfPoints::new((style.font_size * 0.06).max(0.4)),
                )
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not create the Text Box underline: {error:?}"),
                    )
                })?;
                objects.push(PdfPageObject::from(path));
            }

            if visible && style.strikeout && *width > 0.0 {
                let strikeout_y = local_y - style.font_size * 0.3;
                let (start_x, start_y) = rotate_local(offset + cursor, strikeout_y, theta);
                let (end_x, end_y) = rotate_local(offset + cursor + width, strikeout_y, theta);
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
                    style.color,
                    PdfPoints::new((style.font_size * 0.06).max(0.4)),
                )
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not create the Text Box strikeout: {error:?}"),
                    )
                })?;
                objects.push(PdfPageObject::from(path));
            }
            cursor += width;
        }
        line_top += line_height;
    }

    Ok(objects)
}

/// Emits the text objects of a legacy Text Object (one without a canonical
/// layout) whose lines may contain several font runs.
///
/// Each run becomes its own text object, placed at the measured advance of the
/// runs before it, and the box's original matrix is reused unchanged. This
/// keeps the characters the embedded font can still paint in that font while
/// only new characters use a fallback.
fn build_native_text_objects(
    document: &PdfDocument<'static>,
    matrix: [f32; 6],
    line_height: f32,
    fill_color: Option<PdfColor>,
    render_mode: PdfPageTextRenderMode,
    styles: &[SegmentStyle],
    lines: &[StyledLine],
) -> Result<Vec<PdfPageObject<'static>>, CommandError> {
    let [a, b, c, d, e, f] = matrix;
    let mut objects = Vec::new();

    for (line_number, line) in lines.iter().enumerate() {
        let segments = line_segments(line);
        let widths: Vec<f32> = segments
            .iter()
            .map(|(index, text)| {
                measure_segment_advance(
                    document,
                    styles[*index].token,
                    styles[*index].font_size,
                    text,
                )
                .unwrap_or(0.0)
                .max(0.0)
            })
            .collect();
        let downward = line_number as f32 * line_height;
        let mut cursor = 0.0;

        for ((style_index, text), width) in segments.iter().zip(&widths) {
            let style = styles[*style_index];
            let mut object = PdfPageTextObject::new(
                document,
                text,
                style.token,
                PdfPoints::new(style.font_size),
            )
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not create replacement text: {error:?}"),
                )
            })?;
            object
                .set_fill_color(fill_color.unwrap_or(PdfColor::BLACK))
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not copy the text colour: {error:?}"),
                    )
                })?;
            if render_mode != PdfPageTextRenderMode::Unknown {
                object.set_render_mode(render_mode).map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not copy the text render mode: {error:?}"),
                    )
                })?;
            }
            object
                .apply_matrix(PdfMatrix::new(
                    a,
                    b,
                    c,
                    d,
                    e + cursor * a - downward * c,
                    f + cursor * b - downward * d,
                ))
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::Internal,
                        format!("could not place replacement text: {error:?}"),
                    )
                })?;
            objects.push(PdfPageObject::from(object));
            cursor += width;
        }
    }

    Ok(objects)
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
        line_height: None,
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

/// Which tracked record the caller is editing. That record keeps its start;
/// every other tracked page-object range is shifted.
pub(crate) enum ObjectRef {
    TextBox(u64),
    Image(u64),
    Line(u64),
}

/// Updates every tracked page-object range after objects at `start` changed
/// count. Text Boxes, images and lines share one page-object list, so a change
/// in any of them must keep the others' indices correct.
///
/// When objects are inserted (`delta > 0`), a later record may already sit at
/// exactly `start` (this happens when undoing a deletion, where the deleted
/// record had count 0 before the undo), so an insertion shifts records at or
/// after `start`. A removal (`delta < 0`) only moves records strictly after it.
pub(crate) fn shift_object_indexes(
    session: &mut EditSession,
    page_index: u32,
    start: usize,
    delta: isize,
    keep: ObjectRef,
) {
    if delta == 0 {
        return;
    }
    let moves = |index: usize| {
        if delta > 0 {
            index >= start
        } else {
            index > start
        }
    };
    if let Some(records) = session.records.get_mut(&page_index) {
        for record in records.iter_mut() {
            if matches!(keep, ObjectRef::TextBox(id) if record.box_id == id) {
                continue;
            }
            if moves(record.start) {
                record.start = (record.start as isize + delta).max(0) as usize;
            }
        }
    }
    if let Some(images) = session.images.get_mut(&page_index) {
        for image in images.iter_mut() {
            if matches!(keep, ObjectRef::Image(id) if image.image_id == id) {
                continue;
            }
            if moves(image.start) {
                image.start = (image.start as isize + delta).max(0) as usize;
            }
        }
    }
    if let Some(lines) = session.lines.get_mut(&page_index) {
        for line in lines.iter_mut() {
            if matches!(keep, ObjectRef::Line(id) if line.line_id == id) {
                continue;
            }
            if moves(line.start) {
                line.start = (line.start as isize + delta).max(0) as usize;
            }
        }
    }
    if let Some(state) = session.watermark.as_mut() {
        if let Some(page) = state
            .pages
            .iter_mut()
            .find(|page| page.physical_page == page_index)
        {
            if moves(page.start) {
                page.start = (page.start as isize + delta).max(0) as usize;
            }
        }
    }
}

/// Replaces `remove_count` page objects at `start` with `insert_objects` and
/// returns the detached originals.
///
/// Undo/redo use this both to take the current objects off the page and to put
/// snapshot objects back at the same index, which preserves object order and
/// therefore text extraction order.
pub(crate) fn detach_and_insert(
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

/// Appends a committed created-field change (creation, property update,
/// deletion or value) to the unified undo stack.
pub(crate) fn push_form_structure_action(
    entry: &mut OpenDocument,
    uid: u64,
    page_index: u32,
    before: Option<crate::form_create::CreatedFieldSnapshot>,
    after: Option<crate::form_create::CreatedFieldSnapshot>,
) -> EditHistoryView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::FormStructure(FormStructureEditAction {
            uid,
            page_index,
            before_state,
            after_state,
            before,
            after,
        }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.edit.history_view()
}

/// Applies one side of a created-field action. `None` means the field did not
/// exist on that side, so it is removed from the session store.
fn apply_form_structure(
    entry: &mut OpenDocument,
    uid: u64,
    target: Option<&crate::form_create::CreatedFieldSnapshot>,
) {
    match target {
        Some(snapshot) => entry.edit.created_fields.restore(snapshot.field.clone()),
        None => {
            entry.edit.created_fields.remove(uid);
        }
    }
}

fn undo_form_structure(
    entry: &mut OpenDocument,
    action: FormStructureEditAction,
) -> Result<EditStepView, CommandError> {
    apply_form_structure(entry, action.uid, action.before.as_ref());
    let page_index = action.page_index;
    entry.edit.current_state = action.before_state;
    entry
        .edit
        .redo_stack
        .push(EditAction::FormStructure(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn redo_form_structure(
    entry: &mut OpenDocument,
    action: FormStructureEditAction,
) -> Result<EditStepView, CommandError> {
    apply_form_structure(entry, action.uid, action.after.as_ref());
    let page_index = action.page_index;
    entry.edit.current_state = action.after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::FormStructure(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// Appends a committed annotation change to the unified undo stack.
pub(crate) fn push_annotation_action(
    entry: &mut OpenDocument,
    page_index: u32,
    name: String,
    before: Option<AnnotationSnapshot>,
    after: Option<AnnotationSnapshot>,
) -> EditHistoryView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::Annotation(Box::new(AnnotationEditAction {
            page_index,
            name,
            before_state,
            after_state,
            before,
            after,
        })));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.edit.history_view()
}

/// Applies one side of an annotation action. `None` means the side has no
/// annotation, so the current annotation is removed.
fn apply_annotation_target(
    document: &PdfDocument<'static>,
    target: Option<&AnnotationSnapshot>,
    name: &str,
) -> Result<(), CommandError> {
    match target {
        Some(snapshot) => restore_annotation_snapshot(document, snapshot),
        None => remove_annotation_named(document, name),
    }
}

fn undo_annotation(
    entry: &mut OpenDocument,
    action: Box<AnnotationEditAction>,
) -> Result<EditStepView, CommandError> {
    apply_annotation_target(&entry.document, action.before.as_ref(), &action.name)?;
    let page_index = action.page_index;
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Annotation(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn redo_annotation(
    entry: &mut OpenDocument,
    action: Box<AnnotationEditAction>,
) -> Result<EditStepView, CommandError> {
    apply_annotation_target(&entry.document, action.after.as_ref(), &action.name)?;
    let page_index = action.page_index;
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Annotation(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// Appends a committed link change to the unified undo stack.
pub(crate) fn push_link_action(
    entry: &mut OpenDocument,
    page_index: u32,
    name: String,
    before: Option<crate::links::LinkSnapshot>,
    after: Option<crate::links::LinkSnapshot>,
) -> EditHistoryView {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::Link(Box::new(LinkEditAction {
            page_index,
            name,
            before_state,
            after_state,
            before,
            after,
        })));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
    entry.edit.history_view()
}

fn undo_link(
    entry: &mut OpenDocument,
    action: Box<LinkEditAction>,
) -> Result<EditStepView, CommandError> {
    crate::links::apply_link_snapshot(
        &entry.document,
        action.page_index,
        action.before.as_ref(),
        &action.name,
    )?;
    let page_index = action.page_index;
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Link(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn redo_link(
    entry: &mut OpenDocument,
    action: Box<LinkEditAction>,
) -> Result<EditStepView, CommandError> {
    crate::links::apply_link_snapshot(
        &entry.document,
        action.page_index,
        action.after.as_ref(),
        &action.name,
    )?;
    let page_index = action.page_index;
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Link(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn redo_field(
    entry: &mut OpenDocument,
    action: FieldEditAction,
) -> Result<EditStepView, CommandError> {
    crate::form::apply_field_snapshot(
        &entry.document,
        &action.field_id,
        &action.after,
        action.pdfium_write,
    )?;
    let page_index = action.page_index;
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Field(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn undo_for(entry: &mut OpenDocument) -> Result<EditStepView, CommandError> {
    // Check the permission of the action that is about to be applied without
    // popping it: a Box, Image, Line or created-field action needs content
    // permission, a Field action needs form-filling permission, an annotation
    // action needs annotation permission.
    match entry.edit.undo_stack.last() {
        Some(EditAction::Box(_))
        | Some(EditAction::TextMove(_))
        | Some(EditAction::Image(_))
        | Some(EditAction::Line(_))
        | Some(EditAction::FormStructure(_))
        | Some(EditAction::Page(_))
        | Some(EditAction::Attachment(_))
        | Some(EditAction::Watermark(_))
        | Some(EditAction::Layer(_)) => ensure_content_editable(&entry.document)?,
        Some(EditAction::Field(_)) => crate::form::ensure_forms_editable(&entry.document)?,
        Some(EditAction::Annotation(_)) | Some(EditAction::Link(_)) => {
            ensure_annotation_editable(&entry.document)?
        }
        None => {
            return Err(CommandError::new(
                CommandErrorCode::InvalidArgument,
                "there is nothing to undo",
            ))
        }
    }
    let Some(action) = entry.edit.undo_stack.pop() else {
        unreachable!("the undo stack was checked immediately above");
    };
    let structural = matches!(action, EditAction::Page(_));
    let mut step = match action {
        EditAction::Box(action) => undo_box(entry, action),
        EditAction::TextMove(action) => restore_text_move(entry, action, false),
        EditAction::Image(action) => crate::image_edit::undo_image(entry, action),
        EditAction::Line(action) => crate::line_edit::undo_line(entry, action),
        EditAction::Field(action) => undo_field(entry, action),
        EditAction::FormStructure(action) => undo_form_structure(entry, action),
        EditAction::Annotation(action) => undo_annotation(entry, action),
        EditAction::Link(action) => undo_link(entry, action),
        EditAction::Page(action) => crate::page_ops::undo_page(entry, action),
        EditAction::Attachment(action) => crate::attachments::undo_attachment(entry, action),
        EditAction::Watermark(action) => crate::watermark::undo_watermark(entry, action),
        EditAction::Layer(action) => crate::layers::undo_layer(entry, action),
    }?;
    // Edit actions address physical pages; report the slot the user sees.
    // Page actions already report a logical slot.
    if !structural {
        step.page_index = entry.logical_page(step.page_index).unwrap_or(0);
    }
    Ok(step)
}

fn undo_field(
    entry: &mut OpenDocument,
    action: FieldEditAction,
) -> Result<EditStepView, CommandError> {
    crate::form::apply_field_snapshot(
        &entry.document,
        &action.field_id,
        &action.before,
        action.pdfium_write,
    )?;
    let page_index = action.page_index;
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Field(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn undo_box(
    entry: &mut OpenDocument,
    mut action: BoxEditAction,
) -> Result<EditStepView, CommandError> {
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
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        before_count as isize - after_count as isize,
        ObjectRef::TextBox(action.box_id),
    );

    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Box(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn redo_for(entry: &mut OpenDocument) -> Result<EditStepView, CommandError> {
    // Check the permission of the action that is about to be applied without
    // popping it: a Box, Image, Line or created-field action needs content
    // permission, a Field action needs form-filling permission, an annotation
    // action needs annotation permission.
    match entry.edit.redo_stack.last() {
        Some(EditAction::Box(_))
        | Some(EditAction::TextMove(_))
        | Some(EditAction::Image(_))
        | Some(EditAction::Line(_))
        | Some(EditAction::FormStructure(_))
        | Some(EditAction::Page(_))
        | Some(EditAction::Attachment(_))
        | Some(EditAction::Watermark(_))
        | Some(EditAction::Layer(_)) => ensure_content_editable(&entry.document)?,
        Some(EditAction::Field(_)) => crate::form::ensure_forms_editable(&entry.document)?,
        Some(EditAction::Annotation(_)) | Some(EditAction::Link(_)) => {
            ensure_annotation_editable(&entry.document)?
        }
        None => {
            return Err(CommandError::new(
                CommandErrorCode::InvalidArgument,
                "there is nothing to redo",
            ))
        }
    }
    let Some(action) = entry.edit.redo_stack.pop() else {
        unreachable!("the redo stack was checked immediately above");
    };
    let structural = matches!(action, EditAction::Page(_));
    let mut step = match action {
        EditAction::Box(action) => redo_box(entry, action),
        EditAction::TextMove(action) => restore_text_move(entry, action, true),
        EditAction::Image(action) => crate::image_edit::redo_image(entry, action),
        EditAction::Line(action) => crate::line_edit::redo_line(entry, action),
        EditAction::Field(action) => redo_field(entry, action),
        EditAction::FormStructure(action) => redo_form_structure(entry, action),
        EditAction::Annotation(action) => redo_annotation(entry, action),
        EditAction::Link(action) => redo_link(entry, action),
        EditAction::Page(action) => crate::page_ops::redo_page(entry, action),
        EditAction::Attachment(action) => crate::attachments::redo_attachment(entry, action),
        EditAction::Watermark(action) => crate::watermark::redo_watermark(entry, action),
        EditAction::Layer(action) => crate::layers::redo_layer(entry, action),
    }?;
    // Edit actions address physical pages; report the slot the user sees.
    // Page actions already report a logical slot.
    if !structural {
        step.page_index = entry.logical_page(step.page_index).unwrap_or(0);
    }
    Ok(step)
}

fn redo_box(
    entry: &mut OpenDocument,
    mut action: BoxEditAction,
) -> Result<EditStepView, CommandError> {
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
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        after_count as isize - before_count as isize,
        ObjectRef::TextBox(action.box_id),
    );

    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Box(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// A size/colour restyle of a legacy Text Object that keeps its original
/// embedded font.
///
/// The panel sends this when the user changes only the size or colour of text
/// that still uses a PDF font PDFX cannot reproduce itself; converting the box
/// to a bundled font would visibly change every character.
#[derive(Clone, Copy)]
struct LegacyRestyle {
    font_size: f32,
    /// `#rrggbb` components.
    color: (u8, u8, u8),
}

struct LegacyRenderPlan {
    start: usize,
    old_count: usize,
    box_width: f32,
    style: StyleSnapshot,
    plan: FontPlan,
    styles: Vec<SegmentStyle>,
    char_styles: Vec<usize>,
    font_size: f32,
    color: PdfColor,
}

/// Editing and resizing must resolve exactly the same fonts and widths.
fn prepare_legacy_render(
    entry: &mut OpenDocument,
    page_index: u32,
    record_index: usize,
    normalized: &str,
    restyle: Option<LegacyRestyle>,
) -> Result<LegacyRenderPlan, CommandError> {
    // Inspect the box without holding a borrow of the document that would
    // block loading a fallback font from its font collection.
    let (start, old_count, box_width, style, plan, repair_space) = {
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
        let plan = plan_fonts(&style, &normalized)?;
        let repair_space = if normalized.contains(' ') && plan.uses_original() {
            let object = objects.get(record.start).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("could not inspect original font: {error:?}"),
                )
            })?;
            let font = object.as_text_object().unwrap().font();
            style
                .font_data
                .as_deref()
                .and_then(|data| ttf_parser::Face::parse(data, 0).ok())
                .and_then(|face| {
                    let gid = face.glyph_index(' ')?;
                    let expected = f32::from(face.glyph_hor_advance(gid)?) * style.font_size
                        / f32::from(face.units_per_em());
                    let actual = font
                        .character_advance(' ', PdfPoints::new(style.font_size))?
                        .value;
                    Some((actual - expected).abs() > 0.05)
                })
                .unwrap_or(false)
        } else {
            false
        };
        (
            record.start,
            record.count,
            record.width,
            style,
            plan,
            repair_space,
        )
    };

    let font_size = restyle.map_or(style.font_size, |restyle| restyle.font_size);
    validate_font_size(font_size)?;
    let color = match restyle {
        Some(restyle) => {
            let (red, green, blue) = restyle.color;
            PdfColor::new(red, green, blue, 255)
        }
        None => style.fill_color.unwrap_or(PdfColor::BLACK),
    };

    let fallback_token = match plan.fallback_font() {
        Some(GlyphFont::BuiltIn(built_in)) => {
            Some(entry.document.fonts_mut().new_built_in(built_in))
        }
        Some(GlyphFont::Bundled(bundled)) => {
            let mut fallback_text = plan.fallback_text(&normalized);
            // `wrap_styled` draws blank lines as a single space in style 0.
            if !fallback_text.contains(' ') {
                fallback_text.push(' ');
            }
            Some(bundled_token(
                entry,
                bundled,
                &fallback_text,
                style.font_is_bold,
            )?)
        }
        Some(GlyphFont::System(installed)) => {
            let mut fallback_text = plan.fallback_text(&normalized);
            if !fallback_text.contains(' ') {
                fallback_text.push(' ');
            }
            Some(system_token(
                entry,
                installed,
                &fallback_text,
                style.font_is_bold,
            )?)
        }
        Some(GlyphFont::Original) | None => None,
    };

    let repaired_token = if repair_space {
        let text: String = normalized
            .chars()
            .zip(&plan.fonts)
            .filter(|(character, font)| {
                !is_line_break(*character) && matches!(font, GlyphFont::Original)
            })
            .map(|(character, _)| character)
            .collect();
        Some(entry.edit.bundled_fonts.repaired_embedded_token(
            &mut entry.document,
            style.font_data.as_deref().unwrap(),
            &style.font_family,
            &text,
        )?)
    } else {
        None
    };
    let page = load_page(&entry.document, page_index)?;
    let original_token = if repaired_token.is_some() {
        repaired_token
    } else if plan.uses_original() {
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
        let token = text_object.font().token();
        Some(token)
    } else {
        None
    };

    // `wrap_styled` renders a blank line with style 0, so the fallback style
    // comes first: bundled and base-14 fonts always have a space glyph.
    let mut styles: Vec<SegmentStyle> = Vec::new();
    let fallback_index = fallback_token.map(|token| {
        styles.push(SegmentStyle {
            token,
            font_size,
            color,
            underline: false,
            strikeout: false,
        });
        0
    });
    let original_index = match original_token {
        Some(token) => {
            let index = styles.len();
            styles.push(SegmentStyle {
                token,
                font_size,
                color,
                underline: false,
                strikeout: false,
            });
            index
        }
        None => 0,
    };
    let char_styles: Vec<usize> = normalized
        .chars()
        .zip(&plan.fonts)
        .map(|(_, font)| match fallback_index {
            Some(index) if !matches!(font, GlyphFont::Original) => index,
            _ => original_index,
        })
        .collect();

    drop(page);
    Ok(LegacyRenderPlan {
        start,
        old_count,
        box_width,
        style,
        plan,
        styles,
        char_styles,
        font_size,
        color,
    })
}

fn apply_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    text: String,
    restyle: Option<LegacyRestyle>,
) -> Result<TextEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");

    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (deleted, uses_style, old_text) = {
        let record = &entry.edit.records[&page_index][record_index];
        (record.deleted, record.style.is_some(), record.text.clone())
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }
    if uses_style {
        // Created boxes and legacy boxes converted to a built-in style use the
        // canonical layout path, which also permits clearing the text. Styled
        // ranges are tied to character offsets: an unchanged text keeps them,
        // any replacement resets them to the base style.
        let keep_runs = normalized == old_text;
        return apply_to_styled_box(entry, page_index, record_index, normalized, keep_runs);
    }
    if normalized.trim().is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "replacement text is empty; delete the Text Box instead",
        ));
    }

    let LegacyRenderPlan {
        start,
        old_count,
        box_width,
        style,
        plan,
        styles,
        char_styles,
        font_size,
        color,
    } = prepare_legacy_render(entry, page_index, record_index, &normalized, restyle)?;
    let mut page = load_page(&entry.document, page_index)?;

    let lines = wrap_styled(
        &entry.document,
        &normalized,
        &char_styles,
        box_width,
        &styles,
    );
    let line_height = entry.edit.records[&page_index][record_index]
        .layout
        .as_ref()
        .and_then(|layout| layout.line_height)
        .unwrap_or(0.0)
        .max(font_size * 1.2);
    let replacements = build_native_text_objects(
        &entry.document,
        style.matrix,
        line_height,
        Some(color),
        style.render_mode,
        &styles,
        &lines,
    )?;

    let new_count = replacements.len();
    // Appending text grows an auto-height original box as well as a created
    // one; the selection frame must include every newly wrapped line.
    let content_height = lines.len() as f32 * line_height;
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
            if let Some(layout) = record.layout.as_mut() {
                if layout.auto_height || restyle.is_some() {
                    layout.height = layout.height.max(content_height);
                }
            }
        }
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        new_count as isize - old_count as isize,
        ObjectRef::TextBox(box_id),
    );
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
        font: plan.fallback,
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
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        -(count as isize),
        ObjectRef::TextBox(box_id),
    );
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

/// The concrete font a styled box is rendered with.
struct ResolvedBoxFont {
    token: PdfFontToken,
    /// The style actually used. On a fallback this differs from the requested
    /// style, and callers persist it so later edits and transforms agree.
    style: TextBoxStyle,
    fallback: Option<FontUsageView>,
}

/// Resolves a styled box's family and text to a renderable font.
///
/// A base-14 style keeps working for WinAnsi text. Once the text needs glyphs
/// a base-14 font cannot encode, the bundled CJK font takes over and the
/// returned fallback tells the property panel why the family changed.
fn resolve_box_font(
    entry: &mut OpenDocument,
    style: &TextBoxStyle,
    text: &str,
) -> Result<ResolvedBoxFont, CommandError> {
    if style.is_bundled() {
        let bundled = fonts::bundled_family(&style.font_family).unwrap_or_else(fonts::noto_sans_sc);
        let token = bundled_token(entry, bundled, text, style.bold)?;
        return Ok(ResolvedBoxFont {
            token,
            style: style.clone(),
            fallback: None,
        });
    }

    if style.is_installed() {
        let catalog = system_fonts::catalog();
        if let Some(face) = catalog.face_for_style(&style.font_family, style.bold, style.italic) {
            if face.covers(text) {
                let token = system_token(entry, face, text, style.bold)?;
                let mut effective = style.clone();
                effective.font_family = face.family.clone();
                return Ok(ResolvedBoxFont {
                    token,
                    style: effective,
                    fallback: None,
                });
            }
        }
        // The installed family cannot paint this text (a rare CJK extension
        // character, or a Latin face asked to render Chinese). Latin text can
        // use a base-14 face; anything else falls through to the bundled CJK
        // font below and is reported as a fallback.
        if text_is_win_ansi(text) {
            let base_family = if fonts::is_serif_family(&style.font_family) {
                "Times"
            } else {
                "Helvetica"
            };
            let built_in = builtin_for_style(base_family, style.bold, style.italic)
                .expect("base-14 provides all four variants");
            let token = entry.document.fonts_mut().new_built_in(built_in);
            let mut effective = style.clone();
            effective.font_family = base_family.to_owned();
            return Ok(ResolvedBoxFont {
                token,
                style: effective,
                fallback: Some(FontUsageView {
                    original_family: style.font_family.clone(),
                    family: base_family.to_owned(),
                    fallback: true,
                    reason: Some(format!(
                        "the installed font {} cannot represent the replacement text; \
                         {base_family} was used instead",
                        style.font_family
                    )),
                }),
            });
        }
    } else if text_is_win_ansi(text) {
        let token = entry.document.fonts_mut().new_built_in(style.builtin());
        return Ok(ResolvedBoxFont {
            token,
            style: style.clone(),
            fallback: None,
        });
    }

    if style.italic {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedFont,
            format!(
                "the bundled font {} has no italic variant; clear italic for text the \
                 {} font cannot represent",
                fonts::NOTO_SANS_SC_FAMILY,
                style.font_family
            ),
        ));
    }

    let bundled = fonts::noto_sans_sc();
    if !bundled.covers(text) {
        return Err(fonts::missing_glyph_error(bundled.family, text));
    }
    let token = bundled_token(entry, bundled, text, style.bold)?;
    let mut effective = style.clone();
    effective.font_family = fonts::NOTO_SANS_SC_FAMILY.to_owned();
    Ok(ResolvedBoxFont {
        token,
        style: effective,
        fallback: Some(FontUsageView {
            original_family: style.font_family.clone(),
            family: fonts::NOTO_SANS_SC_FAMILY.to_owned(),
            fallback: true,
            reason: Some(format!(
                "the font {} cannot represent the replacement text; {} was used instead",
                style.font_family,
                fonts::NOTO_SANS_SC_FAMILY
            )),
        }),
    })
}

/// A slice of the box text with character offsets.
fn text_slice(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end - start).collect()
}

fn validate_runs(text: &str, runs: &[TextBoxRun]) -> Result<(), CommandError> {
    let char_count = text.chars().count();
    let mut previous_end = 0;
    for run in runs {
        if run.start < previous_end || run.start >= run.end || run.end > char_count {
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                format!(
                    "Text Box run {}..{} is invalid for {} characters",
                    run.start, run.end, char_count
                ),
            ));
        }
        previous_end = run.end;
    }
    Ok(())
}

/// Keeps runs sorted and merges neighbours that render identically.
fn merge_runs(runs: Vec<TextBoxRun>) -> Vec<TextBoxRun> {
    let mut merged: Vec<TextBoxRun> = Vec::with_capacity(runs.len());
    for run in runs {
        if let Some(last) = merged.last_mut() {
            if last.end == run.start && last.style == run.style {
                last.end = run.end;
                continue;
            }
        }
        merged.push(run);
    }
    merged
}

/// Replaces the style of one range, splitting runs that straddle its edges.
/// The parts outside the selection keep their previous run style.
fn with_run_style(
    runs: &[TextBoxRun],
    start: usize,
    end: usize,
    style: TextBoxStyle,
) -> Vec<TextBoxRun> {
    let mut result: Vec<TextBoxRun> = Vec::with_capacity(runs.len() + 1);
    for run in runs {
        if run.end <= start || run.start >= end {
            result.push(run.clone());
            continue;
        }
        if run.start < start {
            result.push(TextBoxRun {
                start: run.start,
                end: start,
                style: run.style.clone(),
            });
        }
        if run.end > end {
            result.push(TextBoxRun {
                start: end,
                end: run.end,
                style: run.style.clone(),
            });
        }
    }
    result.push(TextBoxRun { start, end, style });
    result.sort_by_key(|run| run.start);
    merge_runs(result)
}

/// A box rendered with its base style and styled runs.
struct RenderedBox {
    objects: Vec<PdfPageObject<'static>>,
    height: f32,
    /// The base style actually used (family changed on a fallback).
    style: TextBoxStyle,
    /// The runs after font resolution, merging and clipping.
    runs: Vec<TextBoxRun>,
    /// The first font fallback encountered, for the canvas notice.
    fallback: Option<FontUsageView>,
}

/// Renders `text` with `style` as the base and `runs` as overrides.
///
/// Every run is resolved against only its own characters, so a font fallback
/// inside one run does not change the family of the untouched text. The
/// returned runs are the effective styles (including fallbacks) that callers
/// persist.
fn render_styled_box(
    entry: &mut OpenDocument,
    page_index: u32,
    layout: &BoxLayout,
    style: &TextBoxStyle,
    text: &str,
    runs: &[TextBoxRun],
) -> Result<RenderedBox, CommandError> {
    validate_runs(text, runs)?;

    let page = load_page(&entry.document, page_index)?;
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

    let chars: Vec<char> = text.chars().collect();
    let mut base_text = String::new();
    let mut cursor = 0;
    for run in runs {
        base_text.extend(&chars[cursor..run.start]);
        cursor = run.end;
    }
    base_text.extend(&chars[cursor..]);

    // PDFium cannot build a subset for an empty string; resolving the base
    // style with a space keeps an entirely styled box renderable without
    // adding any glyph outside its own text.
    let base_probe = if base_text.is_empty() {
        " "
    } else {
        base_text.as_str()
    };
    let base = resolve_box_font(entry, style, base_probe)?;
    let mut fallback = base.fallback.clone();
    // A run keeps the token resolved for its own characters: bundled subsets
    // cover only the text they were built from, so runs must not be merged
    // after resolution.
    let mut resolved_runs: Vec<(TextBoxRun, PdfFontToken)> = Vec::with_capacity(runs.len());
    for run in runs {
        let run_text = text_slice(text, run.start, run.end);
        let resolved = resolve_box_font(entry, &run.style, &run_text)?;
        if fallback.is_none() {
            fallback = resolved.fallback.clone();
        }
        let mut effective = resolved.style;
        // Alignment is a box property, not a run property.
        effective.alignment = base.style.alignment;
        resolved_runs.push((
            TextBoxRun {
                start: run.start,
                end: run.end,
                style: effective,
            },
            resolved.token,
        ));
    }

    let mut styles = Vec::with_capacity(resolved_runs.len() + 1);
    styles.push(SegmentStyle {
        token: base.token,
        font_size: base.style.font_size,
        color: base.style.pdf_color(),
        underline: base.style.underline,
        strikeout: base.style.strikeout,
    });
    for (run, token) in &resolved_runs {
        styles.push(SegmentStyle {
            token: *token,
            font_size: run.style.font_size,
            color: run.style.pdf_color(),
            underline: run.style.underline,
            strikeout: run.style.strikeout,
        });
    }
    let runs: Vec<TextBoxRun> = resolved_runs.into_iter().map(|(run, _)| run).collect();

    let mut char_styles = vec![0usize; chars.len()];
    for (index, run) in runs.iter().enumerate() {
        for slot in &mut char_styles[run.start..run.end] {
            *slot = index + 1;
        }
    }

    let (lines, content_height) = if text.is_empty() {
        (Vec::new(), 0.0)
    } else {
        let lines = wrap_styled(&entry.document, text, &char_styles, layout.width, &styles);
        let content_height = if styles.len() == 1 {
            lines.len() as f32
                * layout
                    .line_height
                    .unwrap_or(0.0)
                    .max(styles[0].font_size * 1.2)
        } else {
            lines
                .iter()
                .map(|line| {
                    let size = line
                        .iter()
                        .map(|(_, index)| styles[*index].font_size)
                        .fold(0.0, f32::max);
                    layout.line_height.unwrap_or(0.0).max(size * 1.2)
                })
                .sum()
        };
        (lines, content_height)
    };
    let height = effective_height(layout, content_height);
    let objects = build_styled_box_objects(
        &mut entry.document,
        &geometry,
        layout,
        base.style.alignment,
        &styles,
        &lines,
        height,
        None,
    )?;

    Ok(RenderedBox {
        objects,
        height,
        style: base.style,
        runs,
        fallback,
    })
}

/// Applies text to a box with a canonical layout and a chosen built-in style.
///
/// `keep_runs` is true only when the text did not change: styling ranges are
/// tied to character offsets, so a changed text resets them to the base style.
fn apply_to_styled_box(
    entry: &mut OpenDocument,
    page_index: u32,
    record_index: usize,
    normalized: String,
    keep_runs: bool,
) -> Result<TextEditResultView, CommandError> {
    let (box_id, start, old_count, style, layout, runs) = {
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
            record.runs.clone(),
        )
    };

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    drop(page);

    let runs = if keep_runs { runs } else { Vec::new() };
    let rendered = render_styled_box(entry, page_index, &layout, &style, &normalized, &runs)?;
    let RenderedBox {
        objects,
        height,
        style: effective_style,
        runs: effective_runs,
        fallback,
    } = rendered;
    let new_count = objects.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, objects)?;

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
        record.style = Some(effective_style);
        record.runs = effective_runs;
        if layout.auto_height {
            record.layout = Some(BoxLayout { height, ..layout });
        }
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        new_count as isize - old_count as isize,
        ObjectRef::TextBox(box_id),
    );
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
        font: fallback,
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
        line_height: None,
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
        runs: Vec::new(),
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
                // Legacy Text Objects enter the list once they are resolved
                // by a click, so selecting one survives a page reload.
                .filter(|record| !record.deleted)
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
    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, old_count, text, deleted, uses_style) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.start,
            record.count,
            record.text.clone(),
            record.deleted,
            record.style.is_some(),
        )
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }

    // A box that still paints an arbitrary embedded font cannot be recreated
    // through the style system. Size and colour only change the same text
    // objects' font size and fill, so the original program (and every original
    // glyph) survives instead of being replaced by a bundled look-alike.
    if !uses_style && old_count > 0 {
        let original = {
            let page = load_page(&entry.document, page_index)?;
            let objects = page.objects();
            validate_box_range(objects, start, old_count)?;
            read_style(objects, start)?
        };
        if original.font_is_embedded
            && !font_is_advertised(&original.font_family)
            && style_view.font_family.trim() == original.font_family
        {
            if style_view.bold != original.font_is_bold
                || style_view.italic != original.font_is_italic
                || style_view.underline
                || style_view.strikeout
                || style_view.alignment != TextAlignment::Left
            {
                return Err(CommandError::new(
                    CommandErrorCode::UnsupportedFont,
                    format!(
                        "the original font {} can keep only its size and colour; choose \
                         another font before changing weight, underline or alignment",
                        original.font_family
                    ),
                ));
            }
            let restyled = apply_for(
                entry,
                page_index,
                box_id,
                text,
                Some(LegacyRestyle {
                    font_size: style_view.font_size,
                    color: parse_color(&style_view.color)?,
                }),
            )?;
            let target = read_target(entry, page_index, box_id)?;
            return Ok(TextBoxEditResultView {
                page_index,
                target,
                font: restyled.font,
                history: restyled.history,
            });
        }
    }

    let new_style = TextBoxStyle::from_view(&style_view)?;
    let layout = style_change_layout(entry, page_index, record_index, new_style.font_size)?;
    check_layout(&layout)?;

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    drop(page);

    // A whole-box style change replaces every styled range with the new base
    // style: the user asked for one look for the whole box.
    let rendered = render_styled_box(entry, page_index, &layout, &new_style, &text, &[])?;
    let RenderedBox {
        objects,
        height,
        style: effective_style,
        fallback,
        ..
    } = rendered;
    let new_count = objects.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, objects)?;

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        let record = &mut records[record_index];
        record.style = Some(effective_style);
        record.runs.clear();
        record.layout = Some(BoxLayout { height, ..layout });
        record.width = layout.width;
        record.count = new_count;
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        new_count as isize - old_count as isize,
        ObjectRef::TextBox(box_id),
    );
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
        font: fallback,
        history: entry.edit.history_view(),
    })
}

/// Reads a legacy Text Object's look as a built-in style.
///
/// Selection styling needs a canonical base style to describe the text outside
/// the selected range. Only the project's registry fonts have a faithful style
/// representation; an arbitrary embedded font cannot be recreated through the
/// style panel, so it is refused with a message asking for a whole-box style
/// change first.
fn legacy_base_style(
    entry: &OpenDocument,
    page_index: u32,
    record_index: usize,
) -> Result<TextBoxStyle, CommandError> {
    let record = &entry.edit.records[&page_index][record_index];
    let page = load_page(&entry.document, page_index)?;
    let object = page.objects().get(record.start).map_err(|error| {
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
    let font_name = text_object.font().name();
    let matrix = text_object
        .matrix()
        .map(|matrix| {
            [
                matrix.a(),
                matrix.b(),
                matrix.c(),
                matrix.d(),
                matrix.e(),
                matrix.f(),
            ]
        })
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let font_size = display_font_size(text_object.unscaled_font_size().value, matrix);
    if !font_size.is_finite() || !(4.0..=400.0).contains(&font_size) {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedFont,
            format!("the Text Box font size {font_size} cannot be styled"),
        ));
    }

    let (font_family, bold, italic) = if fonts::is_bundled_family(&font_name) {
        let (bold, italic) = style_flags_from_font_name(&font_name);
        (fonts::normalize_family(&font_name), bold, italic)
    } else {
        let name = base_font_name(&font_name);
        let font = BASE14_FONTS
            .iter()
            .find(|font| font.postscript_name == name)
            .ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::UnsupportedFont,
                    format!(
                        "the original font {font_name} has no built-in variant; apply a whole-box \
                         style before styling a selection"
                    ),
                )
            })?;
        (font.family.to_owned(), font.bold, font.italic)
    };
    if italic && fonts::is_bundled_family(&font_family) {
        return Err(CommandError::new(
            CommandErrorCode::UnsupportedFont,
            format!("the bundled font {font_family} has no italic variant"),
        ));
    }

    let color = text_object
        .fill_color()
        .map(|color| (color.red(), color.green(), color.blue()))
        .unwrap_or((0, 0, 0));
    Ok(TextBoxStyle {
        font_family,
        font_size,
        bold,
        italic,
        underline: false,
        strikeout: false,
        color,
        alignment: TextAlignment::Left,
    })
}

/// Applies a style to one character range of a box.
///
/// The box is first given a canonical base style and layout when it is still a
/// legacy Text Object; the selected range becomes (or joins) a run, and only
/// that run's text objects are emitted with the new look.
fn selection_style_for(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    range: TextRangeView,
    style_view: TextBoxStyleView,
) -> Result<TextBoxEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let new_style = TextBoxStyle::from_view(&style_view)?;
    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, old_count, text, deleted, style, runs) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.start,
            record.count,
            record.text.clone(),
            record.deleted,
            record.style.clone(),
            record.runs.clone(),
        )
    };
    if deleted {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "this Text Box has been deleted",
        ));
    }

    let range_start = range.start as usize;
    let range_end = range.end as usize;
    let char_count = text.chars().count();
    if range_start >= range_end || range_end > char_count {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!(
                "selection {}..{} is invalid for the {} characters in Text Box {box_id}",
                range.start, range.end, char_count
            ),
        ));
    }

    let (base_style, layout) = match style {
        Some(style) => {
            let layout = entry.edit.records[&page_index][record_index]
                .layout
                .clone()
                .ok_or_else(|| {
                    CommandError::new(CommandErrorCode::Internal, "the Text Box lost its layout")
                })?;
            (style, layout)
        }
        None => {
            let base = legacy_base_style(entry, page_index, record_index)?;
            let layout = style_change_layout(entry, page_index, record_index, base.font_size)?;
            (base, layout)
        }
    };
    check_layout(&layout)?;

    let page = load_page(&entry.document, page_index)?;
    {
        let objects = page.objects();
        validate_box_range(objects, start, old_count)?;
    }
    drop(page);

    let runs = with_run_style(&runs, range_start, range_end, new_style);
    let rendered = render_styled_box(entry, page_index, &layout, &base_style, &text, &runs)?;
    let RenderedBox {
        objects,
        height,
        style: effective_style,
        runs: effective_runs,
        fallback,
    } = rendered;
    let new_count = objects.len();
    let before_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, objects)?;

    {
        let records = entry
            .edit
            .records
            .get_mut(&page_index)
            .expect("Text Box page disappeared");
        let record = &mut records[record_index];
        record.style = Some(effective_style);
        record.runs = effective_runs;
        record.layout = Some(BoxLayout { height, ..layout });
        record.width = layout.width;
        record.count = new_count;
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        new_count as isize - old_count as isize,
        ObjectRef::TextBox(box_id),
    );
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
        font: fallback,
        history: entry.edit.history_view(),
    })
}

fn set_box_matrices(
    entry: &OpenDocument,
    page_index: u32,
    box_id: u64,
    matrices: &[PdfMatrix],
) -> Result<(), CommandError> {
    let index = find_record(&entry.edit, page_index, box_id)?;
    let record = &entry.edit.records[&page_index][index];
    if record.count != matrices.len() {
        return Err(CommandError::new(
            CommandErrorCode::Internal,
            "the moved text object's range changed",
        ));
    }
    let page = load_page(&entry.document, page_index)?;
    for (offset, matrix) in matrices.iter().enumerate() {
        let mut object = page.objects().get(record.start + offset).map_err(|e| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("cannot read moved text: {e:?}"),
            )
        })?;
        object.reset_matrix(*matrix).map_err(|e| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("cannot move text: {e:?}"),
            )
        })?;
    }
    Ok(())
}

fn restore_text_move(
    entry: &mut OpenDocument,
    action: TextMoveAction,
    redo: bool,
) -> Result<EditStepView, CommandError> {
    let index = find_record(&entry.edit, action.page_index, action.box_id)?;
    set_box_matrices(
        entry,
        action.page_index,
        action.box_id,
        if redo {
            &action.after_matrices
        } else {
            &action.before_matrices
        },
    )?;
    entry.edit.records.get_mut(&action.page_index).unwrap()[index].layout = if redo {
        action.after_layout.clone()
    } else {
        action.before_layout.clone()
    };
    entry.edit.current_state = if redo {
        action.after_state
    } else {
        action.before_state
    };
    let page_index = action.page_index;
    if redo {
        entry.edit.undo_stack.push(EditAction::TextMove(action));
    } else {
        entry.edit.redo_stack.push(EditAction::TextMove(action));
    }
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

fn translate_box(
    entry: &mut OpenDocument,
    page_index: u32,
    box_id: u64,
    before: &TextBoxTargetView,
    x: f32,
    y: f32,
    frame_height: Option<f32>,
) -> Result<TextBoxEditResultView, CommandError> {
    let index = find_record(&entry.edit, page_index, box_id)?;
    let record = &entry.edit.records[&page_index][index];
    let before_layout = record.layout.clone();
    let mut after_layout = before_layout.clone().map(|mut layout| {
        layout.x = x;
        layout.y = y;
        layout
    });
    let page = load_page(&entry.document, page_index)?;
    let rotation = page
        .rotation()
        .map_err(|e| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("cannot read rotation: {e:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    if let Some(height) = frame_height {
        let (layer, raw) = build_text_layer_with_raw_indices(&page, page_index)?;
        let text = page.text().map_err(|_| no_editable_text(page_index))?;
        let minimum = box_display_rect(&layer, &raw, &text, page.objects(), record)
            .map(|(_, top, _, content_height)| top + content_height - before.y)
            .unwrap_or(before.font_size * 1.2)
            .max(before.font_size * 1.2);
        after_layout = Some(BoxLayout {
            x,
            y,
            width: before.width,
            height: height.max(minimum),
            rotation: before.rotation,
            auto_height: true,
            line_height: before_layout.as_ref().and_then(|layout| layout.line_height),
        });
    }
    let (zero_x, zero_y) = geometry.point_inverse(0.0, 0.0);
    let (delta_x, delta_y) = geometry.point_inverse(x - before.x, y - before.y);
    let (dx, dy) = (delta_x - zero_x, delta_y - zero_y);
    let mut before_matrices = Vec::with_capacity(record.count);
    for offset in 0..record.count {
        let object = page.objects().get(record.start + offset).map_err(|e| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("cannot read text matrix: {e:?}"),
            )
        })?;
        before_matrices.push(object.matrix().map_err(|e| {
            CommandError::new(
                CommandErrorCode::Internal,
                format!("cannot read text matrix: {e:?}"),
            )
        })?);
    }
    drop(page);
    let after_matrices: Vec<_> = before_matrices
        .iter()
        .map(|m| PdfMatrix::new(m.a(), m.b(), m.c(), m.d(), m.e() + dx, m.f() + dy))
        .collect();
    if let Err(error) = set_box_matrices(entry, page_index, box_id, &after_matrices) {
        set_box_matrices(entry, page_index, box_id, &before_matrices)?;
        return Err(error);
    }
    entry.edit.records.get_mut(&page_index).unwrap()[index].layout = after_layout.clone();
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry.edit.current_state = after_state;
    entry.edit.redo_stack.clear();
    entry
        .edit
        .undo_stack
        .push(EditAction::TextMove(TextMoveAction {
            page_index,
            box_id,
            before_state,
            after_state,
            before_matrices,
            after_matrices,
            before_layout,
            after_layout,
        }));
    Ok(TextBoxEditResultView {
        page_index,
        target: read_target(entry, page_index, box_id)?,
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
    let mut layout = BoxLayout {
        x: request.x,
        y: request.y,
        width: request.width,
        height: request.height,
        rotation: normalize_angle(request.rotation),
        auto_height: request.auto_height,
        line_height: None,
    };
    check_layout(&layout)?;

    let original = read_target(entry, page_index, box_id)?;
    layout.line_height = original.line_height;
    if (layout.width - original.width).abs() < 0.01
        && (layout.height - original.height).abs() < 0.01
        && (layout.rotation - original.rotation).abs() < 0.01
        && layout.auto_height == original.auto_height
    {
        if (layout.x - original.x).abs() < 0.01 && (layout.y - original.y).abs() < 0.01 {
            return Ok(TextBoxEditResultView {
                page_index,
                target: original,
                font: None,
                history: entry.edit.history_view(),
            });
        }
        return translate_box(
            entry, page_index, box_id, &original, layout.x, layout.y, None,
        );
    }

    if (layout.width - original.width).abs() < 0.01
        && layout.rotation.abs() < 0.01
        && original.rotation.abs() < 0.01
        && layout.auto_height
    {
        return translate_box(
            entry,
            page_index,
            box_id,
            &original,
            layout.x,
            layout.y,
            Some(layout.height),
        );
    }

    let record_index = find_record(&entry.edit, page_index, box_id)?;
    let (start, old_count, text, deleted, style, runs) = {
        let record = &entry.edit.records[&page_index][record_index];
        (
            record.start,
            record.count,
            record.text.clone(),
            record.deleted,
            record.style.clone(),
            record.runs.clone(),
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
    drop(page);

    // A transform keeps every styled range: the text and its offsets do not
    // change, only the frame it is laid out in.
    let (replacements, height, effective_style, effective_runs, fallback) = match &style {
        Some(style) => {
            let rendered = render_styled_box(entry, page_index, &layout, style, &text, &runs)?;
            let RenderedBox {
                objects,
                height,
                style: effective_style,
                runs: effective_runs,
                fallback,
            } = rendered;
            (
                objects,
                height,
                Some(effective_style),
                effective_runs,
                fallback,
            )
        }
        None => {
            let prepared = prepare_legacy_render(entry, page_index, record_index, &text, None)?;
            let lines = wrap_styled(
                &entry.document,
                &text,
                &prepared.char_styles,
                layout.width,
                &prepared.styles,
            );
            let height = effective_height(
                &layout,
                lines.len() as f32
                    * layout
                        .line_height
                        .unwrap_or(0.0)
                        .max(prepared.font_size * 1.2),
            );
            let objects = build_styled_box_objects(
                &mut entry.document,
                &geometry,
                &layout,
                TextAlignment::Left,
                &prepared.styles,
                &lines,
                height,
                Some(prepared.style.render_mode),
            )?;
            (objects, height, None, Vec::new(), prepared.plan.fallback)
        }
    };
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
        if let Some(style) = effective_style {
            record.style = Some(style);
            record.runs = effective_runs;
        }
        record.layout = Some(BoxLayout { height, ..layout });
        record.width = layout.width;
        record.count = new_count;
    }
    let after_snapshot = entry.edit.records[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        new_count as isize - old_count as isize,
        ObjectRef::TextBox(box_id),
    );
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
        font: fallback,
        history: entry.edit.history_view(),
    })
}

/// The `apply_text_edit` command: replaces a Text Box's content, wrapping at
/// its width and keeping the original baseline, font, size and colour when
/// possible.
///
/// Styled ranges are tied to character offsets: unchanged normalized text keeps
/// the existing runs, while any replacement resets the box to its base style so
/// a run never silently points at different characters.
#[tauri::command]
pub fn apply_text_edit(
    document_id: String,
    page_index: u32,
    box_id: u64,
    text: String,
) -> Result<TextEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = apply_for(entry, physical, box_id, text, None)?;
        view.page_index = entry.logical_page(physical).unwrap_or(0);
        Ok(view)
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
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = delete_for(entry, physical, box_id)?;
        view.page_index = entry.logical_page(physical).unwrap_or(0);
        Ok(view)
    })
}

/// The `create_text_box` command: adds an empty, visible Text Box and records
/// its creation in the document-wide undo history.
#[tauri::command]
pub fn create_text_box(
    document_id: String,
    page_index: u32,
    request: CreateTextBoxRequest,
) -> Result<TextBoxEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = create_for(entry, physical, request)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.target.page_index = logical;
        Ok(view)
    })
}

/// The `text_boxes` command: every Text Box on the page in registration
/// order, including empty created boxes and existing Text Objects that were
/// resolved by a click this session. Deleted boxes are excluded.
#[tauri::command]
pub fn text_boxes(
    document_id: String,
    page_index: u32,
) -> Result<Vec<TextBoxTargetView>, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut views = boxes_for(entry, physical)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        for view in &mut views {
            view.page_index = logical;
        }
        Ok(views)
    })
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
        let physical = entry.physical_page(page_index)?;
        let mut view = style_for(entry, physical, box_id, style)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.target.page_index = logical;
        Ok(view)
    })
}

/// The `apply_text_box_selection_style` command: styles only the characters in
/// `range`, leaving the rest of the Text Box at its base style.
#[tauri::command]
pub fn apply_text_box_selection_style(
    document_id: String,
    page_index: u32,
    box_id: u64,
    range: TextRangeView,
    style: TextBoxStyleView,
) -> Result<TextBoxEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = selection_style_for(entry, physical, box_id, range, style)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.target.page_index = logical;
        Ok(view)
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
        let physical = entry.physical_page(page_index)?;
        let mut view = transform_for(entry, physical, box_id, transform)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.target.page_index = logical;
        Ok(view)
    })
}

/// Captures reusable native glyph pixels for smooth client-side translation.
#[tauri::command]
pub fn prepare_text_box_move_preview(
    document_id: String,
    page_index: u32,
    box_id: u64,
    scale: f32,
) -> Result<Option<crate::text_move_preview::TextBoxMovePreview>, CommandError> {
    let captured = with_open_document(&document_id, |entry| {
        ensure_content_editable(&entry.document)?;
        let physical = entry.physical_page(page_index)?;
        let index = find_record(&entry.edit, physical, box_id)?;
        let record = &entry.edit.records[&physical][index];
        if record.deleted || record.count == 0 {
            return Ok(None);
        }
        let range = record.start..record.start + record.count;
        crate::text_move_preview::capture(entry, page_index, range, scale)
    })?;
    captured
        .map(|capture| capture.encode(document_id, page_index))
        .transpose()
}

/// A drag preview rendered with the real PDF font metrics and page background.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxTransformPreview {
    pub image: crate::commands::PageImageView,
    pub target: TextBoxTargetView,
}

/// Render a reversible transform while holding the registry lock. No caller
/// can observe the intermediate page; history, redo, and the original objects
/// are restored before returning, even if rasterization fails. State IDs stay
/// monotonic so layer-render caches never reuse a previous preview's pixels.
#[tauri::command]
pub fn preview_text_box_transform(
    document_id: String,
    page_index: u32,
    box_id: u64,
    transform: TextBoxTransformRequest,
    scale: f32,
) -> Result<TextBoxTransformPreview, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let before_len = entry.edit.undo_stack.len();
        let saved_redo = std::mem::take(&mut entry.edit.redo_stack);
        let transformed = transform_for(entry, physical, box_id, transform);
        let rendered = match &transformed {
            Ok(_) => crate::commands::render_page_preview_for(
                entry,
                &document_id,
                page_index,
                scale.clamp(0.1, 2.0),
            ),
            Err(error) => Err(error.clone()),
        };
        let restored = if entry.edit.undo_stack.len() > before_len {
            undo_for(entry).map(|_| ())
        } else {
            Ok(())
        };
        entry.edit.redo_stack = saved_redo;
        restored?;
        let mut target = transformed?.target;
        target.page_index = page_index;
        Ok(TextBoxTransformPreview {
            image: rendered?,
            target,
        })
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
/// This is the contract the save pipeline (ticket 10) applies *after* a
/// successful write, through the same [`EditSession::mark_document_saved`]
/// transition. It sets the clean baseline without clearing the undo/redo
/// stacks, so history survives Save Changes and the user can undo past the
/// save point. `save_document_bytes` serializes the live in-memory document
/// for that pipeline without reloading it, so the same history stays valid.
#[tauri::command]
pub fn mark_document_saved(document_id: String) -> Result<EditHistoryView, CommandError> {
    with_open_document(&document_id, |entry| {
        entry.edit.mark_document_saved();
        Ok(entry.edit.history_view())
    })
}

#[cfg(test)]
mod tests {
    use super::face_paints;

    #[test]
    fn font_coverage_requires_a_painted_outline() {
        let face = crate::fonts::noto_sans_sc()
            .face()
            .expect("the bundled font parses");

        assert!(face_paints(&face, '改'));
        assert!(face_paints(&face, ' '));
        assert!(!face_paints(&face, '\u{E000}'));
    }
}
