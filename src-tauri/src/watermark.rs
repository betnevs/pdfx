//! Text and image watermarks as real page content.
//!
//! A watermark is a PDFium page object appended to each target page, not a
//! native annotation. It therefore renders in the live canvas, thumbnails,
//! print, PNG/JPEG export, flatten export and the saved Output Document through
//! the same pipeline as every other page object.
//!
//! # One watermark per session
//!
//! The session holds at most one active watermark configuration. Applying
//! again removes every object of the previous watermark before inserting the
//! new one, so repeated applications replace instead of stacking. The unified
//! edit history stores detached page-object snapshots for both sides of an
//! apply/replace/delete, which makes undo and redo restore the exact previous
//! rendering.
//!
//! # Page scope
//!
//! "Current page" and "all pages" are logical-page operations. `all` walks the
//! visible [`crate::page_ops::PageModel`] order, so deleted pages never receive
//! a watermark even though their physical PDFium pages still exist.
//!
//! # Text fonts
//!
//! Text watermarks use the bundled Noto Sans SC subset through the same cache
//! as Text Boxes (ADR-0006), so Simplified Chinese and Latin share one
//! freely-embeddable font program and repeated applications do not embed the
//! same subset twice.

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfMatrix, PdfPageObject, PdfPageObjectCommon, PdfPageObjectsCommon,
    PdfPageTextObject, PdfPoints,
};
use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::image_edit::{self, DecodedImage, ImageLayout};
use crate::text_edit::{ensure_content_editable, EditAction, EditHistoryView, EditStepView};

/// Longest accepted watermark text, in characters.
const MAX_TEXT_CHARS: usize = 200;
const MIN_OPACITY: u8 = 1;
const MAX_OPACITY: u8 = 100;
const MIN_SCALE: f32 = 10.0;
const MAX_SCALE: f32 = 400.0;
const MIN_ROTATION: f32 = -180.0;
const MAX_ROTATION: f32 = 180.0;
/// Font size at 100% scale as a fraction of the page's shorter display side.
const TEXT_FONT_RATIO: f32 = 0.10;
/// Smallest font size PDFium can place reliably, in points.
const MIN_FONT_SIZE: f32 = 1.0;
/// Default image watermark box at 100% scale, as a fraction of the page.
const IMAGE_FIT_RATIO: f32 = 0.6;

/// Which logical pages receive the watermark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatermarkScope {
    Current,
    All,
}

/// Text or image watermark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatermarkKind {
    Text,
    Image,
}

/// One apply request from the watermark panel.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkRequest {
    pub scope: WatermarkScope,
    pub kind: WatermarkKind,
    /// Text to draw when `kind` is `Text`.
    pub text: Option<String>,
    /// PNG or JPEG file to place when `kind` is `Image`.
    pub image_path: Option<String>,
    /// Constant opacity in percent (1-100).
    pub opacity: u8,
    /// Clockwise rotation around the mark's center, in degrees.
    pub rotation: f32,
    /// Mark size in percent of its default size.
    pub scale: f32,
    /// Center position as a fraction of the display width (0-1).
    pub position_x: f32,
    /// Center position as a fraction of the display height (0-1).
    pub position_y: f32,
}

/// The active watermark as the panel and overlays see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkView {
    pub kind: WatermarkKind,
    /// The scope chosen when this watermark was applied.
    pub scope: WatermarkScope,
    pub text: Option<String>,
    pub image_name: Option<String>,
    pub image_path: Option<String>,
    pub opacity: u8,
    pub rotation: f32,
    pub scale: f32,
    pub position_x: f32,
    pub position_y: f32,
    /// Logical page indices carrying the watermark, in page order. Deleted
    /// pages are omitted.
    pub page_indices: Vec<u32>,
}

/// The outcome of applying or deleting a watermark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkEditResult {
    /// `None` after deleting the watermark.
    pub watermark: Option<WatermarkView>,
    pub history: EditHistoryView,
}

/// A local image chosen for an image watermark, previewed before applying.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkImagePreview {
    pub name: String,
    pub mime_type: String,
    /// `data:` URL the panel can show without another file read.
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

/// Style and content of the one active watermark. Detached page objects live
/// in the history snapshots, so this does not keep image bytes or fonts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatermarkConfig {
    pub(crate) kind: WatermarkKind,
    pub(crate) scope: WatermarkScope,
    pub(crate) text: Option<String>,
    pub(crate) image_name: Option<String>,
    pub(crate) image_path: Option<String>,
    pub(crate) opacity: u8,
    pub(crate) rotation: f32,
    pub(crate) scale: f32,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
}

impl WatermarkConfig {
    fn view(&self, page_indices: Vec<u32>) -> WatermarkView {
        WatermarkView {
            kind: self.kind,
            scope: self.scope,
            text: self.text.clone(),
            image_name: self.image_name.clone(),
            image_path: self.image_path.clone(),
            opacity: self.opacity,
            rotation: self.rotation,
            scale: self.scale,
            position_x: self.position_x,
            position_y: self.position_y,
            page_indices,
        }
    }
}

/// The object range of the active watermark on one physical page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatermarkPage {
    pub(crate) physical_page: u32,
    pub(crate) start: usize,
    pub(crate) count: usize,
}

/// Session state of the one active watermark.
pub(crate) struct WatermarkState {
    pub(crate) config: WatermarkConfig,
    pub(crate) pages: Vec<WatermarkPage>,
}

/// One physical page's contribution to a history snapshot.
///
/// `objects` is `Some` while the objects are detached off the page, mirroring
/// [`crate::text_edit::ImageEditAction`]. Whichever side is not currently
/// rendered keeps its objects here.
pub(crate) struct WatermarkPageSnapshot {
    pub(crate) physical_page: u32,
    pub(crate) start: usize,
    pub(crate) objects: Option<Vec<PdfPageObject<'static>>>,
}

/// One side of a watermark change. `None` at the action level means no
/// watermark was active.
pub(crate) struct WatermarkSnapshot {
    pub(crate) config: WatermarkConfig,
    pub(crate) pages: Vec<WatermarkPageSnapshot>,
}

/// One committed apply, replace or delete of the watermark.
pub(crate) struct WatermarkEditAction {
    pub(crate) before_state: u64,
    pub(crate) after_state: u64,
    pub(crate) before: Option<WatermarkSnapshot>,
    pub(crate) after: Option<WatermarkSnapshot>,
}

fn invalid_argument(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn internal(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::Internal, message)
}

fn opacity_alpha(opacity: u8) -> u8 {
    ((u32::from(opacity) * 255 + 50) / 100) as u8
}

fn normalize_rotation(angle: f32) -> f32 {
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

/// Validates a request and turns it into the session configuration.
///
/// Validation runs before any document mutation, so a rejected request leaves
/// the page objects and the edit history untouched.
fn config_from_request(request: &WatermarkRequest) -> Result<WatermarkConfig, CommandError> {
    if !(MIN_OPACITY..=MAX_OPACITY).contains(&request.opacity) {
        return Err(invalid_argument(format!(
            "watermark opacity must be between {MIN_OPACITY} and {MAX_OPACITY} percent"
        )));
    }
    if !request.rotation.is_finite() || !(MIN_ROTATION..=MAX_ROTATION).contains(&request.rotation) {
        return Err(invalid_argument(format!(
            "watermark rotation must be between {MIN_ROTATION} and {MAX_ROTATION} degrees"
        )));
    }
    if !request.scale.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&request.scale) {
        return Err(invalid_argument(format!(
            "watermark scale must be between {MIN_SCALE} and {MAX_SCALE} percent"
        )));
    }
    for (name, value) in [
        ("positionX", request.position_x),
        ("positionY", request.position_y),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(invalid_argument(format!(
                "watermark {name} must be between 0 and 1"
            )));
        }
    }

    let scope = request.scope;
    let (text, image_name, image_path) = match request.kind {
        WatermarkKind::Text => {
            let raw = request.text.as_deref().unwrap_or_default();
            let text = raw.trim();
            if text.is_empty() {
                return Err(invalid_argument("watermark text must not be empty"));
            }
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(invalid_argument(format!(
                    "watermark text must not exceed {MAX_TEXT_CHARS} characters"
                )));
            }
            if text.contains(['\n', '\r']) {
                return Err(invalid_argument(
                    "watermark text must be a single line without line breaks",
                ));
            }
            (Some(text.to_owned()), None, None)
        }
        WatermarkKind::Image => {
            let path = request
                .image_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| invalid_argument("an image watermark requires an image path"))?;
            let name = std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid_argument("the image path has no file name"))?;
            (None, Some(name), Some(path.to_owned()))
        }
    };

    Ok(WatermarkConfig {
        kind: request.kind,
        scope,
        text,
        image_name,
        image_path,
        opacity: request.opacity,
        rotation: normalize_rotation(request.rotation),
        scale: request.scale,
        position_x: request.position_x,
        position_y: request.position_y,
    })
}

fn target_physical_pages(
    entry: &OpenDocument,
    scope: WatermarkScope,
    page_index: u32,
) -> Result<Vec<u32>, CommandError> {
    match scope {
        WatermarkScope::Current => Ok(vec![entry.physical_page(page_index)?]),
        WatermarkScope::All => Ok(entry.pages.order.clone()),
    }
}

fn view_for(entry: &OpenDocument) -> Option<WatermarkView> {
    let state = entry.edit.watermark.as_ref()?;
    let mut indices: Vec<u32> = state
        .pages
        .iter()
        .filter_map(|page| entry.pages.logical(page.physical_page))
        .collect();
    indices.sort_unstable();
    indices.dedup();
    Some(state.config.view(indices))
}

/// The page size, rotation and current object count used to place a new mark.
fn page_geometry(
    document: &PdfDocument<'static>,
    physical_page: u32,
) -> Result<(DisplayGeometry, usize), CommandError> {
    let page = load_page(document, physical_page)?;
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {physical_page} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    let count = page.objects().len();
    Ok((geometry, count))
}

/// Builds a text watermark object centered at the requested display position.
fn build_text_object(
    entry: &mut OpenDocument,
    geometry: &DisplayGeometry,
    config: &WatermarkConfig,
    text: &str,
) -> Result<PdfPageObject<'static>, CommandError> {
    let token = entry
        .edit
        .bundled_fonts
        .token(&mut entry.document, text, 400)?;
    let (display_width, display_height) = geometry.display_size();
    let min_side = display_width.min(display_height);
    let font_size = (min_side * TEXT_FONT_RATIO * config.scale / 100.0).max(MIN_FONT_SIZE);
    let mut object =
        PdfPageTextObject::new(&entry.document, text, token, PdfPoints::new(font_size))
            .map_err(|error| internal(format!("could not create the watermark text: {error:?}")))?;
    object
        .set_fill_color(PdfColor::new(0, 0, 0, opacity_alpha(config.opacity)))
        .map_err(|error| internal(format!("could not set watermark opacity: {error:?}")))?;

    let bounds = object
        .bounds()
        .map_err(|error| internal(format!("could not measure the watermark text: {error:?}")))?
        .to_rect();
    let left = bounds.left().value;
    let bottom = bounds.bottom().value;
    let width = bounds.width().value;
    let height = bounds.height().value;
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err(internal("the watermark text has no measurable size"));
    }

    let center_x = config.position_x * display_width;
    let center_y = config.position_y * display_height;
    let origin_x = center_x - width / 2.0;
    let origin_y = center_y - height / 2.0;
    let radians = config.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let rotate = |x: f32, y: f32| {
        let dx = x - center_x;
        let dy = y - center_y;
        (
            center_x + dx * cos - dy * sin,
            center_y + dx * sin + dy * cos,
        )
    };
    // Display space is y-down, so the text's local bottom edge is at the
    // rectangle's bottom (origin_y + height).
    let (bl_x, bl_y) = rotate(origin_x, origin_y + height);
    let (br_x, br_y) = rotate(origin_x + width, origin_y + height);
    let (tl_x, tl_y) = rotate(origin_x, origin_y);
    let (bl_x, bl_y) = geometry.point_inverse(bl_x, bl_y);
    let (br_x, br_y) = geometry.point_inverse(br_x, br_y);
    let (tl_x, tl_y) = geometry.point_inverse(tl_x, tl_y);

    let a = (br_x - bl_x) / width;
    let b = (br_y - bl_y) / width;
    let c = (tl_x - bl_x) / height;
    let d = (tl_y - bl_y) / height;
    let e = bl_x - a * left - c * bottom;
    let f = bl_y - b * left - d * bottom;

    object
        .apply_matrix(PdfMatrix::new(a, b, c, d, e, f))
        .map_err(|error| internal(format!("could not place the watermark text: {error:?}")))?;
    Ok(object.into())
}

/// Multiplies an image's alpha channel so a page object carries the requested
/// opacity. PDFium page image objects have no fill colour, so the alpha is
/// baked into the bitmap; undo/redo snapshots keep the resulting object.
fn apply_image_opacity(decoded: &mut DecodedImage, opacity: u8) {
    if opacity == 100 {
        return;
    }
    for pixel in decoded.bgra.chunks_exact_mut(4) {
        pixel[3] = ((u32::from(pixel[3]) * u32::from(opacity) + 50) / 100) as u8;
    }
}

/// Display-space box for an image watermark: the image keeps its aspect ratio
/// and is scaled around its center.
fn image_layout(
    geometry: &DisplayGeometry,
    decoded: &DecodedImage,
    config: &WatermarkConfig,
) -> Result<ImageLayout, CommandError> {
    let (display_width, display_height) = geometry.display_size();
    if decoded.width == 0 || decoded.height == 0 {
        return Err(invalid_argument("the watermark image has no pixels"));
    }
    let aspect = decoded.width as f32 / decoded.height as f32;
    let base_width =
        (IMAGE_FIT_RATIO * display_width).min(IMAGE_FIT_RATIO * display_height * aspect);
    let width = (base_width * config.scale / 100.0).max(1.0);
    let height = (width / aspect).max(1.0);
    Ok(ImageLayout {
        x: config.position_x * display_width - width / 2.0,
        y: config.position_y * display_height - height / 2.0,
        width,
        height,
        rotation: config.rotation,
    })
}

/// Builds every target page's object before the document is touched, so a
/// failure leaves the previous watermark in place.
fn build_objects(
    entry: &mut OpenDocument,
    config: &WatermarkConfig,
    targets: &[u32],
) -> Result<Vec<(u32, PdfPageObject<'static>)>, CommandError> {
    let mut built = Vec::with_capacity(targets.len());
    match config.kind {
        WatermarkKind::Text => {
            let text = config
                .text
                .as_deref()
                .expect("a text watermark has validated text");
            for &physical_page in targets {
                let (geometry, _) = page_geometry(&entry.document, physical_page)?;
                let object = build_text_object(entry, &geometry, config, text)?;
                built.push((physical_page, object));
            }
        }
        WatermarkKind::Image => {
            let path = config
                .image_path
                .as_deref()
                .expect("an image watermark has a validated path");
            let mut loaded = image_edit::load_image(path)?;
            apply_image_opacity(&mut loaded.decoded, config.opacity);
            for &physical_page in targets {
                let (geometry, _) = page_geometry(&entry.document, physical_page)?;
                let layout = image_layout(&geometry, &loaded.decoded, config)?;
                image_edit::check_layout(&layout)?;
                let object = image_edit::build_image_object(
                    &entry.document,
                    &geometry,
                    &layout,
                    loaded.decoded.clone(),
                )?;
                built.push((physical_page, object));
            }
        }
    }
    Ok(built)
}

/// Whether `object_index` belongs to the active watermark on `physical_page`.
///
/// The watermark is page content and stays searchable, but it is not a Text
/// Box: editing it through the text layer would replace objects underneath the
/// watermark's tracked range and let a later delete detach the wrong object.
pub(crate) fn is_watermark_object(
    entry: &OpenDocument,
    physical_page: u32,
    object_index: usize,
) -> bool {
    entry
        .edit
        .watermark
        .as_ref()
        .and_then(|state| {
            state
                .pages
                .iter()
                .find(|page| page.physical_page == physical_page)
        })
        .is_some_and(|page| object_index >= page.start && object_index < page.start + page.count)
}

/// Registers the watermark objects a page duplication carried along.
///
/// `copy_page` clones the source page from a serialization of the live
/// document, so a page that carries the active watermark produces a copy with
/// the same content objects in the same order. Recording the copy lets a later
/// replace or delete remove the mark there too; without it the copy would look
/// like ordinary unmarked page content and the mark would linger as a second,
/// untracked layer. Returns whether the copy was added to the active watermark.
pub(crate) fn track_copied_page(
    entry: &mut OpenDocument,
    source: u32,
    copy: u32,
) -> Result<bool, CommandError> {
    let Some(state) = entry.edit.watermark.as_mut() else {
        return Ok(false);
    };
    if state.pages.iter().any(|page| page.physical_page == copy) {
        return Ok(false);
    }
    let Some(source_page) = state
        .pages
        .iter()
        .find(|page| page.physical_page == source)
        .cloned()
    else {
        return Ok(false);
    };

    let source_count = load_page(&entry.document, source)?.objects().len();
    let copy_count = load_page(&entry.document, copy)?.objects().len();
    if source_count != copy_count || source_page.start + source_page.count > copy_count {
        // The importer changed the object layout; do not guess an index range
        // that could later detach the wrong object.
        return Ok(false);
    }
    state.pages.push(WatermarkPage {
        physical_page: copy,
        start: source_page.start,
        count: source_page.count,
    });
    Ok(true)
}

/// Removes the watermark's objects from every tracked page and returns them as
/// history snapshots.
fn take_current(entry: &mut OpenDocument) -> Result<Option<WatermarkSnapshot>, CommandError> {
    let Some(state) = entry.edit.watermark.take() else {
        return Ok(None);
    };
    let mut pages = Vec::with_capacity(state.pages.len());
    for page in state.pages {
        let mut handle = load_page(&entry.document, page.physical_page)?;
        let objects = handle.objects_mut();
        if page.start + page.count > objects.len() {
            return Err(internal(format!(
                "the watermark refers to page objects {}..{} but page {} has {} objects",
                page.start,
                page.start + page.count,
                page.physical_page,
                objects.len()
            )));
        }
        let mut removed = Vec::with_capacity(page.count);
        for _ in 0..page.count {
            removed.push(
                objects
                    .remove_object_at_index(page.start)
                    .map_err(|error| {
                        internal(format!("could not detach the watermark: {error:?}"))
                    })?,
            );
        }
        pages.push(WatermarkPageSnapshot {
            physical_page: page.physical_page,
            start: page.start,
            objects: Some(removed),
        });
    }
    Ok(Some(WatermarkSnapshot {
        config: state.config,
        pages,
    }))
}

/// Puts a snapshot's objects back on its pages and makes it the session's
/// active watermark.
fn install_snapshot(
    entry: &mut OpenDocument,
    snapshot: WatermarkSnapshot,
) -> Result<(), CommandError> {
    let mut pages = Vec::with_capacity(snapshot.pages.len());
    for mut page in snapshot.pages {
        let objects = page
            .objects
            .take()
            .ok_or_else(|| internal("the watermark history lost its detached objects"))?;
        let mut handle = load_page(&entry.document, page.physical_page)?;
        let page_objects = handle.objects_mut();
        if page.start > page_objects.len() {
            return Err(internal(format!(
                "could not restore the watermark at object {}; page {} has {} objects",
                page.start,
                page.physical_page,
                page_objects.len()
            )));
        }
        let count = objects.len();
        for (offset, object) in objects.into_iter().enumerate() {
            page_objects
                .insert_object_at_index(page.start + offset, object)
                .map_err(|error| internal(format!("could not restore the watermark: {error:?}")))?;
        }
        pages.push(WatermarkPage {
            physical_page: page.physical_page,
            start: page.start,
            count,
        });
    }
    entry.edit.watermark = Some(WatermarkState {
        config: snapshot.config,
        pages,
    });
    Ok(())
}

/// Appends a committed watermark change to the unified undo stack.
fn push_action(
    entry: &mut OpenDocument,
    before: Option<WatermarkSnapshot>,
    after: Option<WatermarkSnapshot>,
) {
    let before_state = entry.edit.current_state;
    let after_state = entry.edit.next_state + 1;
    entry.edit.next_state = after_state;
    entry
        .edit
        .undo_stack
        .push(EditAction::Watermark(WatermarkEditAction {
            before_state,
            after_state,
            before,
            after,
        }));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
}

fn apply_for(
    entry: &mut OpenDocument,
    page_index: u32,
    request: WatermarkRequest,
) -> Result<WatermarkEditResult, CommandError> {
    ensure_content_editable(&entry.document)?;
    let config = config_from_request(&request)?;
    let targets = target_physical_pages(entry, request.scope, page_index)?;
    if targets.is_empty() {
        return Err(invalid_argument("the document has no visible pages"));
    }

    if let Some(state) = &entry.edit.watermark {
        let same_pages = {
            let mut current: Vec<u32> = state.pages.iter().map(|page| page.physical_page).collect();
            let mut wanted = targets.clone();
            current.sort_unstable();
            wanted.sort_unstable();
            current == wanted
        };
        if same_pages && state.config == config {
            // Re-applying the same watermark is a no-op, not a second layer.
            return Ok(WatermarkEditResult {
                watermark: view_for(entry),
                history: entry.edit.history_view(),
            });
        }
    }

    let built = build_objects(entry, &config, &targets)?;
    let before = take_current(entry)?;

    let mut pages = Vec::with_capacity(built.len());
    for (physical_page, object) in built {
        let mut handle = load_page(&entry.document, physical_page)?;
        let objects = handle.objects_mut();
        let start = objects.len();
        objects.add_object(object).map_err(|error| {
            internal(format!(
                "could not add the watermark to the page: {error:?}"
            ))
        })?;
        drop(handle);
        pages.push(WatermarkPage {
            physical_page,
            start,
            count: 1,
        });
    }
    entry.edit.watermark = Some(WatermarkState {
        config: config.clone(),
        pages: pages.clone(),
    });
    let after = Some(WatermarkSnapshot {
        config,
        pages: pages
            .into_iter()
            .map(|page| WatermarkPageSnapshot {
                physical_page: page.physical_page,
                start: page.start,
                objects: None,
            })
            .collect(),
    });
    push_action(entry, before, after);

    Ok(WatermarkEditResult {
        watermark: view_for(entry),
        history: entry.edit.history_view(),
    })
}

/// The `apply_watermark` command.
#[tauri::command]
pub fn apply_watermark(
    document_id: String,
    page_index: u32,
    request: WatermarkRequest,
) -> Result<WatermarkEditResult, CommandError> {
    with_open_document(&document_id, |entry| apply_for(entry, page_index, request))
}

/// The `watermark_image_preview` command: validates an image file and returns
/// its bytes as a data URL for the panel's live preview.
#[tauri::command]
pub fn watermark_image_preview(path: String) -> Result<WatermarkImagePreview, CommandError> {
    let loaded = image_edit::load_image(&path)?;
    let mime_type = match loaded.format {
        image_edit::ImageFormat::Png => "image/png",
        image_edit::ImageFormat::Jpeg => "image/jpeg",
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(&loaded.source);
    Ok(WatermarkImagePreview {
        name: loaded.name,
        mime_type: mime_type.to_owned(),
        data_url: format!("data:{mime_type};base64,{encoded}"),
        width: loaded.decoded.width,
        height: loaded.decoded.height,
    })
}

/// The `watermark_state` command: the active watermark, or `None`.
#[tauri::command]
pub fn watermark_state(document_id: String) -> Result<Option<WatermarkView>, CommandError> {
    with_open_document(&document_id, |entry| Ok(view_for(entry)))
}

/// The `delete_watermark` command.
#[tauri::command]
pub fn delete_watermark(document_id: String) -> Result<WatermarkEditResult, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_content_editable(&entry.document)?;
        let Some(before) = take_current(entry)? else {
            return Err(invalid_argument("there is no watermark to delete"));
        };
        push_action(entry, Some(before), None);
        Ok(WatermarkEditResult {
            watermark: None,
            history: entry.edit.history_view(),
        })
    })
}

fn step_for(entry: &OpenDocument, physical_page: u32) -> EditStepView {
    EditStepView {
        page_index: physical_page,
        page_count: entry.logical_page_count(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    }
}

/// Undo side of a watermark action: detach the live watermark, restore the
/// recorded previous one.
pub(crate) fn undo_watermark(
    entry: &mut OpenDocument,
    mut action: WatermarkEditAction,
) -> Result<EditStepView, CommandError> {
    let physical_page = action
        .after
        .as_ref()
        .or(action.before.as_ref())
        .and_then(|snapshot| snapshot.pages.first())
        .map(|page| page.physical_page)
        .unwrap_or(0);
    action.after = take_current(entry)?;
    if let Some(before) = action.before.take() {
        install_snapshot(entry, before)?;
    }
    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Watermark(action));
    Ok(step_for(entry, physical_page))
}

/// Redo side of a watermark action.
pub(crate) fn redo_watermark(
    entry: &mut OpenDocument,
    mut action: WatermarkEditAction,
) -> Result<EditStepView, CommandError> {
    let physical_page = action
        .before
        .as_ref()
        .or(action.after.as_ref())
        .and_then(|snapshot| snapshot.pages.first())
        .map(|page| page.physical_page)
        .unwrap_or(0);
    action.before = take_current(entry)?;
    if let Some(after) = action.after.take() {
        install_snapshot(entry, after)?;
    }
    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Watermark(action));
    Ok(step_for(entry, physical_page))
}
