//! Defines the [PdfPageTextAnnotation] struct, exposing functionality related to a single
//! user annotation of type [PdfPageAnnotationType::Text].

use crate::bindgen::{FPDF_ANNOTATION, FPDF_DOCUMENT, FPDF_PAGE};
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

/// A single [PdfPageAnnotation] of type [PdfPageAnnotationType::Text].
pub struct PdfPageTextAnnotation<'a> {
    handle: FPDF_ANNOTATION,
    objects: PdfPageAnnotationObjects<'a>,
    attachment_points: PdfPageAnnotationAttachmentPoints<'a>,
    lifetime: PhantomData<&'a FPDF_ANNOTATION>,
}

impl<'a> PdfPageTextAnnotation<'a> {
    pub(crate) fn from_pdfium(
        document_handle: FPDF_DOCUMENT,
        page_handle: FPDF_PAGE,
        annotation_handle: FPDF_ANNOTATION,
    ) -> Self {
        PdfPageTextAnnotation {
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

    /// Consumes this annotation and releases the underlying Pdfium annotation
    /// handle with `FPDFPage_CloseAnnot()`.
    ///
    /// Pdfium requires the caller to close every handle returned by
    /// `FPDFPage_CreateAnnot()`, but `pdfium-render` annotation types do not
    /// implement `Drop`. Use this method once the annotation is no longer needed.
    pub fn close(self) {
        unsafe {
            self.bindings().FPDFPage_CloseAnnot(self.handle);
        }
    }
}

impl<'a> PdfPageAnnotationPrivate<'a> for PdfPageTextAnnotation<'a> {
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

impl<'a> PdfiumLibraryBindingsAccessor<'a> for PdfPageTextAnnotation<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Send for PdfPageTextAnnotation<'a> {}

#[cfg(feature = "thread_safe")]
unsafe impl<'a> Sync for PdfPageTextAnnotation<'a> {}
