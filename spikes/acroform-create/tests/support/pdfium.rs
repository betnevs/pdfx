use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use lopdf::{Document, Object};
use pdfium_render::prelude::*;

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

pub fn pdfium() -> &'static Pdfium {
    let result = PDFIUM.get_or_init(|| {
        let path = library_path();
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&path))
            .map(Pdfium::new)
            .map_err(|error| {
                format!(
                    "cannot bind PDFium at {}: {error}; run ../pdf-engine/scripts/setup-native.sh \
                     or set PDFIUM_DYNAMIC_LIB_PATH",
                    path.display()
                )
            })
    });
    match result {
        Ok(pdfium) => pdfium,
        Err(message) => panic!("{message}"),
    }
}

fn library_path() -> PathBuf {
    if let Some(configured) = std::env::var_os("PDFIUM_DYNAMIC_LIB_PATH") {
        let path = PathBuf::from(configured);
        return if path.is_dir() {
            path
        } else {
            path.parent().unwrap().to_path_buf()
        };
    }

    let candidates = [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pdf-engine/vendor/pdfium/lib"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/vendor/pdfium/lib"),
    ];
    candidates
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "PDFium library not found; expected one of {:?} or PDFIUM_DYNAMIC_LIB_PATH",
                candidates
            )
        })
}

/// A rendered page as RGBA bytes.
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Rendered {
    pub fn dark_pixels_in(&self, rect: [f32; 4]) -> usize {
        let [x0, y0, x1, y1] = rect;
        // PDF user space is bottom-left; PDFium's bitmap is top-left.
        let left = x0.max(0.0).min(self.width as f32) as u32;
        let right = x1.max(0.0).min(self.width as f32) as u32;
        let top = (self.height as f32 - y1.max(0.0)).max(0.0) as u32;
        let bottom = (self.height as f32 - y0.max(0.0)).max(0.0) as u32;
        let mut count = 0;
        for y in top..bottom.min(self.height) {
            for x in left..right.min(self.width) {
                let index = ((y * self.width + x) * 4) as usize;
                let pixel = &self.rgba[index..index + 4];
                if pixel[0] < 96 && pixel[1] < 96 && pixel[2] < 96 {
                    count += 1;
                }
            }
        }
        count
    }

    pub fn pixels_differing(&self, other: &Rendered) -> Vec<(u32, u32)> {
        assert_eq!((self.width, self.height), (other.width, other.height));
        let mut differing = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let index = ((y * self.width + x) * 4) as usize;
                if self.rgba[index..index + 4] != other.rgba[index..index + 4] {
                    differing.push((x, y));
                }
            }
        }
        differing
    }
}

/// Returns a copy of the PDF with `/NeedAppearances` forced to `value`.
///
/// Generated forms carry explicit appearance streams, so their tests render
/// the bytes directly. This helper is for the merge fixture: the pre-existing
/// form sets `/NeedAppearances true`, and PDFium's plain page renderer then
/// delegates widgets to its form-fill path, which lacks the platform callbacks
/// an interactive viewer supplies and paints only part of the widgets. The
/// merge test forces the flag off on both sides so the pixel comparison
/// exercises the static appearance streams.
pub fn set_need_appearances(bytes: &[u8], value: bool) -> Vec<u8> {
    let mut document = Document::load_mem(bytes).expect("parse for NeedAppearances");
    let acroform_id = {
        let catalog = document.catalog().expect("catalog");
        match catalog.get(b"AcroForm") {
            Ok(Object::Reference(id)) => Some(*id),
            _ => None,
        }
    };
    let acroform = match acroform_id {
        Some(id) => document
            .get_object_mut(id)
            .and_then(Object::as_dict_mut)
            .expect("indirect AcroForm dictionary"),
        None => document
            .catalog_mut()
            .expect("catalog")
            .get_mut(b"AcroForm")
            .expect("AcroForm")
            .as_dict_mut()
            .expect("direct AcroForm dictionary"),
    };
    acroform.set("NeedAppearances", Object::Boolean(value));
    let mut output = Vec::new();
    document
        .save_to(&mut output)
        .expect("serialize patched PDF");
    output
}

pub fn open(bytes: &[u8]) -> PdfDocument<'_> {
    pdfium()
        .load_pdf_from_byte_slice(bytes, None)
        .expect("PDFium opens the generated document")
}

pub fn render(document: &PdfDocument<'_>, page_index: usize) -> Rendered {
    // Initialise PDFium's form-fill environment before rendering, the way an
    // interactive viewer does. Without it PDFium skips widget annotations when
    // the form asks readers to generate appearances.
    let _ = document.form();
    let page = document
        .pages()
        .get(page_index as i32)
        .expect("page exists");
    let width = page.width().value.round() as i32;
    let height = page.height().value.round() as i32;
    let bitmap = page
        .render(width, height, None)
        .expect("PDFium renders the page");
    Rendered {
        width: width as u32,
        height: height as u32,
        rgba: bitmap.as_rgba_bytes(),
    }
}
