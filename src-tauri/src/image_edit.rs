//! Inserting, moving, resizing, rotating and deleting raster images.
//!
//! Images are PDFium page objects, so they travel through the same save
//! pipeline as Text Boxes: `FPDF_INCREMENTAL` writes them as one revision and
//! the Source Document is never touched. The page-object snapshots kept by the
//! unified [`crate::text_edit::EditSession`] make every image action undoable
//! and redoable, including after a save.
//!
//! # Out-of-page policy
//!
//! Display space is the cropped, rotated page box. The user may move an image
//! so that part of it leaves that box; PDF rendering clips content to the page,
//! and the overlay follows the same rule (`overflow: hidden`) while showing a
//! visible warning. Clipping is therefore a property of the page, not a
//! destructive edit: moving the image back restores the hidden area, and the
//! saved document keeps the full image with its original geometry.
//!
//! # Accepted input
//!
//! Only PNG and JPEG files are read. The extension selects the decoder and the
//! file's magic bytes must agree with it. Files above [`MAX_IMAGE_BYTES`] or
//! [`MAX_IMAGE_PIXELS`] are rejected before the document is touched, as are
//! malformed files, so a failed insertion never leaves a partial page edit.
//!
//! # Signatures
//!
//! [`insert_signature_image`] accepts the same PNG/JPEG bytes in memory rather
//! than a file path, because the signature dialog draws or generates the image
//! in the frontend. Signatures are still ordinary image page objects (same
//! save, alpha and undo path); the [`ImageKind`] label only distinguishes them
//! in the overlay and in the image list. They use a smaller default rectangle
//! so a signature does not cover most of the page.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfMatrix, PdfPageImageObject, PdfPageObject,
    PdfPageObjectsCommon,
};
use serde::{Deserialize, Serialize};

use crate::commands::{
    load_page, with_open_document, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument,
};
use crate::text_edit::{
    detach_and_insert, ensure_content_editable, shift_object_indexes, EditAction, EditHistoryView,
    EditSession, EditStepView, ImageEditAction, ObjectRef,
};

/// Largest image file accepted from disk. Larger files are rejected before
/// they are read into memory.
pub(crate) const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

/// Largest decoded image accepted. A 20 MP cap keeps the transient RGB +
/// BGRA buffers of a JPEG decode comfortably below 150 MB.
pub(crate) const MAX_IMAGE_PIXELS: u64 = 20_000_000;

/// Fraction of the page's display width and height a new photo may occupy.
const DEFAULT_FIT_RATIO: f32 = 0.8;

/// Fraction of the page's display width and height a new signature may occupy.
///
/// A signature is a personal mark, not a picture: filling 80% of the page (as
/// a photo does) would place a six-inch-wide signature on Letter paper. The
/// rectangle stays aspect-preserved and centered, so the user can still move,
/// scale and rotate it with the normal image controls.
const SIGNATURE_FIT_WIDTH: f32 = 0.35;
const SIGNATURE_FIT_HEIGHT: f32 = 0.25;

/// Minimum display-space size of an image, in points.
const MIN_IMAGE_SIZE: f32 = 1.0;

/// Upper bound for a display-space size, protecting the PDF matrix from
/// nonsensical values while staying far above any real page.
const MAX_IMAGE_SIZE: f32 = 100_000.0;

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_MAGIC: [u8; 3] = [0xff, 0xd8, 0xff];

/// Base64 length that encodes [`MAX_IMAGE_BYTES`], padding included. Checked
/// before decoding so an oversized payload is rejected without allocating its
/// decoded size.
const MAX_SIGNATURE_BASE64_LEN: usize = (MAX_IMAGE_BYTES as usize).div_ceil(3) * 4;

/// Longest signature name accepted, counted in Unicode scalar values.
const MAX_SIGNATURE_NAME_CHARS: usize = 80;

/// The two image encodings accepted by the insertion tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
}

/// What a page image represents.
///
/// Both kinds are ordinary PDFium image page objects and share the same save,
/// alpha, transform and undo path. The label lets the overlay show a signature
/// badge and lets the frontend list/filter them without a second object type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageKind {
    Image,
    Signature,
}

/// Pixel position, size and clockwise rotation of an image, in display-space
/// points with a top-left origin. Rotation is around the rectangle's center.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageLayout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) rotation: f32,
}

/// A decoded image ready to be placed on a page.
#[derive(Clone)]
pub(crate) struct DecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Tightly packed BGRA rows, matching [`PdfBitmapFormat::BGRA`].
    pub(crate) bgra: Vec<u8>,
}

/// One image this session inserted on a page.
pub(crate) struct ImageRecord {
    pub(crate) image_id: u64,
    /// Index of the image's page object.
    pub(crate) start: usize,
    /// 1 while the object is on the page, 0 while deleted or after an undone
    /// insertion.
    pub(crate) count: usize,
    pub(crate) layout: ImageLayout,
    pub(crate) name: String,
    pub(crate) kind: ImageKind,
    pub(crate) pixel_width: u32,
    pub(crate) pixel_height: u32,
    pub(crate) format: ImageFormat,
    /// The original file bytes. Transforms rebuild the page object from them,
    /// because PDFium page objects cannot be copied.
    pub(crate) source: Arc<Vec<u8>>,
    pub(crate) deleted: bool,
}

/// The complete record state an undo/redo step restores.
#[derive(Debug, Clone)]
pub(crate) struct ImageSnapshot {
    pub(crate) image_id: u64,
    pub(crate) start: usize,
    pub(crate) count: usize,
    pub(crate) layout: ImageLayout,
    pub(crate) name: String,
    pub(crate) kind: ImageKind,
    pub(crate) pixel_width: u32,
    pub(crate) pixel_height: u32,
    pub(crate) format: ImageFormat,
    pub(crate) source: Arc<Vec<u8>>,
    pub(crate) deleted: bool,
    pub(crate) record_index: usize,
}

impl ImageRecord {
    fn snapshot(&self, record_index: usize) -> ImageSnapshot {
        ImageSnapshot {
            image_id: self.image_id,
            start: self.start,
            count: self.count,
            layout: self.layout.clone(),
            name: self.name.clone(),
            kind: self.kind,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
            format: self.format,
            source: Arc::clone(&self.source),
            deleted: self.deleted,
            record_index,
        }
    }

    fn restore(&mut self, snapshot: &ImageSnapshot) {
        self.image_id = snapshot.image_id;
        self.start = snapshot.start;
        self.count = snapshot.count;
        self.layout = snapshot.layout.clone();
        self.name = snapshot.name.clone();
        self.kind = snapshot.kind;
        self.pixel_width = snapshot.pixel_width;
        self.pixel_height = snapshot.pixel_height;
        self.format = snapshot.format;
        self.source = Arc::clone(&snapshot.source);
        self.deleted = snapshot.deleted;
    }

    fn from_snapshot(snapshot: &ImageSnapshot) -> Self {
        Self {
            image_id: snapshot.image_id,
            start: snapshot.start,
            count: snapshot.count,
            layout: snapshot.layout.clone(),
            name: snapshot.name.clone(),
            kind: snapshot.kind,
            pixel_width: snapshot.pixel_width,
            pixel_height: snapshot.pixel_height,
            format: snapshot.format,
            source: Arc::clone(&snapshot.source),
            deleted: snapshot.deleted,
        }
    }

    fn view(&self, page_index: u32) -> ImageObjectView {
        ImageObjectView {
            image_id: self.image_id,
            page_index,
            name: self.name.clone(),
            kind: self.kind,
            x: self.layout.x,
            y: self.layout.y,
            width: self.layout.width,
            height: self.layout.height,
            rotation: self.layout.rotation,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        }
    }
}

/// One image as the frontend overlay sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageObjectView {
    pub image_id: u64,
    pub page_index: u32,
    pub name: String,
    /// Whether this object is a photo or a signature.
    pub kind: ImageKind,
    /// Unrotated top-left corner in display-space points.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Clockwise rotation around the rectangle's center, in degrees.
    pub rotation: f32,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

/// Move, resize or rotate an existing image.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageTransformRequest {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
}

/// The outcome of inserting or transforming an image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageEditResultView {
    pub page_index: u32,
    pub image: ImageObjectView,
    pub history: EditHistoryView,
}

pub(crate) struct LoadedImage {
    pub(crate) name: String,
    pub(crate) format: ImageFormat,
    pub(crate) source: Vec<u8>,
    pub(crate) decoded: DecodedImage,
}

fn decode_failed(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::ImageDecodeFailed, message)
}

fn unsupported_format(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::UnsupportedImageFormat, message)
}

fn too_large(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::ImageTooLarge, message)
}

/// Reads and decodes an image file without touching any open document.
pub(crate) fn load_image(path: &str) -> Result<LoadedImage, CommandError> {
    let path_ref = Path::new(path);
    let name = path_ref
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| unsupported_format("the image path has no file name"))?;

    let extension = path_ref
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let format = match extension.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        other => {
            return Err(unsupported_format(format!(
                "unsupported image extension {other:?}; choose a PNG or JPEG file"
            )));
        }
    };

    let metadata = std::fs::metadata(path_ref).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            CommandErrorCode::FileNotFound
        } else {
            CommandErrorCode::FileReadFailed
        };
        CommandError::new(code, format!("could not read image metadata: {error}"))
    })?;
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(too_large(format!(
            "image file is {} bytes, above the {} byte limit",
            metadata.len(),
            MAX_IMAGE_BYTES
        )));
    }

    // Read at most one byte past the limit so a file that grew after the
    // metadata check is still rejected without loading it whole.
    let file = File::open(path_ref).map_err(|error| {
        CommandError::new(
            CommandErrorCode::FileReadFailed,
            format!("could not open image file: {error}"),
        )
    })?;
    let mut source = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut source)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::FileReadFailed,
                format!("could not read image file: {error}"),
            )
        })?;
    if source.len() as u64 > MAX_IMAGE_BYTES {
        return Err(too_large(format!(
            "image file is larger than the {} byte limit",
            MAX_IMAGE_BYTES
        )));
    }

    match format {
        ImageFormat::Png if !source.starts_with(&PNG_MAGIC) => Err(unsupported_format(
            "the .png file does not contain PNG data",
        )),
        ImageFormat::Jpeg if !source.starts_with(&JPEG_MAGIC) => Err(unsupported_format(
            "the .jpg file does not contain JPEG data",
        )),
        _ => {
            let decoded = match format {
                ImageFormat::Png => decode_png(&source)?,
                ImageFormat::Jpeg => decode_jpeg(&source)?,
            };
            Ok(LoadedImage {
                name,
                format,
                source,
                decoded,
            })
        }
    }
}

/// Trims and validates a signature label. The label is shown by the overlay
/// and stored only in the editing session — it is not written into the PDF.
fn check_signature_name(name: &str) -> Result<String, CommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "the signature name must not be empty",
        ));
    }
    if trimmed.chars().count() > MAX_SIGNATURE_NAME_CHARS {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("the signature name must not exceed {MAX_SIGNATURE_NAME_CHARS} characters"),
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "the signature name must not contain control characters",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Decodes the in-memory image a signature dialog produced.
///
/// The bytes never touch disk: the dialog hands the command a base64 PNG or
/// JPEG payload, and the same magic-byte, size, pixel and decode checks as a
/// file insertion apply before any document is opened. The format comes from
/// the bytes themselves, so a mislabeled payload cannot slip through.
fn load_signature(encoded: &str, name: &str) -> Result<LoadedImage, CommandError> {
    let name = check_signature_name(name)?;
    if encoded.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "the signature image data is empty",
        ));
    }
    if encoded.len() > MAX_SIGNATURE_BASE64_LEN {
        return Err(too_large(format!(
            "the signature image exceeds the {} byte limit",
            MAX_IMAGE_BYTES
        )));
    }
    let source = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            decode_failed(format!("the signature image is not valid base64: {error}"))
        })?;
    if source.len() as u64 > MAX_IMAGE_BYTES {
        return Err(too_large(format!(
            "the signature image is larger than the {} byte limit",
            MAX_IMAGE_BYTES
        )));
    }
    let (format, decoded) = if source.starts_with(&PNG_MAGIC) {
        (ImageFormat::Png, decode_png(&source)?)
    } else if source.starts_with(&JPEG_MAGIC) {
        (ImageFormat::Jpeg, decode_jpeg(&source)?)
    } else {
        return Err(unsupported_format(
            "the signature image is neither PNG nor JPEG data",
        ));
    };
    Ok(LoadedImage {
        name,
        format,
        source,
        decoded,
    })
}

fn check_pixel_count(width: u32, height: u32) -> Result<(), CommandError> {
    if width == 0 || height == 0 {
        return Err(decode_failed("the image has zero width or height"));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(too_large(format!(
            "image is {width}x{height} ({pixels} pixels), above the {} pixel limit",
            MAX_IMAGE_PIXELS
        )));
    }
    Ok(())
}

fn decode_png(source: &[u8]) -> Result<DecodedImage, CommandError> {
    let mut decoder = png::Decoder::new(Cursor::new(source));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| decode_failed(format!("could not read PNG header: {error}")))?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    check_pixel_count(width, height)?;

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| decode_failed("PNG output size is out of range"))?;
    let mut buffer = vec![0; size];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| decode_failed(format!("could not decode PNG data: {error}")))?;
    buffer.truncate(frame.buffer_size());
    let color = from_png_color(frame.color_type)?;
    let bgra = to_bgra(color, &buffer, width, height)?;
    Ok(DecodedImage {
        width,
        height,
        bgra,
    })
}

fn decode_jpeg(source: &[u8]) -> Result<DecodedImage, CommandError> {
    use image::ImageDecoder as _;

    let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(source))
        .map_err(|error| decode_failed(format!("could not read JPEG header: {error}")))?;
    let (width, height) = decoder.dimensions();
    check_pixel_count(width, height)?;
    let color = from_image_color(decoder.color_type())?;
    let total = usize::try_from(decoder.total_bytes())
        .map_err(|_| decode_failed("JPEG decoded size is out of range"))?;
    let mut buffer = vec![0; total];
    decoder
        .read_image(&mut buffer)
        .map_err(|error| decode_failed(format!("could not decode JPEG data: {error}")))?;
    let bgra = to_bgra(color, &buffer, width, height)?;
    Ok(DecodedImage {
        width,
        height,
        bgra,
    })
}

/// Converts a tightly packed 8-bit image into the BGRA buffer PDFium expects.
///
/// PNG and JPEG decoders use different colour-type enums, so both are mapped
/// onto this small internal set before conversion.
fn to_bgra(
    color: PixelColor,
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CommandError> {
    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| decode_failed("image dimensions are out of range"))?;
    let mut bgra = Vec::with_capacity(pixels.saturating_mul(4));
    match color {
        PixelColor::Grayscale => {
            if data.len() < pixels {
                return Err(decode_failed("decoded PNG buffer is too short"));
            }
            for &value in &data[..pixels] {
                bgra.extend_from_slice(&[value, value, value, 255]);
            }
        }
        PixelColor::GrayscaleAlpha => {
            if data.len() < pixels * 2 {
                return Err(decode_failed("decoded PNG buffer is too short"));
            }
            for pixel in data[..pixels * 2].chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        PixelColor::Rgb => {
            if data.len() < pixels * 3 {
                return Err(decode_failed("decoded image buffer is too short"));
            }
            for pixel in data[..pixels * 3].chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        PixelColor::Rgba => {
            if data.len() < pixels * 4 {
                return Err(decode_failed("decoded image buffer is too short"));
            }
            for pixel in data[..pixels * 4].chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
    }
    Ok(bgra)
}

/// The pixel layouts both decoders can produce after normalisation.
#[derive(Debug, Clone, Copy)]
enum PixelColor {
    Grayscale,
    GrayscaleAlpha,
    Rgb,
    Rgba,
}

fn from_png_color(color: png::ColorType) -> Result<PixelColor, CommandError> {
    match color {
        png::ColorType::Grayscale => Ok(PixelColor::Grayscale),
        png::ColorType::GrayscaleAlpha => Ok(PixelColor::GrayscaleAlpha),
        png::ColorType::Rgb => Ok(PixelColor::Rgb),
        png::ColorType::Rgba => Ok(PixelColor::Rgba),
        other => Err(decode_failed(format!(
            "unsupported decoded image colour type {other:?}"
        ))),
    }
}

fn from_image_color(color: image::ColorType) -> Result<PixelColor, CommandError> {
    match color {
        image::ColorType::L8 => Ok(PixelColor::Grayscale),
        image::ColorType::La8 => Ok(PixelColor::GrayscaleAlpha),
        image::ColorType::Rgb8 => Ok(PixelColor::Rgb),
        image::ColorType::Rgba8 => Ok(PixelColor::Rgba),
        other => Err(decode_failed(format!(
            "unsupported decoded image colour type {other:?}"
        ))),
    }
}

/// Default insertion rectangle: the image's natural pixel size, shrunk if
/// needed to occupy at most 80% of the page on either axis, and centered.
///
/// A signature uses a smaller box — 35% of the page width and 25% of its
/// height — so it reads as a signature rather than a photo. Its aspect ratio
/// is preserved and it is centered the same way.
fn default_layout(
    page_width: f32,
    page_height: f32,
    pixel_width: u32,
    pixel_height: u32,
    kind: ImageKind,
) -> ImageLayout {
    let (fit_width, fit_height) = match kind {
        ImageKind::Image => (DEFAULT_FIT_RATIO, DEFAULT_FIT_RATIO),
        ImageKind::Signature => (SIGNATURE_FIT_WIDTH, SIGNATURE_FIT_HEIGHT),
    };
    let natural_width = pixel_width as f32;
    let natural_height = pixel_height as f32;
    let scale = (fit_width * page_width / natural_width)
        .min(fit_height * page_height / natural_height)
        .min(1.0);
    let width = (natural_width * scale).max(MIN_IMAGE_SIZE);
    let height = (natural_height * scale).max(MIN_IMAGE_SIZE);
    ImageLayout {
        x: (page_width - width) / 2.0,
        y: (page_height - height) / 2.0,
        width,
        height,
        rotation: 0.0,
    }
}

/// Normalizes a clockwise display-space angle to `(-180, 180]`.
fn normalize_rotation(angle: f32) -> f32 {
    let mut angle = angle % 360.0;
    if angle <= -180.0 {
        angle += 360.0;
    } else if angle > 180.0 {
        angle -= 360.0;
    }
    if angle == -0.0 {
        0.0
    } else {
        angle
    }
}

pub(crate) fn check_layout(layout: &ImageLayout) -> Result<(), CommandError> {
    if !layout.x.is_finite()
        || !layout.y.is_finite()
        || !layout.width.is_finite()
        || !layout.height.is_finite()
        || !layout.rotation.is_finite()
    {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "image position, size and rotation must be finite numbers",
        ));
    }
    if layout.width < MIN_IMAGE_SIZE || layout.height < MIN_IMAGE_SIZE {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "image width and height must be at least 1 point",
        ));
    }
    if layout.width > MAX_IMAGE_SIZE || layout.height > MAX_IMAGE_SIZE {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("image width and height must not exceed {MAX_IMAGE_SIZE} points"),
        ));
    }
    Ok(())
}

/// Builds the PDF matrix that places the image's unit square at `layout`.
///
/// Display corners are rotated clockwise around the rectangle center, then
/// mapped into PDF user space through the page's display geometry. The matrix
/// columns are the bottom edge and the left edge, so the unit square keeps the
/// image upright and unmirrored.
fn image_matrix(geometry: &DisplayGeometry, layout: &ImageLayout) -> PdfMatrix {
    let center_x = layout.x + layout.width / 2.0;
    let center_y = layout.y + layout.height / 2.0;
    let radians = layout.rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let rotate = |x: f32, y: f32| {
        let dx = x - center_x;
        let dy = y - center_y;
        (
            center_x + dx * cos - dy * sin,
            center_y + dx * sin + dy * cos,
        )
    };

    let (bl_x, bl_y) = rotate(layout.x, layout.y + layout.height);
    let (br_x, br_y) = rotate(layout.x + layout.width, layout.y + layout.height);
    let (tl_x, tl_y) = rotate(layout.x, layout.y);
    let (bl_x, bl_y) = geometry.point_inverse(bl_x, bl_y);
    let (br_x, br_y) = geometry.point_inverse(br_x, br_y);
    let (tl_x, tl_y) = geometry.point_inverse(tl_x, tl_y);

    PdfMatrix::new(
        br_x - bl_x,
        br_y - bl_y,
        tl_x - bl_x,
        tl_y - bl_y,
        bl_x,
        bl_y,
    )
}

fn page_geometry(
    document: &PdfDocument<'static>,
    page_index: u32,
) -> Result<(DisplayGeometry, usize), CommandError> {
    let page = load_page(document, page_index)?;
    let rotation = page
        .rotation()
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not read page {page_index} rotation: {error:?}"),
            )
        })?
        .as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    let object_count = page.objects().len();
    Ok((geometry, object_count))
}

pub(crate) fn build_image_object(
    document: &PdfDocument<'static>,
    geometry: &DisplayGeometry,
    layout: &ImageLayout,
    decoded: DecodedImage,
) -> Result<PdfPageObject<'static>, CommandError> {
    let mut object = PdfPageImageObject::new(document).map_err(|error| {
        decode_failed(format!("could not create an image page object: {error:?}"))
    })?;
    let mut buffer = decoded.bgra;
    let width =
        i32::try_from(decoded.width).map_err(|_| decode_failed("image width is out of range"))?;
    let height =
        i32::try_from(decoded.height).map_err(|_| decode_failed("image height is out of range"))?;
    let bitmap = PdfBitmap::from_bytes(width, height, PdfBitmapFormat::BGRA, &mut buffer)
        .map_err(|error| decode_failed(format!("could not wrap image pixels: {error:?}")))?;
    object
        .set_bitmap(&bitmap)
        .map_err(|error| decode_failed(format!("could not apply image pixels: {error:?}")))?;
    object
        .apply_matrix(image_matrix(geometry, layout))
        .map_err(|error| decode_failed(format!("could not place the image: {error:?}")))?;
    Ok(object.into())
}

fn decode_source(record: &ImageRecord) -> Result<DecodedImage, CommandError> {
    match record.format {
        ImageFormat::Png => decode_png(&record.source),
        ImageFormat::Jpeg => decode_jpeg(&record.source),
    }
}

fn find_image(
    session: &EditSession,
    page_index: u32,
    image_id: u64,
) -> Result<usize, CommandError> {
    session
        .images
        .get(&page_index)
        .and_then(|records| {
            records
                .iter()
                .position(|record| record.image_id == image_id)
        })
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("no image {image_id} on page {page_index}"),
            )
        })
}

fn deleted_image(image_id: u64) -> CommandError {
    CommandError::new(
        CommandErrorCode::InvalidArgument,
        format!("image {image_id} has been deleted"),
    )
}

fn push_image_action(entry: &mut OpenDocument, mut action: ImageEditAction) {
    action.before_state = entry.edit.current_state;
    action.after_state = entry.edit.next_state + 1;
    entry.edit.next_state = action.after_state;
    let after_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Image(action));
    entry.edit.redo_stack.clear();
    entry.edit.current_state = after_state;
}

/// The outcome of an image action, rebuilt from the session record.
fn result_for(
    entry: &OpenDocument,
    page_index: u32,
    image_id: u64,
) -> Result<ImageObjectView, CommandError> {
    let records = entry
        .edit
        .images
        .get(&page_index)
        .ok_or_else(|| deleted_image(image_id))?;
    let record = records
        .iter()
        .find(|record| record.image_id == image_id && !record.deleted)
        .ok_or_else(|| deleted_image(image_id))?;
    Ok(record.view(page_index))
}

fn insert_for(
    entry: &mut OpenDocument,
    page_index: u32,
    loaded: LoadedImage,
    kind: ImageKind,
) -> Result<ImageEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let (geometry, start) = page_geometry(&entry.document, page_index)?;
    let pixel_width = loaded.decoded.width;
    let pixel_height = loaded.decoded.height;
    let (page_width, page_height) = geometry.display_size();
    let layout = default_layout(page_width, page_height, pixel_width, pixel_height, kind);
    check_layout(&layout)?;

    let object = build_image_object(&entry.document, &geometry, &layout, loaded.decoded)?;
    {
        let mut page = load_page(&entry.document, page_index)?;
        page.objects_mut().add_object(object).map_err(|error| {
            decode_failed(format!("could not add the image to the page: {error:?}"))
        })?;
    }

    entry.edit.next_image_id += 1;
    let image_id = entry.edit.next_image_id;
    let record_index = entry
        .edit
        .images
        .get(&page_index)
        .map(Vec::len)
        .unwrap_or(0);
    let record = ImageRecord {
        image_id,
        start,
        count: 1,
        layout: layout.clone(),
        name: loaded.name,
        kind,
        pixel_width,
        pixel_height,
        format: loaded.format,
        source: Arc::new(loaded.source),
        deleted: false,
    };
    let after_snapshot = record.snapshot(record_index);
    entry
        .edit
        .images
        .entry(page_index)
        .or_default()
        .push(record);
    push_image_action(
        entry,
        ImageEditAction {
            page_index,
            image_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: None,
            before_objects: Some(Vec::new()),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );

    let image = result_for(entry, page_index, image_id)?;
    Ok(ImageEditResultView {
        page_index,
        image,
        history: entry.edit.history_view(),
    })
}

fn images_for(entry: &OpenDocument, page_index: u32) -> Result<Vec<ImageObjectView>, CommandError> {
    // Reject a page index outside the document even when no images exist.
    load_page(&entry.document, page_index)?;
    Ok(entry
        .edit
        .images
        .get(&page_index)
        .map(|records| {
            records
                .iter()
                .filter(|record| !record.deleted)
                .map(|record| record.view(page_index))
                .collect()
        })
        .unwrap_or_default())
}

fn transform_for(
    entry: &mut OpenDocument,
    page_index: u32,
    image_id: u64,
    request: ImageTransformRequest,
) -> Result<ImageEditResultView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let layout = ImageLayout {
        x: request.x,
        y: request.y,
        width: request.width,
        height: request.height,
        rotation: normalize_rotation(request.rotation),
    };
    check_layout(&layout)?;

    let record_index = find_image(&entry.edit, page_index, image_id)?;
    let (start, old_count, deleted) = {
        let record = &entry.edit.images[&page_index][record_index];
        (record.start, record.count, record.deleted)
    };
    if deleted {
        return Err(deleted_image(image_id));
    }

    // Rebuild from the original bytes before detaching anything, so a decode
    // failure cannot leave the page half-edited.
    let decoded = {
        let record = &entry.edit.images[&page_index][record_index];
        decode_source(record)?
    };
    let (geometry, _) = page_geometry(&entry.document, page_index)?;
    let object = build_image_object(&entry.document, &geometry, &layout, decoded)?;
    let before_snapshot = entry.edit.images[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, old_count, vec![object])?;
    {
        let record = &mut entry.edit.images.get_mut(&page_index).expect("image page")[record_index];
        record.layout = layout;
        record.count = 1;
    }
    let after_snapshot = entry.edit.images[&page_index][record_index].snapshot(record_index);
    push_image_action(
        entry,
        ImageEditAction {
            page_index,
            image_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: Some(before_snapshot),
            before_objects: Some(removed),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );

    let image = result_for(entry, page_index, image_id)?;
    Ok(ImageEditResultView {
        page_index,
        image,
        history: entry.edit.history_view(),
    })
}

fn delete_for(
    entry: &mut OpenDocument,
    page_index: u32,
    image_id: u64,
) -> Result<EditStepView, CommandError> {
    ensure_content_editable(&entry.document)?;
    let record_index = find_image(&entry.edit, page_index, image_id)?;
    let (start, count, deleted) = {
        let record = &entry.edit.images[&page_index][record_index];
        (record.start, record.count, record.deleted)
    };
    if deleted {
        return Err(deleted_image(image_id));
    }
    let before_snapshot = entry.edit.images[&page_index][record_index].snapshot(record_index);
    let removed = detach_and_insert(&entry.document, page_index, start, count, Vec::new())?;
    {
        let record = &mut entry.edit.images.get_mut(&page_index).expect("image page")[record_index];
        record.count = 0;
        record.deleted = true;
    }
    let after_snapshot = entry.edit.images[&page_index][record_index].snapshot(record_index);
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        start,
        -(count as isize),
        ObjectRef::Image(image_id),
    );
    push_image_action(
        entry,
        ImageEditAction {
            page_index,
            image_id,
            start,
            before_state: 0,
            after_state: 0,
            before_record: Some(before_snapshot),
            before_objects: Some(removed),
            after_record: Some(after_snapshot),
            after_objects: None,
        },
    );
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// Undo of an image action. Called by the unified history after the action has
/// been popped from the undo stack.
pub(crate) fn undo_image(
    entry: &mut OpenDocument,
    mut action: ImageEditAction,
) -> Result<EditStepView, CommandError> {
    let before_count = action
        .before_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let after_count = action
        .after_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let restore = action.before_objects.take().unwrap_or_default();
    let detached = detach_and_insert(
        &entry.document,
        action.page_index,
        action.start,
        after_count,
        restore,
    )?;
    action.after_objects = Some(detached);

    let page_index = action.page_index;
    {
        let images = entry.edit.images.entry(page_index).or_default();
        match &action.before_record {
            Some(before) => {
                let index = images
                    .iter()
                    .position(|record| record.image_id == action.image_id)
                    .ok_or_else(|| {
                        CommandError::new(
                            CommandErrorCode::Internal,
                            format!(
                                "the edit history lost image {} on page {page_index}",
                                action.image_id
                            ),
                        )
                    })?;
                images[index].restore(before);
            }
            None => images.retain(|record| record.image_id != action.image_id),
        }
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        before_count as isize - after_count as isize,
        ObjectRef::Image(action.image_id),
    );

    entry.edit.current_state = action.before_state;
    entry.edit.redo_stack.push(EditAction::Image(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// Redo of an image action. Called by the unified history after the action has
/// been popped from the redo stack.
pub(crate) fn redo_image(
    entry: &mut OpenDocument,
    mut action: ImageEditAction,
) -> Result<EditStepView, CommandError> {
    let before_count = action
        .before_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let after_count = action
        .after_record
        .as_ref()
        .map(|record| record.count)
        .unwrap_or(0);
    let restore = action.after_objects.take().unwrap_or_default();
    let detached = detach_and_insert(
        &entry.document,
        action.page_index,
        action.start,
        before_count,
        restore,
    )?;
    action.before_objects = Some(detached);

    let page_index = action.page_index;
    {
        let images = entry.edit.images.entry(page_index).or_default();
        match &action.after_record {
            Some(after) => {
                if let Some(index) = images
                    .iter()
                    .position(|record| record.image_id == action.image_id)
                {
                    images[index].restore(after);
                } else {
                    let index = after.record_index.min(images.len());
                    images.insert(index, ImageRecord::from_snapshot(after));
                }
            }
            None => images.retain(|record| record.image_id != action.image_id),
        }
    }
    shift_object_indexes(
        &mut entry.edit,
        page_index,
        action.start,
        after_count as isize - before_count as isize,
        ObjectRef::Image(action.image_id),
    );

    entry.edit.current_state = action.after_state;
    entry.edit.undo_stack.push(EditAction::Image(action));
    Ok(EditStepView {
        page_index,
        page_count: entry.pages.len(),
        page_structure_changed: false,
        history: entry.edit.history_view(),
    })
}

/// The `insert_image` command: validates and decodes a PNG/JPEG file, then
/// places it centered on the current Page as one undoable action.
#[tauri::command]
pub fn insert_image(
    document_id: String,
    page_index: u32,
    path: String,
) -> Result<ImageEditResultView, CommandError> {
    let loaded = load_image(&path)?;
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = insert_for(entry, physical, loaded, ImageKind::Image)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.image.page_index = logical;
        Ok(view)
    })
}

/// The `insert_signature_image` command: places a signature produced by the
/// draw or type tab, or an uploaded PNG/JPEG, on the current Page.
///
/// The payload is decoded and validated in memory before the document is
/// opened, so malformed data leaves the document, its history and its saved
/// state untouched. The resulting object is an ordinary image page object and
/// is undoable through the shared Edit Session.
#[tauri::command]
pub fn insert_signature_image(
    document_id: String,
    page_index: u32,
    image_base64: String,
    name: String,
) -> Result<ImageEditResultView, CommandError> {
    let loaded = load_signature(&image_base64, &name)?;
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = insert_for(entry, physical, loaded, ImageKind::Signature)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.image.page_index = logical;
        Ok(view)
    })
}

/// The `page_images` command: every image this session inserted on the Page,
/// in creation order. Deleted images are excluded.
#[tauri::command]
pub fn page_images(
    document_id: String,
    page_index: u32,
) -> Result<Vec<ImageObjectView>, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut views = images_for(entry, physical)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        for view in &mut views {
            view.page_index = logical;
        }
        Ok(views)
    })
}

/// The `transform_image` command: moves, resizes or rotates an image around
/// its center as one undoable action.
#[tauri::command]
pub fn transform_image(
    document_id: String,
    page_index: u32,
    image_id: u64,
    transform: ImageTransformRequest,
) -> Result<ImageEditResultView, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut view = transform_for(entry, physical, image_id, transform)?;
        let logical = entry.logical_page(physical).unwrap_or(0);
        view.page_index = logical;
        view.image.page_index = logical;
        Ok(view)
    })
}

/// The `delete_image` command: removes the image's page object. Undo restores
/// the page exactly as it was.
#[tauri::command]
pub fn delete_image(
    document_id: String,
    page_index: u32,
    image_id: u64,
) -> Result<EditStepView, CommandError> {
    with_open_document(&document_id, |entry| {
        let physical = entry.physical_page(page_index)?;
        let mut step = delete_for(entry, physical, image_id)?;
        step.page_index = entry.logical_page(physical).unwrap_or(0);
        Ok(step)
    })
}
