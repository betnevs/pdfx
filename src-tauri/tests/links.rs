//! Command-boundary tests for hyperlinks.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is parsed independently with lopdf and reopened
//! through the command boundary, so a link "exists and is clickable in other
//! readers" is checked against a fresh parse and PDFium's own action reader.

mod support;

use std::path::Path;

use lopdf::Object;
use pdfx_lib::annotations::TextMarkupRect;
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, CommandErrorCode, TextLayerView,
};
use pdfx_lib::links::{
    create_link, CreateLinkRequest, LinkTarget, LinkView, UpdateLinkTargetRequest,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

/// Display-space box covering the first occurrence of `needle`, built from the
/// same character geometry the frontend selection uses.
fn selection_rect(layer: &TextLayerView, needle: &str) -> TextMarkupRect {
    let byte_offset = layer.text.find(needle).expect("needle in text layer");
    let start = layer.text[..byte_offset].chars().count();
    let count = needle.chars().count();
    let chars = &layer.chars[start..start + count];
    let left = chars
        .iter()
        .map(|character| character.x)
        .fold(f32::INFINITY, f32::min);
    let top = chars
        .iter()
        .map(|character| character.y)
        .fold(f32::INFINITY, f32::min);
    let right = chars
        .iter()
        .map(|character| character.x + character.width)
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = chars
        .iter()
        .map(|character| character.y + character.height)
        .fold(f32::NEG_INFINITY, f32::max);
    TextMarkupRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save output");
}

fn page_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    let page_id = *document.get_pages().get(&page_number).expect("page number");
    let page = document
        .get_object(page_id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary");
    let annots = match page.get(b"Annots") {
        Ok(annots) => annots,
        Err(_) => return Vec::new(),
    };
    let array = match annots {
        Object::Reference(id) => document
            .get_object(*id)
            .expect("annots object")
            .as_array()
            .expect("/Annots array"),
        Object::Array(array) => array,
        other => panic!("/Annots is not an array: {other:?}"),
    };
    array
        .iter()
        .map(|object| match object {
            Object::Reference(id) => document
                .get_object(*id)
                .expect("annotation object")
                .as_dict()
                .expect("annotation dictionary")
                .clone(),
            Object::Dictionary(dictionary) => dictionary.clone(),
            other => panic!("annotation is not a dictionary: {other:?}"),
        })
        .collect()
}

fn text_of(object: &Object) -> String {
    match object {
        Object::String(bytes, _) if bytes.starts_with(&[0xFE, 0xFF]) => {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
        other => panic!("not a string: {other:?}"),
    }
}

fn name_of(dictionary: &lopdf::Dictionary, key: &[u8]) -> String {
    String::from_utf8_lossy(
        dictionary
            .get(key)
            .expect("dictionary entry")
            .as_name()
            .expect("name object"),
    )
    .into_owned()
}

fn float_of(object: &Object) -> f32 {
    object.as_float().expect("numeric object")
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= 0.05,
        "{what}: expected {expected}, got {actual}"
    );
}

fn assert_rect_close(actual: &TextMarkupRect, expected: &TextMarkupRect, what: &str) {
    assert_close(actual.x, expected.x, &format!("{what} x"));
    assert_close(actual.y, expected.y, &format!("{what} y"));
    assert_close(actual.width, expected.width, &format!("{what} width"));
    assert_close(actual.height, expected.height, &format!("{what} height"));
}

fn link_annotations(document: &lopdf::Document, page_number: u32) -> Vec<lopdf::Dictionary> {
    page_annotations(document, page_number)
        .into_iter()
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Link")
        })
        .collect()
}

#[test]
fn a_url_link_created_from_selected_text_saves_as_a_native_clickable_link() {
    let dir = TempDir::new("links-url");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    let mutation = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/pdfx".to_owned(),
            },
        },
    )
    .expect("create URL link");

    let created = mutation.link.expect("created link view");
    assert_eq!(created.page_index, 0);
    assert_eq!(
        created.target,
        Some(LinkTarget::Url {
            url: "https://example.com/pdfx".to_owned()
        })
    );
    assert_eq!(created.unsupported, None);
    assert_rect_close(&created.rects[0], &rect, "created link rect");
    assert!(
        created.name.as_deref().is_some_and(|name| !name.is_empty()),
        "a created link must have a stable /NM name"
    );
    assert!(
        mutation.history.dirty,
        "creating a link dirties the document"
    );
    assert!(mutation.history.can_undo);

    let output = dir.path().join("url-link.pdf");
    save_as(&document_id, &output);

    // Independent structure check: the save contains exactly one native /Link
    // annotation with a URI action and the selected region as /Rect.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let links: Vec<_> = annotations
        .iter()
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Link")
        })
        .collect();
    assert_eq!(links.len(), 1, "exactly one /Link annotation");
    let link = links[0];
    assert_eq!(
        text_of(link.get(b"NM").expect("/NM name")),
        created.name.as_deref().unwrap()
    );
    let action = link
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "URI");
    assert_eq!(
        text_of(action.get(b"URI").expect("/URI target")),
        "https://example.com/pdfx"
    );
    assert!(
        link.get(b"Dest").is_err(),
        "an edited link must not keep a stale /Dest"
    );

    let rect_array = link
        .get(b"Rect")
        .expect("/Rect")
        .as_array()
        .expect("/Rect array");
    assert_eq!(rect_array.len(), 4);
    let page_height = 792.0f32;
    assert_close(float_of(&rect_array[0]), rect.x, "saved /Rect left");
    assert_close(
        float_of(&rect_array[1]),
        page_height - (rect.y + rect.height),
        "saved /Rect bottom",
    );
    assert_close(
        float_of(&rect_array[2]),
        rect.x + rect.width,
        "saved /Rect right",
    );
    assert_close(
        float_of(&rect_array[3]),
        page_height - rect.y,
        "saved /Rect top",
    );

    let quad_points = link
        .get(b"QuadPoints")
        .expect("/QuadPoints")
        .as_array()
        .expect("/QuadPoints array");
    assert_eq!(quad_points.len(), 8, "one box is one quadrilateral");
    let expected_quad = [
        (rect.x, page_height - (rect.y + rect.height)),
        (rect.x + rect.width, page_height - (rect.y + rect.height)),
        (rect.x + rect.width, page_height - rect.y),
        (rect.x, page_height - rect.y),
    ];
    for (corner, (expected_x, expected_y)) in expected_quad.iter().enumerate() {
        assert_close(
            float_of(&quad_points[corner * 2]),
            *expected_x,
            "saved quad x",
        );
        assert_close(
            float_of(&quad_points[corner * 2 + 1]),
            *expected_y,
            "saved quad y",
        );
    }

    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1, "the saved link reopens");
    assert_eq!(
        listed[0].target,
        Some(LinkTarget::Url {
            url: "https://example.com/pdfx".to_owned()
        })
    );
    assert_rect_close(&listed[0].rects[0], &rect, "reopened link rect");
    assert_eq!(listed[0].name, created.name);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn an_internal_page_link_saves_as_a_goto_destination_and_reopens() {
    let dir = TempDir::new("links-page");
    let pdf = synthetic::two_page_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    let mutation = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Page { page_index: 1 },
        },
    )
    .expect("create Page link");

    let created = mutation.link.expect("created link view");
    assert_eq!(created.target, Some(LinkTarget::Page { page_index: 1 }));

    let output = dir.path().join("page-link.pdf");
    save_as(&document_id, &output);

    // Independent structure check: the placeholder PDFium action must have been
    // replaced by a real in-document destination naming the second page object.
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let second_page_object = *parsed.get_pages().get(&2).expect("page 2 object id");
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 1, "exactly one /Link annotation");
    let link = &links[0];
    assert_eq!(
        text_of(link.get(b"NM").expect("/NM name")),
        created.name.as_deref().unwrap()
    );
    assert!(
        link.get(b"Dest").is_err(),
        "a GoTo action must not carry a stale direct /Dest"
    );
    let action = link
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "GoTo");
    assert!(
        action.get(b"URI").is_err(),
        "an internal link must not keep the placeholder URI"
    );
    let destination = action
        .get(b"D")
        .expect("/D destination")
        .as_array()
        .expect("/D array");
    assert_eq!(destination.len(), 5, "[page /XYZ left top zoom]");
    assert_eq!(
        destination[0].as_reference().expect("page reference"),
        second_page_object,
        "the destination names the link's target page"
    );
    assert_eq!(
        destination[1].as_name().expect("destination type"),
        b"XYZ",
        "an XYZ destination keeps the reader's current zoom and position"
    );
    for value in &destination[2..] {
        assert!(
            matches!(value, Object::Null),
            "XYZ coordinates are left to the reader: {value:?}"
        );
    }

    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1, "the saved page link reopens");
    assert_eq!(listed[0].target, Some(LinkTarget::Page { page_index: 1 }));
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn page_links_follow_logical_page_numbers_after_reordering() {
    let dir = TempDir::new("links-page-reorder");
    let pdf = synthetic::page_operations_pdf();
    let document_id = open_synthetic(&pdf);

    // Move the first page to the last slot so logical and physical indexes
    // differ. Logical page 0 is then the physical page carrying "Page two"
    // (object 6), and logical page 3 is the original page one (object 4).
    pdfx_lib::page_ops::move_page(document_id.clone(), 0, 2).expect("move page");
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    assert!(
        layer.text.contains("Page two"),
        "logical page 0 is the moved-to-slot page: {}",
        layer.text
    );
    let rect = selection_rect(&layer, "Page");

    let created = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Page { page_index: 1 },
        },
    )
    .expect("create Page link")
    .link
    .expect("created link");
    assert_eq!(created.page_index, 0, "the host page is reported logically");
    assert_eq!(created.target, Some(LinkTarget::Page { page_index: 1 }));
    let created_name = created.name.clone().expect("created link name");

    // Retargeting speaks logical page numbers too: slot 2 currently shows the
    // original page one.
    let updated = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: created.id,
            target: LinkTarget::Page { page_index: 2 },
        },
    )
    .expect("retarget the link")
    .link
    .expect("updated link");
    assert_eq!(updated.page_index, 0);
    assert_eq!(updated.target, Some(LinkTarget::Page { page_index: 2 }));

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    let ours = listed
        .iter()
        .find(|link| link.name.as_deref() == Some(created_name.as_str()))
        .expect("the created link is listed");
    assert_eq!(ours.page_index, 0);
    assert_eq!(ours.target, Some(LinkTarget::Page { page_index: 2 }));

    let output = dir.path().join("reordered-page-link.pdf");
    save_as(&document_id, &output);

    // The saved page tree is the logical arrangement: the link hangs off the
    // first saved page (object 6) and its destination names the third saved
    // page, which is the old page-one object (4).
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let target_page = *parsed.get_pages().get(&3).expect("saved page 3 object");
    let links = link_annotations(&parsed, 1);
    let link = links
        .iter()
        .find(|link| text_of(link.get(b"NM").expect("/NM name")) == created_name)
        .expect("saved link");
    let action = link
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "GoTo");
    let destination = action
        .get(b"D")
        .expect("/D destination")
        .as_array()
        .expect("/D array");
    assert_eq!(
        destination[0].as_reference().expect("page reference"),
        target_page,
        "the destination follows the logical target page after reordering"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    let ours = listed
        .iter()
        .find(|link| link.name.as_deref() == Some(created_name.as_str()))
        .expect("the reordered link reopens");
    assert_eq!(ours.page_index, 0);
    assert_eq!(ours.target, Some(LinkTarget::Page { page_index: 2 }));
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn a_link_target_edit_saves_and_is_undoable_and_redoable() {
    let dir = TempDir::new("links-edit");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    let created = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/one".to_owned(),
            },
        },
    )
    .expect("create URL link")
    .link
    .expect("created link view");

    let edited = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: created.id.clone(),
            target: LinkTarget::Url {
                url: "https://example.com/two".to_owned(),
            },
        },
    )
    .expect("edit link target");
    let edited_link = edited.link.expect("edited link view");
    assert_eq!(edited_link.name, created.name, "the stable /NM survives");
    assert_eq!(
        edited_link.target,
        Some(LinkTarget::Url {
            url: "https://example.com/two".to_owned()
        })
    );
    assert!(edited.history.can_undo);

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(
        listed.len(),
        1,
        "editing replaces the link, never duplicates it"
    );

    let output = dir.path().join("edited-link.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 1, "exactly one /Link annotation");
    let action = links[0]
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "URI");
    assert_eq!(
        text_of(action.get(b"URI").expect("/URI target")),
        "https://example.com/two"
    );

    undo_text_edit(document_id.clone()).expect("undo the target edit");
    let after_undo = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(after_undo.len(), 1);
    assert_eq!(
        after_undo[0].target,
        Some(LinkTarget::Url {
            url: "https://example.com/one".to_owned()
        }),
        "undo restores the previous target"
    );

    redo_text_edit(document_id.clone()).expect("redo the target edit");
    let after_redo = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(after_redo.len(), 1);
    assert_eq!(
        after_redo[0].target,
        Some(LinkTarget::Url {
            url: "https://example.com/two".to_owned()
        }),
        "redo reapplies the edited target"
    );

    close_document(document_id).expect("close source");
}

#[test]
fn deleting_a_foreign_link_is_undoable_and_preserves_other_annotations() {
    let dir = TempDir::new("links-delete");
    let pdf = synthetic::foreign_links_pdf();
    let document_id = open_synthetic(&pdf);

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 2, "a URI link and a JavaScript link");
    let foreign = listed
        .iter()
        .find(|link| {
            link.target
                == Some(LinkTarget::Url {
                    url: "https://foreign.example/one".to_owned(),
                })
        })
        .expect("the foreign URI link")
        .clone();
    let foreign_rects = foreign.rects.clone();
    assert_eq!(foreign.name, None, "the fixture link has no /NM");

    let deleted =
        pdfx_lib::links::delete_link(document_id.clone(), foreign.id.clone()).expect("delete link");
    assert!(deleted.link.is_none());
    assert!(deleted.history.can_undo);

    let remaining = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].unsupported,
        Some(pdfx_lib::links::LinkUnsupportedReason::Action),
        "the unsupported link is still listed, untouched"
    );

    let output = dir.path().join("foreign-delete.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let links: Vec<_> = annotations
        .iter()
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Link")
        })
        .collect();
    assert_eq!(links.len(), 1, "only the unsupported link is left");
    let action = links[0]
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(
        name_of(action, b"S"),
        "JavaScript",
        "a foreign action PDFX cannot represent must stay untouched"
    );
    assert!(
        annotations.iter().any(|annotation| {
            annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|name| name == b"Square")
        }),
        "non-link annotations survive a link deletion"
    );

    undo_text_edit(document_id.clone()).expect("undo the deletion");
    let restored = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(restored.len(), 2, "undo restores the deleted link");
    let uri = restored
        .iter()
        .find(|link| {
            link.target
                == Some(LinkTarget::Url {
                    url: "https://foreign.example/one".to_owned(),
                })
        })
        .expect("the URI link is back");
    assert_eq!(uri.rects.len(), foreign_rects.len());
    for (actual, expected) in uri.rects.iter().zip(&foreign_rects) {
        assert_rect_close(actual, expected, "restored foreign link rect");
    }

    redo_text_edit(document_id.clone()).expect("redo the deletion");
    let after_redo = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(after_redo.len(), 1, "redo removes the link again");

    close_document(document_id).expect("close source");
}

#[test]
fn an_unsupported_foreign_link_is_refused_and_left_untouched() {
    let dir = TempDir::new("links-unsupported");
    let pdf = synthetic::foreign_links_pdf();
    let document_id = open_synthetic(&pdf);

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    let unsupported = listed
        .iter()
        .find(|link| link.unsupported == Some(pdfx_lib::links::LinkUnsupportedReason::Action))
        .expect("the JavaScript link")
        .clone();
    assert_eq!(unsupported.target, None);

    let update_error = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: unsupported.id.clone(),
            target: LinkTarget::Url {
                url: "https://example.com/replacement".to_owned(),
            },
        },
    )
    .expect_err("an unsupported action must be refused");
    assert_eq!(
        update_error.code,
        CommandErrorCode::UnsupportedLink,
        "the frontend turns this into a Chinese explanation"
    );
    let delete_error = pdfx_lib::links::delete_link(document_id.clone(), unsupported.id.clone())
        .expect_err("an unsupported action must be refused for deletion too");
    assert_eq!(delete_error.code, CommandErrorCode::UnsupportedLink);

    let after = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(after.len(), 2, "the refused link is still present");
    let untouched = after
        .iter()
        .find(|link| link.unsupported == Some(pdfx_lib::links::LinkUnsupportedReason::Action))
        .expect("the JavaScript link is still listed");
    assert_eq!(untouched.name, None, "refusal must not assign a /NM");
    assert_rect_close(
        &untouched.rects[0],
        &unsupported.rects[0],
        "refused link rect",
    );

    let history = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert!(!history.can_undo, "a refused edit must not enter history");
    assert!(!history.dirty, "a refused edit must not dirty the document");

    let output = dir.path().join("unsupported-untouched.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 2);
    let javascript: Vec<_> = links
        .iter()
        .filter(|link| {
            link.get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"S").ok())
                .and_then(|object| object.as_name().ok())
                .is_some_and(|name| name == b"JavaScript")
        })
        .collect();
    assert_eq!(javascript.len(), 1);
    assert!(
        javascript[0].get(b"NM").is_err(),
        "the refused link must not gain PDFX metadata"
    );

    close_document(document_id).expect("close source");
}

fn link_at_x(links: &[LinkView], x: f32) -> LinkView {
    links
        .iter()
        .find(|link| (link.rects[0].x - x).abs() <= 0.05)
        .unwrap_or_else(|| panic!("no link at x={x}: {links:?}"))
        .clone()
}

fn saved_link_at_x<'a>(links: &'a [lopdf::Dictionary], x: f32) -> &'a lopdf::Dictionary {
    links
        .iter()
        .find(|link| {
            link.get(b"Rect")
                .ok()
                .and_then(|rect| rect.as_array().ok())
                .and_then(|rect| rect.first())
                .is_some_and(|left| (float_of(left) - x).abs() <= 0.05)
        })
        .unwrap_or_else(|| panic!("no saved link at x={x}"))
}

#[test]
fn foreign_goto_and_dest_links_are_read_and_editable() {
    let dir = TempDir::new("links-dest");
    let pdf = synthetic::destination_links_pdf();
    let document_id = open_synthetic(&pdf);

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 4, "all four foreign links are listed");
    for x in [72.0, 200.0, 320.0] {
        let link = link_at_x(&listed, x);
        assert_eq!(link.unsupported, None, "link at x={x} is supported");
        assert_eq!(
            link.target,
            Some(LinkTarget::Page { page_index: 1 }),
            "link at x={x} resolves to page 2"
        );
    }
    let missing = link_at_x(&listed, 440.0);
    assert_eq!(missing.target, None);
    assert_eq!(
        missing.unsupported,
        Some(pdfx_lib::links::LinkUnsupportedReason::Destination),
        "an unresolvable named destination is reported, not guessed"
    );

    let goto = link_at_x(&listed, 72.0);
    let edited = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: goto.id,
            target: LinkTarget::Url {
                url: "https://example.com/from-goto".to_owned(),
            },
        },
    )
    .expect("edit a GoTo-action link");
    let edited_link = edited.link.expect("edited link");
    assert_eq!(
        edited_link.target,
        Some(LinkTarget::Url {
            url: "https://example.com/from-goto".to_owned()
        })
    );
    assert!(
        edited_link.name.is_some(),
        "editing an unnamed foreign link assigns the stable /NM identity"
    );

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    let direct = link_at_x(&listed, 200.0);
    pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: direct.id,
            target: LinkTarget::Url {
                url: "https://example.com/from-dest".to_owned(),
            },
        },
    )
    .expect("edit a direct /Dest link");

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    let named = link_at_x(&listed, 320.0);
    pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: named.id,
            target: LinkTarget::Page { page_index: 0 },
        },
    )
    .expect("edit a named-destination link");

    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(
        link_at_x(&listed, 440.0).unsupported,
        Some(pdfx_lib::links::LinkUnsupportedReason::Destination),
        "the unresolved destination survives the other edits"
    );

    let output = dir.path().join("dest-edited.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let first_page_object = *parsed.get_pages().get(&1).expect("page 1 object id");
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 4);

    let from_goto = saved_link_at_x(&links, 72.0);
    assert!(
        from_goto.get(b"Dest").is_err(),
        "the edited GoTo link drops its old destination"
    );
    let action = from_goto
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "URI");
    assert_eq!(
        text_of(action.get(b"URI").expect("/URI")),
        "https://example.com/from-goto"
    );

    let from_dest = saved_link_at_x(&links, 200.0);
    assert!(
        from_dest.get(b"Dest").is_err(),
        "the edited direct-destination link drops /Dest"
    );
    let action = from_dest
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "URI");
    assert_eq!(
        text_of(action.get(b"URI").expect("/URI")),
        "https://example.com/from-dest"
    );

    let to_first_page = saved_link_at_x(&links, 320.0);
    assert!(
        to_first_page.get(b"Dest").is_err(),
        "the named destination is replaced by the explicit page target"
    );
    let action = to_first_page
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "GoTo");
    let destination = action
        .get(b"D")
        .expect("/D destination")
        .as_array()
        .expect("/D array");
    assert_eq!(
        destination[0].as_reference().expect("page reference"),
        first_page_object
    );

    let unresolved = saved_link_at_x(&links, 440.0);
    assert!(
        unresolved.get(b"A").is_err(),
        "the refused link keeps its original form"
    );
    assert_eq!(
        text_of(unresolved.get(b"Dest").expect("/Dest")),
        "missing-destination"
    );
    assert!(
        unresolved.get(b"NM").is_err(),
        "the refused link was never named or recreated"
    );

    close_document(document_id).expect("close source");
}

#[test]
fn link_rect_follows_page_rotation_and_crop_box() {
    let dir = TempDir::new("links-rotated");
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = TextMarkupRect {
        x: 24.0,
        y: 40.0,
        width: 90.0,
        height: 24.0,
    };

    for page_index in 0..3 {
        let mutation = create_link(
            document_id.clone(),
            CreateLinkRequest {
                page_index,
                rects: vec![rect],
                target: LinkTarget::Page { page_index: 0 },
            },
        )
        .expect("create link on a rotated page");
        let view = mutation.link.expect("created link view");
        assert_rect_close(&view.rects[0], &rect, "display-space round trip");
    }

    let output = dir.path().join("rotated-links.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");

    // Hand-derived from the CropBox [50 80 450 680] and each page's /Rotate:
    // display (24,40)-(114,64) maps as follows in PDF user space.
    let expected: [([f32; 4], [[f32; 2]; 4]); 3] = [
        // 90 degrees: (left + y, bottom + x)
        (
            [90.0, 104.0, 114.0, 194.0],
            [[114.0, 104.0], [114.0, 194.0], [90.0, 194.0], [90.0, 104.0]],
        ),
        // 180 degrees: (left + width - x, bottom + y)
        (
            [336.0, 120.0, 426.0, 144.0],
            [
                [426.0, 144.0],
                [336.0, 144.0],
                [336.0, 120.0],
                [426.0, 120.0],
            ],
        ),
        // 270 degrees: (left + width - y, bottom + height - x)
        (
            [386.0, 566.0, 410.0, 656.0],
            [
                [386.0, 656.0],
                [386.0, 566.0],
                [410.0, 566.0],
                [410.0, 656.0],
            ],
        ),
    ];

    for (page_index, (rect_values, quad_corners)) in expected.iter().enumerate() {
        let links = link_annotations(&parsed, (page_index + 1) as u32);
        assert_eq!(links.len(), 1, "page {} has one link", page_index + 1);
        let link = &links[0];
        let saved_rect = link
            .get(b"Rect")
            .expect("/Rect")
            .as_array()
            .expect("/Rect array");
        assert_eq!(saved_rect.len(), 4);
        for (index, expected_value) in rect_values.iter().enumerate() {
            assert_close(
                float_of(&saved_rect[index]),
                *expected_value,
                &format!("page {} /Rect[{index}]", page_index + 1),
            );
        }
        let quads = link
            .get(b"QuadPoints")
            .expect("/QuadPoints")
            .as_array()
            .expect("/QuadPoints array");
        assert_eq!(quads.len(), 8);
        for (corner, corner_values) in quad_corners.iter().enumerate() {
            let (expected_x, expected_y) = (corner_values[0], corner_values[1]);
            assert_close(
                float_of(&quads[corner * 2]),
                expected_x,
                &format!("page {} quad {corner} x", page_index + 1),
            );
            assert_close(
                float_of(&quads[corner * 2 + 1]),
                expected_y,
                &format!("page {} quad {corner} y", page_index + 1),
            );
        }
    }

    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 3);
    for link in &listed {
        assert_eq!(link.rects.len(), 1);
        assert_rect_close(
            &link.rects[0],
            &rect,
            &format!("reopened page {} rect", link.page_index),
        );
    }
    close_document(reopened.document_id).expect("close reopened");
}

fn fixture_path(id: &str) -> String {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture '{id}' is missing from the manifest"));
    support::fixtures::fixture_path(fixture)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn read_only_documents_refuse_link_changes_without_touching_history() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;
    let listed_before = pdfx_lib::links::list_links(document_id.clone()).expect("listing");
    let history_before = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");

    let rect = TextMarkupRect {
        x: 10.0,
        y: 10.0,
        width: 100.0,
        height: 20.0,
    };
    let create_error = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/denied".to_owned(),
            },
        },
    )
    .expect_err("link creation is denied");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);

    let update_error = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: "0:0".to_owned(),
            target: LinkTarget::Url {
                url: "https://example.com/denied".to_owned(),
            },
        },
    )
    .expect_err("link edits are denied");
    assert_eq!(update_error.code, CommandErrorCode::PermissionDenied);

    let delete_error = pdfx_lib::links::delete_link(document_id.clone(), "0:0".to_owned())
        .expect_err("link deletion is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);

    let listed_after = pdfx_lib::links::list_links(document_id.clone()).expect("listing");
    assert_eq!(
        listed_after.len(),
        listed_before.len(),
        "a denied change must not alter the link list"
    );
    let history_after = pdfx_lib::text_edit::edit_history(document_id.clone()).expect("history");
    assert_eq!(history_after.can_undo, history_before.can_undo);
    assert_eq!(history_after.can_redo, history_before.can_redo);
    assert_eq!(history_after.dirty, history_before.dirty);

    close_document(document_id).expect("close document");
}

fn save_changes(document_id: &str) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save changes");
}

#[test]
fn repeated_saves_do_not_duplicate_links_and_never_touch_the_source() {
    let dir = TempDir::new("links-repeat");
    let pdf = synthetic::two_page_pdf();
    let source_path = pdf.path_string();
    let source_bytes = std::fs::read(&source_path).expect("read source bytes");
    let source_modified = std::fs::metadata(&source_path)
        .expect("source metadata")
        .modified()
        .expect("source mtime");

    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");
    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/repeat".to_owned(),
            },
        },
    )
    .expect("create URL link");
    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Page { page_index: 1 },
        },
    )
    .expect("create page link");

    let output = dir.path().join("repeat.pdf");
    save_as(&document_id, &output);
    save_changes(&document_id);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let links = link_annotations(&parsed, 1);
    assert_eq!(
        links.len(),
        2,
        "repeat saves must not duplicate annotations"
    );
    assert_eq!(
        link_annotations(&parsed, 2).len(),
        0,
        "nothing is written to the target page"
    );
    let uri_count = links
        .iter()
        .filter(|link| {
            link.get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"S").ok())
                .and_then(|object| object.as_name().ok())
                == Some(b"URI".as_slice())
        })
        .count();
    let goto_count = links
        .iter()
        .filter(|link| {
            link.get(b"A")
                .ok()
                .and_then(|action| action.as_dict().ok())
                .and_then(|action| action.get(b"S").ok())
                .and_then(|object| object.as_name().ok())
                == Some(b"GoTo".as_slice())
        })
        .count();
    assert_eq!((uri_count, goto_count), (1, 1));

    assert_eq!(
        std::fs::read(&source_path).expect("read source bytes"),
        source_bytes,
        "the Source Document is byte-identical after two saves"
    );
    assert_eq!(
        std::fs::metadata(&source_path)
            .expect("source metadata")
            .modified()
            .expect("source mtime"),
        source_modified,
        "the Source Document's mtime is unchanged"
    );

    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 2);
    let mut targets: Vec<_> = listed.into_iter().filter_map(|link| link.target).collect();
    targets.sort_by_key(|target| match target {
        LinkTarget::Url { .. } => 0,
        LinkTarget::Page { .. } => 1,
    });
    assert_eq!(
        targets,
        vec![
            LinkTarget::Url {
                url: "https://example.com/repeat".to_owned()
            },
            LinkTarget::Page { page_index: 1 }
        ]
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn saving_a_link_preserves_existing_form_fields() {
    let dir = TempDir::new("links-forms");
    let document_id = open_document(fixture_path("form-acroform"), None)
        .expect("open the form fixture")
        .document_id;
    let fields_before = pdfx_lib::form::form_fields(document_id.clone()).expect("form fields");
    assert!(!fields_before.is_empty(), "the fixture has form fields");

    let rect = TextMarkupRect {
        x: 10.0,
        y: 10.0,
        width: 100.0,
        height: 20.0,
    };
    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/forms".to_owned(),
            },
        },
    )
    .expect("create a link beside the form");

    let output = dir.path().join("form-and-link.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let fields_after =
        pdfx_lib::form::form_fields(reopened.document_id.clone()).expect("reopened form fields");
    assert_eq!(
        fields_after, fields_before,
        "adding a link must not alter the AcroForm fields"
    );
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].target,
        Some(LinkTarget::Url {
            url: "https://example.com/forms".to_owned()
        })
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn link_creation_is_undoable_and_redoable() {
    let dir = TempDir::new("links-create-history");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/history".to_owned(),
            },
        },
    )
    .expect("create URL link");
    assert_eq!(
        pdfx_lib::links::list_links(document_id.clone())
            .expect("list after create")
            .len(),
        1
    );

    undo_text_edit(document_id.clone()).expect("undo creation");
    assert!(
        pdfx_lib::links::list_links(document_id.clone())
            .expect("list after undo")
            .is_empty(),
        "undoing the creation removes the link"
    );

    redo_text_edit(document_id.clone()).expect("redo creation");
    let restored = pdfx_lib::links::list_links(document_id.clone()).expect("list after redo");
    assert_eq!(restored.len(), 1, "redo recreates the link exactly once");
    assert_eq!(
        restored[0].target,
        Some(LinkTarget::Url {
            url: "https://example.com/history".to_owned()
        })
    );
    assert_rect_close(&restored[0].rects[0], &rect, "redone link rect");

    let output = dir.path().join("created-again.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_eq!(link_annotations(&parsed, 1).len(), 1);

    close_document(document_id).expect("close source");
}

#[test]
fn a_multi_line_selection_keeps_one_quad_per_line() {
    let dir = TempDir::new("links-multiline");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rects = vec![
        TextMarkupRect {
            x: 72.0,
            y: 100.0,
            width: 120.0,
            height: 18.0,
        },
        TextMarkupRect {
            x: 72.0,
            y: 130.0,
            width: 60.0,
            height: 18.0,
        },
    ];

    let mutation = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: rects.clone(),
            target: LinkTarget::Url {
                url: "https://example.com/multiline".to_owned(),
            },
        },
    )
    .expect("create a multi-line link");
    let view = mutation.link.expect("created link view");
    assert_eq!(view.rects.len(), 2);
    for (actual, expected) in view.rects.iter().zip(&rects) {
        assert_rect_close(actual, expected, "multi-line view rect");
    }

    let output = dir.path().join("multiline-link.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 1);
    let quads = links[0]
        .get(b"QuadPoints")
        .expect("/QuadPoints")
        .as_array()
        .expect("/QuadPoints array");
    assert_eq!(quads.len(), 16, "two lines are two quadrilaterals");
    let saved_rect = links[0]
        .get(b"Rect")
        .expect("/Rect")
        .as_array()
        .expect("/Rect array");
    let page_height = 792.0f32;
    assert_close(float_of(&saved_rect[0]), 72.0, "/Rect left");
    // The bounding box spans both lines: from the bottom of the second
    // (display y=148) to the top of the first (display y=100).
    assert_close(
        float_of(&saved_rect[1]),
        page_height - (130.0 + 18.0),
        "/Rect bottom",
    );
    assert_close(float_of(&saved_rect[2]), 192.0, "/Rect right");
    assert_close(float_of(&saved_rect[3]), page_height - 100.0, "/Rect top");

    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].rects.len(), 2);
    for (actual, expected) in listed[0].rects.iter().zip(&rects) {
        assert_rect_close(actual, expected, "reopened multi-line rect");
    }
    close_document(reopened.document_id).expect("close reopened");
}

/// Counts the cross-reference sections in saved PDF bytes. Each incremental
/// revision appends one, so the delta measures how many writers touched the
/// output after the original document.
fn startxref_sections(bytes: &[u8]) -> usize {
    const NEEDLE: &[u8] = b"startxref";
    bytes
        .windows(NEEDLE.len())
        .filter(|window| *window == NEEDLE)
        .count()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_link_to_an_inserted_blank_page_saves_and_reopens() {
    let dir = TempDir::new("links-insert-target");
    let pdf = synthetic::two_page_pdf();
    let source_sections =
        startxref_sections(&std::fs::read(pdf.path_string()).expect("source bytes"));
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    // Insert the blank page at logical slot 1, between the link's page and the
    // original second page.
    pdfx_lib::page_ops::insert_blank_page(
        document_id.clone(),
        1,
        pdfx_lib::page_ops::BlankPageSize {
            width: 300.0,
            height: 400.0,
        },
    )
    .expect("insert a blank page");

    let created = create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Page { page_index: 1 },
        },
    )
    .expect("create a link to the inserted page")
    .link
    .expect("created link view");
    assert_eq!(created.target, Some(LinkTarget::Page { page_index: 1 }));
    let created_name = created.name.clone().expect("created link name");

    let output = dir.path().join("inserted-target-link.pdf");
    save_as(&document_id, &output);
    let output_bytes = std::fs::read(&output).expect("read output bytes");

    // The inserted page forces the lopdf page-tree edit. That edit and the
    // page-link materialization must still share exactly one appended
    // revision after PDFium's incremental one.
    assert_eq!(
        startxref_sections(&output_bytes),
        source_sections + 2,
        "one PDFium revision plus exactly one lopdf revision"
    );

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_eq!(parsed.get_pages().len(), 3, "the inserted page is saved");
    let blank_id = *parsed.get_pages().get(&2).expect("inserted page is second");
    let blank = parsed
        .get_dictionary(blank_id)
        .expect("inserted page dictionary");
    let media_box = blank
        .get(b"MediaBox")
        .expect("/MediaBox")
        .as_array()
        .expect("/MediaBox array");
    assert_close(float_of(&media_box[2]), 300.0, "inserted page width");
    assert_close(float_of(&media_box[3]), 400.0, "inserted page height");

    let links = link_annotations(&parsed, 1);
    let link = links
        .iter()
        .find(|link| text_of(link.get(b"NM").expect("/NM name")) == created_name)
        .expect("the saved link");
    assert!(
        link.get(b"PDFXLinkTarget").is_err(),
        "the materialized annotation no longer carries the private key"
    );
    let action = link
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "GoTo");
    let destination = action
        .get(b"D")
        .expect("/D destination")
        .as_array()
        .expect("/D array");
    assert_eq!(
        destination[0].as_reference().expect("page reference"),
        blank_id,
        "the destination names the inserted blank page"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    let link = listed
        .iter()
        .find(|link| link.name.as_deref() == Some(created_name.as_str()))
        .expect("the link to the inserted page reopens");
    assert_eq!(link.page_index, 0);
    assert_eq!(link.target, Some(LinkTarget::Page { page_index: 1 }));
    assert_rect_close(&link.rects[0], &rect, "reopened inserted-target rect");
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn deleting_a_link_target_page_reports_the_link_as_unresolved() {
    let dir = TempDir::new("links-deleted-target");
    let pdf = synthetic::two_page_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Page { page_index: 1 },
        },
    )
    .expect("create a link to the second page");

    pdfx_lib::page_ops::delete_page(document_id.clone(), 1).expect("delete the target page");
    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1, "the link itself survives on the kept page");
    assert_eq!(listed[0].page_index, 0);
    assert_eq!(
        listed[0].target, None,
        "a deleted target must never be reported as page 1"
    );
    assert_eq!(
        listed[0].unsupported,
        Some(pdfx_lib::links::LinkUnsupportedReason::Destination),
        "the overlay explains that the destination cannot be resolved"
    );

    // Undoing the page deletion restores the target page's logical slot, so
    // the same annotation speaks a normal page target again.
    undo_text_edit(document_id.clone()).expect("undo the page deletion");
    let restored = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(restored[0].target, Some(LinkTarget::Page { page_index: 1 }));

    redo_text_edit(document_id.clone()).expect("redo the page deletion");
    let after_redo = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(after_redo[0].target, None);

    let output = dir.path().join("deleted-target-link.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert_eq!(parsed.get_pages().len(), 1, "the deleted page is not saved");
    let saved_page_ids: Vec<lopdf::ObjectId> = parsed.get_pages().values().copied().collect();
    let links = link_annotations(&parsed, 1);
    assert_eq!(links.len(), 1);
    let action = links[0]
        .get(b"A")
        .expect("/A action")
        .as_dict()
        .expect("action dictionary");
    assert_eq!(name_of(action, b"S"), "GoTo");
    assert!(
        links[0].get(b"PDFXLinkTarget").is_err(),
        "the private key is still removed when its page is gone"
    );
    let destination = action
        .get(b"D")
        .expect("/D destination")
        .as_array()
        .expect("/D array");
    let destination_page = destination[0].as_reference().expect("page reference");
    assert!(
        !saved_page_ids.contains(&destination_page),
        "ticket 26 leaves the destination dangling rather than rewriting it"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let listed = pdfx_lib::links::list_links(reopened.document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 1, "the dangling link is still listed");
    assert_eq!(
        listed[0].target, None,
        "a dangling saved destination reopens as unresolved, never as page 1"
    );
    assert_eq!(
        listed[0].unsupported,
        Some(pdfx_lib::links::LinkUnsupportedReason::Destination)
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn link_identity_stays_on_its_own_page_when_another_page_repeats_the_nm() {
    let pdf = synthetic::duplicate_link_names_pdf();
    let document_id = open_synthetic(&pdf);
    let listed = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(listed.len(), 2);
    let page_one = listed
        .iter()
        .find(|link| link.page_index == 0)
        .expect("page-one link")
        .clone();
    let page_two = listed
        .iter()
        .find(|link| link.page_index == 1)
        .expect("page-two link")
        .clone();
    assert_eq!(page_one.name.as_deref(), Some("shared-name"));
    assert_eq!(page_two.name.as_deref(), Some("shared-name"));

    let updated = pdfx_lib::links::update_link_target(
        document_id.clone(),
        UpdateLinkTargetRequest {
            link_id: page_two.id.clone(),
            target: LinkTarget::Url {
                url: "https://example.com/replaced".to_owned(),
            },
        },
    )
    .expect("edit the page-two link")
    .link
    .expect("updated link view");
    assert_eq!(updated.page_index, 1);
    assert_eq!(
        updated.target,
        Some(LinkTarget::Url {
            url: "https://example.com/replaced".to_owned()
        })
    );

    let after = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(
        after.len(),
        2,
        "the page-one link with the same name survives"
    );
    let still_page_one = after
        .iter()
        .find(|link| link.page_index == 0)
        .expect("page-one link is untouched");
    assert_eq!(
        still_page_one.target,
        Some(LinkTarget::Url {
            url: "https://foreign.example/page-one".to_owned()
        }),
        "editing page two must not delete or retarget page one"
    );
    let replaced_page_two = after
        .iter()
        .find(|link| link.page_index == 1)
        .expect("page-two link");
    assert_eq!(
        replaced_page_two.target,
        Some(LinkTarget::Url {
            url: "https://example.com/replaced".to_owned()
        })
    );

    undo_text_edit(document_id.clone()).expect("undo the target edit");
    let restored = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(restored.len(), 2);
    assert_eq!(
        restored
            .iter()
            .find(|link| link.page_index == 1)
            .expect("page-two link")
            .target,
        Some(LinkTarget::Url {
            url: "https://foreign.example/page-two".to_owned()
        }),
        "undo restores the page-two link, not the page-one one"
    );
    assert_eq!(
        restored
            .iter()
            .find(|link| link.page_index == 0)
            .expect("page-one link")
            .target,
        Some(LinkTarget::Url {
            url: "https://foreign.example/page-one".to_owned()
        })
    );

    // Deleting the page-two link must leave the identically named page-one
    // link alone.
    let page_two = restored
        .iter()
        .find(|link| link.page_index == 1)
        .expect("page-two link")
        .clone();
    pdfx_lib::links::delete_link(document_id.clone(), page_two.id)
        .expect("delete the page-two link");
    let remaining = pdfx_lib::links::list_links(document_id.clone()).expect("list links");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].page_index, 0);
    assert_eq!(
        remaining[0].target,
        Some(LinkTarget::Url {
            url: "https://foreign.example/page-one".to_owned()
        })
    );

    undo_text_edit(document_id.clone()).expect("undo the deletion");
    assert_eq!(
        pdfx_lib::links::list_links(document_id.clone())
            .expect("list links")
            .len(),
        2
    );
    close_document(document_id).expect("close source");
}

#[test]
fn a_url_only_link_save_appends_only_the_pdfium_revision() {
    let dir = TempDir::new("links-url-revisions");
    let pdf = synthetic::single_line_pdf();
    let source_sections =
        startxref_sections(&std::fs::read(pdf.path_string()).expect("source bytes"));
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");
    create_link(
        document_id.clone(),
        CreateLinkRequest {
            page_index: 0,
            rects: vec![rect],
            target: LinkTarget::Url {
                url: "https://example.com/revision".to_owned(),
            },
        },
    )
    .expect("create URL link");

    let output = dir.path().join("url-revision.pdf");
    save_as(&document_id, &output);
    let output_bytes = std::fs::read(&output).expect("read output bytes");
    assert_eq!(
        startxref_sections(&output_bytes),
        source_sections + 1,
        "a URL-only change needs no lopdf revision"
    );
    assert!(!contains(&output_bytes, b"PDFXLinkTarget"));

    close_document(document_id).expect("close source");
}
