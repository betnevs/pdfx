//! Thin raw-PDFium helpers on top of `pdfium-render`'s public bindings table.
//!
//! These exist because `pdfium-render` 0.9.4 does not expose:
//!   * `FPDF_SaveAsCopy` with `FPDF_INCREMENTAL` (its `save_to_writer` hardcodes 0),
//!   * `FPDFPage_CreateAnnot` for circle annotations,
//!   * `FPDFAnnot_SetAP` (custom appearance streams),
//! nor does it expose raw document/page/annotation handles.
//!
//! Everything here still goes through `pdfium-render`'s `PdfiumLibraryBindings`
//! abstraction; no separate FFI loader is involved.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::os::raw::{c_int, c_ulong};

use anyhow::{Result, anyhow, bail};
use pdfium_render::prelude::*;

use crate::engine::SpikeEngine;

/// PDFium save flag: append an incremental revision instead of rewriting the file.
pub const FPDF_INCREMENTAL: FPDF_DWORD = 1;

/// `FPDFANNOT_COLORTYPE_Color` — the annotation's stroke/text color.
pub const ANNOT_COLOR: FPDFANNOT_COLORTYPE = 0;
/// `FPDFANNOT_COLORTYPE_InteriorColor` — the annotation's fill color.
pub const ANNOT_INTERIOR_COLOR: FPDFANNOT_COLORTYPE = 1;
/// `FPDF_ANNOT_APPEARANCEMODE_NORMAL`.
pub const ANNOT_APPEARANCE_NORMAL: FPDF_ANNOT_APPEARANCEMODE = 0;

/// A rectangle in PDF page coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

impl RectF {
    pub const fn new(left: f32, bottom: f32, right: f32, top: f32) -> Self {
        Self {
            left,
            bottom,
            right,
            top,
        }
    }

    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.top - self.bottom
    }

    fn as_pdfium(self) -> FS_RECTF {
        FS_RECTF {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }

    fn from_pdfium(rect: FS_RECTF) -> Self {
        Self {
            left: rect.left,
            bottom: rect.bottom,
            right: rect.right,
            top: rect.top,
        }
    }
}

#[repr(C)]
struct FileWriter {
    base: FPDF_FILEWRITE,
    buffer: Vec<u8>,
}

unsafe extern "C" fn write_block(
    this: *mut FPDF_FILEWRITE,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    if this.is_null() || (data.is_null() && size != 0) {
        return 0;
    }
    // SAFETY: `this` was created from a `&mut FileWriter` whose first field is
    // `base: FPDF_FILEWRITE`; a pointer to that field is a pointer to the struct.
    let writer = unsafe { &mut *(this as *mut FileWriter) };
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) };
    writer.buffer.extend_from_slice(bytes);
    1
}

/// An owned raw `FPDF_DOCUMENT`.
pub struct RawDocument<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    handle: FPDF_DOCUMENT,
    marker: PhantomData<&'a ()>,
}

impl<'a> RawDocument<'a> {
    pub fn open(
        engine: &'a SpikeEngine,
        bytes: &[u8],
        password: Option<&str>,
    ) -> Result<Self> {
        let bindings = engine.raw();
        // SAFETY: `bytes` outlives the call; PDFium copies the buffer it needs.
        let handle = unsafe { bindings.FPDF_LoadMemDocument64(bytes, password) };
        if handle.is_null() {
            let code = unsafe { bindings.FPDF_GetLastError() };
            bail!("FPDF_LoadMemDocument64 failed (FPDF error {code})");
        }
        Ok(Self {
            bindings,
            handle,
            marker: PhantomData,
        })
    }

    pub fn page(&self, index: i32) -> Result<RawPage<'_>> {
        let handle = unsafe { self.bindings.FPDF_LoadPage(self.handle, index) };
        if handle.is_null() {
            bail!("FPDF_LoadPage({index}) failed");
        }
        Ok(RawPage {
            bindings: self.bindings,
            document: self.handle,
            handle,
            marker: PhantomData,
        })
    }

    /// Saves an incremental revision (`FPDF_SaveAsCopy` with `FPDF_INCREMENTAL`).
    pub fn save_incremental(&self) -> Result<Vec<u8>> {
        let mut writer = FileWriter {
            base: FPDF_FILEWRITE {
                version: 1,
                WriteBlock: Some(write_block),
            },
            buffer: Vec::new(),
        };
        let ok = unsafe {
            self.bindings
                .FPDF_SaveAsCopy(self.handle, &mut writer.base, FPDF_INCREMENTAL)
        };
        if !self.bindings.is_true(ok) {
            bail!("FPDF_SaveAsCopy(FPDF_INCREMENTAL) failed");
        }
        Ok(writer.buffer)
    }

    pub fn handle(&self) -> FPDF_DOCUMENT {
        self.handle
    }
}

impl Drop for RawDocument<'_> {
    fn drop(&mut self) {
        unsafe { self.bindings.FPDF_CloseDocument(self.handle) };
    }
}

/// A raw page handle tied to its [`RawDocument`].
pub struct RawPage<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    handle: FPDF_PAGE,
    marker: PhantomData<&'a ()>,
}

impl<'a> RawPage<'a> {
    pub fn annot_count(&self) -> i32 {
        unsafe { self.bindings.FPDFPage_GetAnnotCount(self.handle) }
    }

    pub fn get_annot(&self, index: i32) -> Result<RawAnnotation<'_>> {
        let handle = unsafe { self.bindings.FPDFPage_GetAnnot(self.handle, index) };
        if handle.is_null() {
            bail!("FPDFPage_GetAnnot({index}) failed");
        }
        Ok(RawAnnotation {
            bindings: self.bindings,
            handle,
            marker: PhantomData,
        })
    }

    pub fn create_annot(
        &self,
        subtype: PdfPageAnnotationType,
    ) -> Result<RawAnnotation<'_>> {
        let handle = unsafe {
            self.bindings
                .FPDFPage_CreateAnnot(self.handle, subtype as FPDF_ANNOTATION_SUBTYPE)
        };
        if handle.is_null() {
            bail!("FPDFPage_CreateAnnot({subtype:?}) failed");
        }
        Ok(RawAnnotation {
            bindings: self.bindings,
            handle,
            marker: PhantomData,
        })
    }

    pub fn regenerate_content(&self) -> Result<()> {
        let ok = unsafe { self.bindings.FPDFPage_GenerateContent(self.handle) };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(anyhow!("FPDFPage_GenerateContent failed"))
        }
    }

    pub fn document(&self) -> FPDF_DOCUMENT {
        self.document
    }
}

impl Drop for RawPage<'_> {
    fn drop(&mut self) {
        unsafe { self.bindings.FPDF_ClosePage(self.handle) };
    }
}

/// A raw `FPDF_ANNOTATION` handle.
pub struct RawAnnotation<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    handle: FPDF_ANNOTATION,
    marker: PhantomData<&'a ()>,
}

impl<'a> RawAnnotation<'a> {
    pub fn subtype(&self) -> i32 {
        unsafe { self.bindings.FPDFAnnot_GetSubtype(self.handle) }
    }

    pub fn rect(&self) -> Result<RectF> {
        let mut rect = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        let ok = unsafe { self.bindings.FPDFAnnot_GetRect(self.handle, &mut rect) };
        if self.bindings.is_true(ok) {
            Ok(RectF::from_pdfium(rect))
        } else {
            Err(anyhow!("FPDFAnnot_GetRect failed"))
        }
    }

    pub fn set_rect(&self, rect: RectF) -> Result<()> {
        let ok = unsafe { self.bindings.FPDFAnnot_SetRect(self.handle, &rect.as_pdfium()) };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(anyhow!("FPDFAnnot_SetRect failed"))
        }
    }

    pub fn set_color(&self, color_type: FPDFANNOT_COLORTYPE, rgba: [u8; 4]) -> Result<()> {
        let ok = unsafe {
            self.bindings.FPDFAnnot_SetColor(
                self.handle,
                color_type,
                rgba[0] as u32,
                rgba[1] as u32,
                rgba[2] as u32,
                rgba[3] as u32,
            )
        };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(anyhow!("FPDFAnnot_SetColor failed"))
        }
    }

    pub fn set_string(&self, key: &str, value: &str) -> Result<()> {
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_SetStringValue_str(self.handle, key, value)
        };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(anyhow!("FPDFAnnot_SetStringValue({key}) failed"))
        }
    }

    pub fn set_contents(&self, contents: &str) -> Result<()> {
        self.set_string("Contents", contents)
    }

    pub fn set_creator(&self, creator: &str) -> Result<()> {
        self.set_string("T", creator)
    }

    /// Installs a Form XObject appearance stream generated from `content`.
    ///
    /// PDFium uses the annotation's `/Rect` as the stream's `/BBox`, so the
    /// content stream is written in page coordinates.
    pub fn set_appearance(&self, content: &str) -> Result<()> {
        let ok = unsafe {
            self.bindings.FPDFAnnot_SetAP_str(
                self.handle,
                ANNOT_APPEARANCE_NORMAL,
                content,
            )
        };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(anyhow!("FPDFAnnot_SetAP failed (rect may be empty)"))
        }
    }
}

impl Drop for RawAnnotation<'_> {
    fn drop(&mut self) {
        unsafe { self.bindings.FPDFPage_CloseAnnot(self.handle) };
    }
}

/// Builds a simple, visible appearance stream for the given annotation subtype.
///
/// PDFium has no generic annotation appearance generator; without `/AP` most
/// annotation types are not painted by PDFium's renderer. Coordinates are page
/// coordinates because PDFium sets the appearance `/BBox` to the annotation rect.
pub fn appearance_stream(subtype: i32, rect: RectF) -> String {
    let (l, b, r, t) = (rect.left, rect.bottom, rect.right, rect.top);
    let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
    let circle = PdfPageAnnotationType::Circle as i32;
    let highlight = PdfPageAnnotationType::Highlight as i32;
    let underline = PdfPageAnnotationType::Underline as i32;
    let strikeout = PdfPageAnnotationType::Strikeout as i32;
    let ink = PdfPageAnnotationType::Ink as i32;
    let square = PdfPageAnnotationType::Square as i32;

    if subtype == circle {
        let (cx, cy) = (l + w / 2.0, b + h / 2.0);
        let (rx, ry) = (w / 2.0 - 1.5, h / 2.0 - 1.5);
        let k = 0.552_284_8;
        return format!(
            "0 0.12 0.86 RG 3 w {cx} {y0} m \
             {cx1} {y0} {cx2} {cy1} {x1} {cy} c \
             {x1} {cy2} {cx1} {y1} {cx} {y1} c \
             {cx0} {y1} {x0} {cy2} {x0} {cy} c \
             {x0} {cy1} {cx0} {y0} {cx} {y0} c S\n",
            cx = cx,
            cy = cy,
            y0 = cy + ry,
            y1 = cy - ry,
            x0 = cx - rx,
            x1 = cx + rx,
            cx0 = cx - k * rx,
            cx1 = cx + k * rx,
            cx2 = cx + rx,
            cy1 = cy + k * ry,
            cy2 = cy - k * ry,
        );
    }
    if subtype == highlight || subtype == underline || subtype == strikeout {
        let color = if subtype == highlight {
            "1 0.87 0 rg"
        } else {
            "0.86 0.12 0.12 RG"
        };
        return if subtype == highlight {
            format!("{color} {l} {b} {w} {h} re f\n")
        } else {
            let y = if subtype == underline { b + 1.5 } else { b + h / 2.0 };
            format!("{color} 2 w {l} {y} m {r} {y} l S\n")
        };
    }
    if subtype == ink {
        return format!("0.12 0.35 0.86 RG 3 w {l} {b} m {r} {t} l S\n");
    }
    if subtype == square {
        return format!(
            "0.78 0.88 1 rg {l} {b} {w} {h} re f 0 0.35 0.86 RG 3 w {l} {b} {w} {h} re S\n"
        );
    }
    // Note icon (Text annotation) and any unknown type: a filled square.
    format!("1 0.87 0 {l} {b} {w} {h} re f\n")
}
