//! Exercises annotation handle lifetime through the adapter on every platform.
mod support;
use pdfx_lib::{engine::Engine, pdfium_adapter as adapter};

#[test]
fn annotation_metadata_survives_handle_close_save_and_reopen() {
    let fixture = support::synthetic::single_line_pdf();
    let engine = Engine::shared().unwrap();
    let document = engine
        .open_bytes(std::fs::read(fixture.path_string()).unwrap(), None)
        .unwrap();
    let annotation = adapter::create_ink_annotation(
        &document,
        0,
        adapter::AnnotationRect::new(100.0, 100.0, 200.0, 200.0),
    )
    .unwrap();
    adapter::set_annotation_name(&document, annotation, "lifecycle").unwrap();
    adapter::set_annotation_author(&document, annotation, "作者").unwrap();
    adapter::set_annotation_contents(&document, annotation, "内容").unwrap();
    adapter::set_annotation_color(&document, annotation, 200, 30, 40, 255).unwrap();
    adapter::set_annotation_border_width(&document, annotation, 2.0).unwrap();
    adapter::append_ink_strokes(
        &document,
        annotation,
        &[vec![(110.0, 110.0), (180.0, 180.0)]],
    )
    .unwrap();
    adapter::set_annotation_appearance(
        &document,
        annotation,
        "q /GS gs 1 0 0 RG 2 w 110 110 m 180 180 l S Q",
    )
    .unwrap();
    // AP streams do not have a single dictionary color. The wrapper must
    // return an error instead of treating an annotation handle as a page object.
    assert!(adapter::annotation_color(&document, annotation).is_err());
    assert!(adapter::annotation_fill_color(&document, annotation).is_err());
    assert!(adapter::set_annotation_color(&document, annotation, 0, 0, 0, 255).is_err());
    assert!(adapter::set_annotation_fill_color(&document, annotation, 0, 0, 0, 255).is_err());
    assert_eq!(
        adapter::annotation_name(&document, annotation)
            .unwrap()
            .as_deref(),
        Some("lifecycle")
    );
    assert_eq!(
        adapter::annotation_author(&document, annotation)
            .unwrap()
            .as_deref(),
        Some("作者")
    );
    let bytes = document.save_to_bytes().unwrap();
    let reopened = engine.open_bytes(bytes, None).unwrap();
    assert_eq!(
        adapter::annotation_contents(&reopened, annotation)
            .unwrap()
            .as_deref(),
        Some("内容")
    );
}
