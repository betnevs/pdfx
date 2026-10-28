//! Creating new AcroForm text fields.
//!
//! PDFium cannot create form structures, so a text field created in this
//! session lives in the edit session until the next save. The store below
//! backs discovery, filling and the required-field gate immediately; the save
//! pipeline materializes the AcroForm, widgets, appearances and the embedded
//! CJK font in its single lopdf revision.
//!
//! Every change (creation, property update, deletion, value) is one
//! [`crate::text_edit::FormStructureEditAction`] carrying a full snapshot of
//! the record, so undo/redo restores the field list, geometry, properties,
//! value and therefore the widget appearance together.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::form::{FormFieldKind, FormFieldRect, FormFieldView};
use crate::text_edit::{push_form_structure_action, EditHistoryView, EditSession};

/// Field ids minted for created fields. Existing widgets use
/// `"<page-index>:<annotation-index>"`.
pub(crate) const CREATED_ID_PREFIX: &str = "new:";
const DEFAULT_FONT_SIZE: f32 = 12.0;
const MIN_FONT_SIZE: f32 = 4.0;
const MAX_FONT_SIZE: f32 = 200.0;
/// The smallest widget the form tool lets the user draw, in page points.
const MIN_WIDGET_SIZE: f32 = 4.0;
const DEFAULT_NAME_PREFIX: &str = "text_field_";

/// One text field created by this session and not yet flattened into the
/// PDFium document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreatedTextField {
    pub(crate) uid: u64,
    pub(crate) field_id: String,
    pub(crate) page_index: u32,
    pub(crate) name: String,
    pub(crate) tooltip: Option<String>,
    pub(crate) required: bool,
    pub(crate) multiline: bool,
    pub(crate) font_size: f32,
    /// Display-space rectangle the user drew (top-left origin).
    pub(crate) rect: FormFieldRect,
    /// The same rectangle in unrotated PDF user space, `[left bottom right top]`.
    pub(crate) user_rect: [f32; 4],
    /// The page's `/Rotate` when the widget was placed. The save pipeline uses
    /// it to compensate the appearance stream so text stays upright.
    pub(crate) rotation: u16,
    /// Current text value; empty until the user fills the field.
    pub(crate) value: String,
}

/// Undo/redo snapshot of a created field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreatedFieldSnapshot {
    pub(crate) field: CreatedTextField,
}

/// Session store for created fields, ordered by creation.
#[derive(Debug, Default, Clone)]
pub(crate) struct CreatedFieldStore {
    pub(crate) fields: Vec<CreatedTextField>,
    /// Monotonic; undo never reuses an id, so redo restores the same one.
    pub(crate) next_uid: u64,
}

impl CreatedFieldStore {
    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub(crate) fn find(&self, field_id: &str) -> Option<&CreatedTextField> {
        self.fields.iter().find(|field| field.field_id == field_id)
    }

    pub(crate) fn find_by_uid(&self, uid: u64) -> Option<&CreatedTextField> {
        self.fields.iter().find(|field| field.uid == uid)
    }

    /// Inserts a record, or replaces the record with the same uid. Used by
    /// redo and by every snapshot restore.
    pub(crate) fn restore(&mut self, field: CreatedTextField) {
        match self
            .fields
            .iter()
            .position(|candidate| candidate.uid == field.uid)
        {
            Some(index) => self.fields[index] = field,
            None => {
                let index = self
                    .fields
                    .partition_point(|candidate| candidate.uid < field.uid);
                self.fields.insert(index, field);
            }
        }
    }

    pub(crate) fn remove(&mut self, uid: u64) -> Option<CreatedTextField> {
        let index = self.fields.iter().position(|field| field.uid == uid)?;
        Some(self.fields.remove(index))
    }

    /// A name not used by any existing widget or created field.
    pub(crate) fn generated_name(&self, used: &HashSet<String>) -> String {
        for suffix in 1.. {
            let candidate = format!("{DEFAULT_NAME_PREFIX}{suffix}");
            if !used.contains(&candidate)
                && !self.fields.iter().any(|field| field.name == candidate)
            {
                return candidate;
            }
        }
        unreachable!("u64 suffix space exhausted")
    }
}

/// The property panel's creation request. `name: None` asks for a generated
/// `text_field_N` name; the panel always sends the other values explicitly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFormTextFieldRequest {
    pub rect: FormFieldRect,
    pub name: Option<String>,
    pub tooltip: Option<String>,
    pub required: bool,
    pub multiline: bool,
    pub font_size: Option<f32>,
}

/// The outcome of placing a field: the new id, the refreshed field list (for
/// the overlay and tab order) and the unified history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFormTextFieldResult {
    pub field_id: String,
    pub fields: Vec<FormFieldView>,
    pub history: EditHistoryView,
}

/// The full desired state of a created field, sent by the property panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFormTextFieldRequest {
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub multiline: bool,
    pub font_size: f32,
}

/// The outcome of a property update or a deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormStructureEditResult {
    pub fields: Vec<FormFieldView>,
    pub history: EditHistoryView,
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn invalid_name(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::FormFieldNameInvalid, message)
}

fn duplicate_name(name: &str) -> CommandError {
    CommandError::new(
        CommandErrorCode::FormFieldNameDuplicate,
        format!("表单字段名称“{name}”已存在，请换一个名称。"),
    )
}

/// Creating fields changes document content, so the content-modification
/// permission applies (not the form-filling permission). The message is the
/// Chinese reason the UI shows.
fn ensure_creatable(entry: &OpenDocument) -> Result<(), CommandError> {
    crate::text_edit::ensure_content_editable(&entry.document).map_err(|_| {
        CommandError::new(
            CommandErrorCode::PermissionDenied,
            "此文档的权限限制不允许创建表单字段。",
        )
    })
}

fn validate_rect(rect: FormFieldRect, width: f32, height: f32) -> Result<(), CommandError> {
    let values = [rect.x, rect.y, rect.width, rect.height];
    if !values.iter().all(|value| value.is_finite()) || rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(invalid("表单字段的位置和大小必须是有效数字。"));
    }
    if rect.width < MIN_WIDGET_SIZE || rect.height < MIN_WIDGET_SIZE {
        return Err(invalid(format!(
            "表单字段至少需要 {MIN_WIDGET_SIZE}×{MIN_WIDGET_SIZE} 点。"
        )));
    }
    if rect.x < 0.0
        || rect.y < 0.0
        || rect.x + rect.width > width + 0.01
        || rect.y + rect.height > height + 0.01
    {
        return Err(invalid("表单字段必须完整位于页面范围内。"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), CommandError> {
    if name.is_empty() {
        return Err(invalid_name("表单字段名称不能为空。"));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid_name("表单字段名称不能包含控制字符。"));
    }
    if name.chars().count() > 128 {
        return Err(invalid_name("表单字段名称不能超过 128 个字符。"));
    }
    Ok(())
}

fn normalize_tooltip(tooltip: Option<String>) -> Option<String> {
    tooltip
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_font_size(font_size: Option<f32>) -> Result<f32, CommandError> {
    match font_size {
        None => Ok(DEFAULT_FONT_SIZE),
        Some(size) if size.is_finite() && (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) => {
            Ok(size)
        }
        Some(_) => Err(invalid(format!(
            "字号必须在 {MIN_FONT_SIZE} 到 {MAX_FONT_SIZE} 点之间。"
        ))),
    }
}

impl CreatedTextField {
    pub(crate) fn view(&self, order: u32) -> FormFieldView {
        FormFieldView {
            field_id: self.field_id.clone(),
            name: self.name.clone(),
            kind: FormFieldKind::Text,
            page_index: self.page_index,
            order,
            rect: self.rect,
            value: (!self.value.is_empty()).then(|| self.value.clone()),
            checked: false,
            options: Vec::new(),
            selected_index: None,
            required: self.required,
            read_only: false,
            multiline: self.multiline,
            radio_group: None,
            export_value: None,
            tooltip: self.tooltip.clone(),
            font_size: Some(self.font_size),
            created: true,
        }
    }

    pub(crate) fn snapshot(&self) -> CreatedFieldSnapshot {
        CreatedFieldSnapshot {
            field: self.clone(),
        }
    }
}

/// Merges created fields into PDFium's widget list, page by page, with created
/// fields after the existing widgets of their page. `order` is renumbered to
/// stay a contiguous document-wide tab order.
pub(crate) fn merge_views(
    existing: Vec<FormFieldView>,
    store: &CreatedFieldStore,
) -> Vec<FormFieldView> {
    if store.is_empty() {
        return existing;
    }

    let mut entries: Vec<(u32, u8, u64, FormFieldView)> = existing
        .into_iter()
        .map(|view| (view.page_index, 0, u64::from(view.order), view))
        .collect();
    entries.extend(
        store
            .fields
            .iter()
            .map(|field| (field.page_index, 1, field.uid, field.view(0))),
    );
    entries.sort_by_key(|(page_index, created, tie, _)| (*page_index, *created, *tie));
    entries
        .into_iter()
        .enumerate()
        .map(|(order, (_, _, _, mut view))| {
            view.order = order as u32;
            view
        })
        .collect()
}

/// Parses a created-field id back to its session uid.
pub(crate) fn created_uid(field_id: &str) -> Option<u64> {
    field_id
        .strip_prefix(CREATED_ID_PREFIX)
        .and_then(|uid| uid.parse().ok())
}

fn not_created(field_id: &str) -> CommandError {
    CommandError::new(
        CommandErrorCode::FormFieldNotFound,
        format!("只能修改或删除本次会话创建的表单字段（{field_id}）。"),
    )
}

fn create_for(
    entry: &mut OpenDocument,
    page_index: u32,
    request: CreateFormTextFieldRequest,
) -> Result<CreateFormTextFieldResult, CommandError> {
    ensure_creatable(entry)?;

    let geometry = {
        let page = load_page(&entry.document, page_index)?;
        let rotation = page
            .rotation()
            .map_err(|error| {
                CommandError::new(
                    CommandErrorCode::RenderFailed,
                    format!("could not read the rotation of page {page_index}: {error:?}"),
                )
            })?
            .as_degrees() as u16;
        DisplayGeometry::new(&page, rotation)?
    };
    let (display_width, display_height) = geometry.display_size();
    validate_rect(request.rect, display_width, display_height)?;
    let font_size = normalize_font_size(request.font_size)?;
    let tooltip = normalize_tooltip(request.tooltip);

    let existing = crate::form::discover_all(entry)?;
    let used: HashSet<String> = existing.iter().map(|field| field.name.clone()).collect();
    let name = match request.name {
        Some(raw) => {
            let name = raw.trim().to_owned();
            validate_name(&name)?;
            if used.contains(&name) {
                return Err(duplicate_name(&name));
            }
            name
        }
        None => entry.edit.created_fields.generated_name(&used),
    };

    let (x0, y0) = geometry.point_inverse(request.rect.x, request.rect.y);
    let (x1, y1) = geometry.point_inverse(
        request.rect.x + request.rect.width,
        request.rect.y + request.rect.height,
    );
    let user_rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];

    let uid = {
        let store = &mut entry.edit.created_fields;
        store.next_uid += 1;
        store.next_uid
    };
    let field_id = format!("{CREATED_ID_PREFIX}{uid}");
    let field = CreatedTextField {
        uid,
        field_id: field_id.clone(),
        page_index,
        name,
        tooltip,
        required: request.required,
        multiline: request.multiline,
        font_size,
        rect: request.rect,
        user_rect,
        rotation: geometry.rotation(),
        value: String::new(),
    };
    let snapshot = field.snapshot();
    entry.edit.created_fields.restore(field);
    let history = push_form_structure_action(entry, uid, page_index, None, Some(snapshot));

    Ok(CreateFormTextFieldResult {
        field_id,
        fields: crate::form::discover_all(entry)?,
        history,
    })
}

/// The `create_form_text_field` command. Places a text widget on a page and
/// records the creation as one undoable action.
#[tauri::command]
pub fn create_form_text_field(
    document_id: String,
    page_index: u32,
    request: CreateFormTextFieldRequest,
) -> Result<CreateFormTextFieldResult, CommandError> {
    with_open_document(&document_id, |entry| create_for(entry, page_index, request))
}

fn update_for(
    entry: &mut OpenDocument,
    field_id: &str,
    request: UpdateFormTextFieldRequest,
) -> Result<FormStructureEditResult, CommandError> {
    ensure_creatable(entry)?;
    let current = entry
        .edit
        .created_fields
        .find(field_id)
        .cloned()
        .ok_or_else(|| not_created(field_id))?;

    let name = request.name.trim().to_owned();
    validate_name(&name)?;
    if name != current.name
        && crate::form::discover_all(entry)?
            .iter()
            .any(|field| field.field_id != field_id && field.name == name)
    {
        return Err(duplicate_name(&name));
    }
    let font_size = normalize_font_size(Some(request.font_size))?;

    let mut updated = current.clone();
    updated.name = name;
    updated.tooltip = normalize_tooltip(request.tooltip);
    updated.required = request.required;
    updated.multiline = request.multiline;
    updated.font_size = font_size;

    let history = if updated == current {
        entry.edit.history_view()
    } else {
        let before = current.snapshot();
        let after = updated.snapshot();
        entry.edit.created_fields.restore(updated.clone());
        push_form_structure_action(
            entry,
            updated.uid,
            updated.page_index,
            Some(before),
            Some(after),
        )
    };

    Ok(FormStructureEditResult {
        fields: crate::form::discover_all(entry)?,
        history,
    })
}

/// The `update_form_text_field` command. Changes a created field's properties
/// and records the previous state for undo.
#[tauri::command]
pub fn update_form_text_field(
    document_id: String,
    field_id: String,
    request: UpdateFormTextFieldRequest,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| update_for(entry, &field_id, request))
}

fn delete_for(
    entry: &mut OpenDocument,
    field_id: &str,
) -> Result<FormStructureEditResult, CommandError> {
    ensure_creatable(entry)?;
    let current = entry
        .edit
        .created_fields
        .find(field_id)
        .cloned()
        .ok_or_else(|| not_created(field_id))?;
    entry.edit.created_fields.remove(current.uid);
    let history = push_form_structure_action(
        entry,
        current.uid,
        current.page_index,
        Some(current.snapshot()),
        None,
    );

    Ok(FormStructureEditResult {
        fields: crate::form::discover_all(entry)?,
        history,
    })
}

/// The `delete_form_text_field` command. Deletes a field created in this
/// session; the removal is undone by the shared history.
#[tauri::command]
pub fn delete_form_text_field(
    document_id: String,
    field_id: String,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| delete_for(entry, &field_id))
}

// ---------------------------------------------------------------------------
// Save pipeline: materialize created fields as AcroForm objects.
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;

use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream};

use crate::fonts::FontSubset;

/// One created field as the save pipeline writes it. Only plain values are
/// carried; the appearance is generated from them.
#[derive(Debug, Clone, PartialEq)]
pub struct TextFieldSpec {
    pub page_index: u32,
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub multiline: bool,
    pub font_size: f32,
    /// Unrotated PDF user-space rectangle, `[left bottom right top]`.
    pub user_rect: [f32; 4],
    /// Display-space size the appearance is laid out in.
    pub width: f32,
    pub height: f32,
    pub value: String,
    /// Page `/Rotate` when the widget was placed.
    pub rotation: u16,
}

impl From<&CreatedTextField> for TextFieldSpec {
    fn from(field: &CreatedTextField) -> Self {
        Self {
            page_index: field.page_index,
            name: field.name.clone(),
            tooltip: field.tooltip.clone(),
            required: field.required,
            multiline: field.multiline,
            font_size: field.font_size,
            user_rect: field.user_rect,
            width: field.rect.width,
            height: field.rect.height,
            value: field.value.clone(),
            rotation: field.rotation,
        }
    }
}

/// Object-level edits for the fields this session created. One edit carries
/// every field, so the save pipeline stays at one writer per revision.
pub(crate) fn save_object_edits(edit: &EditSession) -> Vec<crate::save::ObjectEdit> {
    let fields: Vec<TextFieldSpec> = edit
        .created_fields
        .fields
        .iter()
        .map(TextFieldSpec::from)
        .collect();
    if fields.is_empty() {
        Vec::new()
    } else {
        vec![crate::save::ObjectEdit::CreateTextFields { fields }]
    }
}

fn pdf_error(context: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::SaveFailed, context.into())
}

fn dictionary<const N: usize>(entries: [(&'static str, Object); N]) -> Dictionary {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        dictionary.set(key.as_bytes().to_vec(), value);
    }
    dictionary
}

fn name(value: &str) -> Object {
    Object::Name(value.as_bytes().to_vec())
}

fn text_string(value: &str) -> Object {
    if value.is_ascii() {
        Object::string_literal(value.as_bytes().to_vec())
    } else {
        // PDF text strings outside ASCII must be UTF-16BE with a BOM.
        let mut bytes = vec![0xFE, 0xFF];
        for unit in value.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Literal)
    }
}

fn number(value: f32) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

fn normalized_rect(rect: [f32; 4]) -> [f32; 4] {
    let [x0, y0, x1, y1] = rect;
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

fn rect_array(rect: [f32; 4]) -> Object {
    let [x0, y0, x1, y1] = normalized_rect(rect);
    Object::Array(vec![
        Object::Real(x0),
        Object::Real(y0),
        Object::Real(x1),
        Object::Real(y1),
    ])
}

fn resolved<'a>(
    document: &'a Document,
    object: &'a Object,
    context: &str,
) -> Result<&'a Object, CommandError> {
    match object {
        Object::Reference(id) => document
            .get_object(*id)
            .map_err(|error| pdf_error(format!("{context}: {error}"))),
        other => Ok(other),
    }
}

fn array_entry(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    context: &str,
) -> Result<Vec<Object>, CommandError> {
    match dictionary.get(key).ok() {
        None => Ok(Vec::new()),
        Some(object) => resolved(document, object, context).and_then(|object| {
            object
                .as_array()
                .cloned()
                .map_err(|error| pdf_error(format!("{context}: {error}")))
        }),
    }
}

fn ensure_acroform(
    incremental: &mut IncrementalDocument,
    root_id: ObjectId,
) -> Result<ObjectId, CommandError> {
    let existing = incremental
        .new_document
        .get_object(root_id)
        .and_then(Object::as_dict)
        .map_err(|error| pdf_error(format!("the saved catalog is not a dictionary: {error}")))?
        .get(b"AcroForm")
        .ok()
        .cloned();

    match existing {
        None => {
            let catalog = incremental
                .new_document
                .get_object_mut(root_id)
                .and_then(Object::as_dict_mut)
                .map_err(|error| {
                    pdf_error(format!("the saved catalog is not a dictionary: {error}"))
                })?;
            catalog.set(b"AcroForm".to_vec(), Object::Dictionary(Dictionary::new()));
            Ok(root_id)
        }
        Some(Object::Reference(id)) => {
            incremental
                .opt_clone_object_to_new_document(id)
                .map_err(|error| pdf_error(format!("could not clone /AcroForm: {error}")))?;
            Ok(id)
        }
        Some(Object::Dictionary(_)) => Ok(root_id),
        Some(_) => Err(pdf_error("the saved /AcroForm is not a dictionary")),
    }
}

fn acroform_dict(
    document: &Document,
    acroform_id: ObjectId,
    root_id: ObjectId,
) -> Result<&Dictionary, CommandError> {
    if acroform_id == root_id {
        document
            .get_object(root_id)
            .and_then(Object::as_dict)
            .map_err(|error| pdf_error(format!("the saved catalog is not a dictionary: {error}")))?
            .get(b"AcroForm")
            .and_then(Object::as_dict)
            .map_err(|error| pdf_error(format!("the saved /AcroForm is not a dictionary: {error}")))
    } else {
        document
            .get_object(acroform_id)
            .and_then(Object::as_dict)
            .map_err(|error| pdf_error(format!("the saved /AcroForm is not a dictionary: {error}")))
    }
}

fn acroform_dict_mut(
    document: &mut Document,
    acroform_id: ObjectId,
    root_id: ObjectId,
) -> Result<&mut Dictionary, CommandError> {
    if acroform_id == root_id {
        document
            .get_object_mut(root_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| pdf_error(format!("the saved catalog is not a dictionary: {error}")))?
            .get_mut(b"AcroForm")
            .and_then(Object::as_dict_mut)
            .map_err(|error| pdf_error(format!("the saved /AcroForm is not a dictionary: {error}")))
    } else {
        document
            .get_object_mut(acroform_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| pdf_error(format!("the saved /AcroForm is not a dictionary: {error}")))
    }
}

fn read_dr(
    document: &Document,
    acroform_id: ObjectId,
    root_id: ObjectId,
) -> Result<(Dictionary, Dictionary), CommandError> {
    let acroform = acroform_dict(document, acroform_id, root_id)?;
    let mut dr = match acroform.get(b"DR").ok() {
        Some(object) => resolved(document, object, "the saved /AcroForm /DR")?
            .as_dict()
            .map_err(|error| {
                pdf_error(format!(
                    "the saved /AcroForm /DR is not a dictionary: {error}"
                ))
            })?
            .clone(),
        None => Dictionary::new(),
    };
    let fonts = match dr.get(b"Font").ok() {
        Some(object) => resolved(document, object, "the saved /AcroForm /DR /Font")?
            .as_dict()
            .map_err(|error| {
                pdf_error(format!("the saved /DR /Font is not a dictionary: {error}"))
            })?
            .clone(),
        None => Dictionary::new(),
    };
    dr.remove(b"Font");
    Ok((dr, fonts))
}

fn unique_font_name(fonts: &Dictionary) -> String {
    let base = "PdfxNoto";
    if !fonts.has(b"PdfxNoto") {
        return base.to_owned();
    }
    for suffix in 2.. {
        let candidate = format!("{base}{suffix}");
        if !fonts.has(candidate.as_bytes()) {
            return candidate;
        }
    }
    unreachable!("font resource suffix space exhausted")
}

/// Metrics of the embedded subset, scaled to the 1000-unit PDF glyph space.
struct FontMetrics {
    widths: Vec<i64>,
    ascent: i64,
    descent: i64,
    cap_height: i64,
    bbox: [i64; 4],
}

impl FontMetrics {
    fn new(subset: &FontSubset) -> Result<Self, CommandError> {
        let face = ttf_parser::Face::parse(&subset.bytes, 0).map_err(|error| {
            pdf_error(format!("could not parse the embedded font subset: {error}"))
        })?;
        let units = f64::from(face.units_per_em());
        let scale = |value: f64| (value * 1000.0 / units).round() as i64;
        let bbox = face.global_bounding_box();
        let widths = subset
            .cid_to_gid
            .iter()
            .enumerate()
            .map(|(cid, gid)| {
                if cid == 0 {
                    return 0;
                }
                face.glyph_hor_advance(ttf_parser::GlyphId(*gid))
                    .map(|advance| (f64::from(advance) * 1000.0 / units).round() as i64)
                    .unwrap_or(1000)
            })
            .collect();
        let ascent = scale(f64::from(face.ascender()));
        Ok(Self {
            widths,
            ascent,
            descent: scale(f64::from(face.descender())),
            cap_height: face
                .capital_height()
                .map(|height| scale(f64::from(height)))
                .unwrap_or((ascent * 7 / 10).max(0)),
            bbox: [
                scale(f64::from(bbox.x_min)),
                scale(f64::from(bbox.y_min)),
                scale(f64::from(bbox.x_max)),
                scale(f64::from(bbox.y_max)),
            ],
        })
    }

    fn width(&self, cid: u16) -> f32 {
        self.widths.get(usize::from(cid)).copied().unwrap_or(1000) as f32 / 1000.0
    }
}

fn character_cids(subset: &FontSubset) -> BTreeMap<char, u16> {
    let mut cids = BTreeMap::new();
    for (cid, character) in &subset.unicode_by_cid {
        cids.entry(*character).or_insert(*cid);
    }
    cids
}

fn write_type0_font(
    document: &mut Document,
    subset: &FontSubset,
    font_name: &str,
    metrics: &FontMetrics,
) -> Result<ObjectId, CommandError> {
    let font_program = Stream::new(
        dictionary([("Length1", Object::Integer(subset.bytes.len() as i64))]),
        subset.bytes.clone(),
    )
    .with_compression(false);
    let font_program_id = document
        .try_add_object(font_program)
        .map_err(|error| pdf_error(format!("could not add the embedded font program: {error}")))?;

    let descriptor = dictionary([
        ("Type", name("FontDescriptor")),
        ("FontName", Object::Name(font_name.as_bytes().to_vec())),
        ("Flags", Object::Integer(4)),
        (
            "FontBBox",
            Object::Array(
                metrics
                    .bbox
                    .iter()
                    .map(|value| Object::Integer(*value))
                    .collect(),
            ),
        ),
        ("ItalicAngle", Object::Integer(0)),
        ("Ascent", Object::Integer(metrics.ascent)),
        ("Descent", Object::Integer(metrics.descent)),
        ("CapHeight", Object::Integer(metrics.cap_height)),
        ("StemV", Object::Integer(80)),
        ("FontFile2", Object::Reference(font_program_id)),
    ]);
    let descriptor_id = document
        .try_add_object(descriptor)
        .map_err(|error| pdf_error(format!("could not add the font descriptor: {error}")))?;

    let gid_map = Stream::new(Dictionary::new(), subset.cid_to_gid_map()).with_compression(false);
    let gid_map_id = document
        .try_add_object(gid_map)
        .map_err(|error| pdf_error(format!("could not add the CIDToGIDMap stream: {error}")))?;

    let to_unicode = Stream::new(Dictionary::new(), subset.to_unicode_cmap().into_bytes());
    let to_unicode_id = document
        .try_add_object(to_unicode)
        .map_err(|error| pdf_error(format!("could not add the ToUnicode CMap: {error}")))?;

    let widths: Vec<Object> = metrics
        .widths
        .iter()
        .skip(1)
        .map(|width| Object::Integer(*width))
        .collect();
    let cid_font = dictionary([
        ("Type", name("Font")),
        ("Subtype", name("CIDFontType2")),
        ("BaseFont", Object::Name(font_name.as_bytes().to_vec())),
        (
            "CIDSystemInfo",
            Object::Dictionary(dictionary([
                ("Registry", Object::string_literal(b"Adobe".to_vec())),
                ("Ordering", Object::string_literal(b"Identity".to_vec())),
                ("Supplement", Object::Integer(0)),
            ])),
        ),
        ("FontDescriptor", Object::Reference(descriptor_id)),
        ("DW", Object::Integer(1000)),
        (
            "W",
            Object::Array(vec![Object::Integer(1), Object::Array(widths)]),
        ),
        ("CIDToGIDMap", Object::Reference(gid_map_id)),
    ]);
    let cid_font_id = document
        .try_add_object(cid_font)
        .map_err(|error| pdf_error(format!("could not add the CID font: {error}")))?;

    let type0 = dictionary([
        ("Type", name("Font")),
        ("Subtype", name("Type0")),
        ("BaseFont", Object::Name(font_name.as_bytes().to_vec())),
        ("Encoding", name("Identity-H")),
        (
            "DescendantFonts",
            Object::Array(vec![Object::Reference(cid_font_id)]),
        ),
        ("ToUnicode", Object::Reference(to_unicode_id)),
    ]);
    document
        .try_add_object(type0)
        .map_err(|error| pdf_error(format!("could not add the Type0 font: {error}")))
}

/// Wraps the display-local appearance in the matrix that maps it into the
/// widget's user-space rectangle for the page's intrinsic rotation.
///
/// Content is drawn Y-up in display coordinates. `/BBox` is the same user-space
/// rectangle as `/Rect`, so a viewer's BBox-to-Rect mapping is the identity.
fn rotation_matrix(rotation: u16, rect: [f32; 4]) -> (f32, f32, f32, f32, f32, f32) {
    let [x0, y0, x1, y1] = normalized_rect(rect);
    match rotation % 360 {
        90 => (0.0, 1.0, -1.0, 0.0, x1, y0),
        180 => (-1.0, 0.0, 0.0, -1.0, x1, y1),
        270 => (0.0, -1.0, 1.0, 0.0, x0, y1),
        _ => (1.0, 0.0, 0.0, 1.0, x0, y0),
    }
}

fn line_width(text: &str, cids: &BTreeMap<char, u16>, metrics: &FontMetrics, size: f32) -> f32 {
    text.chars()
        .map(|character| {
            let cid = if character == '\t' {
                cids.get(&' ').copied()
            } else {
                cids.get(&character).copied()
            };
            cid.map(|cid| metrics.width(cid) * size)
                .unwrap_or(size * 0.5)
        })
        .sum()
}

/// Splits a value into appearance lines. Single-line fields collapse line
/// breaks; multiline fields wrap at the widget width.
fn layout_lines(
    value: &str,
    multiline: bool,
    available_width: f32,
    cids: &BTreeMap<char, u16>,
    metrics: &FontMetrics,
    size: f32,
) -> Vec<String> {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    if !multiline {
        return vec![normalized.replace('\n', " ")];
    }

    let mut lines = Vec::new();
    for paragraph in normalized.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut width = 0.0;
        for character in paragraph.chars() {
            let advance = line_width(&character.to_string(), cids, metrics, size);
            if width + advance > available_width && !current.is_empty() {
                lines.push(current);
                current = String::new();
                width = 0.0;
            }
            current.push(character);
            width += advance;
        }
        lines.push(current);
    }
    lines
}

fn hex_text(text: &str, cids: &BTreeMap<char, u16>) -> String {
    let space = cids.get(&' ').copied();
    let mut hex = String::new();
    for character in text.chars() {
        let cid = if character == '\t' {
            space
        } else {
            cids.get(&character).copied()
        };
        if let Some(cid) = cid {
            hex.push_str(&format!("{cid:04X}"));
        }
    }
    hex
}

fn appearance_content(
    spec: &TextFieldSpec,
    font_name: &str,
    subset: &FontSubset,
    metrics: &FontMetrics,
) -> String {
    let width = spec.width.max(0.01);
    let height = spec.height.max(0.01);
    let (a, b, c, d, e, f) = rotation_matrix(spec.rotation, spec.user_rect);

    let mut content = format!(
        "q {} {} {} {} {} {} cm\n\
         1 1 1 rg 0 0 {w} {h} re f\n\
         0 0 0 RG 0.75 w 0.5 0.5 {bw} {bh} re S\n",
        number(a),
        number(b),
        number(c),
        number(d),
        number(e),
        number(f),
        w = number(width),
        h = number(height),
        bw = number((width - 1.0).max(0.0)),
        bh = number((height - 1.0).max(0.0)),
    );

    if !spec.value.is_empty() {
        let cids = character_cids(subset);
        let lines = layout_lines(
            &spec.value,
            spec.multiline,
            (width - 4.0).max(spec.font_size),
            &cids,
            metrics,
            spec.font_size,
        );
        let first_baseline = if spec.multiline {
            height - 2.0 - spec.font_size * 0.8
        } else {
            (height - spec.font_size) / 2.0
        };
        content.push_str(&format!(
            "BT /{} {} Tf 0 g\n",
            font_name,
            number(spec.font_size)
        ));
        for (index, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            let baseline = first_baseline - index as f32 * spec.font_size * 1.2;
            content.push_str(&format!(
                "1 0 0 1 2 {} Tm <{}> Tj\n",
                number(baseline),
                hex_text(line, &cids)
            ));
        }
        content.push_str("ET\n");
    }
    content.push_str("Q\n");
    content
}

fn write_appearance(
    document: &mut Document,
    spec: &TextFieldSpec,
    font_name: &str,
    font_id: ObjectId,
    subset: &FontSubset,
    metrics: &FontMetrics,
) -> Result<ObjectId, CommandError> {
    let content = appearance_content(spec, font_name, subset, metrics);

    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
    resources.set(b"Font".to_vec(), Object::Dictionary(fonts));

    let stream = Stream::new(
        dictionary([
            ("Type", name("XObject")),
            ("Subtype", name("Form")),
            ("FormType", Object::Integer(1)),
            ("BBox", rect_array(spec.user_rect)),
            ("Resources", Object::Dictionary(resources)),
        ]),
        content.into_bytes(),
    );
    document
        .try_add_object(stream)
        .map_err(|error| pdf_error(format!("could not add the widget appearance: {error}")))
}

fn write_field(
    document: &mut Document,
    spec: &TextFieldSpec,
    page_id: ObjectId,
    appearance_id: ObjectId,
    font_name: &str,
) -> Result<ObjectId, CommandError> {
    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }
    if spec.multiline {
        flags |= 1 << 12; // Multiline
    }

    let mut appearance = Dictionary::new();
    appearance.set(b"N".to_vec(), Object::Reference(appearance_id));

    let mut field = dictionary([
        ("Type", name("Annot")),
        ("Subtype", name("Widget")),
        ("FT", name("Tx")),
        ("T", text_string(&spec.name)),
        ("Rect", rect_array(spec.user_rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        (
            "DA",
            text_string(&format!("/{font_name} {} Tf 0 g", number(spec.font_size))),
        ),
        ("Q", Object::Integer(0)),
        ("AP", Object::Dictionary(appearance)),
        (
            "Border",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(0),
            ]),
        ),
    ]);
    if let Some(tooltip) = &spec.tooltip {
        field.set(b"TU".to_vec(), text_string(tooltip));
    }
    if !spec.value.is_empty() {
        field.set(b"V".to_vec(), text_string(&spec.value));
    }
    document
        .try_add_object(field)
        .map_err(|error| pdf_error(format!("could not add the form field: {error}")))
}

/// Writes every created field into the in-progress incremental revision.
///
/// This is the only writer of AcroForm structures: the catalog, `/AcroForm`,
/// each target page and the font objects are cloned/added here, then `/Fields`,
/// `/DR`, `/DA` (when missing) and page `/Annots` are updated once. Existing
/// form entries, values and fonts are preserved, and `/NeedAppearances` is
/// never set.
pub(crate) fn apply_created_fields(
    incremental: &mut IncrementalDocument,
    fields: &[TextFieldSpec],
) -> Result<(), CommandError> {
    if fields.is_empty() {
        return Ok(());
    }

    let (root_id, page_ids) = {
        let previous = incremental.get_prev_documents();
        let root_id = previous
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference)
            .map_err(|_| pdf_error("the saved document has no /Root reference"))?;
        let pages = previous.get_pages();
        let page_ids = fields
            .iter()
            .map(|spec| {
                pages.get(&(spec.page_index + 1)).copied().ok_or_else(|| {
                    pdf_error(format!(
                        "page {} does not exist in the saved document",
                        spec.page_index + 1
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        (root_id, page_ids)
    };

    incremental
        .opt_clone_object_to_new_document(root_id)
        .map_err(|error| pdf_error(format!("could not clone the catalog: {error}")))?;
    let acroform_id = ensure_acroform(incremental, root_id)?;

    let mut unique_pages = page_ids.clone();
    unique_pages.sort_unstable();
    unique_pages.dedup();
    for page_id in &unique_pages {
        incremental
            .opt_clone_object_to_new_document(*page_id)
            .map_err(|error| pdf_error(format!("could not clone a target page: {error}")))?;
    }

    let existing_fields = {
        let document = &incremental.new_document;
        let acroform = acroform_dict(document, acroform_id, root_id)?;
        array_entry(document, acroform, b"Fields", "the saved /AcroForm /Fields")?
    };
    let (mut dr, mut fonts) = read_dr(&incremental.new_document, acroform_id, root_id)?;
    let has_default_da = acroform_dict(&incremental.new_document, acroform_id, root_id)?.has(b"DA");

    // One subset covers every value; empty values still contribute the space
    // glyph, so the font and /DR entry are always valid.
    let value_text: String = fields
        .iter()
        .map(|spec| spec.value.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let subset =
        crate::fonts::noto_sans_sc().subset(&value_text, crate::fonts::weight_for(false))?;
    let metrics = FontMetrics::new(&subset)?;
    let font_name = unique_font_name(&fonts);
    let font_id = write_type0_font(&mut incremental.new_document, &subset, &font_name, &metrics)?;
    fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
    dr.set(b"Font".to_vec(), Object::Dictionary(fonts));

    let mut new_field_refs = Vec::with_capacity(fields.len());
    let mut page_widgets: BTreeMap<ObjectId, Vec<Object>> = BTreeMap::new();
    for (spec, page_id) in fields.iter().zip(page_ids.iter().copied()) {
        let appearance_id = write_appearance(
            &mut incremental.new_document,
            spec,
            &font_name,
            font_id,
            &subset,
            &metrics,
        )?;
        let field_id = write_field(
            &mut incremental.new_document,
            spec,
            page_id,
            appearance_id,
            &font_name,
        )?;
        new_field_refs.push(Object::Reference(field_id));
        page_widgets
            .entry(page_id)
            .or_default()
            .push(Object::Reference(field_id));
    }

    {
        let mut all_fields = existing_fields;
        all_fields.extend(new_field_refs);
        let acroform = acroform_dict_mut(&mut incremental.new_document, acroform_id, root_id)?;
        acroform.set(b"Fields".to_vec(), Object::Array(all_fields));
        acroform.set(b"DR".to_vec(), Object::Dictionary(dr));
        if !has_default_da {
            acroform.set(
                b"DA".to_vec(),
                text_string(&format!("/{font_name} 0 Tf 0 g")),
            );
        }
        // /NeedAppearances is deliberately never written: every widget carries
        // a static appearance and readers must not regenerate it.
    }

    for (page_id, widgets) in page_widgets {
        let mut annots = {
            let document = &incremental.new_document;
            let page = document
                .get_object(page_id)
                .and_then(Object::as_dict)
                .map_err(|error| {
                    pdf_error(format!("a target page is not a dictionary: {error}"))
                })?;
            array_entry(document, page, b"Annots", "the saved page /Annots")?
        };
        annots.extend(widgets);
        let page = incremental
            .new_document
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|error| pdf_error(format!("a target page is not a dictionary: {error}")))?;
        page.set(b"Annots".to_vec(), Object::Array(annots));
    }

    Ok(())
}
