mod support;

use lopdf::Document;

use pdfx_acroform_create_spike::{
    add_fields, blank_page, display_rect_to_user_space, FieldKind, FieldSpec, PageSetup,
};

fn checkbox(name: &str, rect: [f32; 4]) -> FieldSpec {
    FieldSpec {
        name: name.to_owned(),
        tooltip: None,
        rect,
        required: false,
        font_size: 12.0,
        kind: FieldKind::Checkbox { checked: true },
    }
}

fn difference_bbox(
    with_field: &support::pdfium::Rendered,
    without: &support::pdfium::Rendered,
) -> (u32, u32, u32, u32) {
    let differing = with_field.pixels_differing(without);
    assert!(!differing.is_empty(), "the widget must paint");
    let mut bbox = (u32::MAX, u32::MAX, 0, 0);
    for (x, y) in differing {
        bbox.0 = bbox.0.min(x);
        bbox.1 = bbox.1.min(y);
        bbox.2 = bbox.2.max(x);
        bbox.3 = bbox.3.max(y);
    }
    bbox
}

/// A rectangle in displayed-page coordinates: bottom-left origin, after
/// CropBox and /Rotate are applied.
const DISPLAY_RECT: [f32; 4] = [40.0, 50.0, 90.0, 70.0];

fn assert_display_placement(tag: &str, page: PageSetup, crop: [f32; 4]) {
    let user_rect = display_rect_to_user_space(crop, page.rotate, DISPLAY_RECT).unwrap();
    let base = blank_page(page).unwrap();
    let form = add_fields(&base, 0, &[checkbox("probe", user_rect)]).unwrap();

    let document = Document::load_mem(&form.bytes).unwrap();
    let page_id = *document.get_pages().get(&1).unwrap();
    let page_dict = document
        .get_object(page_id)
        .and_then(lopdf::Object::as_dict)
        .unwrap();
    let stored = page_dict.get(b"Rect"); // page has no Rect; prove field below
    assert!(stored.is_err());
    let field_dict = {
        let catalog = document.catalog().unwrap();
        let acroform = catalog
            .get(b"AcroForm")
            .and_then(lopdf::Object::as_dict)
            .unwrap();
        let field_id = acroform
            .get(b"Fields")
            .and_then(lopdf::Object::as_array)
            .unwrap()[0]
            .as_reference()
            .unwrap();
        document
            .get_object(field_id)
            .and_then(lopdf::Object::as_dict)
            .unwrap()
    };
    let rect: Vec<f32> = field_dict
        .get(b"Rect")
        .and_then(lopdf::Object::as_array)
        .unwrap()
        .iter()
        .map(|object| object.as_float().unwrap())
        .collect();
    assert_eq!(
        rect,
        user_rect.to_vec().as_slice(),
        "{tag}: stored /Rect is raw user space"
    );

    let base_pdfium = support::pdfium::open(&base);
    let static_document = support::pdfium::open(&form.bytes);
    let rendered_with = support::pdfium::render(&static_document, 0);
    let rendered_without = support::pdfium::render(&base_pdfium, 0);
    let bbox = difference_bbox(&rendered_with, &rendered_without);

    // Displayed page -> rendered image: x = u, y = image_height - v.
    let expected_x0 = DISPLAY_RECT[0].round() as u32;
    let expected_x1 = DISPLAY_RECT[2].round() as u32 - 1;
    let expected_y0 = (rendered_with.height as f32 - DISPLAY_RECT[3]).round() as u32;
    let expected_y1 = (rendered_with.height as f32 - DISPLAY_RECT[1]).round() as u32 - 1;
    assert!(
        bbox.0.abs_diff(expected_x0) <= 1
            && bbox.2.abs_diff(expected_x1) <= 1
            && bbox.1.abs_diff(expected_y0) <= 1
            && bbox.3.abs_diff(expected_y1) <= 1,
        "{tag}: painted at {bbox:?}, expected ({expected_x0}, {expected_y0}, {expected_x1}, {expected_y1})"
    );
}

#[test]
fn widget_rects_map_from_display_coordinates_for_every_rotation_and_crop() {
    let crop = [36.0, 36.0, 540.0, 756.0];
    for rotate in [0, 90, 180, 270] {
        assert_display_placement(
            &format!("rotate {rotate}, no crop"),
            PageSetup {
                width: 612.0,
                height: 792.0,
                rotate,
                crop_box: None,
            },
            [0.0, 0.0, 612.0, 792.0],
        );
        assert_display_placement(
            &format!("rotate {rotate}, offset CropBox"),
            PageSetup {
                width: 612.0,
                height: 792.0,
                rotate,
                crop_box: Some(crop),
            },
            crop,
        );
    }
}

#[test]
fn using_raw_coordinates_on_a_rotated_page_moves_the_widget() {
    // The same raw rect that lands at DISPLAY_RECT on an unrotated page lands
    // somewhere else once /Rotate is applied; this is why the transform is not
    // optional.
    let unrotated = display_rect_to_user_space([0.0, 0.0, 612.0, 792.0], 0, DISPLAY_RECT).unwrap();
    let rotated = display_rect_to_user_space([0.0, 0.0, 612.0, 792.0], 90, DISPLAY_RECT).unwrap();
    assert_ne!(unrotated, rotated);

    let base = blank_page(PageSetup {
        width: 612.0,
        height: 792.0,
        rotate: 90,
        crop_box: None,
    })
    .unwrap();
    let form = add_fields(&base, 0, &[checkbox("probe", unrotated)]).unwrap();
    let base_pdfium = support::pdfium::open(&base);
    let static_document = support::pdfium::open(&form.bytes);
    let bbox = difference_bbox(
        &support::pdfium::render(&static_document, 0),
        &support::pdfium::render(&base_pdfium, 0),
    );
    assert_ne!(
        (bbox.0, bbox.1),
        (DISPLAY_RECT[0] as u32, (792.0 - DISPLAY_RECT[3]) as u32),
        "untransformed raw rect does not land at the requested display position"
    );
}
