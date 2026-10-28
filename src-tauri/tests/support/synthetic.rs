//! Minimal in-memory PDFs for tests that need page geometry the shared
//! fixture corpus does not cover (offset CropBox, extra rotations, CJK fonts).
//!
//! The fixtures live in a unique temporary file for the duration of the test.

// Each integration test binary uses a different subset of these shared
// fixtures, so unused ones are expected rather than dead code.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TempPdf {
    path: PathBuf,
}

impl TempPdf {
    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn write_pdf(name: &str, bytes: Vec<u8>) -> TempPdf {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pdfx-text-layer-{}-{unique}-{name}.pdf",
        std::process::id()
    ));
    std::fs::write(&path, bytes).expect("write synthetic PDF");
    TempPdf { path }
}

fn build_pdf(mut objects: Vec<(u32, Vec<u8>)>, root: u32) -> Vec<u8> {
    objects.sort_by_key(|(number, _)| *number);
    let last = objects.last().expect("at least one object").0;

    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = vec![0usize; (last + 1) as usize];
    for (number, body) in &objects {
        offsets[*number as usize] = out.len();
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", last + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for number in 1..=last {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[number as usize]).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            last + 1
        )
        .as_bytes(),
    );
    out
}

fn stream(extra: &str, data: &[u8]) -> Vec<u8> {
    let mut body = format!("<< /Length {} {extra} >>\nstream\n", data.len()).into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

const HELVETICA: &[u8] =
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// One 612x792 page whose CropBox is [100 100 500 700], with Helvetica 12pt
/// text drawn at user-space (150, 600). The rendered page is 400x600 points
/// and the text starts 50 points from the left and 100 points from the top of
/// the *crop* box.
pub fn crop_offset_pdf() -> TempPdf {
    let content = b"BT /F1 12 Tf 150 600 Td (Crop offset) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, HELVETICA.to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [100 100 500 700] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_vec(),
        ),
        (5, stream("", &content)),
    ];
    write_pdf("crop-offset", build_pdf(objects, 1))
}

/// Three 612x792 pages sharing a [50 80 450 680] CropBox, rotated 90, 180 and
/// 270 degrees. Each page draws Helvetica 12pt text at user-space (100, 600).
pub fn rotated_crop_pdf() -> TempPdf {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (3, HELVETICA.to_vec()),
    ];
    let mut kids = Vec::new();
    let mut next = 4;
    for rotation in [90u16, 180, 270] {
        let page_number = next;
        let content_number = next + 1;
        next += 2;
        kids.push(format!("{page_number} 0 R"));
        objects.push((
            page_number,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [50 80 450 680] /Rotate {rotation} /Resources << /Font << /F1 3 0 R >> >> /Contents {content_number} 0 R >>"
            )
            .into_bytes(),
        ));
        let content = format!("BT /F1 12 Tf 100 600 Td (Rotate {rotation}) Tj ET\n");
        objects.push((content_number, stream("", content.as_bytes())));
    }
    objects.push((
        2,
        format!("<< /Type /Pages /Kids [{}] /Count 3 >>", kids.join(" ")).into_bytes(),
    ));
    write_pdf("rotated-crop", build_pdf(objects, 1))
}

fn utf16be_hex(text: &str) -> String {
    text.encode_utf16()
        .map(|unit| format!("{unit:04X}"))
        .collect()
}

/// A 612x792 page with a non-embedded Type0/CIDFontType2 font using the
/// predefined UniGB-UCS2-H CMap, drawing mixed Chinese and Latin text at
/// Helvetica-compatible 18pt.
pub fn cjk_mixed_pdf() -> TempPdf {
    let text = "中文 PDF 混合";
    let content = format!("BT /F1 18 Tf 72 720 Td <{}> Tj ET\n", utf16be_hex(text)).into_bytes();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [6 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Font /Subtype /Type0 /BaseFont /SimSun /Encoding /UniGB-UCS2-H /DescendantFonts [4 0 R] >>".to_vec(),
        ),
        (
            4,
            b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /SimSun /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /FontDescriptor 5 0 R /DW 1000 >>".to_vec(),
        ),
        (
            5,
            b"<< /Type /FontDescriptor /FontName /SimSun /Flags 4 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 80 >>".to_vec(),
        ),
        (
            6,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".to_vec(),
        ),
        (7, stream("", &content)),
    ];
    write_pdf("cjk-mixed", build_pdf(objects, 1))
}

/// A 612x792 page whose Latin character code is mapped by ToUnicode to
/// U+1F600, a supplementary-plane character stored as a UTF-16 surrogate pair.
pub fn surrogate_pair_pdf() -> TempPdf {
    let to_unicode = b"/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n\
/CMapType 2 def\n\
1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
1 beginbfchar\n<41> <D83DDE00>\nendbfchar\n\
endcmap\n\
CMapName currentdict /CMap defineresource pop\n\
end\nend\n";
    let content = b"BT /F1 18 Tf 72 720 Td (A) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [5 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding /ToUnicode 4 0 R >>".to_vec(),
        ),
        (4, stream("", to_unicode)),
        (
            5,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 6 0 R >>".to_vec(),
        ),
        (6, stream("", &content)),
    ];
    write_pdf("surrogate-pair", build_pdf(objects, 1))
}

/// A 612x792 page with one line of Helvetica 18pt text at user-space
/// (72, 720), so editing it changes exactly one Text Object.
pub fn single_line_pdf() -> TempPdf {
    let content = b"BT /F1 18 Tf 72 720 Td (Original line.) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, HELVETICA.to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_vec(),
        ),
        (5, stream("", &content)),
    ];
    write_pdf("single-line", build_pdf(objects, 1))
}

/// Two 612x792 pages drawing the same Helvetica 18pt line at user-space
/// (72, 720). Page objects are 4 (page 1) and 6 (page 2), so a saved
/// destination for the second page must reference object 6.
pub fn two_page_pdf() -> TempPdf {
    let content = b"BT /F1 18 Tf 72 720 Td (Original line.) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".to_vec()),
        (3, HELVETICA.to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_vec(),
        ),
        (5, stream("", &content)),
        (
            6,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".to_vec(),
        ),
        (7, stream("", &content)),
    ];
    write_pdf("two-page", build_pdf(objects, 1))
}

/// Two 612x792 pages carrying links written by another tool.
///
/// Page 1 has a referenced URI link (object 8), an inline JavaScript link PDFX
/// cannot represent, and a plain Square annotation, so tests can check that
/// editing one annotation preserves the others. Page 2 is an empty target page.
pub fn foreign_links_pdf() -> TempPdf {
    let content = b"BT /F1 18 Tf 72 720 Td (Original line.) Tj ET\n".to_vec();
    let page_two = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".to_vec()),
        (3, HELVETICA.to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R /Annots [8 0 R << /Type /Annot /Subtype /Link /Rect [200 690 300 710] /A << /S /JavaScript /JS (app.alert) >> >> 9 0 R] >>".to_vec(),
        ),
        (5, stream("", &content)),
        (6, page_two),
        (7, stream("", &content)),
        (
            8,
            b"<< /Type /Annot /Subtype /Link /Rect [72 690 180 710] /Border [0 0 0] /A << /S /URI /URI (https://foreign.example/one) >> >>".to_vec(),
        ),
        (
            9,
            b"<< /Type /Annot /Subtype /Square /Rect [250 500 350 560] /C [1 0 0] >>".to_vec(),
        ),
    ];
    write_pdf("foreign-links", build_pdf(objects, 1))
}

/// Two 612x792 pages with the destination forms other writers emit.
///
/// Page 1 carries four foreign links: a `/GoTo` action, a direct `/Dest` array,
/// a `/Dest` naming a document destination, and a `/Dest` naming a destination
/// that does not exist. All four point at page 2 when they resolve.
pub fn destination_links_pdf() -> TempPdf {
    let content = b"BT /F1 18 Tf 72 720 Td (Original line.) Tj ET\n".to_vec();
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /Names << /Dests 10 0 R >> >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".to_vec()),
        (3, HELVETICA.to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R /Annots [8 0 R << /Type /Annot /Subtype /Link /Rect [200 690 300 710] /Dest [6 0 R /Fit] >> 9 0 R 11 0 R] >>".to_vec(),
        ),
        (5, stream("", &content)),
        (
            6,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".to_vec(),
        ),
        (7, stream("", &content)),
        (
            8,
            b"<< /Type /Annot /Subtype /Link /Rect [72 690 180 710] /A << /S /GoTo /D [6 0 R /Fit] >> >>".to_vec(),
        ),
        (
            9,
            b"<< /Type /Annot /Subtype /Link /Rect [320 690 420 710] /Dest (chapter-two) >>".to_vec(),
        ),
        (10, b"<< /Names [(chapter-two) [6 0 R /Fit]] >>".to_vec()),
        (
            11,
            b"<< /Type /Annot /Subtype /Link /Rect [440 690 540 710] /Dest (missing-destination) >>".to_vec(),
        ),
    ];
    write_pdf("destination-links", build_pdf(objects, 1))
}

/// Wraps arbitrary bytes (for example a document saved by the backend) in a
/// temporary file so it can be reopened through the command boundary.
pub fn pdf_bytes_file(name: &str, bytes: Vec<u8>) -> TempPdf {
    write_pdf(name, bytes)
}

/// Liberation Sans, used to build a PDF with a real embedded TrueType font.
/// Liberation fonts are SIL Open Font License; see `tests/fonts/LICENSE_LIBERATION`.
const EMBEDDED_FONT: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

fn scale_to_1000(value: i16, units_per_em: u16) -> i64 {
    (f64::from(value) * 1000.0 / f64::from(units_per_em)).round() as i64
}

/// A 612x792 page drawing 18pt Liberation Sans as an embedded simple TrueType
/// font with WinAnsiEncoding. The font program retains its cmap, so every
/// Latin character can be checked for glyph coverage.
pub fn embedded_true_type_pdf() -> TempPdf {
    let face = ttf_parser::Face::parse(EMBEDDED_FONT, 0).expect("parse test font");
    let units_per_em = face.units_per_em();
    let widths = (32u8..=126)
        .map(|code| {
            let glyph = face
                .glyph_index(char::from(code))
                .expect("test font covers ASCII");
            let advance = face.glyph_hor_advance(glyph).unwrap_or(0);
            scale_to_1000(advance as i16, units_per_em).to_string()
        })
        .collect::<Vec<_>>()
        .join(" ");

    let bbox = face.global_bounding_box();
    let font_descriptor = format!(
        "<< /Type /FontDescriptor /FontName /LiberationSans /Flags 32 \
/FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 5 0 R >>",
        scale_to_1000(bbox.x_min, units_per_em),
        scale_to_1000(bbox.y_min, units_per_em),
        scale_to_1000(bbox.x_max, units_per_em),
        scale_to_1000(bbox.y_max, units_per_em),
        scale_to_1000(face.ascender(), units_per_em),
        scale_to_1000(face.descender(), units_per_em),
        scale_to_1000(face.capital_height().unwrap_or(face.ascender()), units_per_em),
    );
    let font = format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /LiberationSans /FirstChar 32 /LastChar 126 \
/Widths [{widths}] /FontDescriptor 4 0 R /Encoding /WinAnsiEncoding >>"
    );
    let content = b"BT /F1 18 Tf 72 720 Td (Embedded font sample.) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [6 0 R] /Count 1 >>".to_vec()),
        (3, font.into_bytes()),
        (4, font_descriptor.into_bytes()),
        (
            5,
            stream(&format!("/Length1 {}", EMBEDDED_FONT.len()), EMBEDDED_FONT),
        ),
        (
            6,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".to_vec(),
        ),
        (7, stream("", &content)),
    ];
    write_pdf("embedded-true-type", build_pdf(objects, 1))
}

/// A 612x792 page using a non-embedded, non-standard font name. Pdfium
/// substitutes a rendering font, but the text object still advertises the
/// document's original (non-standard) font.
pub fn non_standard_font_pdf() -> TempPdf {
    let content = b"BT /F1 18 Tf 72 720 Td (Fallback font sample.) Tj ET\n".to_vec();
    let objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [5 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /PDFXMissingFont /Encoding /WinAnsiEncoding /FontDescriptor 4 0 R >>".to_vec(),
        ),
        (
            4,
            b"<< /Type /FontDescriptor /FontName /PDFXMissingFont /Flags 32 /FontBBox [-200 -250 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>".to_vec(),
        ),
        (
            5,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 6 0 R >>".to_vec(),
        ),
        (6, stream("", &content)),
    ];
    write_pdf("non-standard-font", build_pdf(objects, 1))
}
