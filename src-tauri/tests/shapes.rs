//! Command-boundary tests for rectangle (Square) and ellipse (Circle) shapes.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is parsed with lopdf and reopened through the command
//! boundary so "the shape is a native annotation with an appearance stream"
//! is checked against a fresh parse, not the in-memory document.

mod support;

use std::path::Path;

use base64::Engine as _;
use lopdf::Object;
use pdfx_lib::annotations::{
    create_shape_from_settings, create_shape_with_author, delete_annotation, list_annotations,
    update_shape_geometry, update_shape_style, AnnotationKind, CreateShapeRequest, ShapeGeometry,
    ShapeKind, ShapeStyle,
};
use pdfx_lib::commands::{close_document, open_document, render_page, CommandErrorCode};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::settings::{default_annotation_author, Settings, SettingsStore};
use pdfx_lib::text_edit::{edit_history, redo_text_edit, undo_text_edit};
use support::synthetic;
use support::temp::TempDir;

fn open_synthetic(pdf: &synthetic::TempPdf) -> String {
    open_document(pdf.path_string(), None)
        .expect("open synthetic PDF")
        .document_id
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

fn geometry(x: f32, y: f32, width: f32, height: f32, rotation: f32) -> ShapeGeometry {
    ShapeGeometry {
        x,
        y,
        width,
        height,
        rotation,
    }
}

fn style(fill: Option<&str>, stroke: Option<&str>, opacity: u8, line_width: f32) -> ShapeStyle {
    ShapeStyle {
        fill_color: fill.map(str::to_owned),
        stroke_color: stroke.map(str::to_owned),
        opacity,
        line_width,
    }
}

fn request(kind: ShapeKind, geometry: ShapeGeometry, style: ShapeStyle) -> CreateShapeRequest {
    CreateShapeRequest {
        page_index: 0,
        kind,
        geometry,
        style,
    }
}

fn save_as(document_id: &str, path: &Path) {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path.to_string_lossy().into_owned()),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: false,
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

fn number(object: &Object) -> f32 {
    object
        .as_float()
        .or_else(|_| object.as_i64().map(|value| value as f32))
        .expect("numeric object")
}

fn components(annotation: &lopdf::Dictionary, key: &[u8]) -> Vec<f32> {
    annotation
        .get(key)
        .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key)))
        .as_array()
        .unwrap_or_else(|_| panic!("{:?} is not an array", String::from_utf8_lossy(key)))
        .iter()
        .map(number)
        .collect()
}

fn scalar(annotation: &lopdf::Dictionary, key: &[u8]) -> f32 {
    number(
        annotation
            .get(key)
            .unwrap_or_else(|_| panic!("missing {:?}", String::from_utf8_lossy(key))),
    )
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

struct DecodedPng {
    width: u32,
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

#[test]
fn a_rectangle_saves_as_a_native_square_annotation_with_appearance_colours_and_border() {
    let dir = TempDir::new("shapes-square");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let mutation = create_shape_with_author(
        document_id.clone(),
        "形状作者".to_owned(),
        request(
            ShapeKind::Square,
            geometry(100.0, 300.0, 120.0, 60.0, 0.0),
            style(Some("#3366cc"), Some("#000000"), 80, 3.0),
        ),
    )
    .expect("create a rectangle");

    let created = mutation.annotation.expect("created annotation view");
    assert_eq!(created.kind, AnnotationKind::Square);
    assert_eq!(created.author.as_deref(), Some("形状作者"));
    assert!(created
        .created_at
        .as_deref()
        .is_some_and(|date| !date.is_empty()));
    assert!(mutation.history.dirty);
    let shape = created.shape.expect("shape view");
    assert_eq!(shape.kind, ShapeKind::Square);
    assert_eq!(shape.geometry, geometry(100.0, 300.0, 120.0, 60.0, 0.0));
    assert_eq!(
        shape.style,
        style(Some("#3366cc"), Some("#000000"), 80, 3.0)
    );

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, AnnotationKind::Square);

    let output = dir.path().join("square.pdf");
    save_as(&document_id, &output);

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let square = find_annotation(&annotations, b"Square");
    assert_eq!(text_of(square.get(b"T").expect("/T author")), "形状作者");
    assert_eq!(text_of(square.get(b"NM").expect("/NM name")), created.name);
    assert_normal_appearance_is_stream(&parsed, square);

    // Display rect (100, 300, 120, 60) on a 792pt-tall page maps to
    // PDF user space [100 432 220 492]. The border widens /Rect a little so
    // the stroke is not clipped by the appearance bounding box.
    let rect = components(square, b"Rect");
    assert_eq!(rect.len(), 4);
    assert!(rect[0] <= 100.01 && rect[1] <= 432.01, "Rect {rect:?}");
    assert!(rect[2] >= 219.99 && rect[3] >= 491.99, "Rect {rect:?}");
    assert!(
        rect[2] - rect[0] < 130.0 && rect[3] - rect[1] < 70.0,
        "Rect {rect:?}"
    );

    let stroke = components(square, b"C");
    assert!((stroke[0] - 0.0).abs() < 0.01, "C {stroke:?}");
    assert!((stroke[1] - 0.0).abs() < 0.01, "C {stroke:?}");
    assert!((stroke[2] - 0.0).abs() < 0.01, "C {stroke:?}");
    let fill = components(square, b"IC");
    assert!((fill[0] - 0.2).abs() < 0.01, "IC {fill:?}");
    assert!((fill[1] - 0.4).abs() < 0.01, "IC {fill:?}");
    assert!((fill[2] - 0.8).abs() < 0.01, "IC {fill:?}");
    assert!((scalar(square, b"CA") - 0.8).abs() < 0.01);
    let border = components(square, b"Border");
    assert!((border[2] - 3.0).abs() < 0.01, "Border {border:?}");

    close_document(document_id).expect("close source");
    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let reopened_annotations =
        list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(reopened_annotations.len(), 1);
    let reopened_shape = reopened_annotations[0]
        .shape
        .as_ref()
        .expect("shape survives reopen");
    assert_eq!(reopened_shape.kind, ShapeKind::Square);
    assert_eq!(
        reopened_shape.geometry,
        geometry(100.0, 300.0, 120.0, 60.0, 0.0)
    );
    assert_eq!(
        reopened_shape.style,
        style(Some("#3366cc"), Some("#000000"), 80, 3.0)
    );

    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let centre = pixels.pixel(160, 330);
    assert!(
        centre.2 > 140 && centre.0 < 130,
        "the fill should paint blue at the shape centre, got {centre:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn a_stroke_only_ellipse_is_a_native_circle_annotation_that_paints_a_ring() {
    let dir = TempDir::new("shapes-circle");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let mutation = create_shape_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            ShapeKind::Circle,
            geometry(150.0, 250.0, 200.0, 120.0, 0.0),
            style(None, Some("#cc0000"), 100, 6.0),
        ),
    )
    .expect("create an ellipse");
    let created = mutation.annotation.expect("created annotation view");
    assert_eq!(created.kind, AnnotationKind::Circle);

    let output = dir.path().join("circle.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let circle = find_annotation(&annotations, b"Circle");
    assert_normal_appearance_is_stream(&parsed, circle);
    // A stroke-only shape has no interior colour entry; a reader that honours
    // /IC would otherwise paint an opaque default fill.
    assert!(
        circle.get(b"IC").is_err(),
        "stroke-only shapes must not carry /IC: {circle:?}"
    );
    let stroke = components(circle, b"C");
    assert!((stroke[0] - 0.8).abs() < 0.01, "C {stroke:?}");
    assert!((stroke[1] - 0.0).abs() < 0.01, "C {stroke:?}");
    assert!((stroke[2] - 0.0).abs() < 0.01, "C {stroke:?}");
    let border = components(circle, b"Border");
    assert!((border[2] - 6.0).abs() < 0.01, "Border {border:?}");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(views.len(), 1);
    let shape = views[0].shape.as_ref().expect("shape survives reopen");
    assert_eq!(shape.kind, ShapeKind::Circle);
    assert_eq!(shape.style, style(None, Some("#cc0000"), 100, 6.0));

    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let red = |pixel: (u8, u8, u8)| pixel.0 > 180 && pixel.1 < 90 && pixel.2 < 90;
    let edge = pixels.pixel(150, 310);
    assert!(
        red(edge),
        "the ellipse stroke should paint at its left edge, got {edge:?}"
    );
    let middle = pixels.pixel(250, 310);
    assert!(
        !red(middle),
        "a stroke-only ellipse must stay transparent inside, got {middle:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn a_fill_only_rectangle_has_no_stroke_and_saves_no_stroke_colour() {
    let dir = TempDir::new("shapes-fill-only");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    create_shape_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            ShapeKind::Square,
            geometry(300.0, 250.0, 100.0, 80.0, 0.0),
            style(Some("#00aa44"), None, 100, 1.0),
        ),
    )
    .expect("create a fill-only rectangle");

    let output = dir.path().join("fill-only.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let square = find_annotation(&annotations, b"Square");
    assert_normal_appearance_is_stream(&parsed, square);
    assert!(
        square.get(b"C").is_err(),
        "a fill-only shape must not carry a stroke colour: {square:?}"
    );
    let fill = components(square, b"IC");
    assert!((fill[0] - 0.0).abs() < 0.01, "IC {fill:?}");
    assert!((fill[1] - (170.0 / 255.0)).abs() < 0.01, "IC {fill:?}");
    assert!((fill[2] - (68.0 / 255.0)).abs() < 0.01, "IC {fill:?}");
    let border = components(square, b"Border");
    assert!((border[2] - 0.0).abs() < 0.01, "Border {border:?}");

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(
        views[0].shape.as_ref().expect("shape").style,
        style(Some("#00aa44"), None, 100, 1.0)
    );
    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let green = |pixel: (u8, u8, u8)| pixel.1 > 130 && pixel.0 < 100 && pixel.2 < 140;
    let centre = pixels.pixel(350, 290);
    assert!(green(centre), "the fill should paint green, got {centre:?}");
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn a_rotated_rectangle_saves_rotated_ink_and_rotated_geometry() {
    let dir = TempDir::new("shapes-rotation");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);

    let created = create_shape_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            ShapeKind::Square,
            geometry(200.0, 300.0, 240.0, 40.0, 0.0),
            style(Some("#1e50c8"), None, 100, 1.0),
        ),
    )
    .expect("create a long rectangle")
    .annotation
    .expect("created view");

    // Rotation is a real transform: the unrotated box is 240x40, the rotated
    // one is a 45-degree diagonal. A CSS-only or /Rect-only rotation would
    // leave the painted ink axis-aligned.
    let rotated = update_shape_geometry(
        document_id.clone(),
        created.name.clone(),
        geometry(200.0, 300.0, 240.0, 40.0, 45.0),
    )
    .expect("rotate the rectangle")
    .annotation
    .expect("rotated view");
    assert_eq!(
        rotated.shape.as_ref().expect("shape").geometry,
        geometry(200.0, 300.0, 240.0, 40.0, 45.0)
    );
    assert_eq!(rotated.author, created.author);
    assert_eq!(rotated.created_at, created.created_at);

    let output = dir.path().join("rotated.pdf");
    save_as(&document_id, &output);
    close_document(document_id).expect("close source");

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    let annotations = page_annotations(&parsed, 1);
    let square = find_annotation(&annotations, b"Square");
    assert_normal_appearance_is_stream(&parsed, square);
    let rect = components(square, b"Rect");
    let rect_width = rect[2] - rect[0];
    let rect_height = rect[3] - rect[1];
    // The axis-aligned bounding box of the rotated 240x40 strip is roughly
    // 198x198. An unrotated rect would be 240x40, so this proves /Rect covers
    // the rotated ink.
    assert!(
        (190.0..=210.0).contains(&rect_width) && (190.0..=210.0).contains(&rect_height),
        "expected a ~198x198 bounding box, got {rect_width}x{rect_height}: {rect:?}"
    );

    let reopened = open_document(output.to_string_lossy().into_owned(), None)
        .expect("reopen the saved output");
    let views = list_annotations(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(
        views[0].shape.as_ref().expect("shape").geometry,
        geometry(200.0, 300.0, 240.0, 40.0, 45.0),
        "the exact editable geometry, including rotation, is restored"
    );

    let pixels = render_pixels(&reopened.document_id, 0, 1.0);
    let blue = |pixel: (u8, u8, u8)| pixel.2 > 140 && pixel.0 < 120;
    let centre = pixels.pixel(320, 320);
    assert!(
        blue(centre),
        "the rotated fill covers its centre, got {centre:?}"
    );
    // 100pt along the 45-degree long axis from the centre is inside the strip.
    let along_axis = pixels.pixel(391, 391);
    assert!(
        blue(along_axis),
        "ink should follow the 45-degree axis, got {along_axis:?}"
    );
    // The corner of the /Rect bounding box is outside the rotated strip.
    let bounding_corner = pixels.pixel(410, 410);
    assert!(
        !blue(bounding_corner),
        "the bounding-box corner must stay unpainted, got {bounding_corner:?}"
    );
    // This point is inside the unrotated rectangle's row but outside the
    // rotated strip, so it catches implementations that only rotate the
    // selection chrome.
    let off_axis = pixels.pixel(410, 320);
    assert!(
        !blue(off_axis),
        "the unrotated footprint must not stay painted, got {off_axis:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
}

#[test]
fn shape_transforms_style_changes_and_deletion_are_undoable() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let original = geometry(100.0, 300.0, 120.0, 60.0, 0.0);
    let original_style = style(Some("#3366cc"), Some("#000000"), 80, 3.0);
    let created = create_shape_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(ShapeKind::Square, original, original_style.clone()),
    )
    .expect("create a shape")
    .annotation
    .expect("created view");

    let transformed = geometry(200.0, 250.0, 180.0, 90.0, 30.0);
    let moved = update_shape_geometry(document_id.clone(), created.name.clone(), transformed)
        .expect("transform the shape")
        .annotation
        .expect("transformed view");
    assert_eq!(moved.name, created.name);
    assert_eq!(moved.shape.as_ref().expect("shape").geometry, transformed);

    let restyled = style(None, Some("#00aa00"), 60, 5.0);
    let styled = update_shape_style(document_id.clone(), created.name.clone(), restyled.clone())
        .expect("restyle the shape")
        .annotation
        .expect("restyled view");
    assert_eq!(styled.shape.as_ref().expect("shape").geometry, transformed);
    assert_eq!(styled.shape.as_ref().expect("shape").style, restyled);
    assert_eq!(styled.author, created.author);
    assert_eq!(styled.created_at, created.created_at);

    undo_text_edit(document_id.clone()).expect("undo restyle");
    let after_style_undo = list_annotations(document_id.clone()).expect("list after style undo");
    let shape = after_style_undo[0].shape.as_ref().expect("shape");
    assert_eq!(shape.style, original_style);
    assert_eq!(
        shape.geometry, transformed,
        "undoing a style keeps the transform"
    );

    undo_text_edit(document_id.clone()).expect("undo transform");
    let after_transform_undo =
        list_annotations(document_id.clone()).expect("list after transform undo");
    let shape = after_transform_undo[0].shape.as_ref().expect("shape");
    assert_eq!(shape.geometry, original);
    assert_eq!(shape.style, original_style);

    undo_text_edit(document_id.clone()).expect("undo creation");
    assert!(list_annotations(document_id.clone())
        .expect("list after undoing creation")
        .is_empty());

    redo_text_edit(document_id.clone()).expect("redo creation");
    redo_text_edit(document_id.clone()).expect("redo transform");
    let redone_transform = list_annotations(document_id.clone()).expect("list after redos");
    let shape = redone_transform[0].shape.as_ref().expect("shape");
    assert_eq!(shape.geometry, transformed);
    assert_eq!(shape.style, original_style);
    assert_eq!(redone_transform[0].created_at, created.created_at);

    redo_text_edit(document_id.clone()).expect("redo restyle");
    let redone_style = list_annotations(document_id.clone()).expect("list after style redo");
    assert_eq!(
        redone_style[0].shape.as_ref().expect("shape").style,
        restyled
    );

    delete_annotation(document_id.clone(), created.name.clone()).expect("delete the shape");
    assert!(list_annotations(document_id.clone())
        .expect("list after delete")
        .is_empty());
    assert!(edit_history(document_id.clone()).expect("history").dirty);

    undo_text_edit(document_id.clone()).expect("undo deletion");
    let restored = list_annotations(document_id.clone()).expect("list after undo deletion");
    assert_eq!(restored.len(), 1);
    let view = &restored[0];
    assert_eq!(view.name, created.name, "the stable name is restored");
    assert_eq!(view.author.as_deref(), Some("作者"));
    assert_eq!(view.created_at, created.created_at);
    let shape = view.shape.as_ref().expect("restored shape");
    assert_eq!(shape.geometry, transformed);
    assert_eq!(shape.style, restyled);

    redo_text_edit(document_id.clone()).expect("redo deletion");
    assert!(list_annotations(document_id.clone())
        .expect("list after redo deletion")
        .is_empty());
    close_document(document_id).expect("close document");
}

#[test]
fn shapes_take_their_author_from_settings_at_creation_time() {
    let dir = TempDir::new("shapes-author");
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let store = SettingsStore::new(dir.path());
    let request = || {
        request(
            ShapeKind::Circle,
            geometry(100.0, 300.0, 120.0, 60.0, 0.0),
            style(Some("#3366cc"), None, 100, 1.0),
        )
    };

    let mut settings = Settings {
        annotation_author: "Alice".to_owned(),
        ..Settings::default()
    };
    store.save(&settings).expect("persist settings");
    let alice = create_shape_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Alice")
        .annotation
        .expect("Alice's shape");
    assert_eq!(alice.author.as_deref(), Some("Alice"));

    settings.annotation_author = "Bob".to_owned();
    store.save(&settings).expect("persist changed author");
    let bob = create_shape_from_settings(document_id.clone(), &store.load(), request())
        .expect("create as Bob")
        .annotation
        .expect("Bob's shape");
    assert_eq!(bob.author.as_deref(), Some("Bob"));

    let listed = list_annotations(document_id.clone()).expect("list annotations");
    assert_eq!(
        listed
            .iter()
            .find(|view| view.name == alice.name)
            .and_then(|view| view.author.as_deref()),
        Some("Alice"),
        "changing the setting must not rewrite existing shapes"
    );
    let default_author =
        create_shape_from_settings(document_id.clone(), &Settings::default(), request())
            .expect("create with the default author")
            .annotation
            .expect("default-author shape")
            .author;
    assert_eq!(default_author, Some(default_annotation_author()));
    close_document(document_id).expect("close document");
}

#[test]
fn read_only_documents_refuse_shape_changes_but_still_list_them() {
    let document_id = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("open the permission-restricted fixture")
        .document_id;

    let create_error = create_shape_with_author(
        document_id.clone(),
        "作者".to_owned(),
        request(
            ShapeKind::Square,
            geometry(10.0, 10.0, 20.0, 20.0, 0.0),
            style(Some("#3366cc"), Some("#000000"), 50, 2.0),
        ),
    )
    .expect_err("shape annotations are denied");
    assert_eq!(create_error.code, CommandErrorCode::PermissionDenied);

    let update_error = update_shape_geometry(
        document_id.clone(),
        "missing".to_owned(),
        geometry(10.0, 10.0, 20.0, 20.0, 0.0),
    )
    .expect_err("transforms are denied");
    assert_eq!(update_error.code, CommandErrorCode::PermissionDenied);

    let style_error = update_shape_style(
        document_id.clone(),
        "missing".to_owned(),
        style(Some("#3366cc"), None, 50, 2.0),
    )
    .expect_err("restyling is denied");
    assert_eq!(style_error.code, CommandErrorCode::PermissionDenied);

    let delete_error = delete_annotation(document_id.clone(), "missing".to_owned())
        .expect_err("deleting is denied");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);

    assert!(
        list_annotations(document_id.clone())
            .expect("listing is allowed")
            .is_empty(),
        "listing stays available on restricted documents"
    );
    close_document(document_id).expect("close document");
}

#[test]
fn invalid_shape_requests_are_rejected_without_dirtying_the_document() {
    let pdf = synthetic::single_line_pdf();
    let document_id = open_synthetic(&pdf);
    let valid = || {
        request(
            ShapeKind::Square,
            geometry(10.0, 10.0, 20.0, 20.0, 0.0),
            style(Some("#3366cc"), Some("#000000"), 50, 2.0),
        )
    };
    let invalid = |label: &str, request: CreateShapeRequest| {
        let error = create_shape_with_author(document_id.clone(), "作者".to_owned(), request)
            .expect_err(label);
        assert_eq!(error.code, CommandErrorCode::InvalidArgument, "{label}");
    };

    invalid("zero width", {
        let mut request = valid();
        request.geometry.width = 0.0;
        request
    });
    invalid("negative height", {
        let mut request = valid();
        request.geometry.height = -10.0;
        request
    });
    invalid("NaN rotation", {
        let mut request = valid();
        request.geometry.rotation = f32::NAN;
        request
    });
    invalid("neither fill nor stroke", {
        let mut request = valid();
        request.style.fill_color = None;
        request.style.stroke_color = None;
        request
    });
    invalid("invalid fill colour", {
        let mut request = valid();
        request.style.fill_color = Some("blue".to_owned());
        request
    });
    invalid("opacity above 100", {
        let mut request = valid();
        request.style.opacity = 101;
        request
    });
    invalid("zero line width with a stroke", {
        let mut request = valid();
        request.style.line_width = 0.0;
        request
    });
    invalid("line width above the maximum", {
        let mut request = valid();
        request.style.line_width = 101.0;
        request
    });

    let page_error = create_shape_with_author(document_id.clone(), "作者".to_owned(), {
        let mut request = valid();
        request.page_index = 7;
        request
    })
    .expect_err("out-of-range page must fail");
    assert_eq!(page_error.code, CommandErrorCode::PageOutOfRange);

    let author_error = create_shape_with_author(document_id.clone(), "   ".to_owned(), valid())
        .expect_err("empty author must fail");
    assert_eq!(author_error.code, CommandErrorCode::InvalidArgument);

    let missing_error = update_shape_geometry(
        document_id.clone(),
        "missing".to_owned(),
        geometry(10.0, 10.0, 20.0, 20.0, 0.0),
    )
    .expect_err("unknown annotation");
    assert_eq!(missing_error.code, CommandErrorCode::InvalidArgument);

    assert!(list_annotations(document_id.clone())
        .expect("list after rejected requests")
        .is_empty());
    let history = edit_history(document_id.clone()).expect("history after rejected requests");
    assert!(!history.dirty, "rejected requests must not create history");
    assert!(!history.can_undo);
    close_document(document_id).expect("close document");
}
