//! Text markup annotations: highlight, underline and strikeout.
//!
//! Annotations are native PDFium annotations. Every created annotation carries
//! its author (`/T`), creation time (`/CreationDate`), colour (`/C`), constant
//! opacity (`/CA`), quadrilateral geometry (`/QuadPoints`) and a custom normal
//! appearance stream (`/AP`). Pdfium does not synthesize appearances, so the
//! stream is generated here; without it the saved file would show nothing in
//! other readers.
//!
//! Annotations are identified by their `/NM` name. The name is stable across
//! edits and undo/redo, which makes it safe to look an annotation up again
//! after another annotation was deleted. Pre-existing unnamed markup is
//! rendered by PDFium but intentionally not listed: it has no stable identity
//! and no author/creation metadata to manage.
//!
//! The colour and opacity are also mirrored into private string keys
//! (`PDFXColor`, `PDFXOpacity`). Pdfium refuses to read `/C` and `/CA` back once
//! an appearance stream exists; the private keys keep the annotation list
//! correct without touching the standard, reader-visible entries.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pdfium_render::prelude::{PdfDocument, PdfPageAnnotationType};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry,
};
use crate::pdfium_adapter::{
    annotation_author, annotation_bounds, annotation_color, annotation_creation_date,
    annotation_name, annotation_quads, annotation_string_value, annotation_type,
    append_annotation_quads, create_text_markup_annotation as create_markup_annotation,
    remove_annotation, set_annotation_appearance, set_annotation_author, set_annotation_color,
    set_annotation_creation_date, set_annotation_name, set_annotation_string_value, AnnotationRect,
    AnnotationRef, TextMarkupAnnotationKind,
};
use crate::settings::Settings;
use crate::text_edit::{push_annotation_action, EditHistoryView};

/// Private annotation keys mirroring `/C` and `/CA` for reliable read-back.
const COLOR_KEY: &str = "PDFXColor";
const OPACITY_KEY: &str = "PDFXOpacity";
const DEFAULT_MARKUP_COLOR: &str = "#ffd400";
/// Underline and strikeout stroke width in page points.
const MARKUP_LINE_WIDTH: f32 = 1.5;
const MAX_AUTHOR_CHARS: usize = 64;

static ANNOTATION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The three text markup annotations PDFX can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextMarkupKind {
    Highlight,
    Underline,
    Strikeout,
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

impl TextMarkupKind {
    fn from_pdfium(kind: PdfPageAnnotationType) -> Option<Self> {
        match kind {
            PdfPageAnnotationType::Highlight => Some(Self::Highlight),
            PdfPageAnnotationType::Underline => Some(Self::Underline),
            PdfPageAnnotationType::Strikeout => Some(Self::Strikeout),
            _ => None,
        }
    }
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

/// One annotation as the annotation list and overlay consume it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationView {
    pub name: String,
    pub page_index: u32,
    pub kind: TextMarkupKind,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub color: String,
    pub opacity: u8,
    /// Display-space boxes, one per quadrilateral.
    pub rects: Vec<TextMarkupRect>,
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
    pub(crate) kind: TextMarkupKind,
    pub(crate) author: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) color: String,
    pub(crate) opacity: u8,
    /// Bounds in PDF user space.
    pub(crate) rect: AnnotationRect,
    /// Quadrilaterals in PDF user space, in the order PDFX creates them.
    pub(crate) quads: Vec<[f32; 8]>,
    /// Display-space boxes derived from `quads`, kept for the list/overlay.
    pub(crate) rects: Vec<TextMarkupRect>,
}

impl AnnotationSnapshot {
    fn view(&self) -> AnnotationView {
        AnnotationView {
            name: self.name.clone(),
            page_index: self.page_index,
            kind: self.kind,
            author: self.author.clone(),
            created_at: self.created_at.clone(),
            color: self.color.clone(),
            opacity: self.opacity,
            rects: self.rects.clone(),
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

fn validate_request(request: &CreateTextMarkupRequest) -> Result<(u8, u8, u8), CommandError> {
    let rgb = parse_color(&request.color)?;
    if request.opacity > 100 {
        return Err(invalid_argument(
            "the annotation opacity must be between 0 and 100",
        ));
    }
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

/// Builds the normal appearance stream for an annotation.
///
/// Pdfium interprets the stream in page coordinates with the annotation's
/// `/Rect` as the Form XObject `/BBox`, so the operators use the same PDF user
/// space as the quads.
fn appearance_stream(kind: TextMarkupKind, rgb: (u8, u8, u8), quads: &[[f32; 8]]) -> String {
    let red = f32::from(rgb.0) / 255.0;
    let green = f32::from(rgb.1) / 255.0;
    let blue = f32::from(rgb.2) / 255.0;
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

fn capture_snapshot(
    document: &PdfDocument<'static>,
    page_index: u32,
    target: AnnotationRef,
    geometry: &DisplayGeometry,
) -> Result<AnnotationSnapshot, CommandError> {
    let annotation_type = annotation_type(document, target).map_err(map_adapter_error)?;
    let kind = TextMarkupKind::from_pdfium(annotation_type).ok_or_else(|| {
        internal_error(format!(
            "annotation {} on page {page_index} is not a text markup annotation",
            target.index
        ))
    })?;
    let name = annotation_name(document, target)
        .map_err(map_adapter_error)?
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| internal_error("the annotation has no stable /NM name"))?;
    let author = annotation_author(document, target).map_err(map_adapter_error)?;
    let created_at = annotation_creation_date(document, target).map_err(map_adapter_error)?;
    let measured = annotation_color(document, target).ok();
    let color = annotation_string_value(document, target, COLOR_KEY)
        .map_err(map_adapter_error)?
        .unwrap_or_else(|| {
            measured
                .map(|(red, green, blue, _)| format_color(red, green, blue))
                .unwrap_or_else(|| DEFAULT_MARKUP_COLOR.to_owned())
        });
    let opacity = annotation_string_value(document, target, OPACITY_KEY)
        .map_err(map_adapter_error)?
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|opacity| *opacity <= 100)
        .or_else(|| measured.map(|(_, _, _, alpha)| alpha_to_opacity(alpha)))
        .unwrap_or(100);
    let quads = annotation_quads(document, target).map_err(map_adapter_error)?;
    let rect = annotation_bounds(document, target).map_err(map_adapter_error)?;
    let rects = display_rects(geometry, &quads);
    Ok(AnnotationSnapshot {
        page_index,
        name,
        kind,
        author,
        created_at,
        color,
        opacity,
        rect,
        quads,
        rects,
    })
}

/// Identity and style written onto a newly created (or restored) annotation.
struct MarkupProperties<'a> {
    name: &'a str,
    author: Option<&'a str>,
    created_at: Option<&'a str>,
    color: &'a str,
    opacity: u8,
    kind: TextMarkupKind,
}

/// Writes the standard properties every PDFX-created markup annotation carries.
/// The appearance stream must be installed last: Pdfium fixes the Form XObject
/// `/BBox` from `/Rect` at that moment.
fn write_annotation_properties(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
    properties: MarkupProperties<'_>,
    quads: &[[f32; 8]],
) -> Result<(), CommandError> {
    let MarkupProperties {
        name,
        author,
        created_at,
        color,
        opacity,
        kind,
    } = properties;
    let rgb = parse_color(color)?;
    set_annotation_name(document, target, name).map_err(map_adapter_error)?;
    if let Some(author) = author {
        set_annotation_author(document, target, author).map_err(map_adapter_error)?;
    }
    if let Some(created_at) = created_at {
        set_annotation_creation_date(document, target, created_at).map_err(map_adapter_error)?;
    }
    append_annotation_quads(document, target, quads).map_err(map_adapter_error)?;
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
    set_annotation_appearance(document, target, &appearance_stream(kind, rgb, quads))
        .map_err(map_adapter_error)?;
    Ok(())
}

/// Recreates the annotation described by `snapshot`, first removing any
/// annotation with the same name. Recreating rather than mutating is deliberate:
/// Pdfium cannot change `/C`/`/CA` once an appearance stream exists, and the
/// generated appearance depends on both.
pub(crate) fn restore_annotation_snapshot(
    document: &PdfDocument<'static>,
    snapshot: &AnnotationSnapshot,
) -> Result<(), CommandError> {
    if let Some(existing) = find_target(document, &snapshot.name)? {
        remove_annotation(document, existing).map_err(map_adapter_error)?;
    }
    let target = create_markup_annotation(
        document,
        snapshot.page_index as i32,
        snapshot.kind.into(),
        snapshot.rect,
    )
    .map_err(map_adapter_error)?;
    write_annotation_properties(
        document,
        target,
        MarkupProperties {
            name: &snapshot.name,
            author: snapshot.author.as_deref(),
            created_at: snapshot.created_at.as_deref(),
            color: &snapshot.color,
            opacity: snapshot.opacity,
            kind: snapshot.kind,
        },
        &snapshot.quads,
    )
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
        write_annotation_properties(
            &entry.document,
            target,
            MarkupProperties {
                name: &name,
                author: Some(&author),
                created_at: Some(&created_at),
                color: &color,
                opacity: request.opacity,
                kind: request.kind,
            },
            &quads,
        )?;

        let snapshot = capture_snapshot(&entry.document, request.page_index, target, &geometry)?;
        let view = snapshot.view();
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

/// Lists every named text markup annotation, grouped by page order.
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
                if TextMarkupKind::from_pdfium(annotation_type).is_none() {
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
                views.push(snapshot.view());
            }
        }
        Ok(views)
    })
}

/// Changes the colour and opacity of an existing annotation. Creates a new
/// undo step; the annotation keeps its name, author and creation time.
#[tauri::command]
pub fn update_annotation_style(
    document_id: String,
    name: String,
    color: String,
    opacity: u8,
) -> Result<AnnotationMutationView, CommandError> {
    let rgb = parse_color(&color)?;
    if opacity > 100 {
        return Err(invalid_argument(
            "the annotation opacity must be between 0 and 100",
        ));
    }
    let color = format_color(rgb.0, rgb.1, rgb.2);
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let Some((page_index, target, geometry)) = locate(&entry.document, &name)? else {
            return Err(invalid_argument(format!(
                "no annotation named {name} exists in this document"
            )));
        };
        let before = capture_snapshot(&entry.document, page_index, target, &geometry)?;
        let mut after = before.clone();
        after.color = color;
        after.opacity = opacity;
        restore_annotation_snapshot(&entry.document, &after)?;
        let view = after.view();
        let history = push_annotation_action(entry, page_index, name, Some(before), Some(after));
        Ok(AnnotationMutationView {
            annotation: Some(view),
            page_index,
            history,
        })
    })
}

/// Deletes an annotation. The undo step recreates it, including its original
/// author and creation time.
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
