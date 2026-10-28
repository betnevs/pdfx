//! Drawing straight lines and arrows as page content objects.
//!
//! A line or arrow is a PDF *page path object* written into the page content
//! stream, not a Native Annotation. It therefore has **no annotation thread,
//! no annotation selection and no standard comment delete semantics**: it never
//! appears in [`crate::annotations::list_annotations`] or the annotation
//! panel, and deleting it is a content edit that goes through the unified
//! undo/redo history.
//!
//! Because the objects are page content, they travel through the ticket-10
//! save pipeline exactly like Text Boxes and images: PDFium's incremental save
//! writes them as one revision and the Source Document is never touched. The
//! page-object snapshots kept by [`crate::text_edit::EditSession`] make every
//! create, transform, restyle and delete action undoable and redoable,
//! including after a save.
//!
//! # Geometry
//!
//! Endpoints are stored in display-space points (top-left origin, CropBox and
//! intrinsic page rotation already applied), matching the overlay. At draw
//! time they are mapped into the document's own user space through
//! [`DisplayGeometry::point_inverse`]. Rotation is real geometry: rotating a
//! line rotates both endpoints, so the saved path (not only the overlay chrome)
//! follows the new angle.
//!
//! # Permissions
//!
//! Lines are content, so every mutation requires content-editing permission
//! ([`crate::text_edit::ensure_content_editable`]), not annotation permission.

use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfPageObject, PdfPageObjectCommon, PdfPageObjectLineCap,
    PdfPageObjectsCommon, PdfPagePathObject, PdfPoints,
};
use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::text_edit::{
    detach_and_insert, ensure_content_editable, shift_object_indexes, EditAction, EditHistoryView,
    EditSession, EditStepView, LineEditAction, ObjectRef,
};

/// Upper bound for a line stroke width, in page points.
const MAX_LINE_WIDTH: f32 = 100.0;

/// Shortest drawable line, in display-space points. A click or a sliver leaves
/// the document untouched instead of creating an invisible object.
const MIN_LINE_LENGTH: f32 = 2.0;

/// Guard against nonsensical coordinates while staying far above any real page.
const MAX_COORDINATE: f32 = 100_000.0;

/// Arrow-head length is proportional to the stroke width, clamped so heads are
/// legible on hairlines and do not dwarf thin pages. The overlay uses the same
/// formula so the preview matches the saved path.
const ARROW_HEAD_MIN_LENGTH: f32 = 8.0;
const ARROW_HEAD_MAX_LENGTH: f32 = 24.0;
const ARROW_HEAD_LENGTH_PER_WIDTH: f32 = 4.0;
/// Half-width of the arrow head as a fraction of its length.
const ARROW_HEAD_HALF_WIDTH_RATIO: f32 = 0.35;

/// The two linear object kinds PDFX creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// A single stroked segment.
    Line,
    /// A stroked segment with a filled arrow head at the second endpoint.
    Arrow,
}

/// Editable endpoints in display-space points (top-left origin).
///
/// `(x1, y1)` is the start and `(x2, y2)` the end; an arrow head sits at the
/// end. Rotation is expressed by the endpoint positions themselves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineGeometry {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

/// Stroke colour, opacity and width of a linear object.
///
/// Lines always have a stroke, so `stroke_color` is required. Opacity is a
/// percentage (0-100); it is applied to the path's stroke (and to the arrow
/// head's fill) through the PDF colour alpha channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineStyle {
    pub stroke_color: String,
    pub opacity: u8,
    pub line_width: f32,
}

/// The frontend's request to draw a line or arrow on one Page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLineRequest {
    pub page_index: u32,
    pub kind: LineKind,
    pub geometry: LineGeometry,
    pub style: LineStyle,
}

/// New endpoints for an existing line, in display-space points.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineTransformRequest {
    pub geometry: LineGeometry,
}

/// A line or arrow as the overlay and property panel see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineObjectView {
    pub line_id: u64,
    pub page_index: u32,
    pub kind: LineKind,
    pub geometry: LineGeometry,
    pub style: LineStyle,
}

/// The outcome of creating or changing one linear object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineEditResultView {
    pub page_index: u32,
    pub line: LineObjectView,
    pub history: EditHistoryView,
}

/// One line or arrow this session created on a page.
pub(crate) struct LineRecord {
    pub(crate) line_id: u64,
    /// Index of the line's page object.
    pub(crate) start: usize,
    /// 1 while the object is on the page, 0 while deleted or after an undone
    /// creation.
    pub(crate) count: usize,
    pub(crate) kind: LineKind,
    pub(crate) geometry: LineGeometry,
    pub(crate) style: LineStyle,
    pub(crate) deleted: bool,
}

/// The complete record state an undo/redo step restores.
#[derive(Debug, Clone)]
pub(crate) struct LineSnapshot {
    pub(crate) line_id: u64,
    pub(crate) start: usize,
    pub(crate) count: usize,
    pub(crate) kind: LineKind,
    pub(crate) geometry: LineGeometry,
    pub(crate) style: LineStyle,
    pub(crate) deleted: bool,
    pub(crate) record_index: usize,
}

impl LineRecord {
    fn snapshot(&self, record_index: usize) -> LineSnapshot {
        LineSnapshot {
            line_id: self.line_id,
            start: self.start,
            count: self.count,
            kind: self.kind,
            geometry: self.geometry,
            style: self.style.clone(),
            deleted: self.deleted,
            record_index,
        }
    }

    fn restore(&mut self, snapshot: &LineSnapshot) {
        self.line_id = snapshot.line_id;
        self.start = snapshot.start;
        self.count = snapshot.count;
        self.kind = snapshot.kind;
        self.geometry = snapshot.geometry;
        self.style = snapshot.style.clone();
        self.deleted = snapshot.deleted;
    }

    fn from_snapshot(snapshot: &LineSnapshot) -> Self {
        Self {
            line_id: snapshot.line_id,
            start: snapshot.start,
            count: snapshot.count,
            kind: snapshot.kind,
            geometry: snapshot.geometry,
            style: snapshot.style.clone(),
            deleted: snapshot.deleted,
        }
    }

    fn view(&self, page_index: u32) -> LineObjectView {
        LineObjectView {
            line_id: self.line_id,
            page_index,
            kind: self.kind,
            geometry: self.geometry,
            style: self.style.clone(),
        }
    }
}

fn invalid_argument(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn parse_color(color: &str) -> Result<(u8, u8, u8), CommandError> {
    let hex = color
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit()))
        .ok_or_else(|| invalid_argument(format!("invalid line colour {color:?}")))?;
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16).expect("validated hexadecimal colour channel")
    };
    Ok((channel(0..2), channel(2..4), channel(4..6)))
}

fn normalize_style(mut style: LineStyle) -> Result<LineStyle, CommandError> {
    let (red, green, blue) = parse_color(&style.stroke_color)?;
    style.stroke_color = format!("#{red:02x}{green:02x}{blue:02x}");
    if style.opacity > 100 {
        return Err(invalid_argument(
            "the line opacity must be between 0 and 100",
        ));
    }
    if !style.line_width.is_finite() || style.line_width <= 0.0 || style.line_width > MAX_LINE_WIDTH
    {
        return Err(invalid_argument(format!(
            "the line width must be greater than 0 and at most {MAX_LINE_WIDTH}"
        )));
    }
    Ok(style)
}

fn line_length(geometry: &LineGeometry) -> f32 {
    let dx = geometry.x2 - geometry.x1;
    let dy = geometry.y2 - geometry.y1;
    (dx * dx + dy * dy).sqrt()
}

fn normalize_geometry(geometry: LineGeometry) -> Result<LineGeometry, CommandError> {
    let finite = geometry.x1.is_finite()
        && geometry.y1.is_finite()
        && geometry.x2.is_finite()
        && geometry.y2.is_finite();
    if !finite {
        return Err(invalid_argument("line endpoints must be finite numbers"));
    }
    let coordinates = [geometry.x1, geometry.y1, geometry.x2, geometry.y2];
    if coordinates.iter().any(|value| value.abs() > MAX_COORDINATE) {
        return Err(invalid_argument(format!(
            "line endpoints must stay within {MAX_COORDINATE} points of the page origin"
        )));
    }
    if line_length(&geometry) < MIN_LINE_LENGTH {
        return Err(invalid_argument(
            "a line needs two distinct endpoints at least 2 points apart",
        ));
    }
    Ok(geometry)
}

fn opacity_to_alpha(opacity: u8) -> u8 {
    ((u32::from(opacity) * 255 + 50) / 100) as u8
}

fn pdf_color(red: u8, green: u8, blue: u8, alpha: u8) -> PdfColor {
    PdfColor::new(red, green, blue, alpha)
}

/// The three display-space points of the arrow head: tip, left base, right
/// base. The overlay mirrors this calculation for its live preview.
fn arrow_head(geometry: &LineGeometry, line_width: f32) -> [(f32, f32); 3] {
    let dx = geometry.x2 - geometry.x1;
    let dy = geometry.y2 - geometry.y1;
    let length = (dx * dx + dy * dy).sqrt().max(f32::MIN_POSITIVE);
    let (ux, uy) = (dx / length, dy / length);
    let head_length = (line_width * ARROW_HEAD_LENGTH_PER_WIDTH)
        .clamp(ARROW_HEAD_MIN_LENGTH, ARROW_HEAD_MAX_LENGTH)
        .min(length);
    let half_width = head_length * ARROW_HEAD_HALF_WIDTH_RATIO;
    let base_x = geometry.x2 - ux * head_length;
    let base_y = geometry.y2 - uy * head_length;
    [
        (geometry.x2, geometry.y2),
        (base_x - uy * half_width, base_y + ux * half_width),
        (base_x + uy * half_width, base_y - ux * half_width),
    ]
}

/// Builds the detached path object for a line or arrow.
///
/// The path is drawn directly in PDF user space, so page rotation, CropBox
/// offset and the object's own angle are all baked into its coordinates. An
/// arrow is one path with two sub-paths: the shaft and a filled triangle, so a
/// single page object covers both the stroke and the head.
fn build_line_object(
    document: &PdfDocument<'static>,
    page: &DisplayGeometry,
    kind: LineKind,
    geometry: &LineGeometry,
    style: &LineStyle,
) -> Result<PdfPageObject<'static>, CommandError> {
    let (red, green, blue) = parse_color(&style.stroke_color)?;
    let stroke = pdf_color(red, green, blue, opacity_to_alpha(style.opacity));
    let fill = if kind == LineKind::Arrow {
        Some(stroke)
    } else {
        None
    };
    let start = page.point_inverse(geometry.x1, geometry.y1);
    let end = page.point_inverse(geometry.x2, geometry.y2);
    let mut object = PdfPagePathObject::new(
        document,
        PdfPoints::new(start.0),
        PdfPoints::new(start.1),
        Some(stroke),
        Some(PdfPoints::new(style.line_width)),
        fill,
    )
    .map_err(|error| {
        CommandError::new(
            CommandErrorCode::RenderFailed,
            format!("could not create a line page object: {error:?}"),
        )
    })?;
    object
        .line_to(PdfPoints::new(end.0), PdfPoints::new(end.1))
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not add the line segment: {error:?}"),
            )
        })?;
    if kind == LineKind::Arrow {
        let points = arrow_head(geometry, style.line_width);
        let mut mapped = points.iter().map(|(x, y)| page.point_inverse(*x, *y));
        let first = mapped.next().expect("the arrow head has three points");
        object
            .move_to(PdfPoints::new(first.0), PdfPoints::new(first.1))
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::RenderFailed,
                    format!("could not start the arrow head: {error:?}"),
                )
            })?;
        for (x, y) in mapped {
            object
                .line_to(PdfPoints::new(x), PdfPoints::new(y))
                .map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::RenderFailed,
                        format!("could not add an arrow head edge: {error:?}"),
                    )
                })?;
        }
        object.close_path().map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not close the arrow head: {error:?}"),
            )
        })?;
    }
    object
        .set_line_cap(PdfPageObjectLineCap::Round)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not set the line cap: {error:?}"),
            )
        })?;
    Ok(object.into())
}

fn page_geometry(
    document: &PdfDocument<'static>,
    page_index: u32,
) -> Result<(DisplayGeometry, usize), CommandError> {
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
    let geometry = DisplayGeometry::new(&page, rotation)?;
    let object_count = page.objects().len();
    Ok((geometry, object_count))
}

fn find_line(session: &EditSession, page_index: u32, line_id: u64) -> Result<usize, CommandError> {
    session
        .lines
        .get(&page_index)
        .and_then(|records| records.iter().position(|record| record.line_id == line_id))
        .ok_or_else(|| invalid_argument(format!("no line {line_id} on page {page_index}")))
}

fn deleted_line(line_id: u64) -> CommandError {
    invalid_argument(format!("line {line_id} has been deleted"))
}

fn push_line_action(entry: &mut OpenDocument, mut action: LineEditAction) {
    action.before_state = entry.edit.current_state;
    action.after_state = entry.edit.next_state + 1;
    entry.edit.next_state = action.after_state;
    let after_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Line(action));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
}

fn result_for(
    entry: &OpenDocument,
    page_index: u32,
    line_id: u64,
) -> Result<LineObjectView, CommandError> {
    entry
        .edit
        .lines
        .get(&page_index)
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.line_id == line_id && !record.deleted)
        })
        .map(|record| record.view(page_index))
        .ok_or_else(|| deleted_line(line_id))
}

fn insert_for(
    entry: &mut OpenDocument,
    page_index: u32,
    request: CreateLineRequest,
) -> Result<LineEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let geometry = normalize_geometry(request.geometry)?;
    let style = normalize_style(request.style)?;
    let (page, start) = page_geometry(&entry.document, page_index)?;
    let object = build_line_object(&entry.document, &page, request.kind, &geometry, &style)?;
    {
        let mut page = load_page(&entry.document, page_index)?;
        page.objects_mut().add_object(object).map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not add the line to the page: {error:?}"),
            )
        })?;
    }

    entry.edit.next_line_id += 1;
    let line_id = entry.edit.next_line_id;
    let record_index = entry.edit.lines.get(&page_index).map(Vec::len).unwrap_or(0);
    let record = LineRecord {
        line_id,
        start,
        count: 1,
        kind: request.kind,
        geometry,
        style,
        deleted: false,
    };
    let after_snapshot = record.snapshot(record_index);
    entry.edit.lines.entry(page_index).or_default().push(record);
    push_line_action(
        entry,
        LineEditAction {
            page_index,
            line_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: None,
            before_objects: Some(Vec::new()),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );

    let line = result_for(entry, page_index, line_id)?;
    Ok(LineEditResultView {
        page_index,
        line,
        history: entry.edit.history_view(),
    })
}

fn lines_for(entry: &OpenDocument, page_index: u32) -> Result<Vec<LineObjectView>, CommandError> {
    // Reject a page index outside the document even when no lines exist.
    load_page(&entry.document, page_index)?;
    Ok(entry
        .edit
        .lines
        .get(&page_index)
        .map(|records| {
            records
                .iter()
                .filter(|record| !record.deleted)
                .map(|record| record.view(page_index))
                .collect()
        })
        .unwrap_or_default())
}

fn transform_for(
    entry: &mut OpenDocument,
    page_index: u32,
    line_id: u64,
    request: LineTransformRequest,
) -> Result<LineEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let geometry = normalize_geometry(request.geometry)?;
    let record_index = find_line(&entry.edit, page_index, line_id)?;
    let (start, count, deleted, kind, style) = {
        let record = &entry.edit.lines[&page_index][record_index];
        (
            record.start,
            record.count,
            record.deleted,
            record.kind,
            record.style.clone(),
        )
    };
    if deleted {
        return Err(deleted_line(line_id));
    }

    // Rebuild the object before detaching anything, so a build failure cannot
    // leave the page half-edited.
    let (page, _) = page_geometry(&entry.document, page_index)?;
    let object = build_line_object(&entry.document, &page, kind, &geometry, &style)?;
    let before_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, count, vec![object])?;
    {
        let record = &mut entry.edit.lines.get_mut(&page_index).expect("line page")[record_index];
        record.geometry = geometry;
        record.count = 1;
    }
    let after_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    push_line_action(
        entry,
        LineEditAction {
            page_index,
            line_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: Some(before_snapshot),
            before_objects: Some(removed),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );

    let line = result_for(entry, page_index, line_id)?;
    Ok(LineEditResultView {
        page_index,
        line,
        history: entry.edit.history_view(),
    })
}

fn style_for(
    entry: &mut OpenDocument,
    page_index: u32,
    line_id: u64,
    style: LineStyle,
) -> Result<LineEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let style = normalize_style(style)?;
    let record_index = find_line(&entry.edit, page_index, line_id)?;
    let (start, count, deleted, kind, geometry) = {
        let record = &entry.edit.lines[&page_index][record_index];
        (
            record.start,
            record.count,
            record.deleted,
            record.kind,
            record.geometry,
        )
    };
    if deleted {
        return Err(deleted_line(line_id));
    }

    let (page, _) = page_geometry(&entry.document, page_index)?;
    let object = build_line_object(&entry.document, &page, kind, &geometry, &style)?;
    let before_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, count, vec![object])?;
    {
        let record = &mut entry.edit.lines.get_mut(&page_index).expect("line page")[record_index];
        record.style = style;
        record.count = 1;
    }
    let after_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    push_line_action(
        entry,
        LineEditAction {
            page_index,
            line_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: Some(before_snapshot),
            before_objects: Some(removed),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );

    let line = result_for(entry, page_index, line_id)?;
    Ok(LineEditResultView {
        page_index,
        line,
        history: entry.edit.history_view(),
    })
}

fn delete_for(
    entry: &mut OpenDocument,
    page_index: u32,
    line_id: u64,
) -> Result<EditStepView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let record_index = find_line(&entry.edit, page_index, line_id)?;
    let (start, count, deleted) = {
        let record = &entry.edit.lines[&page_index][record_index];
        (record.start, record.count, record.deleted)
    };
    if deleted {
        return Err(deleted_line(line_id));
    }
    let before_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, count, Vec::new())?;
    {
        let record = &mut entry.edit.lines.get_mut(&page_index).expect("line page")[record_index];
        record.count = 0;
        record.deleted = true;
    }
    let after_snapshot = entry.edit.lines[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        -(count as isize),
        ObjectRef::Line(line_id),
    );
    push_line_action(
        entry,
        LineEditAction {
            page_index,
            line_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: Some(before_snapshot),
            before_objects: Some(removed),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );
    Ok(EditStepView {
        page_index,
        history: entry.edit.history_view(),
    })
}

/// Undo of a line action. Called by the unified history after the action has
/// been popped from the undo stack.
pub(crate) fn undo_line(
    entry: &mut OpenDocument,
    mut action: LineEditAction,
) -> Result<EditStepView, CommandError> {
    let before_count = action
        .before_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let after_count = action
        .after_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
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
        let lines = entry.edit.lines.entry(page_index).or_default();
        match &action.before_record {
            Some(before) => {
                let index = lines
                    .iter()
                    .position(|record| record.line_id == action.line_id)
                    .ok_or_else(|| {
                        CommandError::new(
                            CommandErrorCode::Internal,
                            format!(
                                "the edit history lost line {} on page {page_index}",
                                action.line_id
                            ),
                        )
                    })?;
                lines[index].restore(before);
            }
            None => lines.retain(|record| record.line_id != action.line_id),
        }
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        before_count as isize - after_count as isize,
        ObjectRef::Line(action.line_id),
    );

    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Line(action));
    Ok(EditStepView {
        page_index,
        history: entry.edit.history_view(),
    })
}

/// Redo of a line action. Called by the unified history after the action has
/// been popped from the redo stack.
pub(crate) fn redo_line(
    entry: &mut OpenDocument,
    mut action: LineEditAction,
) -> Result<EditStepView, CommandError> {
    let before_count = action
        .before_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let after_count = action
        .after_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
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
        let lines = entry.edit.lines.entry(page_index).or_default();
        match &action.after_record {
            Some(after) => {
                if let Some(index) = lines
                    .iter()
                    .position(|record| record.line_id == action.line_id)
                {
                    lines[index].restore(after);
                } else {
                    let index = after.record_index.min(lines.len());
                    lines.insert(index, LineRecord::from_snapshot(after));
                }
            }
            None => lines.retain(|record| record.line_id != action.line_id),
        }
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        after_count as isize - before_count as isize,
        ObjectRef::Line(action.line_id),
    );

    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Line(action));
    Ok(EditStepView {
        page_index,
        history: entry.edit.history_view(),
    })
}

/// The `create_line_object` command: draws a line or arrow as one undoable
/// page-content object on the current Page.
#[tauri::command]
pub fn create_line_object(
    document_id: String,
    request: CreateLineRequest,
) -> Result<LineEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        insert_for(entry, request.page_index, request)
    })
}

/// The `page_line_objects` command: every line or arrow this session created
/// on the Page, in creation order. Deleted objects are excluded.
#[tauri::command]
pub fn page_line_objects(
    document_id: String,
    page_index: u32,
) -> Result<Vec<LineObjectView>, CommandError> {
    with_open_document(&document_id, |entry| lines_for(entry, page_index))
}

/// The `transform_line_object` command: replaces the endpoints, which covers
/// moving, scaling, endpoint dragging and rotation as one undoable action.
#[tauri::command]
pub fn transform_line_object(
    document_id: String,
    page_index: u32,
    line_id: u64,
    transform: LineTransformRequest,
) -> Result<LineEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        transform_for(entry, page_index, line_id, transform)
    })
}

/// The `update_line_style` command: changes colour, opacity or width as one
/// undoable content edit.
#[tauri::command]
pub fn update_line_style(
    document_id: String,
    page_index: u32,
    line_id: u64,
    style: LineStyle,
) -> Result<LineEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        style_for(entry, page_index, line_id, style)
    })
}

/// The `delete_line_object` command: removes the line's page object. Undo
/// restores the page exactly as it was.
#[tauri::command]
pub fn delete_line_object(
    document_id: String,
    page_index: u32,
    line_id: u64,
) -> Result<EditStepView, CommandError> {
    with_open_document(&document_id, |entry| delete_for(entry, page_index, line_id))
}
