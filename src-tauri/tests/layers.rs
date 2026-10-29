//! Command-boundary acceptance tests for optional-content layers.
//!
//! Layer listing, visibility, renaming and deletion go through the same Rust
//! functions the Tauri IPC layer exposes. Rendering is checked against pixels
//! produced by PDFium, while saved output is parsed independently with lopdf,
//! so persistence is verified from a fresh parse instead of the session's own
//! state.

mod support;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use support::fixtures;
use support::synthetic;
use support::temp::TempDir;

use pdfx_lib::commands::{
    close_document, open_document, render_page, render_page_region, render_thumbnail,
    CommandErrorCode,
};
use pdfx_lib::layers::{delete_layer, list_layers, rename_layer, set_layer_visible, LayerView};
use pdfx_lib::page_ops::{copy_page, rotate_page};
use pdfx_lib::print_export::{export_page, PageImageFormat};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};

const RED_LAYER: u32 = 0;
const GREEN_LAYER: u32 = 1;
const BLUE_LAYER: u32 = 2;

fn layers_fixture() -> (PathBuf, String) {
    let manifest = fixtures::load_manifest();
    let fixture = fixtures::fixture(&manifest, "layers").expect("layers fixture");
    let path = fixtures::fixture_path(fixture);
    (path.clone(), path.to_string_lossy().into_owned())
}

fn open_layers() -> String {
    let (_, path) = layers_fixture();
    open_document(path, None)
        .expect("the layers fixture should open")
        .document_id
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            password: None,
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
        },
        &mut |_| {},
    )
    .expect("save output");
}

struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl DecodedPng {
    fn pixel(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let offset = ((y * self.width + x) * 4) as usize;
        (
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
        )
    }
}

fn decode_png_bytes(bytes: &[u8]) -> DecodedPng {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes.to_vec()));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buffer).expect("PNG frame");
    buffer.truncate(info.buffer_size());
    DecodedPng {
        width: info.width,
        height: info.height,
        rgba: buffer,
    }
}

fn render_pixels(document_id: &str, page_index: u32) -> DecodedPng {
    let image = render_page(document_id.to_owned(), page_index, 1.0).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload is base64");
    decode_png_bytes(&bytes)
}

fn decode_png_base64(encoded: &str) -> DecodedPng {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("PNG payload is base64");
    decode_png_bytes(&bytes)
}

fn count_pixels(pixels: &DecodedPng, matches: impl Fn((u8, u8, u8)) -> bool) -> u32 {
    (0..pixels.height)
        .flat_map(|y| (0..pixels.width).map(move |x| (x, y)))
        .filter(|(x, y)| matches(pixels.pixel(*x, *y)))
        .count() as u32
}

fn red_count(pixels: &DecodedPng) -> u32 {
    count_pixels(pixels, |(r, g, b)| r > 200 && g < 60 && b < 60)
}

fn green_count(pixels: &DecodedPng) -> u32 {
    count_pixels(pixels, |(r, g, b)| g > 200 && r < 60 && b < 60)
}

fn blue_count(pixels: &DecodedPng) -> u32 {
    count_pixels(pixels, |(r, g, b)| b > 200 && r < 60 && g < 60)
}

fn decode_pdf_name(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = utf16
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|byte| *byte as char).collect()
    }
}

fn oc_properties(document: &lopdf::Document) -> &lopdf::Dictionary {
    document
        .catalog()
        .expect("catalog")
        .get_deref(b"OCProperties", document)
        .expect("/OCProperties")
        .as_dict()
        .expect("/OCProperties is a dictionary")
}

fn default_config(document: &lopdf::Document) -> &lopdf::Dictionary {
    oc_properties(document)
        .get_deref(b"D", document)
        .expect("default config")
        .as_dict()
        .expect("default config is a dictionary")
}

fn array_refs(document: &lopdf::Document, object: &lopdf::Object) -> Vec<lopdf::ObjectId> {
    let array = match object {
        lopdf::Object::Array(array) => array.clone(),
        lopdf::Object::Reference(id) => document
            .get_object(*id)
            .expect("array reference")
            .as_array()
            .expect("array reference")
            .clone(),
        _ => panic!("expected an array"),
    };
    array
        .iter()
        .filter_map(|entry| entry.as_reference().ok())
        .collect()
}

fn ocg_ids(document: &lopdf::Document) -> Vec<lopdf::ObjectId> {
    array_refs(
        document,
        oc_properties(document).get(b"OCGs").expect("/OCGs"),
    )
}

fn config_refs(document: &lopdf::Document, key: &[u8]) -> Vec<lopdf::ObjectId> {
    match default_config(document).get(key).ok() {
        Some(object) => array_refs(document, object),
        None => Vec::new(),
    }
}

fn ocg_names(document: &lopdf::Document) -> Vec<String> {
    ocg_ids(document)
        .iter()
        .map(|id| {
            let dictionary = document.get_dictionary(*id).expect("OCG dictionary");
            decode_pdf_name(
                dictionary
                    .get(b"Name")
                    .expect("OCG name")
                    .as_str()
                    .expect("string"),
            )
        })
        .collect()
}

fn objects_with_type(document: &lopdf::Document, name: &[u8]) -> usize {
    document
        .objects
        .values()
        .filter(|object| {
            object
                .as_dict()
                .ok()
                .and_then(|dictionary| dictionary.get(b"Type").ok())
                .and_then(|object| object.as_name().ok())
                == Some(name)
        })
        .count()
}

fn page_content(document: &lopdf::Document, page_number: u32) -> String {
    let pages = document.get_pages();
    let page_id = *pages.get(&page_number).expect("page");
    String::from_utf8_lossy(&document.get_page_content(page_id)).into_owned()
}

#[test]
fn layer_panel_lists_document_ocgs_with_utf16_names() {
    let document_id = open_layers();
    let list = list_layers(document_id.clone()).expect("list layers");

    assert_eq!(list.document_id, document_id);
    assert_eq!(
        list.layers,
        vec![
            LayerView {
                index: 0,
                name: "红色图层".to_owned(),
                visible: true,
            },
            LayerView {
                index: 1,
                name: "绿色图层".to_owned(),
                visible: true,
            },
            LayerView {
                index: 2,
                name: "蓝色图层".to_owned(),
                visible: false,
            },
        ]
    );

    close_document(document_id).expect("close document");
}

#[test]
fn default_visibility_controls_what_pdfium_paints() {
    let document_id = open_layers();
    let page_one = render_pixels(&document_id, 0);
    let page_two = render_pixels(&document_id, 1);

    assert!(red_count(&page_one) > 0, "红色图层 is on by default");
    assert_eq!(
        blue_count(&page_one),
        0,
        "蓝色图层 is off by default and must not paint"
    );
    assert!(red_count(&page_two) > 0, "红色图层 spans both pages");
    assert!(green_count(&page_two) > 0, "绿色图层 is on by default");

    close_document(document_id).expect("close document");
}

#[test]
fn toggling_visibility_repaints_the_canvas_and_is_undoable() {
    let document_id = open_layers();
    let page_two_before = render_pixels(&document_id, 1);

    let hidden = set_layer_visible(document_id.clone(), RED_LAYER, false).expect("hide 红色图层");
    assert!(!hidden.layers[0].visible);
    assert_eq!(hidden.history.revision, 1);
    assert!(hidden.history.can_undo);
    assert!(hidden.history.dirty);

    let page_one = render_pixels(&document_id, 0);
    let page_two = render_pixels(&document_id, 1);
    assert_eq!(red_count(&page_one), 0, "the hidden layer leaves page 1");
    assert_eq!(red_count(&page_two), 0, "the hidden layer leaves page 2");
    assert_eq!(
        green_count(&page_two),
        green_count(&page_two_before),
        "unrelated layer content is untouched"
    );

    let shown = set_layer_visible(document_id.clone(), RED_LAYER, true).expect("show 红色图层");
    assert!(shown.layers[0].visible);
    assert!(red_count(&render_pixels(&document_id, 0)) > 0);

    let blue = set_layer_visible(document_id.clone(), BLUE_LAYER, true).expect("show 蓝色图层");
    assert!(blue.layers[2].visible);
    assert!(blue_count(&render_pixels(&document_id, 0)) > 0);

    let rehidden =
        set_layer_visible(document_id.clone(), BLUE_LAYER, false).expect("hide 蓝色图层");
    assert!(!rehidden.layers[2].visible);
    assert_eq!(blue_count(&render_pixels(&document_id, 0)), 0);

    // A toggle that changes nothing is not a history step.
    let unchanged =
        set_layer_visible(document_id.clone(), RED_LAYER, true).expect("no-op visibility");
    assert_eq!(unchanged.history.revision, rehidden.history.revision);
    assert!(unchanged.history.can_undo);

    let undo = undo_text_edit(document_id.clone()).expect("undo visibility");
    assert!(undo.history.can_redo);
    let layers = list_layers(document_id.clone())
        .expect("list after undo")
        .layers;
    assert!(layers[2].visible, "undo restores the previous visibility");
    assert!(blue_count(&render_pixels(&document_id, 0)) > 0);

    let redo = redo_text_edit(document_id.clone()).expect("redo visibility");
    assert!(!redo.history.can_redo);
    assert!(!list_layers(document_id.clone()).expect("list").layers[2].visible);
    assert_eq!(blue_count(&render_pixels(&document_id, 0)), 0);

    close_document(document_id).expect("close document");
}

#[test]
fn visibility_and_rename_persist_after_save_and_reopen() {
    let (source, _) = layers_fixture();
    let source_bytes = fs::read(&source).expect("read source");
    let temp = TempDir::new("layers-save");
    let output = temp.path().join("layers-output.pdf");

    let document_id = open_layers();
    set_layer_visible(document_id.clone(), GREEN_LAYER, false).expect("hide 绿色图层");
    rename_layer(document_id.clone(), RED_LAYER, "前景".to_owned()).expect("rename 红色图层");
    save_as(&document_id, &output);

    assert_eq!(
        fs::read(&source).expect("source still readable"),
        source_bytes,
        "saving must never rewrite the Source Document"
    );

    let saved = lopdf::Document::load(&output).expect("load saved PDF");
    assert_eq!(
        ocg_names(&saved),
        ["前景", "绿色图层", "蓝色图层"],
        "the renamed OCG and the untouched names are in one output"
    );
    let ids = ocg_ids(&saved);
    assert_eq!(config_refs(&saved, b"ON"), vec![ids[0]]);
    assert_eq!(config_refs(&saved, b"OFF"), vec![ids[1], ids[2]]);
    let config = default_config(&saved);
    assert_eq!(
        config.get(b"BaseState").expect("BaseState"),
        &lopdf::Object::Name(b"OFF".to_vec())
    );
    assert!(
        !config.has(b"AS"),
        "/AS is dropped so it cannot contradict the explicit state"
    );
    assert_eq!(
        config.get(b"ListMode").expect("ListMode"),
        &lopdf::Object::Name(b"VisiblePages".to_vec()),
        "unknown /D keys are preserved"
    );
    assert_eq!(config_refs(&saved, b"Order"), ids);
    assert_eq!(objects_with_type(&saved, b"OCG"), 3);

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen saved document");
    let reopened_id = reopened.document_id;
    let layers = list_layers(reopened_id.clone())
        .expect("list reopened layers")
        .layers;
    assert_eq!(layers[0].name, "前景");
    assert!(layers[0].visible);
    assert!(!layers[1].visible, "saved /OFF hides 绿色图层");
    assert!(!layers[2].visible);
    assert!(
        green_count(&render_pixels(&reopened_id, 1)) == 0,
        "the saved default visibility is what PDFium paints after reopening"
    );
    assert!(red_count(&render_pixels(&reopened_id, 0)) > 0);

    close_document(reopened_id).expect("close reopened document");
}

#[test]
fn rename_is_one_undoable_history_step_and_the_saved_name_follows_the_history() {
    let temp = TempDir::new("layers-rename");
    let output = temp.path().join("rename-output.pdf");
    let document_id = open_layers();

    let renamed = rename_layer(document_id.clone(), RED_LAYER, "  前景  ".to_owned())
        .expect("rename a layer");
    assert_eq!(renamed.layers[0].name, "前景", "names are trimmed");
    assert_eq!(renamed.history.revision, 1);

    let blank = rename_layer(document_id.clone(), RED_LAYER, "   ".to_owned())
        .expect_err("a blank name is rejected");
    assert_eq!(blank.code, CommandErrorCode::LayerNameInvalid);
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers[0].name,
        "前景"
    );

    undo_text_edit(document_id.clone()).expect("undo rename");
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers[0].name,
        "红色图层"
    );
    redo_text_edit(document_id.clone()).expect("redo rename");
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers[0].name,
        "前景"
    );
    save_as(&document_id, &output);
    assert_eq!(
        ocg_names(&lopdf::Document::load(&output).expect("load output")),
        ["前景", "绿色图层", "蓝色图层"]
    );

    close_document(document_id).expect("close document");
}

#[test]
fn deleting_a_layer_removes_nested_content_and_the_ocg_and_can_be_undone() {
    let temp = TempDir::new("layers-delete");
    let output = temp.path().join("delete-output.pdf");
    let document_id = open_layers();

    set_layer_visible(document_id.clone(), BLUE_LAYER, true).expect("show 蓝色图层");
    assert!(blue_count(&render_pixels(&document_id, 0)) > 0);

    let deleted = delete_layer(document_id.clone(), BLUE_LAYER).expect("delete 蓝色图层");
    assert_eq!(deleted.layers.len(), 2);
    assert!(
        deleted.layers.iter().all(|layer| layer.index != BLUE_LAYER),
        "the deleted layer leaves the panel immediately"
    );
    assert_eq!(blue_count(&render_pixels(&document_id, 0)), 0);
    assert!(red_count(&render_pixels(&document_id, 0)) > 0);

    let undo = undo_text_edit(document_id.clone()).expect("undo deletion");
    assert!(undo.history.can_redo);
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers.len(),
        3,
        "undo brings the layer back in the session"
    );
    assert!(
        blue_count(&render_pixels(&document_id, 0)) > 0,
        "undo brings the layer's content back without saving"
    );
    redo_text_edit(document_id.clone()).expect("redo deletion");
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers.len(),
        2
    );

    save_as(&document_id, &output);
    let saved = lopdf::Document::load(&output).expect("load output");
    assert_eq!(ocg_names(&saved), ["红色图层", "绿色图层"]);
    let ids = ocg_ids(&saved);
    assert_eq!(config_refs(&saved, b"ON"), vec![ids[0], ids[1]]);
    assert!(config_refs(&saved, b"OFF").is_empty());
    assert_eq!(config_refs(&saved, b"Order"), ids);
    // The deleted OCG object itself stays as an unreferenced object in the
    // appended revision; it is gone from /OCGs and /Order, so no reader lists
    // or paints it.
    assert_eq!(ocg_ids(&saved).len(), 2);

    let page_one = page_content(&saved, 1);
    assert!(
        !page_one.contains("/MC2"),
        "the deleted layer's marked-content block is gone: {page_one:?}"
    );
    assert!(
        !page_one.contains("0 0 1 rg"),
        "the deleted layer's blue paint is gone: {page_one:?}"
    );
    assert!(
        page_one.contains("/MC0"),
        "nested surviving content stays: {page_one:?}"
    );
    assert!(
        page_one.contains("1 0 0 rg"),
        "the outer surviving layer stays: {page_one:?}"
    );

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    assert_eq!(
        list_layers(reopened_id.clone())
            .expect("list reopened")
            .layers
            .len(),
        2
    );
    assert_eq!(blue_count(&render_pixels(&reopened_id, 0)), 0);
    assert!(red_count(&render_pixels(&reopened_id, 0)) > 0);
    close_document(reopened_id).expect("close reopened");
}

#[test]
fn a_document_without_layers_lists_nothing_and_reports_missing_layers() {
    let pdf = synthetic::two_page_pdf();
    let document_id = open_document(pdf.path_string(), None)
        .expect("open synthetic document")
        .document_id;

    let list = list_layers(document_id.clone()).expect("list layers");
    assert!(list.layers.is_empty(), "no /OCProperties means no layers");

    let error =
        set_layer_visible(document_id.clone(), 0, false).expect_err("there is no layer to toggle");
    assert_eq!(error.code, CommandErrorCode::LayerNotFound);
    let error =
        rename_layer(document_id.clone(), 0, "x".to_owned()).expect_err("no layer to rename");
    assert_eq!(error.code, CommandErrorCode::LayerNotFound);
    let error = delete_layer(document_id.clone(), 0).expect_err("no layer to delete");
    assert_eq!(error.code, CommandErrorCode::LayerNotFound);

    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert_eq!(history.revision, 0, "refusals do not enter the history");
    assert!(!history.dirty);

    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_layer_edits() {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == "encrypted-owner-restrictions")
        .expect("restricted fixture");
    let view = open_document(
        fixtures::fixture_path(fixture)
            .to_string_lossy()
            .into_owned(),
        None,
    )
    .expect("open restricted document");
    let document_id = view.document_id;
    assert!(view.read_only);

    let list = list_layers(document_id.clone()).expect("listing is read-only");
    assert!(list.layers.is_empty());

    let error = set_layer_visible(document_id.clone(), 0, false)
        .expect_err("read-only documents cannot toggle layers");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error = rename_layer(document_id.clone(), 0, "x".to_owned())
        .expect_err("read-only documents cannot rename layers");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    let error =
        delete_layer(document_id.clone(), 0).expect_err("read-only documents cannot delete layers");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);

    close_document(document_id).expect("close document");
}

#[test]
fn repeated_saves_do_not_duplicate_layer_objects() {
    let temp = TempDir::new("layers-repeat");
    let first = temp.path().join("first.pdf");
    let second = temp.path().join("second.pdf");
    let document_id = open_layers();

    set_layer_visible(document_id.clone(), GREEN_LAYER, false).expect("hide 绿色图层");
    rename_layer(document_id.clone(), BLUE_LAYER, "新蓝色".to_owned()).expect("rename 蓝色图层");
    save_as(&document_id, &first);
    save_as(&document_id, &second);

    for path in [&first, &second] {
        let saved = lopdf::Document::load(path).expect("load output");
        assert_eq!(ocg_names(&saved), ["红色图层", "绿色图层", "新蓝色"]);
        assert_eq!(objects_with_type(&saved, b"OCG"), 3);
        assert_eq!(ocg_ids(&saved).len(), 3, "/OCGs keeps one entry per layer");
        assert_eq!(
            config_refs(&saved, b"Order").len(),
            3,
            "/Order has no duplicates"
        );
    }
    let first_document = lopdf::Document::load(&first).expect("load first output");
    let second_document = lopdf::Document::load(&second).expect("load second output");
    assert_eq!(
        ocg_ids(&first_document),
        ocg_ids(&second_document),
        "two saves of the same session state write the same layer structure"
    );
    assert_eq!(
        config_refs(&first_document, b"ON"),
        config_refs(&second_document, b"ON")
    );
    assert_eq!(
        config_refs(&first_document, b"OFF"),
        config_refs(&second_document, b"OFF")
    );

    close_document(document_id).expect("close document");
}

#[test]
fn copied_and_rotated_pages_render_the_current_layer_state() {
    let document_id = open_layers();

    // The copy is inserted right after its source, so logical page 1 is the
    // copy of page 0 and the original page 2 moves to logical index 2.
    let copied = copy_page(document_id.clone(), 0).expect("copy page 1");
    assert_eq!(copied.page_count, 3);
    assert_eq!(copied.page_index, 1);
    assert!(red_count(&render_pixels(&document_id, 1)) > 0);

    set_layer_visible(document_id.clone(), BLUE_LAYER, true).expect("show 蓝色图层");
    assert!(blue_count(&render_pixels(&document_id, 0)) > 0);
    assert!(
        blue_count(&render_pixels(&document_id, 1)) > 0,
        "the copied page carries the marked content of its source"
    );

    set_layer_visible(document_id.clone(), RED_LAYER, false).expect("hide 红色图层");
    assert_eq!(red_count(&render_pixels(&document_id, 0)), 0);
    assert_eq!(
        red_count(&render_pixels(&document_id, 1)),
        0,
        "hiding a layer repaints the copied page too"
    );

    rotate_page(document_id.clone(), 1, 90).expect("rotate the copied page");
    let rotated = render_pixels(&document_id, 1);
    assert_eq!(
        red_count(&rotated),
        0,
        "rotation keeps the hidden layer hidden"
    );
    assert_eq!((rotated.width, rotated.height), (792, 612));

    set_layer_visible(document_id.clone(), RED_LAYER, true).expect("show 红色图层");
    let shown = render_pixels(&document_id, 1);
    assert!(
        red_count(&shown) > 0,
        "the rotated copied page paints the layer again"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn deleting_a_layer_also_removes_its_content_from_a_copied_page() {
    let temp = TempDir::new("layers-copy-delete");
    let output = temp.path().join("copy-delete.pdf");
    let document_id = open_layers();

    copy_page(document_id.clone(), 0).expect("copy page 1");
    set_layer_visible(document_id.clone(), BLUE_LAYER, true).expect("show 蓝色图层");
    assert!(blue_count(&render_pixels(&document_id, 1)) > 0);

    delete_layer(document_id.clone(), BLUE_LAYER).expect("delete 蓝色图层");
    assert_eq!(blue_count(&render_pixels(&document_id, 0)), 0);
    assert_eq!(
        blue_count(&render_pixels(&document_id, 1)),
        0,
        "the copied page's marked-content block is removed with the layer"
    );
    assert!(red_count(&render_pixels(&document_id, 1)) > 0);

    save_as(&document_id, &output);
    let saved = lopdf::Document::load(&output).expect("load output");
    assert_eq!(ocg_names(&saved), ["红色图层", "绿色图层"]);
    let copy_page = page_content(&saved, 2);
    assert!(
        !copy_page.contains("/MC2") && !copy_page.contains("0 0 1 rg"),
        "the saved copy has no content from the deleted layer: {copy_page:?}"
    );
    assert!(copy_page.contains("1 0 0 rg"));
    // The copy's remaining property names are remapped from PDFium's cloned
    // OCG objects to the canonical /OCProperties entries.
    let ids = ocg_ids(&saved);
    let copy_dictionary = saved
        .get_dictionary(*saved.get_pages().get(&2).expect("copy page"))
        .expect("copy page dictionary");
    let properties = copy_dictionary
        .get_deref(b"Resources", &saved)
        .expect("copy resources")
        .as_dict()
        .expect("resources dictionary")
        .get_deref(b"Properties", &saved)
        .expect("copy properties")
        .as_dict()
        .expect("properties dictionary");
    assert_eq!(
        properties
            .get(b"MC0")
            .expect("MC0")
            .as_reference()
            .expect("reference"),
        ids[0]
    );

    close_document(document_id).expect("close live document");
    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    assert_eq!(
        list_layers(reopened_id.clone())
            .expect("list reopened")
            .layers
            .len(),
        2
    );
    assert_eq!(blue_count(&render_pixels(&reopened_id, 1)), 0);
    assert!(red_count(&render_pixels(&reopened_id, 1)) > 0);
    close_document(reopened_id).expect("close reopened");
}

// ---------------------------------------------------------------------------
// Synthetic optional-content structures
// ---------------------------------------------------------------------------

fn stream_object(extra: &str, data: &[u8]) -> Vec<u8> {
    let mut body = format!("<< /Length {} {extra} >>\nstream\n", data.len()).into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

fn build_test_pdf(objects: &[(u32, Vec<u8>)], root: u32) -> Vec<u8> {
    let max = objects.iter().map(|(number, _)| *number).max().unwrap_or(0);
    let mut output = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0usize; max as usize + 1];
    for number in 1..=max {
        offsets[number as usize] = output.len();
        output.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        match objects.iter().find(|(candidate, _)| *candidate == number) {
            Some((_, body)) => output.extend_from_slice(body),
            None => output.extend_from_slice(b"null"),
        }
        output.extend_from_slice(b"\nendobj\n");
    }
    let xref = output.len();
    output.extend_from_slice(format!("xref\n0 {}\n", max + 1).as_bytes());
    output.extend_from_slice(b"0000000000 65535 f \n");
    for number in 1..=max {
        output.extend_from_slice(format!("{:010} 00000 n \n", offsets[number as usize]).as_bytes());
    }
    output.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            max + 1
        )
        .as_bytes(),
    );
    output
}

fn open_synthetic(temp: &TempDir, name: &str, bytes: Vec<u8>) -> String {
    let path = temp.path().join(name);
    fs::write(&path, bytes).expect("write synthetic PDF");
    open_document(path.to_string_lossy().into_owned(), None)
        .expect("open synthetic document")
        .document_id
}

/// One OCG whose marked content uses an inline OCMD with `/P`.
fn ocmd_policy_pdf() -> Vec<u8> {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /MC0 10 0 R >> >> /Contents 4 0 R >>".to_vec(),
        ),
        (
            4,
            stream_object(
                "",
                b"/OC << /Type /OCMD /OCGs [10 0 R] /P /AllOn >> BDC\nq 1 0 0 rg 20 20 100 100 re f Q\nEMC\n",
            ),
        ),
        (7, b"<< /OCGs [10 0 R] /D << /ON [10 0 R] >> >>".to_vec()),
        (10, b"<< /Type /OCG /Name (Policy) >>".to_vec()),
    ];
    build_test_pdf(&objects, 1)
}

/// A form XObject whose `/OC` names the layer.
fn xobject_oc_pdf() -> Vec<u8> {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /XObject << /Fm1 11 0 R >> >> /Contents 4 0 R >>".to_vec(),
        ),
        (4, stream_object("", b"q /Fm1 Do Q\n")),
        (7, b"<< /OCGs [10 0 R] /D << /ON [10 0 R] >> >>".to_vec()),
        (10, b"<< /Type /OCG /Name (XObject) >>".to_vec()),
        (
            11,
            stream_object(
                "/Type /XObject /Subtype /Form /BBox [0 0 200 200] /OC 10 0 R /Resources << >>",
                b"q 1 0 0 rg 20 20 100 100 re f Q\n",
            ),
        ),
    ];
    build_test_pdf(&objects, 1)
}

/// An annotation whose `/OC` references the layer.
fn annotation_oc_pdf() -> Vec<u8> {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /MC0 10 0 R >> >> /Annots [12 0 R] /Contents 4 0 R >>".to_vec(),
        ),
        (
            4,
            stream_object(
                "",
                b"/OC /MC0 BDC\nq 1 0 0 rg 20 20 100 100 re f Q\nEMC\n",
            ),
        ),
        (7, b"<< /OCGs [10 0 R] /D << /ON [10 0 R] >> >>".to_vec()),
        (10, b"<< /Type /OCG /Name (Annotated) >>".to_vec()),
        (
            12,
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (note) /OC 10 0 R >>".to_vec(),
        ),
    ];
    build_test_pdf(&objects, 1)
}

/// A marked-content block that is opened but never closed.
fn unbalanced_marked_content_pdf() -> Vec<u8> {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /MC0 10 0 R >> >> /Contents 4 0 R >>".to_vec(),
        ),
        (
            4,
            stream_object("", b"/OC /MC0 BDC\nq 1 0 0 rg 20 20 100 100 re f Q\n"),
        ),
        (7, b"<< /OCGs [10 0 R] /D << /ON [10 0 R] >> >>".to_vec()),
        (10, b"<< /Type /OCG /Name (Unbalanced) >>".to_vec()),
    ];
    build_test_pdf(&objects, 1)
}

/// Two layers plus `/Configs`, nested `/Order`, `/RBGroups` and `/AS`.
fn rbgroups_configs_pdf() -> Vec<u8> {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /MC0 10 0 R /MC1 11 0 R >> >> /Contents 4 0 R >>".to_vec(),
        ),
        (
            4,
            stream_object(
                "",
                b"/OC /MC0 BDC\nq 1 0 0 rg 20 20 80 80 re f Q\nEMC\n/OC /MC1 BDC\nq 0 0 1 rg 100 100 80 80 re f Q\nEMC\n",
            ),
        ),
        (
            7,
            b"<< /OCGs [10 0 R 11 0 R] /D 8 0 R /Configs [9 0 R] >>".to_vec(),
        ),
        (
            8,
            b"<< /Type /OCProperties /ListMode /AllPages /ON [10 0 R 11 0 R] /Order [10 0 R [11 0 R] 10 0 R] /RBGroups [[10 0 R 11 0 R]] /AS [<< /Event /View /OCGs [10 0 R 11 0 R] >>] >>".to_vec(),
        ),
        (
            9,
            b"<< /Name (Alt) /ON [10 0 R 11 0 R] /Order [11 0 R 10 0 R] /RBGroups [[10 0 R 11 0 R]] /AS [<< /Event /View /OCGs [11 0 R] >>] >>".to_vec(),
        ),
        (10, b"<< /Type /OCG /Name (Red) >>".to_vec()),
        (11, b"<< /Type /OCG /Name (Blue) >>".to_vec()),
    ];
    build_test_pdf(&objects, 1)
}

/// Every OCG object whose decoded `/Name` equals `name`, reachable or not.
fn ocgs_named(document: &lopdf::Document, name: &str) -> Vec<lopdf::ObjectId> {
    document
        .objects
        .iter()
        .filter_map(|(id, object)| {
            let dictionary = object.as_dict().ok()?;
            if !dictionary.has_type(b"OCG") {
                return None;
            }
            let object_name = dictionary.get(b"Name").ok()?.as_str().ok()?;
            (decode_pdf_name(object_name) == name).then_some(*id)
        })
        .collect()
}

fn assert_unreferenced(document: &lopdf::Document, target: lopdf::ObjectId) {
    fn walk(
        object: &lopdf::Object,
        target: lopdf::ObjectId,
        visited: &mut HashSet<lopdf::ObjectId>,
        document: &lopdf::Document,
    ) {
        match object {
            lopdf::Object::Reference(id) => {
                assert_ne!(*id, target, "a reachable object references the retired OCG");
                if visited.insert(*id) {
                    if let Ok(next) = document.get_object(*id) {
                        walk(next, target, visited, document);
                    }
                }
            }
            lopdf::Object::Array(items) => {
                for item in items {
                    walk(item, target, visited, document);
                }
            }
            lopdf::Object::Dictionary(dictionary) => {
                for (_, value) in dictionary.iter() {
                    walk(value, target, visited, document);
                }
            }
            lopdf::Object::Stream(stream) => {
                for (_, value) in stream.dict.iter() {
                    walk(value, target, visited, document);
                }
            }
            _ => {}
        }
    }

    let mut visited = HashSet::new();
    walk(
        &lopdf::Object::Dictionary(document.trailer.clone()),
        target,
        &mut visited,
        document,
    );
    assert!(
        !visited.contains(&target),
        "the retired OCG object is still reachable"
    );
}

// ---------------------------------------------------------------------------
// Copied pages saved before a layer edit
// ---------------------------------------------------------------------------

#[test]
fn a_copied_page_saved_before_a_layer_edit_keeps_hidden_layers_hidden() {
    let temp = TempDir::new("layers-copy-no-edit");
    let output = temp.path().join("copy-no-edit.pdf");
    let document_id = open_layers();

    copy_page(document_id.clone(), 0).expect("copy page 1");

    // No layer command has touched the session yet. The copy still references
    // PDFium's cloned OCG dictionaries, so this render proves the shadow maps
    // them back to the canonical layers.
    let copy_before = render_pixels(&document_id, 1);
    assert!(red_count(&copy_before) > 0, "the copy paints 红色图层");
    assert_eq!(
        blue_count(&copy_before),
        0,
        "蓝色图层 is off by default and must stay hidden on the copy"
    );

    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let saved = lopdf::Document::load(&output).expect("load output");
    let ids = ocg_ids(&saved);
    let copy_id = *saved.get_pages().get(&2).expect("copy is the second page");
    let copy = saved.get_dictionary(copy_id).expect("copy page dictionary");
    let properties = copy
        .get_deref(b"Resources", &saved)
        .expect("copy resources")
        .as_dict()
        .expect("resources dictionary")
        .get_deref(b"Properties", &saved)
        .expect("copy properties")
        .as_dict()
        .expect("properties dictionary");
    assert!(!properties.is_empty());
    for (key, value) in properties.iter() {
        let id = value
            .as_reference()
            .expect("every property names an OCG reference");
        assert!(
            ids.contains(&id),
            "property /{} points at cloned OCG {id:?} instead of a canonical layer",
            String::from_utf8_lossy(key)
        );
    }

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    let copy_after = render_pixels(&reopened_id, 1);
    assert_eq!(
        blue_count(&copy_after),
        0,
        "the saved copy keeps 蓝色图层 hidden"
    );
    assert!(red_count(&copy_after) > 0);
    assert_eq!(blue_count(&render_pixels(&reopened_id, 0)), 0);
    assert_eq!(
        list_layers(reopened_id.clone()).expect("list").layers.len(),
        3
    );
    close_document(reopened_id).expect("close reopened");
}

// ---------------------------------------------------------------------------
// Deleted layers must not remain reachable
// ---------------------------------------------------------------------------

#[test]
fn a_deleted_layer_is_unreachable_in_the_saved_document() {
    let temp = TempDir::new("layers-delete-reachability");
    let output = temp.path().join("delete-reachability.pdf");
    let document_id = open_layers();

    delete_layer(document_id.clone(), BLUE_LAYER).expect("delete 蓝色图层");
    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let saved = lopdf::Document::load(&output).expect("load output");
    assert_eq!(ocg_names(&saved), ["红色图层", "绿色图层"]);
    let lingering = ocgs_named(&saved, "蓝色图层");
    assert!(
        !lingering.is_empty(),
        "the incremental revision still carries the retired OCG object"
    );
    for id in &lingering {
        assert_unreferenced(&saved, *id);
    }
    assert!(
        !page_content(&saved, 1).contains("/MC2"),
        "the retired layer's marked content and property name are gone"
    );

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    assert_eq!(
        list_layers(reopened_id.clone()).expect("list").layers.len(),
        2
    );
    assert_eq!(blue_count(&render_pixels(&reopened_id, 0)), 0);
    close_document(reopened_id).expect("close reopened");
}

#[test]
fn layer_deletion_filters_alternative_configs_nested_order_and_rbgroups() {
    let temp = TempDir::new("layers-rbgroups-configs");
    let output = temp.path().join("filtered-config.pdf");
    let document_id = open_synthetic(&temp, "rbgroups.pdf", rbgroups_configs_pdf());
    assert_eq!(
        list_layers(document_id.clone()).expect("list").layers.len(),
        2
    );

    delete_layer(document_id.clone(), 1).expect("delete Blue");
    save_as(&document_id, &output);
    close_document(document_id).expect("close live document");

    let saved = lopdf::Document::load(&output).expect("load output");
    assert_eq!(ocg_names(&saved), ["Red"]);
    let ids = ocg_ids(&saved);
    let lingering = ocgs_named(&saved, "Blue");
    assert!(!lingering.is_empty());
    for id in &lingering {
        assert_unreferenced(&saved, *id);
    }

    assert_eq!(
        config_refs(&saved, b"Order"),
        vec![ids[0], ids[0]],
        "nested /Order keeps only the surviving layer"
    );
    let configs = oc_properties(&saved)
        .get_deref(b"Configs", &saved)
        .expect("alternative configs survive")
        .as_array()
        .expect("Configs is an array");
    assert_eq!(configs.len(), 1);
    let alternative = match &configs[0] {
        lopdf::Object::Reference(id) => saved.get_dictionary(*id).expect("config object"),
        lopdf::Object::Dictionary(dictionary) => dictionary,
        other => panic!("unexpected config entry: {other:?}"),
    };
    assert_eq!(
        array_refs(&saved, alternative.get(b"Order").expect("config Order")),
        ids,
        "the alternative configuration keeps only the surviving layer"
    );

    let reopened =
        open_document(output.to_string_lossy().into_owned(), None).expect("reopen output");
    let reopened_id = reopened.document_id;
    let layers = list_layers(reopened_id.clone())
        .expect("list reopened")
        .layers;
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].name, "Red");
    assert!(red_count(&render_pixels(&reopened_id, 0)) > 0);
    assert_eq!(blue_count(&render_pixels(&reopened_id, 0)), 0);
    close_document(reopened_id).expect("close reopened");
}

// ---------------------------------------------------------------------------
// Refused deletions leave the session untouched
// ---------------------------------------------------------------------------

#[test]
fn unsafe_layer_deletions_are_refused_without_history_or_mutation() {
    let cases: [(&str, fn() -> Vec<u8>); 4] = [
        ("ocmd-policy", ocmd_policy_pdf),
        ("xobject-oc", xobject_oc_pdf),
        ("annotation-oc", annotation_oc_pdf),
        ("unbalanced", unbalanced_marked_content_pdf),
    ];

    for (name, build) in cases {
        let temp = TempDir::new(name);
        let document_id = open_synthetic(&temp, &format!("{name}.pdf"), build());
        let before = list_layers(document_id.clone())
            .expect("list before refusal")
            .layers;
        assert_eq!(
            before.len(),
            1,
            "{name}: the synthetic document has one layer"
        );

        let error = delete_layer(document_id.clone(), 0)
            .expect_err("this layer's content cannot be removed safely");
        assert_eq!(
            error.code,
            CommandErrorCode::LayerDeleteUnsupported,
            "{name}"
        );

        let history = edit_history(document_id.clone()).expect("history");
        assert_eq!(history.revision, 0, "{name}: a refusal adds no history");
        assert!(!history.dirty, "{name}");
        assert_eq!(
            list_layers(document_id.clone())
                .expect("list after refusal")
                .layers,
            before,
            "{name}: the panel is unchanged"
        );
        assert!(
            red_count(&render_pixels(&document_id, 0)) > 0,
            "{name}: the document still renders"
        );

        close_document(document_id).expect("close document");
    }
}

// ---------------------------------------------------------------------------
// Every rendering surface follows the layer state
// ---------------------------------------------------------------------------

#[test]
fn hiding_a_layer_updates_thumbnails_region_renders_and_exported_pages() {
    let temp = TempDir::new("layers-render-surfaces");
    let document_id = open_layers();
    set_layer_visible(document_id.clone(), BLUE_LAYER, true).expect("show 蓝色图层");
    assert!(blue_count(&render_pixels(&document_id, 0)) > 0);

    let thumbnail = render_thumbnail(document_id.clone(), 0, 200).expect("thumbnail");
    assert!(
        blue_count(&decode_png_base64(&thumbnail.png_base64)) > 0,
        "the thumbnail paints the visible layer"
    );
    let region =
        render_page_region(document_id.clone(), 0, 1.0, 280, 170, 240, 140).expect("page region");
    assert!(
        blue_count(&decode_png_base64(&region.png_base64)) > 0,
        "the region covering 蓝色图层 paints it"
    );
    let export_path = temp.path().join("page.png");
    export_page(
        document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        export_path.to_string_lossy().into_owned(),
    )
    .expect("export page");
    assert!(
        blue_count(&decode_png_bytes(
            &fs::read(&export_path).expect("read export")
        )) > 0
    );

    set_layer_visible(document_id.clone(), BLUE_LAYER, false).expect("hide 蓝色图层");

    let thumbnail = render_thumbnail(document_id.clone(), 0, 200).expect("thumbnail");
    assert_eq!(
        blue_count(&decode_png_base64(&thumbnail.png_base64)),
        0,
        "the thumbnail drops the hidden layer"
    );
    let region =
        render_page_region(document_id.clone(), 0, 1.0, 280, 170, 240, 140).expect("page region");
    assert_eq!(
        blue_count(&decode_png_base64(&region.png_base64)),
        0,
        "the region drops the hidden layer"
    );
    export_page(
        document_id.clone(),
        0,
        PageImageFormat::Png,
        72,
        export_path.to_string_lossy().into_owned(),
    )
    .expect("export page");
    assert_eq!(
        blue_count(&decode_png_bytes(
            &fs::read(&export_path).expect("read export")
        )),
        0,
        "the exported page drops the hidden layer"
    );
    assert!(red_count(&render_pixels(&document_id, 0)) > 0);

    close_document(document_id).expect("close document");
}

#[test]
fn layers_survive_an_encrypted_output_and_an_encrypted_source_edit() {
    let temp = TempDir::new("layers-encrypted");
    let encrypted = temp.path().join("encrypted.pdf");
    let second = temp.path().join("second.pdf");
    let document_id = open_layers();

    set_layer_visible(document_id.clone(), GREEN_LAYER, false).expect("hide 绿色图层");
    save_document_with_progress(
        &document_id,
        SaveDocumentRequest {
            save_as_path: Some(encrypted.to_string_lossy().into_owned()),
            password: Some("secret".to_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
        },
        &mut |_| {},
    )
    .expect("save encrypted output");
    close_document(document_id).expect("close live document");

    assert!(lopdf::Document::load(&encrypted)
        .expect("parse encrypted output")
        .is_encrypted());
    let reopened = open_document(
        encrypted.to_string_lossy().into_owned(),
        Some("secret".to_owned()),
    )
    .expect("reopen encrypted output");
    let reopened_id = reopened.document_id;
    let layers = list_layers(reopened_id.clone())
        .expect("list encrypted layers")
        .layers;
    assert_eq!(layers.len(), 3);
    assert!(!layers[1].visible);
    assert_eq!(green_count(&render_pixels(&reopened_id, 1)), 0);

    // Editing and saving from an encrypted Source exercises the decrypted
    // serialization path with a layer state change.
    set_layer_visible(reopened_id.clone(), RED_LAYER, false).expect("hide 红色图层 on reopen");
    save_document_with_progress(
        &reopened_id,
        SaveDocumentRequest {
            save_as_path: Some(second.to_string_lossy().into_owned()),
            password: None,
            confirm_unencrypted_output: true,
            confirm_signature_invalidation: false,
        },
        &mut |_| {},
    )
    .expect("save unencrypted output from an encrypted source");
    close_document(reopened_id).expect("close reopened");

    let reopened_second =
        open_document(second.to_string_lossy().into_owned(), None).expect("reopen second output");
    let second_id = reopened_second.document_id;
    let layers = list_layers(second_id.clone())
        .expect("list second output")
        .layers;
    assert!(!layers[0].visible);
    assert!(!layers[1].visible);
    assert_eq!(red_count(&render_pixels(&second_id, 0)), 0);
    assert_eq!(green_count(&render_pixels(&second_id, 1)), 0);
    close_document(second_id).expect("close second output");
}
