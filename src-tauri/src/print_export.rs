//! Printing and exporting the current Page.
//!
//! Both features are read-only: they render through the same pipeline as the
//! page viewer, never touch the Source Document and never change the edit
//! history, so the Document's unsaved state is unaffected.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::commands::{
    checked_page_pixels, encode_png, load_page, render_page_rgba, with_open_document, CommandError,
    CommandErrorCode,
};
use crate::logging;
use crate::target_path::same_file;

/// Export resolution bounds. 72 DPI is actual size (scale 1).
pub(crate) const MIN_EXPORT_DPI: u32 = 36;
pub(crate) const MAX_EXPORT_DPI: u32 = 600;

/// Image format of an exported Page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageImageFormat {
    Png,
    Jpg,
}

impl PageImageFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
        }
    }
}

/// What the frontend needs to confirm an export and offer to reveal the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageExportView {
    pub path: String,
    pub format: PageImageFormat,
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

fn validate_dpi(dpi: u32) -> Result<(), CommandError> {
    if !(MIN_EXPORT_DPI..=MAX_EXPORT_DPI).contains(&dpi) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            format!("export DPI must be between {MIN_EXPORT_DPI} and {MAX_EXPORT_DPI}, got {dpi}"),
        ));
    }
    Ok(())
}

/// Renders `page_index` at `dpi` and writes it as a PNG or JPEG image.
///
/// The page bitmap comes from [`render_page_rgba`], the same rendering path the
/// page viewer uses, so the file matches the canvas at the chosen resolution.
/// Export is read-only: it never changes the Document or its edit history.
#[tauri::command]
pub fn export_page(
    document_id: String,
    page_index: u32,
    format: PageImageFormat,
    dpi: u32,
    destination: String,
) -> Result<PageExportView, CommandError> {
    let result = export_page_impl(&document_id, page_index, format, dpi, &destination);
    match &result {
        Ok(view) => logging::event(
            "document.export_page",
            &format!(
                "documentId={document_id} page={page_index} format={} dpi={dpi} bytes={}",
                view.format.extension(),
                view.bytes
            ),
        ),
        Err(error) => logging::error(
            "document.export_page.failed",
            &format!("code={:?} message={}", error.code, error.message),
        ),
    }
    result
}

fn export_page_impl(
    document_id: &str,
    page_index: u32,
    format: PageImageFormat,
    dpi: u32,
    destination: &str,
) -> Result<PageExportView, CommandError> {
    validate_dpi(dpi)?;
    let scale = dpi as f32 / 72.0;
    let path = Path::new(destination);
    if destination.trim().is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidArgument,
            "export destination must not be empty",
        ));
    }

    let (bytes, width, height) = with_open_document(document_id, |entry| {
        ensure_target_is_not_open_document(
            &entry.source_path,
            entry.save.output_path.as_deref(),
            path,
        )?;
        let physical_index = entry.physical_page(page_index)?;
        let document = entry.render_document()?;
        let page = load_page(document, physical_index)?;
        let (width, height) = checked_page_pixels(&page, physical_index, scale)?;
        let rgba = render_page_rgba(&page, physical_index, width, height)?;
        let bytes = match format {
            PageImageFormat::Png => encode_png(&rgba, width, height)?,
            PageImageFormat::Jpg => encode_jpeg(&rgba, width, height)?,
        };
        Ok((bytes, width, height))
    })?;

    crate::atomic_write::write_atomically(path, &bytes)?;

    Ok(PageExportView {
        path: path.to_string_lossy().into_owned(),
        format,
        dpi,
        width,
        height,
        bytes: bytes.len() as u64,
    })
}

/// JPEG quality for page export. High enough that text edges stay crisp while
/// keeping files small.
const JPEG_QUALITY: u8 = 90;

/// Encodes RGBA pixels as JPEG. JPEG has no alpha channel, so transparent
/// pixels are composited over white, the page background.
fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CommandError> {
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = pixel[3] as u16;
        for channel in &pixel[..3] {
            let value = (*channel as u16 * alpha + 255 * (255 - alpha) + 127) / 255;
            rgb.push(value as u8);
        }
    }

    let mut output = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, JPEG_QUALITY);
    encoder
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|error| {
            CommandError::new(
                CommandErrorCode::RenderFailed,
                format!("could not encode rendered page as JPEG: {error}"),
            )
        })?;
    Ok(output)
}

/// Applies the Document's Permission Restrictions to printing.
///
/// PDF Permission Restrictions must be respected, so the system print dialog
/// is never opened for a Document whose owner denied printing. Public because
/// it is the part of the `print_document` command that can be tested at the
/// command boundary without a native window (the same pattern as
/// `save_document_bytes`).
pub fn ensure_print_allowed(document_id: &str) -> Result<(), CommandError> {
    with_open_document(document_id, |entry| {
        let permissions = entry.document.permissions();
        let allowed = permissions.can_print_high_quality().unwrap_or(false)
            || permissions.can_print_only_low_quality().unwrap_or(false);
        if allowed {
            Ok(())
        } else {
            Err(CommandError::new(
                CommandErrorCode::PermissionDenied,
                "the document's Permission Restrictions do not allow printing",
            ))
        }
    })
}

/// Opens the platform print dialog for the print surface the frontend built.
///
/// The frontend renders one bitmap per selected Page into a print-only DOM
/// before calling this, so the native dialog sees the correct page count,
/// orientation and scaling options. `WebviewWindow::print` uses the native
/// print operation on macOS and `window.print()` elsewhere.
#[tauri::command]
pub fn print_document(
    window: tauri::WebviewWindow,
    document_id: String,
) -> Result<(), CommandError> {
    ensure_print_allowed(&document_id)?;
    window.print().map_err(|error| {
        CommandError::new(
            CommandErrorCode::PrintFailed,
            format!("could not start the print dialog: {error}"),
        )
    })?;
    logging::event("document.print", &format!("documentId={document_id}"));
    Ok(())
}

/// Refuses an export target that names the open Source Document or the current
/// Output Document. Writing image bytes over either one would silently destroy
/// a PDF; the Source Document is never modified (ADR-0007) and the Output
/// Document holds the user's saved work.
///
/// The comparison resolves symlinks and case variants through
/// [`same_file`], so an alias cannot bypass the check.
fn ensure_target_is_not_open_document(
    source: &Path,
    output: Option<&Path>,
    destination: &Path,
) -> Result<(), CommandError> {
    if same_file(source, destination) {
        return Err(CommandError::new(
            CommandErrorCode::ExportTargetIsSource,
            format!(
                "{} is the open Source Document; choose a different export file",
                destination.display()
            ),
        ));
    }
    if output.is_some_and(|output| same_file(output, destination)) {
        return Err(CommandError::new(
            CommandErrorCode::ExportTargetIsOutput,
            format!(
                "{} is the current Output Document; choose a different export file",
                destination.display()
            ),
        ));
    }
    Ok(())
}
