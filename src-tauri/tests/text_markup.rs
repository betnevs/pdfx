//! Command-boundary tests for text markup annotations.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is parsed with lopdf and reopened through the command
//! boundary so "the annotation is a native object with an appearance stream"
//! is checked against a fresh parse, not the in-memory document.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_text_markup_from_settings, create_text_markup_with_author, delete_annotation,
    list_annotations, update_annotation_style, CreateTextMarkupRequest, TextMarkupKind,
    TextMarkupRect,
};
use pdfx_lib::commands::{
    close_document, extract_text_layer, open_document, render_page, CommandErrorCode, TextLayerView,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::settings::{default_annotation_author, Settings, SettingsStore};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::fixtures;
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

fn request(kind: TextMarkupKind, rects: Vec<TextMarkupRect>) -> CreateTextMarkupRequest {
    CreateTextMarkupRequest {
        page_index: 0,
        kind,
        color: "#ffd400".to_owned(),
        opacity: 100,
        rects,
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

    /// True when a pixel within one point of `(x, y)` satisfies `matches`.
    /// Rendering anti-aliases thin strokes, so the exact centre pixel is not
    /// guaranteed to carry the full colour.
    fn ink_near(&self, x: u32, y: u32, matches: impl Fn((u8, u8, u8)) -> bool) -> bool {
        for dy in y.saturating_sub(1)..=(y + 1) {
            for dx in x.saturating_sub(1)..=(x + 1) {
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

fn colour_components(annotation: &lopdf::Dictionary) -> Vec<f32> {
    annotation
        .get(b"C")
        .expect("/C colour")
        .as_array()
        .expect("/C array")
        .iter()
        .map(|component| component.as_float().expect("numeric colour component"))
        .collect()
}

#[test]
fn a_selection_highlight_saves_as_a_native_annotation_with_an_appearance_and_author() {
    let dir = TempDir::new("text-markup-highlight");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let layer = extract_text_layer(document_id.clone(), 0).expect("text layer");
    let rect = selection_rect(&layer, "Original");

    let mutation = create_text_markup_with_author(
        document_id.clone(),
        "测试作者".to_owned(),
        request(TextMarkupKind::Highlight, vec![rect]),
    )
    .expect("create highlight");

    let created = mutation.annotation.expect("created annotation view");
    assert_eq!(created.kind, TextMarkupKind::Highlight);
    assert_eq!(created.author.as_deref(), Some("测试作者"));
    assert!(
        created
            .created_at
            .as_deref()
            .is_some_and(|date| !date.is_empty()),
        "the annotation must record a creation time"
    );
    assert_eq!(created.color, "#ffd400");
    assert_eq!(created.opacity, 100);
    assert!(
        mutation.history.dirty,
        "creating an annotation dirties the document"
    );
    assert!(mutation.history.can_undo);

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, created.name);
    assert_eq!(listed[0].page_index, 0);
    assert_eq!(listed[0].author.as_deref(), Some("测试作者"));

    let output = dir.path().join("highlight.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let highlight = annotations
        .iter()
        .find(|annotation| {
            annotation
                .get(b"Subtype")
                .ok()
                .and_then(|object| object.as_name().ok())
                .is_some_and(|name| name == b"Highlight")
        })
        .expect("a Highlight annotation dictionary");
    assert_eq!(
        text_of(highlight.get(b"T").expect("/T author")),
        "测试作者",
        "the author must be the native /T entry"
    );
    assert!(
        highlight.get(b"CreationDate").is_ok(),
        "the annotation must record /CreationDate"
    );
    assert_eq!(
        text_of(highlight.get(b"NM").expect("/NM name")),
        created.name,
        "the stable annotation name must survive save"
    );
    let quad_points = highlight
        .get(b"QuadPoints")
        .expect("/QuadPoints")
        .as_array()
        .expect("QuadPoints array");
    assert_eq!(
        quad_points.len(),
        8,
        "one selected line is one quadrilateral"
    );
    let appearance = highlight
        .get(b"AP")
        .expect("/AP appearance dictionary")
        .as_dict()
        .expect("appearance dictionary");
    let normal = match appearance.get(b"N").expect("normal appearance") {
        Object::Reference(id) => parsed.get_object(*id).expect("appearance stream"),
        object => object,
    };
    assert!(
        normal.as_stream().is_ok(),
        "the normal appearance must be a stream, got {normal:?}"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_annotations =
        list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_annotations.len(), 1);
    assert_eq!(reopened_annotations[0].author.as_deref(), Some("测试作者"));
    assert_eq!(reopened_annotations[0].kind, TextMarkupKind::Highlight);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn annotation_opacity_is_visible_in_rendered_pixels_after_reopen() {
    let dir = TempDir::new("text-markup-opacity");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    // A blank patch of the page, away from the text line: half-opacity black
    // over white must render mid-grey if the constant opacity is honoured.
    let mut half = request(
        TextMarkupKind::Highlight,
        vec![TextMarkupRect {
            x: 100.0,
            y: 300.0,
            width: 120.0,
            height: 40.0,
        }],
    );
    half.color = "#000000".to_owned();
    half.opacity = 50;
    create_text_markup_with_author(document_id.clone(), "作者".to_owned(), half)
        .expect("create half-opacity highlight");

    let output = dir.path().join("opacity.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let image =
        pdfx_lib::commands::render_page(reopened.document_id.clone(), 0, 1.0).expect("render page");
    let decoder = png::Decoder::new(std::io::Cursor::new(
        base64::engine::general_purpose::STANDARD
            .decode(&image.png_base64)
            .expect("render payload is base64"),
    ));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut buffer = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buffer).expect("PNG frame");
    let pixel = |x: u32, y: u32| {
        let offset = ((y * info.width + x) * 4) as usize;
        (buffer[offset], buffer[offset + 1], buffer[offset + 2])
    };
    let (red, green, blue) = pixel(160, 320);
    assert!(
        (80..=190).contains(&red) && (80..=190).contains(&green) && (80..=190).contains(&blue),
        "50% black over white should render mid-grey, got ({red}, {green}, {blue})"
    );
    close_document(reopened.document_id).expect("close reopened");

    let error = create_text_markup_with_author(
        "missing-document".to_owned(),
        "作者".to_owned(),
        request(
            TextMarkupKind::Highlight,
            vec![TextMarkupRect {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 20.0,
            }],
        ),
    )
    .expect_err("unknown document");
    assert_eq!(error.code, CommandErrorCode::DocumentNotFound);
}

#[test]
fn underline_and_strikeout_are_native_annotations_with_visible_appearance_ink() {
    let dir = TempDir::new("text-markup-lines");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let underline = create_text_markup_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Underline,
            color: "#ff0000".to_owned(),
            opacity: 100,
            rects: vec![
                TextMarkupRect {
                    x: 100.0,
                    y: 300.0,
                    width: 120.0,
                    height: 30.0,
                },
                TextMarkupRect {
                    x: 100.0,
                    y: 340.0,
                    width: 120.0,
                    height: 30.0,
                },
            ],
        },
    )
    .expect("create underline");
    let strikeout = create_text_markup_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Strikeout,
            color: "#0000ff".to_owned(),
            opacity: 100,
            rects: vec![TextMarkupRect {
                x: 100.0,
                y: 400.0,
                width: 120.0,
                height: 30.0,
            }],
        },
    )
    .expect("create strikeout");

    let underline_view = underline.annotation.expect("underline view");
    let strikeout_view = strikeout.annotation.expect("strikeout view");
    assert_eq!(
        underline_view.rects.len(),
        2,
        "one rectangle per selected line"
    );
    assert_eq!(underline_view.color, "#ff0000");
    assert_eq!(strikeout_view.color, "#0000ff");

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].kind, TextMarkupKind::Underline);
    assert_eq!(listed[1].kind, TextMarkupKind::Strikeout);

    let output = dir.path().join("lines.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let underline = find_annotation(&annotations, b"Underline");
    let strikeout = find_annotation(&annotations, b"StrikeOut");
    assert_normal_appearance_is_stream(&parsed, underline);
    assert_normal_appearance_is_stream(&parsed, strikeout);
    assert_eq!(
        underline
            .get(b"QuadPoints")
            .expect("/QuadPoints")
            .as_array()
            .expect("QuadPoints array")
            .len(),
        16,
        "two lines means two quadrilaterals"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let red = |pixel: (u8, u8, u8)| pixel.0 > 200 && pixel.1 < 120 && pixel.2 < 120;
    let blue = |pixel: (u8, u8, u8)| pixel.2 > 200 && pixel.0 < 120 && pixel.1 < 120;
    assert!(
        pixels.ink_near(160, 329, red),
        "the first underline stroke should be visible near the bottom of its rectangle"
    );
    assert!(
        pixels.ink_near(160, 369, red),
        "the second underline stroke should be visible"
    );
    assert!(
        pixels.ink_near(160, 415, blue),
        "the strikeout stroke should be visible through the middle of its rectangle"
    );

    let reopened_views =
        list_annotations(reopened.document_id.clone()).expect("list reopened annotations");
    assert_eq!(reopened_views.len(), 2);
    assert_eq!(reopened_views[0].color, "#ff0000");
    assert_eq!(reopened_views[1].color, "#0000ff");
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn style_changes_are_undoable_and_saved_as_native_colour_and_opacity() {
    let dir = TempDir::new("text-markup-style");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let created = create_text_markup_with_author(
        document_id.clone(),
        "样式作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 40,
            rects: vec![TextMarkupRect {
                x: 100.0,
                y: 300.0,
                width: 120.0,
                height: 30.0,
            }],
        },
    )
    .expect("create highlight")
    .annotation
    .expect("created view");
    let original_created_at = created.created_at.clone();

    let restyled = update_annotation_style(
        document_id.clone(),
        created.name.clone(),
        "#00ff00".to_owned(),
        70,
    )
    .expect("restyle highlight")
    .annotation
    .expect("restyled view");
    assert_eq!(
        restyled.name, created.name,
        "the stable name survives restyling"
    );
    assert_eq!(restyled.author, created.author);
    assert_eq!(restyled.created_at, original_created_at);
    assert_eq!(restyled.color, "#00ff00");
    assert_eq!(restyled.opacity, 70);

    let after_undo = undo_text_edit(document_id.clone()).expect("undo restyle");
    assert_eq!(after_undo.page_index, 0);
    let undone = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(undone.len(), 1);
    assert_eq!(undone[0].color, "#ffd400");
    assert_eq!(undone[0].opacity, 40);

    undo_text_edit(document_id.clone()).expect("undo creation");
    assert!(
        list_annotations(document_id.clone())
            .expect("list after undoing creation")
            .is_empty(),
        "undoing the creation removes the annotation"
    );

    redo_text_edit(document_id.clone()).expect("redo creation");
    let redone_creation =
        list_annotations(document_id.clone()).expect("list after redoing creation");
    assert_eq!(redone_creation.len(), 1);
    assert_eq!(redone_creation[0].color, "#ffd400");
    assert_eq!(redone_creation[0].opacity, 40);
    assert_eq!(redone_creation[0].created_at, original_created_at);

    redo_text_edit(document_id.clone()).expect("redo restyle");
    let redone_style = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone_style[0].color, "#00ff00");
    assert_eq!(redone_style[0].opacity, 70);

    let output = dir.path().join("styled.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let highlight = find_annotation(&annotations, b"Highlight");
    let colour = colour_components(highlight);
    assert_eq!(colour.len(), 3);
    assert!((colour[0] - 0.0).abs() < 0.001, "red channel: {colour:?}");
    assert!((colour[1] - 1.0).abs() < 0.001, "green channel: {colour:?}");
    assert!((colour[2] - 0.0).abs() < 0.001, "blue channel: {colour:?}");
    let alpha = highlight
        .get(b"CA")
        .expect("/CA opacity")
        .as_float()
        .expect("numeric /CA");
    assert!(
        (alpha - 0.7).abs() < 0.01,
        "70% opacity should be written as /CA 0.7, got {alpha}"
    );
    assert_normal_appearance_is_stream(&parsed, highlight);

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_views.len(), 1);
    assert_eq!(reopened_views[0].name, created.name);
    assert_eq!(reopened_views[0].color, "#00ff00");
    assert_eq!(reopened_views[0].opacity, 70);
    assert_eq!(reopened_views[0].created_at, original_created_at);
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn deleting_an_annotation_is_undoable_and_restores_its_author_and_creation_time() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let make = |author: &str, y: f32| {
        create_text_markup_with_author(
            document_id.clone(),
            author.to_owned(),
            CreateTextMarkupRequest {
                page_index: 0,
                kind: TextMarkupKind::Highlight,
                color: "#ffd400".to_owned(),
                opacity: 100,
                rects: vec![TextMarkupRect {
                    x: 100.0,
                    y,
                    width: 120.0,
                    height: 30.0,
                }],
            },
        )
        .expect("create highlight")
        .annotation
        .expect("created view")
    };
    let first = make("甲", 300.0);
    let second = make("乙", 400.0);
    let first_created_at = first.created_at.clone();

    let deleted =
        delete_annotation(document_id.clone(), first.name.clone()).expect("delete annotation");
    assert!(deleted.annotation.is_none());
    assert_eq!(deleted.page_index, 0);
    let remaining = list_annotations(document_id.clone()).expect("list after delete");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, second.name);

    undo_text_edit(document_id.clone()).expect("undo deletion");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 2);
    let restored_first = restored
        .iter()
        .find(|view| view.name == first.name)
        .expect("the deleted annotation is back");
    assert_eq!(restored_first.author.as_deref(), Some("甲"));
    assert_eq!(restored_first.created_at, first_created_at);
    assert_eq!(restored_first.color, first.color);
    assert_eq!(restored_first.rects, first.rects);

    redo_text_edit(document_id.clone()).expect("redo deletion");
    let after_redo = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(after_redo.len(), 1);
    assert_eq!(after_redo[0].name, second.name);

    delete_annotation(document_id.clone(), second.name.clone()).expect("delete last annotation");
    assert!(list_annotations(document_id.clone())
        .expect("list after deleting all")
        .is_empty());
    undo_text_edit(document_id.clone()).expect("undo deleting the last annotation");
    let last = list_annotations(document_id.clone()).expect("list after last undo");
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].name, second.name);
    close_document(document_id).expect("close document");
}

#[test]
fn annotations_take_their_author_from_settings_at_creation_time() {
    let dir = TempDir::new("text-markup-author");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let store = SettingsStore::new(dir.path());
    let request = || CreateTextMarkupRequest {
        page_index: 0,
        kind: TextMarkupKind::Highlight,
        color: "#ffd400".to_owned(),
        opacity: 100,
        rects: vec![TextMarkupRect {
            x: 100.0,
            y: 300.0,
            width: 120.0,
            height: 30.0,
        }],
    };

    let mut settings = Settings {
        annotation_author: "Alice".to_owned(),
        ..Settings::default()
    };
    store.save(&settings).expect("persist settings");
    let alice = create_text_markup_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Alice")
        .annotation
        .expect("Alice's annotation");
    assert_eq!(alice.author.as_deref(), Some("Alice"));

    settings.annotation_author = "Bob".to_owned();
    store.save(&settings).expect("persist changed author");
    let bob = create_text_markup_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Bob")
        .annotation
        .expect("Bob's annotation");
    assert_eq!(bob.author.as_deref(), Some("Bob"));

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == alice.name)
            .and_then(|view| view.author.as_deref()),
        Some("Alice"),
        "changing the setting must not rewrite existing annotations"
    );
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == bob.name)
            .and_then(|view| view.author.as_deref()),
        Some("Bob")
    );

    // The default author is the system username with a safe fallback, and it
    // flows through the same settings contract.
    assert_eq!(
        Settings::default().annotation_author,
        default_annotation_author()
    );
    assert!(!default_annotation_author().is_empty());
    let default_author =
        create_text_markup_from_settings(document_id.clone(), &Settings::default(), request())
            .expect("create with the default author")
            .annotation
            .expect("default-author annotation")
            .author;
    assert_eq!(default_author, Some(default_annotation_author()));
    close_document(document_id).expect("close document");
}

#[test]
fn saving_moves_the_dirty_baseline_without_clearing_annotation_history() {
    let dir = TempDir::new("text-markup-dirty");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_text_markup_with_author(
        document_id.clone(),
        "作者".to_owned(),
        CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 50,
            rects: vec![TextMarkupRect {
                x: 100.0,
                y: 300.0,
                width: 120.0,
                height: 30.0,
            }],
        },
    )
    .expect("create highlight")
    .annotation
    .expect("created view");

    let dirty = edit_history(document_id.clone()).expect("history before save");
    assert!(dirty.dirty);
    assert!(dirty.can_undo);

    let output = dir.path().join("dirty.pdf");
    save_as(&document_id, &output);
    let saved = edit_history(document_id.clone()).expect("history after save");
    assert!(
        !saved.dirty,
        "a successful save establishes the dirty baseline"
    );
    assert_eq!(saved.saved_revision, saved.revision);
    assert!(saved.can_undo, "saving keeps the undo history");

    undo_text_edit(document_id.clone()).expect("undo past the save point");
    let undone = edit_history(document_id.clone()).expect("history after undo");
    assert!(
        undone.dirty,
        "moving away from the saved state is dirty again"
    );
    assert!(list_annotations(document_id.clone())
        .expect("list after undo")
        .is_empty());

    redo_text_edit(document_id.clone()).expect("redo back to the saved state");
    let redone = edit_history(document_id.clone()).expect("history after redo");
    assert!(
        !redone.dirty,
        "redoing back to the saved state is clean again"
    );
    assert_eq!(
        list_annotations(document_id.clone())
            .expect("list after redo")
            .first()
            .map(|view| view.name.clone()),
        Some(created.name)
    );
    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_annotation_changes_but_still_list_them() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;
    let request = CreateTextMarkupRequest {
        page_index: 0,
        kind: TextMarkupKind::Highlight,
        color: "#ffd400".to_owned(),
        opacity: 50,
        rects: vec![TextMarkupRect {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 20.0,
        }],
    };

    let create_error =
        create_text_markup_with_author(document_id.clone(), "作者".to_owned(), request)
            .expect_err("annotations are denied");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);
    assert!(list_annotations(document_id.clone())
        .expect("listing is allowed")
        .is_empty());
    let update_error = update_annotation_style(
        document_id.clone(),
        "missing".to_owned(),
        "#00ff00".to_owned(),
        50,
    )
    .expect_err("restyling is denied");
    assert_eq!(update_error.code, CommandErrorCode::PermissionDenied);
    let delete_error = delete_annotation(document_id.clone(), "missing".to_owned())
        .expect_err("deleting is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_annotation_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid_rect = TextMarkupRect {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };
    let request = |mutate: &dyn Fn(&mut CreateTextMarkupRequest)| {
        let mut request = CreateTextMarkupRequest {
            page_index: 0,
            kind: TextMarkupKind::Highlight,
            color: "#ffd400".to_owned(),
            opacity: 50,
            rects: vec![valid_rect],
        };
        mutate(&mut request);
        request
    };

    let cases = [
        (
            "empty selection",
            create_text_markup_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.rects.clear()),
            )
            .expect_err("empty selection must fail"),
        ),
        (
            "opacity above 100",
            create_text_markup_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.opacity = 101),
            )
            .expect_err("invalid opacity must fail"),
        ),
        (
            "invalid colour",
            create_text_markup_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.color = "yellow".to_owned()),
            )
            .expect_err("invalid colour must fail"),
        ),
        (
            "page outside the document",
            create_text_markup_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.page_index = 7),
            )
            .expect_err("out-of-range page must fail"),
        ),
        (
            "empty author",
            create_text_markup_with_author(document_id.clone(), "   ".to_owned(), request(&|_| {}))
                .expect_err("empty author must fail"),
        ),
    ];
    assert_eq!(
        cases[0].1.code,
        CommandErrorCode::InvalidArgument,
        "{}",
        cases[0].0
    );
    assert_eq!(
        cases[1].1.code,
        CommandErrorCode::InvalidArgument,
        "{}",
        cases[1].0
    );
    assert_eq!(
        cases[2].1.code,
        CommandErrorCode::InvalidArgument,
        "{}",
        cases[2].0
    );
    assert_eq!(
        cases[3].1.code,
        CommandErrorCode::PageOutOfRange,
        "{}",
        cases[3].0
    );
    assert_eq!(
        cases[4].1.code,
        CommandErrorCode::InvalidArgument,
        "{}",
        cases[4].0
    );

    assert_eq!(
        update_annotation_style(
            document_id.clone(),
            "missing".to_owned(),
            "#00ff00".to_owned(),
            50,
        )
        .expect_err("unknown annotation")
        .code,
        CommandErrorCode::InvalidArgument
    );
    assert_eq!(
        delete_annotation(document_id.clone(), "missing".to_owned())
            .expect_err("unknown annotation")
            .code,
        CommandErrorCode::InvalidArgument
    );
    assert_eq!(
        undo_text_edit(document_id.clone())
            .expect_err("nothing to undo")
            .code,
        CommandErrorCode::InvalidArgument
    );

    assert!(list_annotations(document_id.clone())
        .expect("list after rejected requests")
        .is_empty());
    let history = edit_history(document_id.clone()).expect("history after rejected requests");
    assert!(!history.dirty, "rejected requests must not create history");
    assert!(!history.can_undo);
    close_document(document_id).expect("close document");
}
