//! Command-boundary tests for creating text Form Fields.
//!
//! The tests use the same Rust functions the Tauri IPC layer exposes. Saved
//! output is reopened through the command boundary with a fresh PDFium parse
//! and parsed independently with lopdf, so the structure other readers see is
//! checked rather than PDFX's in-memory session state.

mod support;

use support::synthetic;

use pdfx_lib::commands::{close_document, open_document, CommandErrorCode};
use pdfx_lib::form::{
    fill_form_field, form_fields, missing_required_fields, FormFieldKind, FormFieldRect,
    FormFieldValueInput,
};
use pdfx_lib::form_create::{
    create_form_field, create_form_text_field, delete_form_field, update_form_field,
    CreateFormFieldRequest, CreateFormTextFieldRequest, UpdateFormFieldRequest,
};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn create_request(rect: FormFieldRect) -> CreateFormTextFieldRequest {
    CreateFormTextFieldRequest {
        rect,
        name: None,
        tooltip: Some("请输入姓名".to_owned()),
        required: true,
        multiline: false,
        font_size: Some(12.0),
    }
}

#[test]
fn creates_a_text_field_that_discovery_reports_and_undo_removes() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };

    let created = create_form_text_field(document_id.clone(), 0, create_request(rect))
        .expect("create a text field");
    assert!(
        created.history.dirty,
        "creating a field dirties the document"
    );
    assert!(created.history.can_undo);
    assert_eq!(created.history.revision, created.history.saved_revision + 1);

    let fields = form_fields(document_id.clone()).expect("discover fields");
    let field = fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("the created field is discovered");
    assert_eq!(field.kind, FormFieldKind::Text);
    assert_eq!(field.name, "text_field_1");
    assert_eq!(field.page_index, 0);
    assert_eq!(field.rect, rect);
    assert_eq!(field.tooltip.as_deref(), Some("请输入姓名"));
    assert!(field.required);
    assert!(!field.multiline);
    assert!(!field.read_only);
    assert_eq!(field.font_size, Some(12.0));
    assert!(
        field.created,
        "the property panel needs to identify new fields"
    );
    assert_eq!(field.value, None);

    let undo = undo_text_edit(document_id.clone()).expect("undo the creation");
    assert_eq!(undo.page_index, 0);
    assert!(
        form_fields(document_id.clone())
            .expect("fields after undo")
            .iter()
            .all(|field| field.field_id != created.field_id),
        "undo removes the created field from discovery and the tab order"
    );
    assert!(!undo.history.can_undo && undo.history.can_redo);

    let redo = redo_text_edit(document_id.clone()).expect("redo the creation");
    assert_eq!(redo.page_index, 0);
    let restored = form_fields(document_id.clone()).expect("fields after redo");
    let field = restored
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("redo re-inserts the same field id");
    assert_eq!(field.rect, rect);
    assert_eq!(field.name, "text_field_1");
    assert!(field.required);
    assert_eq!(field.tooltip.as_deref(), Some("请输入姓名"));
    assert!(!redo.history.can_redo && redo.history.dirty);

    close_document(document_id).expect("close document");
}

#[test]
fn creates_checkboxes_that_discovery_reports_and_the_shared_history_toggles() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 16.0,
        height: 16.0,
    };

    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect,
            name: None,
            tooltip: Some("同意条款".to_owned()),
            required: true,
            checked: false,
        },
    )
    .expect("create a checkbox");
    assert!(created.history.dirty);
    assert_eq!(created.history.revision, created.history.saved_revision + 1);

    let fields = form_fields(document_id.clone()).expect("discover fields");
    let field = fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("the created checkbox is discovered");
    assert_eq!(field.kind, FormFieldKind::Checkbox);
    assert_eq!(field.name, "checkbox_1");
    assert_eq!(field.rect, rect);
    assert_eq!(field.tooltip.as_deref(), Some("同意条款"));
    assert!(field.required);
    assert!(!field.checked);
    assert!(!field.read_only);
    assert!(field.created);
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1,
        "an unchecked required checkbox blocks saving"
    );

    let toggled = fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("toggle the created checkbox without saving");
    let field = toggled
        .fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("still discovered");
    assert!(field.checked);
    assert!(toggled.history.dirty);
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    undo_text_edit(document_id.clone()).expect("undo the toggle");
    let field = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .find(|field| field.field_id == created.field_id)
        .expect("undo restores the checkbox");
    assert!(!field.checked);
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1
    );

    redo_text_edit(document_id.clone()).expect("redo the toggle");
    assert!(
        form_fields(document_id.clone())
            .expect("fields after redo")
            .into_iter()
            .find(|field| field.field_id == created.field_id)
            .expect("redo restores the checkbox")
            .checked
    );
    undo_text_edit(document_id.clone()).expect("undo the toggle again");

    let checked_by_default = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 100.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("newsletter".to_owned()),
            tooltip: None,
            required: true,
            checked: true,
        },
    )
    .expect("create a checkbox with a default");
    let field = checked_by_default
        .fields
        .iter()
        .find(|field| field.field_id == checked_by_default.field_id)
        .expect("the default-checked checkbox is discovered");
    assert!(field.checked, "the creation default is reported");
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1,
        "only the unchecked required checkbox is missing"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn creates_radio_groups_with_kid_widgets_and_a_consistent_value() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 16.0,
        height: 16.0,
    };

    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect,
            name: Some("contact".to_owned()),
            tooltip: Some("联系方式".to_owned()),
            required: true,
            options: vec!["Email".to_owned(), "Phone".to_owned(), "Email".to_owned()],
            selected_index: None,
        },
    )
    .expect("create a radio group");
    assert!(created.history.dirty);
    assert_eq!(
        created.field_id, "new:1:0",
        "the first option widget is selected after placement"
    );

    let fields = form_fields(document_id.clone()).expect("discover fields");
    let group: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "contact")
        .collect();
    assert_eq!(group.len(), 3, "one widget per option");
    assert!(group.iter().all(|field| field.kind == FormFieldKind::Radio
        && field.created
        && field.required
        && field.tooltip.as_deref() == Some("联系方式")));
    assert!(group
        .iter()
        .all(|field| field.radio_group.as_deref() == Some("contact")));
    assert_eq!(
        group
            .iter()
            .map(|field| field.field_id.as_str())
            .collect::<Vec<_>>(),
        vec!["new:1:0", "new:1:1", "new:1:2"]
    );
    assert_eq!(
        group
            .iter()
            .map(|field| field.export_value.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["Email", "Phone", "Email_2"],
        "duplicate labels still get distinct PDF on-states"
    );
    assert_eq!(
        group[0].options,
        vec!["Email", "Phone", "Email"],
        "the panel sees every option label"
    );
    assert!(group
        .iter()
        .all(|field| !field.checked && field.value.is_none() && field.selected_index.is_none()));
    assert_eq!(group[1].rect.y, rect.y + rect.height + 4.0);
    assert_eq!(group[2].rect.y, rect.y + 2.0 * (rect.height + 4.0));
    assert_eq!(
        group.iter().map(|field| field.order).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1,
        "an unanswered required radio group blocks saving once"
    );

    let filled = fill_form_field(
        document_id.clone(),
        "new:1:1".to_owned(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select the second option");
    let group: Vec<_> = filled
        .fields
        .iter()
        .filter(|field| field.name == "contact")
        .collect();
    assert_eq!(group.len(), 3);
    assert_eq!(
        group.iter().map(|field| field.checked).collect::<Vec<_>>(),
        vec![false, true, false]
    );
    assert!(group
        .iter()
        .all(|field| field.value.as_deref() == Some("Phone") && field.selected_index == Some(1)));
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    let error = fill_form_field(
        document_id.clone(),
        "new:1:1".to_owned(),
        FormFieldValueInput::Checked { checked: false },
    )
    .expect_err("a radio selection cannot be cleared in place");
    assert_eq!(error.code, CommandErrorCode::FormValueInvalid);
    assert!(error.message.contains("不能取消选择"), "{}", error.message);

    undo_text_edit(document_id.clone()).expect("undo the selection");
    let group: Vec<_> = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .filter(|field| field.name == "contact")
        .collect();
    assert!(group
        .iter()
        .all(|field| !field.checked && field.value.is_none()));
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields after undo")
            .len(),
        1
    );

    redo_text_edit(document_id.clone()).expect("redo the selection");
    let group: Vec<_> = form_fields(document_id.clone())
        .expect("fields after redo")
        .into_iter()
        .filter(|field| field.name == "contact")
        .collect();
    assert_eq!(
        group.iter().map(|field| field.checked).collect::<Vec<_>>(),
        vec![false, true, false]
    );

    let deleted = delete_form_field(document_id.clone(), "new:1:2".to_owned())
        .expect("delete any widget of the group");
    assert!(deleted.fields.iter().all(|field| field.name != "contact"));
    undo_text_edit(document_id.clone()).expect("undo the group deletion");
    assert_eq!(
        form_fields(document_id.clone())
            .expect("fields after undo")
            .into_iter()
            .filter(|field| field.name == "contact")
            .count(),
        3,
        "undo restores the whole group"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn radio_options_never_use_the_reserved_off_state() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-radio-off-state");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("state".to_owned()),
            tooltip: None,
            required: true,
            options: vec!["Off".to_owned(), "On".to_owned()],
            selected_index: Some(0),
        },
    )
    .expect("create a radio group with an Off label");

    let group = |fields: &[pdfx_lib::form::FormFieldView]| -> Vec<_> {
        fields
            .iter()
            .filter(|field| field.name == "state")
            .cloned()
            .collect()
    };
    let fields = form_fields(document_id.clone()).expect("fields");
    let widgets = group(&fields);
    assert_eq!(
        widgets
            .iter()
            .map(|field| field.export_value.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["Choice1", "On"],
        "an option label that sanitizes to Off gets a ChoiceN on-state"
    );
    assert_eq!(
        widgets
            .iter()
            .map(|field| field.checked)
            .collect::<Vec<_>>(),
        vec![true, false],
        "the default option stays selected"
    );

    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let parent = field_named(&parsed, "state");
    assert_eq!(
        name_text(parent.get(b"V").expect("/V")),
        "Choice1",
        "the parent value is the option's on-state, never /Off"
    );
    let kids = parent
        .get(b"Kids")
        .expect("/Kids")
        .as_array()
        .expect("kids");
    assert_eq!(kids.len(), 2);
    let first = lopdf_dict(&parsed, &kids[0]);
    assert_eq!(name_text(first.get(b"AS").expect("/AS")), "Choice1");
    let normal = lopdf_dict(
        &parsed,
        lopdf_dict(&parsed, first.get(b"AP").expect("/AP"))
            .get(b"N")
            .expect("/AP /N"),
    );
    assert!(
        normal.get(b"Off").is_ok(),
        "the off state is the off appearance"
    );
    assert!(
        normal.get(b"Choice1").is_ok(),
        "the on state does not overwrite /Off"
    );
    assert_ne!(
        stream_content(&parsed, normal.get(b"Off").unwrap()),
        stream_content(&parsed, normal.get(b"Choice1").unwrap()),
        "the selected option paints a different appearance"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let widgets = group(&form_fields(reopened.clone()).expect("reopened fields"));
    assert_eq!(
        widgets
            .iter()
            .map(|field| field.checked)
            .collect::<Vec<_>>(),
        vec![true, false],
        "the default survives a save and reopen"
    );
    assert_eq!(widgets[0].value.as_deref(), Some("Choice1"));
    assert!(missing_required_fields(reopened.clone())
        .expect("required fields")
        .is_empty());
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn updates_radio_options_and_default_together_and_history_restores_them() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("size".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["S".to_owned(), "M".to_owned()],
            selected_index: None,
        },
    )
    .expect("create a radio group");

    let group = |document_id: &str| -> Vec<pdfx_lib::form::FormFieldView> {
        form_fields(document_id.to_owned())
            .expect("discover fields")
            .into_iter()
            .filter(|field| field.name == "size")
            .collect()
    };

    let updated = update_form_field(
        document_id.clone(),
        created.field_id.clone(),
        UpdateFormFieldRequest::Radio {
            name: "size".to_owned(),
            tooltip: Some("尺码".to_owned()),
            required: true,
            options: vec!["S".to_owned(), "M".to_owned(), "L".to_owned()],
            selected_index: Some(2),
        },
    )
    .expect("add an option and choose the default");
    assert!(updated.history.dirty);
    let fields: Vec<_> = updated
        .fields
        .iter()
        .filter(|field| field.name == "size")
        .collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(
        fields
            .iter()
            .map(|field| field.options.clone())
            .collect::<Vec<_>>(),
        vec![
            vec!["S".to_owned(), "M".to_owned(), "L".to_owned()],
            vec!["S".to_owned(), "M".to_owned(), "L".to_owned()],
            vec!["S".to_owned(), "M".to_owned(), "L".to_owned()],
        ]
    );
    assert_eq!(
        fields.iter().map(|field| field.checked).collect::<Vec<_>>(),
        vec![false, false, true]
    );
    assert!(fields
        .iter()
        .all(|field| field.value.as_deref() == Some("L")
            && field.selected_index == Some(2)
            && field.required
            && field.tooltip.as_deref() == Some("尺码")));
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    let updated = update_form_field(
        document_id.clone(),
        "new:1:1".to_owned(),
        UpdateFormFieldRequest::Radio {
            name: "size".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["Small".to_owned(), "Medium".to_owned()],
            selected_index: Some(0),
        },
    )
    .expect("edit labels, remove an option and choose another default");
    let fields: Vec<_> = updated
        .fields
        .iter()
        .filter(|field| field.name == "size")
        .collect();
    assert_eq!(fields.len(), 2, "removing an option removes its widget");
    assert_eq!(
        fields
            .iter()
            .map(|field| field.export_value.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["Small", "Medium"]
    );
    assert_eq!(
        fields.iter().map(|field| field.checked).collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(fields
        .iter()
        .all(|field| field.value.as_deref() == Some("Small")));
    assert_eq!(fields[0].rect.y, 100.0);
    assert_eq!(fields[1].rect.y, 120.0);

    let before_error = group(&document_id);
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Text {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("taken".to_owned()),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: None,
        },
    )
    .expect("create a field that owns the name");
    let duplicate = update_form_field(
        document_id.clone(),
        "new:1".to_owned(),
        UpdateFormFieldRequest::Radio {
            name: "taken".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["Small".to_owned(), "Medium".to_owned()],
            selected_index: Some(0),
        },
    )
    .expect_err("an existing field name is rejected");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);
    let invalid = update_form_field(
        document_id.clone(),
        "new:1".to_owned(),
        UpdateFormFieldRequest::Radio {
            name: "   ".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["Small".to_owned(), "Medium".to_owned()],
            selected_index: Some(0),
        },
    )
    .expect_err("a blank name is rejected");
    assert_eq!(invalid.code, CommandErrorCode::FormFieldNameInvalid);
    let out_of_range = update_form_field(
        document_id.clone(),
        "new:1".to_owned(),
        UpdateFormFieldRequest::Radio {
            name: "size".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["Only".to_owned()],
            selected_index: Some(2),
        },
    )
    .expect_err("a default outside the option list is rejected");
    assert_eq!(out_of_range.code, CommandErrorCode::InvalidArgument);
    assert_eq!(
        group(&document_id),
        before_error,
        "rejected updates change nothing"
    );

    undo_text_edit(document_id.clone()).expect("undo the last option edit");
    // The name owner was created after the option edits, so the first undo
    // removes it and the next one reverts the option edit.
    assert!(form_fields(document_id.clone())
        .expect("fields after undo")
        .iter()
        .all(|field| field.name != "taken"));
    undo_text_edit(document_id.clone()).expect("undo the option edit");
    let fields = group(&document_id);
    assert_eq!(fields.len(), 3);
    assert!(fields
        .iter()
        .all(|field| field.value.as_deref() == Some("L")));

    undo_text_edit(document_id.clone()).expect("undo the option addition");
    let fields = group(&document_id);
    assert_eq!(fields.len(), 2);
    assert!(fields
        .iter()
        .all(|field| field.value.is_none() && !field.required));
    assert_eq!(
        fields
            .iter()
            .map(|field| field.export_value.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["S", "M"]
    );

    close_document(document_id).expect("close document");
}

#[test]
fn creates_choice_fields_with_options_defaults_and_font_size() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let combo = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("country".to_owned()),
            tooltip: Some("请选择国家".to_owned()),
            required: true,
            options: vec!["中国".to_owned(), "Japan".to_owned()],
            selected_index: Some(1),
            font_size: Some(14.0),
        },
    )
    .expect("create a dropdown field");
    let field = combo
        .fields
        .iter()
        .find(|field| field.field_id == combo.field_id)
        .expect("the dropdown is discovered");
    assert_eq!(field.kind, FormFieldKind::ComboBox);
    assert_eq!(field.name, "country");
    assert_eq!(field.options, ["中国", "Japan"]);
    assert_eq!(field.value.as_deref(), Some("Japan"));
    assert_eq!(field.selected_index, Some(1));
    assert_eq!(field.selected_indices, [1]);
    assert!(!field.multi_select);
    assert_eq!(field.font_size, Some(14.0));
    assert!(field.required && field.created);
    assert_eq!(field.tooltip.as_deref(), Some("请选择国家"));
    assert!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .is_empty(),
        "a default selection answers the required dropdown"
    );

    let list = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 140.0,
                width: 180.0,
                height: 48.0,
            },
            name: Some("tags".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            selected_indices: vec![0, 2],
            multi_select: true,
            font_size: Some(10.0),
        },
    )
    .expect("create a multi-select list");
    let field = list
        .fields
        .iter()
        .find(|field| field.field_id == list.field_id)
        .expect("the list is discovered");
    assert_eq!(field.kind, FormFieldKind::ListBox);
    assert_eq!(field.options, ["A", "B", "C"]);
    assert_eq!(field.value.as_deref(), Some("A"));
    assert_eq!(field.selected_index, Some(0));
    assert_eq!(field.selected_indices, [0, 2]);
    assert!(field.multi_select);
    assert_eq!(field.font_size, Some(10.0));

    let required_list = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 48.0,
            },
            name: Some("level".to_owned()),
            tooltip: None,
            required: true,
            options: vec!["Low".to_owned(), "High".to_owned()],
            selected_indices: Vec::new(),
            multi_select: false,
            font_size: None,
        },
    )
    .expect("create an unanswered required list");
    assert_eq!(required_list.field_id, "new:3");
    let field = required_list
        .fields
        .iter()
        .find(|field| field.field_id == required_list.field_id)
        .expect("the required list is discovered");
    assert_eq!(field.value, None);
    assert_eq!(field.selected_indices, Vec::<u32>::new());
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1,
        "an unanswered required list box blocks saving"
    );

    let invalid = fill_form_field(
        document_id.clone(),
        required_list.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Nope".to_owned(),
        },
    )
    .expect_err("a value outside /Opt is rejected");
    assert_eq!(invalid.code, CommandErrorCode::FormValueInvalid);

    let filled = fill_form_field(
        document_id.clone(),
        required_list.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "High".to_owned(),
        },
    )
    .expect("choose a list option");
    let field = filled
        .fields
        .iter()
        .find(|field| field.field_id == required_list.field_id)
        .expect("still discovered");
    assert_eq!(field.value.as_deref(), Some("High"));
    assert_eq!(field.selected_index, Some(1));
    assert_eq!(field.selected_indices, [1]);
    assert!(filled.history.dirty);
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    undo_text_edit(document_id.clone()).expect("undo the list selection");
    let field = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .find(|field| field.field_id == required_list.field_id)
        .expect("the required list survives undo");
    assert_eq!(field.value, None);
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields after undo")
            .len(),
        1
    );
    redo_text_edit(document_id.clone()).expect("redo the list selection");
    assert_eq!(
        form_fields(document_id.clone())
            .expect("fields after redo")
            .into_iter()
            .find(|field| field.field_id == required_list.field_id)
            .and_then(|field| field.value),
        Some("High".to_owned())
    );

    let deleted = delete_form_field(document_id.clone(), combo.field_id.clone())
        .expect("delete the dropdown");
    assert!(deleted.fields.iter().all(|field| field.name != "country"));
    undo_text_edit(document_id.clone()).expect("undo the dropdown deletion");
    assert!(form_fields(document_id.clone())
        .expect("fields after undo")
        .iter()
        .any(|field| field.name == "country" && field.kind == FormFieldKind::ComboBox));

    close_document(document_id).expect("close document");
}

#[test]
fn updates_choice_and_signature_properties_through_the_history() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-choice-signature-update");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };

    let combo = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect,
            name: Some("country".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["中国".to_owned(), "Japan".to_owned()],
            selected_index: None,
            font_size: None,
        },
    )
    .expect("create the dropdown");
    let list = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect { y: 140.0, ..rect },
            name: Some("tags".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned(), "B".to_owned()],
            selected_indices: Vec::new(),
            multi_select: false,
            font_size: None,
        },
    )
    .expect("create the list");
    let signature = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                y: 200.0,
                width: 240.0,
                height: 80.0,
                ..rect
            },
            name: Some("sign_here".to_owned()),
            tooltip: None,
        },
    )
    .expect("create the signature");

    let updated = update_form_field(
        document_id.clone(),
        combo.field_id.clone(),
        UpdateFormFieldRequest::ComboBox {
            name: "nation".to_owned(),
            tooltip: Some("国家".to_owned()),
            required: true,
            options: vec!["中国".to_owned(), "日本".to_owned(), "Korea".to_owned()],
            selected_index: Some(2),
            font_size: 18.0,
        },
    )
    .expect("update the dropdown properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == combo.field_id)
        .expect("updated dropdown is discovered");
    assert_eq!(field.name, "nation");
    assert_eq!(field.tooltip.as_deref(), Some("国家"));
    assert_eq!(field.options, ["中国", "日本", "Korea"]);
    assert_eq!(field.value.as_deref(), Some("Korea"));
    assert_eq!(field.selected_indices, [2]);
    assert_eq!(field.font_size, Some(18.0));
    assert!(field.required);
    assert!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .is_empty(),
        "the updated default answers the required dropdown"
    );

    undo_text_edit(document_id.clone()).expect("undo the dropdown update");
    let field = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .find(|field| field.field_id == combo.field_id)
        .expect("dropdown survives undo");
    assert_eq!(field.name, "country");
    assert_eq!(field.options, ["中国", "Japan"]);
    assert_eq!(field.value, None);
    assert_eq!(field.font_size, Some(12.0));
    assert!(!field.required);
    redo_text_edit(document_id.clone()).expect("redo the dropdown update");
    assert!(form_fields(document_id.clone())
        .expect("fields after redo")
        .iter()
        .any(|field| field.name == "nation" && field.selected_indices == [2]));

    let updated = update_form_field(
        document_id.clone(),
        list.field_id.clone(),
        UpdateFormFieldRequest::ListBox {
            name: "labels".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            selected_indices: vec![1, 2],
            multi_select: true,
            font_size: 9.0,
        },
    )
    .expect("update the list properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == list.field_id)
        .expect("updated list is discovered");
    assert_eq!(field.name, "labels");
    assert_eq!(field.options, ["A", "B", "C"]);
    assert_eq!(field.selected_indices, [1, 2]);
    assert_eq!(field.value.as_deref(), Some("B"));
    assert!(field.multi_select);
    assert_eq!(field.font_size, Some(9.0));
    assert!(field.required);

    let before_error = form_fields(document_id.clone()).expect("fields before the bad update");
    let rejected = update_form_field(
        document_id.clone(),
        list.field_id.clone(),
        UpdateFormFieldRequest::ListBox {
            name: "labels".to_owned(),
            tooltip: None,
            required: true,
            options: vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            selected_indices: vec![1, 2],
            multi_select: false,
            font_size: 9.0,
        },
    )
    .expect_err("two defaults need multi-select");
    assert_eq!(rejected.code, CommandErrorCode::InvalidArgument);
    assert_eq!(
        form_fields(document_id.clone()).expect("fields after the rejection"),
        before_error,
        "a rejected update changes nothing"
    );

    let updated = update_form_field(
        document_id.clone(),
        signature.field_id.clone(),
        UpdateFormFieldRequest::Signature {
            name: "signature_here".to_owned(),
            tooltip: Some("签名处".to_owned()),
        },
    )
    .expect("update the signature properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == signature.field_id)
        .expect("updated signature is discovered");
    assert_eq!(field.name, "signature_here");
    assert_eq!(field.tooltip.as_deref(), Some("签名处"));
    assert_eq!(field.kind, FormFieldKind::Signature);
    assert_eq!(field.value, None);

    let wrong_kind = update_form_field(
        document_id.clone(),
        combo.field_id.clone(),
        UpdateFormFieldRequest::Signature {
            name: "nation".to_owned(),
            tooltip: None,
        },
    )
    .expect_err("a dropdown cannot be updated as a signature");
    assert_eq!(wrong_kind.code, CommandErrorCode::InvalidArgument);

    let duplicate = update_form_field(
        document_id.clone(),
        signature.field_id.clone(),
        UpdateFormFieldRequest::Signature {
            name: "nation".to_owned(),
            tooltip: None,
        },
    )
    .expect_err("a duplicate name is rejected");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);

    let filled_signature = fill_form_field(
        document_id.clone(),
        signature.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "signed".to_owned(),
        },
    )
    .expect_err("a signature field cannot be filled");
    assert_eq!(filled_signature.code, CommandErrorCode::FormValueInvalid);
    assert!(
        filled_signature.message.contains("不执行证书签名"),
        "{}",
        filled_signature.message
    );

    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse the saved output");
    let saved_combo = field_named(&parsed, "nation");
    let combo_flags = saved_combo.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(combo_flags & (1 << 1), 1 << 1, "Required bit");
    let options: Vec<String> = saved_combo
        .get(b"Opt")
        .expect("/Opt")
        .as_array()
        .unwrap()
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(options, ["中国", "日本", "Korea"]);
    assert_eq!(decode_pdf_text(saved_combo.get(b"V").expect("/V")), "Korea");
    assert_eq!(
        saved_combo
            .get(b"I")
            .expect("/I")
            .as_array()
            .unwrap()
            .first()
            .unwrap()
            .as_i64()
            .unwrap(),
        2
    );
    let da = decode_pdf_text(saved_combo.get(b"DA").expect("/DA"));
    assert!(da.contains("18 Tf"), "got /DA {da:?}");

    let saved_list = field_named(&parsed, "labels");
    let list_flags = saved_list.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(list_flags & (1 << 21), 1 << 21, "MultiSelect bit");
    let values: Vec<String> = saved_list
        .get(b"V")
        .expect("/V")
        .as_array()
        .expect("multi-select /V array")
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(values, ["B", "C"]);
    let saved_signature = field_named(&parsed, "signature_here");
    assert_eq!(
        decode_pdf_text(saved_signature.get(b"TU").expect("/TU")),
        "签名处"
    );

    // The signature survives a delete/undo cycle like every created field.
    let deleted = delete_form_field(document_id.clone(), signature.field_id.clone())
        .expect("delete the signature");
    assert!(deleted
        .fields
        .iter()
        .all(|field| field.name != "signature_here"));
    undo_text_edit(document_id.clone()).expect("undo the signature deletion");
    let restored = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .find(|field| field.name == "signature_here")
        .expect("the signature is restored");
    assert_eq!(restored.kind, FormFieldKind::Signature);
    assert_eq!(restored.tooltip.as_deref(), Some("签名处"));

    close_document(document_id).expect("close document");
}

#[test]
fn rejects_invalid_choice_and_signature_requests_without_creating_fields() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };
    let combo = |rect: FormFieldRect,
                 name: Option<String>,
                 options: Vec<String>,
                 selected_index: Option<usize>| {
        CreateFormFieldRequest::ComboBox {
            rect,
            name,
            tooltip: None,
            required: false,
            options,
            selected_index,
            font_size: None,
        }
    };
    let list = |selected_indices: Vec<usize>, multi_select: bool| CreateFormFieldRequest::ListBox {
        rect,
        name: None,
        tooltip: None,
        required: false,
        options: vec!["A".to_owned(), "B".to_owned()],
        selected_indices,
        multi_select,
        font_size: None,
    };
    let assert_no_fields = |document_id: &str| {
        assert!(
            form_fields(document_id.to_owned())
                .expect("fields after a rejected request")
                .is_empty(),
            "a rejected request creates nothing"
        );
    };

    let out_of_range = create_form_field(
        document_id.clone(),
        0,
        combo(rect, Some("bad".to_owned()), vec!["A".to_owned()], Some(1)),
    )
    .expect_err("an out-of-range default is rejected");
    assert_eq!(out_of_range.code, CommandErrorCode::InvalidArgument);
    assert!(
        out_of_range.message.contains("默认选项"),
        "{}",
        out_of_range.message
    );
    assert_no_fields(&document_id);

    let out_of_range = create_form_field(document_id.clone(), 0, list(vec![2], false))
        .expect_err("an out-of-range list default is rejected");
    assert_eq!(out_of_range.code, CommandErrorCode::InvalidArgument);
    assert_no_fields(&document_id);

    let too_many = create_form_field(document_id.clone(), 0, list(vec![0, 1], false))
        .expect_err("a single-select list accepts one default");
    assert_eq!(too_many.code, CommandErrorCode::InvalidArgument);
    assert!(too_many.message.contains("多选"), "{}", too_many.message);
    assert_no_fields(&document_id);

    let tiny = create_form_field(
        document_id.clone(),
        0,
        combo(
            FormFieldRect {
                width: 1.0,
                height: 1.0,
                ..rect
            },
            None,
            vec!["A".to_owned()],
            None,
        ),
    )
    .expect_err("a widget below the minimum size is rejected");
    assert_eq!(tiny.code, CommandErrorCode::InvalidArgument);
    assert_no_fields(&document_id);

    let outside = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                x: 600.0,
                y: 700.0,
                width: 100.0,
                height: 100.0,
            },
            name: Some("bad".to_owned()),
            tooltip: None,
        },
    )
    .expect_err("a widget outside the page is rejected");
    assert_eq!(outside.code, CommandErrorCode::InvalidArgument);
    assert_no_fields(&document_id);

    let bad_font = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect,
            name: None,
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
            font_size: Some(1.0),
        },
    )
    .expect_err("a font size below the minimum is rejected");
    assert_eq!(bad_font.code, CommandErrorCode::InvalidArgument);
    assert_no_fields(&document_id);

    let control_name = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect,
            name: Some("bad\nname".to_owned()),
            tooltip: None,
        },
    )
    .expect_err("control characters are rejected");
    assert_eq!(control_name.code, CommandErrorCode::FormFieldNameInvalid);
    assert_no_fields(&document_id);

    // Generated names use per-kind prefixes and avoid every existing field.
    let first = create_form_field(
        document_id.clone(),
        0,
        combo(rect, None, vec!["A".to_owned()], None),
    )
    .expect("create an unnamed dropdown");
    assert!(first
        .fields
        .iter()
        .any(|field| field.field_id == first.field_id && field.name == "combo_box_1"));
    let second = create_form_field(document_id.clone(), 0, list(Vec::new(), false))
        .expect("create an unnamed list");
    assert!(second
        .fields
        .iter()
        .any(|field| field.field_id == second.field_id && field.name == "list_box_1"));
    let signature = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect,
            name: None,
            tooltip: None,
        },
    )
    .expect("create an unnamed signature");
    assert!(signature
        .fields
        .iter()
        .any(|field| field.field_id == signature.field_id && field.name == "signature_1"));
    let duplicate = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect,
            name: Some("combo_box_1".to_owned()),
            tooltip: None,
        },
    )
    .expect_err("names stay unique across kinds");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);

    let unknown_update = update_form_field(
        document_id.clone(),
        "new:99".to_owned(),
        UpdateFormFieldRequest::Signature {
            name: "any".to_owned(),
            tooltip: None,
        },
    )
    .expect_err("only session-created fields can be updated");
    assert_eq!(unknown_update.code, CommandErrorCode::FormFieldNotFound);

    close_document(document_id).expect("close document");
}

#[test]
fn choice_labels_may_be_duplicated_or_off_without_confusing_values() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-choice-labels");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);

    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("states".to_owned()),
            tooltip: None,
            required: true,
            // "Off" is only reserved for button on-state names; a choice label
            // is a PDF text string and may be anything. Duplicates are told
            // apart by /I, and a blank label gets a stable default.
            options: vec!["Off".to_owned(), "Off".to_owned(), String::new()],
            selected_index: Some(1),
            font_size: None,
        },
    )
    .expect("a choice label has no reserved words");
    let field = created
        .fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("the dropdown is discovered");
    assert_eq!(field.options, ["Off", "Off", "选项 3"]);
    assert_eq!(field.value.as_deref(), Some("Off"));
    assert_eq!(field.selected_indices, [1]);
    assert!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .is_empty(),
        "a label that spells Off still answers the field"
    );

    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse the output");
    let saved = field_named(&parsed, "states");
    let options: Vec<String> = saved
        .get(b"Opt")
        .expect("/Opt")
        .as_array()
        .unwrap()
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(options, ["Off", "Off", "选项 3"]);
    assert_eq!(
        decode_pdf_text(saved.get(b"V").expect("/V")),
        "Off",
        "choice values are text strings, not the button Off name"
    );
    assert_eq!(
        saved
            .get(b"I")
            .expect("/I")
            .as_array()
            .unwrap()
            .first()
            .unwrap()
            .as_i64()
            .unwrap(),
        1,
        "/I tells the two Off labels apart"
    );

    // The required gate still sees the saved value: PDFium reports the second
    // Off option as selected and the field as answered.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let field = form_fields(reopened.clone())
        .expect("reopened fields")
        .into_iter()
        .find(|field| field.name == "states")
        .expect("saved dropdown rediscovered");
    assert_eq!(field.value.as_deref(), Some("Off"));
    assert_eq!(field.selected_indices, [1]);
    assert!(missing_required_fields(reopened.clone())
        .expect("required fields")
        .is_empty());
    close_document(reopened).expect("close reopened");

    close_document(document_id).expect("close document");
}

#[test]
fn rejects_invalid_bounds_and_duplicate_names_without_creating_a_field() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };
    let mut with_name = create_request(rect);
    with_name.name = Some("customer_name".to_owned());
    create_form_text_field(document_id.clone(), 0, with_name)
        .expect("create the first named field");

    let mut duplicate = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 24.0,
    });
    duplicate.name = Some("customer_name".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, duplicate)
        .expect_err("duplicate names are rejected");
    assert_eq!(error.code, CommandErrorCode::FormFieldNameDuplicate);

    let fields = form_fields(document_id.clone()).expect("fields");
    assert_eq!(
        fields
            .iter()
            .filter(|field| field.name == "customer_name")
            .count(),
        1,
        "a rejected duplicate does not create a second field"
    );

    let mut empty_name = create_request(FormFieldRect {
        x: 72.0,
        y: 180.0,
        width: 180.0,
        height: 24.0,
    });
    empty_name.name = Some("  ".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, empty_name)
        .expect_err("blank names are rejected");
    assert_eq!(error.code, CommandErrorCode::FormFieldNameInvalid);

    let mut outside = create_request(FormFieldRect {
        x: 500.0,
        y: 700.0,
        width: 200.0,
        height: 100.0,
    });
    outside.name = Some("outside".to_owned());
    let error = create_form_text_field(document_id.clone(), 0, outside)
        .expect_err("a widget must fit on its page");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert_eq!(
        history.revision,
        created_revision(&document_id),
        "rejected creations leave the history untouched"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn updates_checkbox_properties_through_the_history_and_rejects_wrong_values() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: None,
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect("create a checkbox");
    assert_eq!(created.field_id, "new:1");

    let updated = update_form_field(
        document_id.clone(),
        created.field_id.clone(),
        UpdateFormFieldRequest::Checkbox {
            name: "agree_terms".to_owned(),
            tooltip: Some("同意条款".to_owned()),
            required: true,
            checked: true,
        },
    )
    .expect("update the checkbox properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("updated checkbox is discovered");
    assert_eq!(field.name, "agree_terms");
    assert_eq!(field.tooltip.as_deref(), Some("同意条款"));
    assert!(field.required && field.checked);
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    let updated = update_form_field(
        document_id.clone(),
        created.field_id.clone(),
        UpdateFormFieldRequest::Checkbox {
            name: "agree_terms".to_owned(),
            tooltip: Some("同意条款".to_owned()),
            required: true,
            checked: false,
        },
    )
    .expect("uncheck the checkbox property");
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1
    );

    let wrong_value = fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Text {
            text: "not a bool".to_owned(),
        },
    )
    .expect_err("a checkbox does not accept text");
    assert_eq!(wrong_value.code, CommandErrorCode::FormValueInvalid);
    assert!(
        wrong_value.message.contains("选中状态"),
        "{}",
        wrong_value.message
    );

    undo_text_edit(document_id.clone()).expect("undo the uncheck");
    let field = form_fields(document_id.clone())
        .expect("fields after undo")
        .into_iter()
        .find(|field| field.field_id == created.field_id)
        .expect("checkbox restored");
    assert!(field.checked && field.required);
    assert_eq!(field.name, "agree_terms");
    assert_eq!(field.tooltip.as_deref(), Some("同意条款"));
    assert!(missing_required_fields(document_id.clone())
        .expect("required fields")
        .is_empty());

    redo_text_edit(document_id.clone()).expect("redo the uncheck");
    let field = form_fields(document_id.clone())
        .expect("fields after redo")
        .into_iter()
        .find(|field| field.field_id == created.field_id)
        .expect("checkbox restored");
    assert!(!field.checked);

    // The updated state is what the save pipeline writes.
    let dir = support::temp::TempDir::new("create-checkbox-properties");
    let output = dir.path().join("output.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved = field_named(&parsed, "agree_terms");
    assert_eq!(name_text(saved.get(b"V").unwrap()), "Off");
    assert_eq!(name_text(saved.get(b"AS").unwrap()), "Off");
    assert!(saved.get(b"Ff").unwrap().as_i64().unwrap() & 2 == 2);
    assert_eq!(decode_pdf_text(saved.get(b"TU").unwrap()), "同意条款");

    close_document(document_id).expect("close document");
}

#[test]
fn rejects_invalid_checkbox_and_radio_requests_without_creating_fields() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let checkbox_rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 16.0,
        height: 16.0,
    };
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: checkbox_rect,
            name: Some("first".to_owned()),
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect("create the name owner");
    let revision_after_create = created_revision(&document_id);
    let fields_before = form_fields(document_id.clone()).expect("fields after create");

    let duplicate = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 100.0,
                y: 100.0,
                ..checkbox_rect
            },
            name: Some("first".to_owned()),
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect_err("a duplicate checkbox name is rejected");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);
    assert!(
        duplicate.message.contains("“first”") && duplicate.message.contains("已存在"),
        "{}",
        duplicate.message
    );

    let duplicate_radio = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: checkbox_rect,
            name: Some("first".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
        },
    )
    .expect_err("a radio group cannot reuse an existing name");
    assert_eq!(
        duplicate_radio.code,
        CommandErrorCode::FormFieldNameDuplicate
    );

    let blank = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: checkbox_rect,
            name: Some("   ".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
        },
    )
    .expect_err("a blank name is rejected");
    assert_eq!(blank.code, CommandErrorCode::FormFieldNameInvalid);
    assert!(blank.message.contains("不能为空"), "{}", blank.message);

    let outside = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 780.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("outside".to_owned()),
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect_err("a checkbox must fit on its page");
    assert_eq!(outside.code, CommandErrorCode::InvalidArgument);
    assert!(outside.message.contains("页面"), "{}", outside.message);

    let too_small = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 2.0,
                height: 2.0,
            },
            name: Some("tiny".to_owned()),
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect_err("a checkbox below the minimum size is rejected");
    assert_eq!(too_small.code, CommandErrorCode::InvalidArgument);
    assert!(too_small.message.contains("至少"), "{}", too_small.message);

    let too_tall = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 700.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("tall".to_owned()),
            tooltip: None,
            required: false,
            options: (0..10).map(|index| format!("选项{index}")).collect(),
            selected_index: None,
        },
    )
    .expect_err("options that run off the page are rejected");
    assert_eq!(too_tall.code, CommandErrorCode::InvalidArgument);
    assert!(too_tall.message.contains("页面"), "{}", too_tall.message);

    let bad_default = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: checkbox_rect,
            name: Some("bad_default".to_owned()),
            tooltip: None,
            required: false,
            options: Vec::new(),
            selected_index: Some(1),
        },
    )
    .expect_err("a default outside the normalized option list is rejected");
    assert_eq!(bad_default.code, CommandErrorCode::InvalidArgument);

    let missing_update = update_form_field(
        document_id.clone(),
        "new:999".to_owned(),
        UpdateFormFieldRequest::Checkbox {
            name: "ghost".to_owned(),
            tooltip: None,
            required: false,
            checked: false,
        },
    )
    .expect_err("only session-created fields can be updated");
    assert_eq!(missing_update.code, CommandErrorCode::FormFieldNotFound);
    let missing_delete = delete_form_field(document_id.clone(), "new:999".to_owned())
        .expect_err("only session-created fields can be deleted");
    assert_eq!(missing_delete.code, CommandErrorCode::FormFieldNotFound);

    assert_eq!(
        form_fields(document_id.clone()).expect("fields after rejections"),
        fields_before,
        "rejected requests create, change and delete nothing"
    );
    assert_eq!(
        created_revision(&document_id),
        revision_after_create,
        "rejected requests add no history entries"
    );

    let created = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: checkbox_rect,
            name: Some("contact".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned(), "B".to_owned()],
            selected_index: Some(0),
        },
    )
    .expect("a valid radio group still works after the rejections");
    assert_eq!(created.fields.len(), fields_before.len() + 2);

    close_document(document_id).expect("close document");
}

fn created_revision(document_id: &str) -> u64 {
    pdfx_lib::text_edit::edit_history(document_id.to_owned())
        .expect("history")
        .revision
}

#[test]
fn fills_a_created_field_and_the_required_gate_follows_the_value() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("full_name".to_owned());
    request.tooltip = None;
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create a field");

    let missing = missing_required_fields(document_id.clone()).expect("required fields");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].field_id, created.field_id);

    let filled = pdfx_lib::form::fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Text {
            text: "张三".to_owned(),
        },
    )
    .expect("fill the created field without saving");
    let field = filled
        .fields
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("still discovered");
    assert_eq!(field.value.as_deref(), Some("张三"));
    assert!(field.created);
    assert!(filled.history.dirty);
    assert!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .is_empty(),
        "a filled required field no longer blocks saving"
    );

    undo_text_edit(document_id.clone()).expect("undo the value");
    let after_undo = form_fields(document_id.clone()).expect("fields after undo");
    let field = after_undo
        .iter()
        .find(|field| field.field_id == created.field_id)
        .expect("undo restores the field, not its record");
    assert_eq!(field.value, None, "undo restores the empty value");
    assert_eq!(
        missing_required_fields(document_id.clone())
            .expect("required fields")
            .len(),
        1
    );

    redo_text_edit(document_id.clone()).expect("redo the value");
    let after_redo = form_fields(document_id.clone()).expect("fields after redo");
    assert_eq!(
        after_redo
            .iter()
            .find(|field| field.field_id == created.field_id)
            .and_then(|field| field.value.as_deref()),
        Some("张三")
    );

    close_document(document_id).expect("close document");
}

#[test]
fn updates_and_deletes_created_fields_through_the_history() {
    use pdfx_lib::form_create::{
        delete_form_text_field, update_form_text_field, UpdateFormTextFieldRequest,
    };

    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let mut first = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    first.name = Some("first".to_owned());
    let first = create_form_text_field(document_id.clone(), 0, first).expect("first field");
    let mut second = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 24.0,
    });
    second.name = Some("second".to_owned());
    second.required = false;
    second.tooltip = None;
    let second = create_form_text_field(document_id.clone(), 0, second).expect("second field");

    let duplicate = update_form_text_field(
        document_id.clone(),
        second.field_id.clone(),
        UpdateFormTextFieldRequest {
            name: "first".to_owned(),
            tooltip: None,
            required: false,
            multiline: false,
            font_size: 12.0,
        },
    )
    .expect_err("renaming onto another field is rejected");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);
    assert_eq!(
        form_fields(document_id.clone())
            .expect("fields")
            .iter()
            .find(|field| field.field_id == second.field_id)
            .map(|field| field.name.as_str()),
        Some("second"),
        "the rejected rename changes nothing"
    );

    let updated = update_form_text_field(
        document_id.clone(),
        second.field_id.clone(),
        UpdateFormTextFieldRequest {
            name: "  renamed  ".to_owned(),
            tooltip: Some("请输入备注".to_owned()),
            required: true,
            multiline: true,
            font_size: 16.0,
        },
    )
    .expect("update the properties");
    let field = updated
        .fields
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("still discovered");
    assert_eq!(field.name, "renamed");
    assert_eq!(field.tooltip.as_deref(), Some("请输入备注"));
    assert!(field.required);
    assert!(field.multiline);
    assert_eq!(field.font_size, Some(16.0));
    assert_eq!(field.rect.width, 180.0, "property edits keep the geometry");
    assert!(updated.history.dirty);

    undo_text_edit(document_id.clone()).expect("undo the update");
    let restored = form_fields(document_id.clone()).expect("fields after undo");
    let field = restored
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("field");
    assert_eq!(field.name, "second");
    assert_eq!(field.tooltip, None);
    assert!(!field.required && !field.multiline);
    assert_eq!(field.font_size, Some(12.0));

    redo_text_edit(document_id.clone()).expect("redo the update");
    let field = form_fields(document_id.clone())
        .expect("fields after redo")
        .into_iter()
        .find(|field| field.field_id == second.field_id)
        .expect("field");
    assert_eq!(field.name, "renamed");
    assert!(field.required);

    let deleted = delete_form_text_field(document_id.clone(), second.field_id.clone())
        .expect("delete the created field");
    assert!(!deleted.history.can_redo);
    assert!(
        deleted
            .fields
            .iter()
            .all(|field| field.field_id != second.field_id),
        "deletion removes the field and its tab stop"
    );
    assert_eq!(deleted.fields.len(), 1, "the other created field survives");

    undo_text_edit(document_id.clone()).expect("undo the deletion");
    let restored = form_fields(document_id.clone()).expect("fields after undoing deletion");
    let field = restored
        .iter()
        .find(|field| field.field_id == second.field_id)
        .expect("undo restores the deleted field");
    assert_eq!(field.name, "renamed");
    assert_eq!(field.rect.y, 140.0);
    assert!(field.multiline && field.required);

    redo_text_edit(document_id.clone()).expect("redo the deletion");
    assert!(form_fields(document_id.clone())
        .expect("fields after redo deletion")
        .iter()
        .all(|field| field.field_id != second.field_id));
    assert!(form_fields(document_id.clone())
        .expect("first field")
        .iter()
        .any(|field| field.field_id == first.field_id));

    close_document(document_id).expect("close document");
}

// ---------------------------------------------------------------------------
// Save pipeline and independent-reader assertions.
// ---------------------------------------------------------------------------

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::commands::render_page;
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveResultView};

fn save_as(document_id: &str, path: &Path) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save the document")
}

fn save_changes(document_id: &str) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |_| {},
    )
    .expect("save the document changes")
}

fn lopdf_catalog(document: &lopdf::Document) -> &lopdf::Dictionary {
    let root = document
        .trailer
        .get(b"Root")
        .expect("trailer Root")
        .as_reference()
        .expect("Root reference");
    document
        .get_object(root)
        .expect("catalog")
        .as_dict()
        .expect("catalog dictionary")
}

fn lopdf_dict<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> &'a lopdf::Dictionary {
    match object {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("referenced object")
            .as_dict()
            .expect("referenced dictionary"),
        other => other.as_dict().expect("direct dictionary"),
    }
}

fn decode_pdf_text(object: &lopdf::Object) -> String {
    let bytes = object.as_str().expect("text string");
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn name_text(object: &lopdf::Object) -> String {
    String::from_utf8_lossy(object.as_name().expect("name object")).into_owned()
}

/// Button state objects may be names (as PDFX writes them) or strings (as
/// PDFium's fill path serializes them); both spellings mean the same state.
fn state_text(object: &lopdf::Object) -> String {
    match object {
        lopdf::Object::Name(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        other => {
            let text = decode_pdf_text(other);
            text.strip_prefix('/').unwrap_or(&text).to_owned()
        }
    }
}

fn page_annots(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::ObjectId> {
    let page_id = document.get_pages().get(&page_number).copied().unwrap();
    let page = document
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    page.get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("annotation array")
        .iter()
        .map(|annotation| annotation.as_reference().expect("annotation reference"))
        .collect()
}

fn field_named<'a>(document: &'a lopdf::Document, name: &str) -> &'a lopdf::Dictionary {
    let catalog = lopdf_catalog(document);
    let acro_form = lopdf_dict(document, catalog.get(b"AcroForm").expect("/AcroForm"));
    acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .iter()
        .map(|field| lopdf_dict(document, field))
        .find(|field| field.get(b"T").ok().map(decode_pdf_text).as_deref() == Some(name))
        .unwrap_or_else(|| panic!("field {name} in the saved document"))
}

fn stream_content(document: &lopdf::Document, object: &lopdf::Object) -> Vec<u8> {
    let stream = match object {
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("referenced stream")
            .as_stream()
            .expect("stream"),
        other => other.as_stream().expect("stream"),
    };
    stream.get_plain_content().expect("plain stream content")
}

fn numbers(array: &lopdf::Object) -> Vec<f32> {
    array
        .as_array()
        .expect("number array")
        .iter()
        .map(|value| match value {
            lopdf::Object::Integer(value) => *value as f32,
            lopdf::Object::Real(value) => *value,
            other => panic!("not a number: {other:?}"),
        })
        .collect()
}

fn assert_rect_close(actual: &[f32], expected: [f32; 4]) {
    assert_eq!(actual.len(), 4, "rectangle has four numbers");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 0.05,
            "rect {actual:?} differs from {expected:?}"
        );
    }
}

struct DecodedPng {
    width: u32,
    rgba: Vec<u8>,
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    use base64::Engine as _;

    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload should be base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header should parse");
    let mut buffer = vec![0; reader.output_buffer_size().expect("output buffer size")];
    let info = reader
        .next_frame(&mut buffer)
        .expect("PNG frame should decode");
    buffer.truncate(info.buffer_size());
    DecodedPng {
        width: info.width,
        rgba: buffer,
    }
}

/// Non-white pixels inside a page-point region, and their bounding box.
fn ink_in_region(
    pixels: &DecodedPng,
    scale: f32,
    region: (f32, f32, f32, f32),
) -> (usize, (f32, f32, f32, f32)) {
    let (mut left, mut top) = (f32::INFINITY, f32::INFINITY);
    let (mut right, mut bottom) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let mut count = 0;
    for (index, pixel) in pixels.rgba.chunks_exact(4).enumerate() {
        if pixel[0] >= 245 && pixel[1] >= 245 && pixel[2] >= 245 {
            continue;
        }
        let x = (index as u32 % pixels.width) as f32 / scale;
        let y = (index as u32 / pixels.width) as f32 / scale;
        if x < region.0 || x > region.2 || y < region.1 || y > region.3 {
            continue;
        }
        count += 1;
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    (count, (left, top, right, bottom))
}

fn fixture_path(id: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id}"));
    support::fixtures::fixture_path(fixture)
}

#[test]
fn saves_a_created_field_with_ap_da_dr_and_a_cjk_appearance() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read synthetic source");
    let dir = support::temp::TempDir::new("create-text-save");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let rect = FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    };
    let mut request = create_request(rect);
    request.name = Some("full_name".to_owned());
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create field");
    fill_form_field(
        document_id.clone(),
        created.field_id.clone(),
        FormFieldValueInput::Text {
            text: "张三".to_owned(),
        },
    )
    .expect("fill the created field");

    save_as(&document_id, &output);
    assert_eq!(
        fs::read(pdf.path_string()).expect("source after save"),
        source,
        "the Source Document is never modified"
    );
    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source),
        "created fields are appended after the unmodified source bytes"
    );

    // PDFium reopens the output: the field is discovered, fillable and its
    // required flag and value survived.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1, "exactly one widget was written");
    let field = &fields[0];
    assert_eq!(field.name, "full_name");
    assert_eq!(field.kind, FormFieldKind::Text);
    assert_eq!(field.value.as_deref(), Some("张三"));
    assert!(field.required);
    assert!(!field.multiline);
    assert!(!field.created, "saved fields are ordinary widgets");
    assert!((field.rect.x - rect.x).abs() < 0.01);
    assert!((field.rect.y - rect.y).abs() < 0.01);
    assert!((field.rect.width - rect.width).abs() < 0.01);
    assert!((field.rect.height - rect.height).abs() < 0.01);
    assert!(
        missing_required_fields(reopened.clone())
            .expect("required fields")
            .is_empty(),
        "the saved required field is already answered"
    );

    let pixels = render_pixels(&reopened, 0, 2.0);
    let (ink, bbox) = ink_in_region(&pixels, 2.0, (76.0, 104.0, 248.0, 120.0));
    assert!(ink > 20, "the CJK value is visible: {ink} dark pixels");
    assert!(
        bbox.2 - bbox.0 > 12.0,
        "the value is painted horizontally: {bbox:?}"
    );
    close_document(reopened).expect("close reopened");

    // Independently parse the AcroForm structure with lopdf.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert!(
        !acro_form.has(b"NeedAppearances"),
        "generated forms never set /NeedAppearances"
    );
    let saved = field_named(&parsed, "full_name");
    assert_eq!(saved.get(b"FT").expect("/FT").as_name().unwrap(), b"Tx");
    assert_eq!(
        saved.get(b"Subtype").expect("/Subtype").as_name().unwrap(),
        b"Widget"
    );
    assert_eq!(
        decode_pdf_text(saved.get(b"TU").expect("/TU")),
        "请输入姓名"
    );
    let flags = saved.get(b"Ff").expect("/Ff").as_i64().expect("flags");
    assert_eq!(flags & (1 << 1), 1 << 1, "Required bit");
    assert_eq!(flags & (1 << 12), 0, "single-line field");
    assert_eq!(
        decode_pdf_text(saved.get(b"V").expect("/V")),
        "张三",
        "non-ASCII /V is UTF-16BE"
    );
    assert_rect_close(
        &numbers(saved.get(b"Rect").expect("/Rect")),
        [72.0, 668.0, 252.0, 692.0],
    );

    // /DA names the embedded font and carries the chosen size.
    let da = decode_pdf_text(saved.get(b"DA").expect("/DA"));
    assert!(da.contains("PdfxNoto"), "got /DA {da:?}");
    assert!(da.contains("12 Tf"), "got /DA {da:?}");

    // The widget has an explicit /AP /N appearance in the same user rect.
    let appearance = lopdf_dict(&parsed, saved.get(b"AP").expect("/AP"));
    let normal = appearance.get(b"N").expect("/AP /N");
    let normal_stream = parsed
        .get_object(normal.as_reference().expect("appearance reference"))
        .expect("appearance object");
    let appearance_dict = normal_stream
        .as_stream()
        .expect("appearance stream")
        .dict
        .clone();
    assert_rect_close(
        &numbers(appearance_dict.get(b"BBox").expect("/BBox")),
        [72.0, 668.0, 252.0, 692.0],
    );
    let content = String::from_utf8_lossy(&stream_content(&parsed, normal)).into_owned();
    assert!(content.contains("BT /PdfxNoto 12 Tf"), "got {content:?}");
    assert!(
        content.contains(" Tj"),
        "the value is drawn in the appearance"
    );
    assert!(
        content.contains("1 0 0 1 72 668 cm"),
        "the unrotated appearance matrix: {content:?}"
    );

    // The font is a real Type0/CIDFontType2 with ToUnicode and a subset.
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    let type0 = lopdf_dict(&parsed, fonts.get(b"PdfxNoto").expect("PdfxNoto font"));
    assert_eq!(
        type0.get(b"Subtype").expect("/Subtype").as_name().unwrap(),
        b"Type0"
    );
    assert_eq!(
        type0
            .get(b"Encoding")
            .expect("/Encoding")
            .as_name()
            .unwrap(),
        b"Identity-H"
    );
    let descendants = type0
        .get(b"DescendantFonts")
        .expect("/DescendantFonts")
        .as_array()
        .expect("descendant array");
    let cid_font = lopdf_dict(&parsed, &descendants[0]);
    assert_eq!(
        cid_font
            .get(b"Subtype")
            .expect("/Subtype")
            .as_name()
            .unwrap(),
        b"CIDFontType2"
    );
    assert!(cid_font.get(b"CIDToGIDMap").is_ok());
    assert!(cid_font.get(b"W").is_ok());
    let descriptor = lopdf_dict(
        &parsed,
        cid_font.get(b"FontDescriptor").expect("/FontDescriptor"),
    );
    assert!(descriptor.get(b"FontFile2").is_ok());
    let cmap = String::from_utf8_lossy(&stream_content(
        &parsed,
        type0.get(b"ToUnicode").expect("/ToUnicode"),
    ))
    .into_owned();
    assert!(
        cmap.contains("<5F20>"),
        "张 is in the ToUnicode CMap: {cmap}"
    );
    assert!(
        cmap.contains("<4E09>"),
        "三 is in the ToUnicode CMap: {cmap}"
    );
    let program = stream_content(&parsed, descriptor.get(b"FontFile2").expect("/FontFile2"));
    assert!(
        program.starts_with(&[0x00, 0x01, 0x00, 0x00]),
        "FontFile2 is a TrueType program"
    );

    // The widget is wired into the page's /Annots and the form's /Fields.
    let page_id = parsed.get_pages().get(&1).copied().expect("page 1");
    let page = parsed.get_object(page_id).expect("page").as_dict().unwrap();
    let annots = page.get(b"Annots").expect("/Annots").as_array().unwrap();
    assert_eq!(annots.len(), 1);
    assert_eq!(
        saved.get(b"P").expect("/P").as_reference().unwrap(),
        page_id
    );
    assert_eq!(
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .unwrap()
            .len(),
        1
    );

    close_document(document_id).expect("close document");
}

#[test]
fn saves_checkbox_and_radio_widget_structures_that_other_readers_agree_on() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read synthetic source");
    let dir = support::temp::TempDir::new("create-check-radio-save");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("agree".to_owned()),
            tooltip: Some("同意条款".to_owned()),
            required: true,
            checked: true,
        },
    )
    .expect("create a checkbox");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 140.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("contact".to_owned()),
            tooltip: Some("联系方式".to_owned()),
            required: true,
            options: vec!["Email".to_owned(), "Phone".to_owned()],
            selected_index: Some(1),
        },
    )
    .expect("create a radio group");

    save_as(&document_id, &output);
    assert_eq!(
        fs::read(pdf.path_string()).expect("source after save"),
        source,
        "the Source Document is never modified"
    );
    let output_bytes = fs::read(&output).expect("read output");
    assert!(output_bytes.starts_with(&source));
    assert!(
        !output_bytes.windows(4).any(|window| window == b"new:"),
        "session ids never reach the saved file"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 3);
    let checkbox = fields
        .iter()
        .find(|field| field.name == "agree")
        .expect("the checkbox is rediscovered");
    assert_eq!(checkbox.kind, FormFieldKind::Checkbox);
    assert!(checkbox.checked && checkbox.required && !checkbox.created);
    let group: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "contact")
        .collect();
    assert_eq!(group.len(), 2);
    assert!(group.iter().all(|field| field.kind == FormFieldKind::Radio
        && field.required
        && !field.created
        && field.radio_group.as_deref() == Some("contact")
        && field.value.as_deref() == Some("Phone")));
    assert_eq!(
        group
            .iter()
            .map(|field| field.export_value.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Email"), Some("Phone")]
    );
    assert_eq!(
        group.iter().map(|field| field.checked).collect::<Vec<_>>(),
        vec![false, true]
    );
    assert!(missing_required_fields(reopened.clone())
        .expect("required fields")
        .is_empty());

    let pixels = render_pixels(&reopened, 0, 2.0);
    let (selected_ink, _) = ink_in_region(&pixels, 2.0, (72.0, 160.0, 88.0, 176.0));
    let (unselected_ink, _) = ink_in_region(&pixels, 2.0, (72.0, 140.0, 88.0, 156.0));
    assert!(
        selected_ink > unselected_ink,
        "the selected radio option paints its dot: {selected_ink} vs {unselected_ink}"
    );
    let (checkbox_ink, _) = ink_in_region(&pixels, 2.0, (72.0, 100.0, 88.0, 116.0));
    assert!(checkbox_ink > 10, "the checked checkbox paints a mark");
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert!(
        acro_form.get(b"NeedAppearances").is_err(),
        "generated widgets carry static appearances"
    );
    assert!(
        acro_form.get(b"DR").is_err() && acro_form.get(b"DA").is_err(),
        "checkbox and radio widgets need no font resources"
    );
    let field_refs: Vec<lopdf::ObjectId> = acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .iter()
        .map(|field| field.as_reference().expect("field reference"))
        .collect();
    assert_eq!(field_refs.len(), 2, "the radio group is one parent field");
    assert_eq!(
        decode_pdf_text(
            lopdf_dict(
                &parsed,
                &acro_form.get(b"Fields").unwrap().as_array().unwrap()[0]
            )
            .get(b"T")
            .unwrap()
        ),
        "agree"
    );
    let checkbox_dict = parsed
        .get_object(field_refs[0])
        .expect("checkbox object")
        .as_dict()
        .expect("checkbox dictionary");
    assert_eq!(name_text(checkbox_dict.get(b"FT").unwrap()), "Btn");
    let flags = checkbox_dict.get(b"Ff").unwrap().as_i64().unwrap();
    assert_eq!(flags & 2, 2, "the required bit is set");
    assert_eq!(flags & (1 << 15), 0, "a checkbox is not a radio group");
    assert_eq!(flags & (1 << 12), 0);
    let state = name_text(checkbox_dict.get(b"V").unwrap());
    assert_eq!(state, "Yes");
    assert_eq!(name_text(checkbox_dict.get(b"AS").unwrap()), state);
    assert_eq!(
        decode_pdf_text(checkbox_dict.get(b"TU").unwrap()),
        "同意条款"
    );
    assert_eq!(checkbox_dict.get(b"F").unwrap().as_i64().unwrap(), 4);
    assert!(checkbox_dict.get(b"P").unwrap().as_reference().is_ok());
    assert_rect_close(
        &numbers(checkbox_dict.get(b"Rect").unwrap()),
        [72.0, 676.0, 88.0, 692.0],
    );
    let ap = lopdf_dict(&parsed, checkbox_dict.get(b"AP").unwrap());
    let normal = lopdf_dict(&parsed, ap.get(b"N").unwrap());
    let off_content = stream_content(&parsed, normal.get(b"Off").expect("Off appearance"));
    let on_content = stream_content(&parsed, normal.get(b"Yes").expect("Yes appearance"));
    assert!(!off_content.is_empty() && !on_content.is_empty());
    assert_ne!(off_content, on_content, "the two states paint differently");

    let parent_dict = parsed
        .get_object(field_refs[1])
        .expect("radio parent object")
        .as_dict()
        .expect("radio parent dictionary");
    assert_eq!(name_text(parent_dict.get(b"FT").unwrap()), "Btn");
    let flags = parent_dict.get(b"Ff").unwrap().as_i64().unwrap();
    assert_ne!(flags & (1 << 15), 0, "the radio bit is set");
    assert_eq!(flags & 2, 2, "the required bit is set");
    assert!(
        parent_dict.get(b"Rect").is_err(),
        "the parent field has no widget of its own"
    );
    assert_eq!(name_text(parent_dict.get(b"V").unwrap()), "Phone");
    assert_eq!(decode_pdf_text(parent_dict.get(b"T").unwrap()), "contact");
    assert_eq!(decode_pdf_text(parent_dict.get(b"TU").unwrap()), "联系方式");
    let kids = parent_dict
        .get(b"Kids")
        .expect("/Kids")
        .as_array()
        .expect("kid array");
    assert_eq!(kids.len(), 2);
    let mut export_values = Vec::new();
    for (index, kid_object) in kids.iter().enumerate() {
        let kid = lopdf_dict(&parsed, kid_object);
        assert_eq!(
            kid.get(b"Parent")
                .expect("/Parent")
                .as_reference()
                .expect("parent reference"),
            field_refs[1],
            "each widget points at the parent field"
        );
        assert!(kid.get(b"T").is_err(), "kids do not repeat the field name");
        assert_eq!(kid.get(b"F").unwrap().as_i64().unwrap(), 4);
        assert_rect_close(
            &numbers(kid.get(b"Rect").unwrap()),
            [
                72.0,
                792.0 - (156.0 + index as f32 * 20.0),
                88.0,
                792.0 - (140.0 + index as f32 * 20.0),
            ],
        );
        let export = if index == 0 { "Email" } else { "Phone" };
        export_values.push(export.to_owned());
        assert_eq!(
            name_text(kid.get(b"AS").unwrap()),
            if index == 1 { "Phone" } else { "Off" },
            "kid /AS agrees with the parent /V"
        );
        let ap = lopdf_dict(&parsed, kid.get(b"AP").unwrap());
        let normal = lopdf_dict(&parsed, ap.get(b"N").unwrap());
        assert!(normal.get(b"Off").is_ok());
        assert!(
            normal.get(export.as_bytes()).is_ok(),
            "option {index} has an on-state named {export}"
        );
        assert_ne!(
            stream_content(&parsed, normal.get(b"Off").unwrap()),
            stream_content(&parsed, normal.get(export.as_bytes()).unwrap()),
            "the selected state paints its dot"
        );
    }
    export_values.sort();
    assert_eq!(export_values, vec!["Email", "Phone"]);

    let pages = parsed.get_pages();
    let page_id = pages.get(&1).copied().expect("first page");
    let page = parsed
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    let annots: Vec<lopdf::ObjectId> = page
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("annotation array")
        .iter()
        .map(|annotation| annotation.as_reference().expect("annotation reference"))
        .collect();
    assert_eq!(annots.len(), 3, "checkbox plus two radio widgets");
    assert!(annots.contains(&field_refs[0]));
    assert!(annots.contains(&kids[0].as_reference().unwrap()));
    assert!(annots.contains(&kids[1].as_reference().unwrap()));

    // A second save re-applies the same writer without duplicating structure.
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse the repeat save");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        2
    );
    let page = parsed
        .get_object(parsed.get_pages().get(&1).copied().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(page.get(b"Annots").unwrap().as_array().unwrap().len(), 3);
    let parent = field_named(&parsed, "contact");
    assert_eq!(parent.get(b"Kids").unwrap().as_array().unwrap().len(), 2);

    close_document(document_id).expect("close document");
}

#[test]
fn saves_choice_and_signature_structures_that_other_readers_agree_on() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read synthetic source");
    let dir = support::temp::TempDir::new("create-choice-signature-save");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let combo = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("country".to_owned()),
            tooltip: Some("请选择国家".to_owned()),
            required: true,
            options: vec!["中国".to_owned(), "Japan".to_owned()],
            selected_index: Some(1),
            font_size: Some(14.0),
        },
    )
    .expect("create the dropdown");
    fill_form_field(
        document_id.clone(),
        combo.field_id,
        FormFieldValueInput::Choice {
            value: "中国".to_owned(),
        },
    )
    .expect("pick the first option before saving");

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 140.0,
                width: 180.0,
                height: 48.0,
            },
            name: Some("tags".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
            selected_indices: vec![0, 2],
            multi_select: true,
            font_size: Some(10.0),
        },
    )
    .expect("create the multi-select list");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 200.0,
                width: 180.0,
                height: 48.0,
            },
            name: Some("level".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["Low".to_owned(), "High".to_owned()],
            selected_indices: Vec::new(),
            multi_select: false,
            font_size: None,
        },
    )
    .expect("create an unanswered list");
    let signature = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                x: 72.0,
                y: 300.0,
                width: 240.0,
                height: 80.0,
            },
            name: Some("sign_here".to_owned()),
            tooltip: Some("请在此签名".to_owned()),
        },
    )
    .expect("create the signature placeholder");
    assert_eq!(signature.field_id, "new:4");

    save_as(&document_id, &output);
    assert_eq!(
        fs::read(pdf.path_string()).expect("source after save"),
        source,
        "the Source Document is never modified"
    );
    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source),
        "created fields are appended after the unmodified source bytes"
    );

    // PDFium reopens the output and reports the same structure it would show
    // for a form authored elsewhere.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 4, "one widget per created field");
    assert!(fields.iter().all(|field| !field.created));

    let country = fields
        .iter()
        .find(|field| field.name == "country")
        .expect("dropdown rediscovered");
    assert_eq!(country.kind, FormFieldKind::ComboBox);
    assert_eq!(country.options, ["中国", "Japan"]);
    assert_eq!(country.value.as_deref(), Some("中国"));
    assert_eq!(country.selected_indices, [0]);
    assert!(!country.multi_select);
    assert!(country.required);

    let tags = fields
        .iter()
        .find(|field| field.name == "tags")
        .expect("multi-select list rediscovered");
    assert_eq!(tags.kind, FormFieldKind::ListBox);
    assert_eq!(tags.options, ["A", "B", "C"]);
    let mut selected = tags.selected_indices.clone();
    selected.sort_unstable();
    assert_eq!(selected, [0, 2], "both defaults are selected");
    assert!(tags.multi_select);

    let level = fields
        .iter()
        .find(|field| field.name == "level")
        .expect("unanswered list rediscovered");
    assert_eq!(level.kind, FormFieldKind::ListBox);
    assert_eq!(level.value, None);
    assert!(level.selected_indices.is_empty());
    assert!(!level.multi_select);

    let saved_signature = fields
        .iter()
        .find(|field| field.name == "sign_here")
        .expect("signature placeholder rediscovered");
    assert_eq!(saved_signature.kind, FormFieldKind::Signature);
    assert_eq!(saved_signature.value, None);
    assert!(!saved_signature.required);
    assert!(
        missing_required_fields(reopened.clone())
            .expect("required fields")
            .is_empty(),
        "the answered dropdown and the unfillable signature do not block saving"
    );

    let pixels = render_pixels(&reopened, 0, 2.0);
    let (combo_ink, _) = ink_in_region(&pixels, 2.0, (74.0, 103.0, 210.0, 121.0));
    assert!(combo_ink > 10, "the dropdown value is painted: {combo_ink}");
    let (signature_ink, _) = ink_in_region(&pixels, 2.0, (74.0, 302.0, 310.0, 378.0));
    assert!(
        signature_ink > 10,
        "the empty signature area is visible: {signature_ink}"
    );
    close_document(reopened).expect("close reopened");

    // Independently parse the AcroForm structure with lopdf.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert!(
        !acro_form.has(b"NeedAppearances"),
        "generated forms never set /NeedAppearances"
    );
    let sig_flags = acro_form
        .get(b"SigFlags")
        .expect("/SigFlags")
        .as_i64()
        .expect("signature flags");
    assert_eq!(sig_flags & 1, 1, "SignaturesExist");
    assert_eq!(sig_flags & 2, 0, "an empty placeholder is not AppendOnly");

    let saved_combo = field_named(&parsed, "country");
    assert_eq!(
        saved_combo.get(b"FT").expect("/FT").as_name().unwrap(),
        b"Ch"
    );
    let combo_flags = saved_combo.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(combo_flags & (1 << 17), 1 << 17, "Combo bit");
    assert_eq!(combo_flags & (1 << 21), 0, "a dropdown is not multi-select");
    assert_eq!(combo_flags & (1 << 1), 1 << 1, "Required bit");
    let combo_options: Vec<String> = saved_combo
        .get(b"Opt")
        .expect("/Opt")
        .as_array()
        .unwrap()
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(combo_options, ["中国", "Japan"]);
    assert_eq!(
        decode_pdf_text(saved_combo.get(b"V").expect("/V")),
        "中国",
        "non-ASCII /V is UTF-16BE"
    );
    let combo_indices: Vec<i64> = saved_combo
        .get(b"I")
        .expect("/I")
        .as_array()
        .unwrap()
        .iter()
        .map(|index| index.as_i64().unwrap())
        .collect();
    assert_eq!(combo_indices, [0]);
    let combo_da = decode_pdf_text(saved_combo.get(b"DA").expect("/DA"));
    assert!(combo_da.contains("PdfxNoto"), "got /DA {combo_da:?}");
    assert!(combo_da.contains("14 Tf"), "got /DA {combo_da:?}");
    assert_rect_close(
        &numbers(saved_combo.get(b"Rect").expect("/Rect")),
        [72.0, 668.0, 252.0, 692.0],
    );
    for action in [b"AA".as_slice(), b"A".as_slice()] {
        assert!(
            saved_combo.get(action).is_err(),
            "choice fields carry no actions"
        );
    }

    let saved_tags = field_named(&parsed, "tags");
    let tag_flags = saved_tags.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(tag_flags & (1 << 21), 1 << 21, "MultiSelect bit");
    assert_eq!(tag_flags & (1 << 17), 0, "a list box is not a combo box");
    let tag_options: Vec<String> = saved_tags
        .get(b"Opt")
        .expect("/Opt")
        .as_array()
        .unwrap()
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(tag_options, ["A", "B", "C"]);
    let tag_values: Vec<String> = saved_tags
        .get(b"V")
        .expect("/V")
        .as_array()
        .expect("multi-select /V is an array")
        .iter()
        .map(decode_pdf_text)
        .collect();
    assert_eq!(tag_values, ["A", "C"]);
    let tag_indices: Vec<i64> = saved_tags
        .get(b"I")
        .expect("/I")
        .as_array()
        .unwrap()
        .iter()
        .map(|index| index.as_i64().unwrap())
        .collect();
    assert_eq!(tag_indices, [0, 2]);
    let tags_da = decode_pdf_text(saved_tags.get(b"DA").expect("/DA"));
    assert!(tags_da.contains("10 Tf"), "got /DA {tags_da:?}");

    let saved_level = field_named(&parsed, "level");
    assert!(
        saved_level.get(b"V").is_err(),
        "an unanswered list has no /V"
    );
    assert!(
        saved_level.get(b"I").is_err(),
        "an unanswered list has no /I"
    );
    let level_ap = lopdf_dict(&parsed, saved_level.get(b"AP").expect("/AP"));
    let level_content =
        String::from_utf8_lossy(&stream_content(&parsed, level_ap.get(b"N").unwrap())).into_owned();
    assert!(
        level_content.contains("Tj"),
        "even an empty list paints its options: {level_content:?}"
    );

    let saved_sig = field_named(&parsed, "sign_here");
    assert_eq!(
        saved_sig.get(b"FT").expect("/FT").as_name().unwrap(),
        b"Sig"
    );
    assert_eq!(
        saved_sig.get(b"Ff").expect("/Ff").as_i64().unwrap(),
        0,
        "signature fields are never required or otherwise flagged"
    );
    assert_eq!(
        decode_pdf_text(saved_sig.get(b"TU").expect("/TU")),
        "请在此签名"
    );
    for forbidden in [
        b"V".as_slice(),
        b"SV".as_slice(),
        b"Lock".as_slice(),
        b"AA".as_slice(),
    ] {
        assert!(
            saved_sig.get(forbidden).is_err(),
            "an empty signature placeholder writes no /{}",
            String::from_utf8_lossy(forbidden)
        );
    }
    let page_id = parsed.get_pages().get(&1).copied().expect("page 1");
    assert_eq!(
        saved_sig.get(b"P").expect("/P").as_reference().unwrap(),
        page_id
    );
    assert_rect_close(
        &numbers(saved_sig.get(b"Rect").expect("/Rect")),
        [72.0, 412.0, 312.0, 492.0],
    );
    let sig_ap = lopdf_dict(&parsed, saved_sig.get(b"AP").expect("/AP"));
    let sig_normal = sig_ap.get(b"N").expect("/AP /N");
    let sig_stream = parsed
        .get_object(sig_normal.as_reference().expect("appearance reference"))
        .expect("signature appearance")
        .as_stream()
        .expect("stream");
    assert_rect_close(
        &numbers(sig_stream.dict.get(b"BBox").expect("/BBox")),
        [72.0, 412.0, 312.0, 492.0],
    );
    let sig_content =
        String::from_utf8_lossy(&sig_stream.get_plain_content().unwrap()).into_owned();
    assert!(
        sig_content.contains("/PdfxNoto"),
        "the Chinese placeholder label uses the embedded font: {sig_content:?}"
    );
    assert!(
        sig_content.contains("Tj"),
        "the Chinese 签名 label is drawn: {sig_content:?}"
    );

    // The subset really contains the choice labels and the signature label.
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    let type0 = lopdf_dict(&parsed, fonts.get(b"PdfxNoto").expect("PdfxNoto font"));
    let cmap = String::from_utf8_lossy(&stream_content(
        &parsed,
        type0.get(b"ToUnicode").expect("/ToUnicode"),
    ))
    .into_owned();
    for codepoint in ["<4E2D>", "<56FD>", "<7B7E>", "<540D>"] {
        assert!(
            cmap.contains(codepoint),
            "{codepoint} is in the ToUnicode CMap: {cmap}"
        );
    }

    // Every field name is unique across the saved form.
    let names: Vec<String> = acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .unwrap()
        .iter()
        .map(|field| decode_pdf_text(lopdf_dict(&parsed, field).get(b"T").unwrap()))
        .collect();
    assert_eq!(names.len(), 4);
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), 4, "field names stay unique: {names:?}");
    assert_eq!(page_annots(&parsed, 1).len(), 4);

    // A second save re-applies the same writer without duplicating structure.
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse the repeat save");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        4
    );
    assert_eq!(page_annots(&parsed, 1).len(), 4);
    assert!(
        field_named(&parsed, "sign_here").get(b"SigFlags").is_err(),
        "SigFlags belongs to /AcroForm, not the field"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn saved_widgets_fill_through_the_shared_path_and_keep_values_consistent() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-check-radio-refill");
    let first_output = dir.path().join("first.pdf");
    let second_output = dir.path().join("second.pdf");
    let document_id = open_synthetic(&pdf);

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("agree".to_owned()),
            tooltip: None,
            required: true,
            checked: false,
        },
    )
    .expect("create a required checkbox");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 140.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("pick".to_owned()),
            tooltip: None,
            required: true,
            options: vec!["A".to_owned(), "B".to_owned()],
            selected_index: None,
        },
    )
    .expect("create a required radio group");
    save_as(&document_id, &first_output);

    let reopened = open_document(first_output.to_string_lossy().into_owned(), None)
        .expect("reopen the first save")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(fields.len(), 3);
    assert_eq!(
        missing_required_fields(reopened.clone())
            .expect("required fields")
            .len(),
        2,
        "the saved unchecked checkbox and unanswered radio both block saving"
    );
    let checkbox_id = fields
        .iter()
        .find(|field| field.name == "agree")
        .expect("checkbox")
        .field_id
        .clone();
    let radio_b_id = fields
        .iter()
        .find(|field| field.name == "pick" && field.export_value.as_deref() == Some("B"))
        .expect("second radio option")
        .field_id
        .clone();

    fill_form_field(
        reopened.clone(),
        checkbox_id,
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("check the saved checkbox");
    fill_form_field(
        reopened.clone(),
        radio_b_id,
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select the saved radio option");
    assert!(missing_required_fields(reopened.clone())
        .expect("required fields after filling")
        .is_empty());
    save_as(&reopened, &second_output);
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&second_output).expect("parse the second save");
    let checkbox = field_named(&parsed, "agree");
    assert_eq!(state_text(checkbox.get(b"V").unwrap()), "Yes");
    assert_eq!(state_text(checkbox.get(b"AS").unwrap()), "Yes");
    let parent = field_named(&parsed, "pick");
    assert_eq!(state_text(parent.get(b"V").unwrap()), "B");
    let kids = parent.get(b"Kids").unwrap().as_array().unwrap();
    assert_eq!(
        state_text(lopdf_dict(&parsed, &kids[0]).get(b"AS").unwrap()),
        "Off"
    );
    assert_eq!(
        state_text(lopdf_dict(&parsed, &kids[1]).get(b"AS").unwrap()),
        "B"
    );

    let reopened = open_document(second_output.to_string_lossy().into_owned(), None)
        .expect("reopen the second save")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after refill");
    assert!(
        fields
            .iter()
            .find(|field| field.name == "agree")
            .expect("checkbox")
            .checked
    );
    assert_eq!(
        fields
            .iter()
            .filter(|field| field.name == "pick")
            .map(|field| field.checked)
            .collect::<Vec<_>>(),
        vec![false, true]
    );
    assert!(missing_required_fields(reopened.clone())
        .expect("required fields after refill save")
        .is_empty());
    close_document(reopened).expect("close reopened");

    close_document(document_id).expect("close document");
}

#[test]
fn repeated_saves_never_duplicate_created_fields() {
    let pdf = synthetic::single_line_pdf();
    let source = fs::read(pdf.path_string()).expect("read source");
    let dir = support::temp::TempDir::new("create-text-repeat");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let mut first = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    first.name = Some("first".to_owned());
    first.required = false;
    let first = create_form_text_field(document_id.clone(), 0, first).expect("first field");
    save_as(&document_id, &output);

    // Fill after the first save, then create a multiline field and Save Changes.
    fill_form_field(
        document_id.clone(),
        first.field_id.clone(),
        FormFieldValueInput::Text {
            text: "第一版".to_owned(),
        },
    )
    .expect("fill after save");
    let mut second = create_request(FormFieldRect {
        x: 72.0,
        y: 140.0,
        width: 180.0,
        height: 48.0,
    });
    second.name = Some("second".to_owned());
    second.required = false;
    second.multiline = true;
    let second = create_form_text_field(document_id.clone(), 0, second).expect("second field");
    fill_form_field(
        document_id.clone(),
        second.field_id.clone(),
        FormFieldValueInput::Text {
            text: "第一行\n第二行".to_owned(),
        },
    )
    .expect("fill the multiline field");
    save_changes(&document_id);
    assert!(fs::read(&output).expect("output").starts_with(&source));

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let fields = parsed
        .catalog()
        .and_then(|catalog| catalog.get_deref(b"AcroForm", &parsed))
        .and_then(lopdf::Object::as_dict)
        .and_then(|acro_form| acro_form.get_deref(b"Fields", &parsed))
        .and_then(lopdf::Object::as_array)
        .expect("/Fields");
    assert_eq!(fields.len(), 2, "one /Fields entry per created field");
    let second_dict = field_named(&parsed, "second");
    let flags = second_dict.get(b"Ff").expect("/Ff").as_i64().unwrap();
    assert_eq!(flags & (1 << 12), 1 << 12, "Multiline bit");
    let appearance = lopdf_dict(&parsed, second_dict.get(b"AP").unwrap());
    let content = String::from_utf8_lossy(&stream_content(&parsed, appearance.get(b"N").unwrap()))
        .into_owned();
    assert_eq!(
        content.matches(" Tm ").count(),
        2,
        "multiline values get one Tm per line: {content}"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "first")
            .and_then(|field| field.value.as_deref()),
        Some("第一版")
    );
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "second")
            .and_then(|field| field.value.as_deref()),
        Some("第一行\n第二行")
    );
    assert_eq!(
        fields.iter().filter(|field| field.name == "first").count(),
        1,
        "repeated saves do not duplicate the field"
    );
    close_document(reopened).expect("close reopened");

    // Undo the second creation, save again: the output loses exactly that
    // field and keeps the first one's saved value.
    undo_text_edit(document_id.clone()).expect("undo the multiline fill");
    undo_text_edit(document_id.clone()).expect("undo the second creation");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after undo");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .unwrap()
            .len(),
        1,
        "the undone field is not written"
    );
    assert_eq!(
        decode_pdf_text(field_named(&parsed, "first").get(b"V").expect("/V")),
        "第一版"
    );
    let page_id = parsed.get_pages().get(&1).copied().unwrap();
    let page = parsed.get_object(page_id).unwrap().as_dict().unwrap();
    assert_eq!(
        page.get(b"Annots").unwrap().as_array().unwrap().len(),
        1,
        "the undone widget leaves the page /Annots"
    );

    // Redo the creation and save: it comes back once.
    redo_text_edit(document_id.clone()).expect("redo the second creation");
    redo_text_edit(document_id.clone()).expect("redo the multiline fill");
    save_changes(&document_id);
    let parsed = lopdf::Document::load(&output).expect("parse output after redo");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        2
    );

    close_document(document_id).expect("close document");
}

#[test]
fn rotated_pages_get_upright_compensated_appearances() {
    let pdf = synthetic::rotated_crop_pdf();
    let dir = support::temp::TempDir::new("create-text-rotated");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    for page_index in 0..3u32 {
        let mut request = create_request(FormFieldRect {
            x: 40.0,
            y: 200.0,
            width: 180.0,
            height: 30.0,
        });
        request.name = Some(format!("rotated_{page_index}"));
        request.tooltip = None;
        request.required = false;
        let created = create_form_text_field(document_id.clone(), page_index, request)
            .expect("create a field on a rotated page");
        fill_form_field(
            document_id.clone(),
            created.field_id,
            FormFieldValueInput::Text {
                text: "MMMM".to_owned(),
            },
        )
        .expect("fill the rotated field");
    }
    save_as(&document_id, &output);

    // Display rect (40 200 180 30) back to user space on each crop+rotation.
    let expected_rects = [
        [250.0, 120.0, 280.0, 300.0], // /Rotate 90
        [230.0, 280.0, 410.0, 310.0], // /Rotate 180
        [220.0, 460.0, 250.0, 640.0], // /Rotate 270
    ];
    let expected_matrices = [
        "0 1 -1 0 280 120 cm",
        "-1 0 0 -1 410 310 cm",
        "0 -1 1 0 220 640 cm",
    ];
    let parsed = lopdf::Document::load(&output).expect("parse output");
    for (page_index, (expected_rect, expected_matrix)) in expected_rects
        .iter()
        .zip(expected_matrices.iter())
        .enumerate()
    {
        let field = field_named(&parsed, &format!("rotated_{page_index}"));
        assert_rect_close(&numbers(field.get(b"Rect").expect("/Rect")), *expected_rect);
        let appearance = lopdf_dict(&parsed, field.get(b"AP").expect("/AP"));
        let normal = appearance.get(b"N").expect("/AP /N");
        let stream = match normal {
            lopdf::Object::Reference(id) => parsed.get_object(*id).unwrap().as_stream().unwrap(),
            other => other.as_stream().unwrap(),
        };
        assert_rect_close(
            &numbers(stream.dict.get(b"BBox").expect("/BBox")),
            *expected_rect,
        );
        let content = String::from_utf8_lossy(
            &stream
                .get_plain_content()
                .expect("plain appearance content"),
        )
        .into_owned();
        assert!(
            content.contains(expected_matrix),
            "page {page_index} appearance matrix: {content}"
        );
    }

    // The reopened document paints each value upright: the ink box of a wide
    // four-glyph word is wider than it is tall inside a 180x30 widget.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    for page_index in 0..3u32 {
        let pixels = render_pixels(&reopened, page_index, 2.0);
        let (ink, bbox) = ink_in_region(&pixels, 2.0, (42.0, 202.0, 218.0, 228.0));
        assert!(
            ink > 40,
            "page {page_index} paints the appearance: {ink} dark pixels"
        );
        assert!(
            bbox.2 - bbox.0 > 2.0 * (bbox.3 - bbox.1),
            "page {page_index} text is upright, not sideways: {bbox:?}"
        );
    }
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn created_field_appearance_follows_a_later_page_rotation() {
    let pdf = synthetic::rotated_crop_pdf();
    let dir = support::temp::TempDir::new("create-text-rerotated");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let mut request = create_request(FormFieldRect {
        x: 40.0,
        y: 200.0,
        width: 180.0,
        height: 30.0,
    });
    request.name = Some("rerotated".to_owned());
    request.tooltip = None;
    request.required = false;
    create_form_text_field(document_id.clone(), 0, request)
        .expect("create a field on the rotated first page");

    // Rotate the page back to 0 after the field exists. The widget keeps its
    // user-space rect, so its appearance matrix must be re-derived from the
    // page's current rotation rather than the rotation at creation time.
    pdfx_lib::page_ops::rotate_page(document_id.clone(), 0, 270)
        .expect("rotate the page back to zero");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let page_id = *parsed.get_pages().get(&1).expect("page one");
    let page = parsed
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    assert_eq!(
        page.get(b"Rotate")
            .and_then(lopdf::Object::as_i64)
            .unwrap_or(0),
        0,
        "the rotation itself is persisted"
    );
    let field = field_named(&parsed, "rerotated");
    let rect = numbers(field.get(b"Rect").expect("/Rect"));
    assert_rect_close(&rect, [250.0, 120.0, 280.0, 300.0]);
    let appearance = lopdf_dict(&parsed, field.get(b"AP").expect("/AP"));
    let normal = appearance.get(b"N").expect("/AP /N");
    let stream = match normal {
        lopdf::Object::Reference(id) => parsed.get_object(*id).unwrap().as_stream().unwrap(),
        other => other.as_stream().unwrap(),
    };
    let content = String::from_utf8_lossy(
        &stream
            .get_plain_content()
            .expect("plain appearance content"),
    )
    .into_owned();
    assert!(
        content.contains("1 0 0 1 250 120 cm"),
        "appearance must use the current zero rotation: {content}"
    );

    // The field is still attached and discoverable after reopening.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("discover fields");
    assert!(fields.iter().any(|field| field.name == "rerotated"));
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn checkbox_and_radio_appearances_compensate_page_rotation() {
    let pdf = synthetic::rotated_crop_pdf();
    let dir = support::temp::TempDir::new("create-check-radio-rotated");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);

    for page_index in 0..3u32 {
        create_form_field(
            document_id.clone(),
            page_index,
            CreateFormFieldRequest::Checkbox {
                rect: FormFieldRect {
                    x: 40.0,
                    y: 200.0,
                    width: 180.0,
                    height: 30.0,
                },
                name: Some(format!("rotated_agree_{page_index}")),
                tooltip: None,
                required: false,
                checked: true,
            },
        )
        .expect("create a checkbox on a rotated page");
        create_form_field(
            document_id.clone(),
            page_index,
            CreateFormFieldRequest::Radio {
                rect: FormFieldRect {
                    x: 40.0,
                    y: 250.0,
                    width: 180.0,
                    height: 30.0,
                },
                name: Some(format!("rotated_pick_{page_index}")),
                tooltip: None,
                required: false,
                options: vec!["A".to_owned()],
                selected_index: Some(0),
            },
        )
        .expect("create a radio group on a rotated page");
    }
    save_as(&document_id, &output);

    // The same display rect as the ticket-31 rotation test: its user-space
    // rectangles and appearance matrices are already known-good literals.
    let expected_rects = [
        [250.0, 120.0, 280.0, 300.0], // /Rotate 90
        [230.0, 280.0, 410.0, 310.0], // /Rotate 180
        [220.0, 460.0, 250.0, 640.0], // /Rotate 270
    ];
    let expected_matrices = [
        "0 1 -1 0 280 120 cm",
        "-1 0 0 -1 410 310 cm",
        "0 -1 1 0 220 640 cm",
    ];
    let rotations = [90u16, 180, 270];
    let parsed = lopdf::Document::load(&output).expect("parse output");
    for page_index in 0..3usize {
        let checkbox = field_named(&parsed, &format!("rotated_agree_{page_index}"));
        assert_rect_close(
            &numbers(checkbox.get(b"Rect").expect("/Rect")),
            expected_rects[page_index],
        );
        let ap = lopdf_dict(&parsed, checkbox.get(b"AP").expect("/AP"));
        let normal = lopdf_dict(&parsed, ap.get(b"N").expect("/AP /N"));
        let off = normal.get(b"Off").expect("Off state");
        let on = normal.get(b"Yes").expect("Yes state");
        for (state, object) in [("Off", off), ("Yes", on)] {
            let stream = parsed
                .get_object(object.as_reference().expect("stream reference"))
                .expect("appearance stream")
                .as_stream()
                .expect("stream");
            assert_rect_close(
                &numbers(stream.dict.get(b"BBox").expect("/BBox")),
                expected_rects[page_index],
            );
            let content =
                String::from_utf8_lossy(&stream.get_plain_content().expect("plain appearance"))
                    .into_owned();
            assert!(
                content.contains(expected_matrices[page_index]),
                "checkbox {state} appearance on page {page_index}: {content}"
            );
        }

        let parent = field_named(&parsed, &format!("rotated_pick_{page_index}"));
        assert_eq!(
            name_text(parent.get(b"V").expect("/V")),
            "A",
            "page {page_index} parent value"
        );
        let kids = parent
            .get(b"Kids")
            .expect("/Kids")
            .as_array()
            .expect("kids");
        let kid = lopdf_dict(&parsed, &kids[0]);
        assert_eq!(name_text(kid.get(b"AS").expect("/AS")), "A");
        let kid_rect = numbers(kid.get(b"Rect").expect("/Rect"));
        assert_eq!(kid_rect.len(), 4);
        let [x0, y0, x1, y1] = [kid_rect[0], kid_rect[1], kid_rect[2], kid_rect[3]];
        let expected_matrix = match rotations[page_index] {
            90 => format!("0 1 -1 0 {x1} {y0} cm"),
            180 => format!("-1 0 0 -1 {x1} {y1} cm"),
            _ => format!("0 -1 1 0 {x0} {y1} cm"),
        };
        let ap = lopdf_dict(&parsed, kid.get(b"AP").expect("/AP"));
        let normal = lopdf_dict(&parsed, ap.get(b"N").expect("/AP /N"));
        let selected = parsed
            .get_object(
                normal
                    .get(b"A")
                    .expect("selected appearance")
                    .as_reference()
                    .expect("reference"),
            )
            .expect("stream")
            .as_stream()
            .expect("stream");
        assert_rect_close(
            &numbers(selected.dict.get(b"BBox").expect("/BBox")),
            [x0, y0, x1, y1],
        );
        let content =
            String::from_utf8_lossy(&selected.get_plain_content().expect("plain appearance"))
                .into_owned();
        assert!(
            content.contains(&expected_matrix),
            "radio appearance on page {page_index}: {content}"
        );
    }

    // PDFium maps every widget back to the display rectangle it was drawn in.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    for page_index in 0..3u32 {
        let checkbox = fields
            .iter()
            .find(|field| field.name == format!("rotated_agree_{page_index}"))
            .expect("checkbox rediscovered");
        assert!(checkbox.checked);
        assert!((checkbox.rect.x - 40.0).abs() < 0.01);
        assert!((checkbox.rect.y - 200.0).abs() < 0.01);
        assert!((checkbox.rect.width - 180.0).abs() < 0.01);
        assert!((checkbox.rect.height - 30.0).abs() < 0.01);
        let radio = fields
            .iter()
            .find(|field| field.name == format!("rotated_pick_{page_index}"))
            .expect("radio rediscovered");
        assert!(radio.checked);
        assert_eq!(radio.value.as_deref(), Some("A"));
        assert!((radio.rect.x - 40.0).abs() < 0.01);
        assert!((radio.rect.y - 250.0).abs() < 0.01);
        assert!((radio.rect.width - 180.0).abs() < 0.01);
        assert!((radio.rect.height - 30.0).abs() < 0.01);
    }

    for page_index in 0..3u32 {
        let pixels = render_pixels(&reopened, page_index, 2.0);
        let (checkbox_ink, _) = ink_in_region(&pixels, 2.0, (42.0, 202.0, 218.0, 228.0));
        assert!(
            checkbox_ink > 10,
            "page {page_index} paints the checkbox mark: {checkbox_ink}"
        );
        let (radio_ink, _) = ink_in_region(&pixels, 2.0, (42.0, 252.0, 218.0, 278.0));
        assert!(
            radio_ink > 10,
            "page {page_index} paints the selected radio: {radio_ink}"
        );
    }
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn choice_and_signature_appearances_compensate_page_rotation() {
    let pdf = synthetic::rotated_crop_pdf();
    let dir = support::temp::TempDir::new("create-choice-signature-rotated");
    let output = dir.path().join("output.pdf");
    let document_id = open_synthetic(&pdf);

    for page_index in 0..3u32 {
        create_form_field(
            document_id.clone(),
            page_index,
            CreateFormFieldRequest::ComboBox {
                rect: FormFieldRect {
                    x: 40.0,
                    y: 200.0,
                    width: 180.0,
                    height: 30.0,
                },
                name: Some(format!("rotated_country_{page_index}")),
                tooltip: None,
                required: false,
                options: vec!["中国".to_owned(), "Japan".to_owned()],
                selected_index: Some(0),
                font_size: Some(11.0),
            },
        )
        .expect("create a dropdown on a rotated page");
        create_form_field(
            document_id.clone(),
            page_index,
            CreateFormFieldRequest::Signature {
                rect: FormFieldRect {
                    x: 40.0,
                    y: 250.0,
                    width: 180.0,
                    height: 30.0,
                },
                name: Some(format!("rotated_sign_{page_index}")),
                tooltip: None,
            },
        )
        .expect("create a signature on a rotated page");
    }
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let rotations = [90u16, 180, 270];
    let prefixes = ["rotated_country_", "rotated_sign_"];
    for page_index in 0..3usize {
        for prefix in prefixes {
            let field = field_named(&parsed, &format!("{prefix}{page_index}"));
            let rect = numbers(field.get(b"Rect").expect("/Rect"));
            assert_eq!(rect.len(), 4);
            let [x0, y0, x1, y1] = [rect[0], rect[1], rect[2], rect[3]];
            let expected_matrix = match rotations[page_index] {
                90 => format!("0 1 -1 0 {x1} {y0} cm"),
                180 => format!("-1 0 0 -1 {x1} {y1} cm"),
                _ => format!("0 -1 1 0 {x0} {y1} cm"),
            };
            let ap = lopdf_dict(&parsed, field.get(b"AP").expect("/AP"));
            let normal = ap.get(b"N").expect("/AP /N");
            let stream = parsed
                .get_object(normal.as_reference().expect("appearance reference"))
                .expect("appearance stream")
                .as_stream()
                .expect("stream");
            assert_rect_close(
                &numbers(stream.dict.get(b"BBox").expect("/BBox")),
                [x0, y0, x1, y1],
            );
            let content =
                String::from_utf8_lossy(&stream.get_plain_content().unwrap()).into_owned();
            assert!(
                content.contains(&expected_matrix),
                "{prefix}{page_index} on page {page_index}: {content}"
            );
        }
    }

    // PDFium maps both kinds back to the display rectangle they were drawn in.
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    for page_index in 0..3u32 {
        let combo = fields
            .iter()
            .find(|field| field.name == format!("rotated_country_{page_index}"))
            .expect("dropdown rediscovered");
        assert_eq!(combo.kind, FormFieldKind::ComboBox);
        assert_eq!(combo.value.as_deref(), Some("中国"));
        assert!((combo.rect.x - 40.0).abs() < 0.01);
        assert!((combo.rect.y - 200.0).abs() < 0.01);
        assert!((combo.rect.width - 180.0).abs() < 0.01);
        assert!((combo.rect.height - 30.0).abs() < 0.01);
        let signature = fields
            .iter()
            .find(|field| field.name == format!("rotated_sign_{page_index}"))
            .expect("signature rediscovered");
        assert_eq!(signature.kind, FormFieldKind::Signature);
        assert!((signature.rect.x - 40.0).abs() < 0.01);
        assert!((signature.rect.y - 250.0).abs() < 0.01);
        assert!((signature.rect.width - 180.0).abs() < 0.01);
        assert!((signature.rect.height - 30.0).abs() < 0.01);
    }
    for page_index in 0..3u32 {
        let pixels = render_pixels(&reopened, page_index, 2.0);
        let (combo_ink, _) = ink_in_region(&pixels, 2.0, (42.0, 202.0, 218.0, 228.0));
        assert!(
            combo_ink > 10,
            "page {page_index} paints the dropdown: {combo_ink}"
        );
        let (signature_ink, _) = ink_in_region(&pixels, 2.0, (42.0, 252.0, 218.0, 278.0));
        assert!(
            signature_ink > 10,
            "page {page_index} paints the signature area: {signature_ink}"
        );
    }
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn merges_created_fields_into_an_existing_acroform() {
    let dir = support::temp::TempDir::new("create-text-merge");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-acroform"), &source).expect("copy fixture");
    let output = dir.path().join("output.pdf");
    let source_fields = {
        let parsed = lopdf::Document::load(&source).expect("parse fixture");
        let catalog = lopdf_catalog(&parsed);
        let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
        acro_form
            .get(b"Fields")
            .expect("/Fields")
            .as_array()
            .expect("field array")
            .len()
    };

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the form fixture")
        .document_id;
    let before = form_fields(document_id.clone()).expect("existing fields");
    assert!(before.len() >= 10, "the fixture is a populated form");
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 520.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("created_note".to_owned());
    request.required = false;
    let created = create_form_text_field(document_id.clone(), 0, request).expect("create field");
    fill_form_field(
        document_id.clone(),
        created.field_id,
        FormFieldValueInput::Text {
            text: "新增字段".to_owned(),
        },
    )
    .expect("fill the new field");
    save_as(&document_id, &output);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen merged output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(fields.len(), before.len() + 1);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "full_name")
            .and_then(|field| field.value.as_deref()),
        Some("Ada Lovelace"),
        "existing values survive the merge"
    );
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "created_note")
            .and_then(|field| field.value.as_deref()),
        Some("新增字段")
    );
    let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
    for name in [
        "email",
        "subscribe",
        "gender",
        "country",
        "department",
        "notes",
    ] {
        assert!(names.contains(&name), "existing field {name} survives");
    }
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&output).expect("parse merged output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    assert!(fonts.get(b"PdfxNoto").is_ok(), "the new font is merged");
    assert!(
        fonts.iter().count() > 1,
        "existing /DR fonts are preserved: {:?}",
        fonts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        source_fields + 1,
        "the existing /Fields entries are preserved and one is added"
    );
    assert!(field_named(&parsed, "full_name").get(b"V").is_ok());

    close_document(document_id).expect("close document");
}

#[test]
fn merges_choice_and_signature_into_an_existing_acroform_and_preserves_it() {
    let dir = support::temp::TempDir::new("create-choice-signature-merge");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-acroform"), &source).expect("copy fixture");
    let output = dir.path().join("output.pdf");
    let (source_fields, source_da, source_sigflags, source_need_appearances) = {
        let parsed = lopdf::Document::load(&source).expect("parse fixture");
        let catalog = lopdf_catalog(&parsed);
        let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
        (
            acro_form
                .get(b"Fields")
                .expect("/Fields")
                .as_array()
                .expect("field array")
                .len(),
            acro_form.get(b"DA").ok().map(decode_pdf_text),
            acro_form
                .get(b"SigFlags")
                .ok()
                .and_then(|o| o.as_i64().ok()),
            acro_form.has(b"NeedAppearances"),
        )
    };

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the form fixture")
        .document_id;
    let before = form_fields(document_id.clone()).expect("existing fields");
    let existing_names: Vec<String> = before.iter().map(|field| field.name.clone()).collect();

    let duplicate = create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 520.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("country".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
            font_size: None,
        },
    )
    .expect_err("an existing field name is reserved");
    assert_eq!(duplicate.code, CommandErrorCode::FormFieldNameDuplicate);

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 520.0,
                width: 180.0,
                height: 24.0,
            },
            name: Some("merged_country".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["中国".to_owned(), "Japan".to_owned()],
            selected_index: Some(0),
            font_size: Some(13.0),
        },
    )
    .expect("create a dropdown beside the existing form");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                x: 72.0,
                y: 560.0,
                width: 240.0,
                height: 60.0,
            },
            name: Some("merged_sign".to_owned()),
            tooltip: Some("签名".to_owned()),
        },
    )
    .expect("create a signature beside the existing form");
    save_as(&document_id, &output);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen merged output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(fields.len(), before.len() + 2);
    for name in &existing_names {
        assert!(
            fields.iter().any(|field| &field.name == name),
            "existing field {name} survives"
        );
    }
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "full_name")
            .and_then(|field| field.value.as_deref()),
        Some("Ada Lovelace"),
        "existing values survive the merge"
    );
    let merged_country = fields
        .iter()
        .find(|field| field.name == "merged_country")
        .expect("the new dropdown is discovered");
    assert_eq!(merged_country.kind, FormFieldKind::ComboBox);
    assert_eq!(merged_country.options, ["中国", "Japan"]);
    assert_eq!(merged_country.value.as_deref(), Some("中国"));
    let merged_sign = fields
        .iter()
        .find(|field| field.name == "merged_sign")
        .expect("the new signature is discovered");
    assert_eq!(merged_sign.kind, FormFieldKind::Signature);
    assert_eq!(merged_sign.value, None);
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&output).expect("parse merged output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form.has(b"NeedAppearances"),
        source_need_appearances,
        "an existing /NeedAppearances setting is preserved untouched"
    );
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        source_fields + 2,
        "the existing /Fields entries are preserved and two are added"
    );
    if let Some(source_da) = source_da {
        assert_eq!(
            decode_pdf_text(acro_form.get(b"DA").expect("/DA")),
            source_da,
            "the existing default appearance is preserved"
        );
    }
    let new_sigflags = acro_form
        .get(b"SigFlags")
        .expect("/SigFlags")
        .as_i64()
        .expect("signature flags");
    assert_eq!(new_sigflags & 1, 1, "SignaturesExist");
    if let Some(source_sigflags) = source_sigflags {
        assert_eq!(
            new_sigflags & !1,
            source_sigflags & !1,
            "existing signature flags are preserved"
        );
    }
    assert_eq!(
        field_named(&parsed, "merged_country")
            .get(b"FT")
            .unwrap()
            .as_name()
            .unwrap(),
        b"Ch"
    );
    assert_eq!(
        field_named(&parsed, "merged_sign")
            .get(b"FT")
            .unwrap()
            .as_name()
            .unwrap(),
        b"Sig"
    );
    assert!(field_named(&parsed, "full_name").get(b"V").is_ok());

    close_document(document_id).expect("close document");
}

#[test]
fn merges_checkbox_and_radio_into_an_existing_acroform_and_preserves_it() {
    let dir = support::temp::TempDir::new("create-check-radio-merge");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form-acroform"), &source).expect("copy fixture");
    let source_bytes = fs::read(&source).expect("read source");
    let source_modified = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");
    let output = dir.path().join("output.pdf");

    let source_parsed = lopdf::Document::load(&source).expect("parse fixture");
    let catalog = lopdf_catalog(&source_parsed);
    let source_acro_form = lopdf_dict(&source_parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    let source_field_count = source_acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .len();
    let annots_before = page_annots(&source_parsed, 1);

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the form fixture")
        .document_id;
    let before = form_fields(document_id.clone()).expect("existing fields");
    assert!(before.len() >= 10, "the fixture is a populated form");

    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 500.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("new_agree".to_owned()),
            tooltip: None,
            required: false,
            checked: true,
        },
    )
    .expect("create a checkbox in the existing form");
    create_form_field(
        document_id.clone(),
        0,
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 300.0,
                y: 500.0,
                width: 16.0,
                height: 16.0,
            },
            name: Some("new_contact".to_owned()),
            tooltip: None,
            required: false,
            options: vec!["Email".to_owned(), "Phone".to_owned()],
            selected_index: None,
        },
    )
    .expect("create a radio group in the existing form");
    save_as(&document_id, &output);

    assert_eq!(
        fs::read(&source).expect("source after save"),
        source_bytes,
        "the Source Document is never modified"
    );
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata after save")
            .modified()
            .expect("source mtime after save"),
        source_modified,
        "saving never touches the Source Document's mtime"
    );
    let output_bytes = fs::read(&output).expect("read output");
    assert!(!output_bytes.windows(4).any(|window| window == b"new:"));

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen merged output")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("reopened fields");
    assert_eq!(fields.len(), before.len() + 3);
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "full_name")
            .and_then(|field| field.value.as_deref()),
        Some("Ada Lovelace"),
        "existing values survive the merge"
    );
    assert_eq!(
        fields
            .iter()
            .find(|field| field.name == "country")
            .and_then(|field| field.value.as_deref()),
        Some("Japan")
    );
    assert!(
        fields
            .iter()
            .find(|field| field.name == "new_agree")
            .expect("the created checkbox is discovered")
            .checked
    );
    let group: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "new_contact")
        .collect();
    assert_eq!(group.len(), 2);
    assert!(group
        .iter()
        .all(|field| !field.checked && field.value.is_none()));
    close_document(reopened).expect("close reopened");

    let parsed = lopdf::Document::load(&output).expect("parse merged output");
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    assert_eq!(
        acro_form
            .get(b"NeedAppearances")
            .expect("/NeedAppearances is preserved")
            .as_bool()
            .expect("boolean"),
        true,
        "the source's NeedAppearances value is left exactly as it was"
    );
    assert_eq!(
        decode_pdf_text(acro_form.get(b"DA").expect("/DA")),
        "/Helv 0 Tf 0 g",
        "the source default appearance is preserved"
    );
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    assert!(
        fonts.get(b"Helv").is_ok(),
        "the source font stays available"
    );
    assert!(
        fonts.get(b"PdfxNoto").is_err(),
        "no font is embedded when only checkbox and radio fields are created"
    );
    assert_eq!(
        acro_form.get(b"Fields").unwrap().as_array().unwrap().len(),
        source_field_count + 2,
        "one new /Fields entry for the checkbox and one for the radio parent"
    );
    assert_eq!(
        decode_pdf_text(field_named(&parsed, "full_name").get(b"V").unwrap()),
        "Ada Lovelace"
    );
    assert_eq!(
        decode_pdf_text(field_named(&parsed, "country").get(b"V").unwrap()),
        "Japan"
    );
    let checkbox = field_named(&parsed, "new_agree");
    assert_eq!(name_text(checkbox.get(b"FT").unwrap()), "Btn");
    assert_eq!(name_text(checkbox.get(b"V").unwrap()), "Yes");
    assert_eq!(name_text(checkbox.get(b"AS").unwrap()), "Yes");
    let parent = field_named(&parsed, "new_contact");
    assert_eq!(name_text(parent.get(b"V").unwrap()), "Off");
    let kids = parent.get(b"Kids").unwrap().as_array().unwrap();
    assert_eq!(kids.len(), 2);
    for kid in kids {
        let kid = lopdf_dict(&parsed, kid);
        assert_eq!(name_text(kid.get(b"AS").unwrap()), "Off");
    }

    let annots_after = page_annots(&parsed, 1);
    assert_eq!(
        annots_after.len(),
        annots_before.len() + 3,
        "existing annotations are kept and the new widgets are appended"
    );
    assert_eq!(
        &annots_after[..annots_before.len()],
        &annots_before[..],
        "the existing annotation order is preserved"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn permission_restricted_documents_refuse_creation_with_a_chinese_reason() {
    use pdfx_lib::text_edit::edit_history;

    let path = fixture_path("encrypted-owner-restrictions");
    let opened = open_document(path.to_string_lossy().into_owned(), None)
        .expect("open the restricted document");
    let error = create_form_text_field(
        opened.document_id.clone(),
        0,
        create_request(FormFieldRect {
            x: 72.0,
            y: 100.0,
            width: 180.0,
            height: 24.0,
        }),
    )
    .expect_err("permission restrictions reject field creation");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    assert!(
        error.message.contains("权限"),
        "the refusal explains the permission restriction in Chinese: {}",
        error.message
    );
    for request in [
        CreateFormFieldRequest::Checkbox {
            rect: FormFieldRect {
                x: 72.0,
                y: 100.0,
                width: 16.0,
                height: 16.0,
            },
            name: None,
            tooltip: None,
            required: false,
            checked: false,
        },
        CreateFormFieldRequest::Radio {
            rect: FormFieldRect {
                x: 72.0,
                y: 140.0,
                width: 16.0,
                height: 16.0,
            },
            name: None,
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
        },
        CreateFormFieldRequest::ComboBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 180.0,
                width: 180.0,
                height: 24.0,
            },
            name: None,
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_index: None,
            font_size: None,
        },
        CreateFormFieldRequest::ListBox {
            rect: FormFieldRect {
                x: 72.0,
                y: 220.0,
                width: 180.0,
                height: 48.0,
            },
            name: None,
            tooltip: None,
            required: false,
            options: vec!["A".to_owned()],
            selected_indices: Vec::new(),
            multi_select: false,
            font_size: None,
        },
        CreateFormFieldRequest::Signature {
            rect: FormFieldRect {
                x: 72.0,
                y: 280.0,
                width: 240.0,
                height: 60.0,
            },
            name: None,
            tooltip: None,
        },
    ] {
        let error = create_form_field(opened.document_id.clone(), 0, request)
            .expect_err("permission restrictions reject every field kind");
        assert_eq!(error.code, CommandErrorCode::PermissionDenied);
        assert!(
            error.message.contains("权限"),
            "Chinese permission refusal: {}",
            error.message
        );
    }
    assert!(form_fields(opened.document_id.clone())
        .expect("fields")
        .is_empty());
    let history = edit_history(opened.document_id.clone()).expect("history");
    assert_eq!(history.revision, 0);
    assert!(!history.dirty && !history.can_undo);
    close_document(opened.document_id).expect("close restricted document");
}

#[test]
fn scanned_pages_accept_created_fields() {
    use pdfx_lib::commands::page_info;

    let dir = support::temp::TempDir::new("create-text-scanned");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("scanned-image-only"), &source).expect("copy scanned fixture");
    let output = dir.path().join("output.pdf");

    let document_id = open_document(source.to_string_lossy().into_owned(), None)
        .expect("open the scanned document")
        .document_id;
    let info = page_info(document_id.clone(), 0).expect("page info");
    let rect = FormFieldRect {
        x: 72.0,
        y: 72.0,
        width: 180.0,
        height: 24.0,
    };
    assert!(
        rect.x + rect.width <= info.width && rect.y + rect.height <= info.height,
        "the test rectangle fits the scanned page"
    );
    let mut request = create_request(rect);
    request.name = Some("scan_note".to_owned());
    request.required = false;
    let created = create_form_text_field(document_id.clone(), 0, request)
        .expect("a page without a text layer still accepts a form field");
    fill_form_field(
        document_id.clone(),
        created.field_id,
        FormFieldValueInput::Text {
            text: "扫描页字段".to_owned(),
        },
    )
    .expect("fill the scanned-page field");
    save_as(&document_id, &output);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "scan_note");
    assert_eq!(fields[0].value.as_deref(), Some("扫描页字段"));
    assert_eq!(fields[0].kind, FormFieldKind::Text);
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}

#[test]
fn saves_an_empty_created_field_with_a_static_appearance() {
    let pdf = synthetic::single_line_pdf();
    let dir = support::temp::TempDir::new("create-text-empty");
    let output = dir.path().join("output.pdf");

    let document_id = open_synthetic(&pdf);
    let mut request = create_request(FormFieldRect {
        x: 72.0,
        y: 100.0,
        width: 180.0,
        height: 24.0,
    });
    request.name = Some("empty_note".to_owned());
    request.required = false;
    request.tooltip = None;
    create_form_text_field(document_id.clone(), 0, request).expect("create an empty field");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("parse output");
    let saved = field_named(&parsed, "empty_note");
    assert!(saved.get(b"V").is_err(), "an empty field has no /V");
    let appearance = lopdf_dict(&parsed, saved.get(b"AP").expect("/AP"));
    assert!(
        appearance.get(b"N").is_ok(),
        "the empty field still has /AP /N"
    );
    let catalog = lopdf_catalog(&parsed);
    let acro_form = lopdf_dict(&parsed, catalog.get(b"AcroForm").expect("/AcroForm"));
    let dr = lopdf_dict(&parsed, acro_form.get(b"DR").expect("/DR"));
    let fonts = lopdf_dict(&parsed, dr.get(b"Font").expect("/DR /Font"));
    assert!(
        fonts.has(b"PdfxNoto"),
        "empty values still embed a usable font subset"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen")
        .document_id;
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "empty_note");
    assert_eq!(fields[0].value, None);
    close_document(reopened).expect("close reopened");
    close_document(document_id).expect("close document");
}
