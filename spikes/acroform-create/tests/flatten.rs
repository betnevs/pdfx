mod support;

use lopdf::{Document, Object};

use pdfx_acroform_create_spike::{
    create_form, flatten_form, FieldKind, FieldSpec, PageSetup, RadioOption,
};

fn field(name: &str, rect: [f32; 4], kind: FieldKind) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: None,
        rect,
        required: false,
        font_size: 12.0,
        kind,
    }
}

#[test]
fn flattening_bakes_every_appearance_and_removes_the_form() {
    let fields = vec![
        field(
            "name",
            [72.0, 700.0, 320.0, 724.0],
            FieldKind::Text {
                value: "Ada Lovelace".to_owned(),
                multiline: false,
            },
        ),
        field(
            "agree",
            [72.0, 660.0, 88.0, 676.0],
            FieldKind::Checkbox { checked: true },
        ),
        field(
            "term",
            [120.0, 660.0, 136.0, 676.0],
            FieldKind::Radio {
                options: vec![
                    RadioOption {
                        label: "Monthly".to_owned(),
                        rect: [120.0, 660.0, 136.0, 676.0],
                    },
                    RadioOption {
                        label: "Yearly".to_owned(),
                        rect: [150.0, 660.0, 166.0, 676.0],
                    },
                ],
                selected: Some(1),
            },
        ),
        field(
            "plan",
            [72.0, 600.0, 220.0, 650.0],
            FieldKind::List {
                options: vec!["Alpha".to_owned(), "Beta".to_owned(), "Gamma".to_owned()],
                selected: vec![1],
            },
        ),
        field(
            "combo",
            [260.0, 600.0, 420.0, 622.0],
            FieldKind::Dropdown {
                options: vec!["One".to_owned(), "Two".to_owned()],
                selected: Some(1),
            },
        ),
    ];
    let form = create_form(
        PageSetup {
            width: 612.0,
            height: 792.0,
            rotate: 0,
            crop_box: None,
        },
        &fields,
    )
    .unwrap();

    let before_pdfium = support::pdfium::open(&form.bytes);
    let before = support::pdfium::render(&before_pdfium, 0);
    let flattened = flatten_form(&form.bytes).expect("flatten");
    assert_ne!(flattened, form.bytes);

    let document = Document::load_mem(&flattened).expect("independent parse");
    let catalog = document.catalog().unwrap();
    assert!(
        catalog.get(b"AcroForm").is_err(),
        "the flattened catalog has no AcroForm"
    );
    let page_id = document.get_pages().get(&1).copied().unwrap();
    assert!(
        document
            .get_page_content(page_id)
            .windows(3)
            .any(|w| w == b" Do"),
        "page content draws the baked appearances"
    );

    let after_pdfium = support::pdfium::open(&flattened);
    assert!(after_pdfium.form().is_none(), "PDFium sees no form");
    let page = after_pdfium.pages().get(0).unwrap();
    assert_eq!(page.annotations().len(), 0, "no widget annotations remain");
    let after = support::pdfium::render(&after_pdfium, 0);
    let differing = before.pixels_differing(&after);
    assert!(
        differing.is_empty(),
        "flattening must not change the rendered page ({} pixels differ)",
        differing.len()
    );

    let baseline = Document::load_mem(&flattened).unwrap();
    let page_id = *baseline.get_pages().get(&1).unwrap();
    let page = baseline
        .get_object(page_id)
        .and_then(Object::as_dict)
        .unwrap();
    assert!(
        page.get(b"Annots")
            .and_then(Object::as_array)
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "page has no annotations"
    );
}
