//! Command-boundary tests for the eraser.
//!
//! Whole-annotation deletion uses the existing `delete_annotation` command the
//! eraser overlay calls after hit-testing. Ink strokes use
//! `erase_ink_stroke`, which rebuilds `/InkList`, `/Rect` and `/AP` from the
//! remaining strokes and records a stroke-granular undo snapshot.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_ink_with_author, create_note_with_author, create_stamp_with_author,
    create_text_markup_with_author, delete_annotation, erase_ink_stroke, list_annotations,
    AnnotationKind, CreateInkRequest, CreateNoteRequest, CreateStampRequest,
    CreateTextMarkupRequest, InkPoint, TextMarkupKind, TextMarkupRect,
};
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::stamps::{StampFrameView, StampKind};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::fixtures;
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn fixture_path(id: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture '{id}' is missing from the manifest"));
    fixtures::fixture_path(fixture)
        .to_string_lossy()
        .into_owned()
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
    let annots = page.get(b"Annots").expect("page has /Annots");
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

fn ink_path_point_counts(document: &lopdf::Document, annotation: &lopdf::Dictionary) -> Vec<usize> {
    let ink_list = annotation
        .get(b"InkList")
        .expect("/InkList")
        .as_array()
        .expect("/InkList array");
    ink_list
        .iter()
        .map(|path| match path {
            Object::Reference(id) => document
                .get_object(*id)
                .expect("ink path")
                .as_array()
                .expect("ink path array")
                .len(),
            object => object.as_array().expect("ink path array").len(),
        })
        .collect()
}

struct DecodedPng {
    width: u32,
    rgba: Vec<u8>,
}

fn render_pixels(document_id: &str, page_index: u32, scale: f32) -> DecodedPng {
    let image = render_page(document_id.to_owned(), page_index, scale).expect("render page");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&image.png_base64)
        .expect("render payload is base64");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buffer).expect("PNG frame");
    buffer.truncate(info.buffer_size());
    DecodedPng {
        width: info.width,
        rgba: buffer,
    }
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

    fn ink_near(&self, x: u32, y: u32, matches: impl Fn((u8, u8, u8)) -> bool) -> bool {
        for dy in y.saturating_sub(2)..=(y + 2) {
            for dx in x.saturating_sub(2)..=(x + 2) {
                if matches(self.pixel(dx, dy)) {
                    return true;
                }
            }
        }
        false
    }
}

fn two_stroke_request() -> CreateInkRequest {
    CreateInkRequest {
        page_index: 0,
        color: "#c62828".to_owned(),
        opacity: 100,
        line_width: 6.0,
        strokes: vec![
            vec![
                InkPoint { x: 100.0, y: 300.0 },
                InkPoint { x: 160.0, y: 300.0 },
            ],
            vec![
                InkPoint { x: 100.0, y: 360.0 },
                InkPoint { x: 160.0, y: 360.0 },
            ],
        ],
    }
}

#[test]
fn erasing_one_ink_stroke_rebuilds_inklist_bounds_and_appearance() {
    let dir = TempDir::new("eraser-ink-stroke");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created =
        create_ink_with_author(document_id.clone(), "作者".to_owned(), two_stroke_request())
            .expect("create ink");
    let created_view = created.annotation.expect("ink view");
    let name = created_view.name.clone();
    assert_eq!(created_view.strokes.len(), 2);

    let mutated = erase_ink_stroke(document_id.clone(), name.clone(), 0).expect("erase stroke");
    let view = mutated.annotation.expect("view after erase");
    assert_eq!(view.kind, AnnotationKind::Ink);
    assert_eq!(view.strokes.len(), 1, "only the hit stroke is removed");
    assert_eq!(view.color, "#c62828");
    assert_eq!(view.line_width, Some(6.0));
    assert!(
        view.rects[0].height < created_view.rects[0].height,
        "the ink bounds shrink to the remaining stroke"
    );

    // One gesture, one undo step: both strokes return together.
    undo_text_edit(document_id.clone()).expect("undo erase");
    let restored = list_annotations(document_id.clone()).expect("list undone");
    assert_eq!(restored[0].strokes.len(), 2);
    redo_text_edit(document_id.clone()).expect("redo erase");
    assert_eq!(
        list_annotations(document_id.clone()).expect("list redone")[0]
            .strokes
            .len(),
        1
    );

    let output = dir.path().join("erased.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    assert_eq!(annotations.len(), 1);
    let ink = &annotations[0];
    assert_eq!(ink.get(b"Subtype").unwrap().as_name().unwrap(), b"Ink");
    assert_eq!(
        ink_path_point_counts(&parsed, ink),
        vec![4],
        "the saved /InkList contains only the remaining stroke (two x/y pairs)"
    );
    let rect: Vec<f32> = ink
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_float().unwrap())
        .collect();
    assert!(
        rect[3] - rect[1] < 20.0,
        "the saved /Rect covers only the remaining stroke: {rect:?}"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None).expect("reopen");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let red = |pixel: (u8, u8, u8)| pixel.0 > 140 && pixel.1 < 110 && pixel.2 < 110;
    assert!(
        pixels.ink_near(130, 360, red),
        "the remaining stroke still renders after reopen"
    );
    assert!(
        !pixels.ink_near(130, 300, red),
        "the erased stroke is gone from the saved appearance"
    );
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views[0].strokes.len(), 1);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn erasing_the_last_stroke_turns_into_one_undoable_delete() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created =
        create_ink_with_author(document_id.clone(), "作者".to_owned(), two_stroke_request())
            .expect("create ink");
    let name = created.annotation.expect("ink view").name;

    erase_ink_stroke(document_id.clone(), name.clone(), 1).expect("erase second stroke");
    let last = erase_ink_stroke(document_id.clone(), name.clone(), 0).expect("erase last stroke");
    assert!(last.annotation.is_none());
    assert!(list_annotations(document_id.clone())
        .expect("list after erase")
        .is_empty());
    assert!(last.history.can_undo);

    undo_text_edit(document_id.clone()).expect("undo last erase");
    let restored = list_annotations(document_id.clone()).expect("list restored");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, name, "the annotation identity is stable");
    assert_eq!(restored[0].strokes.len(), 1);
    assert_eq!(restored[0].strokes[0][0].x, 100.0);
    assert_eq!(restored[0].strokes[0][0].y, 300.0);

    // Undo far enough to restore both strokes.
    undo_text_edit(document_id.clone()).expect("undo first erase");
    assert_eq!(
        list_annotations(document_id.clone()).expect("list both")[0]
            .strokes
            .len(),
        2
    );
}

#[test]
fn erasing_rejects_non_ink_annotations_and_bad_stroke_indices() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let note = create_note_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateNoteRequest {
            page_index: 0,
            color: "#f9a825".to_owned(),
            opacity: 100,
            text: "便签".to_owned(),
            rect: TextMarkupRect {
                x: 40.0,
                y: 40.0,
                width: 30.0,
                height: 30.0,
            },
        },
    )
    .expect("create note");
    let note_name = note.annotation.expect("note view").name;
    let error = erase_ink_stroke(document_id.clone(), note_name, 0).expect_err("note is not ink");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);

    let ink = create_ink_with_author(document_id.clone(), "作者".to_owned(), two_stroke_request())
        .expect("create ink");
    let ink_name = ink.annotation.expect("ink view").name;
    let error = erase_ink_stroke(document_id.clone(), ink_name.clone(), 5).expect_err("bad index");
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    assert_eq!(
        list_annotations(document_id.clone()).expect("list after rejection")[1]
            .strokes
            .len(),
        2
    );

    let missing = erase_ink_stroke(document_id, "missing-name".to_owned(), 0)
        .expect_err("unknown annotations are rejected");
    assert_eq!(missing.code, CommandErrorCode::InvalidArgument);
}

#[test]
fn whole_annotation_eraser_deletion_covers_every_annotation_kind() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let markup = create_text_markup_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 100,
            rects: vec![TextMarkupRect {
                x: 100.0,
                y: 300.0,
                width: 80.0,
                height: 20.0,
            }],
        },
    )
    .expect("markup");
    let note = create_note_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateNoteRequest {
            page_index: 0,
            color: "#f9a825".to_owned(),
            opacity: 100,
            text: "便签".to_owned(),
            rect: TextMarkupRect {
                x: 200.0,
                y: 100.0,
                width: 30.0,
                height: 30.0,
            },
        },
    )
    .expect("note");
    let stamp = create_stamp_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateStampRequest {
            page_index: 0,
            kind: StampKind::Approved,
            opacity: 100,
            frame: StampFrameView {
                x: 240.0,
                y: 120.0,
                width: 120.0,
                height: 40.0,
                rotation: 0.0,
            },
        },
    )
    .expect("stamp");
    let names = vec![
        markup.annotation.expect("markup view").name,
        note.annotation.expect("note view").name,
        stamp.annotation.expect("stamp view").name,
    ];
    for name in &names {
        delete_annotation(document_id.clone(), name.clone()).expect("eraser whole delete");
    }
    assert!(list_annotations(document_id.clone())
        .expect("list after deletes")
        .is_empty());
    // One delete per annotation, each independently undoable.
    for _ in &names {
        undo_text_edit(document_id.clone()).expect("undo delete");
    }
    let restored = list_annotations(document_id.clone()).expect("list restored");
    assert_eq!(restored.len(), 3);
    assert!(restored
        .iter()
        .any(|annotation| annotation.kind == AnnotationKind::Stamp));
    assert!(restored
        .iter()
        .any(|annotation| annotation.kind == AnnotationKind::Note));
    assert!(restored
        .iter()
        .any(|annotation| annotation.kind == AnnotationKind::Highlight));
    assert!(edit_history(document_id).expect("history").can_redo);
}

#[test]
fn erase_is_refused_without_annotation_permission() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open restricted fixture")
        .document_id;
    let error = erase_ink_stroke(document_id.clone(), "pdfx-nothing".to_owned(), 0)
        .expect_err("permission restrictions refuse erasing");
    assert_eq!(error.code, CommandErrorCode::PermissionDenied);
    close_document(document_id).expect("close");
}
