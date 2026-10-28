//! Creating new AcroForm text fields, checkboxes and radio groups.
//!
//! PDFium cannot create form structures, so a field created in this session
//! lives in the edit session until the next save. The store below backs
//! discovery, filling and the required-field gate immediately; the save
//! pipeline materializes the AcroForm, widgets, appearances and — for text
//! fields — the embedded CJK font in its single lopdf revision.
//!
//! Every change (creation, property update, option edit, deletion, value) is
//! one [`crate::text_edit::FormStructureEditAction`] carrying a full snapshot
//! of the record, so undo/redo restores the field list, geometry, properties,
//! options, value and therefore the widget appearances together.
//!
//! Export values: a checkbox uses the standard `/Off` and `/Yes` states. A
//! radio group writes one kid widget per option; each kid's on-state is
//! derived from its option label (ASCII alphanumerics, `ChoiceN` when the label
//! has none, numeric suffixes for collisions), so the `/AP /N` state names,
//! the parent `/V` and every kid `/AS` always agree.

use std::collections::HashSet;

use pdfium_render::prelude::PdfDocument;
use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::form::{FormFieldKind, FormFieldRect, FormFieldView};
use crate::text_edit::{push_form_structure_action, EditHistoryView, EditSession};

/// Field ids minted for created fields. Existing widgets use
/// `"<page-index>:<annotation-index>"`. A radio group's base id is
/// `"new:<uid>"` and every option widget adds `":<option-index>"`.
pub(crate) const CREATED_ID_PREFIX: &str = "new:";
const DEFAULT_FONT_SIZE: f32 = 12.0;
const MIN_FONT_SIZE: f32 = 4.0;
const MAX_FONT_SIZE: f32 = 200.0;
/// The smallest widget the form tool lets the user draw, in page points.
const MIN_WIDGET_SIZE: f32 = 4.0;
const TEXT_NAME_PREFIX: &str = "text_field_";
const CHECKBOX_NAME_PREFIX: &str = "checkbox_";
const RADIO_NAME_PREFIX: &str = "radio_group_";
const COMBO_BOX_NAME_PREFIX: &str = "combo_box_";
const LIST_BOX_NAME_PREFIX: &str = "list_box_";
const SIGNATURE_NAME_PREFIX: &str = "signature_";
/// Vertical gap between auto-laid-out radio options, in page points.
const RADIO_OPTION_GAP: f32 = 4.0;
const CHECKBOX_ON_STATE: &str = "Yes";
/// Text painted in the placeholder appearance of an empty signature field.
const SIGNATURE_APPEARANCE_LABEL: &str = "签名";

/// The field kinds the creation tool supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreatedFieldKind {
    Text,
    Checkbox,
    Radio,
    ComboBox,
    ListBox,
    Signature,
}

/// One widget of a created field in both display space and unrotated PDF user
/// space. A radio group has one widget per option; text and checkbox fields
/// have exactly one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreatedWidget {
    /// The option label shown in the property panel; `None` for text and
    /// checkboxes.
    pub(crate) label: Option<String>,
    /// `/AP /N` on-state name. `Yes` for checkboxes, empty for text.
    pub(crate) export_value: String,
    /// Display-space rectangle the user drew or the auto-layout produced
    /// (top-left origin).
    pub(crate) rect: FormFieldRect,
    /// The same rectangle in unrotated PDF user space, `[left bottom right top]`.
    pub(crate) user_rect: [f32; 4],
    /// The page's `/Rotate` when the widget was placed. The save pipeline uses
    /// it to compensate the appearance stream.
    pub(crate) rotation: u16,
}

/// One field created by this session and not yet flattened into the PDFium
/// document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreatedField {
    pub(crate) uid: u64,
    /// Base id: `new:<uid>`.
    pub(crate) field_id: String,
    pub(crate) page_index: u32,
    pub(crate) kind: CreatedFieldKind,
    pub(crate) name: String,
    pub(crate) tooltip: Option<String>,
    pub(crate) required: bool,
    // Text-only properties.
    pub(crate) multiline: bool,
    pub(crate) font_size: f32,
    /// Current text value; empty until the user fills the field.
    pub(crate) value: String,
    /// Checkbox state (current/default); unused for the other kinds.
    pub(crate) checked: bool,
    /// Radio group's selected option index; `None` when no option is selected.
    pub(crate) selected_option: Option<usize>,
    /// Choice fields: option labels in display order; empty otherwise.
    pub(crate) options: Vec<String>,
    /// Choice fields: selected option indices in ascending order; empty when
    /// unanswered. A dropdown or single-select list has at most one entry.
    pub(crate) selected_options: Vec<usize>,
    /// List boxes: the MultiSelect flag; always false for the other kinds.
    pub(crate) multi_select: bool,
    /// Text and checkbox fields have one widget; radio groups have one per
    /// option in panel order. Choice and signature fields have one widget.
    pub(crate) widgets: Vec<CreatedWidget>,
}

/// Undo/redo snapshot of a created field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CreatedFieldSnapshot {
    pub(crate) field: CreatedField,
}

/// Session store for created fields, ordered by creation.
#[derive(Debug, Default, Clone)]
pub(crate) struct CreatedFieldStore {
    pub(crate) fields: Vec<CreatedField>,
    /// Monotonic; undo never reuses an id, so redo restores the same one.
    pub(crate) next_uid: u64,
}

impl CreatedFieldStore {
    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Finds the field a widget id belongs to. Radio option ids add an index
    /// suffix to the group id; both forms resolve to the same record.
    pub(crate) fn find(&self, field_id: &str) -> Option<&CreatedField> {
        let uid = created_uid(field_id)?;
        self.find_by_uid(uid)
    }

    pub(crate) fn find_by_uid(&self, uid: u64) -> Option<&CreatedField> {
        self.fields.iter().find(|field| field.uid == uid)
    }

    /// Inserts a record, or replaces the record with the same uid. Used by
    /// redo and by every snapshot restore.
    pub(crate) fn restore(&mut self, field: CreatedField) {
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

    pub(crate) fn remove(&mut self, uid: u64) -> Option<CreatedField> {
        let index = self.fields.iter().position(|field| field.uid == uid)?;
        Some(self.fields.remove(index))
    }

    /// A name not used by any existing widget or created field.
    pub(crate) fn generated_name(&self, used: &HashSet<String>, prefix: &str) -> String {
        for suffix in 1.. {
            let candidate = format!("{prefix}{suffix}");
            if !used.contains(&candidate)
                && !self.fields.iter().any(|field| field.name == candidate)
            {
                return candidate;
            }
        }
        unreachable!("u64 suffix space exhausted")
    }
}

/// The property panel's text creation request. `name: None` asks for a
/// generated `text_field_N` name.
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

/// Places any supported field. `name: None` asks for a generated name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CreateFormFieldRequest {
    #[serde(rename_all = "camelCase")]
    Text {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
        required: bool,
        multiline: bool,
        font_size: Option<f32>,
    },
    #[serde(rename_all = "camelCase")]
    Checkbox {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
        required: bool,
        /// Initial `/V`/`/AS` state.
        checked: bool,
    },
    #[serde(rename_all = "camelCase")]
    Radio {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
        required: bool,
        /// Option labels. The first widget uses `rect`; later options stack
        /// below it. An empty list creates one default option.
        options: Vec<String>,
        /// Index of the initially selected option, if any.
        #[serde(default)]
        selected_index: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    ComboBox {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
        required: bool,
        /// `/Opt` labels in display order. An empty list creates one default
        /// option.
        options: Vec<String>,
        /// Default selected option index, if any.
        #[serde(default)]
        selected_index: Option<usize>,
        font_size: Option<f32>,
    },
    #[serde(rename_all = "camelCase")]
    ListBox {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
        required: bool,
        /// `/Opt` labels in display order. An empty list creates one default
        /// option.
        options: Vec<String>,
        /// Default selected option indices; a single-select list accepts at
        /// most one.
        #[serde(default)]
        selected_indices: Vec<usize>,
        /// Sets the MultiSelect field flag; only meaningful for a list box.
        #[serde(default)]
        multi_select: bool,
        font_size: Option<f32>,
    },
    /// An empty signature field: structure only, never a certificate
    /// signature. `required` is deliberately absent because PDFX cannot fill
    /// a signature, so requiring one would block saving permanently.
    #[serde(rename_all = "camelCase")]
    Signature {
        rect: FormFieldRect,
        name: Option<String>,
        tooltip: Option<String>,
    },
}

/// The outcome of placing a text field (the ticket-31 command shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFormTextFieldResult {
    pub field_id: String,
    pub fields: Vec<FormFieldView>,
    pub history: EditHistoryView,
}

/// The outcome of placing any field: the selected widget id (the first radio
/// option for a group), the refreshed field list and the unified history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFormFieldResult {
    pub field_id: String,
    pub fields: Vec<FormFieldView>,
    pub history: EditHistoryView,
}

/// The full desired state of a created text field, sent by the panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFormTextFieldRequest {
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub multiline: bool,
    pub font_size: f32,
}

/// The full desired state of any created field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateFormFieldRequest {
    #[serde(rename_all = "camelCase")]
    Text {
        name: String,
        tooltip: Option<String>,
        required: bool,
        multiline: bool,
        font_size: f32,
    },
    #[serde(rename_all = "camelCase")]
    Checkbox {
        name: String,
        tooltip: Option<String>,
        required: bool,
        checked: bool,
    },
    #[serde(rename_all = "camelCase")]
    Radio {
        name: String,
        tooltip: Option<String>,
        required: bool,
        /// The group's complete option-label list in display order.
        options: Vec<String>,
        selected_index: Option<usize>,
    },
    #[serde(rename_all = "camelCase")]
    ComboBox {
        name: String,
        tooltip: Option<String>,
        required: bool,
        options: Vec<String>,
        selected_index: Option<usize>,
        font_size: f32,
    },
    #[serde(rename_all = "camelCase")]
    ListBox {
        name: String,
        tooltip: Option<String>,
        required: bool,
        options: Vec<String>,
        selected_indices: Vec<usize>,
        multi_select: bool,
        font_size: f32,
    },
    #[serde(rename_all = "camelCase")]
    Signature {
        name: String,
        tooltip: Option<String>,
    },
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

/// Trims option labels and gives blank ones a stable default. A radio group
/// always has at least one option so its `/Kids` array is never empty.
fn normalize_radio_options(options: Vec<String>) -> Vec<String> {
    if options.is_empty() {
        return vec!["选项 1".to_owned()];
    }
    options
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let label = label.trim();
            if label.is_empty() {
                format!("选项 {}", index + 1)
            } else {
                label.to_owned()
            }
        })
        .collect()
}

/// Trims choice option labels and gives blank ones a stable default. A choice
/// field always has at least one option so `/Opt` is never empty; readers keep
/// the label strings verbatim, so duplicates stay allowed and are told apart by
/// their indices in `/I`.
fn normalize_choice_options(options: Vec<String>) -> Vec<String> {
    if options.is_empty() {
        return vec!["选项 1".to_owned()];
    }
    options
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let label = label.trim();
            if label.is_empty() {
                format!("选项 {}", index + 1)
            } else {
                label.to_owned()
            }
        })
        .collect()
}

/// Normalizes selected indices: duplicates collapse, the list stays ascending,
/// and every index must point into the option list. Combo boxes and
/// single-select lists accept at most one selection.
fn normalize_selected_indices(
    mut indices: Vec<usize>,
    option_count: usize,
    multi_select: bool,
) -> Result<Vec<usize>, CommandError> {
    indices.sort_unstable();
    indices.dedup();
    if let Some(index) = indices.iter().find(|index| **index >= option_count) {
        return Err(invalid(format!("默认选项无效（第 {} 项）。", index + 1)));
    }
    if !multi_select && indices.len() > 1 {
        return Err(invalid("未启用多选时只能设置一个默认选项。"));
    }
    Ok(indices)
}

/// Derives a PDF name-object on-state from an option label, uniquifying
/// collisions. Non-ASCII labels cannot be PDF names and fall back to
/// `ChoiceN`; the same fallback applies to `Off`, which is the reserved
/// off-state and cannot be an on-state.
fn radio_export_value(label: &str, index: usize, used: &mut HashSet<String>) -> String {
    let sanitized: String = label
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    let base = if sanitized.is_empty() || sanitized == "Off" {
        format!("Choice{}", index + 1)
    } else {
        sanitized
    };
    if used.insert(base.clone()) {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("radio export-value suffix space exhausted")
}

fn page_geometry(entry: &OpenDocument, page_index: u32) -> Result<DisplayGeometry, CommandError> {
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
    DisplayGeometry::new(&page, rotation)
}

fn widget_from_display(
    geometry: &DisplayGeometry,
    rect: FormFieldRect,
    export_value: String,
    label: Option<String>,
) -> CreatedWidget {
    let (x0, y0) = geometry.point_inverse(rect.x, rect.y);
    let (x1, y1) = geometry.point_inverse(rect.x + rect.width, rect.y + rect.height);
    CreatedWidget {
        label,
        export_value,
        rect,
        user_rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)],
        rotation: geometry.rotation(),
    }
}

/// Lays out one widget per option, stacking below the first rectangle. All
/// options must stay inside the page; otherwise nothing changes.
fn radio_widgets(
    geometry: &DisplayGeometry,
    first: FormFieldRect,
    labels: &[String],
) -> Result<Vec<CreatedWidget>, CommandError> {
    let (_, display_height) = geometry.display_size();
    let mut used = HashSet::new();
    let mut widgets = Vec::with_capacity(labels.len());
    for (index, label) in labels.iter().enumerate() {
        let y = first.y + index as f32 * (first.height + RADIO_OPTION_GAP);
        if y < 0.0 || y + first.height > display_height + 0.01 {
            return Err(invalid("单选按钮选项超出页面范围，请删除选项或缩小字段。"));
        }
        let rect = FormFieldRect {
            x: first.x,
            y,
            width: first.width,
            height: first.height,
        };
        let export_value = radio_export_value(label, index, &mut used);
        widgets.push(widget_from_display(
            geometry,
            rect,
            export_value,
            Some(label.clone()),
        ));
    }
    Ok(widgets)
}

impl CreatedField {
    fn base_view(&self) -> FormFieldView {
        let widget = &self.widgets[0];
        let choice = matches!(
            self.kind,
            CreatedFieldKind::ComboBox | CreatedFieldKind::ListBox
        );
        FormFieldView {
            field_id: self.field_id.clone(),
            name: self.name.clone(),
            kind: match self.kind {
                CreatedFieldKind::Text => FormFieldKind::Text,
                CreatedFieldKind::Checkbox => FormFieldKind::Checkbox,
                CreatedFieldKind::Radio => FormFieldKind::Radio,
                CreatedFieldKind::ComboBox => FormFieldKind::ComboBox,
                CreatedFieldKind::ListBox => FormFieldKind::ListBox,
                CreatedFieldKind::Signature => FormFieldKind::Signature,
            },
            page_index: self.page_index,
            order: 0,
            rect: widget.rect,
            value: if choice {
                self.selected_options
                    .first()
                    .and_then(|index| self.options.get(*index))
                    .cloned()
            } else {
                (!self.value.is_empty()).then(|| self.value.clone())
            },
            checked: self.kind == CreatedFieldKind::Checkbox && self.checked,
            options: if choice {
                self.options.clone()
            } else {
                Vec::new()
            },
            selected_index: (choice && !self.selected_options.is_empty())
                .then(|| self.selected_options[0] as u32),
            selected_indices: if choice {
                self.selected_options
                    .iter()
                    .map(|index| *index as u32)
                    .collect()
            } else {
                Vec::new()
            },
            multi_select: self.multi_select,
            required: self.required,
            read_only: false,
            multiline: self.multiline,
            radio_group: None,
            export_value: None,
            tooltip: self.tooltip.clone(),
            font_size: matches!(
                self.kind,
                CreatedFieldKind::Text | CreatedFieldKind::ComboBox | CreatedFieldKind::ListBox
            )
            .then_some(self.font_size),
            created: true,
            raw_name: self.name.clone(),
        }
    }

    /// One view per widget: text and checkboxes produce one, radio groups one
    /// per option in panel order.
    pub(crate) fn views(&self) -> Vec<FormFieldView> {
        if self.kind != CreatedFieldKind::Radio {
            return vec![self.base_view()];
        }
        let options: Vec<String> = self
            .widgets
            .iter()
            .map(|widget| widget.label.clone().unwrap_or_default())
            .collect();
        let selected_value = self
            .selected_option
            .and_then(|index| self.widgets.get(index))
            .map(|widget| widget.export_value.clone());
        self.widgets
            .iter()
            .enumerate()
            .map(|(index, widget)| {
                let mut view = self.base_view();
                view.field_id = format!("{}:{index}", self.field_id);
                view.rect = widget.rect;
                view.value = selected_value.clone();
                view.checked = self.selected_option == Some(index);
                view.options = options.clone();
                view.selected_index = self.selected_option.map(|index| index as u32);
                view.radio_group = Some(self.name.clone());
                view.export_value = Some(widget.export_value.clone());
                view
            })
            .collect()
    }

    pub(crate) fn snapshot(&self) -> CreatedFieldSnapshot {
        CreatedFieldSnapshot {
            field: self.clone(),
        }
    }
}

/// Merges created fields into PDFium's widget list, page by page, with created
/// fields after the existing widgets of their page. `order` is renumbered to
/// stay a contiguous document-wide tab order; radio options keep their panel
/// order.
pub(crate) fn merge_views(
    existing: Vec<FormFieldView>,
    store: &CreatedFieldStore,
) -> Vec<FormFieldView> {
    if store.is_empty() {
        return existing;
    }

    let mut entries: Vec<(u32, u8, u64, u64, FormFieldView)> = existing
        .into_iter()
        .map(|view| (view.page_index, 0, u64::from(view.order), 0, view))
        .collect();
    for field in &store.fields {
        for (option_index, view) in field.views().into_iter().enumerate() {
            entries.push((field.page_index, 1, field.uid, option_index as u64, view));
        }
    }
    entries.sort_by_key(|(page_index, created, uid, option, _)| {
        (*page_index, *created, *uid, *option)
    });
    entries
        .into_iter()
        .enumerate()
        .map(|(order, (_, _, _, _, mut view))| {
            view.order = order as u32;
            view
        })
        .collect()
}

/// Duplicates the session-created fields of a page when that page is copied.
///
/// Created fields live only in this session until the next save, so Pdfium's
/// page importer cannot carry them onto the copy. Each clone keeps the source
/// field's geometry, properties and current value, but gets a fresh uid (so
/// form edits address exactly one of the two records) and a copy-namespaced
/// name (so the saved AcroForm has two distinct fields). The records are
/// visible through discovery immediately and are written by the normal
/// created-field save step; undoing the page copy hides them with the page.
pub(crate) fn copy_created_fields_for_page(
    store: &mut CreatedFieldStore,
    source_page: u32,
    target_page: u32,
    used_names: &HashSet<String>,
) {
    let sources: Vec<CreatedField> = store
        .fields
        .iter()
        .filter(|field| field.page_index == source_page)
        .cloned()
        .collect();
    let mut used = used_names.clone();
    for mut field in sources {
        store.next_uid += 1;
        let uid = store.next_uid;
        let base = crate::page_ops::copied_field_name(&field.name, target_page);
        let mut name = base.clone();
        let mut suffix = 2;
        while used.contains(&name) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        used.insert(name.clone());
        field.uid = uid;
        field.field_id = format!("{CREATED_ID_PREFIX}{uid}");
        field.page_index = target_page;
        field.name = name;
        store.restore(field);
    }
}

/// Parses a created-field id back to its session uid. Radio option suffixes
/// are ignored here; [`created_option_index`] reads them.
pub(crate) fn created_uid(field_id: &str) -> Option<u64> {
    let rest = field_id.strip_prefix(CREATED_ID_PREFIX)?;
    let uid = rest.split(':').next()?;
    uid.parse().ok()
}

/// Parses the `:<option-index>` suffix of a created radio widget id.
pub(crate) fn created_option_index(field_id: &str) -> Option<usize> {
    let rest = field_id.strip_prefix(CREATED_ID_PREFIX)?;
    let (_, index) = rest.split_once(':')?;
    index.parse().ok()
}

fn not_created(field_id: &str) -> CommandError {
    CommandError::new(
        CommandErrorCode::FormFieldNotFound,
        format!("只能修改或删除本次会话创建的表单字段（{field_id}）。"),
    )
}

fn create_field_for(
    entry: &mut OpenDocument,
    page_index: u32,
    request: CreateFormFieldRequest,
) -> Result<CreateFormFieldResult, CommandError> {
    ensure_creatable(entry)?;

    let geometry = page_geometry(entry, page_index)?;
    let (display_width, display_height) = geometry.display_size();

    let (rect, kind, requested_name, requested_tooltip, required) = match &request {
        CreateFormFieldRequest::Text {
            rect,
            name,
            tooltip,
            required,
            ..
        } => (
            *rect,
            CreatedFieldKind::Text,
            name.clone(),
            tooltip.clone(),
            *required,
        ),
        CreateFormFieldRequest::Checkbox {
            rect,
            name,
            tooltip,
            required,
            ..
        } => (
            *rect,
            CreatedFieldKind::Checkbox,
            name.clone(),
            tooltip.clone(),
            *required,
        ),
        CreateFormFieldRequest::Radio {
            rect,
            name,
            tooltip,
            required,
            ..
        } => (
            *rect,
            CreatedFieldKind::Radio,
            name.clone(),
            tooltip.clone(),
            *required,
        ),
        CreateFormFieldRequest::ComboBox {
            rect,
            name,
            tooltip,
            required,
            ..
        } => (
            *rect,
            CreatedFieldKind::ComboBox,
            name.clone(),
            tooltip.clone(),
            *required,
        ),
        CreateFormFieldRequest::ListBox {
            rect,
            name,
            tooltip,
            required,
            ..
        } => (
            *rect,
            CreatedFieldKind::ListBox,
            name.clone(),
            tooltip.clone(),
            *required,
        ),
        CreateFormFieldRequest::Signature {
            rect,
            name,
            tooltip,
            ..
        } => (
            *rect,
            CreatedFieldKind::Signature,
            name.clone(),
            tooltip.clone(),
            false,
        ),
    };
    validate_rect(rect, display_width, display_height)?;
    let tooltip = normalize_tooltip(requested_tooltip);

    let existing = crate::form::discover_all(entry)?;
    let used: HashSet<String> = existing.iter().map(|field| field.name.clone()).collect();
    let prefix = match kind {
        CreatedFieldKind::Text => TEXT_NAME_PREFIX,
        CreatedFieldKind::Checkbox => CHECKBOX_NAME_PREFIX,
        CreatedFieldKind::Radio => RADIO_NAME_PREFIX,
        CreatedFieldKind::ComboBox => COMBO_BOX_NAME_PREFIX,
        CreatedFieldKind::ListBox => LIST_BOX_NAME_PREFIX,
        CreatedFieldKind::Signature => SIGNATURE_NAME_PREFIX,
    };
    let name = match requested_name {
        Some(raw) => {
            let name = raw.trim().to_owned();
            validate_name(&name)?;
            if used.contains(&name) {
                return Err(duplicate_name(&name));
            }
            name
        }
        None => entry.edit.created_fields.generated_name(&used, prefix),
    };

    let mut multiline = false;
    let mut font_size = DEFAULT_FONT_SIZE;
    let value = String::new();
    let mut checked = false;
    let mut selected_option = None;
    let mut options = Vec::new();
    let mut selected_options = Vec::new();
    let mut multi_select = false;
    let widgets = match &request {
        CreateFormFieldRequest::Text {
            multiline: requested_multiline,
            font_size: requested_font_size,
            ..
        } => {
            multiline = *requested_multiline;
            font_size = normalize_font_size(*requested_font_size)?;
            vec![widget_from_display(&geometry, rect, String::new(), None)]
        }
        CreateFormFieldRequest::Checkbox {
            checked: requested_checked,
            ..
        } => {
            checked = *requested_checked;
            vec![widget_from_display(
                &geometry,
                rect,
                CHECKBOX_ON_STATE.to_owned(),
                None,
            )]
        }
        CreateFormFieldRequest::Radio {
            options: requested_options,
            selected_index,
            ..
        } => {
            let labels = normalize_radio_options(requested_options.clone());
            if let Some(index) = selected_index {
                if *index >= labels.len() {
                    return Err(invalid("默认选项无效。"));
                }
            }
            selected_option = *selected_index;
            radio_widgets(&geometry, rect, &labels)?
        }
        CreateFormFieldRequest::ComboBox {
            options: requested_options,
            selected_index,
            font_size: requested_font_size,
            ..
        } => {
            options = normalize_choice_options(requested_options.clone());
            selected_options = normalize_selected_indices(
                selected_index.iter().copied().collect(),
                options.len(),
                false,
            )?;
            font_size = normalize_font_size(*requested_font_size)?;
            vec![widget_from_display(&geometry, rect, String::new(), None)]
        }
        CreateFormFieldRequest::ListBox {
            options: requested_options,
            selected_indices,
            multi_select: requested_multi_select,
            font_size: requested_font_size,
            ..
        } => {
            multi_select = *requested_multi_select;
            options = normalize_choice_options(requested_options.clone());
            selected_options =
                normalize_selected_indices(selected_indices.clone(), options.len(), multi_select)?;
            font_size = normalize_font_size(*requested_font_size)?;
            vec![widget_from_display(&geometry, rect, String::new(), None)]
        }
        CreateFormFieldRequest::Signature { .. } => {
            vec![widget_from_display(&geometry, rect, String::new(), None)]
        }
    };

    let uid = {
        let store = &mut entry.edit.created_fields;
        store.next_uid += 1;
        store.next_uid
    };
    let field_id = format!("{CREATED_ID_PREFIX}{uid}");
    let field = CreatedField {
        uid,
        field_id: field_id.clone(),
        page_index,
        kind,
        name,
        tooltip,
        required,
        multiline,
        font_size,
        value,
        checked,
        selected_option,
        options,
        selected_options,
        multi_select,
        widgets,
    };
    let snapshot = field.snapshot();
    entry.edit.created_fields.restore(field);
    let history = push_form_structure_action(entry, uid, page_index, None, Some(snapshot));

    let selected_field_id = if kind == CreatedFieldKind::Radio {
        format!("{field_id}:0")
    } else {
        field_id
    };
    Ok(CreateFormFieldResult {
        field_id: selected_field_id,
        fields: crate::form::discover_all(entry)?,
        history,
    })
}

/// The `create_form_field` command. Places a text, checkbox or radio field on
/// a page and records the creation as one undoable action.
#[tauri::command]
pub fn create_form_field(
    document_id: String,
    page_index: u32,
    request: CreateFormFieldRequest,
) -> Result<CreateFormFieldResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut result = create_field_for(entry, physical, request)?;
        result.fields = crate::form::logical_fields(entry, result.fields);
        Ok(result)
    })
}

/// The ticket-31 command. Places a text widget and records the creation as one
/// undoable action; the generic command is the seam ticket 32/33 use.
#[tauri::command]
pub fn create_form_text_field(
    document_id: String,
    page_index: u32,
    request: CreateFormTextFieldRequest,
) -> Result<CreateFormTextFieldResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let result = create_field_for(
            entry,
            physical,
            CreateFormFieldRequest::Text {
                rect: request.rect,
                name: request.name,
                tooltip: request.tooltip,
                required: request.required,
                multiline: request.multiline,
                font_size: request.font_size,
            },
        )?;
        Ok(CreateFormTextFieldResult {
            field_id: result.field_id,
            fields: crate::form::logical_fields(entry, result.fields),
            history: result.history,
        })
    })
}

fn update_field_for(
    entry: &mut OpenDocument,
    field_id: &str,
    request: UpdateFormFieldRequest,
) -> Result<FormStructureEditResult, CommandError> {
    ensure_creatable(entry)?;
    let current = entry
        .edit
        .created_fields
        .find(field_id)
        .cloned()
        .ok_or_else(|| not_created(field_id))?;

    let mut updated = current.clone();
    let (name, tooltip, required) = match request {
        UpdateFormFieldRequest::Text {
            name,
            tooltip,
            required,
            multiline,
            font_size,
        } => {
            if current.kind != CreatedFieldKind::Text {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            updated.multiline = multiline;
            updated.font_size = normalize_font_size(Some(font_size))?;
            (name, tooltip, required)
        }
        UpdateFormFieldRequest::Checkbox {
            name,
            tooltip,
            required,
            checked,
        } => {
            if current.kind != CreatedFieldKind::Checkbox {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            updated.checked = checked;
            (name, tooltip, required)
        }
        UpdateFormFieldRequest::Radio {
            name,
            tooltip,
            required,
            options,
            selected_index,
        } => {
            if current.kind != CreatedFieldKind::Radio {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            let labels = normalize_radio_options(options);
            if let Some(index) = selected_index {
                if index >= labels.len() {
                    return Err(invalid("默认选项无效。"));
                }
            }
            let geometry = page_geometry(entry, current.page_index)?;
            let first = current
                .widgets
                .first()
                .ok_or_else(|| invalid("单选按钮组缺少可用的位置。"))?
                .rect;
            updated.widgets = radio_widgets(&geometry, first, &labels)?;
            updated.selected_option = selected_index;
            (name, tooltip, required)
        }
        UpdateFormFieldRequest::ComboBox {
            name,
            tooltip,
            required,
            options,
            selected_index,
            font_size,
        } => {
            if current.kind != CreatedFieldKind::ComboBox {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            let labels = normalize_choice_options(options);
            updated.selected_options = normalize_selected_indices(
                selected_index.into_iter().collect(),
                labels.len(),
                false,
            )?;
            updated.options = labels;
            updated.font_size = normalize_font_size(Some(font_size))?;
            (name, tooltip, required)
        }
        UpdateFormFieldRequest::ListBox {
            name,
            tooltip,
            required,
            options,
            selected_indices,
            multi_select,
            font_size,
        } => {
            if current.kind != CreatedFieldKind::ListBox {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            let labels = normalize_choice_options(options);
            updated.selected_options =
                normalize_selected_indices(selected_indices, labels.len(), multi_select)?;
            updated.options = labels;
            updated.multi_select = multi_select;
            updated.font_size = normalize_font_size(Some(font_size))?;
            (name, tooltip, required)
        }
        UpdateFormFieldRequest::Signature { name, tooltip, .. } => {
            if current.kind != CreatedFieldKind::Signature {
                return Err(invalid("表单字段类型与更新请求不一致。"));
            }
            // A signature field is structure only; it keeps no required flag
            // (PDFX cannot fill it) and no value to edit.
            (name, tooltip, false)
        }
    };

    let name = name.trim().to_owned();
    validate_name(&name)?;
    if name != current.name
        && crate::form::discover_all(entry)?
            .iter()
            .any(|field| field.name == name && created_uid(&field.field_id) != Some(current.uid))
    {
        return Err(duplicate_name(&name));
    }
    updated.name = name;
    updated.tooltip = normalize_tooltip(tooltip);
    updated.required = required;

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

/// The `update_form_field` command. Changes any created field's properties
/// (including a radio group's option list and default choice) and records the
/// previous state for undo.
#[tauri::command]
pub fn update_form_field(
    document_id: String,
    field_id: String,
    request: UpdateFormFieldRequest,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let mut result = update_field_for(entry, &field_id, request)?;
        result.fields = crate::form::logical_fields(entry, result.fields);
        Ok(result)
    })
}

/// The ticket-31 command. Changes a created text field's properties.
#[tauri::command]
pub fn update_form_text_field(
    document_id: String,
    field_id: String,
    request: UpdateFormTextFieldRequest,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let mut result = update_field_for(
            entry,
            &field_id,
            UpdateFormFieldRequest::Text {
                name: request.name,
                tooltip: request.tooltip,
                required: request.required,
                multiline: request.multiline,
                font_size: request.font_size,
            },
        )?;
        result.fields = crate::form::logical_fields(entry, result.fields);
        Ok(result)
    })
}

fn delete_field_for(
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

/// The `delete_form_field` command. Deletes any field created in this session
/// (a radio group as a whole); the removal is undone by the shared history.
#[tauri::command]
pub fn delete_form_field(
    document_id: String,
    field_id: String,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let mut result = delete_field_for(entry, &field_id)?;
        result.fields = crate::form::logical_fields(entry, result.fields);
        Ok(result)
    })
}

/// The ticket-31 command. Deletes a text field created in this session.
#[tauri::command]
pub fn delete_form_text_field(
    document_id: String,
    field_id: String,
) -> Result<FormStructureEditResult, CommandError> {
    with_open_document(&document_id, |entry| {
        let mut result = delete_field_for(entry, &field_id)?;
        result.fields = crate::form::logical_fields(entry, result.fields);
        Ok(result)
    })
}

// ---------------------------------------------------------------------------
// Save pipeline: materialize created fields as AcroForm objects.
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;

use lopdf::{Dictionary, Document, IncrementalDocument, Object, ObjectId, Stream};

use crate::fonts::FontSubset;

/// One created text field as the save pipeline writes it. Only plain values
/// are carried; the appearance is generated from them.
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

/// One created checkbox as the save pipeline writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckboxFieldSpec {
    pub page_index: u32,
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub checked: bool,
    pub user_rect: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
}

/// One option widget of a created radio group.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioOptionSpec {
    pub label: String,
    pub export_value: String,
    pub user_rect: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
}

/// One created radio group: a parent field with one kid widget per option.
#[derive(Debug, Clone, PartialEq)]
pub struct RadioGroupSpec {
    pub page_index: u32,
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub options: Vec<RadioOptionSpec>,
    /// Selected option index; `None` writes `/V /Off` and `/AS /Off`
    /// everywhere.
    pub selected: Option<usize>,
}

/// One created choice field as the save pipeline writes it: a combo box when
/// `combo` is true, otherwise a list box.
#[derive(Debug, Clone, PartialEq)]
pub struct ChoiceFieldSpec {
    pub page_index: u32,
    pub name: String,
    pub tooltip: Option<String>,
    pub required: bool,
    pub combo: bool,
    pub multi_select: bool,
    pub font_size: f32,
    /// `/Opt` labels in display order.
    pub options: Vec<String>,
    /// Selected option indices in ascending order.
    pub selected: Vec<usize>,
    pub user_rect: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
}

/// One created empty signature field. Structure only: the writer never emits
/// `/V`, `/SV` or `/Lock`, and nothing here signs anything.
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureFieldSpec {
    pub page_index: u32,
    pub name: String,
    pub tooltip: Option<String>,
    pub user_rect: [f32; 4],
    pub width: f32,
    pub height: f32,
    pub rotation: u16,
}

/// One field the save pipeline materializes.
#[derive(Debug, Clone, PartialEq)]
pub enum FormFieldSpec {
    Text(TextFieldSpec),
    Checkbox(CheckboxFieldSpec),
    Radio(RadioGroupSpec),
    Choice(ChoiceFieldSpec),
    Signature(SignatureFieldSpec),
}

impl FormFieldSpec {
    fn page_index(&self) -> u32 {
        match self {
            Self::Text(spec) => spec.page_index,
            Self::Checkbox(spec) => spec.page_index,
            Self::Radio(spec) => spec.page_index,
            Self::Choice(spec) => spec.page_index,
            Self::Signature(spec) => spec.page_index,
        }
    }
}

/// One existing widget value whose appearance must be rebuilt on save.
///
/// Filling an existing field records the value in the edit history; the value
/// itself is written by the `Set*Value` object edits. This spec carries the
/// same value to the appearance writer, which reads the field's widgets, page
/// rotation and layout entries from the document and produces an `/AP /N`
/// that matches.
#[derive(Debug, Clone, PartialEq)]
pub struct ExistingFieldAppearance {
    pub field_name: String,
    pub value: ExistingAppearanceValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExistingAppearanceValue {
    Text(String),
    Checked(bool),
    /// Selected on-state name of a radio group.
    Radio(String),
    /// Selected option label of a choice field.
    Choice(String),
}

/// Widgets and layout inputs resolved before the font subset is built.
struct PreparedAppearance {
    widgets: Vec<PreparedWidget>,
    kind: PreparedKind,
}

struct PreparedWidget {
    id: ObjectId,
    user_rect: [f32; 4],
    /// Display-space dimensions: swapped for pages rotated by 90/270.
    width: f32,
    height: f32,
    rotation: u16,
    /// Non-Off state of a button widget's `/AP /N` dictionary.
    on_state: Option<Vec<u8>>,
}

enum PreparedKind {
    Text {
        value: String,
        multiline: bool,
        font_size: f32,
    },
    Choice {
        options: Vec<String>,
        selected: Vec<usize>,
        combo: bool,
        font_size: f32,
    },
    Checkbox {
        checked: bool,
        on_state: Vec<u8>,
    },
    Radio {
        selected: String,
    },
}

impl CreatedField {
    fn text_spec(&self) -> Option<TextFieldSpec> {
        let widget = self.widgets.first()?;
        (self.kind == CreatedFieldKind::Text).then(|| TextFieldSpec {
            page_index: self.page_index,
            name: self.name.clone(),
            tooltip: self.tooltip.clone(),
            required: self.required,
            multiline: self.multiline,
            font_size: self.font_size,
            user_rect: widget.user_rect,
            width: widget.rect.width,
            height: widget.rect.height,
            value: self.value.clone(),
            rotation: widget.rotation,
        })
    }

    fn checkbox_spec(&self) -> Option<CheckboxFieldSpec> {
        let widget = self.widgets.first()?;
        (self.kind == CreatedFieldKind::Checkbox).then(|| CheckboxFieldSpec {
            page_index: self.page_index,
            name: self.name.clone(),
            tooltip: self.tooltip.clone(),
            required: self.required,
            checked: self.checked,
            user_rect: widget.user_rect,
            width: widget.rect.width,
            height: widget.rect.height,
            rotation: widget.rotation,
        })
    }

    fn radio_spec(&self) -> Option<RadioGroupSpec> {
        if self.kind != CreatedFieldKind::Radio {
            return None;
        }
        Some(RadioGroupSpec {
            page_index: self.page_index,
            name: self.name.clone(),
            tooltip: self.tooltip.clone(),
            required: self.required,
            options: self
                .widgets
                .iter()
                .map(|widget| RadioOptionSpec {
                    label: widget.label.clone().unwrap_or_default(),
                    export_value: widget.export_value.clone(),
                    user_rect: widget.user_rect,
                    width: widget.rect.width,
                    height: widget.rect.height,
                    rotation: widget.rotation,
                })
                .collect(),
            selected: self.selected_option,
        })
    }

    fn choice_spec(&self) -> Option<ChoiceFieldSpec> {
        let widget = self.widgets.first()?;
        let combo = match self.kind {
            CreatedFieldKind::ComboBox => true,
            CreatedFieldKind::ListBox => false,
            _ => return None,
        };
        Some(ChoiceFieldSpec {
            page_index: self.page_index,
            name: self.name.clone(),
            tooltip: self.tooltip.clone(),
            required: self.required,
            combo,
            multi_select: self.multi_select,
            font_size: self.font_size,
            options: self.options.clone(),
            selected: self.selected_options.clone(),
            user_rect: widget.user_rect,
            width: widget.rect.width,
            height: widget.rect.height,
            rotation: widget.rotation,
        })
    }

    fn signature_spec(&self) -> Option<SignatureFieldSpec> {
        let widget = self.widgets.first()?;
        (self.kind == CreatedFieldKind::Signature).then(|| SignatureFieldSpec {
            page_index: self.page_index,
            name: self.name.clone(),
            tooltip: self.tooltip.clone(),
            user_rect: widget.user_rect,
            width: widget.rect.width,
            height: widget.rect.height,
            rotation: widget.rotation,
        })
    }
}

/// The page's current `/Rotate`. A widget placed before a page rotation still
/// needs an appearance that matches the final rotation, so the save pipeline
/// re-reads it instead of trusting the snapshot taken at creation time.
fn current_rotation(
    document: &PdfDocument<'static>,
    physical_page: u32,
) -> Result<u16, CommandError> {
    let page = load_page(document, physical_page)?;
    page.rotation()
        .map(|rotation| rotation.as_degrees() as u16)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read the rotation of page {physical_page}: {error:?}"),
            )
        })
}

/// Rewrites every rotation carried by a spec. Radio groups store one per kid;
/// a group's kids all sit on the same page.
fn with_rotation(mut spec: FormFieldSpec, rotation: u16) -> FormFieldSpec {
    match &mut spec {
        FormFieldSpec::Text(spec) => spec.rotation = rotation,
        FormFieldSpec::Checkbox(spec) => spec.rotation = rotation,
        FormFieldSpec::Radio(spec) => {
            for option in &mut spec.options {
                option.rotation = rotation;
            }
        }
        FormFieldSpec::Choice(spec) => spec.rotation = rotation,
        FormFieldSpec::Signature(spec) => spec.rotation = rotation,
    }
    spec
}

/// Object-level edits for the fields this session created. One edit carries
/// every field, so the save pipeline stays at one writer per revision.
/// Fields on deleted pages are omitted: the page tree no longer references
/// those pages, so their widgets must not appear in the saved file. Undoing
/// the deletion brings the field back on the next save.
pub(crate) fn save_object_edits(
    edit: &EditSession,
    pages: &crate::page_ops::PageModel,
    document: &PdfDocument<'static>,
) -> Result<Vec<crate::save::ObjectEdit>, CommandError> {
    let mut specs = Vec::new();
    for field in &edit.created_fields.fields {
        if pages.logical(field.page_index).is_none() {
            continue;
        }
        let rotation = current_rotation(document, field.page_index)?;
        let spec = match field.kind {
            CreatedFieldKind::Text => field.text_spec().map(FormFieldSpec::Text),
            CreatedFieldKind::Checkbox => field.checkbox_spec().map(FormFieldSpec::Checkbox),
            CreatedFieldKind::Radio => field.radio_spec().map(FormFieldSpec::Radio),
            CreatedFieldKind::ComboBox | CreatedFieldKind::ListBox => {
                field.choice_spec().map(FormFieldSpec::Choice)
            }
            CreatedFieldKind::Signature => field.signature_spec().map(FormFieldSpec::Signature),
        };
        if let Some(spec) = spec {
            specs.push(with_rotation(spec, rotation));
        }
    }
    if specs.is_empty() {
        let appearances = existing_appearances(edit, pages);
        if appearances.is_empty() {
            return Ok(Vec::new());
        }
        return Ok(vec![crate::save::ObjectEdit::CreateFormFields {
            fields: Vec::new(),
            appearances,
        }]);
    }
    Ok(vec![crate::save::ObjectEdit::CreateFormFields {
        fields: specs,
        appearances: existing_appearances(edit, pages),
    }])
}

/// Mirrors the value edits derived from the history overlay into appearance
/// specs: every filled existing field needs a widget appearance that shows the
/// new value, not the source file's stale one.
fn existing_appearances(
    edit: &EditSession,
    pages: &crate::page_ops::PageModel,
) -> Vec<ExistingFieldAppearance> {
    crate::form::save_object_edits(edit, pages)
        .into_iter()
        .filter_map(|object_edit| {
            let (field_name, value) = match object_edit {
                crate::save::ObjectEdit::SetTextValue { field_name, text } => {
                    (field_name, ExistingAppearanceValue::Text(text))
                }
                crate::save::ObjectEdit::SetCheckboxValue {
                    field_name,
                    checked,
                } => (field_name, ExistingAppearanceValue::Checked(checked)),
                crate::save::ObjectEdit::SetRadioGroup { field_name, state } => {
                    (field_name, ExistingAppearanceValue::Radio(state))
                }
                crate::save::ObjectEdit::SetChoiceValue { field_name, label } => {
                    (field_name, ExistingAppearanceValue::Choice(label))
                }
                _ => return None,
            };
            Some(ExistingFieldAppearance { field_name, value })
        })
        .collect()
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

fn text_appearance_content(
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

fn checkbox_appearance_content(
    rotation: u16,
    user_rect: [f32; 4],
    width: f32,
    height: f32,
    checked: bool,
) -> String {
    let width = width.max(0.01);
    let height = height.max(0.01);
    let (a, b, c, d, e, f) = rotation_matrix(rotation, user_rect);
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
    if checked {
        content.push_str(&format!(
            "1.5 w {x1} {y1} m {x2} {y2} l {x3} {y3} l S\n",
            x1 = number(width * 0.22),
            y1 = number(height * 0.52),
            x2 = number(width * 0.42),
            y2 = number(height * 0.28),
            x3 = number(width * 0.78),
            y3 = number(height * 0.72),
        ));
    }
    content.push_str("Q\n");
    content
}

fn circle_path(center_x: f32, center_y: f32, radius: f32) -> String {
    let kappa = 0.552_284_8;
    let control = radius * kappa;
    format!(
        "{x0} {cy} m {x0} {cy1} {cx1} {y0} {cx} {y0} c {cx2} {y0} {x1} {cy1} {x1} {cy} c {x1} {cy2} {cx2} {y1} {cx} {y1} c {cx1} {y1} {x0} {cy2} {x0} {cy} c",
        x0 = number(center_x - radius),
        x1 = number(center_x + radius),
        cx = number(center_x),
        y0 = number(center_y - radius),
        y1 = number(center_y + radius),
        cy = number(center_y),
        cx1 = number(center_x - control),
        cx2 = number(center_x + control),
        cy1 = number(center_y - control),
        cy2 = number(center_y + control),
    )
}

fn radio_appearance_content(
    rotation: u16,
    user_rect: [f32; 4],
    width: f32,
    height: f32,
    selected: bool,
) -> String {
    let width = width.max(0.01);
    let height = height.max(0.01);
    let radius = (width.min(height) / 2.0 - 1.0).max(0.5);
    let center_x = width / 2.0;
    let center_y = height / 2.0;
    let (a, b, c, d, e, f) = rotation_matrix(rotation, user_rect);
    let mut content = format!(
        "q {} {} {} {} {} {} cm\n\
         1 1 1 rg 0 0 {w} {h} re f\n\
         0 0 0 RG 1 w {} S\n",
        number(a),
        number(b),
        number(c),
        number(d),
        number(e),
        number(f),
        circle_path(center_x, center_y, radius),
        w = number(width),
        h = number(height),
    );
    if selected {
        content.push_str(&format!(
            "0 0 0 rg {} f\n",
            circle_path(center_x, center_y, (radius * 0.55).max(0.5))
        ));
    }
    content.push_str("Q\n");
    content
}

fn write_appearance_stream(
    document: &mut Document,
    user_rect: [f32; 4],
    content: String,
    resources: Option<Dictionary>,
) -> Result<ObjectId, CommandError> {
    let mut appearance = dictionary([
        ("Type", name("XObject")),
        ("Subtype", name("Form")),
        ("FormType", Object::Integer(1)),
        ("BBox", rect_array(user_rect)),
    ]);
    if let Some(resources) = resources {
        appearance.set(b"Resources".to_vec(), Object::Dictionary(resources));
    }
    let stream = Stream::new(appearance, content.into_bytes());
    document
        .try_add_object(stream)
        .map_err(|error| pdf_error(format!("could not add the widget appearance: {error}")))
}

fn write_text_appearance(
    document: &mut Document,
    spec: &TextFieldSpec,
    font_name: &str,
    font_id: ObjectId,
    subset: &FontSubset,
    metrics: &FontMetrics,
) -> Result<ObjectId, CommandError> {
    let content = text_appearance_content(spec, font_name, subset, metrics);
    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
    resources.set(b"Font".to_vec(), Object::Dictionary(fonts));
    write_appearance_stream(document, spec.user_rect, content, Some(resources))
}

fn write_text_field(
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

fn write_checkbox(
    document: &mut Document,
    spec: &CheckboxFieldSpec,
    page_id: ObjectId,
) -> Result<ObjectId, CommandError> {
    let off = write_appearance_stream(
        document,
        spec.user_rect,
        checkbox_appearance_content(
            spec.rotation,
            spec.user_rect,
            spec.width,
            spec.height,
            false,
        ),
        None,
    )?;
    let on = write_appearance_stream(
        document,
        spec.user_rect,
        checkbox_appearance_content(spec.rotation, spec.user_rect, spec.width, spec.height, true),
        None,
    )?;
    let mut normal = Dictionary::new();
    normal.set(b"Off".to_vec(), Object::Reference(off));
    normal.set(CHECKBOX_ON_STATE.as_bytes().to_vec(), Object::Reference(on));
    let mut appearance = Dictionary::new();
    appearance.set(b"N".to_vec(), Object::Dictionary(normal));

    let state = if spec.checked {
        CHECKBOX_ON_STATE
    } else {
        "Off"
    };
    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }
    let mut field = dictionary([
        ("Type", name("Annot")),
        ("Subtype", name("Widget")),
        ("FT", name("Btn")),
        ("T", text_string(&spec.name)),
        ("Rect", rect_array(spec.user_rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        ("V", name(state)),
        ("AS", name(state)),
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
    document
        .try_add_object(field)
        .map_err(|error| pdf_error(format!("could not add the checkbox field: {error}")))
}

/// Writes a radio parent field and its one-kid-per-option widgets. The parent
/// `/V` and every kid `/AS` are derived from the same selected index, so they
/// always agree; `/Off` everywhere means the group is unanswered.
fn write_radio_group(
    document: &mut Document,
    spec: &RadioGroupSpec,
    page_id: ObjectId,
) -> Result<(ObjectId, Vec<ObjectId>), CommandError> {
    let parent_id = document
        .try_new_object_id()
        .map_err(|error| pdf_error(format!("could not allocate the radio field: {error}")))?;

    let mut kid_ids = Vec::with_capacity(spec.options.len());
    let mut kid_refs = Vec::with_capacity(spec.options.len());
    for (index, option) in spec.options.iter().enumerate() {
        let off = write_appearance_stream(
            document,
            option.user_rect,
            radio_appearance_content(
                option.rotation,
                option.user_rect,
                option.width,
                option.height,
                false,
            ),
            None,
        )?;
        let on = write_appearance_stream(
            document,
            option.user_rect,
            radio_appearance_content(
                option.rotation,
                option.user_rect,
                option.width,
                option.height,
                true,
            ),
            None,
        )?;
        let mut normal = Dictionary::new();
        normal.set(b"Off".to_vec(), Object::Reference(off));
        normal.set(
            option.export_value.as_bytes().to_vec(),
            Object::Reference(on),
        );
        let mut appearance = Dictionary::new();
        appearance.set(b"N".to_vec(), Object::Dictionary(normal));

        let selected = spec.selected == Some(index);
        let appearance_state: &str = if selected {
            &option.export_value
        } else {
            "Off"
        };
        let kid = dictionary([
            ("Type", name("Annot")),
            ("Subtype", name("Widget")),
            ("Parent", Object::Reference(parent_id)),
            ("Rect", rect_array(option.user_rect)),
            ("P", Object::Reference(page_id)),
            ("F", Object::Integer(4)), // Print
            ("AS", name(appearance_state)),
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
        let kid_id = document
            .try_add_object(kid)
            .map_err(|error| pdf_error(format!("could not add a radio widget: {error}")))?;
        kid_ids.push(kid_id);
        kid_refs.push(Object::Reference(kid_id));
    }

    let mut flags = 1 << 15; // Radio
    if spec.required {
        flags |= 1 << 1; // Required
    }
    let value = spec
        .selected
        .and_then(|index| spec.options.get(index))
        .map(|option| option.export_value.as_str())
        .unwrap_or("Off");
    let mut parent = dictionary([
        ("FT", name("Btn")),
        ("T", text_string(&spec.name)),
        ("Ff", Object::Integer(flags)),
        ("Kids", Object::Array(kid_refs)),
        ("V", name(value)),
    ]);
    if let Some(tooltip) = &spec.tooltip {
        parent.set(b"TU".to_vec(), text_string(tooltip));
    }
    document.set_object(parent_id, parent);
    Ok((parent_id, kid_ids))
}

fn choice_appearance_content(
    spec: &ChoiceFieldSpec,
    font_name: &str,
    subset: &FontSubset,
    _metrics: &FontMetrics,
) -> String {
    let width = spec.width.max(0.01);
    let height = spec.height.max(0.01);
    let cids = character_cids(subset);
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

    if spec.combo {
        let text_size = spec.font_size.min((height - 4.0).max(1.0));
        if let Some(label) = spec
            .selected
            .first()
            .and_then(|index| spec.options.get(*index))
        {
            let baseline = (height - text_size) / 2.0;
            content.push_str(&format!(
                "BT 0 0 0 rg /{} {} Tf 1 0 0 1 3 {} Tm <{}> Tj ET\n",
                font_name,
                number(text_size),
                number(baseline),
                hex_text(label, &cids)
            ));
        }
        // The dropdown affordance stays visible even when nothing is selected,
        // so an empty combo box still reads as a choice control.
        let arrow_width = (width * 0.12).clamp(6.0, 14.0);
        let arrow_x = (width - arrow_width - 3.0).max(0.0);
        let arrow_y = height / 2.0;
        content.push_str(&format!(
            "0 0 0 rg\n{} {} m {} {} l {} {} l f\n",
            number(arrow_x),
            number(arrow_y + arrow_width * 0.35),
            number(arrow_x + arrow_width),
            number(arrow_y + arrow_width * 0.35),
            number(arrow_x + arrow_width / 2.0),
            number(arrow_y - arrow_width * 0.35)
        ));
    } else {
        let count = spec.options.len().max(1) as f32;
        let line_height = (height / count).max(1.0);
        let text_size = spec.font_size.min((line_height * 0.62).max(1.0));
        for (index, option) in spec.options.iter().enumerate() {
            let top = height - index as f32 * line_height;
            let bottom = top - line_height;
            let baseline = bottom + (line_height - text_size) / 2.0;
            if spec.selected.contains(&index) {
                content.push_str(&format!(
                    "0.20 0.45 0.85 rg 1 {} {} {} re f\n\
                     BT 1 1 1 rg /{} {} Tf 1 0 0 1 3 {} Tm <{}> Tj ET\n",
                    number(bottom.max(0.0)),
                    number((width - 2.0).max(0.0)),
                    number(line_height),
                    font_name,
                    number(text_size),
                    number(baseline),
                    hex_text(option, &cids)
                ));
            } else {
                content.push_str(&format!(
                    "BT 0 0 0 rg /{} {} Tf 1 0 0 1 3 {} Tm <{}> Tj ET\n",
                    font_name,
                    number(text_size),
                    number(baseline),
                    hex_text(option, &cids)
                ));
            }
        }
    }
    content.push_str("Q\n");
    content
}

fn signature_appearance_content(
    spec: &SignatureFieldSpec,
    font_name: &str,
    subset: &FontSubset,
) -> String {
    let width = spec.width.max(0.01);
    let height = spec.height.max(0.01);
    let cids = character_cids(subset);
    let (a, b, c, d, e, f) = rotation_matrix(spec.rotation, spec.user_rect);
    let label_size = (height * 0.22).clamp(3.0, 12.0);
    let label_baseline = (height - 2.0 - label_size * 0.8).max(0.0);
    let line_y = (height * 0.3).max(2.0);
    format!(
        "q {} {} {} {} {} {} cm\n\
         1 1 1 rg 0 0 {w} {h} re f\n\
         0 0 0 RG 0.75 w 0.5 0.5 {bw} {bh} re S\n\
         BT 0.35 0.35 0.35 rg /{font} {size} Tf 1 0 0 1 4 {baseline} Tm <{label}> Tj ET\n\
         0.45 0.45 0.45 RG 0.75 w 4 {line_y} m {line_end} {line_y} l S\n\
         Q\n",
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
        font = font_name,
        size = number(label_size),
        baseline = number(label_baseline),
        label = hex_text(SIGNATURE_APPEARANCE_LABEL, &cids),
        line_y = number(line_y),
        line_end = number((width - 4.0).max(4.0)),
    )
}

fn write_choice_appearance(
    document: &mut Document,
    spec: &ChoiceFieldSpec,
    font_name: &str,
    font_id: ObjectId,
    subset: &FontSubset,
    metrics: &FontMetrics,
) -> Result<ObjectId, CommandError> {
    let content = choice_appearance_content(spec, font_name, subset, metrics);
    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
    resources.set(b"Font".to_vec(), Object::Dictionary(fonts));
    write_appearance_stream(document, spec.user_rect, content, Some(resources))
}

fn write_signature_appearance(
    document: &mut Document,
    spec: &SignatureFieldSpec,
    font_name: &str,
    font_id: ObjectId,
    subset: &FontSubset,
) -> Result<ObjectId, CommandError> {
    let content = signature_appearance_content(spec, font_name, subset);
    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
    resources.set(b"Font".to_vec(), Object::Dictionary(fonts));
    write_appearance_stream(document, spec.user_rect, content, Some(resources))
}

fn write_choice_field(
    document: &mut Document,
    spec: &ChoiceFieldSpec,
    page_id: ObjectId,
    appearance_id: ObjectId,
    font_name: &str,
) -> Result<ObjectId, CommandError> {
    let mut flags = 0i64;
    if spec.required {
        flags |= 1 << 1; // Required
    }
    if spec.combo {
        flags |= 1 << 17; // Combo
    } else if spec.multi_select {
        flags |= 1 << 21; // MultiSelect (list boxes only)
    }

    let mut appearance = Dictionary::new();
    appearance.set(b"N".to_vec(), Object::Reference(appearance_id));

    let selected_options: Vec<Object> = spec
        .selected
        .iter()
        .filter_map(|index| spec.options.get(*index))
        .map(|label| text_string(label))
        .collect();
    let option_array: Vec<Object> = spec
        .options
        .iter()
        .map(|label| text_string(label))
        .collect();

    let mut field = dictionary([
        ("Type", name("Annot")),
        ("Subtype", name("Widget")),
        ("FT", name("Ch")),
        ("T", text_string(&spec.name)),
        ("Rect", rect_array(spec.user_rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(flags)),
        ("Opt", Object::Array(option_array)),
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
    if spec.selected.len() == 1 {
        field.set(b"V".to_vec(), text_string(&spec.options[spec.selected[0]]));
    } else if spec.selected.len() > 1 {
        field.set(b"V".to_vec(), Object::Array(selected_options));
    }
    if !spec.selected.is_empty() {
        field.set(
            b"I".to_vec(),
            Object::Array(
                spec.selected
                    .iter()
                    .map(|index| Object::Integer(*index as i64))
                    .collect(),
            ),
        );
    }
    document
        .try_add_object(field)
        .map_err(|error| pdf_error(format!("could not add the choice field: {error}")))
}

fn write_signature_field(
    document: &mut Document,
    spec: &SignatureFieldSpec,
    page_id: ObjectId,
    appearance_id: ObjectId,
) -> Result<ObjectId, CommandError> {
    let mut appearance = Dictionary::new();
    appearance.set(b"N".to_vec(), Object::Reference(appearance_id));

    let mut field = dictionary([
        ("Type", name("Annot")),
        ("Subtype", name("Widget")),
        ("FT", name("Sig")),
        ("T", text_string(&spec.name)),
        ("Rect", rect_array(spec.user_rect)),
        ("P", Object::Reference(page_id)),
        ("F", Object::Integer(4)), // Print
        ("Ff", Object::Integer(0)),
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
    // Deliberately no /V, /SV, /Lock or /AA: this is an empty placeholder and
    // PDFX never signs it.
    document
        .try_add_object(field)
        .map_err(|error| pdf_error(format!("could not add the signature field: {error}")))
}

/// Resolves the existing fields whose appearances must be rebuilt: widget
/// rectangles, page rotation, `/DA` font size, flags and choice options.
fn prepare_existing_appearances(
    incremental: &IncrementalDocument,
    appearances: &[ExistingFieldAppearance],
) -> Result<Vec<PreparedAppearance>, CommandError> {
    let acro_form_da = crate::save::acro_form_any(incremental)
        .and_then(|acro_form| acro_form.get(b"DA").ok())
        .and_then(|da| da.as_str().ok())
        .map(crate::save::decode_pdf_text);
    let mut prepared = Vec::with_capacity(appearances.len());
    for appearance in appearances {
        let found = crate::save::find_form_field_any(incremental, &appearance.field_name)
            .ok_or_else(|| {
                pdf_error(format!(
                    "could not find the field {:?} in the saved document",
                    appearance.field_name
                ))
            })?;
        let field_id = found.field_id;
        let mut widget_ids = found.widget_ids;
        if widget_ids.is_empty() {
            // A merged field is its own widget.
            widget_ids.push(field_id);
        }
        let flags = inherited_field_entry(incremental, field_id, b"Ff")
            .and_then(|flags| flags.as_i64().ok())
            .unwrap_or(0);
        let font_size = widget_da(incremental, &widget_ids)
            .or_else(|| {
                inherited_field_entry(incremental, field_id, b"DA")
                    .and_then(|da| da.as_str().ok().map(crate::save::decode_pdf_text))
            })
            .or_else(|| acro_form_da.clone())
            .as_deref()
            .and_then(font_size_from_da)
            .unwrap_or(DEFAULT_FONT_SIZE);
        let widgets: Vec<PreparedWidget> = widget_ids
            .iter()
            .filter_map(|id| prepared_widget(incremental, *id, field_id))
            .collect();
        let kind = match &appearance.value {
            ExistingAppearanceValue::Text(text) => PreparedKind::Text {
                value: text.clone(),
                multiline: flags & (1 << 12) != 0,
                font_size,
            },
            ExistingAppearanceValue::Checked(checked) => {
                let on_state = widgets
                    .iter()
                    .find_map(|widget| widget.on_state.clone())
                    .unwrap_or_else(|| CHECKBOX_ON_STATE.as_bytes().to_vec());
                PreparedKind::Checkbox {
                    checked: *checked,
                    on_state,
                }
            }
            ExistingAppearanceValue::Radio(state) => PreparedKind::Radio {
                selected: state.clone(),
            },
            ExistingAppearanceValue::Choice(label) => {
                let options = inherited_field_entry(incremental, field_id, b"Opt")
                    .and_then(|options| {
                        crate::save::resolve_object_any(incremental, &options).cloned()
                    })
                    .and_then(|options| options.as_array().ok().cloned())
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(crate::save::option_label)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let selected = options
                    .iter()
                    .enumerate()
                    .filter(|(_, option)| *option == label)
                    .map(|(index, _)| index)
                    .collect();
                PreparedKind::Choice {
                    options,
                    selected,
                    combo: flags & (1 << 17) != 0,
                    font_size,
                }
            }
        };
        prepared.push(PreparedAppearance { widgets, kind });
    }
    Ok(prepared)
}

/// A field entry, falling back to the nearest ancestor that defines it
/// (PDF 32000-1 Table 220: `/FT`, `/Ff`, `/DA` and `/Opt` are inheritable).
fn inherited_field_entry(
    incremental: &IncrementalDocument,
    field_id: ObjectId,
    key: &[u8],
) -> Option<Object> {
    let mut next = Some(field_id);
    for _ in 0..64 {
        let dictionary = crate::save::dictionary_any(incremental, next?)?;
        if let Ok(value) = dictionary.get(key) {
            return Some(value.clone());
        }
        next = dictionary
            .get(b"Parent")
            .ok()
            .and_then(|parent| parent.as_reference().ok());
    }
    None
}

/// The widget-level `/DA`, which overrides the field and AcroForm defaults.
fn widget_da(incremental: &IncrementalDocument, widget_ids: &[ObjectId]) -> Option<String> {
    for id in widget_ids {
        if let Some(da) = crate::save::dictionary_any(incremental, *id)
            .and_then(|widget| widget.get(b"DA").ok())
            .and_then(|da| da.as_str().ok())
        {
            return Some(crate::save::decode_pdf_text(da));
        }
    }
    None
}

/// Reads the font size from a `/DA` string (`/Helv 12 Tf 0 g`). A size of 0
/// means auto-sizing, which this writer does not implement, so it falls back
/// to the default size.
fn font_size_from_da(da: &str) -> Option<f32> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    tokens
        .windows(3)
        .find(|window| window[2] == "Tf")
        .and_then(|window| window[1].parse::<f32>().ok())
        .filter(|size| *size > 0.0)
}

fn prepared_widget(
    incremental: &IncrementalDocument,
    widget_id: ObjectId,
    field_id: ObjectId,
) -> Option<PreparedWidget> {
    let widget = crate::save::dictionary_any(incremental, widget_id)?;
    let rect = widget
        .get(b"Rect")
        .ok()
        .and_then(|rect| crate::save::resolve_object_any(incremental, rect))
        .and_then(|rect| rect.as_array().ok())?;
    if rect.len() < 4 {
        return None;
    }
    let mut values = [0.0_f32; 4];
    for (value, object) in values.iter_mut().zip(rect.iter()) {
        *value = object.as_float().ok()?;
    }
    let [x0, y0, x1, y1] = normalized_rect(values);
    let rotation = annotation_rotation(incremental, widget, field_id);
    let (width, height) = if rotation % 180 == 90 {
        (y1 - y0, x1 - x0)
    } else {
        (x1 - x0, y1 - y0)
    };
    Some(PreparedWidget {
        id: widget_id,
        user_rect: [x0, y0, x1, y1],
        width,
        height,
        rotation,
        on_state: crate::save::widget_on_state_any(incremental, widget),
    })
}

/// The page's intrinsic rotation, inherited through the page tree. Widgets
/// without a resolvable page fall back to an unrotated appearance.
fn annotation_rotation(
    incremental: &IncrementalDocument,
    widget: &Dictionary,
    field_id: ObjectId,
) -> u16 {
    let page_id = widget
        .get(b"P")
        .ok()
        .and_then(|page| page.as_reference().ok())
        .or_else(|| {
            crate::save::dictionary_any(incremental, field_id)
                .and_then(|field| field.get(b"P").ok())
                .and_then(|page| page.as_reference().ok())
        });
    let mut next = page_id;
    for _ in 0..64 {
        let Some(id) = next else { break };
        let Some(dictionary) = crate::save::dictionary_any(incremental, id) else {
            break;
        };
        if let Ok(rotation) = dictionary.get(b"Rotate").and_then(Object::as_i64) {
            return ((rotation % 360 + 360) % 360) as u16;
        }
        next = dictionary
            .get(b"Parent")
            .ok()
            .and_then(|parent| parent.as_reference().ok());
    }
    0
}

/// The shared font subset and the name it is registered under in `/DR`.
struct AppearanceFont<'a> {
    name: &'a str,
    id: ObjectId,
    subset: &'a FontSubset,
    metrics: &'a FontMetrics,
}

impl AppearanceFont<'_> {
    fn resources(&self) -> Dictionary {
        let mut fonts = Dictionary::new();
        fonts.set(self.name.as_bytes().to_vec(), Object::Reference(self.id));
        let mut resources = Dictionary::new();
        resources.set(b"Font".to_vec(), Object::Dictionary(fonts));
        resources
    }
}

/// Writes a fresh `/AP /N` for every widget of every filled existing field.
///
/// The value itself was already written by the `Set*Value` object edits; this
/// pass only replaces appearances, so a reader that draws `/AP` agrees with
/// `/V`, `/AS` and `/I`.
fn write_existing_appearances(
    incremental: &mut IncrementalDocument,
    prepared: &[PreparedAppearance],
    font: Option<&AppearanceFont<'_>>,
) -> Result<(), CommandError> {
    for appearance in prepared {
        for widget in &appearance.widgets {
            let (normal, state) = match &appearance.kind {
                PreparedKind::Text {
                    value,
                    multiline,
                    font_size,
                } => {
                    let font = font.ok_or_else(|| {
                        pdf_error("a text appearance needs an embedded font subset")
                    })?;
                    let spec = TextFieldSpec {
                        page_index: 0,
                        name: String::new(),
                        tooltip: None,
                        required: false,
                        multiline: *multiline,
                        font_size: *font_size,
                        user_rect: widget.user_rect,
                        width: widget.width,
                        height: widget.height,
                        value: value.clone(),
                        rotation: widget.rotation,
                    };
                    let content =
                        text_appearance_content(&spec, font.name, font.subset, font.metrics);
                    let appearance_id = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        content,
                        Some(font.resources()),
                    )?;
                    (Object::Reference(appearance_id), None)
                }
                PreparedKind::Choice {
                    options,
                    selected,
                    combo,
                    font_size,
                } => {
                    let font = font.ok_or_else(|| {
                        pdf_error("a choice appearance needs an embedded font subset")
                    })?;
                    let spec = ChoiceFieldSpec {
                        page_index: 0,
                        name: String::new(),
                        tooltip: None,
                        required: false,
                        combo: *combo,
                        multi_select: false,
                        font_size: *font_size,
                        options: options.clone(),
                        selected: selected.clone(),
                        user_rect: widget.user_rect,
                        width: widget.width,
                        height: widget.height,
                        rotation: widget.rotation,
                    };
                    let content =
                        choice_appearance_content(&spec, font.name, font.subset, font.metrics);
                    let appearance_id = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        content,
                        Some(font.resources()),
                    )?;
                    (Object::Reference(appearance_id), None)
                }
                PreparedKind::Checkbox { checked, on_state } => {
                    let off = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        checkbox_appearance_content(
                            widget.rotation,
                            widget.user_rect,
                            widget.width,
                            widget.height,
                            false,
                        ),
                        None,
                    )?;
                    let on = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        checkbox_appearance_content(
                            widget.rotation,
                            widget.user_rect,
                            widget.width,
                            widget.height,
                            true,
                        ),
                        None,
                    )?;
                    let normal =
                        state_appearance(&[(b"Off".as_slice(), off), (on_state.as_slice(), on)]);
                    let state = if *checked {
                        on_state.clone()
                    } else {
                        b"Off".to_vec()
                    };
                    (normal, Some(state))
                }
                PreparedKind::Radio { selected } => {
                    let on_state = widget
                        .on_state
                        .clone()
                        .unwrap_or_else(|| selected.as_bytes().to_vec());
                    let off = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        radio_appearance_content(
                            widget.rotation,
                            widget.user_rect,
                            widget.width,
                            widget.height,
                            false,
                        ),
                        None,
                    )?;
                    let on = write_appearance_stream(
                        &mut incremental.new_document,
                        widget.user_rect,
                        radio_appearance_content(
                            widget.rotation,
                            widget.user_rect,
                            widget.width,
                            widget.height,
                            true,
                        ),
                        None,
                    )?;
                    let normal =
                        state_appearance(&[(b"Off".as_slice(), off), (on_state.as_slice(), on)]);
                    let state = if selected.as_bytes() == on_state.as_slice() {
                        on_state
                    } else {
                        b"Off".to_vec()
                    };
                    (normal, Some(state))
                }
            };
            apply_widget_appearance(incremental, widget.id, normal, state)?;
        }
    }
    Ok(())
}

fn state_appearance(states: &[(&[u8], ObjectId)]) -> Object {
    let mut normal = Dictionary::new();
    for (state, id) in states {
        normal.set(state.to_vec(), Object::Reference(*id));
    }
    Object::Dictionary(normal)
}

fn apply_widget_appearance(
    incremental: &mut IncrementalDocument,
    widget_id: ObjectId,
    normal: Object,
    state: Option<Vec<u8>>,
) -> Result<(), CommandError> {
    incremental
        .opt_clone_object_to_new_document(widget_id)
        .map_err(|error| pdf_error(format!("could not clone a filled widget: {error}")))?;
    let widget = incremental
        .new_document
        .get_dictionary_mut(widget_id)
        .map_err(|error| pdf_error(format!("could not edit a filled widget: {error}")))?;
    let mut appearance = Dictionary::new();
    appearance.set(b"N".to_vec(), normal);
    widget.set(b"AP".to_vec(), Object::Dictionary(appearance));
    if let Some(state) = state {
        widget.set(b"AS".to_vec(), Object::Name(state));
    }
    Ok(())
}

/// Writes every created field into the in-progress incremental revision.
///
/// This is the only writer of AcroForm structures: the catalog, `/AcroForm`,
/// each target page and — when text fields exist — the font objects are
/// cloned/added here, then `/Fields`, `/DR`, `/DA` (when missing) and page
/// `/Annots` are updated once. Existing form entries, values and fonts are
/// preserved, and `/NeedAppearances` is never set.
pub(crate) fn apply_created_fields(
    incremental: &mut IncrementalDocument,
    fields: &[FormFieldSpec],
    appearances: &[ExistingFieldAppearance],
) -> Result<(), CommandError> {
    if fields.is_empty() && appearances.is_empty() {
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
                pages.get(&(spec.page_index() + 1)).copied().ok_or_else(|| {
                    pdf_error(format!(
                        "page {} does not exist in the saved document",
                        spec.page_index() + 1
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

    let prepared = prepare_existing_appearances(incremental, appearances)?;

    let mut font_text: Vec<&str> = fields
        .iter()
        .flat_map(|spec| match spec {
            FormFieldSpec::Text(text) => vec![text.value.as_str()],
            FormFieldSpec::Choice(choice) => choice.options.iter().map(String::as_str).collect(),
            FormFieldSpec::Signature(_) => vec![SIGNATURE_APPEARANCE_LABEL],
            FormFieldSpec::Checkbox(_) | FormFieldSpec::Radio(_) => Vec::new(),
        })
        .collect();
    for appearance in &prepared {
        match &appearance.kind {
            PreparedKind::Text { value, .. } => font_text.push(value),
            PreparedKind::Choice { options, .. } => {
                font_text.extend(options.iter().map(String::as_str));
            }
            PreparedKind::Checkbox { .. } | PreparedKind::Radio { .. } => {}
        }
    }
    let font_text = font_text.join(" ");
    let has_font = fields.iter().any(|spec| {
        matches!(
            spec,
            FormFieldSpec::Text(_) | FormFieldSpec::Choice(_) | FormFieldSpec::Signature(_)
        )
    }) || prepared.iter().any(|appearance| {
        matches!(
            appearance.kind,
            PreparedKind::Text { .. } | PreparedKind::Choice { .. }
        )
    });
    let has_signature = fields
        .iter()
        .any(|spec| matches!(spec, FormFieldSpec::Signature(_)));
    let has_default_da = acroform_dict(&incremental.new_document, acroform_id, root_id)?.has(b"DA");

    // One subset covers every value, option label and the signature label, so
    // choices and empty signatures stay readable in every viewer without a
    // system font. Empty text values still contribute the space glyph.
    let font = if has_font {
        let (mut dr, mut fonts) = read_dr(&incremental.new_document, acroform_id, root_id)?;
        let subset =
            crate::fonts::noto_sans_sc().subset(&font_text, crate::fonts::weight_for(false))?;
        let metrics = FontMetrics::new(&subset)?;
        let font_name = unique_font_name(&fonts);
        let font_id =
            write_type0_font(&mut incremental.new_document, &subset, &font_name, &metrics)?;
        fonts.set(font_name.as_bytes().to_vec(), Object::Reference(font_id));
        dr.set(b"Font".to_vec(), Object::Dictionary(fonts));
        Some((font_name, font_id, subset, metrics, dr))
    } else {
        None
    };

    let mut new_field_refs = Vec::with_capacity(fields.len());
    let mut page_widgets: BTreeMap<ObjectId, Vec<Object>> = BTreeMap::new();
    for (spec, page_id) in fields.iter().zip(page_ids.iter().copied()) {
        match spec {
            FormFieldSpec::Text(text) => {
                let (font_name, font_id, subset, metrics, _) = font
                    .as_ref()
                    .expect("a text field implies an embedded font");
                let appearance_id = write_text_appearance(
                    &mut incremental.new_document,
                    text,
                    font_name,
                    *font_id,
                    subset,
                    metrics,
                )?;
                let field_id = write_text_field(
                    &mut incremental.new_document,
                    text,
                    page_id,
                    appearance_id,
                    font_name,
                )?;
                new_field_refs.push(Object::Reference(field_id));
                page_widgets
                    .entry(page_id)
                    .or_default()
                    .push(Object::Reference(field_id));
            }
            FormFieldSpec::Checkbox(checkbox) => {
                let field_id = write_checkbox(&mut incremental.new_document, checkbox, page_id)?;
                new_field_refs.push(Object::Reference(field_id));
                page_widgets
                    .entry(page_id)
                    .or_default()
                    .push(Object::Reference(field_id));
            }
            FormFieldSpec::Radio(group) => {
                let (parent_id, kid_ids) =
                    write_radio_group(&mut incremental.new_document, group, page_id)?;
                new_field_refs.push(Object::Reference(parent_id));
                page_widgets.entry(page_id).or_default().extend(
                    kid_ids
                        .iter()
                        .map(|id| Object::Reference(*id))
                        .collect::<Vec<_>>(),
                );
            }
            FormFieldSpec::Choice(choice) => {
                let (font_name, font_id, subset, metrics, _) = font
                    .as_ref()
                    .expect("a choice field implies an embedded font");
                let appearance_id = write_choice_appearance(
                    &mut incremental.new_document,
                    choice,
                    font_name,
                    *font_id,
                    subset,
                    metrics,
                )?;
                let field_id = write_choice_field(
                    &mut incremental.new_document,
                    choice,
                    page_id,
                    appearance_id,
                    font_name,
                )?;
                new_field_refs.push(Object::Reference(field_id));
                page_widgets
                    .entry(page_id)
                    .or_default()
                    .push(Object::Reference(field_id));
            }
            FormFieldSpec::Signature(signature) => {
                let (font_name, font_id, subset, _, _) = font
                    .as_ref()
                    .expect("a signature field implies an embedded font");
                let appearance_id = write_signature_appearance(
                    &mut incremental.new_document,
                    signature,
                    font_name,
                    *font_id,
                    subset,
                )?;
                let field_id = write_signature_field(
                    &mut incremental.new_document,
                    signature,
                    page_id,
                    appearance_id,
                )?;
                new_field_refs.push(Object::Reference(field_id));
                page_widgets
                    .entry(page_id)
                    .or_default()
                    .push(Object::Reference(field_id));
            }
        }
    }

    if !prepared.is_empty() {
        let appearance_font = font
            .as_ref()
            .map(|(name, id, subset, metrics, _)| AppearanceFont {
                name: name.as_str(),
                id: *id,
                subset,
                metrics,
            });
        write_existing_appearances(incremental, &prepared, appearance_font.as_ref())?;
    }

    {
        let mut all_fields = existing_fields;
        all_fields.extend(new_field_refs);
        let acroform = acroform_dict_mut(&mut incremental.new_document, acroform_id, root_id)?;
        acroform.set(b"Fields".to_vec(), Object::Array(all_fields));
        if let Some((font_name, _, _, _, dr)) = font {
            acroform.set(b"DR".to_vec(), Object::Dictionary(dr));
            if !has_default_da {
                acroform.set(
                    b"DA".to_vec(),
                    text_string(&format!("/{font_name} 0 Tf 0 g")),
                );
            }
        }
        if has_signature {
            // Bit 1: SignaturesExist, saying the document has a signature
            // field. Existing bits are preserved; AppendOnly (bit 2) is left
            // alone because PDFX never signs and never rewrites signed data.
            let existing = acroform
                .get(b"SigFlags")
                .ok()
                .and_then(|object| object.as_i64().ok())
                .unwrap_or(0);
            acroform.set(b"SigFlags".to_vec(), Object::Integer(existing | 1));
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
