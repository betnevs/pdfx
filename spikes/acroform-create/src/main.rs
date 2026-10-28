//! Deterministic fixture generator for the AcroForm creation spike.
//!
//! Run from anywhere with `cargo run --manifest-path ...` or from this crate
//! with `cargo run`; output always goes to `spikes/acroform-create/fixtures/`.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, Stream};
use pdfx_acroform_create_spike::{
    add_fields, blank_page, display_rect_to_user_space, flatten_form, FieldKind, FieldSpec,
    PageSetup, RadioOption,
};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = root.join("fixtures");
    fs::create_dir_all(&fixtures).context("create fixture directory")?;

    generate_all_fields(&fixtures)?;
    generate_merged(&root, &fixtures)?;
    generate_rotated_crop(&fixtures)?;
    Ok(())
}

fn page_setup() -> PageSetup {
    PageSetup {
        width: 612.0,
        height: 792.0,
        rotate: 0,
        crop_box: None,
    }
}

fn field(name: &str, rect: [f32; 4], kind: FieldKind) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: Some(name.to_owned()),
        rect,
        required: false,
        font_size: 12.0,
        kind,
    }
}

fn generate_all_fields(fixtures: &PathBuf) -> Result<()> {
    let labels = [
        (72.0, 748.0, "PDFX AcroForm creation spike"),
        (72.0, 730.0, "Full name (required)"),
        (96.0, 603.0, "Subscribe"),
        (184.0, 603.0, "Unsubscribe"),
        (72.0, 570.0, "Billing period"),
        (96.0, 553.0, "Monthly"),
        (184.0, 553.0, "Yearly"),
        (272.0, 553.0, "Quarterly"),
        (72.0, 536.0, "Plan (list)"),
        (260.0, 536.0, "Country (dropdown)"),
        (72.0, 446.0, "Signature (empty)"),
        (72.0, 380.0, "Empty text"),
    ];
    let base = with_labels(&blank_page(page_setup())?, &labels)?;

    let mut full_name = field(
        "full_name",
        [72.0, 700.0, 320.0, 724.0],
        FieldKind::Text {
            value: "Ada Lovelace".to_owned(),
            multiline: false,
        },
    );
    full_name.required = true;

    let fields = vec![
        full_name,
        field(
            "subscribe",
            [72.0, 600.0, 88.0, 616.0],
            FieldKind::Checkbox { checked: true },
        ),
        field(
            "unsubscribe",
            [160.0, 600.0, 176.0, 616.0],
            FieldKind::Checkbox { checked: false },
        ),
        field(
            "billing",
            [72.0, 550.0, 264.0, 566.0],
            FieldKind::Radio {
                options: vec![
                    RadioOption {
                        label: "Monthly".to_owned(),
                        rect: [72.0, 550.0, 88.0, 566.0],
                    },
                    RadioOption {
                        label: "Yearly".to_owned(),
                        rect: [160.0, 550.0, 176.0, 566.0],
                    },
                    RadioOption {
                        label: "Quarterly".to_owned(),
                        rect: [248.0, 550.0, 264.0, 566.0],
                    },
                ],
                selected: Some(1),
            },
        ),
        field(
            "plan",
            [72.0, 470.0, 220.0, 530.0],
            FieldKind::List {
                options: vec!["Alpha".to_owned(), "Beta".to_owned(), "Gamma".to_owned()],
                selected: vec![1],
            },
        ),
        field(
            "country",
            [260.0, 508.0, 420.0, 530.0],
            FieldKind::Dropdown {
                options: vec![
                    "China".to_owned(),
                    "Japan".to_owned(),
                    "Singapore".to_owned(),
                ],
                selected: Some(2),
            },
        ),
        field(
            "signature",
            [72.0, 400.0, 300.0, 440.0],
            FieldKind::Signature,
        ),
        field(
            "empty_text",
            [72.0, 350.0, 320.0, 374.0],
            FieldKind::Text {
                value: String::new(),
                multiline: false,
            },
        ),
    ];

    let form = add_fields(&base, 0, &fields)?;
    write_fixture(fixtures, "all-fields.pdf", &form.bytes)?;
    write_fixture(
        fixtures,
        "all-fields-flattened.pdf",
        &flatten_form(&form.bytes)?,
    )?;
    println!("field names: {:?}", form.field_names);
    Ok(())
}

fn generate_merged(root: &PathBuf, fixtures: &PathBuf) -> Result<()> {
    let source = root.join("../../fixtures/pdfs/form-acroform.pdf");
    let bytes = fs::read(&source).with_context(|| format!("read {}", source.display()))?;
    let labels = [
        (72.0, 530.0, "Email (added by PDFX)"),
        (96.0, 463.0, "Newsletter"),
        (160.0, 488.0, "Plan (added)"),
    ];
    let base = with_labels(&bytes, &labels)?;
    let fields = vec![
        field(
            "email",
            [72.0, 500.0, 320.0, 524.0],
            FieldKind::Text {
                value: "ada@example.com".to_owned(),
                multiline: false,
            },
        ),
        field(
            "newsletter",
            [72.0, 460.0, 88.0, 476.0],
            FieldKind::Checkbox { checked: true },
        ),
        field(
            "plan",
            [160.0, 460.0, 300.0, 482.0],
            FieldKind::Dropdown {
                options: vec!["Free".to_owned(), "Pro".to_owned()],
                selected: Some(1),
            },
        ),
    ];
    let form = add_fields(&base, 0, &fields)?;
    write_fixture(fixtures, "merged-form.pdf", &form.bytes)?;
    println!("merged field names: {:?}", form.field_names);
    Ok(())
}

fn generate_rotated_crop(fixtures: &PathBuf) -> Result<()> {
    let setup = PageSetup {
        width: 612.0,
        height: 792.0,
        rotate: 90,
        crop_box: Some([36.0, 36.0, 540.0, 756.0]),
    };
    let crop = setup.crop_box.unwrap();
    let raw = |display: [f32; 4]| {
        display_rect_to_user_space(crop, setup.rotate, display).expect("valid display rect")
    };
    let fields = vec![
        field(
            "rotated_text",
            raw([40.0, 50.0, 200.0, 80.0]),
            FieldKind::Text {
                value: "Rotated page".to_owned(),
                multiline: false,
            },
        ),
        field(
            "rotated_check",
            raw([230.0, 50.0, 246.0, 66.0]),
            FieldKind::Checkbox { checked: true },
        ),
        field(
            "rotated_choice",
            raw([270.0, 50.0, 400.0, 72.0]),
            FieldKind::Dropdown {
                options: vec!["One".to_owned(), "Two".to_owned()],
                selected: Some(0),
            },
        ),
        field(
            "rotated_radio",
            raw([40.0, 120.0, 120.0, 136.0]),
            FieldKind::Radio {
                options: vec![
                    RadioOption {
                        label: "Left".to_owned(),
                        rect: raw([40.0, 120.0, 56.0, 136.0]),
                    },
                    RadioOption {
                        label: "Right".to_owned(),
                        rect: raw([104.0, 120.0, 120.0, 136.0]),
                    },
                ],
                selected: Some(0),
            },
        ),
    ];
    let form = add_fields(&blank_page(setup)?, 0, &fields)?;
    write_fixture(fixtures, "rotated-90-crop.pdf", &form.bytes)?;
    println!("rotated raw rects: {:?}", form.field_names);
    Ok(())
}

fn write_fixture(fixtures: &PathBuf, name: &str, bytes: &[u8]) -> Result<()> {
    let path = fixtures.join(name);
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    println!("{}: {} bytes", path.display(), bytes.len());
    Ok(())
}

/// Adds a small Helvetica label layer to every page. Labels are ASCII so they
/// stay independent of the field-appearance font limitation.
fn with_labels(bytes: &[u8], labels: &[(f32, f32, &str)]) -> Result<Vec<u8>> {
    let mut document = Document::load_mem(bytes).context("parse base page for labels")?;
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    let font_id = document.add_object(font);

    let page_id = document
        .get_pages()
        .get(&1)
        .copied()
        .context("base document has no page 1")?;
    let page = document
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .context("page is not a dictionary")?;
    let mut resources = page
        .get(b"Resources")
        .and_then(Object::as_dict)
        .cloned()
        .unwrap_or_default();
    let mut fonts = resources
        .get(b"Font")
        .and_then(Object::as_dict)
        .cloned()
        .unwrap_or_default();
    fonts.set("LabelFont", Object::Reference(font_id));
    resources.set("Font", fonts);
    page.set("Resources", resources);

    let mut content = document.get_page_content(page_id);
    for (x, y, label) in labels {
        content.extend_from_slice(
            format!(
                "BT /LabelFont 10 Tf {} {} Td {} Tj ET\n",
                x,
                y,
                literal(label)
            )
            .as_bytes(),
        );
    }
    let mut stream = Stream::new(Dictionary::new(), content);
    stream
        .dict
        .set("Length", Object::Integer(stream.content.len() as i64));
    let content_id = document.add_object(stream);
    let page = document
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .context("page is not a dictionary")?;
    page.set("Contents", Object::Reference(content_id));

    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .context("serialize labelled base page")?;
    Ok(output)
}

fn literal(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('(');
    for character in value.chars() {
        match character {
            '\\' | '(' | ')' => {
                output.push('\\');
                output.push(character);
            }
            character if character.is_ascii() => output.push(character),
            _ => output.push('?'),
        }
    }
    output.push(')');
    output
}
