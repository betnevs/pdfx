//! Command-boundary tests for filling the AcroForm fields a PDF already has.
//!
//! The fixture deliberately contains every fillable widget type on two pages.
//! Assertions use only the Rust functions the Tauri IPC layer exposes, plus
//! independent lopdf/PDFium reads of saved output.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use pdfx_lib::commands::{close_document, extract_text, open_document, CommandErrorCode};
use pdfx_lib::form::{
    fill_form_field, form_fields, missing_required_fields, FormFieldKind, FormFieldValueInput,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};

fn fixture_path(category: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, category).expect("fixture category");
    support::fixtures::fixture_path(fixture)
}

fn fixture_id_path(id: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture id {id}"));
    support::fixtures::fixture_path(fixture)
}

fn open_form() -> String {
    open_document(fixture_path("form").to_string_lossy().into_owned(), None)
        .expect("open form fixture")
        .document_id
}

#[test]
fn discovers_every_existing_field_in_page_and_widget_order() {
    let document_id = open_form();

    let fields = form_fields(document_id.clone()).expect("discover form fields");
    let summary: Vec<_> = fields
        .iter()
        .map(|field| (field.name.as_str(), field.kind, field.page_index))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("full_name", FormFieldKind::Text, 0),
            ("email", FormFieldKind::Text, 0),
            ("subscribe", FormFieldKind::Checkbox, 0),
            ("gender", FormFieldKind::Radio, 0),
            ("gender", FormFieldKind::Radio, 0),
            ("gender", FormFieldKind::Radio, 0),
            ("country", FormFieldKind::ListBox, 0),
            ("department", FormFieldKind::ComboBox, 0),
            ("notes", FormFieldKind::Text, 1),
            ("agree", FormFieldKind::Checkbox, 1),
            ("js_guard", FormFieldKind::Text, 1),
            ("readonly_note", FormFieldKind::Text, 1),
        ],
        "fields follow page order and each page's widget order"
    );

    let orders: Vec<_> = fields.iter().map(|field| field.order).collect();
    assert!(
        orders.windows(2).all(|pair| pair[0] < pair[1]),
        "tab order must increase across the document: {orders:?}"
    );
    let ids: std::collections::HashSet<_> = fields.iter().map(|f| f.field_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        fields.len(),
        "field ids identify one widget each"
    );

    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("field {name}"))
    };

    let full_name = field("full_name");
    assert_eq!(full_name.value.as_deref(), Some("Ada Lovelace"));
    // [72 620 320 644] on an unrotated 612x792 page: top-left-origin y is
    // 792 - 644 = 148 and the box is 248x24 display-space points.
    assert!((full_name.rect.x - 72.0).abs() < 0.01);
    assert!((full_name.rect.y - 148.0).abs() < 0.01);
    assert!((full_name.rect.width - 248.0).abs() < 0.01);
    assert!((full_name.rect.height - 24.0).abs() < 0.01);
    assert!(!full_name.required);
    assert!(!full_name.read_only);
    assert!(!full_name.multiline);

    let email = field("email");
    assert_eq!(email.value, None, "an empty field has no value");
    assert!(email.required, "email carries the Required field flag");

    let subscribe = field("subscribe");
    assert!(!subscribe.checked);

    let gender: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert!(gender
        .iter()
        .all(|field| field.kind == FormFieldKind::Radio));
    assert!(gender.iter().all(|field| field.required));
    assert!(gender.iter().all(|field| !field.checked));
    assert_eq!(
        gender[0].radio_group.as_deref(),
        Some("gender"),
        "radio widgets share their field name as the group"
    );

    let country = field("country");
    assert_eq!(country.kind, FormFieldKind::ListBox);
    assert_eq!(country.options, ["China", "Japan", "Germany"]);
    assert_eq!(country.value.as_deref(), Some("Japan"));
    assert_eq!(country.selected_index, Some(1));
    assert_eq!(country.selected_indices, [1]);
    assert!(
        !country.multi_select,
        "the fixture list box is single-select"
    );

    let department = field("department");
    assert_eq!(department.kind, FormFieldKind::ComboBox);
    assert_eq!(department.options, ["Engineering", "Design", "Sales"]);
    assert_eq!(department.value, None);
    assert_eq!(department.selected_index, None);
    assert!(department.selected_indices.is_empty());
    assert!(!department.multi_select);

    assert!(field("notes").multiline);
    assert!(field("agree").required);
    assert!(!field("agree").checked);
    assert!(field("readonly_note").read_only);
    assert_eq!(field("readonly_note").value.as_deref(), Some("Read only"));

    close_document(document_id).expect("close document");
}

#[test]
fn required_validation_reports_missing_fields_once_per_radio_group() {
    let document_id = open_form();

    let missing = missing_required_fields(document_id.clone()).expect("validate required fields");
    let summary: Vec<_> = missing
        .iter()
        .map(|field| (field.name.as_str(), field.page_index, field.kind))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("email", 0, FormFieldKind::Text),
            // Three widgets share one required radio group; the user must
            // answer the group once, not three times.
            ("gender", 0, FormFieldKind::Radio),
            ("agree", 1, FormFieldKind::Checkbox),
        ],
        "missing required fields are reported in tab order without duplicate radio widgets"
    );
    assert!(
        missing.iter().all(|field| field.required),
        "only fields carrying the Required flag are reported"
    );
    assert_eq!(
        missing[0].field_id,
        form_fields(document_id.clone())
            .expect("fields")
            .into_iter()
            .find(|field| field.name == "email")
            .expect("email")
            .field_id,
        "the first missing field is locatable on the canvas"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn fills_a_text_field_updates_required_state_and_marks_the_document_dirty() {
    let document_id = open_form();
    let email = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "email")
        .expect("email field");

    let result = fill_form_field(
        document_id.clone(),
        email.field_id,
        FormFieldValueInput::Text {
            text: "ada@example.com".to_owned(),
        },
    )
    .expect("fill text field");

    let updated = result
        .fields
        .iter()
        .find(|field| field.name == "email")
        .expect("email field after fill");
    assert_eq!(updated.value.as_deref(), Some("ada@example.com"));
    assert!(result.history.dirty, "a field change dirties the document");
    assert!(result.history.can_undo);
    assert_eq!(result.history.revision, result.history.saved_revision + 1);

    let missing = missing_required_fields(document_id.clone()).expect("validate");
    let names: Vec<_> = missing.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(names, ["gender", "agree"], "email is no longer missing");

    close_document(document_id).expect("close document");
}

#[test]
fn toggles_checkboxes_on_and_off_through_history() {
    let document_id = open_form();
    let subscribe = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "subscribe")
        .expect("subscribe field");

    let checked = fill_form_field(
        document_id.clone(),
        subscribe.field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("check the box");
    let updated = checked
        .fields
        .iter()
        .find(|field| field.name == "subscribe")
        .expect("subscribe after check");
    assert!(updated.checked);
    assert!(checked.history.dirty);
    assert_eq!(checked.history.revision, checked.history.saved_revision + 1);

    let unchecked = fill_form_field(
        document_id.clone(),
        subscribe.field_id,
        FormFieldValueInput::Checked { checked: false },
    )
    .expect("uncheck the box");
    let updated = unchecked
        .fields
        .iter()
        .find(|field| field.name == "subscribe")
        .expect("subscribe after uncheck");
    assert!(!updated.checked);
    assert_eq!(
        unchecked.history.revision,
        unchecked.history.saved_revision + 2
    );

    close_document(document_id).expect("close document");
}

#[test]
fn selects_one_radio_widget_per_group_and_reports_the_group_as_answered() {
    let document_id = open_form();
    let gender: Vec<_> = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert_eq!(gender.len(), 3);
    let result = fill_form_field(
        document_id.clone(),
        gender[1].field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select female");
    let group: Vec<_> = result
        .fields
        .iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert_eq!(
        group.iter().map(|field| field.checked).collect::<Vec<_>>(),
        [false, true, false],
        "only the chosen widget is checked"
    );
    assert_eq!(group[1].value.as_deref(), Some("female"));

    let missing = missing_required_fields(document_id.clone()).expect("validate");
    assert!(
        missing.iter().all(|field| field.name != "gender"),
        "answering any widget answers the group"
    );

    let switched = fill_form_field(
        document_id.clone(),
        gender[2].field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select other");
    let group: Vec<_> = switched
        .fields
        .iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert_eq!(
        group.iter().map(|field| field.checked).collect::<Vec<_>>(),
        [false, false, true],
        "switching selection clears the previous widget in the group"
    );
    assert_eq!(group[0].value.as_deref(), Some("other"));

    close_document(document_id).expect("close document");
}

#[test]
fn undo_and_redo_restore_values_through_the_shared_edit_history() {
    use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};

    let document_id = open_form();
    let fields = form_fields(document_id.clone()).expect("fields");
    let email = fields
        .iter()
        .find(|field| field.name == "email")
        .expect("email");
    let female = fields
        .iter()
        .filter(|field| field.name == "gender")
        .nth(1)
        .expect("female widget");

    fill_form_field(
        document_id.clone(),
        email.field_id.clone(),
        FormFieldValueInput::Text {
            text: "ada@example.com".to_owned(),
        },
    )
    .expect("fill email");
    fill_form_field(
        document_id.clone(),
        female.field_id.clone(),
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select female");

    undo_text_edit(document_id.clone()).expect("undo radio");
    let missing = missing_required_fields(document_id.clone()).expect("validate");
    assert!(
        missing.iter().any(|field| field.name == "gender"),
        "undoing the radio selection makes the required group missing again"
    );

    undo_text_edit(document_id.clone()).expect("undo text");
    let email = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "email")
        .expect("email");
    assert_eq!(email.value, None, "undo restores the empty text field");
    let missing = missing_required_fields(document_id.clone()).expect("validate");
    assert!(missing.iter().any(|field| field.name == "email"));

    redo_text_edit(document_id.clone()).expect("redo text");
    redo_text_edit(document_id.clone()).expect("redo radio");
    let fields = form_fields(document_id.clone()).expect("fields");
    let email = fields
        .iter()
        .find(|field| field.name == "email")
        .expect("email");
    assert_eq!(email.value.as_deref(), Some("ada@example.com"));
    let gender: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert_eq!(
        gender.iter().map(|field| field.checked).collect::<Vec<_>>(),
        [false, true, false],
        "redo restores the radio selection from the same history"
    );
    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert!(history.dirty);
    assert_eq!(history.revision, history.saved_revision + 2);

    close_document(document_id).expect("close document");
}

#[test]
fn selects_list_and_dropdown_values_from_their_options() {
    let document_id = open_form();
    let fields = form_fields(document_id.clone()).expect("fields");
    let country = fields
        .iter()
        .find(|field| field.name == "country")
        .expect("country");
    let department = fields
        .iter()
        .find(|field| field.name == "department")
        .expect("department");

    let result = fill_form_field(
        document_id.clone(),
        country.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Germany".to_owned(),
        },
    )
    .expect("select Germany");
    let updated = result
        .fields
        .iter()
        .find(|field| field.name == "country")
        .expect("country after fill");
    assert_eq!(updated.value.as_deref(), Some("Germany"));
    assert_eq!(updated.selected_index, Some(2));
    assert!(result.history.dirty);

    let result = fill_form_field(
        document_id.clone(),
        department.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Design".to_owned(),
        },
    )
    .expect("select Design");
    let updated = result
        .fields
        .iter()
        .find(|field| field.name == "department")
        .expect("department after fill");
    assert_eq!(updated.value.as_deref(), Some("Design"));
    assert_eq!(updated.selected_index, Some(1));

    let error = fill_form_field(
        document_id.clone(),
        country.field_id.clone(),
        FormFieldValueInput::Choice {
            value: "Atlantis".to_owned(),
        },
    )
    .expect_err("a value outside /Opt is rejected");
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::FormValueInvalid
    );

    close_document(document_id).expect("close document");
}

fn open_at(path: &Path) -> String {
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open document")
        .document_id
}

fn field_by_name<'a>(
    fields: &'a [pdfx_lib::form::FormFieldView],
    name: &str,
) -> &'a pdfx_lib::form::FormFieldView {
    fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("field {name}"))
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
    match object.as_reference() {
        Ok(id) => document
            .get_object(id)
            .expect("referenced object")
            .as_dict()
            .expect("referenced dictionary"),
        Err(_) => object.as_dict().expect("direct dictionary"),
    }
}

fn lopdf_text(object: &lopdf::Object) -> String {
    String::from_utf8_lossy(object.as_str().expect("string")).into_owned()
}

fn form_fields_of<'a>(
    document: &'a lopdf::Document,
) -> Vec<(&'a lopdf::Dictionary, &'a lopdf::Dictionary)> {
    let catalog = lopdf_catalog(document);
    let acro_form = lopdf_dict(document, catalog.get(b"AcroForm").expect("/AcroForm"));
    acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array")
        .iter()
        .map(|field| {
            let dictionary = lopdf_dict(document, field);
            (dictionary, dictionary)
        })
        .collect()
}

fn field_named<'a>(document: &'a lopdf::Document, name: &str) -> &'a lopdf::Dictionary {
    form_fields_of(document)
        .into_iter()
        .map(|(field, _)| field)
        .find(|field| {
            field
                .get(b"T")
                .ok()
                .and_then(|title| title.as_str().ok())
                .map(|title| title == name.as_bytes())
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("field {name} in the saved document"))
}

#[test]
fn saves_filled_values_that_independent_readers_can_reopen() {
    let dir = support::temp::TempDir::new("form-save");
    let source = dir.path().join("source.pdf");
    fs::copy(fixture_path("form"), &source).expect("copy fixture");
    let output = dir.path().join("output.pdf");
    let source_before = fs::read(&source).expect("read source");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let document_id = open_at(&source);
    let fields = form_fields(document_id.clone()).expect("fields");
    let field_id = |name: &str| field_by_name(&fields, name).field_id.clone();
    let female_id = fields
        .iter()
        .filter(|field| field.name == "gender")
        .nth(1)
        .expect("female widget")
        .field_id
        .clone();

    let fill = |field: &str, value: FormFieldValueInput| {
        fill_form_field(document_id.clone(), field_id(field), value).expect("fill field");
    };
    fill(
        "email",
        FormFieldValueInput::Text {
            text: "ada@example.com".to_owned(),
        },
    );
    fill("subscribe", FormFieldValueInput::Checked { checked: true });
    fill("agree", FormFieldValueInput::Checked { checked: true });
    fill_form_field(
        document_id.clone(),
        female_id,
        FormFieldValueInput::Checked { checked: true },
    )
    .expect("select female");
    fill(
        "country",
        FormFieldValueInput::Choice {
            value: "Germany".to_owned(),
        },
    );
    fill(
        "department",
        FormFieldValueInput::Choice {
            value: "Design".to_owned(),
        },
    );
    fill(
        "notes",
        FormFieldValueInput::Text {
            text: "Multi-line notes".to_owned(),
        },
    );
    fill(
        "js_guard",
        FormFieldValueInput::Text {
            text: "typed by PDFX".to_owned(),
        },
    );

    let js_guard = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "js_guard")
        .expect("js_guard");
    assert_eq!(
        js_guard.value.as_deref(),
        Some("typed by PDFX"),
        "the /AA /K JavaScript action never runs, so the typed value survives verbatim"
    );

    assert!(
        missing_required_fields(document_id.clone())
            .expect("validate")
            .is_empty(),
        "every required field is answered before saving"
    );

    let mut progress = Vec::new();
    save_document_with_progress(
        &document_id,
        SaveDocumentRequest {
            save_as_path: Some(output.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
            password: None,
        },
        &mut |update| progress.push(update),
    )
    .expect("save the filled form");

    assert_eq!(
        fs::read(&source).expect("read source"),
        source_before,
        "the Source Document is never modified"
    );
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_mtime,
        "saving does not touch the source file"
    );

    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source_before),
        "form filling appends revisions to the unmodified source bytes"
    );

    // Reopen through the command boundary: PDFium must recognise the values.
    let reopened = open_at(&output);
    let fields = form_fields(reopened.clone()).expect("fields after reopen");
    let email = field_by_name(&fields, "email");
    assert_eq!(email.value.as_deref(), Some("ada@example.com"));
    assert!(field_by_name(&fields, "subscribe").checked);
    assert!(field_by_name(&fields, "agree").checked);
    let gender: Vec<_> = fields
        .iter()
        .filter(|field| field.name == "gender")
        .collect();
    assert_eq!(
        gender.iter().map(|field| field.checked).collect::<Vec<_>>(),
        [false, true, false],
        "the saved group value selects the female widget"
    );
    let country = field_by_name(&fields, "country");
    assert_eq!(country.value.as_deref(), Some("Germany"));
    assert_eq!(country.selected_index, Some(2));
    let department = field_by_name(&fields, "department");
    assert_eq!(department.value.as_deref(), Some("Design"));
    assert_eq!(department.selected_index, Some(1));
    assert_eq!(
        field_by_name(&fields, "notes").value.as_deref(),
        Some("Multi-line notes")
    );
    assert_eq!(
        field_by_name(&fields, "js_guard").value.as_deref(),
        Some("typed by PDFX")
    );
    assert!(missing_required_fields(reopened.clone())
        .expect("validate reopened")
        .is_empty());
    let text = extract_text(reopened.clone(), 0).expect("text").text;
    assert!(text.contains("PDFX AcroForm fixture"), "got {text:?}");
    let _ = extract_text(reopened.clone(), 1).expect("page 2 text");
    close_document(reopened).expect("close reopened");

    // Independently parse the saved structure with lopdf.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let email = field_named(&parsed, "email");
    assert_eq!(
        lopdf_text(email.get(b"V").expect("email /V")),
        "ada@example.com"
    );

    let gender = field_named(&parsed, "gender");
    assert_eq!(
        gender.get(b"V").expect("/V").as_name().expect("name"),
        b"female",
        "the radio group's /V is a name on the parent field"
    );
    let kids = gender
        .get(b"Kids")
        .expect("/Kids")
        .as_array()
        .expect("kid array");
    let appearance_states: Vec<String> = kids
        .iter()
        .map(|kid| {
            let widget = lopdf_dict(&parsed, kid);
            widget
                .get(b"AS")
                .map(|state| String::from_utf8_lossy(state.as_name().expect("name")).into_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        appearance_states,
        ["Off", "female", "Off"],
        "every widget's appearance state matches the saved group value"
    );

    let country = field_named(&parsed, "country");
    assert_eq!(lopdf_text(country.get(b"V").expect("/V")), "Germany");
    let indexes: Vec<i64> = country
        .get(b"I")
        .expect("/I")
        .as_array()
        .expect("index array")
        .iter()
        .map(|index| index.as_i64().expect("integer"))
        .collect();
    assert_eq!(indexes, [2]);

    let department = field_named(&parsed, "department");
    assert_eq!(lopdf_text(department.get(b"V").expect("/V")), "Design");

    let agree = field_named(&parsed, "agree");
    let agree_state = agree.get(b"AS").expect("/AS");
    let agree_state = match agree_state {
        lopdf::Object::Name(name) => name.clone(),
        object => object.as_str().expect("appearance state").to_vec(),
    };
    // `/AS` is a name equal to the `/V` name, not a string with a slash.
    assert_eq!(agree_state, b"Yes");

    let js_guard = field_named(&parsed, "js_guard");
    let action = lopdf_dict(&parsed, js_guard.get(b"AA").expect("/AA"))
        .get(b"K")
        .expect("/K");
    let action = lopdf_dict(&parsed, action);
    let script = lopdf_text(action.get(b"JS").expect("/JS"));
    assert!(
        script.contains("event.value = 'JS_RAN';"),
        "the JavaScript action is preserved untouched, never executed: {script:?}"
    );

    close_document(document_id).expect("close source");
}

#[test]
fn read_only_fields_and_restricted_documents_refuse_filling() {
    let document_id = open_form();
    let readonly_note = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "readonly_note")
        .expect("readonly_note");

    let error = fill_form_field(
        document_id.clone(),
        readonly_note.field_id,
        FormFieldValueInput::Text {
            text: "changed".to_owned(),
        },
    )
    .expect_err("the ReadOnly field flag rejects edits");
    assert_eq!(error.code, CommandErrorCode::FormFieldReadOnly);

    let missing = missing_required_fields(document_id.clone()).expect("validate");
    assert!(
        missing.iter().all(|field| field.name != "readonly_note"),
        "a read-only field is never reported as a missing required answer"
    );
    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert!(
        !history.dirty && !history.can_undo,
        "a rejected fill changes nothing"
    );

    close_document(document_id).expect("close document");

    // Permission Restrictions deny form filling for this encrypted fixture.
    let restricted = open_document(
        fixture_id_path("encrypted-owner-restrictions")
            .to_string_lossy()
            .into_owned(),
        None,
    )
    .expect("open restricted document");
    assert!(!restricted.permissions.can_fill_forms);
    let error = fill_form_field(
        restricted.document_id.clone(),
        "0:0".to_owned(),
        FormFieldValueInput::Text {
            text: "changed".to_owned(),
        },
    )
    .expect_err("permission restrictions reject form filling");
    assert_eq!(error.code, CommandErrorCode::FormFillNotPermitted);
    close_document(restricted.document_id).expect("close restricted document");
}

#[test]
fn a_java_script_keystroke_action_is_never_executed() {
    let document_id = open_form();
    let js_guard = form_fields(document_id.clone())
        .expect("fields")
        .into_iter()
        .find(|field| field.name == "js_guard")
        .expect("js_guard");

    for text in ["JS_RAN", "typed value"] {
        let result = fill_form_field(
            document_id.clone(),
            js_guard.field_id.clone(),
            FormFieldValueInput::Text {
                text: text.to_owned(),
            },
        )
        .expect("fill js_guard");
        let updated = result
            .fields
            .iter()
            .find(|field| field.name == "js_guard")
            .expect("js_guard after fill");
        assert_eq!(
            updated.value.as_deref(),
            Some(text),
            "the /AA /K action must not overwrite the typed value"
        );
    }

    // Clearing the field writes an empty value; the keystroke action still
    // does not run and inject a value.
    let result = fill_form_field(
        document_id.clone(),
        js_guard.field_id.clone(),
        FormFieldValueInput::Text {
            text: String::new(),
        },
    )
    .expect("clear js_guard");
    let updated = result
        .fields
        .iter()
        .find(|field| field.name == "js_guard")
        .expect("js_guard after clear");
    assert_eq!(
        updated.value.as_deref().unwrap_or(""),
        "",
        "clearing leaves the field empty"
    );

    close_document(document_id).expect("close document");
}
