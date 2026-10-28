//! Command-boundary tests for Recovery Files and the recent-files list.
//!
//! Recovery behaviour is exercised through the same Rust functions the Tauri
//! IPC layer exposes: edits really run through the text-edit commands, snapshots
//! through the throttled tick, and a restore reopens the snapshot bytes and
//! saves them through the normal save pipeline. Snapshot scheduling uses the
//! injected clock, so debounce and maximum-interval behaviour is deterministic.
//!
//! Tests in this binary share the process-global recovery/logging stores (the
//! app installs them once via `OnceLock`), so they run serialized. Snapshot
//! layout and recent-list mechanics also have isolated unit tests in
//! `src/recovery.rs`; these tests focus on the command seams.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use pdfx_lib::commands::{
    close_document, discard_recovery, extract_text, forget_recent_file, list_recent_files,
    open_document, recovery_candidates, restore_recovery,
};
use pdfx_lib::recovery::{
    maybe_snapshot_at, maybe_snapshot_in, record_recent_document, RecentFilesStore, RecoveryStore,
};
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest, SaveResultView};
use pdfx_lib::text_edit::{apply_text_edit, create_text_box, edit_history, CreateTextBoxRequest};

use support::temp::TempDir;

/// Serializes tests that touch the process-global recovery store, logger and
/// recent-files list, so entries cannot be evicted by a parallel test.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The process-wide application data directory installed once per test binary.
static APP_DATA: OnceLock<PathBuf> = OnceLock::new();

fn app_data() -> &'static Path {
    APP_DATA.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("pdfx-recovery-appdata-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        pdfx_lib::recovery::initialize(&path).expect("initialize recovery storage");
        path
    })
}

fn global_store() -> RecoveryStore {
    RecoveryStore::new(app_data())
}

/// The process-wide logger can only be installed once; every logging
/// assertion in this binary shares this directory.
static LOG_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

fn log_directory() -> &'static Path {
    LOG_DIRECTORY.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("pdfx-recovery-log-{}", std::process::id()));
        fs::create_dir_all(&path).expect("create log dir");
        pdfx_lib::logging::initialize(&path).expect("initialize test logger");
        path
    })
}

fn fixture_path(category: &str) -> PathBuf {
    let manifest = support::fixtures::load_manifest();
    let fixture = support::fixtures::fixture(&manifest, category).expect("fixture category");
    support::fixtures::fixture_path(fixture)
}

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

fn open(path: &Path) -> pdfx_lib::commands::DocumentView {
    open_document(path_string(path), None).expect("open document")
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

fn save_as(document_id: &str, target: &Path) -> SaveResultView {
    save_document_with_progress(
        document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(target)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save as should succeed")
}

#[test]
fn snapshots_are_throttled_and_created_after_edits() {
    let _serial = serial();
    let dir = TempDir::new("recovery-throttle");
    let source = copy_fixture("text", &dir, "source.pdf");
    let source_before = file_state(&source);
    let document = open(&source);
    create_box(&document.document_id, "Recovery throttled edit");

    let store = RecoveryStore::new(dir.path().join("appdata"));
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 0)
            .expect("tick")
            .written,
        "an edit alone must not write immediately"
    );
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 1_999)
            .expect("tick")
            .written,
        "the debounce window has not elapsed"
    );
    let written = maybe_snapshot_in(&store, &document.document_id, 2_000).expect("tick");
    assert!(written.written, "two quiet seconds after the edit");
    let session = written.session_id.clone().expect("session id");

    let scan = store.scan().expect("scan");
    assert_eq!(scan.candidates.len(), 1);
    assert_eq!(scan.candidates[0].session_id, session);
    assert_eq!(scan.candidates[0].source_path, source);
    assert!(scan.candidates[0].dirty);
    assert!(scan.candidates[0].revision > scan.candidates[0].saved_revision);

    let snapshot = store
        .read_snapshot(&session)
        .expect("read snapshot")
        .expect("snapshot exists");
    assert_ne!(
        snapshot.bytes, source_before.0,
        "the snapshot contains the unsaved edit, not the Source Document bytes"
    );
    assert!(snapshot.bytes.starts_with(b"%PDF"));

    assert_eq!(
        file_state(&source),
        source_before,
        "writing a snapshot must never modify the Source Document"
    );

    // A second edit gets its own debounce window; the previous write does not
    // make the next keystroke write immediately.
    create_box(&document.document_id, "A second unsaved edit");
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 2_100)
            .expect("tick")
            .written
    );
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 4_099)
            .expect("tick")
            .written,
        "the quiet period restarts with the newest edit"
    );
    assert!(
        maybe_snapshot_in(&store, &document.document_id, 4_100)
            .expect("tick")
            .written,
        "the snapshot is written once editing is quiet again"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn snapshots_live_in_app_data_never_at_the_source_or_output() {
    let _serial = serial();
    let dir = TempDir::new("recovery-paths");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let app = dir.path().join("appdata");
    let store = RecoveryStore::new(&app);
    let document = open(&source);

    create_box(&document.document_id, "Saved edit");
    save_as(&document.document_id, &output);
    let source_before = file_state(&source);
    let output_before = file_state(&output);

    create_box(&document.document_id, "Unsaved edit after the save");
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 10_000)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_in(&store, &document.document_id, 12_000).expect("tick");
    assert!(written.written);

    assert!(
        store.directory().starts_with(&app),
        "recovery files live in the app data directory"
    );
    let candidate = store
        .scan()
        .expect("scan")
        .candidates
        .into_iter()
        .next()
        .expect("candidate");
    assert_eq!(candidate.source_path, source);
    assert_eq!(candidate.output_path.as_deref(), Some(output.as_path()));

    for entry in fs::read_dir(store.directory()).expect("list recovery directory") {
        let path = entry.expect("entry").path();
        assert_ne!(
            path, source,
            "a recovery file must not be the Source Document"
        );
        assert_ne!(
            path, output,
            "a recovery file must not be the Output Document"
        );
    }
    assert_eq!(file_state(&source), source_before);
    assert_eq!(file_state(&output), output_before);

    close_document(document.document_id).expect("close document");
}

#[test]
fn a_fresh_scan_finds_the_snapshot_after_an_abnormal_exit() {
    let _serial = serial();
    let dir = TempDir::new("recovery-crash");
    let source = copy_fixture("text", &dir, "source.pdf");
    let app = dir.path().join("appdata");
    let document = open(&source);
    create_box(&document.document_id, "Survives an abnormal exit");

    let store = RecoveryStore::new(&app);
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 0)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_in(&store, &document.document_id, 2_000).expect("tick");
    assert!(written.written);
    let session = written.session_id.expect("session id");

    // Simulate a restart: a brand-new store scans the same app data directory
    // while the crashed process never ran its close cleanup.
    drop(store);
    let fresh = RecoveryStore::new(&app);
    let scan = fresh.scan().expect("scan after restart");
    assert_eq!(scan.discarded, 0);
    assert!(
        scan.candidates
            .iter()
            .any(|candidate| candidate.session_id == session),
        "the new process finds the unsaved edits"
    );
    let restored = fresh
        .read_snapshot(&session)
        .expect("read snapshot")
        .expect("snapshot bytes");
    assert!(restored.bytes.starts_with(b"%PDF"));

    // The crashed process is gone; this only releases test resources.
    close_document(document.document_id).expect("close document");
}

#[test]
fn restoring_opens_unsaved_edits_and_saves_through_the_normal_pipeline() {
    let _serial = serial();
    let _app = app_data();
    let store = global_store();
    let dir = TempDir::new("recovery-restore");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");

    let document = open(&source);
    create_box(&document.document_id, "Saved before the crash");
    save_as(&document.document_id, &output);

    let source_before = file_state(&source);
    create_box(&document.document_id, "RECOVERED UNSAVED EDIT");
    assert!(
        !maybe_snapshot_at(&document.document_id, 0)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_at(&document.document_id, 2_000).expect("tick");
    assert!(written.written, "the unsaved edit is snapshotted");
    let session = written.session_id.expect("session id");

    let candidates = recovery_candidates().expect("list recovery candidates");
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.session_id == session),
        "startup sees the recovery candidate without opening it"
    );

    let restored = restore_recovery(session.clone()).expect("restore recovery file");
    assert!(restored.recovered);
    assert_eq!(
        restored.path,
        path_string(&source),
        "Source path is provenance"
    );
    assert_eq!(restored.name, "source.pdf");
    assert_eq!(
        restored.output_path.as_deref(),
        Some(path_string(&output).as_str()),
        "the previous Output Document is reused while it still exists"
    );
    let history = edit_history(restored.document_id.clone()).expect("history");
    assert!(history.dirty, "restored unsaved edits start dirty");
    assert!(!history.can_undo, "undo state cannot cross a process");
    assert!(
        extract_text(restored.document_id.clone(), 0)
            .expect("extract restored text")
            .text
            .contains("RECOVERED UNSAVED EDIT"),
        "the restored document contains the unsaved edit"
    );
    assert_eq!(file_state(&source), source_before);

    // Save Changes uses the single save path and the remembered Output target.
    let mut progress = Vec::new();
    let result = save_document_with_progress(
        &restored.document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |update| progress.push(update),
    )
    .expect("save the restored document");
    assert_eq!(result.output_path, path_string(&output));
    assert!(result.bytes_written > 0);
    assert!(!progress.is_empty());

    assert!(
        store
            .read_snapshot(&session)
            .expect("read snapshot")
            .is_none(),
        "a successful save removes the Recovery File"
    );

    let reopened = open(&output);
    assert!(
        extract_text(reopened.document_id.clone(), 0)
            .expect("extract output text")
            .text
            .contains("RECOVERED UNSAVED EDIT"),
        "the edit is in the saved Output Document"
    );
    assert_eq!(
        file_state(&source),
        source_before,
        "the Source Document is untouched by restore and save"
    );

    close_document(reopened.document_id).expect("close reopened");

    // A restored Document keeps its session file name. A new unsaved edit can
    // be snapshotted again, and the recovery prompt's discard action removes
    // it explicitly.
    create_box(&restored.document_id, "Discard this snapshot");
    assert!(
        !maybe_snapshot_at(&restored.document_id, 10_000)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_at(&restored.document_id, 12_000).expect("tick");
    assert!(written.written);
    assert_eq!(
        written.session_id.as_deref(),
        Some(session.as_str()),
        "a restored Document continues its recovery session"
    );
    discard_recovery(session.clone()).expect("discard recovery file");
    assert!(
        store
            .read_snapshot(&session)
            .expect("read snapshot")
            .is_none(),
        "discard removes the Recovery File without opening anything"
    );

    close_document(restored.document_id).expect("close restored");
    close_document(document.document_id).expect("close original");
}

#[test]
fn normal_save_and_close_remove_the_recovery_file() {
    let _serial = serial();
    let dir = TempDir::new("recovery-cleanup");
    let source = copy_fixture("text", &dir, "source.pdf");
    let output = dir.path().join("output.pdf");
    let store = RecoveryStore::new(dir.path().join("appdata"));
    let document = open(&source);

    create_box(&document.document_id, "First unsaved edit");
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 0)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_in(&store, &document.document_id, 2_000).expect("tick");
    let session = written.session_id.expect("session id");
    assert!(written.written);
    assert_eq!(store.scan().expect("scan").candidates.len(), 1);

    save_as(&document.document_id, &output);
    assert!(
        store
            .read_snapshot(&session)
            .expect("read snapshot")
            .is_none(),
        "saving makes the Recovery File stale"
    );
    assert!(store.scan().expect("scan").candidates.is_empty());

    let source_before = file_state(&source);
    create_box(&document.document_id, "Second unsaved edit");
    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 10_000)
            .expect("tick")
            .written
    );
    let written = maybe_snapshot_in(&store, &document.document_id, 12_000).expect("tick");
    assert!(written.written);
    assert_eq!(store.scan().expect("scan").candidates.len(), 1);

    close_document(document.document_id).expect("close document");
    assert!(
        store.scan().expect("scan").candidates.is_empty(),
        "a normal close discards the Recovery File"
    );
    assert_eq!(
        file_state(&source),
        source_before,
        "closing a document never writes to the Source Document"
    );
}

#[test]
fn metadata_and_logs_never_contain_document_text_or_passwords() {
    let _serial = serial();
    let logs = log_directory();
    let dir = TempDir::new("recovery-privacy");
    let source = copy_fixture("text", &dir, "source.pdf");
    let store = RecoveryStore::new(dir.path().join("appdata"));
    let secret = "TOP-SECRET-RECOVERY-CONTENT-15";
    let document = open(&source);
    create_box(&document.document_id, secret);

    assert!(
        !maybe_snapshot_in(&store, &document.document_id, 0)
            .expect("tick")
            .written
    );
    assert!(
        maybe_snapshot_in(&store, &document.document_id, 2_000)
            .expect("tick")
            .written
    );

    let mut metadata = String::new();
    for entry in fs::read_dir(store.directory()).expect("list recovery directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            metadata.push_str(&fs::read_to_string(&path).expect("read metadata"));
        }
    }
    assert!(!metadata.is_empty(), "the sidecar metadata was written");
    assert!(
        !metadata.contains(secret),
        "recovery metadata must never contain document text"
    );
    assert!(
        !metadata.to_lowercase().contains("password"),
        "recovery metadata must never contain a password"
    );

    let mut logs_text = String::new();
    for entry in fs::read_dir(logs).expect("list log directory") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            logs_text.push_str(&fs::read_to_string(&path).unwrap_or_default());
        }
    }
    assert!(
        !logs_text.contains(secret),
        "the local log must never contain document text"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn recent_files_persist_and_missing_entries_are_removed() {
    let _serial = serial();
    let _app = app_data();
    let dir = TempDir::new("recent-files");
    let first = dir.path().join("first.pdf");
    fs::write(&first, b"%PDF-1.7 first").expect("write first");

    record_recent_document(&first);
    let listed = list_recent_files().expect("list recent files");
    assert!(
        listed.iter().any(|entry| entry.path == path_string(&first)),
        "a successfully opened file enters the recent-files list"
    );

    // A new store instance over the same app data directory is the restart
    // seam: the list is read back from disk, not from memory.
    let after_restart = RecentFilesStore::new(app_data()).list();
    assert!(
        after_restart
            .iter()
            .any(|entry| entry.path == path_string(&first)),
        "recent files survive a manager restart"
    );
    assert!(
        after_restart.len() <= pdfx_lib::recovery::MAX_RECENT_FILES,
        "the recent-files list is capped"
    );

    let missing = dir.path().join("missing.pdf");
    fs::write(&missing, b"%PDF-1.7 missing").expect("write missing");
    record_recent_document(&missing);
    fs::remove_file(&missing).expect("remove missing file");
    let listed = list_recent_files().expect("list recent files");
    assert!(
        !listed
            .iter()
            .any(|entry| entry.path == path_string(&missing)),
        "a missing recent file is removed from the list"
    );
    let raw = fs::read_to_string(app_data().join("recent-files.json")).expect("persisted list");
    assert!(
        !raw.contains("missing.pdf"),
        "the missing entry is also removed from the persisted file"
    );

    record_recent_document(&first);
    let listed = forget_recent_file(path_string(&first)).expect("forget recent file");
    assert!(
        !listed.iter().any(|entry| entry.path == path_string(&first)),
        "forget removes the entry"
    );
}
