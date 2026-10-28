//! Command-boundary tests for the digital-signature invalidation warning.
//!
//! The editor must detect signature structures at open time through the same
//! `DocumentView` every open path (dialog, drop, recent file, Recovery File)
//! consumes, warn before any save writes a file, and never modify or expose
//! signature contents. Assertions use the public command functions and an
//! independent lopdf parse of the written Output Document.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use pdfx_lib::commands::{
    close_document, open_document, render_page, restore_recovery, CommandError, CommandErrorCode,
    DocumentView,
};
use pdfx_lib::recovery::maybe_snapshot_at;
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveKind, SaveResultView};
use pdfx_lib::signature::SignatureSummary;
use pdfx_lib::text_edit::{apply_text_edit, create_text_box, edit_history, CreateTextBoxRequest};
use support::synthetic::{
    cyclic_signature_fields_pdf, deeply_nested_signature_fields_pdf, inherited_parent_signature_pdf,
};

const SIGNER_CANARY: &str = "PDFX SIGNER CANARY";
const REASON_CANARY: &str = "PDFX REASON CANARY";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The process-wide recovery and logging stores can only be installed once per
/// test binary; the recovery-restore and log-leak tests share them.
static APP_DATA: OnceLock<PathBuf> = OnceLock::new();
static LOG_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

fn app_data() -> &'static Path {
    APP_DATA.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("pdfx-signature-appdata-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        pdfx_lib::recovery::initialize(&path).expect("initialize recovery storage");
        path
    })
}

fn log_directory() -> &'static Path {
    LOG_DIRECTORY.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("pdfx-signature-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        pdfx_lib::logging::initialize(&path).expect("initialize test logger");
        path
    })
}

/// Concatenates the rolling log and its archives, so a leak cannot hide in a
/// rotated file.
fn log_contents() -> String {
    let mut contents = String::new();
    for entry in fs::read_dir(log_directory()).expect("read log dir") {
        let path = entry.expect("log entry").path();
        if path.is_file() {
            contents.push_str(&fs::read_to_string(&path).expect("read log file"));
        }
    }
    contents
}

/// A unique scratch directory removed with the test.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfx-signature-{}-{unique}-{tag}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fixture_by_id(id: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture {id}"));
    support::fixtures::fixture_path(fixture)
}

fn copy_fixture(id: &str, dir: &TempDir, name: &str) -> PathBuf {
    let target = dir.path().join(name);
    fs::copy(fixture_by_id(id), &target).expect("copy fixture");
    target
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open(path: &Path) -> DocumentView {
    open_document(path_string(path), None).expect("open document")
}

fn open_with_password(path: &Path, password: &str) -> DocumentView {
    open_document(path_string(path), Some(password.to_owned())).expect("open protected document")
}

fn file_state(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    (
        fs::read(path).expect("read file"),
        fs::metadata(path)
            .expect("file metadata")
            .modified()
            .expect("mtime"),
    )
}

fn create_box(document_id: &str, text: &str) {
    let created = create_text_box(
        document_id.to_owned(),
        0,
        CreateTextBoxRequest {
            x: 72.0,
            y: 72.0,
            width: 360.0,
            height: 40.0,
            style: None,
        },
    )
    .expect("create a Text Box");
    apply_text_edit(
        document_id.to_owned(),
        0,
        created.target.box_id,
        text.to_owned(),
    )
    .expect("type into the Text Box");
}

/// A save request; `confirm_signature_invalidation` is the explicit acceptance
/// the warning dialog supplies before any write of a signature-bearing file.
fn save_request(
    save_as_path: Option<&Path>,
    confirm_signature_invalidation: bool,
) -> SaveDocumentRequest {
    SaveDocumentRequest {
        save_as_path: save_as_path.map(path_string),
        confirm_unencrypted_output: false,
        confirm_signature_invalidation,
        password: None,
    }
}

fn save(document_id: &str, request: SaveDocumentRequest) -> SaveResultView {
    save_document_with_progress(document_id, request, &mut |_| {}).expect("save should succeed")
}

fn save_error(document_id: &str, request: SaveDocumentRequest) -> CommandError {
    save_document_with_progress(document_id, request, &mut |_| {})
        .expect_err("save should be refused")
}

/// Independent structural assertion on the Output Document: raw lopdf objects
/// with `/Type /Sig` and dictionaries carrying `/FT /Sig`. This deliberately
/// does not reuse the editor's detection walk.
fn lopdf_signature_object_counts(document: &lopdf::Document) -> (usize, usize) {
    let mut signature_dictionaries = 0;
    let mut signature_fields = 0;
    for object in document.objects.values() {
        let Ok(dictionary) = object.as_dict() else {
            continue;
        };
        if dictionary
            .get(b"Type")
            .and_then(lopdf::Object::as_name)
            .is_ok_and(|name| name == b"Sig")
        {
            signature_dictionaries += 1;
        }
        if dictionary
            .get(b"FT")
            .and_then(lopdf::Object::as_name)
            .is_ok_and(|name| name == b"Sig")
        {
            signature_fields += 1;
        }
    }
    (signature_fields, signature_dictionaries)
}

/// The one `/Type /Sig` dictionary, for byte-level preservation assertions.
fn signature_dictionary(document: &lopdf::Document) -> &lopdf::Dictionary {
    document
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .find(|dictionary| {
            dictionary
                .get(b"Type")
                .and_then(lopdf::Object::as_name)
                .is_ok_and(|name| name == b"Sig")
        })
        .expect("one signature dictionary")
}

fn temp_save_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.to_string_lossy();
            name.contains(".pdfx-save-") || name.contains(".pdfx-backup-")
        })
        .collect()
}

#[test]
fn saving_a_signed_document_without_confirmation_writes_nothing() {
    let dir = TempDir::new("guard-refused");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, "Unsaved edit");
    assert!(
        edit_history(document.document_id.clone())
            .expect("history")
            .dirty,
        "the document starts with unsaved changes"
    );

    let error = save_error(&document.document_id, save_request(Some(&output), false));

    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired,
        "the save must be refused at the command boundary"
    );
    assert!(
        !error.message.contains(SIGNER_CANARY) && !error.message.contains(REASON_CANARY),
        "the refusal message must not leak signature contents: {}",
        error.message
    );
    assert!(!output.exists(), "a refused save writes no Output Document");
    assert!(
        temp_save_files(dir.path()).is_empty(),
        "a refused save leaves no temporary or backup file"
    );
    assert_eq!(
        file_state(&source),
        source_before,
        "the Source Document is untouched"
    );
    let history = edit_history(document.document_id.clone()).expect("history");
    assert!(
        history.dirty,
        "a refused save keeps the unsaved changes so it can be retried"
    );
    assert!(history.can_undo, "the unified history is preserved");

    // The refusal is not sticky: confirming the same edit writes the output.
    let result = save(&document.document_id, save_request(Some(&output), true));
    assert!(!result.history.dirty);
    assert!(output.is_file());

    close_document(document.document_id).expect("close document");
}

#[test]
fn confirming_the_warning_saves_and_keeps_the_signature_structures() {
    let dir = TempDir::new("guard-confirmed");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(
        &document.document_id,
        "An edit that invalidates the signature",
    );

    let result = save(&document.document_id, save_request(Some(&output), true));

    assert_eq!(result.kind, SaveKind::SaveAs);
    assert!(!result.history.dirty);
    let signature = result
        .signature
        .expect("the save result still reports signature structures");
    assert!(
        signature.signed_count >= 1,
        "the signer's signature object still exists: {signature:?}"
    );
    assert!(
        result.signature_invalidated,
        "the UI must switch the warning to its invalidated state"
    );

    let parsed = lopdf::Document::load(&output).expect("independent lopdf parse");
    assert_eq!(
        lopdf_signature_object_counts(&parsed),
        (2, 1),
        "two /FT /Sig fields and one /Type /Sig dictionary remain in the output"
    );
    // The signature dictionary must survive the write byte for byte: PDFX
    // warns that saving invalidates the signature, it never rewrites it.
    let source_parsed = lopdf::Document::load(&source).expect("independent parse of the source");
    assert_eq!(
        signature_dictionary(&parsed),
        signature_dictionary(&source_parsed),
        "the signature dictionary entries are unchanged"
    );
    let signature = signature_dictionary(&parsed);
    assert_eq!(
        signature
            .get(b"ByteRange")
            .and_then(lopdf::Object::as_array)
            .expect("/ByteRange")
            .iter()
            .filter_map(|value| value.as_i64().ok())
            .collect::<Vec<_>>(),
        vec![0, 1234, 5678, 9012],
        "/ByteRange is intact"
    );
    assert_eq!(
        signature
            .get(b"Contents")
            .and_then(lopdf::Object::as_str)
            .expect("/Contents"),
        (0u8..64).collect::<Vec<u8>>().as_slice(),
        "the signature contents bytes are intact"
    );
    let acro_form = parsed
        .catalog()
        .expect("catalog")
        .get_deref(b"AcroForm", &parsed)
        .and_then(lopdf::Object::as_dict)
        .expect("/AcroForm dictionary");
    assert_eq!(
        acro_form
            .get(b"SigFlags")
            .and_then(lopdf::Object::as_i64)
            .expect("/SigFlags"),
        1,
        "/SigFlags is preserved"
    );
    assert!(
        !acro_form.has(b"NeedAppearances"),
        "PDFX must not introduce /NeedAppearances"
    );
    assert!(
        !parsed.catalog().expect("catalog").has(b"Perms"),
        "PDFX must not introduce a /Perms signature permission"
    );

    let reopened = open(&output);
    let reopened_signature = reopened
        .signature
        .expect("PDFium reopens the output and sees the signature");
    assert!(reopened_signature.signed_count >= 1);
    let image = render_page(reopened.document_id.clone(), 0, 1.0)
        .expect("PDFium renders the signed output without crashing");
    assert!(image.width > 0 && image.height > 0);

    assert_eq!(
        file_state(&source),
        source_before,
        "the Source Document is never modified"
    );

    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

#[test]
fn a_password_protected_save_keeps_the_signature_structures() {
    let dir = TempDir::new("guard-password");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("protected.pdf");
    let password = "pdfx-signature-output-secret";

    let document = open(&source);
    create_box(&document.document_id, "Protected edit");

    let result = save(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
            confirm_signature_invalidation: true,
            password: Some(password.to_owned()),
        },
    );
    assert!(result.encrypted);
    assert!(result.signature_invalidated);

    let parsed = lopdf::Document::load_with_password(&output, password)
        .expect("the protected output parses with its password");
    assert_eq!(
        lopdf_signature_object_counts(&parsed),
        (2, 1),
        "protection does not remove the signature structures"
    );

    let reopened = open_with_password(&output, password);
    let reopened_signature = reopened
        .signature
        .expect("the protected output still reports a signature");
    assert!(
        reopened_signature.signed_count >= 1,
        "the protected output still reports an actual signature: {reopened_signature:?}"
    );
    assert!(
        reopened_signature.field_count >= 2,
        "both signature fields survive protection: {reopened_signature:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

#[test]
fn every_save_attempt_asks_again_and_the_logs_never_expose_signature_contents() {
    let _ = log_directory();
    let dir = TempDir::new("guard-repeat");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("output.pdf");

    let document = open(&source);
    create_box(&document.document_id, "First save");
    save(&document.document_id, save_request(Some(&output), true));
    let first_output = fs::read(&output).expect("read first output");

    // The confirmation is per-save, never remembered: a second save attempt
    // with new unsaved edits is refused again until it is confirmed.
    create_box(&document.document_id, "Second save");
    let error = save_error(&document.document_id, save_request(None, false));
    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired,
        "each new save attempt must ask again"
    );
    assert_eq!(
        fs::read(&output).expect("read output after refusal"),
        first_output,
        "a refused Save Changes leaves the previous Output Document intact"
    );
    assert!(
        edit_history(document.document_id.clone())
            .expect("history")
            .dirty,
        "the second edit is still unsaved and can be retried"
    );

    let result = save(&document.document_id, save_request(None, true));
    assert!(result.signature_invalidated);
    assert!(!result.history.dirty);

    // Neither the refusal nor the successful save may write signer names,
    // reasons or signature bytes to the local log.
    let logs = log_contents();
    assert!(!logs.contains(SIGNER_CANARY), "signer name leaked to logs");
    assert!(
        !logs.contains(REASON_CANARY),
        "signing reason leaked to logs"
    );
    assert!(
        !logs.contains("ByteRange"),
        "signature bytes leaked to logs"
    );
    assert!(
        !logs.contains("Contents"),
        "signature contents leaked to logs"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn recovery_restore_detects_signatures_and_requires_confirmation() {
    let _ = app_data();
    let dir = TempDir::new("guard-recovery");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("restored-output.pdf");

    let document = open(&source);
    create_box(&document.document_id, "UNSAVED SIGNED EDIT");
    // The first tick starts the throttle window; the second is due after the
    // debounce interval.
    assert!(
        !maybe_snapshot_at(&document.document_id, 0)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_at(&document.document_id, 2_000).expect("snapshot tick");
    assert!(written.written, "the unsaved edit is snapshotted");
    let session = written.session_id.expect("session id");

    // The original handle is still open; restoring models the startup prompt
    // reopening the snapshot bytes as a new Document.
    let restored = restore_recovery(session).expect("restore recovery file");
    assert!(restored.recovered);
    let signature = restored
        .signature
        .expect("a restored signed document still reports its signatures");
    assert!(signature.signed_count >= 1, "{signature:?}");

    let error = save_error(&restored.document_id, save_request(Some(&output), false));
    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired,
        "a recovered document is guarded through the same save boundary"
    );
    assert!(!output.exists(), "the refused save writes no bytes");

    let result = save(&restored.document_id, save_request(Some(&output), true));
    assert!(result.signature_invalidated);
    let parsed = lopdf::Document::load(&output).expect("independent lopdf parse");
    assert_eq!(lopdf_signature_object_counts(&parsed), (2, 1));

    close_document(restored.document_id).expect("close restored");
    close_document(document.document_id).expect("close original");
}

#[test]
fn read_only_documents_still_require_signature_confirmation_for_save_as() {
    let dir = TempDir::new("guard-read-only");
    let source = copy_fixture("encrypted-signature-restricted", &dir, "restricted.pdf");
    let output = dir.path().join("copy.pdf");

    // The empty user password opens the document with its restrictions: Save
    // Changes is forbidden, but Save As may write an unencrypted copy.
    let document = open(&source);
    assert!(
        document.read_only,
        "the fixture denies every editing capability"
    );
    assert!(document.encrypted);
    let signature = document
        .signature
        .expect("the restricted document still reports signatures");
    assert!(signature.signed_count >= 1, "{signature:?}");

    // The unencrypted-output decision (ADR-0007) is given, so the refusal this
    // test observes comes from the signature warning, not from encryption.
    let error = save_error(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: true,
            confirm_signature_invalidation: false,
            password: None,
        },
    );
    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired,
        "read-only documents are not exempt from the signature warning"
    );
    assert!(!output.exists());

    let result = save(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: true,
            confirm_signature_invalidation: true,
            password: None,
        },
    );
    assert!(!result.encrypted, "ADR-0007 writes the copy unencrypted");
    assert!(result.signature_invalidated);

    let parsed = lopdf::Document::load(&output).expect("the copy parses");
    assert_eq!(
        lopdf_signature_object_counts(&parsed),
        (2, 1),
        "Save As keeps the signature structures of the source"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn a_save_failure_after_confirmation_keeps_the_document_retryable() {
    let dir = TempDir::new("guard-write-failure");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, "A confirmed edit");
    // Occupy the target with a directory so the backup step of the atomic
    // write fails after the confirmation has been accepted.
    fs::create_dir(&output).expect("occupy the target path");

    let error = save_error(&document.document_id, save_request(Some(&output), true));
    assert_eq!(error.code, CommandErrorCode::SaveFailed);
    assert!(output.is_dir(), "the blocking directory is untouched");
    assert!(
        edit_history(document.document_id.clone())
            .expect("history")
            .dirty,
        "a failed save keeps the unsaved edits for a retry"
    );
    assert_eq!(
        file_state(&source),
        source_before,
        "the Source Document is untouched"
    );
    assert!(temp_save_files(dir.path()).is_empty());

    fs::remove_dir(&output).expect("remove the obstacle");
    // The retry still asks for the signature confirmation; nothing was
    // remembered from the failed attempt.
    let error = save_error(&document.document_id, save_request(Some(&output), false));
    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired
    );
    let result = save(&document.document_id, save_request(Some(&output), true));
    assert!(result.signature_invalidated);
    let parsed = lopdf::Document::load(&output).expect("the retried output parses");
    assert_eq!(lopdf_signature_object_counts(&parsed), (2, 1));

    close_document(document.document_id).expect("close document");
}

#[test]
fn signed_documents_report_signature_structures_without_leaking_details() {
    let dir = TempDir::new("open-signed");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let document = open(&source);

    let signature = document
        .signature
        .expect("the signed fixture reports signature structures");
    assert!(
        signature.signed_count >= 1,
        "a /V signature dictionary means an actual signature: {signature:?}"
    );
    assert_eq!(
        signature.field_count, 2,
        "the signed field and the nested empty field are counted, \
         the ordinary text field is not: {signature:?}"
    );

    // The view crosses the IPC boundary as JSON. Signer identity, reasons and
    // signature bytes are document content and must never be exposed.
    let json = serde_json::to_string(&document).expect("DocumentView serializes");
    assert!(!json.contains(SIGNER_CANARY), "signer name leaked: {json}");
    assert!(
        !json.contains(REASON_CANARY),
        "signature reason leaked: {json}"
    );
    assert!(
        !json.contains("ByteRange"),
        "signature bytes leaked: {json}"
    );
    assert!(
        !json.contains("Contents"),
        "signature contents leaked: {json}"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn empty_signature_fields_are_detected_through_inherited_ft() {
    let dir = TempDir::new("open-field-only");
    let source = copy_fixture("signature-field-only", &dir, "fields.pdf");
    let document = open(&source);

    let signature = document
        .signature
        .expect("empty signature fields still trigger the warning");
    assert_eq!(
        signature.signed_count, 0,
        "a field without /V is not a signed signature: {signature:?}"
    );
    assert_eq!(
        signature.field_count, 2,
        "merged and /Parent-inherited signature fields are both counted: {signature:?}"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn ordinary_documents_report_no_signature_structures() {
    let dir = TempDir::new("open-plain");

    let text = copy_fixture("text-latin", &dir, "text.pdf");
    let text_document = open(&text);
    assert_eq!(
        text_document.signature, None,
        "a plain text PDF has no signature structures"
    );
    close_document(text_document.document_id).expect("close text document");

    let form = copy_fixture("form-acroform", &dir, "form.pdf");
    let form_document = open(&form);
    assert_eq!(
        form_document.signature, None,
        "fillable form fields are not signature structures"
    );
    close_document(form_document.document_id).expect("close form document");
}

#[test]
fn signature_fields_inherited_from_an_out_of_tree_parent_are_detected() {
    let dir = TempDir::new("open-inherited-parent");
    let source = inherited_parent_signature_pdf();
    let output = dir.path().join("output.pdf");

    let document =
        open_document(source.path_string(), None).expect("open the inherited-parent fixture");
    assert_eq!(
        document.signature,
        Some(SignatureSummary {
            signed_count: 1,
            field_count: 1,
        }),
        "the /FT and /V a widget inherits through /Parent must be followed"
    );

    // The save gate covers this shape too: a widget whose owning field is not
    // listed in /Fields still represents an actual signature.
    let error = save_error(&document.document_id, save_request(Some(&output), false));
    assert_eq!(
        error.code,
        CommandErrorCode::SignatureInvalidationConfirmationRequired
    );
    assert!(!output.exists());
    let result = save(&document.document_id, save_request(Some(&output), true));
    assert!(result.signature_invalidated);

    let parsed = lopdf::Document::load(&output).expect("independent parse");
    assert_eq!(lopdf_signature_object_counts(&parsed), (1, 1));
    assert_eq!(
        signature_dictionary(&parsed)
            .get(b"ByteRange")
            .and_then(lopdf::Object::as_array)
            .expect("/ByteRange")
            .len(),
        4,
        "the inherited signature dictionary survives the save"
    );
    close_document(document.document_id).expect("close document");
}

#[test]
fn a_deeply_nested_field_tree_opens_without_exhausting_the_stack() {
    // Far deeper than any real form. The structural walk must stay bounded;
    // an unbounded /Kids recursion would abort the process while opening.
    let source = deeply_nested_signature_fields_pdf(20_000);
    let document = open_document(source.path_string(), None).expect("the deep tree opens");
    let signature = document
        .signature
        .expect("the root of the chain is a signature field");
    assert!(
        (1..=129).contains(&signature.field_count),
        "the depth guard bounds the walk: {signature:?}"
    );
    close_document(document.document_id).expect("close document");
}

#[test]
fn a_cyclic_field_tree_is_walked_once_without_hanging() {
    let source = cyclic_signature_fields_pdf();
    let document = open_document(source.path_string(), None).expect("the cyclic tree opens");
    assert_eq!(
        document.signature,
        Some(SignatureSummary {
            signed_count: 0,
            field_count: 2,
        }),
        "each field in the cycle is counted exactly once"
    );
    close_document(document.document_id).expect("close document");
}

/// Re-writes `source` as an AES-256 (V5, revision 6) encrypted document. Only
/// used to prove the opening password reaches the structural detection walk:
/// the empty-password fixture would parse even if the password were ignored.
#[allow(deprecated)]
fn write_aes256_signed_document(source: &Path, target: &Path, user_password: &str) {
    let mut document = lopdf::Document::load(source).expect("load fixture for encryption");
    let file_encryption_key = [0x37_u8; 32];
    let crypt_filters: std::collections::BTreeMap<
        Vec<u8>,
        std::sync::Arc<dyn lopdf::encryption::crypt_filters::CryptFilter>,
    > = std::collections::BTreeMap::from([(
        b"StdCF".to_vec(),
        std::sync::Arc::new(lopdf::encryption::crypt_filters::Aes256CryptFilter)
            as std::sync::Arc<dyn lopdf::encryption::crypt_filters::CryptFilter>,
    )]);
    let version = lopdf::EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters,
        file_encryption_key: &file_encryption_key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: "pdfx-test-owner",
        user_password,
        permissions: lopdf::Permissions::all(),
    };
    let state = lopdf::EncryptionState::try_from(version).expect("build the AES-256 state");
    document.encrypt(&state).expect("encrypt the fixture");
    document.save(target).expect("write the encrypted fixture");
}

#[test]
fn detection_uses_the_opening_password_of_a_signed_encrypted_document() {
    let dir = TempDir::new("open-password-signed");
    let source = copy_fixture("signature-signed", &dir, "signed.pdf");
    let protected = dir.path().join("protected.pdf");
    let password = "pdfx-signature-password";
    write_aes256_signed_document(&source, &protected, password);

    let missing = open_document(path_string(&protected), None)
        .expect_err("an encrypted document does not open without its password");
    assert_eq!(missing.code, CommandErrorCode::PasswordRequired);
    let wrong = open_document(path_string(&protected), Some("wrong".to_owned()))
        .expect_err("a wrong password must be refused");
    assert_eq!(wrong.code, CommandErrorCode::InvalidPassword);

    // lopdf can only read the encrypted field tree with this password, so this
    // assertion fails if detection stops forwarding the opening password.
    let document = open_with_password(&protected, password);
    assert!(document.encrypted);
    let signature = document
        .signature
        .expect("the password decrypts the AcroForm field tree");
    assert_eq!(signature.signed_count, 1, "{signature:?}");
    assert_eq!(signature.field_count, 2, "{signature:?}");
    close_document(document.document_id).expect("close document");
}
