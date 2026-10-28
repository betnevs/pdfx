//! Creation of the whitelisted native annotation types via `pdfium-render`.
//!
//! Whitelist (ADR-0010 / ticket 03): highlight, underline, strikeout, note
//! (Text), ink, square, circle. PDFium has no generic appearance generator, so
//! whether these render without a custom `/AP` stream is itself part of the probe.

use anyhow::{Result, anyhow};
use pdfium_render::prelude::*;

use crate::engine::SpikeEngine;
use crate::raw::{ANNOT_COLOR, RawDocument, RectF, appearance_stream};

/// The annotation types ticket 03 is allowed to create as native annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhitelistedAnnotation {
    Highlight,
    Underline,
    Strikeout,
    Note,
    Ink,
    Square,
    Circle,
}

impl WhitelistedAnnotation {
    pub const ALL: [WhitelistedAnnotation; 7] = [
        WhitelistedAnnotation::Highlight,
        WhitelistedAnnotation::Underline,
        WhitelistedAnnotation::Strikeout,
        WhitelistedAnnotation::Note,
        WhitelistedAnnotation::Ink,
        WhitelistedAnnotation::Square,
        WhitelistedAnnotation::Circle,
    ];

    pub fn annotation_type(self) -> PdfPageAnnotationType {
        match self {
            WhitelistedAnnotation::Highlight => PdfPageAnnotationType::Highlight,
            WhitelistedAnnotation::Underline => PdfPageAnnotationType::Underline,
            WhitelistedAnnotation::Strikeout => PdfPageAnnotationType::Strikeout,
            WhitelistedAnnotation::Note => PdfPageAnnotationType::Text,
            WhitelistedAnnotation::Ink => PdfPageAnnotationType::Ink,
            WhitelistedAnnotation::Square => PdfPageAnnotationType::Square,
            WhitelistedAnnotation::Circle => PdfPageAnnotationType::Circle,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WhitelistedAnnotation::Highlight => "highlight",
            WhitelistedAnnotation::Underline => "underline",
            WhitelistedAnnotation::Strikeout => "strikeout",
            WhitelistedAnnotation::Note => "note",
            WhitelistedAnnotation::Ink => "ink",
            WhitelistedAnnotation::Square => "square",
            WhitelistedAnnotation::Circle => "circle",
        }
    }
}

fn rect(bottom: f32, left: f32, top: f32, right: f32) -> PdfRect {
    PdfRect::new_from_values(bottom, left, top, right)
}

/// Creates one of each whitelisted annotation on `page`, returning the types created.
pub fn add_whitelisted_annotations(page: &mut PdfPage<'_>) -> Result<Vec<WhitelistedAnnotation>> {
    let yellow = PdfColor::new(255, 221, 0, 255);
    let red = PdfColor::new(220, 30, 30, 255);
    let blue = PdfColor::new(30, 90, 220, 255);

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_highlight_annotation()
            .map_err(|e| anyhow!("create highlight: {e}"))?;
        let bounds = rect(700.0, 400.0, 720.0, 580.0);
        annotation.set_bounds(bounds)?;
        annotation.set_stroke_color(yellow)?;
        annotation.set_contents("PDFX highlight")?;
        annotation.set_creator("PDFX spike")?;
        annotation
            .attachment_points_mut()
            .create_attachment_point_at_end(bounds.to_quad_points())?;
    }

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_underline_annotation()
            .map_err(|e| anyhow!("create underline: {e}"))?;
        let bounds = rect(660.0, 400.0, 680.0, 580.0);
        annotation.set_bounds(bounds)?;
        annotation.set_stroke_color(red)?;
        annotation.set_contents("PDFX underline")?;
        annotation.set_creator("PDFX spike")?;
        annotation
            .attachment_points_mut()
            .create_attachment_point_at_end(bounds.to_quad_points())?;
    }

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_strikeout_annotation()
            .map_err(|e| anyhow!("create strikeout: {e}"))?;
        let bounds = rect(620.0, 400.0, 640.0, 580.0);
        annotation.set_bounds(bounds)?;
        annotation.set_stroke_color(red)?;
        annotation.set_contents("PDFX strikeout")?;
        annotation.set_creator("PDFX spike")?;
        annotation
            .attachment_points_mut()
            .create_attachment_point_at_end(bounds.to_quad_points())?;
    }

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_text_annotation("PDFX note")
            .map_err(|e| anyhow!("create note: {e}"))?;
        annotation.set_bounds(rect(570.0, 400.0, 590.0, 420.0))?;
        annotation.set_stroke_color(yellow)?;
        annotation.set_creator("PDFX spike")?;
    }

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_ink_annotation()
            .map_err(|e| anyhow!("create ink: {e}"))?;
        annotation.set_bounds(rect(495.0, 395.0, 545.0, 585.0))?;
        annotation.set_stroke_color(blue)?;
        annotation.set_contents("PDFX ink")?;
        annotation.set_creator("PDFX spike")?;
        annotation.objects_mut().create_path_object_line(
            PdfPoints::new(400.0),
            PdfPoints::new(500.0),
            PdfPoints::new(580.0),
            PdfPoints::new(540.0),
            blue,
            PdfPoints::new(3.0),
        )?;
    }

    {
        let annotations = page.annotations_mut();
        let mut annotation = annotations
            .create_square_annotation()
            .map_err(|e| anyhow!("create square: {e}"))?;
        annotation.set_bounds(rect(430.0, 400.0, 480.0, 580.0))?;
        annotation.set_stroke_color(blue)?;
        annotation.set_fill_color(PdfColor::new(200, 225, 255, 255))?;
        annotation.set_contents("PDFX square")?;
        annotation.set_creator("PDFX spike")?;
    }

    Ok(whitelist_high_level().to_vec())
}

/// The subset `pdfium-render` 0.9.4 can create through its high-level API.
/// Circle has no high-level creator and is added by
/// [`add_appearance_streams_and_circle`].
pub fn whitelist_high_level() -> &'static [WhitelistedAnnotation] {
    &[
        WhitelistedAnnotation::Highlight,
        WhitelistedAnnotation::Underline,
        WhitelistedAnnotation::Strikeout,
        WhitelistedAnnotation::Note,
        WhitelistedAnnotation::Ink,
        WhitelistedAnnotation::Square,
    ]
}

/// Completes the native-annotation probe using raw PDFium calls:
///
/// 1. Loads `pdf` (typically the output of `PdfDocument::save_to_bytes()`).
/// 2. Installs a visible `/AP` appearance stream on every existing annotation.
///    PDFium does not generate one for these types, so without `/AP` the
///    annotation is present in the file but not painted.
/// 3. Creates the circle annotation, which `pdfium-render` 0.9.4 cannot create
///    through its high-level API.
/// 4. Saves an **incremental** revision (`FPDF_SaveAsCopy` with
///    `FPDF_INCREMENTAL`), the save mode required by ADR-0004.
///
/// Returns the new, complete file bytes.
pub fn add_appearance_streams_and_circle(
    engine: &SpikeEngine,
    pdf: &[u8],
) -> Result<Vec<u8>> {
    let document = RawDocument::open(engine, pdf, None)?;
    {
        let page = document.page(0)?;
        for index in 0..page.annot_count() {
            let annotation = page.get_annot(index)?;
            let rect = annotation.rect()?;
            annotation.set_appearance(&appearance_stream(annotation.subtype(), rect))?;
        }

        let bounds = RectF::new(360.0, 400.0, 410.0, 580.0);
        let circle = page.create_annot(PdfPageAnnotationType::Circle)?;
        circle.set_rect(bounds)?;
        circle.set_color(ANNOT_COLOR, [220, 30, 30, 255])?;
        circle.set_string("Contents", "PDFX circle")?;
        circle.set_string("T", "PDFX spike")?;
        circle.set_appearance(&appearance_stream(
            PdfPageAnnotationType::Circle as i32,
            bounds,
        ))?;
    }
    document.save_incremental()
}
