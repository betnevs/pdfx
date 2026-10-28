//! Sequential incremental save probe: PDFium first, then lopdf.
//!
//! ADR-0004 requires object-level incremental writes and ADR-0010 splits the
//! engine: PDFium owns rendering/annotations, lopdf owns object edits. This
//! probe performs a single save cycle with both writers in sequence and checks
//! that neither stage loses the other's changes or the source structure.

use anyhow::{Context, Result};
use lopdf::{Document, IncrementalDocument, Object};
use pdfium_render::prelude::*;

use crate::engine::SpikeEngine;
use crate::raw::{ANNOT_COLOR, ANNOT_INTERIOR_COLOR, RawDocument, RectF, appearance_stream};

/// Result of the two-stage sequential save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequentialSave {
    /// Source bytes + the PDFium-appended revision.
    pub pdfium_bytes: Vec<u8>,
    /// `pdfium_bytes` + the lopdf-appended revision.
    pub final_bytes: Vec<u8>,
}

/// Saves once through both engines, in the order required by the spike:
///
/// 1. PDFium loads the source, adds a square native annotation with an
///    appearance stream, and calls `FPDF_SaveAsCopy` with `FPDF_INCREMENTAL`.
/// 2. lopdf parses that output and appends a second revision that edits the
///    document Info dictionary.
///
/// Neither stage rewrites existing bytes: each output starts with the bytes
/// of its input.
pub fn pdfium_then_lopdf(engine: &SpikeEngine, source: &[u8]) -> Result<SequentialSave> {
    let pdfium_bytes = pdfium_annotate(engine, source)?;
    let final_bytes = lopdf_append_marker(&pdfium_bytes)?;
    Ok(SequentialSave {
        pdfium_bytes,
        final_bytes,
    })
}

fn pdfium_annotate(engine: &SpikeEngine, source: &[u8]) -> Result<Vec<u8>> {
    let document = RawDocument::open(engine, source, None)?;
    {
        let page = document.page(0).context("load page 0")?;
        let bounds = RectF::new(100.0, 100.0, 140.0, 240.0);
        let square = page.create_annot(PdfPageAnnotationType::Square)?;
        square.set_rect(bounds)?;
        square.set_color(ANNOT_COLOR, [30, 90, 220, 255])?;
        square.set_color(ANNOT_INTERIOR_COLOR, [200, 225, 255, 255])?;
        square.set_contents("PDFium incremental square")?;
        square.set_creator("PDFX spike")?;
        square.set_appearance(&appearance_stream(
            PdfPageAnnotationType::Square as i32,
            bounds,
        ))?;
    }
    document.save_incremental()
}

fn lopdf_append_marker(pdfium_bytes: &[u8]) -> Result<Vec<u8>> {
    let parsed = Document::load_mem(pdfium_bytes).context("lopdf parse PDFium revision")?;
    let mut incremental = IncrementalDocument::create_from(pdfium_bytes.to_vec(), parsed);

    let info_id = incremental
        .new_document
        .trailer
        .get(b"Info")
        .and_then(Object::as_reference)
        .context("source has no /Info reference")?;
    incremental.opt_clone_object_to_new_document(info_id)?;
    let info = incremental
        .new_document
        .get_object_mut(info_id)?
        .as_dict_mut()
        .context("/Info is not a dictionary")?;
    info.set("PDFXSpike", Object::string_literal("lopdf-append"));

    let mut output = Vec::new();
    incremental.save_to(&mut output)?;
    Ok(output)
}
