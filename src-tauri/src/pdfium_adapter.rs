//! Safe adapter over the vendored `pdfium-render` patch.
//!
//! The adapter always operates on the same `PdfDocument` handle that holds
//! high-level `pdfium-render` edits. It adds only the capabilities PDFium
//! supports but `pdfium-render` 0.9.4 does not expose: flags-aware saves,
//! circle- and stamp-annotation creation, ink strokes, annotation border
//! width, text markup annotations, annotation identity and custom appearance
//! streams. The patch itself lives in `vendor/pdfium-render/PDFX-PATCH.md`;
//! callers of this module never need `unsafe` and never touch a PDFium handle.
//!
//! Saving is explicit: callers choose [SaveMode::Incremental] (append a
//! revision whose output begins with the source bytes), [SaveMode::FullRewrite]
//! or [SaveMode::RemoveSecurity] (full rewrite without encryption). The save
//! pipeline (ticket 10) is expected to append `lopdf` revisions to the bytes
//! returned here, never to the source document.
//!
//! The annotation operations come in free-function form as well as methods on
//! [PdfiumAdapter]. The open document registry borrows the live `PdfDocument`
//! (it cannot move it into the adapter without giving up the undo snapshots
//! that share its handle), so production callers use the free functions; the
//! owning adapter remains the convenient shape for tests.

use std::fmt;

use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfPageAnnotation, PdfPageAnnotationCommon, PdfPageAnnotationType,
    PdfPoints, PdfQuadPoints, PdfRect, PdfSecurityHandlerRevision, PdfiumError, FPDF_DWORD,
};

/// A rectangle in default user space units (points), with PDF's origin at the
/// bottom-left corner of the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnotationRect {
    pub bottom: f32,
    pub left: f32,
    pub top: f32,
    pub right: f32,
}

impl AnnotationRect {
    pub const fn new(bottom: f32, left: f32, top: f32, right: f32) -> Self {
        Self {
            bottom,
            left,
            top,
            right,
        }
    }

    fn to_pdf_rect(self) -> PdfRect {
        PdfRect::new(
            PdfPoints::new(self.bottom),
            PdfPoints::new(self.left),
            PdfPoints::new(self.top),
            PdfPoints::new(self.right),
        )
    }

    fn from_pdf_rect(rect: PdfRect) -> Self {
        Self {
            bottom: rect.bottom().value,
            left: rect.left().value,
            top: rect.top().value,
            right: rect.right().value,
        }
    }
}

/// Identifies an annotation by its page index and its zero-based position in
/// that page's annotation list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnnotationRef {
    pub page_index: i32,
    pub index: usize,
}

impl AnnotationRef {
    pub const fn new(page_index: i32, index: usize) -> Self {
        Self { page_index, index }
    }
}

/// The text markup annotations PDFX creates through PDFium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMarkupAnnotationKind {
    Highlight,
    Underline,
    Strikeout,
}

/// How [PdfiumAdapter::save()] should write the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// Append one incremental revision. The returned bytes begin with the exact
    /// source bytes, and only objects dirtied by the edit session are written.
    Incremental,
    /// Rewrite the whole file. The output is a new, compact PDF rather than an
    /// append; document-level structure is preserved.
    FullRewrite,
    /// Rewrite the whole file without encryption (`FPDF_REMOVE_SECURITY`).
    /// Pdfium implies a full rewrite whenever this flag is set.
    RemoveSecurity,
}

impl SaveMode {
    fn flags(self) -> FPDF_DWORD {
        match self {
            SaveMode::Incremental => PdfDocument::FPDF_INCREMENTAL,
            SaveMode::FullRewrite => PdfDocument::FPDF_NO_INCREMENTAL,
            SaveMode::RemoveSecurity => PdfDocument::FPDF_REMOVE_SECURITY,
        }
    }
}

/// Errors raised by the adapter. The Tauri command boundary maps these to its
/// own stable error codes; no PDFium handles cross this type.
#[derive(Debug)]
pub enum AdapterError {
    /// Pdfium reported a failure.
    Pdfium(PdfiumError),
    /// The document is encrypted and the requested save mode would preserve the
    /// encryption. Such saves are refused so that encrypted bytes are never
    /// written by accident; use [SaveMode::RemoveSecurity] to produce an
    /// unencrypted rewrite.
    EncryptedDocument,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::Pdfium(error) => write!(formatter, "PDFium error: {error}"),
            AdapterError::EncryptedDocument => write!(
                formatter,
                "document is encrypted; use SaveMode::RemoveSecurity to write it unencrypted"
            ),
        }
    }
}

impl std::error::Error for AdapterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdapterError::Pdfium(error) => Some(error),
            AdapterError::EncryptedDocument => None,
        }
    }
}

impl From<PdfiumError> for AdapterError {
    fn from(error: PdfiumError) -> Self {
        AdapterError::Pdfium(error)
    }
}

/// Returns `true` if an already-open document has an active standard security
/// handler.
///
/// The save pipeline borrows the live document instead of moving it into a
/// [`PdfiumAdapter`], because the same handle also owns the undo/redo snapshots
/// and must stay open across saves. This free function exposes the adapter's
/// encryption check for that borrowed case.
pub fn document_is_encrypted(document: &PdfDocument<'_>) -> Result<bool, AdapterError> {
    Ok(document.permissions().security_handler_revision()?
        != PdfSecurityHandlerRevision::Unprotected)
}

/// Saves an already-open document with the given [SaveMode].
///
/// Incremental and plain full-rewrite saves of an encrypted document are
/// refused with [AdapterError::EncryptedDocument] rather than silently writing
/// encrypted output; use [SaveMode::RemoveSecurity] for the unencrypted
/// rewrite ADR-0007 requires.
pub fn save_document_with_mode(
    document: &PdfDocument<'_>,
    mode: SaveMode,
) -> Result<Vec<u8>, AdapterError> {
    if mode != SaveMode::RemoveSecurity && document_is_encrypted(document)? {
        return Err(AdapterError::EncryptedDocument);
    }

    Ok(document.save_to_bytes_with_flags(mode.flags())?)
}

/// Runs `operation` against an annotation of an already-open document.
///
/// The borrowed counterpart of [`PdfiumAdapter`]'s private helper; every
/// annotation operation in this module funnels through it so the handle is
/// always closed when the operation returns.
fn with_annotation_on<R>(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    operation: impl FnOnce(&mut PdfPageAnnotation<'_>) -> Result<R, PdfiumError>,
) -> Result<R, AdapterError> {
    let pages = document.pages();
    let page = pages.get(target.page_index)?;
    let annotations = page.annotations();
    let mut annotation = annotations.get(target.index)?;
    Ok(operation(&mut annotation)?)
}

/// Applies `body` to the concrete highlight, underline or strikeout behind an
/// annotation, so the common annotation trait and the inherent attachment
/// point mutators are available without matching at every call site.
macro_rules! with_text_markup {
    ($annotation:expr, $annotation_ref:ident => $body:expr) => {
        match $annotation {
            PdfPageAnnotation::Highlight($annotation_ref) => $body,
            PdfPageAnnotation::Underline($annotation_ref) => $body,
            PdfPageAnnotation::Strikeout($annotation_ref) => $body,
            _ => Err(PdfiumError::UnknownPdfAnnotationType),
        }
    };
}

/// Creates a highlight, underline or strikeout annotation with `rect` bounds.
///
/// Pdfium does not generate an appearance for the annotation; callers install
/// one with [`set_annotation_appearance`] before saving.
pub fn create_text_markup_annotation(
    document: &PdfDocument<'_>,
    page_index: i32,
    kind: TextMarkupAnnotationKind,
    rect: AnnotationRect,
) -> Result<AnnotationRef, AdapterError> {
    let pages = document.pages();
    let mut page = pages.get(page_index)?;
    let annotations = page.annotations_mut();

    // Each concrete annotation type differs, so the shared work is repeated per
    // arm: set the bounds Pdfium uses for the appearance Form XObject /BBox,
    // then close the creation handle.
    match kind {
        TextMarkupAnnotationKind::Highlight => {
            let mut annotation = annotations.create_highlight_annotation()?;
            let result = annotation.set_bounds(rect.to_pdf_rect());
            annotation.close();
            result?;
        }
        TextMarkupAnnotationKind::Underline => {
            let mut annotation = annotations.create_underline_annotation()?;
            let result = annotation.set_bounds(rect.to_pdf_rect());
            annotation.close();
            result?;
        }
        TextMarkupAnnotationKind::Strikeout => {
            let mut annotation = annotations.create_strikeout_annotation()?;
            let result = annotation.set_bounds(rect.to_pdf_rect());
            annotation.close();
            result?;
        }
    }

    Ok(AnnotationRef::new(page_index, annotations.len() - 1))
}

/// Creates a stamp annotation with `rect` bounds.
///
/// Pdfium does not generate an appearance for the annotation; callers install
/// one with [`set_annotation_appearance`] before saving.
pub fn create_stamp_annotation(
    document: &PdfDocument<'_>,
    page_index: i32,
    rect: AnnotationRect,
) -> Result<AnnotationRef, AdapterError> {
    let pages = document.pages();
    let mut page = pages.get(page_index)?;
    let annotations = page.annotations_mut();
    let mut annotation = annotations.create_stamp_annotation()?;
    annotation.set_bounds(rect.to_pdf_rect())?;
    annotation.close();
    Ok(AnnotationRef::new(page_index, annotations.len() - 1))
}

/// Returns the [PdfPageAnnotationType] of the annotation identified by
/// `target`.
pub fn annotation_type(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<PdfPageAnnotationType, AdapterError> {
    with_annotation_on(document, target, |annotation| {
        Ok(annotation.annotation_type())
    })
}

/// Returns the annotation name (`/NM`), if any.
pub fn annotation_name(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<Option<String>, AdapterError> {
    with_annotation_on(document, target, |annotation| Ok(annotation.name()))
}

/// Sets the annotation name (`/NM`), PDFX's stable identity for text markup.
pub fn set_annotation_name(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    name: &str,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| annotation.set_name(name))
}

/// Returns the annotation author (`/T`), if any.
pub fn annotation_author(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<Option<String>, AdapterError> {
    with_annotation_on(document, target, |annotation| Ok(annotation.creator()))
}

/// Sets the annotation author (`/T`).
pub fn set_annotation_author(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    author: &str,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.set_creator(author)
    })
}

/// Returns the raw `/CreationDate` string, if any.
pub fn annotation_creation_date(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<Option<String>, AdapterError> {
    with_annotation_on(
        document,
        target,
        |annotation| Ok(annotation.creation_date()),
    )
}

/// Sets `/CreationDate` from a raw PDF date string.
pub fn set_annotation_creation_date(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    date: &str,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.set_creation_date_string(date)
    })
}

/// Returns an arbitrary string value from the annotation dictionary.
///
/// PDFX stores the chosen markup colour and opacity under private keys because
/// Pdfium refuses to read `/C` and `/CA` once the annotation has a custom
/// appearance stream. Readers ignore unknown keys; `/C` and `/CA` are still
/// written for interoperability.
pub fn annotation_string_value(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    key: &str,
) -> Result<Option<String>, AdapterError> {
    with_annotation_on(document, target, |annotation| {
        Ok(annotation.string_value(key))
    })
}

/// Sets an arbitrary string value on the annotation dictionary.
pub fn set_annotation_string_value(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    key: &str,
    value: &str,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.set_string_value(key, value)
    })
}

/// Returns the annotation rectangle in default user space.
pub fn annotation_bounds(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<AnnotationRect, AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.bounds().map(AnnotationRect::from_pdf_rect)
    })
}

/// Returns the annotation color as RGBA bytes.
pub fn annotation_color(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<(u8, u8, u8, u8), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation
            .stroke_color()
            .map(|color| (color.red(), color.green(), color.blue(), color.alpha()))
    })
}

/// Sets the annotation color and its constant opacity.
pub fn set_annotation_color(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.set_stroke_color(PdfColor::new(red, green, blue, alpha))
    })
}

/// Returns the annotation's attachment points (`/QuadPoints`) in PDF user
/// space, each as `[x1, y1, x2, y2, x3, y3, x4, y4]`.
pub fn annotation_quads(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<Vec<[f32; 8]>, AdapterError> {
    with_annotation_on(document, target, |annotation| {
        with_text_markup!(annotation, annotation => {
            Ok(annotation
                .attachment_points()
                .iter()
                .map(|quad| {
                    [
                        quad.x1().value,
                        quad.y1().value,
                        quad.x2().value,
                        quad.y2().value,
                        quad.x3().value,
                        quad.y3().value,
                        quad.x4().value,
                        quad.y4().value,
                    ]
                })
                .collect())
        })
    })
}

/// Appends attachment points (`/QuadPoints`) to a text markup annotation.
pub fn append_annotation_quads(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    quads: &[[f32; 8]],
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        with_text_markup!(annotation, annotation => {
            let points = annotation.attachment_points_mut();
            for quad in quads {
                points.create_attachment_point_at_end(PdfQuadPoints::new_from_values(
                    quad[0], quad[1], quad[2], quad[3], quad[4], quad[5], quad[6], quad[7],
                ))?;
            }
            Ok(())
        })
    })
}

/// Installs `appearance` as the annotation's normal appearance stream.
///
/// Pdfium creates a Form XObject whose `/BBox` is the annotation's `/Rect` at
/// the time of this call, and the stream is interpreted in page coordinates.
/// Set the annotation bounds and quads first.
pub fn set_annotation_appearance(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
    appearance: &str,
) -> Result<(), AdapterError> {
    with_annotation_on(document, target, |annotation| {
        annotation.set_appearance(appearance)
    })
}

/// Removes the annotation from the page, which also removes it from the saved
/// document.
pub fn remove_annotation(
    document: &PdfDocument<'_>,
    target: AnnotationRef,
) -> Result<(), AdapterError> {
    let pages = document.pages();
    let mut page = pages.get(target.page_index)?;
    let annotations = page.annotations_mut();
    let annotation = annotations.get(target.index)?;
    annotations.delete_annotation(annotation)?;
    Ok(())
}

/// Owns a [`PdfDocument`] and exposes the patched PDFium edit and save surface.
///
/// All edits made through the high-level `pdfium-render` API before or during
/// the adapter's lifetime are part of the same document, so a later save writes
/// them in one revision.
pub struct PdfiumAdapter<'a> {
    document: PdfDocument<'a>,
}

impl<'a> PdfiumAdapter<'a> {
    /// Wraps an already-open PDFium document.
    pub fn new(document: PdfDocument<'a>) -> Self {
        Self { document }
    }
    /// Returns the wrapped document, for read-only high-level operations such
    /// as rendering or text extraction.
    pub fn document(&self) -> &PdfDocument<'a> {
        &self.document
    }

    /// Returns the wrapped document mutably, for high-level edits that
    /// `pdfium-render` already supports.
    pub fn document_mut(&mut self) -> &mut PdfDocument<'a> {
        &mut self.document
    }

    /// Consumes the adapter and returns the underlying document.
    pub fn into_document(self) -> PdfDocument<'a> {
        self.document
    }

    /// Returns `true` if the document has an active standard security handler.
    ///
    /// A document opened with its user password still reports the handler that
    /// protects it; only [SaveMode::RemoveSecurity] produces unencrypted output.
    pub fn is_encrypted(&self) -> Result<bool, AdapterError> {
        document_is_encrypted(&self.document)
    }

    /// Writes the document using the given [SaveMode].
    ///
    /// Incremental and plain full-rewrite saves of an encrypted document are
    /// refused with [AdapterError::EncryptedDocument] rather than silently
    /// writing encrypted output.
    pub fn save(&self, mode: SaveMode) -> Result<Vec<u8>, AdapterError> {
        save_document_with_mode(&self.document, mode)
    }

    /// Appends an incremental revision to the source bytes.
    pub fn save_incremental(&self) -> Result<Vec<u8>, AdapterError> {
        self.save(SaveMode::Incremental)
    }

    /// Rewrites the whole document.
    pub fn save_full_rewrite(&self) -> Result<Vec<u8>, AdapterError> {
        self.save(SaveMode::FullRewrite)
    }

    /// Rewrites the whole document without encryption.
    pub fn save_full_rewrite_removing_security(&self) -> Result<Vec<u8>, AdapterError> {
        self.save(SaveMode::RemoveSecurity)
    }

    /// Returns the number of annotations on page `page_index`.
    pub fn annotation_count(&self, page_index: i32) -> Result<usize, AdapterError> {
        let pages = self.document.pages();
        let page = pages.get(page_index)?;
        Ok(page.annotations().len())
    }

    /// Creates a circle annotation covering `rect` on page `page_index`.
    ///
    /// Pdfium does not generate an appearance for the annotation, so set a
    /// border and/or a custom appearance stream before saving.
    pub fn create_circle_annotation(
        &self,
        page_index: i32,
        rect: AnnotationRect,
    ) -> Result<AnnotationRef, AdapterError> {
        let pages = self.document.pages();
        let mut page = pages.get(page_index)?;
        let annotations = page.annotations_mut();
        let mut annotation = annotations.create_circle_annotation()?;
        annotation.set_bounds(rect.to_pdf_rect())?;
        annotation.close();
        Ok(AnnotationRef::new(page_index, annotations.len() - 1))
    }

    /// Creates an ink annotation covering `rect` on page `page_index`.
    ///
    /// Add one or more strokes with [Self::add_ink_stroke()]; Pdfium does not
    /// generate an appearance for the annotation, so also install a custom
    /// appearance stream before saving.
    pub fn create_ink_annotation(
        &self,
        page_index: i32,
        rect: AnnotationRect,
    ) -> Result<AnnotationRef, AdapterError> {
        let pages = self.document.pages();
        let mut page = pages.get(page_index)?;
        let annotations = page.annotations_mut();
        let mut annotation = annotations.create_ink_annotation()?;
        annotation.set_bounds(rect.to_pdf_rect())?;
        annotation.close();
        Ok(AnnotationRef::new(page_index, annotations.len() - 1))
    }

    /// Runs `operation` against the annotation identified by `target`.
    ///
    /// The annotation handle is closed when this method returns; callers only
    /// ever see a plain [AnnotationRef].
    fn with_annotation<R>(
        &self,
        target: AnnotationRef,
        operation: impl FnOnce(&mut PdfPageAnnotation<'_>) -> Result<R, PdfiumError>,
    ) -> Result<R, AdapterError> {
        with_annotation_on(&self.document, target, operation)
    }

    /// Returns the border width of the annotation, in default user space units.
    pub fn border_width(&self, target: AnnotationRef) -> Result<f32, AdapterError> {
        self.with_annotation(target, |annotation| annotation.border_width())
    }

    /// Sets the border width of the annotation, in default user space units.
    ///
    /// Pdfium drops any existing appearance stream when the border is set, so
    /// call this before [Self::set_appearance()].
    pub fn set_border_width(&self, target: AnnotationRef, width: f32) -> Result<(), AdapterError> {
        self.with_annotation(target, |annotation| annotation.set_border_width(width))
    }

    /// Installs `appearance` as the annotation's normal appearance stream.
    ///
    /// Pdfium creates a Form XObject whose `/BBox` is the annotation's `/Rect`
    /// at the time of this call, and the stream is interpreted in page
    /// coordinates. Set the annotation bounds first.
    pub fn set_appearance(
        &self,
        target: AnnotationRef,
        appearance: &str,
    ) -> Result<(), AdapterError> {
        set_annotation_appearance(&self.document, target, appearance)
    }

    /// Appends an ink stroke of page-space points to an ink annotation,
    /// returning the zero-based index of the new stroke.
    pub fn add_ink_stroke(
        &self,
        target: AnnotationRef,
        points: &[(f32, f32)],
    ) -> Result<usize, AdapterError> {
        self.with_annotation(target, |annotation| annotation.add_ink_stroke(points))
    }

    /// Returns the number of strokes in an ink annotation.
    pub fn ink_stroke_count(&self, target: AnnotationRef) -> Result<usize, AdapterError> {
        self.with_annotation(target, |annotation| annotation.ink_stroke_count())
    }
}
