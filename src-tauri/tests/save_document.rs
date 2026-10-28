//! Command-boundary tests for the Save As / Save Changes pipeline.
//!
//! Every assertion goes through the same Rust functions the Tauri IPC layer
//! exposes. Saved output is reopened through the command boundary (or with an
//! independent lopdf parse) so "the edit is really in the file" is checked
//! against a fresh parse, not the in-memory document.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use pdfx_lib::commands::{
    close_document, extract_text, open_document, CommandErrorCode, DocumentPermissions,
    DocumentView,
};
use pdfx_lib::save::{
    save_document_with_progress, SaveDocumentRequest, SaveKind, SaveProgress, SaveStage,
};
use pdfx_lib::text_edit::{create_text_box, CreateTextBoxRequest};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory removed with the test.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("pdfx-save-{}-{unique}-{tag}", std::process::id()));
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

fn fixture_path(category: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, category).expect("fixture category");
    support::fixtures::fixture_path(fixture)
}

/// Copies a fixture into `dir` under `name` so mtime assertions never touch the
/// repository corpus.
fn copy_fixture(category: &str, dir: &TempDir, name: &str) -> PathBuf {
    let target = dir.path().join(name);
    fs::copy(fixture_path(category), &target).expect("copy fixture");
    target
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn file_state(path: &Path) -> (Vec<u8>, SystemTime) {
    (
        fs::read(path).expect("read file"),
        fs::metadata(path)
            .expect("file metadata")
            .modified()
            .expect("mtime"),
    )
}

fn save_request(save_as_path: Option<&Path>) -> SaveDocumentRequest {
    save_request_confirmed(save_as_path, false)
}

fn save_request_confirmed(
    save_as_path: Option<&Path>,
    confirm_unencrypted_output: bool,
) -> SaveDocumentRequest {
    SaveDocumentRequest {
        save_as_path: save_as_path.map(path_string),
        confirm_unencrypted_output,
        confirm_signature_invalidation: false,
        password: None,
    }
}

/// A save that asks for a User Password on the Output Document.
fn save_request_with_password(save_as_path: Option<&Path>, password: &str) -> SaveDocumentRequest {
    SaveDocumentRequest {
        save_as_path: save_as_path.map(path_string),
        confirm_unencrypted_output: false,
        confirm_signature_invalidation: false,
        password: Some(password.to_owned()),
    }
}

fn save(
    document_id: &str,
    request: SaveDocumentRequest,
) -> (pdfx_lib::save::SaveResultView, Vec<SaveProgress>) {
    let mut progress = Vec::new();
    let result =
        save_document_with_progress(document_id, request, &mut |update| progress.push(update))
            .expect("save should succeed");
    (result, progress)
}

fn open(path: &Path) -> DocumentView {
    open_document(path_string(path), None).expect("open document")
}

fn saved_temp_files(dir: &Path) -> Vec<PathBuf> {
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

/// Every regular file below `directory`, recursively.
fn files_under(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The process-wide logger can only be installed once. Every logging
/// assertion in this binary shares the same directory.
static LOG_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

fn log_directory() -> &'static Path {
    LOG_DIRECTORY.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("pdfx-save-log-{}", std::process::id()));
        fs::create_dir_all(&path).expect("create log dir");
        pdfx_lib::logging::initialize(&path).expect("initialize test logger");
        path
    })
}

#[test]
fn a_successful_save_is_logged_without_document_content() {
    let dir = TempDir::new("save-log");
    let logs = log_directory();
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("logged-output.pdf");
    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "SECRET SAVE CONTENT");

    let (result, _) = save(&document.document_id, save_request(Some(&output)));

    let log = fs::read_to_string(logs.join(pdfx_lib::logging::LOG_FILE_NAME))
        .expect("the save pipeline writes to the installed logger");
    let line = log
        .lines()
        .find(|line| line.contains("document.save") && line.contains(&path_string(&output)))
        .expect("a document.save event names the output path");
    assert!(line.contains("outcome=ok"), "logged outcome: {line}");
    assert!(line.contains("kind=save_as"), "logged kind: {line}");
    assert!(
        line.contains(&format!("documentId={}", document.document_id)),
        "logged document id: {line}"
    );
    assert!(
        line.contains(&format!("bytes={}", result.bytes_written)),
        "logged byte count: {line}"
    );
    assert!(
        !log.contains("SECRET SAVE CONTENT"),
        "document content must never reach the log"
    );

    close_document(document.document_id).expect("close source");
    // The log directory is shared by every logging assertion in this binary
    // (the process-wide logger can only be installed once), so it must outlive
    // this test. Deleting it would silently break concurrent tests.
}

#[test]
fn save_as_writes_a_new_output_and_leaves_the_source_untouched() {
    let dir = TempDir::new("save-as");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let source_before = file_state(&source);
    let document = open(&source);

    let (result, _) = save(&document.document_id, save_request(Some(&output)));

    assert_eq!(result.document_id, document.document_id);
    assert_eq!(result.kind, SaveKind::SaveAs);
    assert_eq!(result.output_path, path_string(&output));
    assert_eq!(result.backup_path, None, "a first save replaces nothing");
    assert!(
        !result.history.dirty,
        "a successful save is a clean baseline"
    );

    assert!(output.exists(), "Save As must write the target path");
    assert_eq!(
        result.bytes_written,
        fs::metadata(&output).expect("output metadata").len()
    );

    let (source_after_bytes, source_after_modified) = file_state(&source);
    assert_eq!(
        source_after_bytes, source_before.0,
        "source bytes must not change"
    );
    assert_eq!(
        source_after_modified, source_before.1,
        "source mtime must not change"
    );

    let reopened = open(&output);
    assert_eq!(reopened.page_count, 1);
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");

    assert!(
        saved_temp_files(dir.path()).is_empty(),
        "the temp file must be renamed or cleaned up"
    );
}

const OUTPUT_PASSWORD: &str = "pdfx-output-secret-40";

#[test]
fn saving_with_a_password_encrypts_the_output_and_requires_it_to_open() {
    let dir = TempDir::new("output-password");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Protected content");

    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );
    assert!(result.encrypted, "the save result reports the protection");
    assert_eq!(
        result.bytes_written,
        fs::metadata(&output).expect("output metadata").len()
    );

    let bytes = fs::read(&output).expect("read output");
    assert!(
        !bytes
            .windows(OUTPUT_PASSWORD.len())
            .any(|window| window == OUTPUT_PASSWORD.as_bytes()),
        "the User Password must not be stored in the file"
    );
    // Encryption touches every string and stream, so an incremental append
    // cannot be used: the output is one complete revision.
    assert_eq!(
        startxref_count(&bytes),
        1,
        "encryption rewrites the object graph as one complete revision"
    );

    // Independent structure check: lopdf reads the security handler without
    // the password and authenticates the two passwords differently. V5/R6 is
    // AES-256 (PDF 2.0).
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert!(parsed.is_encrypted(), "the output must carry /Encrypt");
    let encrypt = parsed
        .trailer
        .get(b"Encrypt")
        .expect("trailer /Encrypt")
        .as_reference()
        .expect("/Encrypt is a reference");
    let dictionary = parsed
        .get_object(encrypt)
        .expect("encryption dictionary")
        .as_dict()
        .expect("encryption dictionary is a dictionary");
    assert_eq!(dictionary.get(b"V").expect("/V").as_i64().expect("V"), 5);
    assert_eq!(dictionary.get(b"R").expect("/R").as_i64().expect("R"), 6);
    assert_eq!(
        dictionary
            .get(b"Filter")
            .expect("/Filter")
            .as_name()
            .expect("Filter"),
        b"Standard"
    );

    assert!(
        lopdf::Document::load_with_password(&output, "not-the-output-password").is_err(),
        "a wrong password must not decrypt the output"
    );
    let decrypted = lopdf::Document::load_with_password(&output, OUTPUT_PASSWORD)
        .expect("the User Password decrypts the output");
    let text = decrypted
        .extract_text(&[1])
        .expect("extract the decrypted text");
    assert!(
        text.contains("Protected content"),
        "decrypted text {text:?}"
    );

    // PDFium through the command boundary: a missing or wrong password is
    // refused, the right one opens the document.
    let missing = open_document(path_string(&output), None).expect_err("no password supplied");
    assert_eq!(
        missing.code,
        pdfx_lib::commands::CommandErrorCode::PasswordRequired
    );
    let refused = open_document(
        path_string(&output),
        Some("not-the-output-password".to_owned()),
    )
    .expect_err("wrong password supplied");
    assert_eq!(
        refused.code,
        pdfx_lib::commands::CommandErrorCode::InvalidPassword
    );
    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("open with the output password");
    assert!(reopened.encrypted);
    let reopened_text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract the saved text")
        .text;
    assert!(
        reopened_text.contains("Protected content"),
        "got {reopened_text:?}"
    );
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before, "source stays untouched");
    close_document(document.document_id).expect("close source");
}

/// Writes `source` as an AES-256 (V5) encrypted document with the given
/// security-handler revision. Revision 6 is what PDFX's save pipeline writes;
/// revision 5 is the deprecated Adobe extension the vendored PDFium binding
/// must also recognise.
#[allow(deprecated)]
fn write_aes256_document(
    source: &Path,
    target: &Path,
    revision: u8,
    user_password: &str,
    permissions: lopdf::Permissions,
) {
    let mut document = lopdf::Document::load(source).expect("load fixture for encryption");
    let file_encryption_key = [0x5a_u8; 32];
    let crypt_filters: std::collections::BTreeMap<
        Vec<u8>,
        std::sync::Arc<dyn lopdf::encryption::crypt_filters::CryptFilter>,
    > = std::collections::BTreeMap::from([(
        b"StdCF".to_vec(),
        std::sync::Arc::new(lopdf::encryption::crypt_filters::Aes256CryptFilter)
            as std::sync::Arc<dyn lopdf::encryption::crypt_filters::CryptFilter>,
    )]);
    let version = match revision {
        6 => lopdf::EncryptionVersion::V5 {
            encrypt_metadata: true,
            crypt_filters,
            file_encryption_key: &file_encryption_key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "test-owner-password",
            user_password,
            permissions,
        },
        5 => lopdf::EncryptionVersion::R5 {
            encrypt_metadata: true,
            crypt_filters,
            file_encryption_key: &file_encryption_key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: "test-owner-password",
            user_password,
            permissions,
        },
        _ => panic!("only revisions 5 and 6 use AES-256"),
    };
    let state = lopdf::EncryptionState::try_from(version).expect("build the AES-256 state");
    document.encrypt(&state).expect("encrypt the fixture");
    document.save(target).expect("write the encrypted fixture");
}

/// A password-only Output Document grants every permission in `/P`, so
/// reopening it must be editable and Save Changes must work with the password.
/// The vendored PDFium binding must name security-handler revisions 5 and 6;
/// treating an unknown revision as "no permissions" would silently read-only
/// every protected output PDFX writes.
#[test]
fn a_password_only_output_opens_editable_and_saves_changes_with_the_password() {
    let dir = TempDir::new("protected-editable");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First protected version");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );

    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("open the protected output");
    assert!(reopened.encrypted);
    assert!(
        !reopened.read_only,
        "a password-only output must keep its editing permissions"
    );
    assert_eq!(
        reopened.permissions,
        DocumentPermissions {
            can_modify_content: true,
            can_annotate: true,
            can_assemble: true,
            can_fill_forms: true,
            can_print: true,
            can_copy_content: true,
        }
    );

    // A reopened file starts with no Output Document: Save As establishes one
    // with the password, then Save Changes must keep working through the same
    // permission gate.
    let second_output = dir.path().join("protected-again.pdf");
    let (first, _) = save(
        &reopened.document_id,
        save_request_with_password(Some(&second_output), OUTPUT_PASSWORD),
    );
    assert_eq!(first.kind, SaveKind::SaveAs);
    assert!(first.encrypted);

    create_box(
        &reopened.document_id,
        72.0,
        200.0,
        "Second protected version",
    );
    let (result, _) = save(
        &reopened.document_id,
        save_request_with_password(None, OUTPUT_PASSWORD),
    );
    assert_eq!(result.kind, SaveKind::SaveChanges);
    assert!(result.encrypted);

    let reopened_again = open_document(
        path_string(&second_output),
        Some(OUTPUT_PASSWORD.to_owned()),
    )
    .expect("open the re-saved protected output");
    let text = extract_text(reopened_again.document_id.clone(), 0)
        .expect("extract the re-saved text")
        .text;
    assert!(
        text.contains("Second protected version"),
        "the edit saved through the permission gate must survive, got {text:?}"
    );
    close_document(reopened_again.document_id).expect("close re-saved output");
    close_document(reopened.document_id).expect("close reopened output");
    assert_eq!(file_state(&source), source_before, "source stays untouched");
    close_document(document.document_id).expect("close source");
}

/// The revision 5/6 mapping must not weaken restricted documents: a `/P` that
/// denies editing still reports read-only and refuses edit commands. The print
/// bit proves the flags come from `/P`, not from a blanket "unknown revision"
/// answer.
#[test]
fn an_aes256_document_with_denied_permissions_stays_read_only() {
    for revision in [6_u8, 5] {
        let dir = TempDir::new("aes256-restricted");
        let target = dir.path().join(format!("restricted-r{revision}.pdf"));
        write_aes256_document(
            &fixture_path("text"),
            &target,
            revision,
            "restricted-user",
            lopdf::Permissions::PRINTABLE,
        );

        let missing =
            open_document(path_string(&target), None).expect_err("the document is protected");
        assert_eq!(missing.code, CommandErrorCode::PasswordRequired);

        let view = open_document(path_string(&target), Some("restricted-user".to_owned()))
            .expect("open the restricted document");
        assert!(view.encrypted, "revision {revision} is encrypted");
        assert!(
            view.read_only,
            "revision {revision} restrictions must still report read-only"
        );
        assert!(
            view.permissions.can_print,
            "revision {revision} must keep the granted print permission"
        );
        assert!(!view.permissions.can_modify_content);

        let error = create_text_box(
            view.document_id.clone(),
            0,
            CreateTextBoxRequest {
                x: 72.0,
                y: 72.0,
                width: 320.0,
                height: 40.0,
                style: None,
            },
        )
        .expect_err("editing a restricted document is denied");
        assert_eq!(error.code, CommandErrorCode::PermissionDenied);

        // The owner password still lifts the restrictions, so the mapping
        // reads the authenticated permission set rather than a hard-coded one.
        let owner = open_document(path_string(&target), Some("test-owner-password".to_owned()))
            .expect("open the restricted document with its owner password");
        assert!(
            !owner.read_only,
            "the owner password lifts revision {revision}"
        );
        assert_eq!(
            owner.permissions,
            DocumentPermissions {
                can_modify_content: true,
                can_annotate: true,
                can_assemble: true,
                can_fill_forms: true,
                can_print: true,
                can_copy_content: true,
            }
        );
        close_document(owner.document_id).expect("close owner view");

        close_document(view.document_id).expect("close restricted document");
    }
}

/// ISO 32000 requires `/ID` in an encrypted file. The encrypted rewrite must
/// set a fresh identifier from the OS generator rather than inheriting the
/// unencrypted working bytes' identifier, and each write must differ.
#[test]
fn encrypted_outputs_carry_a_fresh_os_random_file_identifier() {
    fn read_file_id(path: &Path) -> Vec<Vec<u8>> {
        let parsed = lopdf::Document::load(path).expect("lopdf parses the output");
        let id = parsed.trailer.get(b"ID").expect("trailer /ID");
        id.as_array()
            .expect("/ID is an array")
            .iter()
            .map(|part| part.as_str().expect("/ID element is a string").to_vec())
            .collect()
    }

    let dir = TempDir::new("encrypted-file-id");
    let source = copy_fixture("text", &dir, "source.pdf");
    let first_output = dir.path().join("first.pdf");
    let second_output = dir.path().join("second.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Identifier check");
    save(
        &document.document_id,
        save_request_with_password(Some(&first_output), OUTPUT_PASSWORD),
    );
    save(
        &document.document_id,
        save_request_with_password(Some(&second_output), OUTPUT_PASSWORD),
    );

    let first = read_file_id(&first_output);
    let second = read_file_id(&second_output);
    assert_eq!(first.len(), 2, "the trailer /ID has two elements");
    assert_eq!(second.len(), 2);
    for part in first.iter().chain(second.iter()) {
        assert_eq!(part.len(), 16, "each /ID element is 16 bytes");
        assert_ne!(part, &[0_u8; 16], "the identifier is not a zero filler");
    }
    assert_ne!(
        first, second,
        "each encrypted write gets a fresh file identifier"
    );

    close_document(document.document_id).expect("close source");
}

/// The User Password lives only in the save request. Logs, diagnostic
/// exports and Recovery Files are all local artifacts the user may share, so
/// none of them may contain it.
#[test]
fn the_output_password_never_reaches_logs_diagnostics_or_recovery_files() {
    let logs = log_directory();
    let dir = TempDir::new("password-secrecy");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Secret work");

    // A Recovery File is written while the edit is unsaved. It is produced by
    // the same serializer the save pipeline uses, before a password exists,
    // and must stay password-free together with its JSON sidecar.
    let app_data = dir.path().join("appdata");
    let store = pdfx_lib::recovery::RecoveryStore::new(&app_data);
    pdfx_lib::recovery::maybe_snapshot_in(&store, &document.document_id, 0)
        .expect("snapshot tick starts the quiet period");
    let snapshot = pdfx_lib::recovery::maybe_snapshot_in(&store, &document.document_id, 2_000)
        .expect("snapshot tick");
    assert!(
        snapshot.written,
        "the dirty document writes a Recovery File"
    );
    assert!(
        !files_under(store.directory()).is_empty(),
        "the Recovery File and its sidecar exist"
    );

    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );
    assert!(result.encrypted);

    let diagnostics = dir.path().join("diagnostics.txt");
    pdfx_lib::commands::export_diagnostics(path_string(&diagnostics))
        .expect("diagnostics export succeeds");

    let log_files = files_under(logs);
    assert!(
        log_files
            .iter()
            .any(|file| file.file_name().is_some_and(|name| name == "pdfx.log")),
        "the save pipeline wrote to the installed logger"
    );
    assert!(diagnostics.exists(), "the diagnostic report exists");

    for file in files_under(dir.path()).into_iter().chain(log_files) {
        let bytes = fs::read(&file).expect("read persisted artifact");
        assert!(
            !contains_bytes(&bytes, OUTPUT_PASSWORD.as_bytes()),
            "the User Password leaked into {}",
            file.display()
        );
    }

    // The protected output itself never stores the password in plaintext.
    assert!(!contains_bytes(
        &fs::read(&output).expect("read output"),
        OUTPUT_PASSWORD.as_bytes()
    ));

    close_document(document.document_id).expect("close source");
}

/// A blank password is not "no password": the UI requires a value to set one,
/// and treating an empty string as protection would produce a file whose
/// "password" nobody chose. The save is retryable with a real password.
#[test]
fn an_empty_output_password_is_rejected_and_the_save_can_be_retried() {
    let dir = TempDir::new("empty-output-password");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Retry with a password");

    let error = save_error(
        &document.document_id,
        save_request_with_password(Some(&output), ""),
    );
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::InvalidArgument
    );
    assert!(
        !output.exists(),
        "nothing may be written for an empty password"
    );
    assert_eq!(file_state(&source), source_before);
    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after rejected password");
    assert!(history.dirty, "the rejected save keeps the document dirty");

    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );
    assert!(result.encrypted);
    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("the retried save protects the output");
    assert!(reopened.encrypted);
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

/// R6 passwords are SASLprep-normalised UTF-8, so a non-ASCII password must
/// round-trip through both the independent lopdf parse and PDFium.
#[test]
fn unicode_output_passwords_open_with_pdfium_and_lopdf() {
    const UNICODE_PASSWORD: &str = "pässwörd-中文密码-Ω";
    let dir = TempDir::new("unicode-output-password");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Unicode protected");
    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), UNICODE_PASSWORD),
    );
    assert!(result.encrypted);

    assert!(
        lopdf::Document::load_with_password(&output, "wrong-password").is_err(),
        "a wrong password must not decrypt a Unicode-password output"
    );
    let decrypted = lopdf::Document::load_with_password(&output, UNICODE_PASSWORD)
        .expect("the Unicode User Password opens the output in lopdf");
    assert!(decrypted
        .extract_text(&[1])
        .expect("extract text")
        .contains("Unicode protected"));

    let missing = open_document(path_string(&output), None).expect_err("the output is protected");
    assert_eq!(missing.code, CommandErrorCode::PasswordRequired);
    let refused = open_document(path_string(&output), Some("wrong-password".to_owned()))
        .expect_err("a wrong password is refused");
    assert_eq!(refused.code, CommandErrorCode::InvalidPassword);
    let reopened = open_document(path_string(&output), Some(UNICODE_PASSWORD.to_owned()))
        .expect("PDFium opens the output with its Unicode password");
    assert!(reopened.encrypted);
    assert!(extract_text(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text
        .contains("Unicode protected"));
    close_document(reopened.document_id).expect("close reopened");
    assert_eq!(file_state(&source), source_before, "source stays untouched");
    close_document(document.document_id).expect("close source");
}

/// ISO 32000-2 truncates the password to 127 bytes. A 127-byte password is
/// supported; a longer one is rejected instead of silently losing its tail.
#[test]
fn an_overlong_output_password_is_rejected_and_writes_nothing() {
    let dir = TempDir::new("overlong-output-password");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Boundary password");

    let overlong = "a".repeat(128);
    let error = save_error(
        &document.document_id,
        save_request_with_password(Some(&output), &overlong),
    );
    assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    assert!(
        !error.message.contains(&overlong),
        "the error must not echo the password"
    );
    assert!(
        !output.exists(),
        "nothing may be written for an overlong password"
    );
    assert_eq!(file_state(&source), source_before);

    // The documented maximum still works end to end.
    let maximum = "a".repeat(127);
    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), &maximum),
    );
    assert!(result.encrypted);
    let reopened = open_document(path_string(&output), Some(maximum)).expect("open at the limit");
    assert!(reopened.encrypted);
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

/// The default save stays exactly as ticket 10 defined it: no password means
/// no encryption, and an unencrypted source needs no confirmation.
#[test]
fn saving_without_a_password_still_writes_an_unencrypted_output() {
    let dir = TempDir::new("default-unencrypted");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("plain.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Plain save");
    let (result, _) = save(&document.document_id, save_request(Some(&output)));

    assert!(!result.encrypted);
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert!(!parsed.is_encrypted());
    open_document(path_string(&output), None).expect("the output opens without a password");
    close_document(document.document_id).expect("close source");
}

/// Once the Output Document is protected, Save Changes must never write an
/// unprotected replacement without an explicit decision (ADR-0007's principle
/// applied to the output side; the User Password itself is never stored).
#[test]
fn save_changes_refuses_to_silently_remove_output_encryption() {
    let dir = TempDir::new("protected-save-changes");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First protected version");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );
    let protected_before = file_state(&output);

    create_box(&document.document_id, 72.0, 200.0, "Second edition");
    let error = save_error(&document.document_id, save_request(None));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::EncryptedOutputConfirmationRequired,
        "Save Changes cannot silently drop the Output Document's protection"
    );
    assert_eq!(
        file_state(&output),
        protected_before,
        "a refused save leaves the protected output untouched"
    );
    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after refused Save Changes");
    assert!(
        history.dirty,
        "refusing to save must keep the document dirty"
    );

    // The explicit unencrypted-output decision resumes the save. The backup
    // keeps the previous protected version, still requiring its password.
    let (result, _) = save(&document.document_id, save_request_confirmed(None, true));
    assert_eq!(result.kind, SaveKind::SaveChanges);
    assert!(!result.encrypted);
    let backup = result
        .backup_path
        .expect("Save Changes backs up the output");
    assert_eq!(
        fs::read(&backup).expect("read backup"),
        protected_before.0,
        "the backup is the protected Output Document byte for byte"
    );

    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert!(
        !parsed.is_encrypted(),
        "the confirmed save writes an unencrypted output"
    );
    let reopened = open_document(path_string(&output), None).expect("open unencrypted output");
    assert!(!reopened.encrypted);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text;
    assert!(text.contains("Second edition"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    let backup_view = open_document(backup.clone(), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("the backup still opens with the old password");
    assert!(backup_view.encrypted);
    close_document(backup_view.document_id).expect("close backup");

    assert_eq!(file_state(&source), source_before, "source stays untouched");
    close_document(document.document_id).expect("close source");
}

/// Save Changes with a password is an explicit decision: the output stays
/// protected and a new password replaces the old one. The backup of the old
/// output keeps the old password, so both versions remain readable by their
/// owners during the transition.
#[test]
fn save_changes_can_keep_or_replace_the_output_password() {
    let dir = TempDir::new("change-output-password");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First protected version");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );

    // Keeping the same password needs no confirmation.
    create_box(
        &document.document_id,
        72.0,
        200.0,
        "Second protected version",
    );
    let (kept, _) = save(
        &document.document_id,
        save_request_with_password(None, OUTPUT_PASSWORD),
    );
    assert_eq!(kept.kind, SaveKind::SaveChanges);
    assert!(kept.encrypted);
    let kept_view = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("the output still opens with its password");
    assert!(kept_view.encrypted);
    let kept_text = extract_text(kept_view.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(
        kept_text.contains("Second protected version"),
        "got {kept_text:?}"
    );
    close_document(kept_view.document_id).expect("close kept view");
    let previous_protected = file_state(&output);

    // A new password replaces the old one; the old password no longer opens
    // the output, and the backup keeps the old password's version.
    create_box(
        &document.document_id,
        72.0,
        320.0,
        "Third protected version",
    );
    let new_password = "pdfx-output-secret-40-replaced";
    let (replaced, _) = save(
        &document.document_id,
        save_request_with_password(None, new_password),
    );
    assert!(replaced.encrypted);
    assert_eq!(
        replaced.backup_path,
        Some(path_string(&pdfx_lib::save::backup_path_for(&output)))
    );

    let error = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect_err("the replaced password must no longer open the output");
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::InvalidPassword
    );
    let replaced_view = open_document(path_string(&output), Some(new_password.to_owned()))
        .expect("the new password opens the output");
    let replaced_text = extract_text(replaced_view.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(
        replaced_text.contains("Third protected version"),
        "got {replaced_text:?}"
    );
    close_document(replaced_view.document_id).expect("close replaced view");

    let backup = pdfx_lib::save::backup_path_for(&output);
    assert_eq!(
        fs::read(&backup).expect("read backup"),
        previous_protected.0,
        "the backup is the previous protected output"
    );
    let backup_view = open_document(path_string(&backup), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("the backup still opens with the previous password");
    assert!(backup_view.encrypted);
    close_document(backup_view.document_id).expect("close backup view");

    close_document(document.document_id).expect("close source");
}

fn create_box(document_id: &str, x: f32, y: f32, text: &str) {
    let created = create_text_box(
        document_id.to_owned(),
        0,
        CreateTextBoxRequest {
            x,
            y,
            width: 320.0,
            height: 40.0,
            style: None,
        },
    )
    .expect("create a Text Box");
    // `create_text_box` only adds the empty box; typing commits the content.
    pdfx_lib::text_edit::apply_text_edit(
        document_id.to_owned(),
        0,
        created.target.box_id,
        text.to_owned(),
    )
    .expect("type into the Text Box");
}

#[test]
fn save_as_writes_pdfium_page_edits_into_the_output() {
    let dir = TempDir::new("save-edits");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let source_before = file_state(&source);
    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Saved by PDFX");
    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after edit");
    assert!(history.dirty, "an edit makes the document dirty");

    let (result, _) = save(&document.document_id, save_request(Some(&output)));

    assert!(!result.history.dirty, "save establishes the clean baseline");
    assert!(
        result.history.can_undo,
        "saving must not clear undo history"
    );

    let source_after = file_state(&source);
    assert_eq!(
        source_after.0, source_before.0,
        "source bytes must not change"
    );
    assert_eq!(
        source_after.1, source_before.1,
        "source mtime must not change"
    );

    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source_before.0),
        "unencrypted saves append a PDFium revision to the source bytes"
    );

    let reopened = open(&output);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text from the saved output")
        .text;
    assert!(
        text.contains("Saved by PDFX"),
        "the Text Box must be in the saved file, got {text:?}"
    );
    assert!(
        text.contains("PDFX fixture: text layer."),
        "unedited text must survive, got {text:?}"
    );

    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

#[test]
fn save_changes_overwrites_the_output_and_backs_up_the_previous_version() {
    let dir = TempDir::new("save-changes");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("report.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First version");
    let (first, _) = save(&document.document_id, save_request(Some(&output)));
    let first_bytes = fs::read(&output).expect("read first output");
    assert_eq!(first.backup_path, None);

    create_box(&document.document_id, 72.0, 200.0, "Second version");
    let (second, _) = save(&document.document_id, save_request(None));

    assert_eq!(second.kind, SaveKind::SaveChanges);
    assert_eq!(second.output_path, path_string(&output));
    assert_eq!(
        second.backup_path,
        Some(path_string(&pdfx_lib::save::backup_path_for(&output))),
        "the backup follows the documented naming convention"
    );

    let backup = pdfx_lib::save::backup_path_for(&output);
    assert_eq!(
        fs::read(&backup).expect("read backup"),
        first_bytes,
        "the backup is the previous Output Document, byte for byte"
    );

    let second_bytes = fs::read(&output).expect("read second output");
    assert_ne!(second_bytes, first_bytes);

    let reopened = open(&output);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text;
    assert!(text.contains("First version"), "got {text:?}");
    assert!(text.contains("Second version"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    let backup_view = open(&backup);
    let backup_text = extract_text(backup_view.document_id.clone(), 0)
        .expect("extract backup text")
        .text;
    assert!(backup_text.contains("First version"), "got {backup_text:?}");
    assert!(
        !backup_text.contains("Second version"),
        "the backup predates the overwrite, got {backup_text:?}"
    );
    close_document(backup_view.document_id).expect("close backup");

    let source_after = file_state(&source);
    assert_eq!(source_after.0, source_before.0);
    assert_eq!(source_after.1, source_before.1);
    assert!(saved_temp_files(dir.path()).is_empty());

    close_document(document.document_id).expect("close source");
}

fn save_error(document_id: &str, request: SaveDocumentRequest) -> pdfx_lib::commands::CommandError {
    let mut progress = Vec::new();
    save_document_with_progress(document_id, request, &mut |update| progress.push(update))
        .expect_err("save should fail")
}

#[test]
fn save_as_into_a_missing_directory_fails_and_can_be_retried() {
    let dir = TempDir::new("missing-dir");
    let source = copy_fixture("text", &dir, "source.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Unsaved work");

    let missing = dir.path().join("missing").join("output.pdf");
    let error = save_error(&document.document_id, save_request(Some(&missing)));
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);
    assert!(!missing.exists());
    assert!(!dir.path().join("missing").exists());

    let source_after = file_state(&source);
    assert_eq!(
        source_after.0, source_before.0,
        "source bytes must not change"
    );
    assert_eq!(
        source_after.1, source_before.1,
        "source mtime must not change"
    );
    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after failed save");
    assert!(history.dirty, "a failed save must not clean the document");

    // Retryable: the same document saves to a valid target afterwards.
    let output = dir.path().join("output.pdf");
    let (result, _) = save(&document.document_id, save_request(Some(&output)));
    assert!(!result.history.dirty);
    assert!(output.exists());
    close_document(document.document_id).expect("close source");
}

#[cfg(unix)]
#[test]
fn save_changes_into_a_read_only_directory_preserves_both_files() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("read-only-dir");
    let source = copy_fixture("text", &dir, "source.pdf");
    let target_dir = dir.path().join("locked");
    fs::create_dir(&target_dir).expect("create target dir");
    let output = target_dir.join("report.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First version");
    save(&document.document_id, save_request(Some(&output)));
    let previous_output = fs::read(&output).expect("read previous output");
    let source_before = file_state(&source);

    create_box(&document.document_id, 72.0, 200.0, "Second version");
    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o555))
        .expect("make target directory read-only");

    let error = save_error(&document.document_id, save_request(None));
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);

    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o755))
        .expect("restore target directory permissions");

    assert_eq!(
        fs::read(&output).expect("read output"),
        previous_output,
        "the previous Output Document must be untouched"
    );
    let source_after = file_state(&source);
    assert_eq!(source_after.0, source_before.0);
    assert_eq!(source_after.1, source_before.1);
    assert!(saved_temp_files(&target_dir).is_empty());
    assert!(
        pdfx_lib::text_edit::edit_history(document.document_id.clone())
            .expect("history")
            .dirty
    );

    // The retry succeeds once the obstacle is gone.
    let (result, _) = save(&document.document_id, save_request(None));
    assert!(!result.history.dirty);
    assert_ne!(fs::read(&output).expect("read output"), previous_output);
    close_document(document.document_id).expect("close source");
}

#[cfg(unix)]
#[test]
fn a_failed_protected_save_changes_preserves_the_previous_encrypted_output() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("protected-write-failure");
    let source = copy_fixture("text", &dir, "source.pdf");
    let target_dir = dir.path().join("locked");
    fs::create_dir(&target_dir).expect("create target dir");
    let output = target_dir.join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First protected version");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );
    let protected_before = file_state(&output);

    create_box(
        &document.document_id,
        72.0,
        200.0,
        "Second protected version",
    );
    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o555))
        .expect("make target directory read-only");

    let new_password = "pdfx-output-secret-40-after-failure";
    let error = save_error(
        &document.document_id,
        save_request_with_password(None, new_password),
    );
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);
    assert!(
        !error.message.contains(OUTPUT_PASSWORD) && !error.message.contains(new_password),
        "error text must not leak the password, got {:?}",
        error.message
    );

    fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o755))
        .expect("restore target directory permissions");

    assert_eq!(
        file_state(&output),
        protected_before,
        "the previous protected output is byte-for-byte intact"
    );
    let still_protected = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("the previous output still opens with its password");
    close_document(still_protected.document_id).expect("close previous output");
    assert_eq!(file_state(&source), source_before);
    assert!(
        saved_temp_files(dir.path()).is_empty(),
        "no temp files remain"
    );
    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after failed protected save");
    assert!(history.dirty);

    // Retryable with a new password.
    let (result, _) = save(
        &document.document_id,
        save_request_with_password(None, new_password),
    );
    assert!(result.kind == SaveKind::SaveChanges);
    assert!(result.encrypted);
    let reopened = open_document(path_string(&output), Some(new_password.to_owned()))
        .expect("the retry writes the new password's output");
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("Second protected version"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

#[test]
fn a_write_failure_keeps_the_previous_output_and_reports_a_retryable_error() {
    let dir = TempDir::new("write-failure");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("report.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "First version");
    save(&document.document_id, save_request(Some(&output)));
    let previous_output = fs::read(&output).expect("read previous output");
    let source_before = file_state(&source);

    // Simulate a write failure: the output path is occupied by a directory, so
    // the backup copy before the replace fails.
    fs::remove_file(&output).expect("remove output");
    fs::create_dir(&output).expect("occupy output path with a directory");

    create_box(&document.document_id, 72.0, 200.0, "Second version");
    let error = save_error(&document.document_id, save_request(None));
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);
    assert!(output.is_dir(), "the blocking directory is untouched");
    assert!(
        pdfx_lib::text_edit::edit_history(document.document_id.clone())
            .expect("history")
            .dirty
    );
    let source_after = file_state(&source);
    assert_eq!(source_after.0, source_before.0);
    assert_eq!(source_after.1, source_before.1);
    assert!(saved_temp_files(dir.path()).is_empty());

    // Remove the obstacle and retry: the save succeeds and replaces the path.
    fs::remove_dir(&output).expect("remove blocking directory");
    let (result, _) = save(&document.document_id, save_request(None));
    assert!(!result.history.dirty);
    assert!(output.is_file());
    assert_ne!(fs::read(&output).expect("read output"), previous_output);
    close_document(document.document_id).expect("close source");
}

#[test]
fn save_changes_without_an_output_document_reports_that_save_as_is_needed() {
    let dir = TempDir::new("no-output");
    let source = copy_fixture("text", &dir, "source.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Unsaved work");
    let error = save_error(&document.document_id, save_request(None));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::NoOutputDocument
    );
    assert!(
        pdfx_lib::text_edit::edit_history(document.document_id.clone())
            .expect("history")
            .dirty
    );
    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

#[test]
fn save_as_refuses_to_overwrite_the_source_document() {
    let dir = TempDir::new("source-target");
    let source = copy_fixture("structure", &dir, "source.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Never written to source");

    let error = save_error(&document.document_id, save_request(Some(&source)));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::SaveTargetIsSource
    );
    assert_eq!(file_state(&source), source_before);
    assert!(
        pdfx_lib::text_edit::edit_history(document.document_id.clone())
            .expect("history")
            .dirty
    );

    // A path that resolves to the source is refused too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = dir.path().join("source-link.pdf");
        symlink(&source, &link).expect("create source symlink");
        let error = save_error(&document.document_id, save_request(Some(&link)));
        assert_eq!(
            error.code,
            pdfx_lib::commands::CommandErrorCode::SaveTargetIsSource
        );
        assert_eq!(file_state(&source), source_before);
    }

    // The same document still saves to a different path.
    let output = dir.path().join("copy.pdf");
    let (result, _) = save(&document.document_id, save_request(Some(&output)));
    assert!(!result.history.dirty);
    close_document(document.document_id).expect("close source");
}

#[test]
fn encrypted_source_requires_confirmation_and_writes_an_unencrypted_output() {
    let dir = TempDir::new("encrypted");
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, "encrypted").expect("encrypted fixture");
    let password = fixture.password.clone().expect("user password");
    let source = dir.path().join("source.pdf");
    fs::copy(support::fixtures::fixture_path(fixture), &source).expect("copy encrypted fixture");
    let source_before = file_state(&source);
    let output = dir.path().join("decrypted.pdf");

    let document =
        open_document(path_string(&source), Some(password)).expect("open encrypted document");
    assert!(document.encrypted);
    assert!(!document.read_only);
    create_box(&document.document_id, 72.0, 72.0, "Kept after decrypting");

    // Without the explicit confirmation nothing is written.
    let error = save_error(&document.document_id, save_request(Some(&output)));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::UnencryptedOutputConfirmationRequired
    );
    assert!(!output.exists());
    assert_eq!(file_state(&source), source_before);

    let (result, _) = save(
        &document.document_id,
        save_request_confirmed(Some(&output), true),
    );
    assert!(!result.history.dirty);

    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        !output_bytes.starts_with(&source_before.0),
        "the encrypted source is rewritten without security, not appended"
    );
    let parsed = lopdf::Document::load(&output).expect("lopdf parses the output");
    assert!(!parsed.is_encrypted(), "output must not carry /Encrypt");

    // Reopening needs no password and the edit survived the full rewrite.
    let reopened = open_document(path_string(&output), None).expect("open output without password");
    assert!(!reopened.encrypted);
    assert!(!reopened.read_only);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text;
    assert!(text.contains("Kept after decrypting"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(
        file_state(&source),
        source_before,
        "source must stay encrypted and untouched"
    );
    close_document(document.document_id).expect("close source");
}

/// An encrypted source saved with a password keeps protection, so the
/// ADR-0007 unencrypted-output confirmation does not apply. The output needs
/// the new password, not the source's.
#[test]
fn an_encrypted_source_saved_with_a_new_password_skips_the_unencrypted_confirmation() {
    let dir = TempDir::new("encrypted-reprotected");
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, "encrypted").expect("encrypted fixture");
    let source_password = fixture.password.clone().expect("user password");
    let source = dir.path().join("source.pdf");
    fs::copy(support::fixtures::fixture_path(fixture), &source).expect("copy encrypted fixture");
    let source_before = file_state(&source);
    let output = dir.path().join("reprotected.pdf");
    let new_password = "pdfx-output-secret-40-new";

    let document =
        open_document(path_string(&source), Some(source_password.clone())).expect("open source");
    create_box(&document.document_id, 72.0, 72.0, "Still protected");

    // `confirm_unencrypted_output` stays false: the output is not unencrypted.
    let (result, _) = save(
        &document.document_id,
        save_request_with_password(Some(&output), new_password),
    );
    assert!(result.encrypted);

    let missing = open_document(path_string(&output), None).expect_err("output is protected");
    assert_eq!(
        missing.code,
        pdfx_lib::commands::CommandErrorCode::PasswordRequired
    );
    let old_password = open_document(path_string(&output), Some(source_password))
        .expect_err("the source password is not the output password");
    assert_eq!(
        old_password.code,
        pdfx_lib::commands::CommandErrorCode::InvalidPassword
    );
    let reopened = open_document(path_string(&output), Some(new_password.to_owned()))
        .expect("open with the new output password");
    assert!(reopened.encrypted);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract saved text")
        .text;
    assert!(text.contains("Still protected"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(
        file_state(&source),
        source_before,
        "source must stay encrypted and untouched"
    );
    close_document(document.document_id).expect("close source");
}

#[test]
fn read_only_documents_refuse_save_changes_but_allow_an_unencrypted_save_as() {
    let dir = TempDir::new("read-only-document");
    let manifest = support::fixtures::load_manifest();
    // Empty user password, restricted owner permissions: opens without a
    // password but denies content modification.
    let restricted = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == "encrypted-owner-restrictions")
        .expect("encrypted-owner-restrictions fixture");
    let source = dir.path().join("source.pdf");
    fs::copy(support::fixtures::fixture_path(restricted), &source)
        .expect("copy restricted fixture");
    let source_before = file_state(&source);
    let output = dir.path().join("copy.pdf");

    let document = open_document(path_string(&source), None).expect("open restricted document");
    assert!(document.encrypted);
    assert!(document.read_only);

    // Save Changes is refused before any target is needed, matching ticket 06.
    let error = save_error(&document.document_id, save_request_confirmed(None, true));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::ReadOnlyDocument
    );
    assert!(!output.exists());

    // Save As is available and still requires the unencrypted-output warning.
    let error = save_error(&document.document_id, save_request(Some(&output)));
    assert_eq!(
        error.code,
        pdfx_lib::commands::CommandErrorCode::UnencryptedOutputConfirmationRequired
    );
    assert!(!output.exists());

    let (result, _) = save(
        &document.document_id,
        save_request_confirmed(Some(&output), true),
    );
    assert_eq!(result.kind, SaveKind::SaveAs);
    let reopened = open_document(path_string(&output), None).expect("open unencrypted copy");
    assert!(!reopened.encrypted);
    assert!(!reopened.read_only);
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

#[test]
fn save_progress_follows_the_documented_contract() {
    let dir = TempDir::new("progress");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Progress");
    let (result, progress) = save(&document.document_id, save_request(Some(&output)));

    let stages: Vec<SaveStage> = progress.iter().map(|update| update.stage).collect();
    assert_eq!(
        stages,
        vec![
            SaveStage::Preparing,
            SaveStage::Serializing,
            SaveStage::Finalizing,
            SaveStage::Writing,
            SaveStage::Done,
        ],
        "every stage is announced in order and Done is last"
    );
    assert_eq!(
        progress
            .iter()
            .map(|update| update.completed)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert!(progress
        .iter()
        .all(|update| update.document_id == document.document_id && update.total == 4));

    // The frontend receives camelCase JSON with stable stage names.
    let done = serde_json::to_value(progress.last().expect("Done")).expect("serialize progress");
    assert_eq!(done["stage"], "done");
    assert_eq!(done["documentId"], document.document_id);
    assert_eq!(done["completed"], 4);
    assert_eq!(done["total"], 4);

    let request: SaveDocumentRequest = serde_json::from_value(serde_json::json!({
        "saveAsPath": path_string(&output),
        "confirmUnencryptedOutput": false,
    }))
    .expect("deserialize a Save As request");
    assert_eq!(request.save_as_path, Some(path_string(&output)));
    assert!(!request.confirm_unencrypted_output);

    let view = serde_json::to_value(&result).expect("serialize the save result");
    assert_eq!(view["kind"], "save_as");
    assert_eq!(view["outputPath"], path_string(&output));
    assert_eq!(view["history"]["dirty"], false);
    assert_eq!(view["history"]["canUndo"], true);
    close_document(document.document_id).expect("close source");
}

#[test]
fn a_failed_save_never_reports_done() {
    let dir = TempDir::new("progress-failure");
    let source = copy_fixture("text", &dir, "source.pdf");
    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Progress");

    let mut progress = Vec::new();
    let missing = dir.path().join("missing").join("output.pdf");
    let error = save_document_with_progress(
        &document.document_id,
        save_request(Some(&missing)),
        &mut |update| progress.push(update),
    )
    .expect_err("the save fails");
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);

    let stages: Vec<SaveStage> = progress.iter().map(|update| update.stage).collect();
    assert_eq!(
        stages,
        vec![
            SaveStage::Preparing,
            SaveStage::Serializing,
            SaveStage::Finalizing,
            SaveStage::Writing,
        ],
        "the failure is reported after the stage that failed, never as Done"
    );

    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after a failed save");
    assert!(history.dirty, "a failed save leaves the document dirty");
    assert!(history.can_undo, "a failed save keeps the edit history");
    close_document(document.document_id).expect("close source");
}

#[test]
fn undo_after_save_marks_the_document_dirty_again() {
    let dir = TempDir::new("undo-after-save");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Undo me");
    let (result, _) = save(&document.document_id, save_request(Some(&output)));
    assert!(!result.history.dirty);
    assert!(result.history.can_undo, "save keeps the undo history");

    let history = pdfx_lib::text_edit::edit_history(document.document_id.clone())
        .expect("history after a successful save");
    assert!(
        !history.dirty,
        "the command boundary reports the clean baseline"
    );
    assert!(
        history.can_undo,
        "the command boundary keeps the undo history"
    );

    let undone =
        pdfx_lib::text_edit::undo_text_edit(document.document_id.clone()).expect("undo after save");
    assert!(
        undone.history.dirty,
        "undoing past the saved baseline is dirty"
    );
    assert!(undone.history.can_redo);

    let redone = pdfx_lib::text_edit::redo_text_edit(document.document_id.clone())
        .expect("redo back to the saved baseline");
    assert!(!redone.history.dirty, "the saved revision is clean again");
    close_document(document.document_id).expect("close source");
}

/// Counts the cross-reference sections in a file. Each incremental revision
/// appends exactly one, so a delta proves who wrote how many revisions.
fn startxref_count(bytes: &[u8]) -> usize {
    bytes
        .windows(b"startxref".len())
        .filter(|window| *window == b"startxref")
        .count()
}

#[test]
fn a_clean_unedited_save_copies_the_source_bytes_verbatim() {
    let dir = TempDir::new("verbatim");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let source_bytes = fs::read(&source).expect("read source");
    let document = open(&source);
    save(&document.document_id, save_request(Some(&output)));
    close_document(document.document_id).expect("close source");

    assert_eq!(
        fs::read(&output).expect("read output"),
        source_bytes,
        "an unedited first save is the source bytes, byte for byte"
    );
}

#[test]
fn an_edited_save_appends_exactly_one_pdfium_revision() {
    let dir = TempDir::new("one-revision");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let source_bytes = fs::read(&source).expect("read source");
    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "One revision");
    save(&document.document_id, save_request(Some(&output)));
    close_document(document.document_id).expect("close source");

    let output_bytes = fs::read(&output).expect("read output");
    assert!(
        output_bytes.starts_with(&source_bytes),
        "the source bytes stay untouched and the revision is appended"
    );
    assert_eq!(
        startxref_count(&output_bytes),
        startxref_count(&source_bytes) + 1,
        "PDFium appends exactly one incremental revision"
    );
}

#[test]
fn save_changes_without_new_edits_keeps_the_previously_saved_edits() {
    let dir = TempDir::new("clean-save-changes");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Saved once");
    let (first, _) = save(&document.document_id, save_request(Some(&output)));
    assert!(!first.history.dirty);

    // Save Changes with no new edits must not regress the Output Document to
    // the Source Document: the edit saved above has to survive.
    let (second, _) = save(&document.document_id, save_request(None));
    assert_eq!(second.kind, SaveKind::SaveChanges);
    assert!(!second.history.dirty);
    assert!(second.history.can_undo, "history survives a no-op save");

    let reopened = open(&output);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(
        text.contains("Saved once"),
        "the previously saved edit must still be in the output, got {text:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

/// Parses `path` independently of PDFium, the way an external viewer would.
fn parse_with_lopdf(path: &Path) -> lopdf::Document {
    lopdf::Document::load(path).expect("lopdf parses the saved file")
}

fn catalog(document: &lopdf::Document) -> &lopdf::Dictionary {
    let root = document
        .trailer
        .get(b"Root")
        .expect("trailer Root")
        .as_reference()
        .expect("Root reference");
    document
        .get_object(root)
        .expect("catalog object")
        .as_dict()
        .expect("catalog dictionary")
}

/// Resolves an indirect or direct dictionary. PDF producers use both forms.
fn dict_of<'a>(document: &'a lopdf::Document, object: &'a lopdf::Object) -> &'a lopdf::Dictionary {
    match object.as_reference() {
        Ok(id) => document
            .get_object(id)
            .expect("referenced object")
            .as_dict()
            .expect("referenced dictionary"),
        Err(_) => object.as_dict().expect("direct dictionary"),
    }
}

fn text_of(object: &lopdf::Object) -> String {
    String::from_utf8_lossy(object.as_str().expect("string object")).into_owned()
}

/// Decodes a PDF text string, including the UTF-16BE form used for CJK
/// values (a leading `FE FF` byte-order mark).
fn text_string_of(object: &lopdf::Object) -> String {
    let bytes = object.as_str().expect("string object");
    if let Some(units) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = units
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn name_or_text(object: &lopdf::Object) -> String {
    match object {
        lopdf::Object::Name(name) => String::from_utf8_lossy(name).into_owned(),
        object => text_of(object),
    }
}

fn outline_titles(document: &lopdf::Document) -> Vec<String> {
    let catalog = catalog(document);
    let outlines = dict_of(document, catalog.get(b"Outlines").expect("Outlines"));
    let mut titles = Vec::new();
    let mut next = outlines.get(b"First").and_then(|o| o.as_reference()).ok();
    while let Some(id) = next {
        let item = document
            .get_object(id)
            .expect("outline item")
            .as_dict()
            .expect("outline dictionary");
        titles.push(text_of(item.get(b"Title").expect("outline title")));
        next = item.get(b"Next").and_then(|o| o.as_reference()).ok();
    }
    titles
}

fn page_dictionary(document: &lopdf::Document, page_number: u32) -> &lopdf::Dictionary {
    let id = *document.get_pages().get(&page_number).expect("page number");
    document
        .get_object(id)
        .expect("page object")
        .as_dict()
        .expect("page dictionary")
}

#[test]
fn save_as_preserves_bookmarks_links_metadata_attachments_and_page_content() {
    let dir = TempDir::new("structure");
    let source = copy_fixture("structure", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "Structure kept");
    save(&document.document_id, save_request(Some(&output)));

    let parsed = parse_with_lopdf(&output);
    assert_eq!(outline_titles(&parsed), vec!["Introduction", "Details"]);

    // The link annotation and its URI action survived.
    let first_page = page_dictionary(&parsed, 1);
    let annotations = first_page
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    let uris: Vec<String> = annotations
        .iter()
        .filter_map(|annotation| {
            let annotation = dict_of(&parsed, annotation);
            let action = annotation.get(b"A").ok()?;
            let action = dict_of(&parsed, action);
            action.get(b"URI").ok().map(text_of)
        })
        .collect();
    assert_eq!(uris, vec!["https://example.com/pdfx"]);

    // Info dictionary and XMP metadata are both still present.
    let info = parsed
        .trailer
        .get(b"Info")
        .expect("trailer Info")
        .as_reference()
        .expect("Info reference");
    let info = parsed
        .get_object(info)
        .expect("Info object")
        .as_dict()
        .unwrap();
    assert_eq!(
        text_of(info.get(b"Title").unwrap()),
        "PDFX structure fixture"
    );
    assert_eq!(text_of(info.get(b"Author").unwrap()), "PDFX QA");

    let metadata = catalog(&parsed).get(b"Metadata").expect("/Metadata");
    let metadata = parsed
        .get_object(metadata.as_reference().unwrap())
        .expect("metadata stream")
        .as_stream()
        .expect("metadata stream object");
    let xmp_bytes = metadata.decompressed_content().expect("XMP content");
    let xmp = String::from_utf8_lossy(&xmp_bytes);
    assert!(
        xmp.contains("PDFX structure fixture"),
        "XMP must be preserved"
    );

    // The embedded file keeps its name and bytes.
    let names = catalog(&parsed).get(b"Names").expect("/Names");
    let names = dict_of(&parsed, names);
    let embedded = names.get(b"EmbeddedFiles").expect("/EmbeddedFiles");
    let embedded = dict_of(&parsed, embedded);
    let pairs = embedded.get(b"Names").expect("/Names").as_array().unwrap();
    assert_eq!(text_of(&pairs[0]), "notes.txt");
    let filespec = dict_of(&parsed, &pairs[1]);
    let ef = filespec.get(b"EF").expect("/EF");
    let ef = dict_of(&parsed, ef);
    let file = parsed
        .get_object(ef.get(b"F").unwrap().as_reference().unwrap())
        .expect("embedded file stream")
        .as_stream()
        .expect("embedded file stream object");
    assert_eq!(
        file.decompressed_content().expect("embedded file content"),
        b"PDFX fixture attachment.\n"
    );

    // Both the original page text and the new Text Box are in the file.
    let reopened = open(&output);
    assert_eq!(reopened.page_count, 1);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("PDFX structure fixture"), "got {text:?}");
    assert!(text.contains("Structure kept"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

#[test]
fn save_as_preserves_existing_acroform_fields() {
    let dir = TempDir::new("form");
    let source = copy_fixture("form", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "Form kept");
    save(&document.document_id, save_request(Some(&output)));

    let parsed = parse_with_lopdf(&output);
    let acro_form = catalog(&parsed).get(b"AcroForm").expect("/AcroForm");
    let acro_form = dict_of(&parsed, acro_form);
    let fields = acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array");

    let mut values = Vec::new();
    let mut kinds = Vec::new();
    for field in fields {
        let field = dict_of(&parsed, field);
        let name = text_of(field.get(b"T").expect("field name"));
        let value = field
            .get(b"V")
            .map(name_or_text)
            .unwrap_or_else(|_| "<missing>".to_owned());
        kinds.push(field.get(b"FT").map(name_or_text).unwrap_or_default());
        values.push((name, value));
    }
    assert_eq!(
        values,
        vec![
            ("full_name".to_owned(), "Ada Lovelace".to_owned()),
            ("subscribe".to_owned(), "Off".to_owned()),
            ("email".to_owned(), "<missing>".to_owned()),
            ("gender".to_owned(), "Off".to_owned()),
            ("country".to_owned(), "Japan".to_owned()),
            ("department".to_owned(), "<missing>".to_owned()),
            ("notes".to_owned(), "<missing>".to_owned()),
            ("agree".to_owned(), "Off".to_owned()),
            ("js_guard".to_owned(), "<missing>".to_owned()),
            ("readonly_note".to_owned(), "Read only".to_owned()),
        ],
        "existing form fields, values and types survive an edit + save"
    );
    assert_eq!(
        kinds,
        vec!["Tx", "Btn", "Tx", "Btn", "Ch", "Ch", "Tx", "Btn", "Tx", "Tx"]
    );

    let reopened = open(&output);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("Form kept"), "got {text:?}");
    assert!(text.contains("Ada Lovelace"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

/// Encryption is a full rewrite of the object graph, so the acceptance check
/// is that everything outside the edited page survives it: bookmarks, link
/// annotations, Info/XMP metadata and the embedded attachment.
#[test]
fn saving_with_a_password_preserves_bookmarks_links_metadata_and_attachments() {
    let dir = TempDir::new("encrypted-structure");
    let source = copy_fixture("structure", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "Structure kept");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );

    let parsed = lopdf::Document::load_with_password(&output, OUTPUT_PASSWORD)
        .expect("the output decrypts with its password");
    assert_eq!(outline_titles(&parsed), vec!["Introduction", "Details"]);

    let first_page = page_dictionary(&parsed, 1);
    let annotations = first_page
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    let uris: Vec<String> = annotations
        .iter()
        .filter_map(|annotation| {
            let annotation = dict_of(&parsed, annotation);
            let action = annotation.get(b"A").ok()?;
            let action = dict_of(&parsed, action);
            action.get(b"URI").ok().map(text_of)
        })
        .collect();
    assert_eq!(uris, vec!["https://example.com/pdfx"]);

    let info = parsed
        .trailer
        .get(b"Info")
        .expect("trailer Info")
        .as_reference()
        .expect("Info reference");
    let info = parsed
        .get_object(info)
        .expect("Info object")
        .as_dict()
        .unwrap();
    assert_eq!(
        text_of(info.get(b"Title").unwrap()),
        "PDFX structure fixture"
    );
    assert_eq!(text_of(info.get(b"Author").unwrap()), "PDFX QA");

    let metadata = catalog(&parsed).get(b"Metadata").expect("/Metadata");
    let metadata = parsed
        .get_object(metadata.as_reference().unwrap())
        .expect("metadata stream")
        .as_stream()
        .expect("metadata stream object");
    let xmp_bytes = metadata.decompressed_content().expect("XMP content");
    assert!(
        String::from_utf8_lossy(&xmp_bytes).contains("PDFX structure fixture"),
        "XMP must survive encryption"
    );

    let names = catalog(&parsed).get(b"Names").expect("/Names");
    let names = dict_of(&parsed, names);
    let embedded = dict_of(
        &parsed,
        names.get(b"EmbeddedFiles").expect("/EmbeddedFiles"),
    );
    let pairs = embedded.get(b"Names").expect("/Names").as_array().unwrap();
    assert_eq!(text_of(&pairs[0]), "notes.txt");
    let filespec = dict_of(&parsed, &pairs[1]);
    let ef = dict_of(&parsed, filespec.get(b"EF").expect("/EF"));
    let file = parsed
        .get_object(ef.get(b"F").unwrap().as_reference().unwrap())
        .expect("embedded file stream")
        .as_stream()
        .expect("embedded file stream object");
    assert_eq!(
        file.decompressed_content().expect("embedded file content"),
        b"PDFX fixture attachment.\n"
    );

    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("reopen the protected output");
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("PDFX structure fixture"), "got {text:?}");
    assert!(text.contains("Structure kept"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

/// Annotations created in the session are PDFium-revision additions. The
/// encryption rewrite must keep their dictionaries and appearance streams,
/// and PDFium must still list them after reopening with the password.
#[test]
fn saving_with_a_password_preserves_native_annotations_created_in_the_session() {
    use pdfx_lib::annotations::{
        create_note_with_author, list_annotations, CreateNoteRequest, TextMarkupRect,
    };

    let dir = TempDir::new("encrypted-annotation");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "Annotation kept");
    create_note_with_author(
        document.document_id.clone(),
        "PDFX QA".to_owned(),
        CreateNoteRequest {
            page_index: 0,
            color: "#f9a825".to_owned(),
            opacity: 100,
            text: "加密保留的便签".to_owned(),
            rect: TextMarkupRect {
                x: 400.0,
                y: 300.0,
                width: 40.0,
                height: 40.0,
            },
        },
    )
    .expect("create a sticky note");

    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );

    let parsed = lopdf::Document::load_with_password(&output, OUTPUT_PASSWORD)
        .expect("the output decrypts with its password");
    let first_page = page_dictionary(&parsed, 1);
    let annotations = first_page
        .get(b"Annots")
        .expect("/Annots")
        .as_array()
        .expect("/Annots array");
    let note = annotations
        .iter()
        .map(|annotation| dict_of(&parsed, annotation))
        .find(|annotation| {
            matches!(
                annotation.get(b"Subtype").and_then(lopdf::Object::as_name),
                Ok(subtype) if subtype == b"Text"
            )
        })
        .expect("the sticky note survives encryption");
    assert_eq!(
        text_string_of(note.get(b"Contents").expect("/Contents")),
        "加密保留的便签"
    );
    assert_eq!(text_string_of(note.get(b"T").expect("/T")), "PDFX QA");
    assert!(
        note.get(b"AP").is_ok(),
        "the custom appearance stream survives encryption"
    );

    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("reopen the protected output");
    let listed = list_annotations(reopened.document_id.clone()).expect("list annotations");
    assert!(
        listed.iter().any(|annotation| {
            annotation.text.as_deref() == Some("加密保留的便签")
                && annotation.author.as_deref() == Some("PDFX QA")
        }),
        "PDFium lists the decrypted note, got {listed:?}"
    );
    close_document(reopened.document_id).expect("close reopened");
    close_document(document.document_id).expect("close source");
}

#[test]
fn saving_with_a_password_preserves_existing_acroform_fields() {
    let dir = TempDir::new("encrypted-form");
    let source = copy_fixture("form", &dir, "source.pdf");
    let output = dir.path().join("protected.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "Form kept");
    save(
        &document.document_id,
        save_request_with_password(Some(&output), OUTPUT_PASSWORD),
    );

    let parsed = lopdf::Document::load_with_password(&output, OUTPUT_PASSWORD)
        .expect("the output decrypts with its password");
    let acro_form = catalog(&parsed).get(b"AcroForm").expect("/AcroForm");
    let acro_form = dict_of(&parsed, acro_form);
    let fields = acro_form
        .get(b"Fields")
        .expect("/Fields")
        .as_array()
        .expect("field array");
    let values: Vec<(String, String)> = fields
        .iter()
        .map(|field| {
            let field = dict_of(&parsed, field);
            (
                text_of(field.get(b"T").expect("field name")),
                field
                    .get(b"V")
                    .map(name_or_text)
                    .unwrap_or_else(|_| "<missing>".to_owned()),
            )
        })
        .collect();
    assert_eq!(
        values,
        vec![
            ("full_name".to_owned(), "Ada Lovelace".to_owned()),
            ("subscribe".to_owned(), "Off".to_owned()),
            ("email".to_owned(), "<missing>".to_owned()),
            ("gender".to_owned(), "Off".to_owned()),
            ("country".to_owned(), "Japan".to_owned()),
            ("department".to_owned(), "<missing>".to_owned()),
            ("notes".to_owned(), "<missing>".to_owned()),
            ("agree".to_owned(), "Off".to_owned()),
            ("js_guard".to_owned(), "<missing>".to_owned()),
            ("readonly_note".to_owned(), "Read only".to_owned()),
        ],
        "form fields, values and types survive encryption"
    );

    let reopened = open_document(path_string(&output), Some(OUTPUT_PASSWORD.to_owned()))
        .expect("reopen the protected output");
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("Form kept"), "got {text:?}");
    assert!(text.contains("Ada Lovelace"), "got {text:?}");
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

#[test]
fn a_large_document_saves_with_progress_and_keeps_untouched_pages() {
    let dir = TempDir::new("large");
    let source = copy_fixture("large", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let source_before = file_state(&source);

    let document = open(&source);
    assert_eq!(document.page_count, 500);
    create_box(&document.document_id, 72.0, 200.0, "Edited page one");
    let (result, progress) = save(&document.document_id, save_request(Some(&output)));
    assert!(!result.history.dirty);
    assert_eq!(
        progress.last().map(|update| update.stage),
        Some(SaveStage::Done),
        "a large save reports the full progress contract"
    );

    // Editing one page leaves every other page byte-identical in the
    // appended revision; all 500 pages are still readable.
    let output_bytes = fs::read(&output).expect("read output");
    assert!(output_bytes.starts_with(&source_before.0));

    let reopened = open(&output);
    assert_eq!(reopened.page_count, 500);
    let first = extract_text(reopened.document_id.clone(), 0)
        .expect("extract first page")
        .text;
    assert!(first.contains("Edited page one"), "got {first:?}");
    assert!(
        first.contains("PDFX large fixture: page 1 of 500"),
        "got {first:?}"
    );
    let last = extract_text(reopened.document_id.clone(), 499)
        .expect("extract last page")
        .text;
    assert!(
        last.contains("PDFX large fixture: page 500 of 500"),
        "untouched pages survive, got {last:?}"
    );
    close_document(reopened.document_id).expect("close reopened");

    assert_eq!(file_state(&source), source_before);
    close_document(document.document_id).expect("close source");
}

#[test]
fn object_level_edits_are_appended_as_one_lopdf_revision() {
    use pdfx_lib::save::{append_object_edits, ObjectEdit};

    let dir = TempDir::new("object-edits");
    let source = copy_fixture("structure", &dir, "source.pdf");
    let pdfium_output = dir.path().join("pdfium.pdf");

    let document = open(&source);
    create_box(&document.document_id, 72.0, 200.0, "PDFium first");
    save(&document.document_id, save_request(Some(&pdfium_output)));
    close_document(document.document_id).expect("close source");

    let pdfium_bytes = fs::read(&pdfium_output).expect("read PDFium output");
    let appended = append_object_edits(
        pdfium_bytes.clone(),
        &[ObjectEdit::SetInfoEntry {
            key: "PDFXSpike".to_owned(),
            value: "lopdf-append".to_owned(),
        }],
    )
    .expect("append the object-level revision");

    assert!(
        appended.starts_with(&pdfium_bytes),
        "lopdf appends to PDFium's bytes; it never rewrites them"
    );
    assert_eq!(
        startxref_count(&appended),
        startxref_count(&pdfium_bytes) + 1,
        "lopdf appends exactly one revision after PDFium's revision"
    );

    let parsed = lopdf::Document::load_mem(&appended).expect("parse the appended output");
    let info = parsed
        .trailer
        .get(b"Info")
        .expect("trailer Info")
        .as_reference()
        .expect("Info reference");
    let info = parsed
        .get_object(info)
        .expect("Info object")
        .as_dict()
        .unwrap();
    assert_eq!(
        text_of(info.get(b"PDFXSpike").expect("new Info entry")),
        "lopdf-append"
    );
    assert_eq!(
        text_of(info.get(b"Title").expect("existing title")),
        "PDFX structure fixture",
        "the lopdf revision only changes the object it names"
    );

    // PDFium still reads everything in both revisions.
    let appended_path = dir.path().join("appended.pdf");
    fs::write(&appended_path, &appended).expect("write appended output");
    let reopened = open(&appended_path);
    let text = extract_text(reopened.document_id.clone(), 0)
        .expect("extract text")
        .text;
    assert!(text.contains("PDFium first"), "got {text:?}");
    assert_eq!(
        outline_titles(&parse_with_lopdf(&appended_path)),
        vec!["Introduction", "Details"]
    );
    close_document(reopened.document_id).expect("close reopened");

    // No object edits means the PDFium bytes are already final.
    assert_eq!(
        append_object_edits(pdfium_bytes.clone(), &[]).expect("empty append"),
        pdfium_bytes
    );
}

#[test]
fn a_malformed_base_for_object_edits_reports_a_save_failure() {
    use pdfx_lib::save::{append_object_edits, ObjectEdit};
    let error = append_object_edits(
        b"not a PDF".to_vec(),
        &[ObjectEdit::SetInfoEntry {
            key: "Title".to_owned(),
            value: "nope".to_owned(),
        }],
    )
    .expect_err("a malformed base cannot be appended to");
    assert_eq!(error.code, pdfx_lib::commands::CommandErrorCode::SaveFailed);
}
