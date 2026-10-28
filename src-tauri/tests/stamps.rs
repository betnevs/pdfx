//! Command-boundary tests for built-in stamps.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is parsed with lopdf and reopened through the command
//! boundary, so "the stamp is a native Stamp annotation with an appearance
//! stream" and the rendered pixels are checked against a fresh parse rather
//! than the in-memory document.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_stamp_with_author, create_text_markup_with_author, delete_annotation, list_annotations,
    transform_stamp_annotation, update_annotation_style, AnnotationKind, CreateStampRequest,
    CreateTextMarkupRequest, TextMarkupKind, TextMarkupRect, TransformStampRequest,
};
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::settings::{Settings, SettingsStore};
use pdfx_lib::stamps::{StampFrameView, StampKind};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};
use support::fixtures;
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
}

fn frame(x: f32, y: f32, width: f32, height: f32, rotation: f32) -> StampFrameView {
    StampFrameView {
        x,
        y,
        width,
        height,
        rotation,
    }
}

fn stamp_request(kind: StampKind, frame: StampFrameView) -> CreateStampRequest {
    CreateStampRequest {
        page_index: 0,
        kind,
        opacity: 100,
        frame,
    }
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            password: None,
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

fn appearance_content(document: &lopdf::Document, annotation: &lopdf::Dictionary) -> String {
    let appearance = annotation
        .get(b"AP")
        .expect("/AP appearance dictionary")
        .as_dict()
        .expect("appearance dictionary");
    let normal = match appearance.get(b"N").expect("normal appearance") {
        Object::Reference(id) => document.get_object(*id).expect("appearance stream"),
        object => object,
    };
    let stream = normal.as_stream().expect("normal appearance stream");
    String::from_utf8_lossy(
        &stream
            .decompressed_content()
            .expect("appearance stream content"),
    )
    .into_owned()
}

fn rect_of(annotation: &lopdf::Dictionary) -> [f32; 4] {
    let values: Vec<f32> = annotation
        .get(b"Rect")
        .expect("/Rect")
        .as_array()
        .expect("/Rect array")
        .iter()
        .map(|value| value.as_float().expect("numeric /Rect"))
        .collect();
    assert_eq!(values.len(), 4, "/Rect has four values");
    [values[0], values[1], values[2], values[3]]
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

fn assert_close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 0.1,
        "{label}: expected {expected}, got {actual}"
    );
}

fn assert_frame_close(actual: &StampFrameView, expected: &StampFrameView) {
    assert_close(actual.x, expected.x, "frame x");
    assert_close(actual.y, expected.y, "frame y");
    assert_close(actual.width, expected.width, "frame width");
    assert_close(actual.height, expected.height, "frame height");
    assert_close(actual.rotation, expected.rotation, "frame rotation");
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

    fn count_in_rect(
        &self,
        rect: &StampFrameView,
        matches: impl Fn((u8, u8, u8)) -> bool,
    ) -> usize {
        let left = rect.x.floor().max(0.0) as u32;
        let top = rect.y.floor().max(0.0) as u32;
        let right = (rect.x + rect.width).ceil().max(0.0) as u32;
        let bottom = (rect.y + rect.height).ceil().max(0.0) as u32;
        let mut count = 0;
        for y in top..bottom {
            for x in left..right {
                if matches(self.pixel(x, y)) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Smallest red channel among pixels matching `matches` inside `rect`.
    fn min_red_in_rect(
        &self,
        rect: &StampFrameView,
        matches: impl Fn((u8, u8, u8)) -> bool,
    ) -> Option<u8> {
        let left = rect.x.floor().max(0.0) as u32;
        let top = rect.y.floor().max(0.0) as u32;
        let right = (rect.x + rect.width).ceil().max(0.0) as u32;
        let bottom = (rect.y + rect.height).ceil().max(0.0) as u32;
        let mut minimum = None;
        for y in top..bottom {
            for x in left..right {
                let pixel = self.pixel(x, y);
                if matches(pixel) {
                    minimum = Some(minimum.map_or(pixel.0, |current: u8| current.min(pixel.0)));
                }
            }
        }
        minimum
    }
}

fn green(pixel: (u8, u8, u8)) -> bool {
    let (red, green, blue) = pixel;
    green > red.saturating_add(30) && green > blue.saturating_add(30)
}

#[test]
fn a_stamp_saves_as_a_native_stamp_annotation_with_appearance_author_and_ink() {
    let dir = TempDir::new("stamp-approved");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let placement = frame(200.0, 300.0, 180.0, 60.0, 0.0);

    let mutation = create_stamp_with_author(
        document_id.clone(),
        "测试作者".to_owned(),
        stamp_request(StampKind::Approved, placement),
    )
    .expect("place approved stamp");
    let created = mutation.annotation.expect("created stamp view");
    assert_eq!(created.kind, AnnotationKind::Stamp);
    assert_eq!(created.author.as_deref(), Some("测试作者"));
    assert_eq!(created.color, "#15803d");
    assert_eq!(created.opacity, 100);
    assert!(
        created
            .created_at
            .as_deref()
            .is_some_and(|date| !date.is_empty()),
        "the stamp must record a creation time"
    );
    let stamp = created.stamp.clone().expect("stamp design");
    assert_eq!(stamp.kind, StampKind::Approved);
    assert_frame_close(&stamp.frame, &frame(200.0, 300.0, 180.0, 60.0, 0.0));
    assert_eq!(created.rects.len(), 1);

    let output = dir.path().join("approved.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let native = find_annotation(&annotations, b"Stamp");
    assert_eq!(
        text_of(native.get(b"T").expect("/T author")),
        "测试作者",
        "the author must be the native /T entry"
    );
    assert!(
        native.get(b"CreationDate").is_ok(),
        "the stamp must record /CreationDate"
    );
    assert_eq!(
        text_of(native.get(b"NM").expect("/NM name")),
        created.name,
        "the stable annotation name must survive save"
    );
    assert_eq!(
        text_of(native.get(b"PDFXStamp").expect("/PDFXStamp kind")),
        "approved"
    );
    // Display (200, 300, 180x60) on a 612x792 page maps to PDF bounds
    // [200 432 380 492].
    let bounds = rect_of(native);
    assert_close(bounds[0], 200.0, "left");
    assert_close(bounds[1], 432.0, "bottom");
    assert_close(bounds[2], 380.0, "right");
    assert_close(bounds[3], 492.0, "top");
    let colour = colour_components(native);
    assert_close(colour[0], 21.0 / 255.0, "red");
    assert_close(colour[1], 128.0 / 255.0, "green");
    assert_close(colour[2], 61.0 / 255.0, "blue");
    let alpha = native
        .get(b"CA")
        .expect("/CA opacity")
        .as_float()
        .expect("numeric /CA");
    assert_close(alpha, 1.0, "opacity");
    assert_normal_appearance_is_stream(&parsed, native);
    let stream = appearance_content(&parsed, native);
    assert!(
        stream.contains(" cm") && stream.contains(" RG") && stream.contains("\nf"),
        "the appearance must place and paint the vector badge, got {stream:?}"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views =
        list_annotations(reopened.document_id.clone()).expect("list reopened stamps");
    assert_eq!(reopened_views.len(), 1);
    let reopened_stamp = reopened_views[0].stamp.expect("reopened stamp design");
    assert_eq!(reopened_stamp.kind, StampKind::Approved);
    assert_frame_close(
        &reopened_stamp.frame,
        &frame(200.0, 300.0, 180.0, 60.0, 0.0),
    );

    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let ink = pixels.count_in_rect(&frame(200.0, 300.0, 180.0, 60.0, 0.0), green);
    assert!(
        ink > 200,
        "the approved stamp must render its green border and label, found {ink} green pixels"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn all_four_builtin_stamps_save_render_and_reload_with_their_own_ink() {
    let dir = TempDir::new("stamp-all-kinds");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let placements = [
        (StampKind::Approved, frame(60.0, 120.0, 180.0, 60.0, 0.0)),
        (StampKind::Draft, frame(300.0, 120.0, 180.0, 60.0, 0.0)),
        (
            StampKind::Confidential,
            frame(60.0, 240.0, 180.0, 60.0, 0.0),
        ),
        (StampKind::Rejected, frame(300.0, 240.0, 180.0, 60.0, 0.0)),
    ];
    for (kind, placement) in placements {
        let mutation = create_stamp_with_author(
            document_id.clone(),
            "作者".to_owned(),
            stamp_request(kind, placement),
        )
        .unwrap_or_else(|error| panic!("place {kind:?}: {error:?}"));
        let view = mutation.annotation.expect("created stamp");
        assert_eq!(view.stamp.expect("stamp design").kind, kind);
    }

    let listed = list_annotations(document_id.clone()).expect("list stamps");
    assert_eq!(listed.len(), 4);
    for (kind, _) in placements {
        assert!(
            listed
                .iter()
                .any(|view| view.stamp.is_some_and(|stamp| stamp.kind == kind)),
            "the list must contain the {kind:?} stamp"
        );
    }

    let output = dir.path().join("all-stamps.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let stamps: Vec<_> = annotations
        .iter()
        .filter(|annotation| {
            annotation
                .get(b"Subtype")
                .ok()
                .and_then(|object| object.as_name().ok())
                .is_some_and(|name| name == b"Stamp")
        })
        .collect();
    assert_eq!(stamps.len(), 4, "four native Stamp annotations");
    let mut saved_kinds: Vec<String> = stamps
        .iter()
        .map(|annotation| text_of(annotation.get(b"PDFXStamp").expect("/PDFXStamp")))
        .collect();
    saved_kinds.sort();
    assert_eq!(
        saved_kinds,
        ["approved", "confidential", "draft", "rejected"],
        "every built-in kind saves its identity"
    );
    for annotation in &stamps {
        assert_normal_appearance_is_stream(&parsed, annotation);
    }

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let approved = pixels.count_in_rect(&placements[0].1, |(red, green, blue)| {
        green > red.saturating_add(30) && green > blue.saturating_add(30)
    });
    let draft = pixels.count_in_rect(&placements[1].1, |(red, green, blue)| {
        blue > red.saturating_add(20) && blue > green.saturating_add(10) && red < 150
    });
    let confidential = pixels.count_in_rect(&placements[2].1, |(red, green, blue)| {
        red > green.saturating_add(60) && red > blue.saturating_add(60)
    });
    let rejected = pixels.count_in_rect(&placements[3].1, |(red, green, blue)| {
        red > green.saturating_add(40) && red > blue.saturating_add(40) && red < 200
    });
    assert!(approved > 200, "approved stamp ink: {approved}");
    assert!(draft > 200, "draft stamp ink: {draft}");
    assert!(confidential > 200, "confidential stamp ink: {confidential}");
    assert!(rejected > 200, "rejected stamp ink: {rejected}");

    let reopened_views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_views.len(), 4);
    assert_eq!(
        reopened_views
            .iter()
            .filter(|view| view.kind == AnnotationKind::Stamp)
            .count(),
        4
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn moving_resizing_and_rotating_a_stamp_is_one_undoable_step() {
    let dir = TempDir::new("stamp-transform");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let original = frame(200.0, 300.0, 180.0, 60.0, 0.0);
    let created = create_stamp_with_author(
        document_id.clone(),
        "作者".to_owned(),
        stamp_request(StampKind::Confidential, original),
    )
    .expect("place stamp")
    .annotation
    .expect("created stamp");

    let transformed = frame(300.0, 200.0, 240.0, 80.0, 30.0);
    let mutation = transform_stamp_annotation(
        document_id.clone(),
        TransformStampRequest {
            name: created.name.clone(),
            frame: transformed,
        },
    )
    .expect("transform stamp");
    let moved = mutation.annotation.expect("transformed stamp");
    assert_eq!(moved.name, created.name, "transform keeps the identity");
    assert_eq!(moved.author, created.author);
    assert_eq!(moved.created_at, created.created_at);
    assert_frame_close(&moved.stamp.expect("stamp design").frame, &transformed);
    assert_eq!(moved.opacity, 100);
    assert_eq!(moved.color, "#b91c1c");

    let saved = {
        let output = dir.path().join("transformed.pdf");
        save_as(&document_id, &output);
        lopdf::Document::load(&output).expect("lopdf parses the output")
    };
    assert_normal_appearance_is_stream(
        &saved,
        find_annotation(&page_annotations(&saved, 1), b"Stamp"),
    );
    let stream = appearance_content(
        &saved,
        find_annotation(&page_annotations(&saved, 1), b"Stamp"),
    );
    // The rotated 240 x 80 frame has axis-aligned bounds of about
    // 240*cos30 + 80*sin30 = 247.85 wide and 240*sin30 + 80*cos30 = 189.28 tall.
    let bounds = rect_of(find_annotation(&page_annotations(&saved, 1), b"Stamp"));
    assert_close(bounds[2] - bounds[0], 247.85, "rotated bounds width");
    assert_close(bounds[3] - bounds[1], 189.28, "rotated bounds height");
    assert!(
        stream.contains(" cm"),
        "the appearance must carry the rotation matrix"
    );

    undo_text_edit(document_id.clone()).expect("undo transform");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 1);
    assert_frame_close(
        &restored[0].stamp.as_ref().expect("stamp design").frame,
        &original,
    );

    redo_text_edit(document_id.clone()).expect("redo transform");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_frame_close(
        &redone[0].stamp.as_ref().expect("stamp design").frame,
        &transformed,
    );

    let output = dir.path().join("transformed-round-trip.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_views.len(), 1);
    assert_frame_close(
        &reopened_views[0]
            .stamp
            .as_ref()
            .expect("stamp design")
            .frame,
        &transformed,
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn stamp_opacity_is_undoable_and_visible_in_rendered_pixels() {
    let dir = TempDir::new("stamp-opacity");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let placement = frame(200.0, 300.0, 180.0, 60.0, 0.0);
    let created = create_stamp_with_author(
        document_id.clone(),
        "作者".to_owned(),
        stamp_request(StampKind::Approved, placement),
    )
    .expect("place stamp")
    .annotation
    .expect("created stamp");

    let opaque_pixels = render_pixels(&document_id, 0, 1.0);
    let opaque_min_red = opaque_pixels
        .min_red_in_rect(&placement, green)
        .expect("full-opacity green ink");

    // A stamp keeps its built-in ink; only opacity is taken from the panel.
    let updated = update_annotation_style(
        document_id.clone(),
        created.name.clone(),
        "#000000".to_owned(),
        30,
        None,
    )
    .expect("change stamp opacity")
    .annotation
    .expect("updated stamp");
    assert_eq!(updated.opacity, 30);
    assert_eq!(updated.color, "#15803d", "stamp ink must not be recoloured");

    let light_pixels = render_pixels(&document_id, 0, 1.0);
    let light_min_red = light_pixels
        .min_red_in_rect(&placement, |(red, green, blue)| {
            green > red.saturating_add(15) && green > blue.saturating_add(15)
        })
        .expect("30%-opacity green ink");
    assert!(
        light_min_red > opaque_min_red.saturating_add(40),
        "30% opacity must blend the ink towards white: full {opaque_min_red}, 30% {light_min_red}"
    );

    undo_text_edit(document_id.clone()).expect("undo opacity change");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored[0].opacity, 100);
    redo_text_edit(document_id.clone()).expect("redo opacity change");
    let redone = list_annotations(document_id.clone()).expect("list after redo");
    assert_eq!(redone[0].opacity, 30);

    let output = dir.path().join("opacity.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let native = find_annotation(&annotations, b"Stamp");
    let alpha = native
        .get(b"CA")
        .expect("/CA opacity")
        .as_float()
        .expect("numeric /CA");
    assert_close(alpha, 0.3, "opacity");

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_views[0].opacity, 30);
    assert_eq!(reopened_views[0].color, "#15803d");
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn deleting_a_stamp_is_undoable_and_restores_its_kind_frame_and_author() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let created = create_stamp_with_author(
        document_id.clone(),
        "甲".to_owned(),
        stamp_request(StampKind::Draft, frame(120.0, 420.0, 200.0, 70.0, 15.0)),
    )
    .expect("place stamp")
    .annotation
    .expect("created stamp");

    let deleted =
        delete_annotation(document_id.clone(), created.name.clone()).expect("delete stamp");
    assert!(deleted.annotation.is_none());
    assert!(list_annotations(document_id.clone())
        .expect("list after delete")
        .is_empty());

    undo_text_edit(document_id.clone()).expect("undo deletion");
    let restored = list_annotations(document_id.clone()).expect("list after undo");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].name, created.name);
    assert_eq!(restored[0].author.as_deref(), Some("甲"));
    assert_eq!(restored[0].created_at, created.created_at);
    let restored_stamp = restored[0].stamp.expect("stamp design");
    assert_eq!(restored_stamp.kind, StampKind::Draft);
    assert_frame_close(
        &restored_stamp.frame,
        &frame(120.0, 420.0, 200.0, 70.0, 15.0),
    );

    redo_text_edit(document_id.clone()).expect("redo deletion");
    assert!(list_annotations(document_id.clone())
        .expect("list after redo")
        .is_empty());
    close_document(document_id).expect("close document");
}

#[test]
fn stamps_take_their_author_from_settings_at_creation_time() {
    let dir = TempDir::new("stamp-author");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let store = SettingsStore::new(dir.path());
    let request = || stamp_request(StampKind::Approved, frame(100.0, 300.0, 180.0, 60.0, 0.0));

    let mut settings = Settings {
        annotation_author: "Alice".to_owned(),
        ..Settings::default()
    };
    store.save(&settings).expect("persist settings");
    let alice = pdfx_lib::annotations::create_stamp_from_settings(
        document_id.clone(),
        &store.load(),
        request(),
    )
    .expect("create as Alice")
    .annotation
    .expect("Alice's stamp");
    assert_eq!(alice.author.as_deref(), Some("Alice"));

    settings.annotation_author = "Bob".to_owned();
    store.save(&settings).expect("persist changed author");
    let bob = pdfx_lib::annotations::create_stamp_from_settings(
        document_id.clone(),
        &store.load(),
        request(),
    )
    .expect("create as Bob")
    .annotation
    .expect("Bob's stamp");
    assert_eq!(bob.author.as_deref(), Some("Bob"));

    let listed = list_annotations(document_id.clone()).expect("list stamps");
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == alice.name)
            .and_then(|view| view.author.as_deref()),
        Some("Alice"),
        "changing the setting must not rewrite existing stamps"
    );
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == bob.name)
            .and_then(|view| view.author.as_deref()),
        Some("Bob")
    );
    close_document(document_id).expect("close document");
}

#[test]
fn a_stamp_on_a_rotated_cropped_page_round_trips_its_frame() {
    let dir = TempDir::new("stamp-rotated-page");
    let pdf = synthetic::rotated_crop_pdf();
    let document_id = open_synthetic(&pdf);
    // Display page is 600 x 400 because the [50 80 450 680] CropBox is rotated
    // 90 degrees on page one.
    let placement = frame(150.0, 120.0, 180.0, 60.0, 25.0);
    create_stamp_with_author(
        document_id.clone(),
        "作者".to_owned(),
        stamp_request(StampKind::Rejected, placement),
    )
    .expect("place stamp on rotated page");

    let output = dir.path().join("rotated.pdf");
    save_as(&document_id, &output);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let native = find_annotation(&annotations, b"Stamp");
    assert_normal_appearance_is_stream(&parsed, native);
    assert_eq!(
        text_of(native.get(b"PDFXStamp").expect("/PDFXStamp kind")),
        "rejected"
    );

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_views.len(), 1);
    assert_frame_close(
        &reopened_views[0]
            .stamp
            .as_ref()
            .expect("stamp design")
            .frame,
        &placement,
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn read_only_documents_refuse_stamp_changes() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;

    let create_error = create_stamp_with_author(
        document_id.clone(),
        "作者".to_owned(),
        stamp_request(StampKind::Approved, frame(10.0, 10.0, 180.0, 60.0, 0.0)),
    )
    .expect_err("stamps are denied");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);
    assert!(list_annotations(document_id.clone())
        .expect("listing is allowed")
        .is_empty());

    let transform_error = transform_stamp_annotation(
        document_id.clone(),
        TransformStampRequest {
            name: "missing".to_owned(),
            frame: frame(10.0, 10.0, 180.0, 60.0, 0.0),
        },
    )
    .expect_err("transforming is denied");
    assert_eq!(transform_error.code, CommandErrorCode::PermissionDenied);

    let update_error = update_annotation_style(
        document_id.clone(),
        "missing".to_owned(),
        "#15803d".to_owned(),
        50,
        None,
    )
    .expect_err("restyling is denied");
    assert_eq!(update_error.code, CommandErrorCode::PermissionDenied);

    let delete_error = delete_annotation(document_id.clone(), "missing".to_owned())
        .expect_err("deleting is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_stamp_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid = frame(100.0, 300.0, 180.0, 60.0, 0.0);
    let request = |mutate: &dyn Fn(&mut CreateStampRequest)| {
        let mut request = stamp_request(StampKind::Approved, valid);
        mutate(&mut request);
        request
    };

    let cases = [
        (
            "opacity above 100",
            create_stamp_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.opacity = 101),
            )
            .expect_err("invalid opacity must fail"),
        ),
        (
            "zero width",
            create_stamp_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.frame.width = 0.0),
            )
            .expect_err("zero width must fail"),
        ),
        (
            "zero height",
            create_stamp_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.frame.height = 0.0),
            )
            .expect_err("zero height must fail"),
        ),
        (
            "non-finite rotation",
            create_stamp_with_author(
                document_id.clone(),
                "作者".to_owned(),
                request(&|request| request.frame.rotation = f32::NAN),
            )
            .expect_err("non-finite geometry must fail"),
        ),
        (
            "empty author",
            create_stamp_with_author(document_id.clone(), "   ".to_owned(), request(&|_| {}))
                .expect_err("empty author must fail"),
        ),
    ];
    for (case, error) in cases {
        assert_eq!(
            error.code,
            CommandErrorCode::InvalidArgument,
            "{case} must be an invalid argument"
        );
    }

    // A text markup annotation is not a stamp.
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
                width: 120.0,
                height: 30.0,
            }],
        },
    )
    .expect("create highlight")
    .annotation
    .expect("highlight view");
    let transform_error = transform_stamp_annotation(
        document_id.clone(),
        TransformStampRequest {
            name: markup.name,
            frame: valid,
        },
    )
    .expect_err("text markup cannot be transformed as a stamp");
    assert_eq!(transform_error.code, CommandErrorCode::InvalidArgument);

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1, "rejected requests must not add stamps");
    assert_eq!(listed[0].kind, AnnotationKind::Highlight);
    close_document(document_id).expect("close document");
}
