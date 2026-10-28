//! Interactive AcroForm fields: discovery, filling and required-field checks.
//!
//! PDFium reads and writes the existing field objects; the module never
//! executes calculations or JavaScript actions. Values are applied to the
//! widget annotations of the live document so the ticket-10 save pipeline can
//! append them as PDFium edits.

use pdfium_render::prelude::{
    PdfDocument, PdfFormField, PdfFormFieldCommon, PdfPageAnnotation, PdfPageAnnotationCommon,
};
use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::text_edit::{
    EditAction, EditHistoryView, EditSession, FieldEditAction, FieldValueSnapshot,
};

/// The widget types PDFX can display. Push buttons and signatures are
/// discovered so the overlay can show them, but they cannot be filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormFieldKind {
    Text,
    Checkbox,
    Radio,
    ComboBox,
    ListBox,
    PushButton,
    Signature,
    Unknown,
}

/// A widget rectangle in display-space points (top-left origin, rotation and
/// crop already applied), matching the text layer and other overlays.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One interactive widget as seen by the frontend.
///
/// A radio group has one view per widget; all widgets share `name` and
/// `radio_group`, while `value` is the group's selected export state and
/// `checked` tells whether this particular widget is the selected one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldView {
    /// Stable within an open document: `"<page-index>:<annotation-index>"`.
    pub field_id: String,
    pub name: String,
    pub kind: FormFieldKind,
    pub page_index: u32,
    /// Zero-based position in document-wide tab order.
    pub order: u32,
    pub rect: FormFieldRect,
    /// Text value, radio-group value or selected choice label.
    pub value: Option<String>,
    /// Checkbox/radio state; always false for other kinds.
    pub checked: bool,
    /// Choice labels in document order; empty for other kinds.
    pub options: Vec<String>,
    /// Index into `options` for choice fields.
    pub selected_index: Option<u32>,
    pub required: bool,
    pub read_only: bool,
    pub multiline: bool,
    /// The shared field name for radio widgets.
    pub radio_group: Option<String>,
    /// The widget's export value for radio widgets.
    pub export_value: Option<String>,
    /// `/TU` tooltip, when the field has one.
    pub tooltip: Option<String>,
    /// Font size from the field's `/DA`, when it can be read.
    pub font_size: Option<f32>,
    /// True for a field created by this session; only those fields expose the
    /// property panel and can be deleted before saving.
    pub created: bool,
}

fn read_error(context: &str, error: impl std::fmt::Debug) -> CommandError {
    CommandError::new(
        CommandErrorCode::RenderFailed,
        format!("could not read form fields: {context}: {error:?}"),
    )
}

fn kind_of(field: &PdfFormField<'_>) -> FormFieldKind {
    match field {
        PdfFormField::Text(_) => FormFieldKind::Text,
        PdfFormField::Checkbox(_) => FormFieldKind::Checkbox,
        PdfFormField::RadioButton(_) => FormFieldKind::Radio,
        PdfFormField::ComboBox(_) => FormFieldKind::ComboBox,
        PdfFormField::ListBox(_) => FormFieldKind::ListBox,
        PdfFormField::PushButton(_) => FormFieldKind::PushButton,
        PdfFormField::Signature(_) => FormFieldKind::Signature,
        PdfFormField::Unknown(_) => FormFieldKind::Unknown,
    }
}

/// Fills the kind-specific parts of a widget view.
///
/// `radio_state` is the widget's on-state name read from the PDF structure.
/// Pdfium's export-value API returns nothing for this fixture's appearance
/// dictionaries, so the on-state is read with lopdf instead.
fn describe_value(
    field: &PdfFormField<'_>,
    view: &mut FormFieldView,
    radio_state: Option<String>,
    overlay: &FormOverlay,
) {
    match field {
        PdfFormField::Text(text) => {
            view.value = text.value();
            view.multiline = text.is_multiline();
        }
        PdfFormField::Checkbox(checkbox) => {
            view.checked = checkbox.is_checked().unwrap_or(false);
        }
        PdfFormField::RadioButton(radio) => {
            // Pdfium treats "Off" as matching the /Off appearance state, so its
            // own is_checked() reports every widget in an untouched group as
            // checked. Compare the group value with this widget's on-state.
            let group_value = radio.group_value().filter(|value| value != "Off");
            let effective = overlay.radio.get(&view.name).cloned().or(group_value);
            view.export_value = radio_state.or_else(|| radio.export_value());
            view.checked = match (&effective, &view.export_value) {
                (Some(value), Some(state)) => value == state,
                _ => {
                    effective.is_some()
                        && effective.as_deref() == radio.appearance_stream().as_deref()
                }
            };
            view.value = effective;
            view.radio_group = Some(view.name.clone());
        }
        PdfFormField::ComboBox(combo) => {
            describe_choice(combo.options(), view);
            view.value = combo.value();
            apply_choice_overlay(view, overlay);
            sync_choice_index(view);
        }
        PdfFormField::ListBox(list) => {
            describe_choice(list.options(), view);
            view.value = list.value();
            apply_choice_overlay(view, overlay);
            sync_choice_index(view);
        }
        PdfFormField::PushButton(_) | PdfFormField::Signature(_) | PdfFormField::Unknown(_) => {}
    }
}

fn sync_choice_index(view: &mut FormFieldView) {
    if let Some(value) = &view.value {
        if let Some(index) = view.options.iter().position(|option| option == value) {
            view.selected_index = Some(index as u32);
        }
    }
}

fn apply_choice_overlay(view: &mut FormFieldView, overlay: &FormOverlay) {
    let Some(value) = overlay.choice.get(&view.name) else {
        return;
    };
    view.value = Some(value.clone());
    view.selected_index = view
        .options
        .iter()
        .position(|option| option == value)
        .map(|index| index as u32);
}

fn describe_choice(
    options: &pdfium_render::prelude::PdfFormFieldOptions<'_>,
    view: &mut FormFieldView,
) {
    view.options = options
        .iter()
        .filter_map(|option| option.label().cloned())
        .collect();
    view.selected_index = options
        .iter()
        .enumerate()
        .find(|(_, option)| option.is_set())
        .map(|(index, _)| index as u32);
}

/// On-state names of every radio widget, keyed by field name and in `/Kids`
/// order: `"gender" -> [Some("male"), Some("female"), Some("other")]`.
pub(crate) type RadioStates = std::collections::HashMap<String, Vec<Option<String>>>;

fn dictionary_of<'a>(
    object: &'a lopdf::Object,
    document: &'a lopdf::Document,
) -> Option<&'a lopdf::Dictionary> {
    match object {
        lopdf::Object::Reference(id) => document.get_dictionary(*id).ok(),
        lopdf::Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

/// Returns the non-Off key of a widget's normal appearance dictionary.
fn normal_appearance_state(
    widget: &lopdf::Dictionary,
    document: &lopdf::Document,
) -> Option<String> {
    let normal = widget
        .get_deref(b"AP", document)
        .ok()?
        .as_dict()
        .ok()?
        .get_deref(b"N", document)
        .ok()?;
    normal
        .as_dict()
        .ok()?
        .iter()
        .find(|(state, _)| state.as_slice() != b"Off")
        .map(|(state, _)| String::from_utf8_lossy(state).into_owned())
}

/// Reads radio on-state names straight from the PDF structure.
///
/// Pdfium's `FPDFAnnot_GetFormFieldExportValue()` returns nothing for these
/// widgets, but selecting a radio widget needs the state name that belongs to
/// it. This reader is best-effort: a document that cannot be parsed still
/// opens, its radio widgets just fall back to Pdfium's group value.
pub(crate) fn radio_states(source_bytes: &[u8]) -> RadioStates {
    let mut states = RadioStates::new();
    let Ok(document) = lopdf::Document::load_mem(source_bytes) else {
        return states;
    };
    let Ok(acro_form) = document
        .catalog()
        .and_then(|catalog| catalog.get_deref(b"AcroForm", &document))
        .and_then(lopdf::Object::as_dict)
    else {
        return states;
    };
    let Ok(fields) = acro_form
        .get_deref(b"Fields", &document)
        .and_then(lopdf::Object::as_array)
    else {
        return states;
    };

    for field_object in fields {
        let Some(field) = dictionary_of(field_object, &document) else {
            continue;
        };
        let flags = field
            .get(b"Ff")
            .and_then(lopdf::Object::as_i64)
            .unwrap_or(0);
        if flags & 32768 == 0 {
            continue;
        }
        let Some(name) = field
            .get(b"T")
            .and_then(lopdf::Object::as_str)
            .ok()
            .map(|name| String::from_utf8_lossy(name).into_owned())
        else {
            continue;
        };
        let widgets: Vec<&lopdf::Dictionary> = match field
            .get_deref(b"Kids", &document)
            .ok()
            .and_then(|kids| kids.as_array().ok())
        {
            Some(kids) => kids
                .iter()
                .filter_map(|kid| dictionary_of(kid, &document))
                .collect(),
            None => vec![field],
        };
        states.insert(
            name,
            widgets
                .iter()
                .map(|widget| normal_appearance_state(widget, &document))
                .collect(),
        );
    }

    states
}

/// Form values that live in session history rather than in the PDFium document.
///
/// Pdfium's public API can set a single widget annotation's `/V`, but a radio
/// group stores its value on the parent field and choice fields expose no
/// setter at all. Those values are recorded by the unified edit history and
/// written as object-level edits by the save pipeline; discovery overlays them
/// on top of what Pdfium reports.
#[derive(Debug, Default, Clone)]
pub(crate) struct FormOverlay {
    /// Radio group name -> selected on-state name.
    pub(crate) radio: std::collections::HashMap<String, String>,
    /// Choice field name -> selected label.
    pub(crate) choice: std::collections::HashMap<String, String>,
}

/// Derives the overlay from the actions currently applied to the document.
///
/// Walking the undo stack is what makes undo/redo work without a second copy
/// of form state: the last applied action per field wins.
pub(crate) fn overlay_for(edit: &EditSession) -> FormOverlay {
    let mut overlay = FormOverlay::default();
    for entry in &edit.undo_stack {
        let EditAction::Field(action) = entry else {
            continue;
        };
        if let Some(group) = &action.radio_group {
            if let FieldValueSnapshot::Radio(state) = &action.after {
                match state {
                    Some(state) => {
                        overlay.radio.insert(group.clone(), state.clone());
                    }
                    None => {
                        overlay.radio.remove(group);
                    }
                }
            }
        }
        if let FieldValueSnapshot::Choice(value) = &action.after {
            if value.is_empty() {
                overlay.choice.remove(&action.field_name);
            } else {
                overlay
                    .choice
                    .insert(action.field_name.clone(), value.clone());
            }
        }
    }
    overlay
}

/// Object-level edits the save pipeline appends for form values that live in
/// the history overlay rather than in the PDFium document.
pub(crate) fn save_object_edits(edit: &EditSession) -> Vec<crate::save::ObjectEdit> {
    let overlay = overlay_for(edit);
    let mut edits = Vec::new();

    let mut groups: Vec<_> = overlay.radio.into_iter().collect();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    for (field_name, state) in groups {
        edits.push(crate::save::ObjectEdit::SetRadioGroup { field_name, state });
    }

    let mut choices: Vec<_> = overlay.choice.into_iter().collect();
    choices.sort_by(|left, right| left.0.cmp(&right.0));
    for (field_name, label) in choices {
        edits.push(crate::save::ObjectEdit::SetChoiceValue { field_name, label });
    }

    edits
}

/// Hands out radio on-states in the order radio widgets are discovered, which
/// matches `/Kids` order in a well-formed form.
struct RadioCursor<'a> {
    states: &'a RadioStates,
    used: std::collections::HashMap<String, usize>,
}

impl RadioCursor<'_> {
    fn next(&mut self, name: &str) -> Option<String> {
        let states = self.states.get(name)?;
        let used = self.used.entry(name.to_owned()).or_insert(0);
        let state = states.get(*used).cloned().flatten();
        *used += 1;
        state
    }
}

fn widget_view(
    annotation: &PdfPageAnnotation<'_>,
    page_index: u32,
    annotation_index: usize,
    order: u32,
    geometry: &DisplayGeometry,
    radio_cursor: &mut RadioCursor<'_>,
    overlay: &FormOverlay,
) -> Result<Option<FormFieldView>, CommandError> {
    let Some(field) = annotation.as_form_field() else {
        return Ok(None);
    };

    let bounds = annotation
        .bounds()
        .map_err(|error| read_error(&format!("widget {page_index}:{annotation_index}"), error))?;
    let (x, y, width, height) = geometry.rect(
        bounds.left().value,
        bounds.bottom().value,
        bounds.right().value,
        bounds.top().value,
    );

    let name = field.name().unwrap_or_default();
    let mut view = FormFieldView {
        field_id: format!("{page_index}:{annotation_index}"),
        name,
        kind: kind_of(field),
        page_index,
        order,
        rect: FormFieldRect {
            x,
            y,
            width,
            height,
        },
        value: None,
        checked: false,
        options: Vec::new(),
        selected_index: None,
        required: field.is_required(),
        read_only: field.is_read_only(),
        multiline: false,
        radio_group: None,
        export_value: None,
        tooltip: None,
        font_size: None,
        created: false,
    };
    let radio_state = match field {
        PdfFormField::RadioButton(_) => radio_cursor.next(&view.name),
        _ => None,
    };
    describe_value(field, &mut view, radio_state, overlay);
    Ok(Some(view))
}

/// Discovers every interactive widget in document order: page by page, then
/// each page's annotation order. This is the tab order the UI uses.
pub(crate) fn discover_fields(
    document: &PdfDocument<'static>,
    radio_states: &RadioStates,
    overlay: &FormOverlay,
) -> Result<Vec<FormFieldView>, CommandError> {
    let page_count = document.pages().len();
    let mut fields = Vec::new();
    let mut order = 0u32;
    let mut radio_cursor = RadioCursor {
        states: radio_states,
        used: std::collections::HashMap::new(),
    };

    for page_index in 0..page_count.max(0) as u32 {
        let page = load_page(document, page_index)?;
        let rotation = page
            .rotation()
            .map_err(|error| read_error(&format!("page {page_index} rotation"), error))?
            .as_degrees() as u16;
        let geometry = DisplayGeometry::new(&page, rotation)?;
        let annotations = page.annotations();

        for annotation_index in 0..annotations.len() {
            let annotation = annotations.get(annotation_index).map_err(|error| {
                read_error(
                    &format!("annotation {page_index}:{annotation_index}"),
                    error,
                )
            })?;
            if let Some(view) = widget_view(
                &annotation,
                page_index,
                annotation_index,
                order,
                &geometry,
                &mut radio_cursor,
                overlay,
            )? {
                fields.push(view);
                order += 1;
            }
        }
    }

    Ok(fields)
}

/// Discovers PDFium widgets and merges the text fields this session created
/// (which do not exist in the PDFium document until the next save).
pub(crate) fn discover_all(entry: &OpenDocument) -> Result<Vec<FormFieldView>, CommandError> {
    let existing = discover_fields(
        &entry.document,
        &radio_states(&entry.source_bytes),
        &overlay_for(&entry.edit),
    )?;
    Ok(crate::form_create::merge_views(
        existing,
        &entry.edit.created_fields,
    ))
}

/// The `form_fields` command. Returns every fillable or displayable widget.
#[tauri::command]
pub fn form_fields(document_id: String) -> Result<Vec<FormFieldView>, CommandError> {
    with_open_document(&document_id, |entry| discover_all(entry))
}

fn text_is_blank(value: Option<&str>) -> bool {
    value.map(str::trim).unwrap_or("").is_empty()
}

/// Selects the required fields that still need an answer.
///
/// Read-only fields are excluded: PDFX cannot change them, so blocking a save
/// on one would leave the user with no way forward. A radio group is reported
/// once, through its first widget in tab order.
pub(crate) fn required_missing(fields: &[FormFieldView]) -> Vec<FormFieldView> {
    let mut seen_radio_groups = std::collections::HashSet::new();
    fields
        .iter()
        .filter(|field| {
            if !field.required || field.read_only {
                return false;
            }
            match field.kind {
                FormFieldKind::Text => text_is_blank(field.value.as_deref()),
                FormFieldKind::Checkbox => !field.checked,
                FormFieldKind::Radio => {
                    if !seen_radio_groups.insert(field.name.clone()) {
                        return false;
                    }
                    field.value.is_none()
                }
                FormFieldKind::ComboBox | FormFieldKind::ListBox => {
                    text_is_blank(field.value.as_deref())
                }
                FormFieldKind::PushButton | FormFieldKind::Signature | FormFieldKind::Unknown => {
                    false
                }
            }
        })
        .cloned()
        .collect()
}

/// The `missing_required_fields` command. The frontend calls it before saving
/// so it can name and focus the first empty required field.
#[tauri::command]
pub fn missing_required_fields(document_id: String) -> Result<Vec<FormFieldView>, CommandError> {
    with_open_document(&document_id, |entry| {
        Ok(required_missing(&discover_all(entry)?))
    })
}

/// A value the user committed for one widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FormFieldValueInput {
    Text { text: String },
    Checked { checked: bool },
    Choice { value: String },
}

/// The outcome of one field change: all fields refreshed, plus unified history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldEditResultView {
    pub field_id: String,
    pub fields: Vec<FormFieldView>,
    pub history: EditHistoryView,
}

/// Refuses form changes when the Permission Restrictions deny them.
pub(crate) fn ensure_forms_editable(document: &PdfDocument<'_>) -> Result<(), CommandError> {
    let allowed = document
        .permissions()
        .can_fill_existing_interactive_form_fields()
        .unwrap_or(false);
    if allowed {
        Ok(())
    } else {
        Err(CommandError::new(
            CommandErrorCode::FormFillNotPermitted,
            "this document does not allow filling existing form fields",
        ))
    }
}

fn field_not_found(field_id: &str) -> CommandError {
    CommandError::new(
        CommandErrorCode::FormFieldNotFound,
        format!("no form widget with id {field_id}"),
    )
}

fn invalid_value(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::FormValueInvalid, message)
}

fn parse_field_id(field_id: &str) -> Result<(u32, usize), CommandError> {
    let (page, annotation) = field_id
        .split_once(':')
        .ok_or_else(|| field_not_found(field_id))?;
    let page = page.parse::<u32>().map_err(|_| field_not_found(field_id))?;
    let annotation = annotation
        .parse::<usize>()
        .map_err(|_| field_not_found(field_id))?;
    Ok((page, annotation))
}

/// Runs `action` against the widget's form field, keeping the owning page alive.
fn with_field<R>(
    document: &PdfDocument<'static>,
    field_id: &str,
    action: impl FnOnce(&mut PdfFormField<'_>) -> Result<R, CommandError>,
) -> Result<R, CommandError> {
    let (page_index, annotation_index) = parse_field_id(field_id)?;
    let page = load_page(document, page_index)?;
    let annotations = page.annotations();
    let mut annotation = annotations
        .get(annotation_index)
        .map_err(|_| field_not_found(field_id))?;
    if annotation.as_form_field().is_none() {
        return Err(field_not_found(field_id));
    }
    let field = annotation
        .as_form_field_mut()
        .ok_or_else(|| field_not_found(field_id))?;
    action(field)
}

fn apply_input(
    document: &PdfDocument<'static>,
    field: &FormFieldView,
    value: &FormFieldValueInput,
) -> Result<(), CommandError> {
    with_field(document, &field.field_id, |form| {
        let result = match (form, value) {
            (PdfFormField::Text(text_field), FormFieldValueInput::Text { text }) => {
                text_field.set_value(text)
            }
            (PdfFormField::Checkbox(checkbox), FormFieldValueInput::Checked { checked }) => {
                checkbox.set_checked(*checked)
            }
            _ => {
                return Err(invalid_value(format!(
                    "a {:?} field cannot accept this kind of value",
                    field.kind
                )));
            }
        };
        result.map_err(|error| {
            CommandError::new(
                CommandErrorCode::FormValueInvalid,
                format!("could not set the field value: {error:?}"),
            )
        })
    })
}

fn snapshot_for(field: &FormFieldView) -> Option<FieldValueSnapshot> {
    match field.kind {
        FormFieldKind::Text => Some(FieldValueSnapshot::Text(
            field.value.clone().unwrap_or_default(),
        )),
        FormFieldKind::Checkbox => Some(FieldValueSnapshot::Checked(field.checked)),
        // The effective group state already includes any overlay value.
        FormFieldKind::Radio => Some(FieldValueSnapshot::Radio(field.value.clone())),
        FormFieldKind::ComboBox | FormFieldKind::ListBox => Some(FieldValueSnapshot::Choice(
            field.value.clone().unwrap_or_default(),
        )),
        FormFieldKind::PushButton | FormFieldKind::Signature | FormFieldKind::Unknown => None,
    }
}

/// Restores a value recorded in the unified edit history.
pub(crate) fn apply_field_snapshot(
    document: &PdfDocument<'static>,
    field_id: &str,
    snapshot: &FieldValueSnapshot,
) -> Result<(), CommandError> {
    match snapshot {
        FieldValueSnapshot::Text(text) => with_field(document, field_id, |form| match form {
            PdfFormField::Text(text_field) => text_field.set_value(text).map_err(|error| {
                CommandError::new(
                    CommandErrorCode::FormValueInvalid,
                    format!("could not restore the text value: {error:?}"),
                )
            }),
            _ => Err(field_not_found(field_id)),
        }),
        FieldValueSnapshot::Checked(checked) => with_field(document, field_id, |form| {
            let result = match form {
                PdfFormField::Checkbox(checkbox) => checkbox.set_checked(*checked),
                _ => return Err(field_not_found(field_id)),
            };
            result.map_err(|error| {
                CommandError::new(
                    CommandErrorCode::FormValueInvalid,
                    format!("could not restore the field value: {error:?}"),
                )
            })
        }),
        // Radio selections are restored by the history overlay that discovery
        // reads; nothing is written into the PDFium document for them.
        FieldValueSnapshot::Radio(_) => Ok(()),
        // Choice values are restored by the history overlay, like radios.
        FieldValueSnapshot::Choice(_) => Ok(()),
    }
}

fn fill_for(
    entry: &mut OpenDocument,
    field_id: String,
    value: FormFieldValueInput,
) -> Result<FormFieldEditResultView, CommandError> {
    ensure_forms_editable(&entry.document)?;

    let fields_before = discover_all(entry)?;
    let field = fields_before
        .iter()
        .find(|field| field.field_id == field_id)
        .cloned()
        .ok_or_else(|| field_not_found(&field_id))?;
    if field.read_only {
        return Err(CommandError::new(
            CommandErrorCode::FormFieldReadOnly,
            format!("form field {} is read-only", field.name),
        ));
    }
    if field.created {
        // Created text fields live in the session store until the save
        // pipeline writes them; their value change is a content edit with a
        // full-record snapshot, so undo restores the empty value.
        let FormFieldValueInput::Text { text } = value else {
            return Err(invalid_value("此表单字段只接受文本值。"));
        };
        let uid =
            crate::form_create::created_uid(&field_id).ok_or_else(|| field_not_found(&field_id))?;
        let before = entry
            .edit
            .created_fields
            .find_by_uid(uid)
            .cloned()
            .ok_or_else(|| field_not_found(&field_id))?;
        let history = if before.value == text {
            entry.edit.history_view()
        } else {
            let mut after = before.clone();
            after.value = text;
            entry.edit.created_fields.restore(after.clone());
            crate::text_edit::push_form_structure_action(
                entry,
                uid,
                field.page_index,
                Some(before.snapshot()),
                Some(after.snapshot()),
            )
        };
        let base_field = field_id.clone();
        return Ok(FormFieldEditResultView {
            field_id: base_field,
            fields: discover_all(entry)?,
            history,
        });
    }
    let before =
        snapshot_for(&field).ok_or_else(|| invalid_value("this widget type cannot be filled"))?;

    let after = match field.kind {
        FormFieldKind::Radio => {
            if !matches!(value, FormFieldValueInput::Checked { checked: true }) {
                return Err(invalid_value(
                    "a radio button cannot be cleared; select another value in its group",
                ));
            }
            FieldValueSnapshot::Radio(field.export_value.clone())
        }
        FormFieldKind::ComboBox | FormFieldKind::ListBox => {
            let FormFieldValueInput::Choice { value } = &value else {
                return Err(invalid_value("this field expects one of its options"));
            };
            if !field.options.iter().any(|option| option == value) {
                return Err(invalid_value(format!(
                    "{value:?} is not one of this field's options"
                )));
            }
            FieldValueSnapshot::Choice(value.clone())
        }
        _ => {
            apply_input(&entry.document, &field, &value)?;
            let fields_after = discover_all(entry)?;
            let updated = fields_after
                .iter()
                .find(|candidate| candidate.field_id == field_id)
                .cloned()
                .ok_or_else(|| field_not_found(&field_id))?;
            snapshot_for(&updated)
                .ok_or_else(|| invalid_value("this widget type cannot be filled"))?
        }
    };

    if before != after {
        let before_state = entry.edit.current_state;
        let after_state = entry.edit.next_state + 1;
        entry.edit.next_state = after_state;
        entry
            .edit
            .undo_stack
            .push(EditAction::Field(FieldEditAction {
                field_id: field_id.clone(),
                field_name: field.name.clone(),
                radio_group: field.radio_group.clone(),
                page_index: field.page_index,
                before_state,
                after_state,
                before,
                after,
            }));
        entry.edit.redo_stack.clear();
        entry.edit.current_state = after_state;
    }

    let fields_after = discover_all(entry)?;

    Ok(FormFieldEditResultView {
        field_id,
        fields: fields_after,
        history: entry.edit.history_view(),
    })
}

/// The `fill_form_field` command. Fails when the document or widget forbids
/// editing; the undo/redo history is shared with every other edit tool.
#[tauri::command]
pub fn fill_form_field(
    document_id: String,
    field_id: String,
    value: FormFieldValueInput,
) -> Result<FormFieldEditResultView, CommandError> {
    with_open_document(&document_id, |entry| fill_for(entry, field_id, value))
}
