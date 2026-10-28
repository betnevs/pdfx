//! Digital-signature structure detection at open time.
//!
//! PDFX only reports *whether* a document contains signature structures so the
//! UI can warn that editing or saving invalidates them. It deliberately never
//! reads signer identities, signing reasons, document text, certificates or
//! signature bytes, never verifies a signature, and never signs. The summary
//! crossing the command boundary therefore contains counts only.
//!
//! Detection is structural: lopdf walks `/AcroForm` `/Fields` and nested
//! `/Kids`, resolving inherited `/FT` through `/Parent`, and treats a
//! signature field with a `/V` dictionary as an actual signature. PDFium's
//! signature objects are an independent second signal; its widgets are the
//! fallback when the PDF structure cannot be parsed. A best-effort parse never
//! blocks opening a document.

use std::collections::HashSet;

use pdfium_render::prelude::{PdfDocument, PdfFormField};
use serde::{Deserialize, Serialize};

/// Counts of the signature structures in a document.
///
/// This is the only signature information that crosses the command boundary.
/// It intentionally carries no field names, signer details or bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureSummary {
    /// Signature dictionaries (`/V`) found. A non-zero count means editing or
    /// saving invalidates an actual signature.
    pub signed_count: u32,
    /// `/FT /Sig` fields, including empty fields that were never signed.
    pub field_count: u32,
}

impl SignatureSummary {
    /// True when the document contains any signature structure.
    pub fn is_present(&self) -> bool {
        self.signed_count > 0 || self.field_count > 0
    }
}

/// Detects signature structures, returning `None` for ordinary documents.
///
/// `source_bytes` are the bytes the live PDFium document was opened from, so
/// the structural walk sees exactly the file the user opened. `password` is the
/// password that opened an encrypted document; lopdf needs it to read the
/// decrypted field tree, and it is only used here, never stored or logged.
pub fn detect(
    document: &PdfDocument<'_>,
    source_bytes: &[u8],
    password: Option<&str>,
) -> Option<SignatureSummary> {
    // Pdfium's signature collection counts signature *fields*, including empty
    // ones, so it is a presence signal only. Whether a field carries an actual
    // signature comes from its `/V` dictionary in the structural walk.
    let pdfium_signature_fields = document.signatures().len() as u32;
    let mut summary = match structural_summary(source_bytes, password) {
        Some(summary) => summary,
        // A structurally unreadable file still opens in PDFium; count the
        // signature widgets it can see so the warning is not lost.
        None => SignatureSummary {
            signed_count: 0,
            field_count: pdfium_signature_widgets(document),
        },
    };
    summary.field_count = summary
        .field_count
        .max(pdfium_signature_fields)
        .max(summary.signed_count);
    summary.is_present().then_some(summary)
}

/// Walks the AcroForm field tree. `None` means the bytes could not be parsed
/// as a PDF at all, which lets the caller fall back to PDFium.
fn structural_summary(source_bytes: &[u8], password: Option<&str>) -> Option<SignatureSummary> {
    let options = match password {
        Some(password) => lopdf::LoadOptions::with_password(password),
        None => lopdf::LoadOptions::default(),
    };
    let document = lopdf::Document::load_mem_with_options(source_bytes, options).ok()?;
    let mut summary = SignatureSummary::default();
    let Ok(fields) = document
        .catalog()
        .and_then(|catalog| catalog.get_deref(b"AcroForm", &document))
        .and_then(lopdf::Object::as_dict)
        .and_then(|acro_form| acro_form.get_deref(b"Fields", &document))
        .and_then(lopdf::Object::as_array)
    else {
        return Some(summary);
    };

    let mut seen = HashSet::new();
    for field in fields {
        tally_field(&document, field, None, true, &mut seen, &mut summary);
    }
    Some(summary)
}

fn dictionary_of<'a>(
    object: &'a lopdf::Object,
    document: &'a lopdf::Document,
) -> Option<&'a lopdf::Dictionary> {
    match object {
        lopdf::Object::Reference(id) => document.get_dictionary(*id).ok(),
        lopdf::Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

/// The nearest `/FT` on the `/Parent` chain, for a widget referenced directly
/// from `/Fields` whose owning field dictionary is not its container.
fn parent_field_type(item: &lopdf::Dictionary, document: &lopdf::Document) -> Option<Vec<u8>> {
    let mut current = item;
    for _ in 0..32 {
        let parent = current
            .get_deref(b"Parent", document)
            .ok()?
            .as_dict()
            .ok()?;
        if let Ok(field_type) = parent.get(b"FT").and_then(lopdf::Object::as_name) {
            return Some(field_type.to_vec());
        }
        current = parent;
    }
    None
}

fn has_signature_value(field: &lopdf::Dictionary, document: &lopdf::Document) -> bool {
    field
        .get_deref(b"V", document)
        .ok()
        .and_then(|value| value.as_dict().ok())
        .is_some()
}

fn tally_field(
    document: &lopdf::Document,
    object: &lopdf::Object,
    inherited_field_type: Option<&[u8]>,
    is_root: bool,
    seen: &mut HashSet<lopdf::ObjectId>,
    summary: &mut SignatureSummary,
) {
    let Some(field) = dictionary_of(object, document) else {
        return;
    };
    // Cyclic /Kids references must not recurse forever.
    if let lopdf::Object::Reference(id) = object {
        if !seen.insert(*id) {
            return;
        }
    }

    let own_field_type = field
        .get(b"FT")
        .and_then(lopdf::Object::as_name)
        .ok()
        .map(<[u8]>::to_vec);
    let parent_field_type = if own_field_type.is_none() && !is_root {
        parent_field_type(field, document)
    } else {
        None
    };
    let effective_field_type = own_field_type
        .as_deref()
        .or(inherited_field_type)
        .or(parent_field_type.as_deref());

    if effective_field_type == Some(b"Sig") {
        let is_widget =
            field.get(b"Subtype").and_then(lopdf::Object::as_name).ok() == Some(b"Widget");
        let has_name = field.get(b"T").is_ok();
        // A widget kid without /T inherits its owner's /FT; it belongs to the
        // field counted at the parent and is not a second field.
        let belongs_to_parent_field =
            is_widget && !has_name && !is_root && inherited_field_type == Some(b"Sig");
        if !belongs_to_parent_field {
            summary.field_count += 1;
        }
        if has_signature_value(field, document) {
            summary.signed_count += 1;
        }
    }

    if let Ok(kids) = field
        .get_deref(b"Kids", document)
        .and_then(lopdf::Object::as_array)
    {
        for kid in kids {
            tally_field(document, kid, effective_field_type, false, seen, summary);
        }
    }
}

/// Counts signature widgets with PDFium, used only when the structure cannot
/// be parsed. This covers inherited `/FT` because PDFium resolves it.
fn pdfium_signature_widgets(document: &PdfDocument<'_>) -> u32 {
    let pages = document.pages();
    let mut count = 0;
    for page_index in 0..pages.len() {
        let Ok(page) = pages.get(page_index) else {
            continue;
        };
        let annotations = page.annotations();
        for annotation_index in 0..annotations.len() {
            let Ok(annotation) = annotations.get(annotation_index) else {
                continue;
            };
            if matches!(annotation.as_form_field(), Some(PdfFormField::Signature(_))) {
                count += 1;
            }
        }
    }
    count
}
