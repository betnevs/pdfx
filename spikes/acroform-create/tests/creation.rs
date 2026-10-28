mod support;

use lopdf::{Document, Object};
use pdfium_render::prelude::*;

use pdfx_acroform_create_spike::{
    add_fields, blank_page, create_form, FieldKind, FieldSpec, PageSetup, RadioOption,
};

fn name<'a>(dict: &'a lopdf::Dictionary, key: &[u8]) -> &'a [u8] {
    dict.get(key)
        .and_then(Object::as_name)
        .unwrap_or_else(|error| panic!("{key:?}: {error}"))
}

fn text<'a>(dict: &'a lopdf::Dictionary, key: &[u8]) -> &'a str {
    std::str::from_utf8(dict.get(key).and_then(Object::as_str).unwrap()).unwrap()
}

fn integer(dict: &lopdf::Dictionary, key: &[u8]) -> i64 {
    dict.get(key).and_then(Object::as_i64).unwrap()
}

fn page_setup() -> PageSetup {
    PageSetup {
        width: 612.0,
        height: 792.0,
        rotate: 0,
        crop_box: None,
    }
}

fn text_spec(name: &str, rect: [f32; 4], value: &str) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: Some("Full name".to_owned()),
        rect,
        required: true,
        font_size: 12.0,
        kind: FieldKind::Text {
            value: value.to_owned(),
            multiline: false,
        },
    }
}

#[test]
fn creates_a_text_field_with_an_appearance_stream() {
    let form = create_form(
        page_setup(),
        &[text_spec(
            "full_name",
            [72.0, 620.0, 320.0, 644.0],
            "Ada Lovelace",
        )],
    )
    .expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog
        .get(b"AcroForm")
        .and_then(Object::as_dict)
        .expect("catalog /AcroForm dictionary");
    let fields = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .expect("/AcroForm /Fields array");
    assert_eq!(fields.len(), 1, "one top-level field");

    let field = document
        .get_object(fields[0].as_reference().expect("field reference"))
        .and_then(Object::as_dict)
        .expect("field dictionary");
    assert_eq!(name(field, b"FT"), b"Tx");
    assert_eq!(text(field, b"T"), "full_name");
    assert_eq!(text(field, b"V"), "Ada Lovelace");
    assert_eq!(name(field, b"Subtype"), b"Widget");
    assert_eq!(integer(field, b"Ff"), 2, "required flag (bit 2)");
    let appearance = field
        .get(b"AP")
        .and_then(Object::as_dict)
        .and_then(|ap| ap.get(b"N"))
        .and_then(Object::as_reference)
        .expect("/AP /N appearance reference");
    let stream = document
        .get_object(appearance)
        .and_then(Object::as_stream)
        .expect("appearance stream");
    let content = stream
        .decompressed_content()
        .expect("appearance stream content");
    assert!(
        String::from_utf8_lossy(&content).contains("Ada Lovelace"),
        "appearance draws the default value"
    );
    assert_eq!(text(acroform, b"DA"), "/Helv 0 Tf 0 g");
    assert!(
        acroform
            .get(b"DR")
            .and_then(Object::as_dict)
            .and_then(|dr| dr.get(b"Font"))
            .and_then(Object::as_dict)
            .map(|fonts| fonts.has(b"Helv"))
            .unwrap_or(false),
        "AcroForm /DR declares /Helv"
    );
    assert!(
        !String::from_utf8_lossy(&form.bytes).contains("NeedAppearances"),
        "fields carry explicit appearances; readers must not regenerate them"
    );
}

#[test]
fn pdfium_recognises_the_text_field_and_paints_its_appearance() {
    let fields = [text_spec(
        "full_name",
        [72.0, 620.0, 320.0, 644.0],
        "Ada Lovelace",
    )];
    let form = create_form(page_setup(), &fields).expect("create form");

    let document = support::pdfium::open(&form.bytes);
    let pdf_form = document.form().expect("PDFium finds an AcroForm");
    assert_eq!(pdf_form.form_type(), PdfFormType::Acrobat);

    let page = document.pages().get(0).expect("page 0");
    let widgets: Vec<_> = page.annotations().iter().collect();
    assert_eq!(widgets.len(), 1, "one widget annotation");
    let annotation = &widgets[0];
    assert_eq!(annotation.annotation_type(), PdfPageAnnotationType::Widget);
    let field = annotation.as_form_field().expect("widget is a form field");
    assert_eq!(field.field_type(), PdfFormFieldType::Text);
    assert_eq!(field.name().as_deref(), Some("full_name"));
    let text = field.as_text_field().expect("text field");
    assert_eq!(text.value().as_deref(), Some("Ada Lovelace"));
    assert!(field.is_required());

    let static_document = support::pdfium::open(&form.bytes);
    let painted = support::pdfium::render(&static_document, 0);
    let baseline_bytes = blank_page(page_setup()).expect("blank page");
    let baseline_document = support::pdfium::open(&baseline_bytes);
    let baseline = support::pdfium::render(&baseline_document, 0);

    let dark_in_field = painted.dark_pixels_in([72.0, 620.0, 320.0, 644.0]);
    assert!(
        dark_in_field > 100,
        "the text and border appearance must paint inside the widget (got {dark_in_field} dark pixels)"
    );

    let differing = painted.pixels_differing(&baseline);
    assert!(!differing.is_empty(), "the field changes the rendered page");
    let (min_x, max_x) = differing.iter().fold((u32::MAX, 0), |(min, max), (x, _)| {
        (min.min(*x), max.max(*x))
    });
    let (min_y, max_y) = differing.iter().fold((u32::MAX, 0), |(min, max), (_, y)| {
        (min.min(*y), max.max(*y))
    });
    // Image space is top-left; the field rect [72 620 320 644] maps to y 148..172.
    assert!(min_x >= 70 && max_x <= 322, "x range {min_x}..={max_x}");
    assert!(min_y >= 146 && max_y <= 174, "y range {min_y}..={max_y}");
}

fn checkbox_spec(name: &str, rect: [f32; 4], checked: bool) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: None,
        rect,
        required: false,
        font_size: 12.0,
        kind: FieldKind::Checkbox { checked },
    }
}

#[test]
fn checkbox_exposes_off_and_yes_states_and_pdfium_reads_them() {
    let fields = [
        checkbox_spec("subscribe", [72.0, 560.0, 88.0, 576.0], false),
        checkbox_spec("terms", [120.0, 560.0, 136.0, 576.0], true),
    ];
    let form = create_form(page_setup(), &fields).expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    let refs: Vec<_> = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_reference().unwrap())
        .collect();
    for (index, expected) in [(0usize, false), (1, true)] {
        let field = document
            .get_object(refs[index])
            .and_then(Object::as_dict)
            .unwrap();
        assert_eq!(name(field, b"FT"), b"Btn");
        assert_eq!(name(field, b"Subtype"), b"Widget");
        let normal = field
            .get(b"AP")
            .and_then(Object::as_dict)
            .and_then(|ap| ap.get(b"N"))
            .and_then(Object::as_dict)
            .expect("/AP /N state dictionary");
        let off = normal.get(b"Off").and_then(Object::as_reference).unwrap();
        let yes = normal.get(b"Yes").and_then(Object::as_reference).unwrap();
        assert_ne!(off, yes, "/Off and /Yes have distinct appearance streams");
        let off_content = document
            .get_object(off)
            .and_then(Object::as_stream)
            .and_then(|stream| stream.decompressed_content())
            .unwrap();
        let yes_content = document
            .get_object(yes)
            .and_then(Object::as_stream)
            .and_then(|stream| stream.decompressed_content())
            .unwrap();
        assert_ne!(
            off_content, yes_content,
            "/Yes appearance must differ from /Off"
        );
        let state = if expected { b"Yes" } else { b"Off" };
        assert_eq!(name(field, b"AS"), state);
        assert_eq!(name(field, b"V"), state);
    }

    let pdfium = support::pdfium::open(&form.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let mut states: Vec<(String, bool)> = page
        .annotations()
        .iter()
        .filter_map(|annotation| {
            let field = annotation.as_form_field()?;
            let checkbox = field.as_checkbox_field()?;
            Some((
                field.name()?,
                checkbox.is_checked().expect("checkbox state"),
            ))
        })
        .collect();
    states.sort();
    assert_eq!(
        states,
        vec![("subscribe".to_owned(), false), ("terms".to_owned(), true)]
    );

    let static_document = support::pdfium::open(&form.bytes);
    let rendered = support::pdfium::render(&static_document, 0);
    let off_dark = rendered.dark_pixels_in([72.0, 560.0, 88.0, 576.0]);
    let on_dark = rendered.dark_pixels_in([120.0, 560.0, 136.0, 576.0]);
    assert!(off_dark > 0, "the unchecked box border paints");
    assert!(
        on_dark > off_dark,
        "the checked box paints a check mark ({on_dark} vs {off_dark})"
    );
}

fn radio_spec(name: &str, options: Vec<(&str, [f32; 4])>, selected: Option<usize>) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: Some("Subscription term".to_owned()),
        rect: options[0].1,
        required: false,
        font_size: 12.0,
        kind: FieldKind::Radio {
            options: options
                .into_iter()
                .map(|(label, rect)| RadioOption {
                    label: label.to_owned(),
                    rect,
                })
                .collect(),
            selected,
        },
    }
}

#[test]
fn radio_group_uses_kids_off_and_on_states_and_one_selected_as() {
    let fields = [radio_spec(
        "term",
        vec![
            ("Monthly", [72.0, 500.0, 88.0, 516.0]),
            ("Yearly", [120.0, 500.0, 136.0, 516.0]),
        ],
        Some(1),
    )];
    let form = create_form(page_setup(), &fields).expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    let field_refs: Vec<_> = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_reference().unwrap())
        .collect();
    assert_eq!(
        field_refs.len(),
        1,
        "the radio group is one top-level field"
    );
    let parent_id = field_refs[0];
    let parent = document
        .get_object(parent_id)
        .and_then(Object::as_dict)
        .unwrap();
    assert_eq!(name(parent, b"FT"), b"Btn");
    assert_eq!(text(parent, b"T"), "term");
    assert!(integer(parent, b"Ff") & (1 << 15) != 0, "Radio flag bit 16");
    assert!(
        parent.get(b"Subtype").is_err(),
        "the parent field is not itself a widget"
    );
    assert_eq!(name(parent, b"V"), b"Yearly", "selected export value");
    let kids: Vec<_> = parent
        .get(b"Kids")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_reference().unwrap())
        .collect();
    assert_eq!(kids.len(), 2);

    let mut on_states = Vec::new();
    let mut selected = Vec::new();
    for (index, kid_id) in kids.iter().enumerate() {
        let kid = document
            .get_object(*kid_id)
            .and_then(Object::as_dict)
            .unwrap();
        assert_eq!(name(kid, b"Subtype"), b"Widget");
        assert_eq!(
            kid.get(b"Parent").and_then(Object::as_reference).unwrap(),
            parent_id,
            "kid points at the radio parent"
        );
        let normal = kid
            .get(b"AP")
            .and_then(Object::as_dict)
            .and_then(|ap| ap.get(b"N"))
            .and_then(Object::as_dict)
            .unwrap();
        assert!(normal.has(b"Off"), "every kid has an /Off state");
        let on_name = if index == 0 {
            b"Monthly".as_slice()
        } else {
            b"Yearly".as_slice()
        };
        assert!(normal.has(on_name), "kid has its own on state");
        on_states.push(name(kid, b"AS").to_vec());
        selected.push(name(kid, b"AS") == on_name);
    }
    assert_eq!(on_states, vec![b"Off".to_vec(), b"Yearly".to_vec()]);
    assert_eq!(selected, vec![false, true]);

    let pdfium = support::pdfium::open(&form.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let mut radio_states: Vec<(String, Option<String>, bool)> = page
        .annotations()
        .iter()
        .filter_map(|annotation| {
            let field = annotation.as_form_field()?;
            let radio = field.as_radio_button_field()?;
            Some((
                field.name()?,
                radio.group_value(),
                radio.is_checked().expect("radio state"),
            ))
        })
        .collect();
    radio_states.sort();
    assert_eq!(radio_states.len(), 2);
    assert!(radio_states.iter().all(|(name, _, _)| name == "term"));
    assert!(radio_states
        .iter()
        .all(|(_, group, _)| group.as_deref() == Some("Yearly")));
    assert_eq!(
        radio_states
            .iter()
            .filter(|(_, _, checked)| *checked)
            .count(),
        1,
        "exactly one radio kid is selected"
    );

    let static_document = support::pdfium::open(&form.bytes);
    let rendered = support::pdfium::render(&static_document, 0);
    let first = rendered.dark_pixels_in([72.0, 500.0, 88.0, 516.0]);
    let second = rendered.dark_pixels_in([120.0, 500.0, 136.0, 516.0]);
    assert!(first > 0 && second > 0, "both radio circles paint");
    assert!(
        second > first,
        "the selected radio paints a filled dot ({second} vs {first})"
    );
}

fn list_spec(name: &str, rect: [f32; 4], options: &[&str], selected: usize) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: Some("Pick one".to_owned()),
        rect,
        required: false,
        font_size: 12.0,
        kind: FieldKind::List {
            options: options.iter().map(|option| option.to_string()).collect(),
            selected: vec![selected],
        },
    }
}

fn dropdown_spec(name: &str, rect: [f32; 4], options: &[&str], selected: usize) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: None,
        rect,
        required: true,
        font_size: 12.0,
        kind: FieldKind::Dropdown {
            options: options.iter().map(|option| option.to_string()).collect(),
            selected: Some(selected),
        },
    }
}

#[test]
fn list_and_dropdown_choice_fields_have_opt_v_i_and_appearances() {
    let fields = [
        list_spec(
            "plan_list",
            [72.0, 440.0, 220.0, 500.0],
            &["Alpha", "Beta", "Gamma"],
            1,
        ),
        dropdown_spec(
            "plan_combo",
            [260.0, 440.0, 420.0, 462.0],
            &["One", "Two", "Three"],
            1,
        ),
    ];
    let form = create_form(page_setup(), &fields).expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    let refs: Vec<_> = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_reference().unwrap())
        .collect();
    assert_eq!(refs.len(), 2);

    let list = document
        .get_object(refs[0])
        .and_then(Object::as_dict)
        .unwrap();
    assert_eq!(name(list, b"FT"), b"Ch");
    assert_eq!(integer(list, b"Ff") & (1 << 17), 0, "list box is not combo");
    assert_eq!(text(list, b"V"), "Beta");
    let list_options: Vec<_> = list
        .get(b"Opt")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| std::str::from_utf8(object.as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(list_options, vec!["Alpha", "Beta", "Gamma"]);
    assert_eq!(
        list.get(b"I")
            .and_then(Object::as_array)
            .unwrap()
            .iter()
            .map(|object| object.as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1],
        "selected index"
    );

    let combo = document
        .get_object(refs[1])
        .and_then(Object::as_dict)
        .unwrap();
    assert_eq!(name(combo, b"FT"), b"Ch");
    assert_ne!(integer(combo, b"Ff") & (1 << 17), 0, "combo flag set");
    assert_eq!(text(combo, b"V"), "Two");

    // The static appearance must show the selected value; otherwise viewers that
    // do not regenerate appearances would render a blank field.
    for (field, needle) in [(&list, "Beta"), (&combo, "Two")] {
        let stream = field
            .get(b"AP")
            .and_then(Object::as_dict)
            .and_then(|ap| ap.get(b"N"))
            .and_then(Object::as_reference)
            .unwrap();
        let content = document
            .get_object(stream)
            .and_then(Object::as_stream)
            .and_then(|stream| stream.decompressed_content())
            .unwrap();
        assert!(
            String::from_utf8_lossy(&content).contains(needle),
            "appearance draws the selected value {needle}"
        );
    }

    let pdfium = support::pdfium::open(&form.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let mut fields: Vec<(String, PdfFormFieldType, String)> = page
        .annotations()
        .iter()
        .filter_map(|annotation| {
            let field = annotation.as_form_field()?;
            let value = match field.field_type() {
                PdfFormFieldType::ListBox => field.as_list_box_field()?.value()?,
                PdfFormFieldType::ComboBox => field.as_combo_box_field()?.value()?,
                _ => return None,
            };
            Some((field.name()?, field.field_type(), value))
        })
        .collect();
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        fields,
        vec![
            (
                "plan_combo".to_owned(),
                PdfFormFieldType::ComboBox,
                "Two".to_owned()
            ),
            (
                "plan_list".to_owned(),
                PdfFormFieldType::ListBox,
                "Beta".to_owned()
            ),
        ]
    );

    let static_document = support::pdfium::open(&form.bytes);
    let rendered = support::pdfium::render(&static_document, 0);
    assert!(
        rendered.dark_pixels_in([72.0, 440.0, 220.0, 500.0]) > 100,
        "the list box appearance paints"
    );
    assert!(
        rendered.dark_pixels_in([260.0, 440.0, 420.0, 462.0]) > 20,
        "the combo box appearance paints"
    );
}

#[test]
fn empty_signature_field_is_only_structure() {
    let specs = [FieldSpec {
        name: "signature".to_owned(),
        tooltip: Some("Sign here".to_owned()),
        rect: [72.0, 360.0, 300.0, 410.0],
        required: true,
        font_size: 12.0,
        kind: FieldKind::Signature,
    }];
    let form = create_form(page_setup(), &specs).expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    assert_eq!(
        acroform.get(b"SigFlags").and_then(Object::as_i64).unwrap() & 1,
        1,
        "SignaturesExist"
    );
    let field_ref = acroform.get(b"Fields").and_then(Object::as_array).unwrap()[0]
        .as_reference()
        .unwrap();
    let field = document
        .get_object(field_ref)
        .and_then(Object::as_dict)
        .unwrap();
    assert_eq!(name(field, b"FT"), b"Sig");
    assert_eq!(name(field, b"Subtype"), b"Widget");
    assert_eq!(text(field, b"T"), "signature");
    assert_eq!(integer(field, b"Ff"), 2, "required");
    assert!(field.get(b"V").is_err(), "an unsigned field has no value");
    assert!(
        field.get(b"AP").is_err(),
        "an empty signature field has no appearance stream"
    );

    let pdfium = support::pdfium::open(&form.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let fields: Vec<_> = page
        .annotations()
        .iter()
        .filter_map(|annotation| {
            let field = annotation.as_form_field()?;
            Some((field.name()?, field.field_type()))
        })
        .collect();
    assert_eq!(
        fields,
        vec![("signature".to_owned(), PdfFormFieldType::Signature)]
    );
}

#[test]
fn merging_into_an_existing_acroform_preserves_old_fields_and_uniquifies_names() {
    let base = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/pdfs/form-acroform.pdf"
    ))
    .expect("read the existing-form fixture");

    let fields = [
        text_spec("full_name", [72.0, 540.0, 320.0, 564.0], "Grace Hopper"),
        checkbox_spec("new_opt_in", [72.0, 500.0, 88.0, 516.0], true),
    ];
    let merged = add_fields(&base, 0, &fields).expect("merge fields");
    assert_eq!(
        merged.field_names,
        vec!["full_name_2".to_owned(), "new_opt_in".to_owned()],
        "existing names are never reused"
    );
    assert!(
        merged.bytes.starts_with(&base),
        "the merge is an incremental append, not a rewrite"
    );

    let document = Document::load_mem(&merged.bytes).expect("independent lopdf parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    let fields: Vec<_> = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_reference().unwrap())
        .collect();
    assert_eq!(fields.len(), 4, "two existing + two new top-level fields");
    let mut names = Vec::new();
    for id in &fields {
        let field = document.get_object(*id).and_then(Object::as_dict).unwrap();
        names.push(text(field, b"T").to_owned());
    }
    assert_eq!(
        names,
        vec![
            "full_name".to_owned(),
            "subscribe".to_owned(),
            "full_name_2".to_owned(),
            "new_opt_in".to_owned(),
        ]
    );
    assert!(
        acroform
            .get(b"NeedAppearances")
            .and_then(Object::as_bool)
            .unwrap(),
        "an existing reader-preference setting is preserved"
    );

    let pdfium = support::pdfium::open(&merged.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let mut state: Vec<(String, String)> = page
        .annotations()
        .iter()
        .filter_map(|annotation| {
            let field = annotation.as_form_field()?;
            let value = if let Some(text) = field.as_text_field() {
                text.value()?
            } else if let Some(checkbox) = field.as_checkbox_field() {
                if checkbox.is_checked().ok()? {
                    "Yes".to_owned()
                } else {
                    "Off".to_owned()
                }
            } else {
                return None;
            };
            Some((field.name()?, value))
        })
        .collect();
    state.sort();
    assert_eq!(
        state,
        vec![
            ("full_name".to_owned(), "Ada Lovelace".to_owned()),
            ("full_name_2".to_owned(), "Grace Hopper".to_owned()),
            ("new_opt_in".to_owned(), "Yes".to_owned()),
            ("subscribe".to_owned(), "Off".to_owned()),
        ]
    );
    assert!(
        page.text().unwrap().all().contains("PDFX AcroForm fixture"),
        "the existing page content survives the incremental merge"
    );

    let merged_static_bytes = support::pdfium::set_need_appearances(&merged.bytes, false);
    let base_static_bytes = support::pdfium::set_need_appearances(&base, false);
    let merged_static = support::pdfium::open(&merged_static_bytes);
    let base_static = support::pdfium::open(&base_static_bytes);
    let merged_render = support::pdfium::render(&merged_static, 0);
    let base_render = support::pdfium::render(&base_static, 0);
    let differing = merged_render.pixels_differing(&base_render);
    assert!(!differing.is_empty(), "the new widgets paint");
    // Rects [72 540 320 564] and [72 500 88 516] become image-space
    // y 228..252 and 276..292; nothing outside those two widgets may change.
    let outside: Vec<_> = differing
        .iter()
        .filter(|(x, y)| {
            let text_field = (70..=322).contains(x) && (226..=254).contains(y);
            let checkbox = (70..=90).contains(x) && (274..=294).contains(y);
            !text_field && !checkbox
        })
        .copied()
        .collect();
    assert!(
        outside.is_empty(),
        "existing page pixels changed outside the new widgets: {outside:?}"
    );
}

#[test]
fn text_appearance_paints_the_value_in_the_static_appearance_stream() {
    let form = create_form(
        page_setup(),
        &[text_spec(
            "full_name",
            [72.0, 620.0, 320.0, 644.0],
            "Ada Lovelace",
        )],
    )
    .expect("create form");

    // Inset past the border: only the value text can paint here. This is the
    // appearance that flattened forms and viewers that honour /AP show.
    let static_document = support::pdfium::open(&form.bytes);
    let rendered = support::pdfium::render(&static_document, 0);
    let interior = rendered.dark_pixels_in([76.0, 624.0, 316.0, 640.0]);
    assert!(
        interior > 20,
        "the text value must paint in the static /AP stream (got {interior} dark pixels)"
    );
}
