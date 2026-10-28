//! Defines the [PdfPageInkAnnotation] struct, exposing functionality related to a single
//! user annotation of type [PdfPageAnnotationType::Ink].

use crate::bindgen::{FPDF_ANNOTATION, FPDF_DOCUMENT, FPDF_PAGE, FS_POINTF};
use crate::error::{PdfiumError, PdfiumInternalError};
use crate::pdf::document::page::annotation::attachment_points::PdfPageAnnotationAttachmentPoints;
use crate::pdf::document::page::annotation::objects::PdfPageAnnotationObjects;
use crate::pdf::document::page::annotation::private::internal::PdfPageAnnotationPrivate;
use crate::pdf::document::page::object::ownership::PdfPageObjectOwnership;
use crate::pdf::document::page::objects::private::internal::PdfPageObjectsPrivate;
use crate::pdfium::PdfiumLibraryBindingsAccessor;
use std::marker::PhantomData;

#[cfg(doc)]
use {
    crate::pdf::document::page::annotation::PdfPageAnnotation,
    crate::pdf::document::page::annotation::PdfPageAnnotationType,
};

/// A single [PdfPageAnnotation] of type [PdfPageAnnotationType::Ink].
pub struct PdfPageInkAnnotation<'a> {
    handle: FPDF_ANNOTATION,
    objects: PdfPageAnnotationObjects<'a>,
    attachment_points: PdfPageAnnotationAttachmentPoints<'a>,
    lifetime: PhantomData<&'a FPDF_ANNOTATION>,
}

impl<'a> PdfPageInkAnnotation<'a> {
    pub(crate) fn from_pdfium(
        document_handle: FPDF_DOCUMENT,
        page_handle: FPDF_PAGE,
        annotation_handle: FPDF_ANNOTATION,
    ) -> Self {
        PdfPageInkAnnotation {
            handle: annotation_handle,
            objects: PdfPageAnnotationObjects::from_pdfium(
                document_handle,
                page_handle,
                annotation_handle,
            ),
            attachment_points: PdfPageAnnotationAttachmentPoints::from_pdfium(annotation_handle),
            lifetime: PhantomData,
        }
    }

    /// Consumes this [PdfPageInkAnnotation] and releases the underlying Pdfium
    /// annotation handle with `FPDFPage_CloseAnnot()`.
    ///
    /// Pdfium requires the caller to close every handle returned by
    /// `FPDFPage_CreateAnnot()`, but `pdfium-render` annotation types do not
    /// implement `Drop`. Use this method once the annotation is no longer needed.
    pub fn close(self) {
        unsafe {
            self.bindings().FPDFPage_CloseAnnot(self.handle);
        }
    }

    /// Appends an ink stroke, represented by the given page-space points, to the
    /// `/InkList` of this [PdfPageInkAnnotation].
    ///
    /// Pdfium creates the `/InkList` entry if it does not already exist. Returns
    /// the zero-based index at which the new stroke was added.
    pub fn add_ink_stroke(&mut self, points: &[(f32, f32)]) -> Result<usize, PdfiumError> {
        let pdfium_points: Vec<FS_POINTF> = points
            .iter()
            .map(|&(x, y)| FS_POINTF { x, y })
            .collect();

        let index = unsafe {
            self.bindings().FPDFAnnot_AddInkStroke(
                self.handle(),
                pdfium_points.as_ptr(),
                pdfium_points.len(),
            )
        };

        if index < 0 {
            Err(PdfiumError::PdfiumLibraryInternalError(
                PdfiumInternalError::Unknown,
            ))
        } else {
            Ok(index as usize)
        }
    }

    /// Returns the number of ink strokes in the `/InkList` of this [PdfPageInkAnnotation].
    pub fn ink_stroke_count(&self) -> usize {
        unsafe { self.bindings().FPDFAnnot_GetInkListCount(self.handle()) as usize }
    }

    /// Returns every stroke in the `/InkList` of this [PdfPageInkAnnotation].
    ///
    /// Each stroke is a list of page-space points. The points are read back
    /// through `FPDFAnnot_GetInkListPath()` so undo and style changes can
    /// recreate a stroke-based snapshot instead of relying on a private copy.
    pub fn ink_strokes(&self) -> Vec<Vec<(f32, f32)>> {
        (0..self.ink_stroke_count())
            .map(|index| {
                let length = unsafe {
                    self.bindings().FPDFAnnot_GetInkListPath(
                        self.handle(),
                        index as _,
                        std::ptr::null_mut(),
                        0,
                    )
                } as usize;

                if length == 0 {
                    return Vec::new();
                }

                let mut points = vec![FS_POINTF { x: 0.0, y: 0.0 }; length];
                let written = unsafe {
                    self.bindings().FPDFAnnot_GetInkListPath(
                        self.handle(),
                        index as _,
                        points.as_mut_ptr(),
                        length as _,
                    )
                } as usize;
                points.truncate(written);
                points
                    .into_iter()
                    .map(|point| (point.x, point.y))
                    .collect()
            })
            .collect()
    }

    /// Returns a mutable collection of all the page objects in this [PdfPageInkAnnotation].
    #[inline]
    pub fn objects_mut(&mut self) -> &mut PdfPageAnnotationObjects<'a> {
        &mut self.objects
    }
}

impl<'a> PdfPageAnnotationPrivate<'a> for PdfPageInkAnnotation<'a> {
    #[inline]
    fn handle(&self) -> FPDF_ANNOTATION {
        self.handle
    }

    #[inline]
    fn ownership(&self) -> &PdfPageObjectOwnership {
        self.objects_impl().ownership()
    }

    #[inline]
    fn objects_impl(&self) -> &PdfPageAnnotationObjects<'_> {
        &self.objects
    }

    #[inline]
    fn attachment_points_impl(&self) -> &PdfPageAnnotationAttachmentPoints<'_> {
        &self.attachment_points
    }
}

impl<'a> PdfiumLibraryBindingsAccessor<'a> for PdfPageInkAnnotation<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Send for PdfPageInkAnnotation<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Sync for PdfPageInkAnnotation<'a> {}
