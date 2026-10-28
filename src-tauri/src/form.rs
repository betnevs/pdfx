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
    /// Every selected index for choice fields (multiple entries only for a
    /// multi-select list box).
    pub selected_indices: Vec<u32>,
    /// True for a list box whose MultiSelect flag is set.
    pub multi_select: bool,
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
    /// The field name Pdfium reports, before copy-page namespacing. Session
    /// internal: it keys inherited form values and is never sent to the UI.
    #[serde(skip)]
    pub(crate) raw_name: String,
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
            view.value = overlay
                .text
                .get(&view.name)
                .cloned()
                .or_else(|| text.value());
            view.multiline = text.is_multiline();
        }
        PdfFormField::Checkbox(checkbox) => {
            view.checked = overlay
                .checked
                .get(&view.name)
                .copied()
                .unwrap_or_else(|| checkbox.is_checked().unwrap_or(false));
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
            describe_choice(combo.options(), false, view);
            view.value = combo.value();
            apply_choice_overlay(view, overlay);
            sync_choice_index(view);
        }
        PdfFormField::ListBox(list) => {
            describe_choice(list.options(), list.is_multiselect(), view);
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
    set_choice_label(view, value);
}

/// Selects the option matching `label` in a choice field view.
fn set_choice_label(view: &mut FormFieldView, label: &str) {
    view.value = Some(label.to_owned());
    view.selected_index = view
        .options
        .iter()
        .position(|option| option == label)
        .map(|index| index as u32);
    view.selected_indices = view.selected_index.into_iter().collect();
}

fn describe_choice(
    options: &pdfium_render::prelude::PdfFormFieldOptions<'_>,
    multi_select: bool,
    view: &mut FormFieldView,
) {
    view.options = options
        .iter()
        .filter_map(|option| option.label().cloned())
        .collect();
    view.selected_indices = options
        .iter()
        .enumerate()
        .filter(|(_, option)| option.is_set())
        .map(|(index, _)| index as u32)
        .collect();
    view.selected_index = view.selected_indices.first().copied();
    view.multi_select = multi_select;
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
///
/// Text and checkbox values are normally written into the PDFium document
/// directly. A terminal field whose `/Kids` are widgets, a field whose value
/// lives on a nested `/Parent`, and every widget on a copied page cannot be
/// written through the annotation API without touching the wrong object. Those
/// fills are recorded here instead; discovery reads the overlay and the save
/// pipeline writes the value and its appearance onto the field's own object.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FormOverlay {
    /// Radio group name -> selected on-state name.
    pub(crate) radio: std::collections::HashMap<String, String>,
    /// Choice field name -> selected label.
    pub(crate) choice: std::collections::HashMap<String, String>,
    /// Text field name -> text value.
    pub(crate) text: std::collections::HashMap<String, String>,
    /// Checkbox field name -> checked state.
    pub(crate) checked: std::collections::HashMap<String, bool>,
}

/// Derives the overlay from the actions currently applied to the document.
///
/// Walking the undo stack is what makes undo/redo work without a second copy
/// of form state: the last applied action per field wins. Values that Pdfium
/// wrote into the document itself are not repeated here — discovery reads
/// them from Pdfium, and an overlay entry would otherwise hide a later write.
pub(crate) fn overlay_for(edit: &EditSession, _pages: &crate::page_ops::PageModel) -> FormOverlay {
    collect_values(edit, false, None)
}

/// Last applied value per field, including values Pdfium already wrote.
///
/// The save pipeline needs these to rebuild the widget appearances: the value
/// edit itself is idempotent, but a stale `/AP` would still draw the old value
/// if no appearance followed.
pub(crate) fn applied_values(
    edit: &EditSession,
    pages: &crate::page_ops::PageModel,
) -> FormOverlay {
    collect_values(edit, true, Some(pages))
}

fn collect_values(
    edit: &EditSession,
    include_pdfium_written: bool,
    live_pages: Option<&crate::page_ops::PageModel>,
) -> FormOverlay {
    let mut overlay = FormOverlay::default();
    for entry in &edit.undo_stack {
        let EditAction::Field(action) = entry else {
            continue;
        };
        // A field on a deleted page is pruned from `/Fields` before the object
        // edits run; writing its value would fail the save.
        if let Some(pages) = live_pages {
            if pages.logical(action.page_index).is_none() {
                continue;
            }
        }
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
        if action.pdfium_write && !include_pdfium_written {
            continue;
        }
        match &action.after {
            FieldValueSnapshot::Choice(value) => {
                if value.is_empty() {
                    overlay.choice.remove(&action.field_name);
                } else {
                    overlay
                        .choice
                        .insert(action.field_name.clone(), value.clone());
                }
            }
            FieldValueSnapshot::Text(text) => {
                overlay.text.insert(action.field_name.clone(), text.clone());
            }
            FieldValueSnapshot::Checked(checked) => {
                overlay.checked.insert(action.field_name.clone(), *checked);
            }
            _ => {}
        }
    }
    overlay
}

/// Object-level edits the save pipeline appends for form values that live in
/// the history overlay rather than in the PDFium document.
///
/// Copied pages contribute two kinds of edits: values the user changed on the
/// copy (the overlay) and the values the copy inherited from its source page
/// when it was created (`CopiedPage::inherited`, keyed by the source's raw
/// field name). An explicit change wins over an inherited value.
pub(crate) fn save_object_edits(
    edit: &EditSession,
    pages: &crate::page_ops::PageModel,
) -> Vec<crate::save::ObjectEdit> {
    let overlay = applied_values(edit, pages);
    let mut edits = Vec::new();

    let mut groups: Vec<_> = overlay.radio.iter().collect();
    groups.sort_by(|left, right| left.0.cmp(right.0));
    for (field_name, state) in groups {
        edits.push(crate::save::ObjectEdit::SetRadioGroup {
            field_name: field_name.clone(),
            state: state.clone(),
        });
    }

    let mut choices: Vec<_> = overlay.choice.iter().collect();
    choices.sort_by(|left, right| left.0.cmp(right.0));
    for (field_name, label) in choices {
        edits.push(crate::save::ObjectEdit::SetChoiceValue {
            field_name: field_name.clone(),
            label: label.clone(),
        });
    }

    let mut texts: Vec<_> = overlay.text.iter().collect();
    texts.sort_by(|left, right| left.0.cmp(right.0));
    for (field_name, text) in texts {
        edits.push(crate::save::ObjectEdit::SetTextValue {
            field_name: field_name.clone(),
            text: text.clone(),
        });
    }

    let mut checked: Vec<_> = overlay.checked.iter().collect();
    checked.sort_by(|left, right| left.0.cmp(right.0));
    for (field_name, state) in checked {
        edits.push(crate::save::ObjectEdit::SetCheckboxValue {
            field_name: field_name.clone(),
            checked: *state,
        });
    }

    let mut copies: Vec<_> = pages
        .copies
        .iter()
        .filter(|(physical, _)| pages.order.contains(physical))
        .collect();
    copies.sort_by_key(|(physical, _)| **physical);
    for (physical, copy) in copies {
        let display = |raw: &str| crate::page_ops::copied_field_name(raw, *physical);

        let mut inherited_radio: Vec<_> = copy.inherited.radio.iter().collect();
        inherited_radio.sort_by(|left, right| left.0.cmp(right.0));
        for (raw_name, state) in inherited_radio {
            let field_name = display(raw_name);
            if overlay.radio.contains_key(&field_name) {
                continue;
            }
            edits.push(crate::save::ObjectEdit::SetRadioGroup {
                field_name,
                state: state.clone(),
            });
        }

        let mut inherited_choice: Vec<_> = copy.inherited.choice.iter().collect();
        inherited_choice.sort_by(|left, right| left.0.cmp(right.0));
        for (raw_name, label) in inherited_choice {
            let field_name = display(raw_name);
            if overlay.choice.contains_key(&field_name) {
                continue;
            }
            edits.push(crate::save::ObjectEdit::SetChoiceValue {
                field_name,
                label: label.clone(),
            });
        }

        let mut inherited_text: Vec<_> = copy.inherited.text.iter().collect();
        inherited_text.sort_by(|left, right| left.0.cmp(right.0));
        for (raw_name, text) in inherited_text {
            let field_name = display(raw_name);
            if overlay.text.contains_key(&field_name) {
                continue;
            }
            edits.push(crate::save::ObjectEdit::SetTextValue {
                field_name,
                text: text.clone(),
            });
        }

        let mut inherited_checked: Vec<_> = copy.inherited.checked.iter().collect();
        inherited_checked.sort_by(|left, right| left.0.cmp(right.0));
        for (raw_name, state) in inherited_checked {
            let field_name = display(raw_name);
            if overlay.checked.contains_key(&field_name) {
                continue;
            }
            edits.push(crate::save::ObjectEdit::SetCheckboxValue {
                field_name,
                checked: *state,
            });
        }
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
    /// Hands out the next on-state for a widget. `key` namespaces the counter
    /// (a copied page has its own widget sequence), while `states_key` names
    /// the source group whose `/Kids` order the states follow.
    fn next_for(&mut self, key: &str, states_key: &str) -> Option<String> {
        let states = self.states.get(states_key)?;
        let used = self.used.entry(key.to_owned()).or_insert(0);
        let state = states.get(*used).cloned().flatten();
        *used += 1;
        state
    }
}

#[allow(clippy::too_many_arguments)]
fn widget_view(
    annotation: &PdfPageAnnotation<'_>,
    page_index: u32,
    annotation_index: usize,
    order: u32,
    geometry: &DisplayGeometry,
    radio_cursor: &mut RadioCursor<'_>,
    overlay: &FormOverlay,
    copies: &std::collections::HashMap<u32, crate::page_ops::CopiedPage>,
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

    let raw_name = field.name().unwrap_or_default();
    let copy = copies.get(&page_index);
    let name = match copy {
        Some(_) => crate::page_ops::copied_field_name(&raw_name, page_index),
        None => raw_name.clone(),
    };
    let mut view = FormFieldView {
        field_id: format!("{page_index}:{annotation_index}"),
        name,
        raw_name: raw_name.clone(),
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
        selected_indices: Vec::new(),
        multi_select: false,
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
        PdfFormField::RadioButton(_) => radio_cursor.next_for(&view.name, &view.raw_name),
        _ => None,
    };
    describe_value(field, &mut view, radio_state, overlay);
    if let Some(copy) = copy {
        apply_copy_values(&mut view, field, overlay, copy);
    }
    Ok(Some(view))
}

/// Applies the session overlay and the values a copied page inherited from its
/// source. An explicit change on the copy always wins over the inherited value,
/// and both win over what Pdfium reports.
fn apply_copy_values(
    view: &mut FormFieldView,
    field: &PdfFormField<'_>,
    overlay: &FormOverlay,
    copy: &crate::page_ops::CopiedPage,
) {
    match field {
        PdfFormField::RadioButton(_) => {
            if overlay.radio.contains_key(&view.name) {
                return;
            }
            let Some(state) = copy.inherited.radio.get(&view.raw_name) else {
                return;
            };
            view.value = Some(state.clone());
            view.checked = view.export_value.as_deref() == Some(state.as_str());
        }
        PdfFormField::Text(_) => {
            if let Some(text) = overlay.text.get(&view.name) {
                view.value = Some(text.clone());
            } else if let Some(text) = copy.inherited.text.get(&view.raw_name) {
                view.value = Some(text.clone());
            }
        }
        PdfFormField::Checkbox(_) => {
            if let Some(checked) = overlay.checked.get(&view.name) {
                view.checked = *checked;
            } else if let Some(checked) = copy.inherited.checked.get(&view.raw_name) {
                view.checked = *checked;
            }
        }
        PdfFormField::ComboBox(_) | PdfFormField::ListBox(_) => {
            if overlay.choice.contains_key(&view.name) {
                return;
            }
            if let Some(label) = copy.inherited.choice.get(&view.raw_name) {
                set_choice_label(view, label);
            }
        }
        PdfFormField::PushButton(_) | PdfFormField::Signature(_) | PdfFormField::Unknown(_) => {}
    }
}

/// Discovers every interactive widget in document order: page by page, then
/// each page's annotation order. This is the tab order the UI uses.
///
/// Widgets on a page created by `copy_page` are shown under a copy-namespaced
/// name so their values never resolve to the source page's field.
pub(crate) fn discover_fields(
    document: &PdfDocument<'static>,
    radio_states: &RadioStates,
    overlay: &FormOverlay,
    copies: &std::collections::HashMap<u32, crate::page_ops::CopiedPage>,
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
                copies,
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
        &overlay_for(&entry.edit, &entry.pages),
        &entry.pages.copies,
    )?;
    Ok(crate::form_create::merge_views(
        existing,
        &entry.edit.created_fields,
    ))
}

/// The `form_fields` command. Returns every fillable or displayable widget.
#[tauri::command]
pub fn form_fields(document_id: String) -> Result<Vec<FormFieldView>, CommandError> {
    with_open_document(&document_id, |entry| {
        Ok(logical_fields(entry, discover_all(entry)?))
    })
}

/// Rewrites discovered fields for the command boundary: page indexes become
/// logical slots and widgets on deleted pages are dropped. Field ids keep the
/// physical page index because they are opaque handles into the live PDFium
/// document. Stable sorting restores the user's page order while keeping the
/// tab order inside each page.
pub(crate) fn logical_fields(
    entry: &OpenDocument,
    fields: Vec<FormFieldView>,
) -> Vec<FormFieldView> {
    let mut fields: Vec<FormFieldView> = fields
        .into_iter()
        .filter_map(|mut field| {
            field.page_index = entry.logical_page(field.page_index)?;
            Some(field)
        })
        .collect();
    fields.sort_by_key(|field| field.page_index);
    fields
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
        Ok(logical_fields(
            entry,
            required_missing(&discover_all(entry)?),
        ))
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
///
/// Only fields whose value Pdfium could reach when the action was recorded are
/// written back into the PDFium document. Text and checkbox values on copied
/// pages and on fields whose terminal object is not the widget live in the
/// history overlay instead; discovery reads the overlay and the save pipeline
/// writes the value. Radio and choice values always take that overlay route.
pub(crate) fn apply_field_snapshot(
    document: &PdfDocument<'static>,
    field_id: &str,
    snapshot: &FieldValueSnapshot,
    pdfium_write: bool,
) -> Result<(), CommandError> {
    match snapshot {
        FieldValueSnapshot::Text(text) => {
            if !pdfium_write {
                return Ok(());
            }
            with_field(document, field_id, |form| match form {
                PdfFormField::Text(text_field) => text_field.set_value(text).map_err(|error| {
                    CommandError::new(
                        CommandErrorCode::FormValueInvalid,
                        format!("could not restore the text value: {error:?}"),
                    )
                }),
                _ => Err(field_not_found(field_id)),
            })
        }
        FieldValueSnapshot::Checked(checked) => {
            if !pdfium_write {
                return Ok(());
            }
            with_field(document, field_id, |form| {
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
            })
        }
        // Radio selections are restored by the history overlay that discovery
        // reads; nothing is written into the PDFium document for them.
        FieldValueSnapshot::Radio(_) => Ok(()),
        // Choice values are restored by the history overlay, like radios.
        FieldValueSnapshot::Choice(_) => Ok(()),
    }
}

/// Whether Pdfium's annotation setter can reach a text or checkbox field's
/// value.
///
/// `FPDFAnnot_SetStringValue` writes `/V` onto the widget annotation. A
/// terminal field that keeps its value on a separate field object (its `/Kids`
/// are the widgets) or that is nested under another field is not addressed by
/// that write, so filling it through Pdfium would silently do nothing. Those
/// fields use the history overlay instead. An unparseable or unknown field is
/// also routed through the overlay, which can always write the correct object
/// at save time.
fn pdfium_can_write_field(source_bytes: &[u8], raw_name: &str) -> bool {
    #[allow(clippy::too_many_arguments)]
    fn walk(
        document: &lopdf::Document,
        fields: &[lopdf::Object],
        prefix: &str,
        target: &str,
        visited: &mut std::collections::HashSet<lopdf::ObjectId>,
        depth: usize,
        matches: &mut Vec<bool>,
    ) {
        if depth > 64 {
            return;
        }
        for entry in fields {
            let Some(id) = entry.as_reference().ok() else {
                continue;
            };
            // Malformed `/Kids` graphs can be cyclic; never revisit a field.
            if !visited.insert(id) {
                continue;
            }
            let Some(dictionary) = document.get_dictionary(id).ok() else {
                continue;
            };
            let title = dictionary
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(|title| String::from_utf8_lossy(title).into_owned());
            let full = match &title {
                Some(title) if prefix.is_empty() => title.clone(),
                Some(title) => format!("{prefix}.{title}"),
                None => prefix.to_owned(),
            };
            let kids: Vec<lopdf::Object> = dictionary
                .get_deref(b"Kids", document)
                .ok()
                .and_then(|kids| kids.as_array().ok())
                .map(|kids| {
                    kids.iter()
                        .filter(|kid| {
                            kid.as_reference()
                                .ok()
                                .and_then(|id| document.get_dictionary(id).ok())
                                .map(|kid| kid.has(b"T"))
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if title.is_some() && full == target && kids.is_empty() {
                let has_widget_kids = dictionary
                    .get_deref(b"Kids", document)
                    .ok()
                    .and_then(|kids| kids.as_array().ok())
                    .map(|kids| !kids.is_empty())
                    .unwrap_or(false);
                matches.push(!has_widget_kids);
                continue;
            }
            for kid in &kids {
                walk(
                    document,
                    std::slice::from_ref(kid),
                    &full,
                    target,
                    visited,
                    depth + 1,
                    matches,
                );
            }
        }
    }

    let Ok(document) = lopdf::Document::load_mem(source_bytes) else {
        return false;
    };
    let Ok(acro_form) = document
        .catalog()
        .and_then(|catalog| catalog.get_deref(b"AcroForm", &document))
        .and_then(lopdf::Object::as_dict)
    else {
        return false;
    };
    let Ok(fields) = acro_form
        .get_deref(b"Fields", &document)
        .and_then(lopdf::Object::as_array)
    else {
        return false;
    };
    let mut matches = Vec::new();
    walk(
        &document,
        fields,
        "",
        raw_name,
        &mut std::collections::HashSet::new(),
        0,
        &mut matches,
    );
    // Pdfium resolves a name to a single field object. Ambiguous names must go
    // through the save overlay, which updates every object carrying the name.
    matches.len() == 1 && matches[0]
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
    if field.kind == FormFieldKind::Signature {
        // Signature fields are structure only: PDFX never signs and has no
        // certificate code, so there is nothing this command could write.
        return Err(invalid_value(
            "此字段仅创建签名结构，PDFX 不执行证书签名，无法填写。",
        ));
    }
    if field.created {
        // Created fields live in the session store until the save pipeline
        // writes them; their value change is a content edit with a full-record
        // snapshot, so undo restores the previous state.
        let uid =
            crate::form_create::created_uid(&field_id).ok_or_else(|| field_not_found(&field_id))?;
        let before = entry
            .edit
            .created_fields
            .find_by_uid(uid)
            .cloned()
            .ok_or_else(|| field_not_found(&field_id))?;
        let mut after = before.clone();
        let after = match field.kind {
            FormFieldKind::Text => {
                let FormFieldValueInput::Text { text } = value else {
                    return Err(invalid_value("此表单字段只接受文本值。"));
                };
                after.value = text;
                after
            }
            FormFieldKind::Checkbox => {
                let FormFieldValueInput::Checked { checked } = value else {
                    return Err(invalid_value("此表单字段只接受选中状态。"));
                };
                after.checked = checked;
                after
            }
            FormFieldKind::Radio => {
                if !matches!(value, FormFieldValueInput::Checked { checked: true }) {
                    return Err(invalid_value(
                        "单选按钮不能取消选择；请选择组内的另一个选项。",
                    ));
                }
                let index = crate::form_create::created_option_index(&field_id)
                    .ok_or_else(|| field_not_found(&field_id))?;
                if index >= before.widgets.len() {
                    return Err(field_not_found(&field_id));
                }
                after.selected_option = Some(index);
                after
            }
            FormFieldKind::ComboBox | FormFieldKind::ListBox => {
                let FormFieldValueInput::Choice { value } = value else {
                    return Err(invalid_value("此表单字段只接受列表中的选项。"));
                };
                let index = before
                    .options
                    .iter()
                    .position(|option| option == &value)
                    .ok_or_else(|| invalid_value(format!("“{value}”不是此字段的可选项。")))?;
                // Picking a value replaces the selection; multiple defaults are
                // set through the property panel's multi-select list box.
                after.selected_options = vec![index];
                after
            }
            _ => return Err(invalid_value("此表单字段类型不支持填写。")),
        };
        if after == before {
            return Ok(FormFieldEditResultView {
                field_id,
                fields: logical_fields(entry, discover_all(entry)?),
                history: entry.edit.history_view(),
            });
        }
        entry.edit.created_fields.restore(after.clone());
        let history = crate::text_edit::push_form_structure_action(
            entry,
            uid,
            field.page_index,
            Some(before.snapshot()),
            Some(after.snapshot()),
        );
        return Ok(FormFieldEditResultView {
            field_id,
            fields: logical_fields(entry, discover_all(entry)?),
            history,
        });
    }
    let before =
        snapshot_for(&field).ok_or_else(|| invalid_value("this widget type cannot be filled"))?;
    let is_copy = entry.pages.copies.contains_key(&field.page_index);
    // Radio and choice values always live in the history overlay. Text and
    // checkbox values only go through Pdfium when the annotation setter can
    // actually reach the field object.
    let pdfium_write = !is_copy
        && matches!(field.kind, FormFieldKind::Text | FormFieldKind::Checkbox)
        && pdfium_can_write_field(&entry.source_bytes, &field.raw_name);

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
        FormFieldKind::Text | FormFieldKind::Checkbox if !pdfium_write => {
            // Copied pages and field shapes Pdfium cannot address are filled
            // through the history overlay. The save pipeline applies the value
            // and regenerates the widget appearance on the field's own object.
            match (&field.kind, &value) {
                (FormFieldKind::Text, FormFieldValueInput::Text { text }) => {
                    FieldValueSnapshot::Text(text.clone())
                }
                (FormFieldKind::Checkbox, FormFieldValueInput::Checked { checked }) => {
                    FieldValueSnapshot::Checked(*checked)
                }
                _ => return Err(invalid_value("this widget type cannot be filled")),
            }
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
                pdfium_write,
            }));
        entry.edit.redo_stack.clear();
        entry.edit.current_state = after_state;
    }

    let fields_after = discover_all(entry)?;

    Ok(FormFieldEditResultView {
        field_id,
        fields: logical_fields(entry, fields_after),
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
