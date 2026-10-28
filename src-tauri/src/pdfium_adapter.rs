//! Safe adapter over the vendored `pdfium-render` patch.
//!
//! The adapter always operates on the same `PdfDocument` handle that holds
//! high-level `pdfium-render` edits. It adds only the capabilities PDFium
//! supports but `pdfium-render` 0.9.4 does not expose: flags-aware saves,
//! circle-annotation creation, ink strokes, annotation border width and custom
//! appearance streams. The patch itself lives in
//! `vendor/pdfium-render/PDFX-PATCH.md`; callers of this module never need
//! `unsafe` and never touch a PDFium handle.
//!
//! Saving is explicit: callers choose [SaveMode::Incremental] (append a
//! revision whose output begins with the source bytes), [SaveMode::FullRewrite]
//! or [SaveMode::RemoveSecurity] (full rewrite without encryption). The save
//! pipeline (ticket 10) is expected to append `lopdf` revisions to the bytes
//! returned here, never to the source document.

use std::fmt;

use pdfium_render::prelude::{
    PdfDocument, PdfPageAnnotation, PdfPageAnnotationCommon, PdfPoints, PdfRect,
    PdfSecurityHandlerRevision, PdfiumError, FPDF_DWORD,
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
        Ok(self.document.permissions().security_handler_revision()?
            != PdfSecurityHandlerRevision::Unprotected)
    }

    /// Writes the document using the given [SaveMode].
    ///
    /// Incremental and plain full-rewrite saves of an encrypted document are
    /// refused with [AdapterError::EncryptedDocument] rather than silently
    /// writing encrypted output.
    pub fn save(&self, mode: SaveMode) -> Result<Vec<u8>, AdapterError> {
        if mode != SaveMode::RemoveSecurity && self.is_encrypted()? {
            return Err(AdapterError::EncryptedDocument);
        }

        Ok(self.document.save_to_bytes_with_flags(mode.flags())?)
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
        let pages = self.document.pages();
        let page = pages.get(target.page_index)?;
        let annotations = page.annotations();
        let mut annotation = annotations.get(target.index)?;
        Ok(operation(&mut annotation)?)
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
        self.with_annotation(target, |annotation| annotation.set_appearance(appearance))
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
