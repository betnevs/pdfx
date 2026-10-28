mod support;

use lopdf::{Document, Object};

use pdfx_acroform_create_spike::{create_form, FieldKind, FieldSpec, PageSetup};

fn page_setup() -> PageSetup {
    PageSetup {
        width: 612.0,
        height: 792.0,
        rotate: 0,
        crop_box: None,
    }
}

#[test]
fn non_ascii_text_values_are_utf16be_and_pdfium_reads_them_back() {
    let value = "你好，PDFX";
    let form = create_form(
        page_setup(),
        &[FieldSpec {
            name: "greeting".to_owned(),
            tooltip: Some("问候".to_owned()),
            rect: [72.0, 620.0, 320.0, 644.0],
            required: false,
            font_size: 12.0,
            kind: FieldKind::Text {
                value: value.to_owned(),
                multiline: false,
            },
        }],
    )
    .expect("create form");

    let document = Document::load_mem(&form.bytes).expect("independent parse");
    let catalog = document.catalog().expect("catalog");
    let acroform = catalog.get(b"AcroForm").and_then(Object::as_dict).unwrap();
    let field_id = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .and_then(|fields| fields[0].as_reference())
        .unwrap();
    let field = document
        .get_object(field_id)
        .and_then(Object::as_dict)
        .unwrap();
    let bytes = match field.get(b"V").unwrap() {
        Object::String(bytes, _) => bytes,
        other => panic!("unexpected /V object: {other:?}"),
    };
    assert_eq!(&bytes[..2], &[0xfe, 0xff], "UTF-16BE BOM");
    let units: Vec<u16> = bytes[2..]
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), value);

    let pdfium = support::pdfium::open(&form.bytes);
    let page = pdfium.pages().get(0).unwrap();
    let annotation = page.annotations().get(0).unwrap();
    let text = annotation
        .as_form_field()
        .expect("text field")
        .as_text_field()
        .expect("text field");
    assert_eq!(text.value().as_deref(), Some(value));
}
