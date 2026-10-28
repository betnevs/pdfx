mod support;

use std::fs;
use std::path::PathBuf;

use pdfium_render::prelude::*;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn all_fields_fixture_exposes_every_supported_field_type() {
    let bytes = fixture("all-fields.pdf");
    let document = support::pdfium::open(&bytes);
    let form = document.form().expect("AcroForm");
    assert_eq!(form.form_type(), PdfFormType::Acrobat);
    let page = document.pages().get(0).expect("page 0");
    let mut counts = std::collections::BTreeMap::new();
    let mut names = Vec::new();
    for annotation in page.annotations().iter() {
        let Some(field) = annotation.as_form_field() else {
            continue;
        };
        names.push(field.name().expect("field name"));
        let kind = match field.field_type() {
            PdfFormFieldType::Text => "text",
            PdfFormFieldType::Checkbox => "checkbox",
            PdfFormFieldType::RadioButton => "radio",
            PdfFormFieldType::ListBox => "list",
            PdfFormFieldType::ComboBox => "combo",
            PdfFormFieldType::Signature => "signature",
            other => panic!("unexpected field type {other:?}"),
        };
        *counts.entry(kind).or_insert(0usize) += 1;
    }
    assert_eq!(counts.get("text"), Some(&2));
    assert_eq!(counts.get("checkbox"), Some(&2));
    assert_eq!(counts.get("radio"), Some(&3));
    assert_eq!(counts.get("list"), Some(&1));
    assert_eq!(counts.get("combo"), Some(&1));
    assert_eq!(counts.get("signature"), Some(&1));
    for expected in [
        "full_name",
        "subscribe",
        "billing",
        "plan",
        "country",
        "signature",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "fixture has field {expected}; names: {names:?}"
        );
    }

    let static_document = support::pdfium::open(&bytes);
    let rendered = support::pdfium::render(&static_document, 0);
    for rect in [
        [72.0, 700.0, 320.0, 724.0],
        [72.0, 600.0, 88.0, 616.0],
        [72.0, 470.0, 220.0, 530.0],
        [260.0, 508.0, 420.0, 530.0],
        [72.0, 350.0, 320.0, 374.0],
    ] {
        assert!(
            rendered.dark_pixels_in(rect) > 0,
            "field appearance paints inside {rect:?}"
        );
    }
}

#[test]
fn merged_fixture_preserves_the_existing_fields_and_values() {
    let bytes = fixture("merged-form.pdf");
    let document = support::pdfium::open(&bytes);
    let page = document.pages().get(0).expect("page 0");
    let mut text_values = std::collections::BTreeMap::new();
    let mut checkbox_values = std::collections::BTreeMap::new();
    let mut combo_values = std::collections::BTreeMap::new();
    for annotation in page.annotations().iter() {
        let Some(field) = annotation.as_form_field() else {
            continue;
        };
        let Some(name) = field.name() else { continue };
        match field.field_type() {
            PdfFormFieldType::Text => {
                text_values.insert(
                    name,
                    field.as_text_field().unwrap().value().unwrap_or_default(),
                );
            }
            PdfFormFieldType::Checkbox => {
                checkbox_values.insert(
                    name,
                    field
                        .as_checkbox_field()
                        .unwrap()
                        .is_checked()
                        .expect("checkbox state"),
                );
            }
            PdfFormFieldType::ComboBox => {
                combo_values.insert(
                    name,
                    field
                        .as_combo_box_field()
                        .unwrap()
                        .value()
                        .unwrap_or_default(),
                );
            }
            _ => {}
        }
    }
    assert_eq!(
        text_values.get("full_name").map(String::as_str),
        Some("Ada Lovelace"),
        "the pre-existing text field keeps its value"
    );
    assert_eq!(checkbox_values.get("subscribe"), Some(&false));
    assert_eq!(
        text_values.get("email").map(String::as_str),
        Some("ada@example.com")
    );
    assert_eq!(checkbox_values.get("newsletter"), Some(&true));
    assert_eq!(combo_values.get("plan").map(String::as_str), Some("Pro"));
}

#[test]
fn flattened_fixture_has_no_form_and_renders_like_the_interactive_form() {
    let interactive = fixture("all-fields.pdf");
    let flattened = fixture("all-fields-flattened.pdf");
    let interactive_document = support::pdfium::open(&interactive);
    let flattened_document = support::pdfium::open(&flattened);
    assert!(interactive_document.form().is_some());
    assert!(
        flattened_document.form().is_none(),
        "flattened copy has no AcroForm"
    );
    let page = flattened_document.pages().get(0).unwrap();
    assert_eq!(page.annotations().len(), 0);
    let static_document = support::pdfium::open(&interactive);
    let before = support::pdfium::render(&static_document, 0);
    let after = support::pdfium::render(&flattened_document, 0);
    assert!(
        before.pixels_differing(&after).is_empty(),
        "flattening preserves the rendered pixels"
    );
}
