//! Command-boundary tests for sticky-note and freehand-ink annotations.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is parsed with lopdf and reopened through the command
//! boundary, so "the annotation is a native object with an appearance stream"
//! is checked against a fresh parse, not the in-memory document.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_ink_from_settings, create_ink_with_author, create_note_from_settings,
    create_note_with_author, delete_annotation, list_annotations, transform_note_annotation,
    update_annotation_style, update_note_text, AnnotationKind, CreateInkRequest, CreateNoteRequest,
    CreateTextMarkupRequest, InkPoint, TextMarkupKind, TextMarkupRect,
};
use pdfx_lib::commands::{close_document, open_document, page_info, render_page, CommandErrorCode};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::settings::{Settings, SettingsStore};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::fixtures;
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
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

fn note_request(rect: TextMarkupRect, text: &str) -> CreateNoteRequest {
    CreateNoteRequest {
        page_index: 0,
        color: "#f9a825".to_owned(),
        opacity: 100,
        text: text.to_owned(),
        rect,
    }
}

fn ink_request(strokes: Vec<Vec<(f32, f32)>>) -> CreateInkRequest {
    CreateInkRequest {
        page_index: 0,
        color: "#c62828".to_owned(),
        opacity: 80,
        line_width: 6.0,
        strokes: strokes
            .into_iter()
            .map(|stroke| stroke.into_iter().map(|(x, y)| InkPoint { x, y }).collect())
            .collect(),
    }
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

fn find_annotation<'a>(
    annotations: &'a [lopdf::Dictionary],
    subtype: &[u8],
) -> &'a lopdf::Dictionary {
    annotations
        .iter()
        .find(|annotation| {
            annotation
                .get(b"Subtype")
                .ok()
                .and_then(|object| object.as_name().ok())
                .is_some_and(|name| name == subtype)
        })
        .unwrap_or_else(|| {
            panic!(
                "no {:?} annotation in {:?}",
                String::from_utf8_lossy(subtype),
                annotations
            )
        })
}

fn assert_normal_appearance_is_stream(document: &lopdf::Document, annotation: &lopdf::Dictionary) {
    let appearance = annotation
        .get(b"AP")
        .expect("/AP appearance dictionary")
        .as_dict()
        .expect("appearance dictionary");
    let normal = match appearance.get(b"N").expect("normal appearance") {
        Object::Reference(id) => document.get_object(*id).expect("appearance stream"),
        object => object,
    };
    assert!(
        normal.as_stream().is_ok(),
        "the normal appearance must be a stream, got {normal:?}"
    );
}

fn numbers(object: &Object) -> Vec<f32> {
    object
        .as_array()
        .expect("numeric array")
        .iter()
        .map(|component| component.as_float().expect("numeric component"))
        .collect()
}

fn assert_close(actual: f32, expected: f32, context: &str) {
    assert!(
        (actual - expected).abs() < 0.6,
        "{context}: expected {expected}, got {actual}"
    );
}

#[test]
fn a_note_saves_as_a_native_text_annotation_with_contents_author_and_appearance() {
    let dir = TempDir::new("note-create");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = TextMarkupRect {
        x: 400.0,
        y: 300.0,
        width: 40.0,
        height: 40.0,
    };

    let mutation = create_note_with_author(
        document_id.clone(),
        "便签作者".to_owned(),
        note_request(rect, "检查这一行"),
    )
    .expect("create note");
    let created = mutation.annotation.expect("created note view");
    assert_eq!(created.kind, AnnotationKind::Note);
    assert_eq!(created.text.as_deref(), Some("检查这一行"));
    assert_eq!(created.author.as_deref(), Some("便签作者"));
    assert!(created
        .created_at
        .as_deref()
        .is_some_and(|date| !date.is_empty()));
    assert_eq!(created.color, "#f9a825");
    assert_eq!(created.opacity, 100);
    assert!(created.strokes.is_empty());
    assert_eq!(created.rects.len(), 1);
    assert_close(created.rects[0].x, 400.0, "note x");
    assert_close(created.rects[0].y, 300.0, "note y");
    assert_close(created.rects[0].width, 40.0, "note width");
    assert_close(created.rects[0].height, 40.0, "note height");
    assert!(mutation.history.dirty);

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, AnnotationKind::Note);
    assert_eq!(listed[0].text.as_deref(), Some("检查这一行"));

    let output = dir.path().join("note.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let note = find_annotation(&annotations, b"Text");
    assert_eq!(
        text_of(note.get(b"T").expect("/T author")),
        "便签作者",
        "the author must be the native /T entry"
    );
    assert_eq!(
        text_of(note.get(b"Contents").expect("/Contents")),
        "检查这一行",
        "the note body must be the native /Contents entry"
    );
    assert_eq!(
        text_of(note.get(b"NM").expect("/NM name")),
        created.name,
        "the stable annotation name must survive save"
    );
    assert!(
        note.get(b"CreationDate").is_ok(),
        "the annotation must record /CreationDate"
    );
    assert_normal_appearance_is_stream(&parsed, note);
    let rect_values = numbers(note.get(b"Rect").expect("/Rect"));
    assert_eq!(rect_values.len(), 4);
    let xs = [rect_values[0], rect_values[2]];
    let ys = [rect_values[1], rect_values[3]];
    assert_close(xs[0].min(xs[1]), 400.0, "native note left");
    assert_close(xs[0].max(xs[1]), 440.0, "native note right");
    assert_close(ys[0].min(ys[1]), 452.0, "native note bottom");
    assert_close(ys[0].max(ys[1]), 492.0, "native note top");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_notes =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_notes.len(), 1);
    assert_eq!(reopened_notes[0].kind, AnnotationKind::Note);
    assert_eq!(reopened_notes[0].text.as_deref(), Some("检查这一行"));
    assert_eq!(reopened_notes[0].author.as_deref(), Some("便签作者"));
    assert_eq!(reopened_notes[0].created_at, created.created_at);
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        pixels.ink_near(420, 320, |(red, green, blue)| {
            red > 200 && (120..=215).contains(&green) && blue < 120
        }),
        "the note icon appearance should render its colour after reopen"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn an_empty_note_can_be_placed_filled_in_and_undone() {
    // The page UI drops a note first and lets the user type afterwards, so an
    // empty body has to be a valid native annotation from the very first save
    // and through the undo snapshot of that creation.
    let dir = TempDir::new("note-empty");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_note_with_author(
        document_id.clone(),
        "便签作者".to_owned(),
        note_request(
            TextMarkupRect {
                x: 360.0,
                y: 260.0,
                width: 28.0,
                height: 28.0,
            },
            "",
        ),
    )
    .expect("create empty note")
    .annotation
    .expect("created view");
    assert_eq!(created.kind, AnnotationKind::Note);
    assert_eq!(created.text.as_deref(), Some(""));

    let edited = update_note_text(
        document_id.clone(),
        created.name.clone(),
        "补写内容".to_owned(),
    )
    .expect("fill in the note")
    .annotation
    .expect("edited view");
    assert_eq!(edited.text.as_deref(), Some("补写内容"));

    undo_text_edit(document_id.clone()).expect("undo the text edit");
    let undone = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(undone.len(), 1);
    assert_eq!(undone[0].text.as_deref(), Some(""));

    redo_text_edit(document_id.clone()).expect("redo the text edit");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone[0].text.as_deref(), Some("补写内容"));

    let output = dir.path().join("note-empty.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let note = find_annotation(&annotations, b"Text");
    assert_eq!(
        text_of(note.get(b"Contents").expect("/Contents")),
        "补写内容"
    );
    assert_normal_appearance_is_stream(&parsed, note);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_notes =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_notes.len(), 1);
    assert_eq!(reopened_notes[0].text.as_deref(), Some("补写内容"));
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn note_text_edits_are_undoable_and_saved_as_native_contents() {
    let dir = TempDir::new("note-edit");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_note_with_author(
        document_id.clone(),
        "编辑作者".to_owned(),
        note_request(
            TextMarkupRect {
                x: 300.0,
                y: 200.0,
                width: 40.0,
                height: 40.0,
            },
            "初稿",
        ),
    )
    .expect("create note")
    .annotation
    .expect("created view");
    let original_created_at = created.created_at.clone();

    let edited = update_note_text(
        document_id.clone(),
        created.name.clone(),
        "修订后的内容".to_owned(),
    )
    .expect("edit note text")
    .annotation
    .expect("edited view");
    assert_eq!(edited.name, created.name);
    assert_eq!(edited.kind, AnnotationKind::Note);
    assert_eq!(edited.text.as_deref(), Some("修订后的内容"));
    assert_eq!(edited.author, created.author);
    assert_eq!(edited.created_at, original_created_at);

    undo_text_edit(document_id.clone()).expect("undo text edit");
    let undone = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(undone[0].text.as_deref(), Some("初稿"));
    redo_text_edit(document_id.clone()).expect("redo text edit");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone[0].text.as_deref(), Some("修订后的内容"));

    let output = dir.path().join("note-edited.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let note = find_annotation(&annotations, b"Text");
    assert_eq!(
        text_of(note.get(b"Contents").expect("/Contents")),
        "修订后的内容"
    );
    assert_normal_appearance_is_stream(&parsed, note);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_notes =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_notes[0].text.as_deref(), Some("修订后的内容"));
    assert_eq!(reopened_notes[0].created_at, original_created_at);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn note_move_and_resize_are_undoable_and_saved_as_native_rectangle() {
    let dir = TempDir::new("note-transform");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_note_with_author(
        document_id.clone(),
        "移动作者".to_owned(),
        note_request(
            TextMarkupRect {
                x: 400.0,
                y: 300.0,
                width: 40.0,
                height: 40.0,
            },
            "移动我",
        ),
    )
    .expect("create note")
    .annotation
    .expect("created view");

    let moved_rect = TextMarkupRect {
        x: 100.0,
        y: 100.0,
        width: 80.0,
        height: 60.0,
    };
    let moved = transform_note_annotation(document_id.clone(), created.name.clone(), moved_rect)
        .expect("move and resize note")
        .annotation
        .expect("moved view");
    assert_eq!(moved.name, created.name);
    assert_eq!(moved.text.as_deref(), Some("移动我"));
    assert_close(moved.rects[0].x, 100.0, "moved x");
    assert_close(moved.rects[0].y, 100.0, "moved y");
    assert_close(moved.rects[0].width, 80.0, "moved width");
    assert_close(moved.rects[0].height, 60.0, "moved height");

    undo_text_edit(document_id.clone()).expect("undo note transform");
    let undone = list_annotations(document_id.clone()).expect("list after undo");
    assert_close(undone[0].rects[0].x, 400.0, "restored x");
    assert_close(undone[0].rects[0].width, 40.0, "restored width");
    redo_text_edit(document_id.clone()).expect("redo note transform");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_close(redone[0].rects[0].width, 80.0, "redone width");

    let output = dir.path().join("note-moved.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let note = find_annotation(&annotations, b"Text");
    let values = numbers(note.get(b"Rect").expect("/Rect"));
    let xs = [values[0], values[2]];
    let ys = [values[1], values[3]];
    assert_close(xs[0].min(xs[1]), 100.0, "native moved left");
    assert_close(xs[0].max(xs[1]), 180.0, "native moved right");
    assert_close(ys[0].min(ys[1]), 632.0, "native moved bottom");
    assert_close(ys[0].max(ys[1]), 692.0, "native moved top");
    assert_normal_appearance_is_stream(&parsed, note);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let notes = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_close(notes[0].rects[0].x, 100.0, "reopened x");
    assert_close(notes[0].rects[0].height, 60.0, "reopened height");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    assert!(
        pixels.ink_near(140, 130, |(red, green, blue)| {
            red > 200 && (120..=215).contains(&green) && blue < 120
        }),
        "the moved note icon should render at its new position"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn deleting_a_note_restores_its_text_author_and_appearance_on_undo() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_note_with_author(
        document_id.clone(),
        "删除作者".to_owned(),
        note_request(
            TextMarkupRect {
                x: 200.0,
                y: 200.0,
                width: 40.0,
                height: 40.0,
            },
            "稍后回来",
        ),
    )
    .expect("create note")
    .annotation
    .expect("created view");

    let deleted =
        delete_annotation(document_id.clone(), created.name.clone()).expect("delete note");
    assert!(deleted.annotation.is_none());
    assert!(list_annotations(document_id.clone())
        .expect("list after delete")
        .is_empty());

    undo_text_edit(document_id.clone()).expect("undo note deletion");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, created.name);
    assert_eq!(restored[0].text.as_deref(), Some("稍后回来"));
    assert_eq!(restored[0].author.as_deref(), Some("删除作者"));
    assert_eq!(restored[0].created_at, created.created_at);

    redo_text_edit(document_id.clone()).expect("redo note deletion");
    assert!(list_annotations(document_id.clone())
        .expect("list after redo")
        .is_empty());
    close_document(document_id).expect("close document");
}

#[test]
fn ink_saves_as_a_native_ink_annotation_with_inklist_and_appearance() {
    let dir = TempDir::new("ink-create");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let strokes = vec![
        vec![(150.0, 500.0), (250.0, 480.0), (350.0, 520.0)],
        vec![(200.0, 550.0), (300.0, 550.0)],
    ];

    let mutation = create_ink_with_author(
        document_id.clone(),
        "手绘作者".to_owned(),
        ink_request(strokes.clone()),
    )
    .expect("create ink");
    let created = mutation.annotation.expect("created ink view");
    assert_eq!(created.kind, AnnotationKind::Ink);
    assert_eq!(created.author.as_deref(), Some("手绘作者"));
    assert!(created
        .created_at
        .as_deref()
        .is_some_and(|date| !date.is_empty()));
    assert_eq!(created.color, "#c62828");
    assert_eq!(created.opacity, 80);
    assert_eq!(created.line_width, Some(6.0));
    assert_eq!(created.strokes.len(), 2);
    assert_eq!(created.strokes[0].len(), 3);
    assert_eq!(created.strokes[1].len(), 2);
    for (actual, expected) in created.strokes[0].iter().zip(&strokes[0]) {
        assert_close(actual.x, expected.0, "ink x");
        assert_close(actual.y, expected.1, "ink y");
    }

    let output = dir.path().join("ink.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let ink = find_annotation(&annotations, b"Ink");
    assert_eq!(text_of(ink.get(b"T").expect("/T author")), "手绘作者");
    assert_eq!(text_of(ink.get(b"NM").expect("/NM name")), created.name);
    assert_normal_appearance_is_stream(&parsed, ink);
    let ink_list = ink
        .get(b"InkList")
        .expect("/InkList")
        .as_array()
        .expect("InkList array");
    assert_eq!(ink_list.len(), 2, "one entry per stroke");
    let first = numbers(&ink_list[0]);
    let second = numbers(&ink_list[1]);
    assert_eq!(first.len(), 6, "three points means six coordinates");
    assert_eq!(second.len(), 4, "two points means four coordinates");
    let colour = numbers(ink.get(b"C").expect("/C"));
    assert_eq!(colour.len(), 3);
    assert!((colour[0] - 0.7765).abs() < 0.01, "red: {colour:?}");
    assert!((colour[1] - 0.1569).abs() < 0.01, "green: {colour:?}");
    assert!((colour[2] - 0.1569).abs() < 0.01, "blue: {colour:?}");
    let alpha = ink
        .get(b"CA")
        .expect("/CA opacity")
        .as_float()
        .expect("numeric /CA");
    assert!((alpha - 0.8).abs() < 0.01, "80% opacity, got {alpha}");
    // Pdfium writes the line width into the standard `/Border` array rather
    // than a `/BS` dictionary; both are native ways to carry the stroke width.
    let border = numbers(ink.get(b"Border").expect("/Border width"));
    assert_eq!(border.len(), 3, "border array is [h v w]");
    assert!((border[2] - 6.0).abs() < 0.01, "line width, got {border:?}");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_ink =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_ink.len(), 1);
    assert_eq!(reopened_ink[0].kind, AnnotationKind::Ink);
    assert_eq!(reopened_ink[0].line_width, Some(6.0));
    assert_eq!(reopened_ink[0].strokes.len(), 2);
    assert_eq!(reopened_ink[0].strokes[0].len(), 3);
    assert_eq!(reopened_ink[0].color, "#c62828");
    assert_eq!(reopened_ink[0].opacity, 80);
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let red_ink = |(red, green, blue): (u8, u8, u8)| red > 170 && green < 140 && blue < 140;
    assert!(
        pixels.ink_near(250, 480, red_ink),
        "the first stroke should render after reopen"
    );
    assert!(
        pixels.ink_near(250, 550, red_ink),
        "the second stroke should render after reopen"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn ink_undo_redo_snapshots_strokes_and_style_changes() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_ink_with_author(
        document_id.clone(),
        "快照作者".to_owned(),
        ink_request(vec![
            vec![(100.0, 400.0), (200.0, 420.0)],
            vec![(150.0, 450.0), (250.0, 430.0), (300.0, 460.0)],
        ]),
    )
    .expect("create ink")
    .annotation
    .expect("created view");

    undo_text_edit(document_id.clone()).expect("undo ink creation");
    assert!(list_annotations(document_id.clone())
        .expect("list after undo")
        .is_empty());
    redo_text_edit(document_id.clone()).expect("redo ink creation");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone.len(), 1);
    assert_eq!(redone[0].name, created.name);
    assert_eq!(redone[0].strokes.len(), 2);
    assert_eq!(redone[0].strokes[1].len(), 3);
    assert_eq!(redone[0].line_width, Some(6.0));

    let restyled = update_annotation_style(
        document_id.clone(),
        created.name.clone(),
        "#1565c0".to_owned(),
        60,
        Some(12.0),
    )
    .expect("restyle ink")
    .annotation
    .expect("restyled view");
    assert_eq!(restyled.color, "#1565c0");
    assert_eq!(restyled.opacity, 60);
    assert_eq!(restyled.line_width, Some(12.0));
    assert_eq!(
        restyled.strokes, created.strokes,
        "style keeps every stroke"
    );
    assert_eq!(restyled.author, created.author);
    assert_eq!(restyled.created_at, created.created_at);

    undo_text_edit(document_id.clone()).expect("undo ink style");
    let undone = list_annotations(document_id.clone()).expect("list after style undo");
    assert_eq!(undone[0].color, "#c62828");
    assert_eq!(undone[0].line_width, Some(6.0));
    assert_eq!(undone[0].strokes, created.strokes);
    redo_text_edit(document_id.clone()).expect("redo ink style");
    let redone_style = list_annotations(document_id.clone()).expect("list after style redo");
    assert_eq!(redone_style[0].color, "#1565c0");
    assert_eq!(redone_style[0].line_width, Some(12.0));
    assert_eq!(redone_style[0].strokes, created.strokes);

    delete_annotation(document_id.clone(), created.name.clone()).expect("delete ink");
    assert!(list_annotations(document_id.clone())
        .expect("list after delete")
        .is_empty());
    undo_text_edit(document_id.clone()).expect("undo ink deletion");
    let restored = list_annotations(document_id.clone()).expect("list after delete undo");
    assert_eq!(restored[0].strokes, created.strokes);
    assert_eq!(restored[0].line_width, Some(12.0));
    assert_eq!(restored[0].color, "#1565c0");
    close_document(document_id).expect("close document");
}

#[test]
fn annotations_are_listed_by_page_across_kinds() {
    let document_id = open_document(fixture_path("rotated-mixed-size"), None)
        .expect("open the rotated multi-page fixture")
        .document_id;
    let note_rect = TextMarkupRect {
        x: 40.0,
        y: 60.0,
        width: 40.0,
        height: 40.0,
    };
    let note = create_note_with_author(
        document_id.clone(),
        "分组作者".to_owned(),
        CreateNoteRequest {
            page_index: 0,
            color: "#f9a825".to_owned(),
            opacity: 100,
            text: "第一页便签".to_owned(),
            rect: note_rect,
        },
    )
    .expect("note on page 1")
    .annotation
    .expect("note view");
    let highlight = pdfx_lib::annotations::create_text_markup_with_author(
        document_id.clone(),
        "分组作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 50,
            rects: vec![TextMarkupRect {
                x: 40.0,
                y: 160.0,
                width: 80.0,
                height: 20.0,
            }],
        },
    )
    .expect("highlight on page 1")
    .annotation
    .expect("highlight view");
    let ink = create_ink_with_author(
        document_id.clone(),
        "分组作者".to_owned(),
        CreateInkRequest {
            page_index: 2,
            color: "#c62828".to_owned(),
            opacity: 80,
            line_width: 4.0,
            strokes: vec![vec![
                InkPoint { x: 40.0, y: 60.0 },
                InkPoint { x: 120.0, y: 90.0 },
            ]],
        },
    )
    .expect("ink on page 3")
    .annotation
    .expect("ink view");

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed
            .iter()
            .map(|view| (view.page_index, view.kind))
            .collect::<Vec<_>>(),
        vec![
            (0, AnnotationKind::Note),
            (0, AnnotationKind::Highlight),
            (2, AnnotationKind::Ink)
        ]
    );
    assert_eq!(listed[0].name, note.name);
    assert_eq!(listed[1].name, highlight.name);
    assert_eq!(listed[2].name, ink.name);
    assert_eq!(listed[0].text.as_deref(), Some("第一页便签"));
    assert_close(listed[0].rects[0].x, note_rect.x, "rotated note x");
    assert_close(listed[0].rects[0].y, note_rect.y, "rotated note y");
    assert_close(
        listed[0].rects[0].width,
        note_rect.width,
        "rotated note width",
    );
    assert_close(listed[2].strokes[0][0].x, 40.0, "rotated ink x");
    assert_close(listed[2].strokes[0][0].y, 60.0, "rotated ink y");
    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_note_and_ink_changes_but_still_list_them() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;
    let rect = TextMarkupRect {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };

    let note_error = create_note_with_author(
        document_id.clone(),
        "作者".to_owned(),
        note_request(rect, "不允许"),
    )
    .expect_err("notes are denied");
    assert_eq!(note_error.code, CommandErrorCode::PermissionDenied);
    let ink_error = create_ink_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateInkRequest {
            page_index: 0,
            color: "#c62828".to_owned(),
            opacity: 80,
            line_width: 4.0,
            strokes: vec![vec![
                InkPoint { x: 10.0, y: 10.0 },
                InkPoint { x: 20.0, y: 20.0 },
            ]],
        },
    )
    .expect_err("ink is denied");
    assert_eq!(ink_error.code, CommandErrorCode::PermissionDenied);
    let edit_error = update_note_text(document_id.clone(), "missing".to_owned(), "x".to_owned())
        .expect_err("note edits are denied");
    assert_eq!(edit_error.code, CommandErrorCode::PermissionDenied);
    let transform_error =
        transform_note_annotation(document_id.clone(), "missing".to_owned(), rect)
            .expect_err("note moves are denied");
    assert_eq!(transform_error.code, CommandErrorCode::PermissionDenied);
    assert!(list_annotations(document_id.clone())
        .expect("listing is allowed")
        .is_empty());
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_note_and_ink_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let rect = TextMarkupRect {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };

    let cases = [
        (
            "invalid colour",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateNoteRequest {
                    color: "yellow".to_owned(),
                    ..note_request(rect, "便签")
                },
            )
            .expect_err("invalid colour must fail"),
        ),
        (
            "opacity above 100",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateNoteRequest {
                    opacity: 101,
                    ..note_request(rect, "便签")
                },
            )
            .expect_err("invalid opacity must fail"),
        ),
        (
            "empty geometry",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                note_request(
                    TextMarkupRect {
                        x: 10.0,
                        y: 10.0,
                        width: 0.0,
                        height: 20.0,
                    },
                    "便签",
                ),
            )
            .expect_err("zero-width note must fail"),
        ),
        (
            "over-long note text",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                note_request(rect, &"字".repeat(5001)),
            )
            .expect_err("over-long note text must fail"),
        ),
        (
            "note text with control characters",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                note_request(rect, "bad\u{7}text"),
            )
            .expect_err("control characters must fail"),
        ),
        (
            "ink without strokes",
            create_ink_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateInkRequest {
                    strokes: vec![],
                    ..ink_request(vec![])
                },
            )
            .expect_err("empty ink must fail"),
        ),
        (
            "ink with an empty stroke",
            create_ink_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateInkRequest {
                    strokes: vec![vec![]],
                    ..ink_request(vec![])
                },
            )
            .expect_err("empty stroke must fail"),
        ),
        (
            "ink with a non-finite point",
            create_ink_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateInkRequest {
                    strokes: vec![vec![InkPoint {
                        x: f32::NAN,
                        y: 10.0,
                    }]],
                    ..ink_request(vec![])
                },
            )
            .expect_err("non-finite point must fail"),
        ),
        (
            "ink with zero width",
            create_ink_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateInkRequest {
                    line_width: 0.0,
                    ..ink_request(vec![vec![(10.0, 10.0), (20.0, 20.0)]])
                },
            )
            .expect_err("zero line width must fail"),
        ),
        (
            "ink with opacity above 100",
            create_ink_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateInkRequest {
                    opacity: 101,
                    ..ink_request(vec![vec![(10.0, 10.0), (20.0, 20.0)]])
                },
            )
            .expect_err("invalid opacity must fail"),
        ),
        (
            "page outside the document",
            create_note_with_author(
                document_id.clone(),
                "作者".to_owned(),
                CreateNoteRequest {
                    page_index: 7,
                    ..note_request(rect, "便签")
                },
            )
            .expect_err("out-of-range page must fail"),
        ),
    ];
    for (context, error) in &cases[..10] {
        assert_eq!(error.code, CommandErrorCode::InvalidArgument, "{context}");
    }
    assert_eq!(
        cases[10].1.code,
        CommandErrorCode::PageOutOfRange,
        "{}",
        cases[10].0
    );

    let highlight = pdfx_lib::annotations::create_text_markup_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 50,
            rects: vec![rect],
        },
    )
    .expect("create highlight")
    .annotation
    .expect("highlight view");

    assert_eq!(
        update_note_text(document_id.clone(), highlight.name.clone(), "x".to_owned())
            .expect_err("text edits only apply to notes")
            .code,
        CommandErrorCode::InvalidArgument
    );
    assert_eq!(
        transform_note_annotation(document_id.clone(), highlight.name.clone(), rect)
            .expect_err("moves only apply to notes")
            .code,
        CommandErrorCode::InvalidArgument
    );
    assert_eq!(
        update_annotation_style(
            document_id.clone(),
            highlight.name.clone(),
            "#00ff00".to_owned(),
            50,
            Some(4.0),
        )
        .expect_err("line width only applies to ink")
        .code,
        CommandErrorCode::InvalidArgument
    );

    let delete_error =
        delete_annotation(document_id.clone(), highlight.name.clone()).expect("cleanup");
    assert!(delete_error.annotation.is_none());
    // The cleanup is a real action, so rewind it to check rejected requests
    // never created history of their own.
    undo_text_edit(document_id.clone()).expect("undo cleanup");
    undo_text_edit(document_id.clone()).expect("undo highlight");
    let history = edit_history(document_id.clone()).expect("history after rejected requests");
    assert!(!history.dirty, "rejected requests must not create history");
    assert!(!history.can_undo);
    assert!(list_annotations(document_id.clone())
        .expect("list after rejected requests")
        .is_empty());
    close_document(document_id).expect("close document");
}

#[test]
fn notes_and_ink_can_be_placed_on_a_scanned_page() {
    let dir = TempDir::new("note-ink-scanned");
    let document_id = open_document(fixture_path("scanned-image-only"), None)
        .expect("open the scanned fixture")
        .document_id;
    let page = page_info(document_id.clone(), 0).expect("page info");
    let note_rect = TextMarkupRect {
        x: page.width * 0.2,
        y: page.height * 0.2,
        width: 48.0,
        height: 48.0,
    };
    let note = create_note_with_author(
        document_id.clone(),
        "扫描作者".to_owned(),
        note_request(note_rect, "扫描页便签"),
    )
    .expect("note on scanned page")
    .annotation
    .expect("note view");
    let ink = create_ink_with_author(
        document_id.clone(),
        "扫描作者".to_owned(),
        CreateInkRequest {
            page_index: 0,
            color: "#1565c0".to_owned(),
            opacity: 100,
            line_width: 5.0,
            strokes: vec![vec![
                InkPoint {
                    x: page.width * 0.3,
                    y: page.height * 0.5,
                },
                InkPoint {
                    x: page.width * 0.6,
                    y: page.height * 0.5,
                },
            ]],
        },
    )
    .expect("ink on scanned page")
    .annotation
    .expect("ink view");

    let output = dir.path().join("scanned-annotations.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let saved_note = find_annotation(&annotations, b"Text");
    assert_eq!(
        text_of(saved_note.get(b"Contents").expect("/Contents")),
        "扫描页便签"
    );
    assert_normal_appearance_is_stream(&parsed, saved_note);
    let saved_ink = find_annotation(&annotations, b"Ink");
    assert_eq!(
        saved_ink
            .get(b"InkList")
            .expect("/InkList")
            .as_array()
            .expect("InkList array")
            .len(),
        1
    );
    assert_normal_appearance_is_stream(&parsed, saved_ink);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_views.len(), 2);
    assert_eq!(reopened_views[0].kind, AnnotationKind::Note);
    assert_eq!(reopened_views[0].name, note.name);
    assert_eq!(reopened_views[0].text.as_deref(), Some("扫描页便签"));
    assert_eq!(reopened_views[1].kind, AnnotationKind::Ink);
    assert_eq!(reopened_views[1].name, ink.name);
    assert_eq!(reopened_views[1].line_width, Some(5.0));
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn notes_and_ink_take_their_author_from_settings_at_creation_time() {
    let dir = TempDir::new("note-ink-author");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let store = SettingsStore::new(dir.path());
    let mut settings = Settings {
        annotation_author: "Alice".to_owned(),
        ..Settings::default()
    };
    store.save(&settings).expect("persist settings");

    let alice_note = create_note_from_settings(
        document_id.clone(),
        &store.load(),
        note_request(
            TextMarkupRect {
                x: 60.0,
                y: 200.0,
                width: 28.0,
                height: 28.0,
            },
            "Alice 的便签",
        ),
    )
    .expect("create note as Alice")
    .annotation
    .expect("Alice's note");
    assert_eq!(alice_note.author.as_deref(), Some("Alice"));
    assert!(alice_note
        .created_at
        .as_deref()
        .is_some_and(|date| date.starts_with("D:")));

    settings.annotation_author = "Bob".to_owned();
    store.save(&settings).expect("persist changed author");
    let bob_ink = create_ink_from_settings(
        document_id.clone(),
        &store.load(),
        ink_request(vec![vec![(120.0, 520.0), (200.0, 540.0)]]),
    )
    .expect("create ink as Bob")
    .annotation
    .expect("Bob's ink");
    assert_eq!(bob_ink.author.as_deref(), Some("Bob"));
    assert!(bob_ink
        .created_at
        .as_deref()
        .is_some_and(|date| date.starts_with("D:")));

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == alice_note.name)
            .and_then(|view| view.author.as_deref()),
        Some("Alice"),
        "changing the setting must not rewrite existing annotations"
    );
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == bob_ink.name)
            .and_then(|view| view.author.as_deref()),
        Some("Bob")
    );
    close_document(document_id).expect("close document");
}
