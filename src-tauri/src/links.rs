//! Native `/Link` annotations with URL and in-document page targets.
//!
//! Links are ordinary PDF link annotations: the clickable region is `/Rect`
//! plus `/QuadPoints`, and the target is either a `/S /URI` action or a
//! `/S /GoTo` action with a direct page destination.
//!
//! Pdfium can create a link with a URI action but has no API for writing a
//! page destination. A page link is therefore created with a reserved
//! `https://pdfx.invalid/...` placeholder URI and a private `PDFXLinkTarget`
//! string key. The save pipeline ([`crate::save`]) rewrites `/A` to
//! `<< /S /GoTo /D [page /XYZ null null null] >>` in its existing annotation
//! dictionary revision, so the placeholder never reaches an Output Document
//! and repeated saves re-derive the same action.
//!
//! Existing links written by other tools are read in their standard forms:
//! `/A << /S /URI >>`, `/A << /S /GoTo >>`, a direct `/Dest` array, and named
//! destinations Pdfium can resolve. Links whose action PDFX cannot represent
//! (JavaScript, Launch, remote or embedded GoTo) are listed with a reason and
//! are refused for edit and delete, so an unsupported link is never corrupted.
//!
//! Creation, target edits and deletion go through the unified edit history as
//! [`crate::text_edit::EditAction::Link`], which makes them undoable/redoable
//! alongside text, image, shape and other annotation edits.

use pdfium_render::prelude::{PdfDocument, PdfPageAnnotationType};
use serde::{Deserialize, Serialize};

use crate::annotations::{
    bounds_of_quads, display_rect, display_rects, ensure_annotation_editable, find_target,
    map_adapter_error, next_annotation_name, page_geometry, quad_for_rect, TextMarkupRect,
};
use crate::commands::{with_open_document, CommandError, CommandErrorCode, DisplayGeometry};
use crate::pdfium_adapter::{
    annotation_author, annotation_bounds, annotation_contents, annotation_creation_date,
    annotation_name, annotation_quads, annotation_string_value, annotation_type, append_link_quads,
    create_link_annotation, link_attributes, remove_annotation, set_annotation_author,
    set_annotation_contents, set_annotation_creation_date, set_annotation_name,
    set_annotation_string_value, AnnotationRect, AnnotationRef,
};
use crate::text_edit::{push_link_action, EditHistoryView};

/// Private key carrying the zero-based target page of a link PDFX created.
///
/// It exists because Pdfium cannot write a destination action. The save pass
/// reads it to materialize `/A << /S /GoTo >>`; readers ignore it.
pub(crate) const LINK_TARGET_KEY: &str = "PDFXLinkTarget";

/// Placeholder URI used while a page link lives in Pdfium's document model.
///
/// `.invalid` is reserved by RFC 2606, so the value is unmistakably internal.
/// It is replaced by a `/GoTo` action before any Output Document is written.
const PLACEHOLDER_URI_PREFIX: &str = "https://pdfx.invalid/page/";

/// The longest accepted URL, mirroring common reader limits.
const MAX_LINK_URL_LENGTH: usize = 2048;

/// Smallest accepted link region edge in display points.
const MIN_LINK_EXTENT: f32 = 0.5;

/// A link target the user can create, read and edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum LinkTarget {
    /// An external URL (`http://`, `https://` or `mailto:`).
    Url { url: String },
    /// Another page in the same document, zero-based.
    Page { page_index: u32 },
}

/// Why an existing link target cannot be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkUnsupportedReason {
    /// The action type is not a URI or in-document GoTo action.
    Action,
    /// The destination exists but its page cannot be resolved.
    Destination,
    /// The annotation has no readable action or destination.
    Malformed,
}

/// One link annotation as the overlay and the link dialog need it.
///
/// `id` is the annotation's position on the page (`"page:index"`). It is
/// stable between mutations; every mutation reloads the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    pub id: String,
    /// Stable `/NM` identity; absent on links written by other tools.
    pub name: Option<String>,
    pub page_index: u32,
    /// Present exactly when the link is editable.
    pub target: Option<LinkTarget>,
    /// Present exactly when the link cannot be edited; the UI shows a Chinese
    /// explanation and leaves the annotation untouched.
    pub unsupported: Option<LinkUnsupportedReason>,
    /// Display-space boxes: one per `/QuadPoints` entry, or `/Rect`.
    pub rects: Vec<TextMarkupRect>,
}

/// Request for creating a link over selected text or an image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLinkRequest {
    pub page_index: u32,
    /// Display-space boxes. Text selections spanning several lines send one
    /// box per line; images send one box.
    pub rects: Vec<TextMarkupRect>,
    pub target: LinkTarget,
}

/// Request for changing one existing link's target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLinkTargetRequest {
    /// The positional id from [`LinkView`] (`"page:index"`).
    pub link_id: String,
    pub target: LinkTarget,
}

/// The outcome of creating, editing or deleting one link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkMutationView {
    /// Null after a deletion.
    pub link: Option<LinkView>,
    pub page_index: u32,
    pub history: EditHistoryView,
}

/// The complete state an undo/redo step restores for one link.
///
/// A link edit is implemented as remove-and-recreate because Pdfium cannot
/// mutate an action in place. The snapshot therefore keeps the standard
/// annotation metadata the recreation must restore, not only the target.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinkSnapshot {
    pub(crate) page_index: u32,
    pub(crate) name: String,
    pub(crate) target: LinkTarget,
    pub(crate) rect: AnnotationRect,
    pub(crate) quads: Vec<[f32; 8]>,
    pub(crate) rects: Vec<TextMarkupRect>,
    pub(crate) author: Option<String>,
    pub(crate) contents: Option<String>,
    pub(crate) created_at: Option<String>,
}

impl LinkSnapshot {
    fn view(&self, id: String) -> LinkView {
        LinkView {
            id,
            name: Some(self.name.clone()),
            page_index: self.page_index,
            target: Some(self.target.clone()),
            unsupported: None,
            rects: self.rects.clone(),
        }
    }
}

enum TargetError {
    Unsupported(LinkUnsupportedReason),
    Failed(CommandError),
}

impl From<CommandError> for TargetError {
    fn from(error: CommandError) -> Self {
        Self::Failed(error)
    }
}

fn invalid_argument(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn internal_error(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::Internal, message)
}

fn link_id(page_index: u32, index: usize) -> String {
    format!("{page_index}:{index}")
}

fn placeholder_uri(page_index: u32) -> String {
    format!("{PLACEHOLDER_URI_PREFIX}{}", page_index + 1)
}

fn page_key(page_index: u32) -> String {
    format!("page:{page_index}")
}

fn parse_page_key(value: &str, page_count: usize) -> Option<u32> {
    let page = value.strip_prefix("page:")?.parse::<u32>().ok()?;
    (page as usize).lt(&page_count).then_some(page)
}

fn validate_page_index(page_index: u32, page_count: usize) -> Result<(), CommandError> {
    if (page_index as usize) < page_count {
        Ok(())
    } else {
        Err(CommandError::new(
            CommandErrorCode::PageOutOfRange,
            format!("page {page_index} is outside 0..{page_count}"),
        ))
    }
}

/// Validates and normalizes a link target against the document page count.
fn normalize_target(target: &LinkTarget, page_count: usize) -> Result<LinkTarget, CommandError> {
    match target {
        LinkTarget::Url { url } => Ok(LinkTarget::Url {
            url: normalize_url(url)?,
        }),
        LinkTarget::Page { page_index } => {
            validate_page_index(*page_index, page_count)?;
            Ok(LinkTarget::Page {
                page_index: *page_index,
            })
        }
    }
}

/// Validates an external link target and trims the surrounding whitespace.
///
/// Only the schemes the ticket accepts are allowed. A URL is stored text and
/// is never fetched or opened automatically.
fn normalize_url(url: &str) -> Result<String, CommandError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("the link URL must not be empty"));
    }
    if trimmed.len() > MAX_LINK_URL_LENGTH {
        return Err(invalid_argument(format!(
            "the link URL must not exceed {MAX_LINK_URL_LENGTH} characters"
        )));
    }
    if trimmed
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid_argument("the link URL must not contain whitespace"));
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("http://") {
        validate_web_url(trimmed, rest)?;
    } else if let Some(rest) = lower.strip_prefix("https://") {
        validate_web_url(trimmed, rest)?;
    } else if let Some(rest) = lower.strip_prefix("mailto:") {
        let address = &trimmed[7..];
        if rest.is_empty() || !address.contains('@') {
            return Err(invalid_argument(
                "a mailto: link needs an email address containing @",
            ));
        }
    } else {
        return Err(invalid_argument(
            "the link URL must start with http://, https:// or mailto:",
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_web_url(trimmed: &str, rest: &str) -> Result<(), CommandError> {
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.starts_with(':') {
        return Err(invalid_argument(format!(
            "the link URL {trimmed:?} has no host"
        )));
    }
    Ok(())
}

fn validate_rects(rects: &[TextMarkupRect]) -> Result<(), CommandError> {
    if rects.is_empty() {
        return Err(invalid_argument("a link needs at least one region"));
    }
    for rect in rects {
        let finite = rect.x.is_finite()
            && rect.y.is_finite()
            && rect.width.is_finite()
            && rect.height.is_finite();
        if !finite || rect.width < MIN_LINK_EXTENT || rect.height < MIN_LINK_EXTENT {
            return Err(invalid_argument(format!(
                "a link region needs finite geometry of at least {MIN_LINK_EXTENT} points"
            )));
        }
    }
    Ok(())
}

/// Resolves the editable target of a live link annotation.
///
/// PDFX's private page key wins over the action so a page link still resolves
/// before the first save, while the live Pdfium action is a placeholder URI.
fn resolve_link_target(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
) -> Result<LinkTarget, TargetError> {
    let page_count = document.pages().len().max(0) as usize;
    if let Some(key) =
        annotation_string_value(document, target, LINK_TARGET_KEY).map_err(map_adapter_error)?
    {
        if let Some(page_index) = parse_page_key(&key, page_count) {
            return Ok(LinkTarget::Page { page_index });
        }
    }

    let attributes = link_attributes(document, target)
        .map_err(map_adapter_error)?
        .ok_or(TargetError::Unsupported(LinkUnsupportedReason::Malformed))?;
    if let Some(url) = attributes.uri {
        return Ok(LinkTarget::Url { url });
    }
    if let Some(page_index) = attributes.destination_page {
        return Ok(LinkTarget::Page { page_index });
    }
    let reason = if attributes.action_unsupported {
        LinkUnsupportedReason::Action
    } else if attributes.destination_unresolved {
        LinkUnsupportedReason::Destination
    } else {
        LinkUnsupportedReason::Malformed
    };
    Err(TargetError::Unsupported(reason))
}

/// Reads one link annotation into the view the overlays consume.
fn read_link_view(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
    geometry: &DisplayGeometry,
) -> Result<LinkView, CommandError> {
    let page_index = target.page_index as u32;
    let id = link_id(page_index, target.index);
    let name = annotation_name(document, target)
        .map_err(map_adapter_error)?
        .filter(|name| !name.trim().is_empty());
    let rect = annotation_bounds(document, target).map_err(map_adapter_error)?;
    // Pdfium exposes `/QuadPoints` for any annotation type that has attachment
    // points; a plain rectangular link written by another tool has none.
    let quads = annotation_quads(document, target).unwrap_or_default();
    let rects = if quads.is_empty() {
        vec![display_rect(geometry, rect)]
    } else {
        display_rects(geometry, &quads)
    };
    let (resolved, unsupported) = match resolve_link_target(document, target) {
        Ok(target) => (Some(target), None),
        Err(TargetError::Unsupported(reason)) => (None, Some(reason)),
        Err(TargetError::Failed(error)) => return Err(error),
    };
    Ok(LinkView {
        id,
        name,
        page_index,
        target: resolved,
        unsupported,
        rects,
    })
}

/// Captures everything an undo step must restore for one link.
fn capture_link_snapshot(
    document: &PdfDocument<'static>,
    page_index: u32,
    target: AnnotationRef,
    geometry: &DisplayGeometry,
) -> Result<LinkSnapshot, CommandError> {
    let name = annotation_name(document, target)
        .map_err(map_adapter_error)?
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| internal_error("the link has no stable /NM name"))?;
    let target_kind = match resolve_link_target(document, target) {
        Ok(target) => target,
        Err(TargetError::Unsupported(_)) => {
            return Err(invalid_argument(
                "this link's action is not supported and cannot be edited",
            ))
        }
        Err(TargetError::Failed(error)) => return Err(error),
    };
    let rect = annotation_bounds(document, target).map_err(map_adapter_error)?;
    let quads = annotation_quads(document, target).unwrap_or_default();
    let rects = if quads.is_empty() {
        vec![display_rect(geometry, rect)]
    } else {
        display_rects(geometry, &quads)
    };
    let author = annotation_author(document, target).map_err(map_adapter_error)?;
    let contents = annotation_contents(document, target).map_err(map_adapter_error)?;
    let created_at = annotation_creation_date(document, target).map_err(map_adapter_error)?;
    Ok(LinkSnapshot {
        page_index,
        name,
        target: target_kind,
        rect,
        quads,
        rects,
        author,
        contents,
        created_at,
    })
}

/// Recreates a link exactly as a snapshot recorded it.
///
/// Pdfium cannot change an action or remove one, so every target change is a
/// removal followed by a creation. The stable name is set before the snapshot
/// is returned, which lets later actions find the link again.
pub(crate) fn restore_link_snapshot(
    document: &PdfDocument<'static>,
    snapshot: &LinkSnapshot,
) -> Result<(), CommandError> {
    if let Some(existing) = find_target(document, &snapshot.name)? {
        remove_annotation(document, existing).map_err(map_adapter_error)?;
    }
    let uri = match &snapshot.target {
        LinkTarget::Url { url } => url.clone(),
        LinkTarget::Page { page_index } => placeholder_uri(*page_index),
    };
    let target = create_link_annotation(document, snapshot.page_index as i32, &uri, snapshot.rect)
        .map_err(map_adapter_error)?;
    set_annotation_name(document, target, &snapshot.name).map_err(map_adapter_error)?;
    if let Some(author) = snapshot.author.as_deref() {
        set_annotation_author(document, target, author).map_err(map_adapter_error)?;
    }
    if let Some(contents) = snapshot.contents.as_deref() {
        set_annotation_contents(document, target, contents).map_err(map_adapter_error)?;
    }
    if let Some(created_at) = snapshot.created_at.as_deref() {
        set_annotation_creation_date(document, target, created_at).map_err(map_adapter_error)?;
    }
    if let LinkTarget::Page { page_index } = snapshot.target {
        set_annotation_string_value(document, target, LINK_TARGET_KEY, &page_key(page_index))
            .map_err(map_adapter_error)?;
    }
    if !snapshot.quads.is_empty() {
        append_link_quads(document, target, &snapshot.quads).map_err(map_adapter_error)?;
    }
    Ok(())
}

/// Removes the link named `name`; used when undo/redo crosses a creation or
/// deletion. Missing annotations are an internal inconsistency, matching the
/// annotation undo path.
pub(crate) fn remove_link_named(
    document: &PdfDocument<'static>,
    name: &str,
) -> Result<(), CommandError> {
    crate::annotations::remove_annotation_named(document, name)
}

/// Applies one side of a link action. `None` removes the link.
pub(crate) fn apply_link_snapshot(
    document: &PdfDocument<'static>,
    target: Option<&LinkSnapshot>,
    name: &str,
) -> Result<(), CommandError> {
    match target {
        Some(snapshot) => restore_link_snapshot(document, snapshot),
        None => remove_link_named(document, name),
    }
}

/// Parses the positional link id the overlay uses.
fn parse_link_id(link_id: &str) -> Result<(u32, usize), CommandError> {
    let invalid = || invalid_argument(format!("{link_id:?} is not a link id"));
    let (page, index) = link_id.split_once(':').ok_or_else(invalid)?;
    let page = page.parse::<u32>().map_err(|_| invalid())?;
    let index = index.parse::<usize>().map_err(|_| invalid())?;
    Ok((page, index))
}

/// Resolves a link id to a live annotation, refusing an id whose position no
/// longer holds a link (the list must be reloaded).
fn locate_link(
    document: &PdfDocument<'static>,
    link_id: &str,
) -> Result<(u32, AnnotationRef), CommandError> {
    let (page_index, index) = parse_link_id(link_id)?;
    let page_count = document.pages().len().max(0) as usize;
    validate_page_index(page_index, page_count)?;
    let target = AnnotationRef::new(page_index as i32, index);
    let kind = annotation_type(document, target)
        .map_err(|_| invalid_argument(format!("the link {link_id:?} no longer exists")))?;
    if kind != PdfPageAnnotationType::Link {
        return Err(invalid_argument(format!(
            "the link {link_id:?} no longer exists"
        )));
    }
    Ok((page_index, target))
}

/// Returns the link's `/NM`, assigning one if the link was written by another
/// tool without it.
///
/// Link edits are remove-and-recreate, so the name is the identity that lets
/// undo/redo and repeated saves find exactly this annotation. Assigning it is
/// the first mutation of an edit; unsupported links are refused before this
/// point so they stay untouched.
fn ensure_link_name(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
) -> Result<String, CommandError> {
    if let Some(name) = annotation_name(document, target)
        .map_err(map_adapter_error)?
        .filter(|name| !name.trim().is_empty())
    {
        return Ok(name);
    }
    let name = next_annotation_name();
    set_annotation_name(document, target, &name).map_err(map_adapter_error)?;
    Ok(name)
}

/// Refuses to edit or delete a link whose action PDFX cannot represent.
///
/// The annotation is never named, moved, recreated or removed before this
/// check, so a foreign JavaScript, Launch or unresolved link is left exactly as
/// it was written.
fn ensure_link_supported(
    document: &PdfDocument<'static>,
    target: AnnotationRef,
) -> Result<LinkTarget, CommandError> {
    match resolve_link_target(document, target) {
        Ok(target) => Ok(target),
        Err(TargetError::Unsupported(_)) => Err(CommandError::new(
            CommandErrorCode::UnsupportedLink,
            "this link's target type is not supported; the link was left untouched",
        )),
        Err(TargetError::Failed(error)) => Err(error),
    }
}

/// Lists every `/Link` annotation in page order.
#[tauri::command]
pub fn list_links(document_id: String) -> Result<Vec<LinkView>, CommandError> {
    with_open_document(&document_id, |entry| {
        let document = &entry.document;
        let page_count = document.pages().len();
        let mut views = Vec::new();
        for page_index in 0..page_count {
            let count = document
                .pages()
                .get(page_index)
                .map_err(|error| {
                    internal_error(format!("could not load page {page_index}: {error:?}"))
                })?
                .annotations()
                .len();
            if count == 0 {
                continue;
            }
            let geometry = page_geometry(document, page_index as u32)?;
            for index in 0..count {
                let target = AnnotationRef::new(page_index, index);
                let Ok(annotation_kind) = annotation_type(document, target) else {
                    continue;
                };
                if annotation_kind != PdfPageAnnotationType::Link {
                    continue;
                }
                let mut view = read_link_view(document, target, &geometry)?;
                view.id = link_id(page_index as u32, index);
                views.push(view);
            }
        }
        Ok(views)
    })
}

/// Creates a link over the selected display-space regions.
#[tauri::command]
pub fn create_link(
    document_id: String,
    request: CreateLinkRequest,
) -> Result<LinkMutationView, CommandError> {
    validate_rects(&request.rects)?;
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let page_count = entry.document.pages().len().max(0) as usize;
        validate_page_index(request.page_index, page_count)?;
        let target = normalize_target(&request.target, page_count)?;
        let geometry = page_geometry(&entry.document, request.page_index)?;
        let quads: Vec<[f32; 8]> = request
            .rects
            .iter()
            .map(|rect| quad_for_rect(&geometry, *rect))
            .collect();
        let bounds = bounds_of_quads(&quads);
        let name = next_annotation_name();
        let uri = match &target {
            LinkTarget::Url { url } => url.clone(),
            LinkTarget::Page { page_index } => placeholder_uri(*page_index),
        };
        let created =
            create_link_annotation(&entry.document, request.page_index as i32, &uri, bounds)
                .map_err(map_adapter_error)?;
        set_annotation_name(&entry.document, created, &name).map_err(map_adapter_error)?;
        if let LinkTarget::Page { page_index } = target {
            set_annotation_string_value(
                &entry.document,
                created,
                LINK_TARGET_KEY,
                &page_key(page_index),
            )
            .map_err(map_adapter_error)?;
        }
        append_link_quads(&entry.document, created, &quads).map_err(map_adapter_error)?;

        let snapshot =
            capture_link_snapshot(&entry.document, request.page_index, created, &geometry)?;
        let name = snapshot.name.clone();
        let view = snapshot.view(link_id(request.page_index, created.index));
        let history = push_link_action(entry, request.page_index, name, None, Some(snapshot));
        Ok(LinkMutationView {
            link: Some(view),
            page_index: request.page_index,
            history,
        })
    })
}

/// Changes an existing link's target, preserving its region and metadata.
///
/// Pdfium cannot rewrite an action in place, so the link is recreated with the
/// new target under the same `/NM`. Both snapshots go into the unified history,
/// which makes the edit undoable and keeps repeat saves idempotent.
#[tauri::command]
pub fn update_link_target(
    document_id: String,
    request: UpdateLinkTargetRequest,
) -> Result<LinkMutationView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let page_count = entry.document.pages().len().max(0) as usize;
        let target = normalize_target(&request.target, page_count)?;
        let (page_index, found) = locate_link(&entry.document, &request.link_id)?;
        ensure_link_supported(&entry.document, found)?;

        let name = ensure_link_name(&entry.document, found)?;
        let geometry = page_geometry(&entry.document, page_index)?;
        let before = capture_link_snapshot(&entry.document, page_index, found, &geometry)?;
        let mut after = before.clone();
        after.target = target;
        restore_link_snapshot(&entry.document, &after)?;

        let restored = find_target(&entry.document, &name)?
            .ok_or_else(|| internal_error("the edited link disappeared"))?;
        let view = after.view(link_id(page_index, restored.index));
        let history = push_link_action(entry, page_index, name, Some(before), Some(after));
        Ok(LinkMutationView {
            link: Some(view),
            page_index,
            history,
        })
    })
}

/// Deletes one link and records the deletion in the unified history.
#[tauri::command]
pub fn delete_link(document_id: String, link_id: String) -> Result<LinkMutationView, CommandError> {
    with_open_document(&document_id, |entry| {
        ensure_annotation_editable(&entry.document)?;
        let (page_index, found) = locate_link(&entry.document, &link_id)?;
        ensure_link_supported(&entry.document, found)?;

        let name = ensure_link_name(&entry.document, found)?;
        let geometry = page_geometry(&entry.document, page_index)?;
        let before = capture_link_snapshot(&entry.document, page_index, found, &geometry)?;
        remove_link_named(&entry.document, &name)?;
        let history = push_link_action(entry, page_index, name, Some(before), None);
        Ok(LinkMutationView {
            link: None,
            page_index,
            history,
        })
    })
}
