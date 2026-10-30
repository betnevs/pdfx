//! Prepare one lossless background and a transparent glyph image for a move.
//! Pointer motion then changes only a CSS transform, not the PDF or its history.
use crate::commands::{
    self, CommandError, CommandErrorCode, DisplayGeometry, OpenDocument, PageImageView,
    PageRegionView,
};
use base64::Engine;
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfColor, PdfPage, PdfPageObjectCommon, PdfPageObjectsCommon,
    PdfRenderConfig,
};
use serde::Serialize;
use std::ops::Range;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxMovePreview {
    pub background: PageImageView,
    pub foreground: PageRegionView,
}

pub(crate) struct Capture {
    width: u32,
    height: u32,
    scale: f32,
    x: u32,
    y: u32,
    sprite_width: u32,
    sprite_height: u32,
    background: Vec<u8>,
    foreground: Vec<u8>,
}

impl Capture {
    /// Encoding happens after the document registry lock has been released.
    pub(crate) fn encode(
        self,
        document_id: String,
        page_index: u32,
    ) -> Result<TextBoxMovePreview, CommandError> {
        let background = commands::encode_preview_png(&self.background, self.width, self.height)?;
        let foreground =
            commands::encode_preview_png(&self.foreground, self.sprite_width, self.sprite_height)?;
        Ok(TextBoxMovePreview {
            background: PageImageView {
                document_id: document_id.clone(),
                page_index,
                scale: self.scale,
                width: self.width,
                height: self.height,
                png_base64: base64::engine::general_purpose::STANDARD.encode(background),
            },
            foreground: PageRegionView {
                document_id,
                page_index,
                scale: self.scale,
                x: self.x,
                y: self.y,
                width: self.sprite_width,
                height: self.sprite_height,
                png_base64: base64::engine::general_purpose::STANDARD.encode(foreground),
            },
        })
    }
}

fn failed(error: impl std::fmt::Debug) -> CommandError {
    CommandError::new(
        CommandErrorCode::RenderFailed,
        format!("could not prepare text move preview: {error:?}"),
    )
}

/// Restore on both ordinary errors and unwinding; no transient active state
/// can escape the registry guard. No content stream is regenerated here.
struct ActiveObjects<'a, 'b> {
    page: &'a PdfPage<'b>,
    states: Vec<bool>,
    restored: bool,
}
impl<'a, 'b> ActiveObjects<'a, 'b> {
    fn new(page: &'a PdfPage<'b>) -> Result<Self, CommandError> {
        let states = page
            .objects()
            .iter()
            .map(|object| object.is_active().map_err(failed))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            page,
            states,
            restored: false,
        })
    }
    fn show(&self, visible: impl Fn(usize) -> bool) -> Result<(), CommandError> {
        for (index, original) in self.states.iter().enumerate() {
            let mut object = self.page.objects().get(index).map_err(failed)?;
            if *original && visible(index) {
                object.set_active().map_err(failed)?;
            } else {
                object.set_inactive().map_err(failed)?;
            }
        }
        Ok(())
    }
    fn restore(&mut self) -> Result<(), CommandError> {
        self.show(|_| true)?;
        self.restored = true;
        Ok(())
    }
}
impl Drop for ActiveObjects<'_, '_> {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.restore();
        }
    }
}

fn region(
    page: &PdfPage<'_>,
    full: (u32, u32),
    rect: (u32, u32, u32, u32),
    transparent: bool,
) -> Result<Vec<u8>, CommandError> {
    let (x, y, width, height) = rect;
    let mut bitmap =
        PdfBitmap::empty(width as i32, height as i32, PdfBitmapFormat::BGRA).map_err(failed)?;
    let config = PdfRenderConfig::new()
        .set_fixed_size(full.0 as i32, full.1 as i32)
        .set_origin(-(x as i32), -(y as i32))
        .render_annotations(false)
        .set_clear_color(if transparent {
            PdfColor::new(0, 0, 0, 0)
        } else {
            PdfColor::WHITE
        });
    page.render_into_bitmap_with_config(&mut bitmap, &config)
        .map_err(failed)?;
    Ok(bitmap.as_rgba_bytes())
}

pub(crate) fn capture(
    entry: &mut OpenDocument,
    page_index: u32,
    selected: Range<usize>,
    scale: f32,
) -> Result<Option<Capture>, CommandError> {
    if !scale.is_finite() || !(0.1..=2.0).contains(&scale) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidScale,
            "move preview scale must be between 0.1 and 2",
        ));
    }
    // Layer overrides use a separate rendering document whose object indices
    // may differ. Annotations also have their own stacking order. Those pages
    // retain the existing whole-page preview path.
    let live = &entry.document as *const _;
    if !std::ptr::eq(entry.render_document()?, live) {
        return Ok(None);
    }
    let physical = entry.physical_page(page_index)?;
    let page = commands::load_page(&entry.document, physical)?;
    if !page.annotations().is_empty() {
        return Ok(None);
    }
    let rotation = page.rotation().map_err(failed)?.as_degrees() as u16;
    let geometry = DisplayGeometry::new(&page, rotation)?;
    let (width, height) = commands::page_pixel_size(&page, scale);
    if width as i64 * height as i64 > commands::MAX_RENDER_PIXELS {
        return Ok(None);
    }
    let mut bounds = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for index in selected.clone() {
        let object = page.objects().get(index).map_err(failed)?;
        let rect = object.bounds().map_err(failed)?;
        for (px, py) in [
            (rect.left().value, rect.bottom().value),
            (rect.right().value, rect.bottom().value),
            (rect.left().value, rect.top().value),
            (rect.right().value, rect.top().value),
        ] {
            let (x, y) = geometry.point(px, py);
            bounds.0 = bounds.0.min(x);
            bounds.1 = bounds.1.min(y);
            bounds.2 = bounds.2.max(x);
            bounds.3 = bounds.3.max(y);
        }
    }
    if !bounds.0.is_finite()
        || bounds.0 < 0.0
        || bounds.1 < 0.0
        || bounds.2 > page.width().value
        || bounds.3 > page.height().value
    {
        return Ok(None);
    }
    let x = ((bounds.0 - 4.0).max(0.0) * scale).floor() as u32;
    let y = ((bounds.1 - 4.0).max(0.0) * scale).floor() as u32;
    let right = (((bounds.2 + 4.0) * scale).ceil() as u32).min(width);
    let bottom = (((bounds.3 + 4.0) * scale).ceil() as u32).min(height);
    let (sw, sh) = (right.saturating_sub(x), bottom.saturating_sub(y));
    if sw == 0 || sh == 0 {
        return Ok(None);
    }
    let original = region(&page, (width, height), (x, y, sw, sh), false)?;
    let mut visibility = ActiveObjects::new(&page)?;
    let captured = (|| {
        visibility.show(|i| !selected.contains(&i))?;
        let background = commands::render_page_rgba(&page, page_index, width, height)?;
        visibility.show(|i| selected.contains(&i))?;
        let foreground = region(&page, (width, height), (x, y, sw, sh), true)?;
        // Do not approximate unusual blend/clipping/occlusion. The layers
        // must reconstruct the original pixels at the original location.
        let mut mismatches = 0usize;
        for row in 0..sh {
            for col in 0..sw {
                let i = ((row * sw + col) * 4) as usize;
                let b = (((row + y) * width + col + x) * 4) as usize;
                let alpha = u32::from(foreground[i + 3]);
                if (0..3).any(|c| {
                    let mixed = (u32::from(foreground[i + c]) * alpha
                        + u32::from(background[b + c]) * (255 - alpha)
                        + 127)
                        / 255;
                    (mixed as i32 - i32::from(original[i + c])).abs() > 4
                }) {
                    mismatches += 1;
                }
            }
        }
        if mismatches * 100 > (sw * sh) as usize {
            return Ok(None);
        }
        Ok(Some(Capture {
            width,
            height,
            scale,
            x,
            y,
            sprite_width: sw,
            sprite_height: sh,
            background,
            foreground,
        }))
    })();
    visibility.restore()?;
    captured
}
