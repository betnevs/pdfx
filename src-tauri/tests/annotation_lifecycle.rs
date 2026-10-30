//! Exercises annotation handle lifetime through the adapter on every platform.
mod support;
use pdfx_lib::{engine::Engine, pdfium_adapter as adapter};

#[test]
fn annotation_metadata_survives_handle_close_save_and_reopen() {
    let fixture = support::synthetic::single_line_pdf();
    eprintln!("[DEBUG-win-annot] open");
    let engine = Engine::shared().unwrap();
    let document = engine
        .open_bytes(std::fs::read(fixture.path_string()).unwrap(), None)
        .unwrap();
    eprintln!("[DEBUG-win-annot] create");
    let annotation = adapter::create_ink_annotation(
        &document,
        0,
        adapter::AnnotationRect::new(100.0, 100.0, 200.0, 200.0),
    )
    .unwrap();
    eprintln!("[DEBUG-win-annot] name");
    adapter::set_annotation_name(&document, annotation, "lifecycle").unwrap();
    eprintln!("[DEBUG-win-annot] author");
    adapter::set_annotation_author(&document, annotation, "作者").unwrap();
    eprintln!("[DEBUG-win-annot] contents");
    adapter::set_annotation_contents(&document, annotation, "内容").unwrap();
    eprintln!("[DEBUG-win-annot] read");
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
    eprintln!("[DEBUG-win-annot] save");
    let bytes = document.save_to_bytes().unwrap();
    eprintln!("[DEBUG-win-annot] reopen");
    let reopened = engine.open_bytes(bytes, None).unwrap();
    assert_eq!(
        adapter::annotation_contents(&reopened, annotation)
            .unwrap()
            .as_deref(),
        Some("内容")
    );
    eprintln!("[DEBUG-win-annot] done");
}
