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

use pdfx_lib::commands::{close_document, extract_text, open_document, DocumentView};
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
    assert!(line.contains("outcome=success"), "logged outcome: {line}");
    assert!(line.contains("kind=SaveAs"), "logged kind: {line}");
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
        ],
        "existing form fields, values and types survive an edit + save"
    );
    assert_eq!(kinds, vec!["Tx".to_owned(), "Btn".to_owned()]);

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

#[test]
fn a_successful_save_records_a_document_save_event() {
    // The logger is process-wide; this binary initializes it once, into a
    // temporary directory, so the event written by the save pipeline can be
    // read back. Other tests in this binary may append their own events.
    let dir = TempDir::new("save-log");
    let log_dir = TempDir::new("save-log-events");
    pdfx_lib::logging::initialize(log_dir.path()).expect("initialize the global logger");

    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let document = open(&source);
    create_box(&document.document_id, 72.0, 72.0, "Logged save");
    save(&document.document_id, save_request(Some(&output)));
    close_document(document.document_id).expect("close source");

    let contents = fs::read_to_string(log_dir.path().join(pdfx_lib::logging::LOG_FILE_NAME))
        .expect("read the local log file");
    let expected_path = format!("path={}", output.display());
    // Other tests in this binary save concurrently to the same global logger,
    // so select this save's event by its output path.
    let save_line = contents
        .lines()
        .find(|line| line.contains(" INFO document.save ") && line.contains(&expected_path))
        .expect("a document.save event for this save");
    assert!(save_line.contains("kind=save_as"), "line: {save_line}");
    assert!(save_line.contains("outcome=ok"), "line: {save_line}");
    assert!(
        !save_line.contains("Logged save"),
        "the log never contains document contents: {save_line}"
    );
}
