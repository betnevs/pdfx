//! Native annotations: text markup (highlight, underline, strikeout),
//! geometric shapes (rectangle, ellipse), sticky notes and freehand ink.
//!
//! Annotations are native PDFium annotations. Every created annotation carries
//! its author (`/T`), creation time (`/CreationDate`), a stable `/NM` name and
//! a custom normal appearance stream (`/AP`). Pdfium does not synthesize
//! appearances, so the stream is generated here; without it the saved file
//! would show nothing in other readers.
//!
//! Text markup additionally uses standard colour (`/C`), constant opacity
//! (`/CA`) and quadrilateral geometry (`/QuadPoints`). Shapes use `/C` for the
//! stroke, `/IC` for the fill, `/Border` for the line width and `/Rect` for the
//! axis-aligned bounding box; the visible rectangle or ellipse, including its
//! rotation, is drawn by the appearance stream. Rotation is therefore real PDF
//! content, not a canvas-only effect: the saved document and the editor show
//! the same rotated shape, and PDFX stores the unrotated geometry and angle in
//! private keys so reopening restores the exact editable shape. Sticky notes
//! store their body in `/Contents`, and ink stores every stroke in `/InkList`
//! together with its border width.
//!
//! Annotations are identified by their `/NM` name. The name is stable across
//! edits and undo/redo, which makes it safe to look an annotation up again
//! after another annotation was deleted. Pre-existing unnamed annotations are
//! rendered by PDFium but intentionally not listed: they have no stable
//! identity and no author/creation metadata to manage.
//!
//! Colour, opacity and ink line width are also mirrored into private string
//! keys (`PDFXColor`, `PDFXOpacity`, `PDFXLineWidth`, and the shape keys
//! below). Pdfium refuses to read `/C` and `/CA` back once an appearance stream
//! exists; the private keys keep the annotation list correct without touching
//! the standard, reader-visible entries.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pdfium_render::prelude::{PdfDocument, PdfPageAnnotationType};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry,
};
use crate::pdfium_adapter::{
    annotation_author, annotation_border_width, annotation_bounds, annotation_color,
    annotation_contents, annotation_creation_date, annotation_fill_color, annotation_ink_strokes,
    annotation_name, annotation_quads, annotation_string_value, annotation_type,
    append_annotation_quads, append_ink_strokes,
    create_ink_annotation as create_native_ink_annotation,
    create_shape_annotation as create_shape_native,
    create_text_annotation as create_native_text_annotation,
    create_text_markup_annotation as create_markup_annotation, remove_annotation,
    set_annotation_appearance, set_annotation_author, set_annotation_border_width,
    set_annotation_color, set_annotation_contents, set_annotation_creation_date,
    set_annotation_fill_color, set_annotation_name, set_annotation_string_value, AnnotationRect,
    AnnotationRef, ShapeAnnotationKind, TextMarkupAnnotationKind,
};
use crate::settings::Settings;
use crate::text_edit::{push_annotation_action, EditHistoryView};

/// Private annotation keys mirroring `/C`, `/CA` and `/BS /W` for reliable
/// read-back.
const COLOR_KEY: &str = "PDFXColor";
const OPACITY_KEY: &str = "PDFXOpacity";
/// Ink stroke width and shape line width share one private key.
const LINE_WIDTH_KEY: &str = "PDFXLineWidth";
/// Private shape keys. Pdfium refuses `/IC` and `/Border` read-back once the
/// custom appearance is installed, and `/Rect` only holds the rotated bounding
/// box, so the editable geometry and style are mirrored here.
const SHAPE_GEOMETRY_KEY: &str = "PDFXShapeGeometry";
const SHAPE_FILL_KEY: &str = "PDFXFillColor";
const SHAPE_STROKE_KEY: &str = "PDFXStrokeColor";
/// Marker stored in a shape colour key for "no fill" / "no stroke".
const NO_COLOR: &str = "none";
const DEFAULT_MARKUP_COLOR: &str = "#ffd400";
const DEFAULT_NOTE_COLOR: &str = "#f9a825";
const DEFAULT_INK_COLOR: &str = "#c62828";
/// Underline and strikeout stroke width in page points.
const MARKUP_LINE_WIDTH: f32 = 1.5;
const DEFAULT_INK_LINE_WIDTH: f32 = 3.0;
const MIN_INK_LINE_WIDTH: f32 = 0.5;
const MAX_INK_LINE_WIDTH: f32 = 32.0;
/// Upper bound for a shape border, in page points.
const MAX_SHAPE_LINE_WIDTH: f32 = 100.0;
/// Extra room around a shape `/Rect` so the appearance boundary never clips
/// the stroke; the editable geometry is recovered from [SHAPE_GEOMETRY_KEY].
const SHAPE_RECT_PADDING: f32 = 0.5;
const MAX_AUTHOR_CHARS: usize = 64;
const MAX_NOTE_CHARS: usize = 5_000;
const MAX_INK_POINTS: usize = 20_000;
/// Smallest note box accepted from the frontend, in display points.
const MIN_NOTE_SIZE: f32 = 4.0;

static ANNOTATION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The three text markup annotations PDFX can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextMarkupKind {
    Highlight,
    Underline,
    Strikeout,
}

/// Every annotation kind PDFX can create and manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationKind {
    Highlight,
    Underline,
    Strikeout,
    Square,
    Circle,
    Note,
    Ink,
}

impl From<TextMarkupKind> for TextMarkupAnnotationKind {
    fn from(kind: TextMarkupKind) -> Self {
        match kind {
            TextMarkupKind::Highlight => Self::Highlight,
            TextMarkupKind::Underline => Self::Underline,
            TextMarkupKind::Strikeout => Self::Strikeout,
        }
    }
}

impl From<TextMarkupKind> for AnnotationKind {
    fn from(kind: TextMarkupKind) -> Self {
        match kind {
            TextMarkupKind::Highlight => Self::Highlight,
            TextMarkupKind::Underline => Self::Underline,
            TextMarkupKind::Strikeout => Self::Strikeout,
        }
    }
}

impl From<ShapeKind> for AnnotationKind {
    fn from(kind: ShapeKind) -> Self {
        match kind {
            ShapeKind::Square => Self::Square,
            ShapeKind::Circle => Self::Circle,
        }
    }
}

impl AnnotationKind {
    fn from_pdfium(kind: PdfPageAnnotationType) -> Option<Self> {
        match kind {
            PdfPageAnnotationType::Highlight => Some(Self::Highlight),
            PdfPageAnnotationType::Underline => Some(Self::Underline),
            PdfPageAnnotationType::Strikeout => Some(Self::Strikeout),
            PdfPageAnnotationType::Square => Some(Self::Square),
            PdfPageAnnotationType::Circle => Some(Self::Circle),
            PdfPageAnnotationType::Text => Some(Self::Note),
            PdfPageAnnotationType::Ink => Some(Self::Ink),
            _ => None,
        }
    }

    fn text_markup(self) -> Option<TextMarkupKind> {
        match self {
            Self::Highlight => Some(TextMarkupKind::Highlight),
            Self::Underline => Some(TextMarkupKind::Underline),
            Self::Strikeout => Some(TextMarkupKind::Strikeout),
            Self::Square | Self::Circle | Self::Note | Self::Ink => None,
        }
    }
}

/// The two geometric shapes PDFX can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeKind {
    Square,
    Circle,
}

impl From<ShapeKind> for ShapeAnnotationKind {
    fn from(kind: ShapeKind) -> Self {
        match kind {
            ShapeKind::Square => Self::Square,
            ShapeKind::Circle => Self::Circle,
        }
    }
}

impl ShapeKind {
    fn from_annotation(kind: AnnotationKind) -> Option<Self> {
        match kind {
            AnnotationKind::Square => Some(Self::Square),
            AnnotationKind::Circle => Some(Self::Circle),
            _ => None,
        }
    }
}

/// Editable shape geometry in display-space points.
///
/// `x`/`y`/`width`/`height` describe the unrotated rectangle (top-left origin,
/// page rotation and CropBox already applied). `rotation` is clockwise degrees
/// around that rectangle's centre, matching CSS `rotate()` in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
}

/// Fill, stroke, opacity and line width of a shape.
///
/// A `None` colour means the shape has no fill or no stroke; at least one of
/// the two must be present. `opacity` is a percentage (0-100) applied to the
/// whole annotation. `line_width` is in page points and is ignored without a
/// stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeStyle {
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub opacity: u8,
    pub line_width: f32,
}

/// The frontend's request to draw a shape on one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShapeRequest {
    pub page_index: u32,
    pub kind: ShapeKind,
    pub geometry: ShapeGeometry,
    pub style: ShapeStyle,
}

/// A shape annotation as the overlay and property panel consume it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeView {
    pub kind: ShapeKind,
    pub geometry: ShapeGeometry,
    pub style: ShapeStyle,
}

/// A rectangle in display-space points: top-left origin, page rotation and
/// CropBox already applied. This is the same coordinate system as the text
/// layer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextMarkupRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One freehand point in display-space points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InkPoint {
    pub x: f32,
    pub y: f32,
}

/// The frontend's request to mark up one or more selected lines.
///
/// `rects` are display-space boxes, one per selected line. `opacity` is a
/// percentage (0-100); `color` is `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTextMarkupRequest {
    pub page_index: u32,
    pub kind: TextMarkupKind,
    pub color: String,
    pub opacity: u8,
    pub rects: Vec<TextMarkupRect>,
}

/// The frontend's request to place a sticky note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoteRequest {
    pub page_index: u32,
    /// Icon colour as `#rrggbb`.
    pub color: String,
    /// Constant opacity in percent (0-100).
    pub opacity: u8,
    /// Note body; may be empty and is edited later through `update_note_text`.
    pub text: String,
    /// Display-space box of the note icon.
    pub rect: TextMarkupRect,
}

/// The frontend's request to draw one freehand annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInkRequest {
    pub page_index: u32,
    pub color: String,
    pub opacity: u8,
    /// Stroke width in display points.
    pub line_width: f32,
    /// Display-space strokes; one entry per pen-down/pen-up gesture.
    pub strokes: Vec<Vec<InkPoint>>,
}

/// One annotation as the annotation list and overlay consume it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationView {
    pub name: String,
    pub page_index: u32,
    pub kind: AnnotationKind,
    pub author: Option<String>,
    pub created_at: Option<String>,
    /// Text markup colour, or the shape's stroke (falling back to fill) colour,
    /// so the list swatch has one colour for every kind.
    pub color: String,
    pub opacity: u8,
    /// Ink stroke width in display points; `None` for other kinds.
    pub line_width: Option<f32>,
    /// Sticky-note body; `None` for other kinds.
    pub text: Option<String>,
    /// Display-space boxes: one per quadrilateral for markup, the icon box for
    /// a note and the ink bounds for a stroke annotation; empty for shapes.
    pub rects: Vec<TextMarkupRect>,
    /// Display-space ink strokes; empty for other kinds.
    pub strokes: Vec<Vec<InkPoint>>,
    /// Shape geometry and style; `None` for every other kind.
    pub shape: Option<ShapeView>,
}

/// The outcome of creating, restyling or deleting one annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationMutationView {
    /// The annotation after the change; `None` after a deletion.
    pub annotation: Option<AnnotationView>,
    pub page_index: u32,
    pub history: EditHistoryView,
}

/// Everything undo/redo needs to recreate an annotation or restyle it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AnnotationSnapshot {
    pub(crate) page_index: u32,
    pub(crate) name: String,
    pub(crate) kind: AnnotationKind,
    pub(crate) author: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) color: String,
    pub(crate) opacity: u8,
    /// Ink stroke width; `None` for other kinds.
    pub(crate) line_width: Option<f32>,
    /// Sticky-note body; `None` for other kinds.
    pub(crate) text: Option<String>,
    /// Bounds in PDF user space.
    pub(crate) rect: AnnotationRect,
    /// Quadrilaterals in PDF user space, in the order PDFX creates them.
    pub(crate) quads: Vec<[f32; 8]>,
    /// Display-space boxes derived from `quads`, `rect` or the shape geometry.
    pub(crate) rects: Vec<TextMarkupRect>,
    /// Ink strokes in PDF user space.
    pub(crate) strokes: Vec<Vec<(f32, f32)>>,
    /// Shape geometry and style; `None` for every other kind.
    pub(crate) shape: Option<ShapeView>,
}

impl AnnotationSnapshot {
    fn view(&self, geometry: &DisplayGeometry) -> AnnotationView {
        AnnotationView {
            name: self.name.clone(),
            page_index: self.page_index,
            kind: self.kind,
            author: self.author.clone(),
            created_at: self.created_at.clone(),
            color: self.color.clone(),
            opacity: self.opacity,
            line_width: self.line_width,
            text: self.text.clone(),
            rects: self.rects.clone(),
            strokes: self
                .strokes
                .iter()
                .map(|stroke| {
                    stroke
                        .iter()
                        .map(|&(x, y)| {
                            let (x, y) = geometry.point(x, y);
                            InkPoint { x, y }
                        })
                        .collect()
                })
                .collect(),
            shape: self.shape.clone(),
        }
    }
}

fn invalid_argument(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn internal_error(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::Internal, message)
}

fn map_adapter_error(error: crate::pdfium_adapter::AdapterError) -> CommandError {
    internal_error(format!("PDFium annotation operation failed: {error}"))
}

/// Annotation permission is separate from content-editing permission: a
/// document may allow highlights while forbidding text edits.
pub(crate) fn ensure_annotation_editable(document: &PdfDocument<'_>) -> Result<(), CommandError> {
    let allowed = document
        .permissions()
        .can_add_or_modify_text_annotations()
        .unwrap_or(false);
    if allowed {
        Ok(())
    } else {
        Err(CommandError::new(
            CommandErrorCode::PermissionDenied,
            "this document does not allow annotations",
        ))
    }
}

fn normalize_author(author: &str) -> Result<String, CommandError> {
    let author = author.trim();
    if author.is_empty() {
        return Err(invalid_argument("the annotation author must not be empty"));
    }
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(invalid_argument(format!(
            "the annotation author must not exceed {MAX_AUTHOR_CHARS} characters"
        )));
    }
    if author.chars().any(char::is_control) {
        return Err(invalid_argument(
            "the annotation author must not contain control characters",
        ));
    }
    Ok(author.to_owned())
}

/// Validates a note body. Newlines and tabs are legal; other control
/// characters are not printable and would corrupt the `/Contents` value.
fn normalize_note_text(text: &str) -> Result<String, CommandError> {
    if text.chars().count() > MAX_NOTE_CHARS {
        return Err(invalid_argument(format!(
            "the note text must not exceed {MAX_NOTE_CHARS} characters"
        )));
    }
    if text
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(invalid_argument(
            "the note text must not contain control characters",
        ));
    }
    Ok(text.to_owned())
}

fn parse_color(color: &str) -> Result<(u8, u8, u8), CommandError> {
    let hex = color
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit()))
        .ok_or_else(|| invalid_argument(format!("invalid annotation colour {color:?}")))?;
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16).expect("validated hexadecimal colour channel")
    };
    Ok((channel(0..2), channel(2..4), channel(4..6)))
}

fn format_color(red: u8, green: u8, blue: u8) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn opacity_to_alpha(opacity: u8) -> u8 {
    ((u32::from(opacity) * 255 + 50) / 100) as u8
}

fn alpha_to_opacity(alpha: u8) -> u8 {
    ((u32::from(alpha) * 100 + 127) / 255) as u8
}

fn validate_color_opacity(color: &str, opacity: u8) -> Result<(u8, u8, u8), CommandError> {
    let rgb = parse_color(color)?;
    if opacity > 100 {
        return Err(invalid_argument(
            "the annotation opacity must be between 0 and 100",
        ));
    }
    Ok(rgb)
}

fn validate_rect(rect: TextMarkupRect, context: &str) -> Result<(), CommandError> {
    let finite = rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite();
    if !finite || rect.width < MIN_NOTE_SIZE || rect.height < MIN_NOTE_SIZE {
        return Err(invalid_argument(format!(
            "{context} needs finite geometry of at least {MIN_NOTE_SIZE} points"
        )));
    }
    Ok(())
}

fn validate_ink_line_width(width: f32) -> Result<(), CommandError> {
    if !width.is_finite() || !(MIN_INK_LINE_WIDTH..=MAX_INK_LINE_WIDTH).contains(&width) {
        return Err(invalid_argument(format!(
            "the ink line width must be between {MIN_INK_LINE_WIDTH} and {MAX_INK_LINE_WIDTH} points"
        )));
    }
    Ok(())
}

fn validate_request(request: &CreateTextMarkupRequest) -> Result<(u8, u8, u8), CommandError> {
    let rgb = validate_color_opacity(&request.color, request.opacity)?;
    if request.rects.is_empty() {
        return Err(invalid_argument(
            "a text markup needs at least one rectangle",
        ));
    }
    for rect in &request.rects {
        let finite = rect.x.is_finite()
            && rect.y.is_finite()
            && rect.width.is_finite()
            && rect.height.is_finite();
        if !finite || rect.width <= 0.0 || rect.height <= 0.0 {
            return Err(invalid_argument(
                "annotation rectangles need finite, positive geometry",
            ));
        }
    }
    Ok(rgb)
}

fn validate_ink_request(request: &CreateInkRequest) -> Result<(u8, u8, u8), CommandError> {
    let rgb = validate_color_opacity(&request.color, request.opacity)?;
    validate_ink_line_width(request.line_width)?;
    if request.strokes.is_empty() {
        return Err(invalid_argument(
            "an ink annotation needs at least one stroke",
        ));
    }
    let mut total = 0usize;
    for stroke in &request.strokes {
        if stroke.is_empty() {
            return Err(invalid_argument("an ink stroke needs at least one point"));
        }
        total += stroke.len();
        if stroke
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return Err(invalid_argument(
                "ink points need finite display coordinates",
            ));
        }
    }
    if total > MAX_INK_POINTS {
        return Err(invalid_argument(format!(
            "an ink annotation must not exceed {MAX_INK_POINTS} points"
        )));
    }
    Ok(rgb)
}

fn normalize_shape_geometry(mut geometry: ShapeGeometry) -> Result<ShapeGeometry, CommandError> {
    let finite = geometry.x.is_finite()
        && geometry.y.is_finite()
        && geometry.width.is_finite()
        && geometry.height.is_finite()
        && geometry.rotation.is_finite();
    if !finite || geometry.width <= 0.0 || geometry.height <= 0.0 {
        return Err(invalid_argument("a shape needs finite, positive geometry"));
    }
    geometry.rotation = geometry.rotation.rem_euclid(360.0);
    Ok(geometry)
}

fn normalize_optional_color(color: Option<&str>) -> Result<Option<String>, CommandError> {
    match color {
        None => Ok(None),
        Some(color) => {
            let (red, green, blue) = parse_color(color)?;
            Ok(Some(format_color(red, green, blue)))
        }
    }
}

fn normalize_shape_style(mut style: ShapeStyle) -> Result<ShapeStyle, CommandError> {
    style.fill_color = normalize_optional_color(style.fill_color.as_deref())?;
    style.stroke_color = normalize_optional_color(style.stroke_color.as_deref())?;
    if style.opacity > 100 {
        return Err(invalid_argument(
            "the annotation opacity must be between 0 and 100",
        ));
    }
    if !style.line_width.is_finite()
        || style.line_width <= 0.0
        || style.line_width > MAX_SHAPE_LINE_WIDTH
    {
        return Err(invalid_argument(format!(
            "the shape line width must be greater than 0 and at most {MAX_SHAPE_LINE_WIDTH}"
        )));
    }
    if style.fill_color.is_none() && style.stroke_color.is_none() {
        return Err(invalid_argument(
            "a shape needs a fill colour, a stroke colour, or both",
        ));
    }
    Ok(style)
}

/// The PDF-space appearance and bounding box of one shape.
struct ShapeAppearance {
    appearance: String,
    bounds: AnnotationRect,
}

/// Maps a point in the shape's local unit space (`[-1, 1]` on each axis) to
/// PDF user space, applying the shape rotation and then the page transform.
fn shape_point(
    page: &DisplayGeometry,
    geometry: &ShapeGeometry,
    local_x: f32,
    local_y: f32,
) -> (f32, f32) {
    let centre_x = geometry.x + geometry.width / 2.0;
    let centre_y = geometry.y + geometry.height / 2.0;
    let radians = geometry.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let dx = local_x * geometry.width / 2.0;
    let dy = local_y * geometry.height / 2.0;
    page.point_inverse(
        centre_x + dx * cos - dy * sin,
        centre_y + dx * sin + dy * cos,
    )
}

/// Control points of the four cubic Bézier arcs that make a unit circle.
/// The affine page/shape transform maps these exactly onto the ellipse, so the
/// saved appearance is a real ellipse rather than a polygon approximation.
const UNIT_CIRCLE_SEGMENTS: [[(f32, f32); 3]; 4] = {
    const K: f32 = 0.552_284_8;
    [
        [(1.0, K), (K, 1.0), (0.0, 1.0)],
        [(-K, 1.0), (-1.0, K), (-1.0, 0.0)],
        [(-1.0, -K), (-K, -1.0), (0.0, -1.0)],
        [(K, -1.0), (1.0, -K), (1.0, 0.0)],
    ]
};

fn append_shape_path(
    stream: &mut String,
    kind: ShapeKind,
    page: &DisplayGeometry,
    geometry: &ShapeGeometry,
) {
    match kind {
        ShapeKind::Square => {
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            for (index, (x, y)) in corners.into_iter().enumerate() {
                let (pdf_x, pdf_y) = shape_point(page, geometry, x, y);
                let operator = if index == 0 { "m" } else { "l" };
                stream.push_str(&format!("{pdf_x:.4} {pdf_y:.4} {operator}\n"));
            }
            stream.push_str("h\n");
        }
        ShapeKind::Circle => {
            let (start_x, start_y) = shape_point(page, geometry, 1.0, 0.0);
            stream.push_str(&format!("{start_x:.4} {start_y:.4} m\n"));
            for segment in UNIT_CIRCLE_SEGMENTS {
                for (x, y) in segment {
                    let (pdf_x, pdf_y) = shape_point(page, geometry, x, y);
                    stream.push_str(&format!("{pdf_x:.4} {pdf_y:.4} "));
                }
                stream.push_str("c\n");
            }
            stream.push_str("h\n");
        }
    }
}

/// Builds the normal appearance stream for a shape and the `/Rect` bounding
/// box that contains it.
///
/// The stream is drawn in PDF user space (Pdfium interprets custom appearance
/// content in page coordinates with `/Rect` as the Form XObject `/BBox`), so
/// page rotation, CropBox offset and the shape's own rotation are all baked
/// into the path. Other readers therefore show exactly what the editor
/// previewed.
fn shape_appearance(
    page: &DisplayGeometry,
    kind: ShapeKind,
    geometry: &ShapeGeometry,
    style: &ShapeStyle,
) -> Result<ShapeAppearance, CommandError> {
    let fill = style.fill_color.as_deref().map(parse_color).transpose()?;
    let stroke = style.stroke_color.as_deref().map(parse_color).transpose()?;

    let mut left = f32::INFINITY;
    let mut bottom = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::NEG_INFINITY;
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (pdf_x, pdf_y) = shape_point(page, geometry, x, y);
        left = left.min(pdf_x);
        right = right.max(pdf_x);
        bottom = bottom.min(pdf_y);
        top = top.max(pdf_y);
    }
    let padding = if stroke.is_some() {
        style.line_width / 2.0 + SHAPE_RECT_PADDING
    } else {
        SHAPE_RECT_PADDING
    };
    let bounds = AnnotationRect::new(
        bottom - padding,
        left - padding,
        top + padding,
        right + padding,
    );

    let mut stream = String::new();
    // Pdfium adds an /ExtGState named /GS carrying the annotation's constant
    // opacity to the Form XObject resources when the appearance is installed.
    stream.push_str("q\n/GS gs\n");
    if let Some((red, green, blue)) = fill {
        stream.push_str(&format!(
            "{:.4} {:.4} {:.4} rg\n",
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0
        ));
    }
    if let Some((red, green, blue)) = stroke {
        stream.push_str(&format!(
            "{:.4} {:.4} {:.4} RG {:.4} w\n",
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0,
            style.line_width
        ));
    }
    append_shape_path(&mut stream, kind, page, geometry);
    stream.push_str(match (fill.is_some(), stroke.is_some()) {
        (true, true) => "B\n",
        (true, false) => "f\n",
        (false, true) => "S\n",
        (false, false) => unreachable!("style validation requires a fill or a stroke"),
    });
    stream.push_str("Q\n");

    Ok(ShapeAppearance {
        appearance: stream,
        bounds,
    })
}

fn parse_shape_geometry(value: &str) -> Option<ShapeGeometry> {
    let values: Vec<Option<f32>> = value
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok())
        .collect();
    if values.len() != 5 {
        return None;
    }
    let geometry = ShapeGeometry {
        x: values[0]?,
        y: values[1]?,
        width: values[2]?,
        height: values[3]?,
        rotation: values[4]?,
    };
    let finite = geometry.x.is_finite()
        && geometry.y.is_finite()
        && geometry.width.is_finite()
        && geometry.height.is_finite()
        && geometry.rotation.is_finite();
    (finite && geometry.width > 0.0 && geometry.height > 0.0).then_some(geometry)
}

fn format_shape_geometry(geometry: &ShapeGeometry) -> String {
    format!(
        "{:.4},{:.4},{:.4},{:.4},{:.4}",
        geometry.x, geometry.y, geometry.width, geometry.height, geometry.rotation
    )
}

/// PDF date string (`D:YYYYMMDDHHmmSSZ`) for the current UTC time.
fn pdf_date_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    format!("D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z")
}

/// Converts days since the Unix epoch to a civil UTC date
/// (Howard Hinnant's `civil_from_days` algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn next_annotation_name() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let counter = ANNOTATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("pdfx-{}-{millis}-{counter}", std::process::id())
}

/// Maps one display-space box to a PDF-space quadrilateral.
///
/// The corner order matches `PdfQuadPoints::from_rect` (counter-clockwise in
/// PDF user space): bottom-left, bottom-right, top-right, top-left of the
/// *display* box. Keeping this order is what lets the appearance generator draw
/// underlines on the visual bottom edge, including on rotated pages.
fn quad_for_rect(geometry: &DisplayGeometry, rect: TextMarkupRect) -> [f32; 8] {
    let corners = [
        (rect.x, rect.y + rect.height),
        (rect.x + rect.width, rect.y + rect.height),
        (rect.x + rect.width, rect.y),
        (rect.x, rect.y),
    ];
    let mut quad = [0.0f32; 8];
    for (index, (x, y)) in corners.into_iter().enumerate() {
        let (pdf_x, pdf_y) = geometry.point_inverse(x, y);
        quad[index * 2] = pdf_x;
        quad[index * 2 + 1] = pdf_y;
    }
    quad
}

fn bounds_of_quads(quads: &[[f32; 8]]) -> AnnotationRect {
    let mut left = f32::INFINITY;
    let mut bottom = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::NEG_INFINITY;
    for quad in quads {
        for point in quad.chunks_exact(2) {
            left = left.min(point[0]);
            right = right.max(point[0]);
            bottom = bottom.min(point[1]);
            top = top.max(point[1]);
        }
    }
    AnnotationRect::new(bottom, left, top, right)
}

/// Maps a display-space box to a PDF-space rectangle, including rotation and
/// CropBox offset.
fn pdf_rect_for_display(geometry: &DisplayGeometry, rect: TextMarkupRect) -> AnnotationRect {
    let corners = [
        (rect.x, rect.y),
        (rect.x + rect.width, rect.y),
        (rect.x + rect.width, rect.y + rect.height),
        (rect.x, rect.y + rect.height),
    ];
    let mut left = f32::INFINITY;
    let mut bottom = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::NEG_INFINITY;
    for (x, y) in corners {
        let (x, y) = geometry.point_inverse(x, y);
        left = left.min(x);
        right = right.max(x);
        bottom = bottom.min(y);
        top = top.max(y);
    }
    AnnotationRect::new(bottom, left, top, right)
}

fn strokes_bounds(strokes: &[Vec<(f32, f32)>], padding: f32) -> AnnotationRect {
    let mut left = f32::INFINITY;
    let mut bottom = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::NEG_INFINITY;
    for stroke in strokes {
        for &(x, y) in stroke {
            left = left.min(x);
            right = right.max(x);
            bottom = bottom.min(y);
            top = top.max(y);
        }
    }
    AnnotationRect::new(
        bottom - padding,
        left - padding,
        top + padding,
        right + padding,
    )
}

fn display_rects(geometry: &DisplayGeometry, quads: &[[f32; 8]]) -> Vec<TextMarkupRect> {
    quads
        .iter()
        .map(|quad| {
            let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
            let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
            for point in quad.chunks_exact(2) {
                let (x, y) = geometry.point(point[0], point[1]);
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
            TextMarkupRect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            }
        })
        .collect()
}

fn display_rect(geometry: &DisplayGeometry, rect: AnnotationRect) -> TextMarkupRect {
    let (x, y, width, height) = geometry.rect(rect.left, rect.bottom, rect.right, rect.top);
    TextMarkupRect {
        x,
        y,
        width,
        height,
    }
}

fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

fn toward(point: (f32, f32), center: (f32, f32), distance: f32) -> (f32, f32) {
    let (dx, dy) = (center.0 - point.0, center.1 - point.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        point
    } else {
        (
            point.0 + dx / length * distance,
            point.1 + dy / length * distance,
        )
    }
}

fn quad_point(quad: &[f32; 8], index: usize) -> (f32, f32) {
    (quad[index * 2], quad[index * 2 + 1])
}

fn color_components(rgb: (u8, u8, u8)) -> (f32, f32, f32) {
    (
        f32::from(rgb.0) / 255.0,
        f32::from(rgb.1) / 255.0,
        f32::from(rgb.2) / 255.0,
    )
}

/// Builds the normal appearance stream for a markup annotation.
///
/// Pdfium interprets the stream in page coordinates with the annotation's
/// `/Rect` as the Form XObject `/BBox`, so the operators use the same PDF user
/// space as the quads.
fn markup_appearance_stream(kind: TextMarkupKind, rgb: (u8, u8, u8), quads: &[[f32; 8]]) -> String {
    let (red, green, blue) = color_components(rgb);
    let mut stream = String::new();
    // Pdfium adds an /ExtGState named /GS carrying the annotation's constant
    // opacity to the Form XObject resources when the appearance is installed.
    // The stream must select it explicitly; the annotation `/CA` alone is not
    // applied to custom appearance content.
    stream.push_str("q\n/GS gs\n");
    match kind {
        TextMarkupKind::Highlight => {
            stream.push_str(&format!("{red:.4} {green:.4} {blue:.4} rg\n"));
            for quad in quads {
                stream.push_str(&format!(
                    "{:.4} {:.4} m {:.4} {:.4} l {:.4} {:.4} l {:.4} {:.4} l h f\n",
                    quad[0], quad[1], quad[2], quad[3], quad[4], quad[5], quad[6], quad[7]
                ));
            }
            stream.push_str("Q\n");
        }
        TextMarkupKind::Underline => {
            stream.push_str(&format!(
                "{red:.4} {green:.4} {blue:.4} RG {MARKUP_LINE_WIDTH} w\n"
            ));
            for quad in quads {
                let center = midpoint(
                    midpoint(quad_point(quad, 0), quad_point(quad, 2)),
                    midpoint(quad_point(quad, 1), quad_point(quad, 3)),
                );
                let start = toward(quad_point(quad, 0), center, MARKUP_LINE_WIDTH / 2.0);
                let end = toward(quad_point(quad, 1), center, MARKUP_LINE_WIDTH / 2.0);
                stream.push_str(&format!(
                    "{:.4} {:.4} m {:.4} {:.4} l S\n",
                    start.0, start.1, end.0, end.1
                ));
            }
            stream.push_str("Q\n");
        }
        TextMarkupKind::Strikeout => {
            stream.push_str(&format!(
                "{red:.4} {green:.4} {blue:.4} RG {MARKUP_LINE_WIDTH} w\n"
            ));
            for quad in quads {
                let start = midpoint(quad_point(quad, 0), quad_point(quad, 3));
                let end = midpoint(quad_point(quad, 1), quad_point(quad, 2));
                stream.push_str(&format!(
                    "{:.4} {:.4} m {:.4} {:.4} l S\n",
                    start.0, start.1, end.0, end.1
                ));
            }
            stream.push_str("Q\n");
        }
    }
    stream
}

/// Builds the sticky-note icon appearance: a coloured page with a folded
/// corner and a darker border, drawn in page coordinates.
fn note_appearance_stream(rgb: (u8, u8, u8), rect: AnnotationRect) -> String {
    let (red, green, blue) = color_components(rgb);
    let width = rect.right - rect.left;
    let height = rect.top - rect.bottom;
    let fold = width.min(height) * 0.32;
    format!(
        concat!(
            "q\n/GS gs\n",
            "{red:.4} {green:.4} {blue:.4} rg\n",
            "{left:.4} {bottom:.4} {width:.4} {height:.4} re f\n",
            "0.30 0.22 0.04 RG 0.8 w\n",
            "{left:.4} {bottom:.4} {width:.4} {height:.4} re S\n",
            "0.55 0.40 0.06 rg\n",
            "{fold_x:.4} {top:.4} m {right:.4} {top:.4} l ",
            "{right:.4} {fold_y:.4} l h f\n",
            "Q\n"
        ),
        red = red,
        green = green,
        blue = blue,
        left = rect.left,
        bottom = rect.bottom,
        width = width,
        height = height,
        fold_x = rect.right - fold,
        top = rect.top,
        right = rect.right,
        fold_y = rect.top - fold,
    )
}

/// Builds the freehand appearance stream from `/InkList` strokes in PDF user
/// space. Round caps and joins keep a one-point stroke visible as a dot.
fn ink_appearance_stream(
    rgb: (u8, u8, u8),
    line_width: f32,
    strokes: &[Vec<(f32, f32)>],
) -> String {
    let (red, green, blue) = color_components(rgb);
    let mut stream =
        format!("q\n/GS gs\n{red:.4} {green:.4} {blue:.4} RG {line_width:.4} w 1 J 1 j\n");
    for stroke in strokes {
        let Some(&(x, y)) = stroke.first() else {
            continue;
        };
        stream.push_str(&format!("{x:.4} {y:.4} m\n"));
        if stroke.len() == 1 {
            stream.push_str(&format!("{x:.4} {y:.4} l S\n"));
            continue;
        }
        for &(x, y) in &stroke[1..] {
            stream.push_str(&format!("{x:.4} {y:.4} l\n"));
        }
        stream.push_str("S\n");
    }
    stream.push_str("Q\n");
    stream
}

fn page_geometry(
    document: &PdfDocument<'static>,
    page_index: u32,
) -> Result<DisplayGeometry, CommandError> {
    let page = load_page(document, page_index)?;
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    DisplayGeometry::new(&page, rotation)
}

fn find_target(
    document: &PdfDocument<'static>,
    name: &str,
) -> Result<Option<AnnotationRef>, CommandError> {
    let page_count = document.pages().len();
    for page_index in 0..page_count {
        let count = {
            let page = document.pages().get(page_index).map_err(|error| {
                internal_error(format!("could not load page {page_index}: {error:?}"))
            })?;
            page.annotations().len()
        };
        for index in 0..count {
            let target = AnnotationRef::new(page_index, index);
            if annotation_name(document, target)
                .map_err(map_adapter_error)?
                .as_deref()
                == Some(name)
            {
                return Ok(Some(target));
            }
        }
    }
    Ok(None)
}

fn locate(
    document: &PdfDocument<'static>,
    name: &str,
) -> Result<Option<(u32, AnnotationRef, DisplayGeometry)>, CommandError> {
    let Some(target) = find_target(document, name)? else {
        return Ok(None);
    };
    let page_index = target.page_index as u32;
    let geometry = page_geometry(document, page_index)?;
    Ok(Some((page_index, target, geometry)))
}

fn stored_line_width(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
) -> Result<f32, CommandError> {
    let stored = annotation_string_value(document, target, LINE_WIDTH_KEY)
        .map_err(map_adapter_error)?
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|width| width.is_finite() && *width > 0.0);
    Ok(stored
        .or_else(|| {
            annotation_border_width(document, target)
                .ok()
                .filter(|width| width.is_finite() && *width > 0.0)
        })
        .unwrap_or(DEFAULT_INK_LINE_WIDTH))
}

fn capture_snapshot(
    document: &PdfDocument<'static>,
    page_index: u32,
    target: AnnotationRef,
    geometry: &DisplayGeometry,
) -> Result<AnnotationSnapshot, CommandError> {
    let annotation_type = annotation_type(document, target).map_err(map_adapter_error)?;
    let kind = AnnotationKind::from_pdfium(annotation_type).ok_or_else(|| {
        internal_error(format!(
            "annotation {} on page {page_index} is not a supported annotation",
            target.index
        ))
    })?;
    let name = annotation_name(document, target)
        .map_err(map_adapter_error)?
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| internal_error("the annotation has no stable /NM name"))?;
    let author = annotation_author(document, target).map_err(map_adapter_error)?;
    let created_at = annotation_creation_date(document, target).map_err(map_adapter_error)?;
    let rect = annotation_bounds(document, target).map_err(map_adapter_error)?;
    let stored_opacity = annotation_string_value(document, target, OPACITY_KEY)
        .map_err(map_adapter_error)?
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|opacity| *opacity <= 100);

    if let Some(shape_kind) = ShapeKind::from_annotation(kind) {
        return capture_shape_snapshot(
            document,
            page_index,
            target,
            geometry,
            shape_kind,
            name,
            author,
            created_at,
            rect,
            stored_opacity,
        );
    }

    let measured = annotation_color(document, target).ok();
    let color = annotation_string_value(document, target, COLOR_KEY)
        .map_err(map_adapter_error)?
        .unwrap_or_else(|| {
            measured
                .map(|(red, green, blue, _)| format_color(red, green, blue))
                .unwrap_or_else(|| default_color_for(kind).to_owned())
        });
    let opacity = stored_opacity
        .or_else(|| measured.map(|(_, _, _, alpha)| alpha_to_opacity(alpha)))
        .unwrap_or(100);

    let (quads, rects, text, line_width, strokes) = match kind {
        AnnotationKind::Note => (
            Vec::new(),
            vec![display_rect(geometry, rect)],
            Some(
                annotation_contents(document, target)
                    .map_err(map_adapter_error)?
                    .unwrap_or_default(),
            ),
            None,
            Vec::new(),
        ),
        AnnotationKind::Ink => {
            let strokes = annotation_ink_strokes(document, target).map_err(map_adapter_error)?;
            (
                Vec::new(),
                vec![display_rect(geometry, rect)],
                None,
                Some(stored_line_width(document, target)?),
                strokes,
            )
        }
        _ => {
            let quads = annotation_quads(document, target).map_err(map_adapter_error)?;
            let rects = display_rects(geometry, &quads);
            (quads, rects, None, None, Vec::new())
        }
    };

    Ok(AnnotationSnapshot {
        page_index,
        name,
        kind,
        author,
        created_at,
        color,
        opacity,
        line_width,
        text,
        rect,
        quads,
        rects,
        strokes,
        shape: None,
    })
}

fn default_color_for(kind: AnnotationKind) -> &'static str {
    match kind {
        AnnotationKind::Note => DEFAULT_NOTE_COLOR,
        AnnotationKind::Ink => DEFAULT_INK_COLOR,
        _ => DEFAULT_MARKUP_COLOR,
    }
}

/// Captures a shape annotation, preferring the private keys that preserve the
/// exact editable geometry over the rotation-expanded `/Rect`.
#[allow(clippy::too_many_arguments)]
fn capture_shape_snapshot(
    document: &PdfDocument<'static>,
    page_index: u32,
    target: AnnotationRef,
    geometry: &DisplayGeometry,
    kind: ShapeKind,
    name: String,
    author: Option<String>,
    created_at: Option<String>,
    rect: AnnotationRect,
    stored_opacity: Option<u8>,
) -> Result<AnnotationSnapshot, CommandError> {
    let stored_geometry = annotation_string_value(document, target, SHAPE_GEOMETRY_KEY)
        .map_err(map_adapter_error)?
        .and_then(|value| parse_shape_geometry(&value));
    let shape_geometry = stored_geometry.unwrap_or_else(|| {
        let (x, y, width, height) = geometry.rect(rect.left, rect.bottom, rect.right, rect.top);
        ShapeGeometry {
            x,
            y,
            width,
            height,
            rotation: 0.0,
        }
    });
    let fill_color = shape_color(
        document,
        target,
        SHAPE_FILL_KEY,
        annotation_fill_color(document, target).ok(),
    )?;
    let stroke_color = shape_color(
        document,
        target,
        SHAPE_STROKE_KEY,
        annotation_color(document, target).ok(),
    )?;
    let line_width = annotation_string_value(document, target, LINE_WIDTH_KEY)
        .map_err(map_adapter_error)?
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|width| width.is_finite() && *width > 0.0)
        .or_else(|| {
            annotation_border_width(document, target)
                .ok()
                .filter(|width| *width > 0.0)
        })
        .unwrap_or(1.0);
    let opacity = stored_opacity
        .or_else(|| {
            annotation_color(document, target)
                .ok()
                .map(|(_, _, _, alpha)| alpha_to_opacity(alpha))
        })
        .unwrap_or(100);
    let style = ShapeStyle {
        fill_color,
        stroke_color,
        opacity,
        line_width,
    };
    let color = style
        .stroke_color
        .clone()
        .or_else(|| style.fill_color.clone())
        .unwrap_or_else(|| "#000000".to_owned());
    let shape = ShapeView {
        kind,
        geometry: shape_geometry,
        style,
    };
    Ok(AnnotationSnapshot {
        page_index,
        name,
        kind: kind.into(),
        author,
        created_at,
        color,
        opacity,
        line_width: None,
        text: None,
        rect,
        quads: Vec::new(),
        rects: Vec::new(),
        strokes: Vec::new(),
        shape: Some(shape),
    })
}

/// Reads one shape colour from its private key, falling back to PDFium's
/// measured value (which only works before an appearance is installed).
fn shape_color(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
    key: &str,
    measured: Option<(u8, u8, u8, u8)>,
) -> Result<Option<String>, CommandError> {
    match annotation_string_value(document, target, key).map_err(map_adapter_error)? {
        Some(value) if value == NO_COLOR => Ok(None),
        Some(value) => Ok(Some(value)),
        None => Ok(measured.map(|(red, green, blue, _)| format_color(red, green, blue))),
    }
}

/// Writes the standard identity and colour properties every PDFX-created
/// annotation carries. The appearance stream must be installed last: Pdfium
/// fixes the Form XObject `/BBox` from `/Rect` at that moment.
fn write_common_properties(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
    name: &str,
    author: Option<&str>,
    created_at: Option<&str>,
    color: &str,
    opacity: u8,
) -> Result<(u8, u8, u8), CommandError> {
    let rgb = parse_color(color)?;
    set_annotation_name(document, target, name).map_err(map_adapter_error)?;
    if let Some(author) = author {
        set_annotation_author(document, target, author).map_err(map_adapter_error)?;
    }
    if let Some(created_at) = created_at {
        set_annotation_creation_date(document, target, created_at).map_err(map_adapter_error)?;
    }
    set_annotation_string_value(document, target, COLOR_KEY, color).map_err(map_adapter_error)?;
    set_annotation_string_value(document, target, OPACITY_KEY, &opacity.to_string())
        .map_err(map_adapter_error)?;
    set_annotation_color(
        document,
        target,
        rgb.0,
        rgb.1,
        rgb.2,
        opacity_to_alpha(opacity),
    )
    .map_err(map_adapter_error)?;
    Ok(rgb)
}

/// Writes the identity, style and appearance of a shape annotation.
///
/// Order matters: Pdfium drops a custom appearance when the border is set and
/// refuses colour changes once an appearance exists, so the standard entries
/// are written first and the appearance last.
fn write_shape_properties(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
    name: &str,
    author: Option<&str>,
    created_at: Option<&str>,
    shape: &ShapeView,
    appearance: &str,
) -> Result<(), CommandError> {
    set_annotation_name(document, target, name).map_err(map_adapter_error)?;
    if let Some(author) = author {
        set_annotation_author(document, target, author).map_err(map_adapter_error)?;
    }
    if let Some(created_at) = created_at {
        set_annotation_creation_date(document, target, created_at).map_err(map_adapter_error)?;
    }
    set_annotation_string_value(
        document,
        target,
        SHAPE_GEOMETRY_KEY,
        &format_shape_geometry(&shape.geometry),
    )
    .map_err(map_adapter_error)?;
    set_annotation_string_value(
        document,
        target,
        SHAPE_FILL_KEY,
        shape.style.fill_color.as_deref().unwrap_or(NO_COLOR),
    )
    .map_err(map_adapter_error)?;
    set_annotation_string_value(
        document,
        target,
        SHAPE_STROKE_KEY,
        shape.style.stroke_color.as_deref().unwrap_or(NO_COLOR),
    )
    .map_err(map_adapter_error)?;
    set_annotation_string_value(
        document,
        target,
        LINE_WIDTH_KEY,
        &format!("{:.4}", shape.style.line_width),
    )
    .map_err(map_adapter_error)?;
    set_annotation_string_value(
        document,
        target,
        OPACITY_KEY,
        &shape.style.opacity.to_string(),
    )
    .map_err(map_adapter_error)?;
    set_annotation_string_value(
        document,
        target,
        COLOR_KEY,
        shape
            .style
            .stroke_color
            .as_deref()
            .or(shape.style.fill_color.as_deref())
            .unwrap_or(NO_COLOR),
    )
    .map_err(map_adapter_error)?;

    let alpha = opacity_to_alpha(shape.style.opacity);
    if let Some(color) = shape.style.fill_color.as_deref() {
        let (red, green, blue) = parse_color(color)?;
        set_annotation_fill_color(document, target, red, green, blue, alpha)
            .map_err(map_adapter_error)?;
    }
    if let Some(color) = shape.style.stroke_color.as_deref() {
        let (red, green, blue) = parse_color(color)?;
        set_annotation_color(document, target, red, green, blue, alpha)
            .map_err(map_adapter_error)?;
    }
    let border = if shape.style.stroke_color.is_some() {
        shape.style.line_width
    } else {
        0.0
    };
    set_annotation_border_width(document, target, border).map_err(map_adapter_error)?;
    set_annotation_appearance(document, target, appearance).map_err(map_adapter_error)
}

/// Recreates the annotation described by `snapshot`, first removing any
/// annotation with the same name. Recreating rather than mutating is deliberate:
/// Pdfium cannot change `/C`/`/CA` or colours once an appearance stream exists,
/// moving a note must rebuild the Form XObject `/BBox` from its new `/Rect`,
/// and the generated shape appearance depends on all of them.
pub(crate) fn restore_annotation_snapshot(
    document: &PdfDocument<'static>,
    snapshot: &AnnotationSnapshot,
) -> Result<(), CommandError> {
    if let Some(existing) = find_target(document, &snapshot.name)? {
        remove_annotation(document, existing).map_err(map_adapter_error)?;
    }
    if let Some(shape) = snapshot.shape.as_ref() {
        let page = page_geometry(document, snapshot.page_index)?;
        let appearance = shape_appearance(&page, shape.kind, &shape.geometry, &shape.style)?;
        let target = create_shape_native(
            document,
            snapshot.page_index as i32,
            shape.kind.into(),
            appearance.bounds,
        )
        .map_err(map_adapter_error)?;
        return write_shape_properties(
            document,
            target,
            &snapshot.name,
            snapshot.author.as_deref(),
            snapshot.created_at.as_deref(),
            shape,
            &appearance.appearance,
        );
    }

    match snapshot.kind {
        AnnotationKind::Note => {
            let text = snapshot.text.as_deref().unwrap_or_default();
            let target = create_native_text_annotation(
                document,
                snapshot.page_index as i32,
                text,
                snapshot.rect,
            )
            .map_err(map_adapter_error)?;
            let rgb = write_common_properties(
                document,
                target,
                &snapshot.name,
                snapshot.author.as_deref(),
                snapshot.created_at.as_deref(),
                &snapshot.color,
                snapshot.opacity,
            )?;
            set_annotation_contents(document, target, text).map_err(map_adapter_error)?;
            set_annotation_appearance(
                document,
                target,
                &note_appearance_stream(rgb, snapshot.rect),
            )
            .map_err(map_adapter_error)?;
        }
        AnnotationKind::Ink => {
            let line_width = snapshot.line_width.unwrap_or(DEFAULT_INK_LINE_WIDTH);
            let target =
                create_native_ink_annotation(document, snapshot.page_index as i32, snapshot.rect)
                    .map_err(map_adapter_error)?;
            let rgb = write_common_properties(
                document,
                target,
                &snapshot.name,
                snapshot.author.as_deref(),
                snapshot.created_at.as_deref(),
                &snapshot.color,
                snapshot.opacity,
            )?;
            set_annotation_string_value(document, target, LINE_WIDTH_KEY, &line_width.to_string())
                .map_err(map_adapter_error)?;
            set_annotation_border_width(document, target, line_width).map_err(map_adapter_error)?;
            append_ink_strokes(document, target, &snapshot.strokes).map_err(map_adapter_error)?;
            set_annotation_appearance(
                document,
                target,
                &ink_appearance_stream(rgb, line_width, &snapshot.strokes),
            )
            .map_err(map_adapter_error)?;
        }
        _ => {
            let markup_kind = snapshot.kind.text_markup().ok_or_else(|| {
                internal_error(format!(
                    "annotation {} has neither markup, shape, note nor ink data",
                    snapshot.name
                ))
            })?;
            let target = create_markup_annotation(
                document,
                snapshot.page_index as i32,
                markup_kind.into(),
                snapshot.rect,
            )
            .map_err(map_adapter_error)?;
            let rgb = write_common_properties(
                document,
                target,
                &snapshot.name,
                snapshot.author.as_deref(),
                snapshot.created_at.as_deref(),
                &snapshot.color,
                snapshot.opacity,
            )?;
            append_annotation_quads(document, target, &snapshot.quads)
                .map_err(map_adapter_error)?;
            set_annotation_appearance(
                document,
                target,
                &markup_appearance_stream(markup_kind, rgb, &snapshot.quads),
            )
            .map_err(map_adapter_error)?;
        }
    }
    Ok(())
}

/// Removes the annotation with `name`. Used by undo/redo for actions with no
/// target snapshot (undoing a creation, redoing a deletion).
pub(crate) fn remove_annotation_named(
    document: &PdfDocument<'static>,
    name: &str,
) -> Result<(), CommandError> {
    let target = find_target(document, name)?
        .ok_or_else(|| internal_error(format!("annotation {name} does not exist")))?;
    remove_annotation(document, target).map_err(map_adapter_error)
}

/// Creates a text markup annotation attributed to `author`.
///
/// The command layer passes the author from settings; tests call this directly
/// so author provenance is covered without an `AppHandle`.
pub fn create_text_markup_with_author(
    document_id: String,
    author: String,
    request: CreateTextMarkupRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let author = normalize_author(&author)?;
    let rgb = validate_request(&request)?;
    let color = format_color(rgb.0, rgb.1, rgb.2);
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let geometry = page_geometry(&entry.document, request.page_index)?;
        let quads: Vec<[f32; 8]> = request
            .rects
            .iter()
            .map(|rect| quad_for_rect(&geometry, *rect))
            .collect();
        let bounds = bounds_of_quads(&quads);
        let name = next_annotation_name();
        let created_at = pdf_date_now();

        let target = create_markup_annotation(
            &entry.document,
            request.page_index as i32,
            request.kind.into(),
            bounds,
        )
        .map_err(map_adapter_error)?;
        let written = write_common_properties(
            &entry.document,
            target,
            &name,
            Some(&author),
            Some(&created_at),
            &color,
            request.opacity,
        )?;
        append_annotation_quads(&entry.document, target, &quads).map_err(map_adapter_error)?;
        set_annotation_appearance(
            &entry.document,
            target,
            &markup_appearance_stream(request.kind, written, &quads),
        )
        .map_err(map_adapter_error)?;

        let snapshot = capture_snapshot(&entry.document, request.page_index, target, &geometry)?;
        let view = snapshot.view(&geometry);
        let history = push_annotation_action(entry, request.page_index, name, None, Some(snapshot));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index: request.page_index,
            history,
        })
    })
}

/// Creates an annotation using the author from persisted settings.
pub fn create_text_markup_from_settings(
    document_id: String,
    settings: &Settings,
    request: CreateTextMarkupRequest,
) -> Result<AnnotationMutationView, CommandError> {
    create_text_markup_with_author(document_id, settings.annotation_author.clone(), request)
}

/// Creates a sticky note attributed to `author`.
pub fn create_note_with_author(
    document_id: String,
    author: String,
    request: CreateNoteRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let author = normalize_author(&author)?;
    let rgb = validate_color_opacity(&request.color, request.opacity)?;
    let color = format_color(rgb.0, rgb.1, rgb.2);
    validate_rect(request.rect, "a sticky note")?;
    let text = normalize_note_text(&request.text)?;
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let geometry = page_geometry(&entry.document, request.page_index)?;
        let bounds = pdf_rect_for_display(&geometry, request.rect);
        let name = next_annotation_name();
        let created_at = pdf_date_now();

        let target = create_native_text_annotation(
            &entry.document,
            request.page_index as i32,
            &text,
            bounds,
        )
        .map_err(map_adapter_error)?;
        let written = write_common_properties(
            &entry.document,
            target,
            &name,
            Some(&author),
            Some(&created_at),
            &color,
            request.opacity,
        )?;
        set_annotation_contents(&entry.document, target, &text).map_err(map_adapter_error)?;
        set_annotation_appearance(
            &entry.document,
            target,
            &note_appearance_stream(written, bounds),
        )
        .map_err(map_adapter_error)?;

        let snapshot = capture_snapshot(&entry.document, request.page_index, target, &geometry)?;
        let view = snapshot.view(&geometry);
        let history = push_annotation_action(entry, request.page_index, name, None, Some(snapshot));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index: request.page_index,
            history,
        })
    })
}

/// Creates a sticky note using the author from persisted settings.
pub fn create_note_from_settings(
    document_id: String,
    settings: &Settings,
    request: CreateNoteRequest,
) -> Result<AnnotationMutationView, CommandError> {
    create_note_with_author(document_id, settings.annotation_author.clone(), request)
}

/// Creates a freehand ink annotation attributed to `author`.
pub fn create_ink_with_author(
    document_id: String,
    author: String,
    request: CreateInkRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let author = normalize_author(&author)?;
    let rgb = validate_ink_request(&request)?;
    let color = format_color(rgb.0, rgb.1, rgb.2);
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let geometry = page_geometry(&entry.document, request.page_index)?;
        let strokes: Vec<Vec<(f32, f32)>> = request
            .strokes
            .iter()
            .map(|stroke| {
                stroke
                    .iter()
                    .map(|point| geometry.point_inverse(point.x, point.y))
                    .collect()
            })
            .collect();
        let bounds = strokes_bounds(&strokes, request.line_width / 2.0 + 1.0);
        let name = next_annotation_name();
        let created_at = pdf_date_now();

        let target =
            create_native_ink_annotation(&entry.document, request.page_index as i32, bounds)
                .map_err(map_adapter_error)?;
        let written = write_common_properties(
            &entry.document,
            target,
            &name,
            Some(&author),
            Some(&created_at),
            &color,
            request.opacity,
        )?;
        set_annotation_string_value(
            &entry.document,
            target,
            LINE_WIDTH_KEY,
            &request.line_width.to_string(),
        )
        .map_err(map_adapter_error)?;
        set_annotation_border_width(&entry.document, target, request.line_width)
            .map_err(map_adapter_error)?;
        append_ink_strokes(&entry.document, target, &strokes).map_err(map_adapter_error)?;
        set_annotation_appearance(
            &entry.document,
            target,
            &ink_appearance_stream(written, request.line_width, &strokes),
        )
        .map_err(map_adapter_error)?;

        let snapshot = capture_snapshot(&entry.document, request.page_index, target, &geometry)?;
        let view = snapshot.view(&geometry);
        let history = push_annotation_action(entry, request.page_index, name, None, Some(snapshot));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index: request.page_index,
            history,
        })
    })
}

/// Creates ink using the author from persisted settings.
pub fn create_ink_from_settings(
    document_id: String,
    settings: &Settings,
    request: CreateInkRequest,
) -> Result<AnnotationMutationView, CommandError> {
    create_ink_with_author(document_id, settings.annotation_author.clone(), request)
}

/// The `create_text_markup_annotation` command. The author is read from
/// settings at creation time and never accepted from the frontend.
#[tauri::command]
pub fn create_text_markup_annotation(
    app: AppHandle,
    document_id: String,
    request: CreateTextMarkupRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let settings = crate::commands::settings_store(&app)?.load();
    create_text_markup_from_settings(document_id, &settings, request)
}

/// The `create_note_annotation` command. The author is read from settings.
#[tauri::command]
pub fn create_note_annotation(
    app: AppHandle,
    document_id: String,
    request: CreateNoteRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let settings = crate::commands::settings_store(&app)?.load();
    create_note_from_settings(document_id, &settings, request)
}

/// The `create_ink_annotation` command. The author is read from settings.
#[tauri::command]
pub fn create_ink_annotation(
    app: AppHandle,
    document_id: String,
    request: CreateInkRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let settings = crate::commands::settings_store(&app)?.load();
    create_ink_from_settings(document_id, &settings, request)
}

/// Creates a rectangle or ellipse attributed to `author`.
///
/// The command layer passes the author from settings; tests call this directly
/// so author provenance is covered without an `AppHandle`.
pub fn create_shape_with_author(
    document_id: String,
    author: String,
    request: CreateShapeRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let author = normalize_author(&author)?;
    let shape_geometry = normalize_shape_geometry(request.geometry)?;
    let style = normalize_shape_style(request.style)?;
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let page = page_geometry(&entry.document, request.page_index)?;
        let appearance = shape_appearance(&page, request.kind, &shape_geometry, &style)?;
        let name = next_annotation_name();
        let created_at = pdf_date_now();

        let target = create_shape_native(
            &entry.document,
            request.page_index as i32,
            request.kind.into(),
            appearance.bounds,
        )
        .map_err(map_adapter_error)?;
        let shape = ShapeView {
            kind: request.kind,
            geometry: shape_geometry,
            style,
        };
        write_shape_properties(
            &entry.document,
            target,
            &name,
            Some(&author),
            Some(&created_at),
            &shape,
            &appearance.appearance,
        )?;

        let snapshot = capture_snapshot(&entry.document, request.page_index, target, &page)?;
        let view = snapshot.view(&page);
        let history = push_annotation_action(entry, request.page_index, name, None, Some(snapshot));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index: request.page_index,
            history,
        })
    })
}

/// Creates a shape using the author from persisted settings.
pub fn create_shape_from_settings(
    document_id: String,
    settings: &Settings,
    request: CreateShapeRequest,
) -> Result<AnnotationMutationView, CommandError> {
    create_shape_with_author(document_id, settings.annotation_author.clone(), request)
}

/// The `create_shape_annotation` command. The author is read from settings at
/// creation time and never accepted from the frontend.
#[tauri::command]
pub fn create_shape_annotation(
    app: AppHandle,
    document_id: String,
    request: CreateShapeRequest,
) -> Result<AnnotationMutationView, CommandError> {
    let settings = crate::commands::settings_store(&app)?.load();
    create_shape_from_settings(document_id, &settings, request)
}

/// Applies a new geometry or style to an existing shape and records one undo
/// step. Shared by the canvas transform and property-panel commands.
fn update_shape(
    document_id: String,
    name: String,
    apply: impl FnOnce(&mut ShapeView) -> Result<(), CommandError>,
) -> Result<AnnotationMutationView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        let mut after = before.clone();
        let Some(shape) = after.shape.as_mut() else {
            return Err(invalid_argument(format!(
                "annotation {name} is not a shape"
            )));
        };
        apply(shape)?;
        shape.geometry = normalize_shape_geometry(shape.geometry)?;
        shape.style = normalize_shape_style(shape.style.clone())?;
        restore_annotation_snapshot(&entry.document, &after)?;
        let view = after.view(&geometry);
        let history = push_annotation_action(entry, page_index, name, Some(before), Some(after));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index,
            history,
        })
    })
}

/// Moves, resizes or rotates a shape as one undoable action.
#[tauri::command]
pub fn update_shape_geometry(
    document_id: String,
    name: String,
    geometry: ShapeGeometry,
) -> Result<AnnotationMutationView, CommandError> {
    update_shape(document_id, name, move |shape| {
        shape.geometry = geometry;
        Ok(())
    })
}

/// Changes a shape's fill, stroke, opacity or line width as one undoable
/// action. The geometry is untouched.
#[tauri::command]
pub fn update_shape_style(
    document_id: String,
    name: String,
    style: ShapeStyle,
) -> Result<AnnotationMutationView, CommandError> {
    update_shape(document_id, name, move |shape| {
        shape.style = style;
        Ok(())
    })
}

/// Lists every named annotation PDFX manages, grouped by page order.
#[tauri::command]
pub fn list_annotations(document_id: String) -> Result<Vec<AnnotationView>, CommandError> {
    with_open_document(&document_id, |entry| {
        let document = &entry.document;
        let page_count = document.pages().len();
        let mut views = Vec::new();
        for page_index in 0..page_count {
            let count = {
                let page = document.pages().get(page_index).map_err(|error| {
                    internal_error(format!("could not load page {page_index}: {error:?}"))
                })?;
                page.annotations().len()
            };
            if count == 0 {
                continue;
            }
            let geometry = page_geometry(document, page_index as u32)?;
            for index in 0..count {
                let target = AnnotationRef::new(page_index, index);
                let Ok(annotation_type) = annotation_type(document, target) else {
                    continue;
                };
                if AnnotationKind::from_pdfium(annotation_type).is_none() {
                    continue;
                }
                let named = annotation_name(document, target)
                    .ok()
                    .flatten()
                    .is_some_and(|name| !name.trim().is_empty());
                if !named {
                    continue;
                }
                let snapshot = capture_snapshot(document, page_index as u32, target, &geometry)?;
                views.push(snapshot.view(&geometry));
            }
        }
        Ok(views)
    })
}

/// Changes the colour, opacity and (for ink) line width of an annotation.
/// Creates a new undo step; the annotation keeps its name, author, creation
/// time, text and strokes.
#[tauri::command]
pub fn update_annotation_style(
    document_id: String,
    name: String,
    color: String,
    opacity: u8,
    line_width: Option<f32>,
) -> Result<AnnotationMutationView, CommandError> {
    validate_color_opacity(&color, opacity)?;
    if let Some(line_width) = line_width {
        validate_ink_line_width(line_width)?;
    }
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        if before.shape.is_some() {
            return Err(invalid_argument(format!(
                "annotation {name} is a shape; use update_shape_style"
            )));
        }
        if line_width.is_some() && before.kind != AnnotationKind::Ink {
            return Err(invalid_argument("only ink annotations have a line width"));
        }
        let mut after = before.clone();
        after.color = color;
        after.opacity = opacity;
        if let Some(line_width) = line_width {
            after.line_width = Some(line_width);
        }
        restore_annotation_snapshot(&entry.document, &after)?;
        let after = locate(&entry.document, &name)?
            .and_then(|(_, target, geometry)| {
                capture_snapshot(&entry.document, page_index, target, &geometry).ok()
            })
            .unwrap_or(after);
        let view = after.view(&geometry);
        let history = push_annotation_action(entry, page_index, name, Some(before), Some(after));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index,
            history,
        })
    })
}

/// Replaces a sticky note's body. The annotation keeps its name, author,
/// creation time, colour and position.
#[tauri::command]
pub fn update_note_text(
    document_id: String,
    name: String,
    text: String,
) -> Result<AnnotationMutationView, CommandError> {
    let text = normalize_note_text(&text)?;
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        if before.kind != AnnotationKind::Note {
            return Err(invalid_argument("only sticky notes have editable text"));
        }
        let mut after = before.clone();
        after.text = Some(text);
        restore_annotation_snapshot(&entry.document, &after)?;
        let after = locate(&entry.document, &name)?
            .and_then(|(_, target, geometry)| {
                capture_snapshot(&entry.document, page_index, target, &geometry).ok()
            })
            .unwrap_or(after);
        let view = after.view(&geometry);
        let history = push_annotation_action(entry, page_index, name, Some(before), Some(after));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index,
            history,
        })
    })
}

/// Moves and resizes a sticky note as one undoable action. The appearance is
/// rebuilt so the icon keeps filling the new `/Rect`.
#[tauri::command]
pub fn transform_note_annotation(
    document_id: String,
    name: String,
    rect: TextMarkupRect,
) -> Result<AnnotationMutationView, CommandError> {
    validate_rect(rect, "a sticky note")?;
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        if before.kind != AnnotationKind::Note {
            return Err(invalid_argument(
                "only sticky notes can be moved or resized",
            ));
        }
        let mut after = before.clone();
        after.rect = pdf_rect_for_display(&geometry, rect);
        after.rects = vec![rect];
        restore_annotation_snapshot(&entry.document, &after)?;
        let after = locate(&entry.document, &name)?
            .and_then(|(_, target, geometry)| {
                capture_snapshot(&entry.document, page_index, target, &geometry).ok()
            })
            .unwrap_or(after);
        let view = after.view(&geometry);
        let history = push_annotation_action(entry, page_index, name, Some(before), Some(after));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index,
            history,
        })
    })
}

/// Deletes an annotation. The undo step recreates it, including its original
/// author, creation time, note text, ink strokes and shape style.
#[tauri::command]
pub fn delete_annotation(
    document_id: String,
    name: String,
) -> Result<AnnotationMutationView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        remove_annotation(&entry.document, target).map_err(map_adapter_error)?;
        let history = push_annotation_action(entry, page_index, name, Some(before), None);
        Ok(AnnotationMutationView {
            annotation: None,
            page_index,
            history,
        })
    })
}
