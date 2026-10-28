//! Command-boundary tests for PDF attachments.
//!
//! Attachments are read through the same PDFium document the UI sees, but the
//! saved structure is asserted with an independent lopdf parse so "the file
//! really contains an `/EmbeddedFiles` entry" is checked against fresh bytes.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pdfx_lib::attachments::{
    add_attachment, delete_attachment, list_attachments, open_attachment_with, save_attachment_as,
    AttachmentView,
};
use pdfx_lib::commands::{close_document, open_document, CommandErrorCode, DocumentView};
use pdfx_lib::flatten_export::export_flattened_copy;
use pdfx_lib::save::{save_document_with_progress, SaveDocumentRequest};
use pdfx_lib::text_edit::{redo_text_edit, undo_text_edit};
use support::fixtures;

use support::synthetic;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory removed with the test.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pdfx-attachments-{}-{unique}-{tag}",
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

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open(path: &Path) -> DocumentView {
    open_document(path_string(path), None).expect("open document")
}

fn fixture_id_path(id: &str) -> PathBuf {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture id {id}"));
    fixtures::fixture_path(fixture)
}

fn names(views: &[AttachmentView]) -> Vec<&str> {
    views.iter().map(|view| view.name.as_str()).collect()
}

fn by_name<'a>(views: &'a [AttachmentView], name: &str) -> &'a AttachmentView {
    views
        .iter()
        .find(|view| view.name == name)
        .unwrap_or_else(|| panic!("attachment {name} missing from {:?}", names(views)))
}

#[test]
fn existing_attachments_are_listed_with_name_size_and_type() {
    let fixture = synthetic::embedded_files_pdf();
    let document = open(Path::new(&fixture.path_string()));
    let document_id = document.document_id.clone();

    let attachments = list_attachments(document_id.clone()).expect("list attachments");
    assert_eq!(names(&attachments), vec!["a.txt", "报告.pdf"]);

    let a_txt = by_name(&attachments, "a.txt");
    assert_eq!(a_txt.size, 17);
    assert_eq!(a_txt.media_type, "text/plain");

    let report = by_name(&attachments, "报告.pdf");
    assert_eq!(report.size, 25);
    assert_eq!(report.media_type, "application/pdf");

    assert_ne!(a_txt.id, report.id);
    let again = list_attachments(document_id.clone()).expect("list attachments again");
    assert_eq!(
        again.iter().map(|view| view.id.clone()).collect::<Vec<_>>(),
        attachments
            .iter()
            .map(|view| view.id.clone())
            .collect::<Vec<_>>(),
        "ids must be stable for the lifetime of an open document"
    );

    close_document(document_id).expect("close document");
}

#[test]
fn added_attachment_is_written_to_the_saved_pdf_and_extractable() {
    let dir = TempDir::new("add-save");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let added_bytes = vec![0u8, 1, 2, 3, 0xFF, b'\n'];
    let added_path = dir.path().join("binary-data.bin");
    fs::write(&added_path, &added_bytes).expect("write attachment source");

    let document = open(Path::new(&fixture.path_string()));
    let result = add_attachment(document.document_id.clone(), path_string(&added_path))
        .expect("add attachment");

    assert_eq!(
        names(&result.attachments),
        vec!["a.txt", "报告.pdf", "binary-data.bin"]
    );
    let added = by_name(&result.attachments, "binary-data.bin");
    assert_eq!(added.size, added_bytes.len() as u64);
    assert_eq!(added.media_type, "application/octet-stream");
    assert!(
        result.history.dirty,
        "adding an attachment dirties the document"
    );

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save the attachment");

    let saved = lopdf::Document::load(&output).expect("parse the saved PDF");
    let entries = embedded_entries(&saved);
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "binary-data.bin", "报告.pdf"],
        "existing attachments and the added one share the /EmbeddedFiles tree"
    );
    let extracted = entries
        .iter()
        .find(|(name, _)| name == "binary-data.bin")
        .expect("added attachment in the name tree")
        .1
        .clone();
    assert_eq!(
        extracted, added_bytes,
        "attachment bytes must be byte-identical"
    );

    close_document(document.document_id).expect("close document");
}

#[test]
fn saved_attachment_is_listed_after_reopening_the_output() {
    let dir = TempDir::new("reopen");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let added_path = dir.path().join("meeting-notes.txt");
    let added_bytes = b"first line\nsecond line\n";
    fs::write(&added_path, added_bytes).expect("write attachment source");

    let document = open(Path::new(&fixture.path_string()));
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add attachment");
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save");
    close_document(document.document_id).expect("close source");

    let reopened = open(&output);
    let attachments = list_attachments(reopened.document_id.clone()).expect("list reopened");
    assert_eq!(
        names(&attachments),
        vec!["a.txt", "meeting-notes.txt", "报告.pdf"]
    );
    let added = by_name(&attachments, "meeting-notes.txt");
    assert_eq!(added.size, added_bytes.len() as u64);
    assert_eq!(added.media_type, "text/plain");

    close_document(reopened.document_id).expect("close output");
}

#[test]
fn save_attachment_as_extracts_exact_bytes_before_and_after_reopening() {
    let dir = TempDir::new("extract");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let added_bytes = vec![0u8, 9, 8, 7, 0xFF];
    let added_path = dir.path().join("payload.bin");
    fs::write(&added_path, &added_bytes).expect("write attachment source");

    let document = open(Path::new(&fixture.path_string()));
    let result = add_attachment(document.document_id.clone(), path_string(&added_path))
        .expect("add attachment");
    let added_id = by_name(&result.attachments, "payload.bin").id;

    let before_save = dir.path().join("before-save.bin");
    let saved = save_attachment_as(
        document.document_id.clone(),
        added_id,
        path_string(&before_save),
    )
    .expect("extract the session attachment");
    assert_eq!(saved.bytes_written, added_bytes.len() as u64);
    assert_eq!(
        fs::read(&before_save).expect("read extraction"),
        added_bytes
    );

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save");
    close_document(document.document_id).expect("close source");

    let reopened = open(&output);
    let attachments = list_attachments(reopened.document_id.clone()).expect("list reopened");
    for (name, expected) in [
        ("a.txt", b"hello attachment\n".as_slice()),
        ("payload.bin", added_bytes.as_slice()),
        ("报告.pdf", b"%PDF-1.4 fake attachment\n".as_slice()),
    ] {
        let view = by_name(&attachments, name);
        let destination = dir.path().join(format!("copy-{name}"));
        let saved = save_attachment_as(
            reopened.document_id.clone(),
            view.id,
            path_string(&destination),
        )
        .unwrap_or_else(|error| panic!("extract {name}: {error:?}"));
        assert_eq!(saved.bytes_written, expected.len() as u64);
        assert_eq!(fs::read(&destination).expect("read extraction"), expected);
    }

    close_document(reopened.document_id).expect("close output");
}

#[test]
fn save_attachment_as_refuses_the_source_and_output_documents() {
    let dir = TempDir::new("extract-guard");
    let fixture = synthetic::embedded_files_pdf();
    let source = Path::new(&fixture.path_string()).to_path_buf();
    let output = dir.path().join("output.pdf");
    let source_bytes = fs::read(&source).expect("read source before");

    let document = open(&source);
    let attachments = list_attachments(document.document_id.clone()).expect("list");
    let a_txt_id = by_name(&attachments, "a.txt").id;
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save as first");
    let output_bytes = fs::read(&output).expect("read output before");

    let source_error =
        save_attachment_as(document.document_id.clone(), a_txt_id, path_string(&source))
            .expect_err("the Source Document must be refused");
    assert_eq!(
        source_error.code,
        CommandErrorCode::AttachmentTargetIsSource
    );
    assert_eq!(fs::read(&source).expect("read source after"), source_bytes);

    let output_error =
        save_attachment_as(document.document_id.clone(), a_txt_id, path_string(&output))
            .expect_err("the Output Document must be refused");
    assert_eq!(
        output_error.code,
        CommandErrorCode::AttachmentTargetIsOutput
    );
    assert_eq!(fs::read(&output).expect("read output after"), output_bytes);

    close_document(document.document_id).expect("close");
}

#[test]
fn deleting_an_attachment_updates_the_saved_tree_and_undo_restores_it() {
    let dir = TempDir::new("delete-undo");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");

    let document = open(Path::new(&fixture.path_string()));
    let original = list_attachments(document.document_id.clone()).expect("list");
    let a_txt_id = by_name(&original, "a.txt").id;

    let deleted =
        delete_attachment(document.document_id.clone(), a_txt_id).expect("delete attachment");
    assert_eq!(names(&deleted.attachments), vec!["报告.pdf"]);
    assert!(deleted.history.dirty);

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save the deletion");
    let saved = lopdf::Document::load(&output).expect("parse saved deletion");
    assert_eq!(
        embedded_entries(&saved)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["报告.pdf"]
    );

    let step = undo_text_edit(document.document_id.clone()).expect("undo deletion");
    assert!(step.history.can_redo);
    assert_eq!(
        list_attachments(document.document_id.clone())
            .expect("list after undo")
            .iter()
            .map(|view| (view.id, view.name.clone()))
            .collect::<Vec<_>>(),
        original
            .iter()
            .map(|view| (view.id, view.name.clone()))
            .collect::<Vec<_>>(),
        "undo must restore ids and order exactly"
    );

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save the undo");
    let saved = lopdf::Document::load(&output).expect("parse saved undo");
    assert_eq!(
        embedded_entries(&saved)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "报告.pdf"]
    );

    let step = redo_text_edit(document.document_id.clone()).expect("redo deletion");
    assert!(step.history.dirty);
    assert_eq!(
        names(&list_attachments(document.document_id.clone()).expect("list after redo")),
        vec!["报告.pdf"]
    );

    close_document(document.document_id).expect("close");
}

#[test]
fn undo_and_redo_cover_adding_an_attachment() {
    let dir = TempDir::new("add-undo");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let added_path = dir.path().join("undo-me.txt");
    fs::write(&added_path, b"undo me\n").expect("write attachment source");

    let document = open(Path::new(&fixture.path_string()));
    let baseline = list_attachments(document.document_id.clone()).expect("list baseline");
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add attachment");

    let step = undo_text_edit(document.document_id.clone()).expect("undo add");
    assert!(
        !step.history.dirty,
        "undoing back to the saved state is clean"
    );
    assert_eq!(
        list_attachments(document.document_id.clone()).expect("list after undo"),
        baseline
    );

    // The undone insertion must not leak into a save.
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save after undo");
    let saved = lopdf::Document::load(&output).expect("parse after undo");
    assert_eq!(
        embedded_entries(&saved)
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "报告.pdf"]
    );

    let step = redo_text_edit(document.document_id.clone()).expect("redo add");
    assert!(step.history.dirty);
    assert_eq!(
        names(&list_attachments(document.document_id.clone()).expect("list after redo")),
        vec!["a.txt", "报告.pdf", "undo-me.txt"]
    );
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save after redo");
    let saved = lopdf::Document::load(&output).expect("parse after redo");
    let entries = embedded_entries(&saved);
    let (_, bytes) = entries
        .iter()
        .find(|(name, _)| name == "undo-me.txt")
        .expect("redone attachment in the saved tree");
    assert_eq!(bytes.as_slice(), b"undo me\n");

    close_document(document.document_id).expect("close");
}

#[test]
fn opening_an_attachment_requires_confirmation_and_stages_safe_bytes() {
    let dir = TempDir::new("open-confirm");
    let fixture = synthetic::embedded_files_pdf();
    let staging = dir.path().join("staging");
    let hostile_name = "..\\..\\owned.txt";
    let hostile_path = dir.path().join(hostile_name);
    let bytes = b"staged attachment bytes\n";
    fs::write(&hostile_path, bytes).expect("write hostile-named attachment");

    let document = open(Path::new(&fixture.path_string()));
    let result = add_attachment(document.document_id.clone(), path_string(&hostile_path))
        .expect("add attachment");
    let attachment_id = by_name(&result.attachments, "owned.txt").id;

    let error = open_attachment_with(
        &document.document_id,
        attachment_id,
        false,
        &staging,
        |_| panic!("an unconfirmed attachment must never launch"),
    )
    .expect_err("opening without confirmation must be refused");
    assert_eq!(
        error.code,
        CommandErrorCode::AttachmentOpenConfirmationRequired
    );
    assert!(
        !staging.exists(),
        "nothing may be extracted before the user confirms"
    );

    let launched = std::cell::Cell::new(None::<PathBuf>);
    let staged = open_attachment_with(
        &document.document_id,
        attachment_id,
        true,
        &staging,
        |path| {
            launched.set(Some(path.to_path_buf()));
            Ok(())
        },
    )
    .expect("open the confirmed attachment");

    assert_eq!(staged, staging.join("owned.txt"));
    assert_eq!(staged.parent(), Some(staging.as_path()));
    assert_eq!(fs::read(&staged).expect("read staged bytes"), bytes);
    assert_eq!(launched.into_inner(), Some(staged));

    close_document(document.document_id).expect("close");
}

#[test]
fn hostile_attachment_names_cannot_escape_or_corrupt_the_saved_pdf() {
    let dir = TempDir::new("hostile-names");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");

    let cases = [
        ("..\\..\\escape.txt", "escape.txt", b"escape".as_slice()),
        ("bad\x01name.txt", "bad_name.txt", b"control".as_slice()),
        ("CON.txt", "_CON.txt", b"reserved".as_slice()),
    ];
    let document = open(Path::new(&fixture.path_string()));
    for (raw, _, bytes) in cases {
        let path = dir.path().join(raw);
        fs::write(&path, bytes).expect("write hostile-named attachment");
        add_attachment(document.document_id.clone(), path_string(&path))
            .unwrap_or_else(|error| panic!("add {raw:?}: {error:?}"));
    }

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save hostile names");

    let saved = lopdf::Document::load(&output).expect("parse hostile-name save");
    let entries = embedded_entries(&saved);
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "_CON.txt",
            "a.txt",
            "bad_name.txt",
            "escape.txt",
            "报告.pdf"
        ]
    );
    for (_, sanitized, bytes) in cases {
        let entry = entries
            .iter()
            .find(|(name, _)| name == sanitized)
            .expect("sanitized attachment in the saved tree");
        assert_eq!(entry.1.as_slice(), bytes);
        assert!(!entry.0.contains('\\'), "name kept a path separator");
        assert!(
            !entry.0.chars().any(char::is_control),
            "name kept a control character"
        );
    }

    close_document(document.document_id).expect("close");
}

#[test]
fn duplicate_attachment_names_get_a_stable_suffix() {
    let dir = TempDir::new("duplicates");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let first_dir = dir.path().join("one");
    let second_dir = dir.path().join("two");
    fs::create_dir_all(&first_dir).expect("create first dir");
    fs::create_dir_all(&second_dir).expect("create second dir");
    let first = first_dir.join("report.txt");
    let second = second_dir.join("report.txt");
    fs::write(&first, b"first report\n").expect("write first");
    fs::write(&second, b"second report\n").expect("write second");
    let same_as_existing = dir.path().join("a.txt");
    fs::write(&same_as_existing, b"not the embedded one\n").expect("write a.txt");

    let document = open(Path::new(&fixture.path_string()));
    add_attachment(document.document_id.clone(), path_string(&first)).expect("add first");
    let result =
        add_attachment(document.document_id.clone(), path_string(&second)).expect("add second");
    assert_eq!(
        names(&result.attachments),
        vec!["a.txt", "报告.pdf", "report.txt", "report (2).txt"]
    );
    let result = add_attachment(document.document_id.clone(), path_string(&same_as_existing))
        .expect("add a.txt");
    assert_eq!(
        names(&result.attachments),
        vec![
            "a.txt",
            "报告.pdf",
            "report.txt",
            "report (2).txt",
            "a (2).txt"
        ]
    );

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save duplicates");
    let saved = lopdf::Document::load(&output).expect("parse duplicates");
    let entries = embedded_entries(&saved);
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "a (2).txt",
            "a.txt",
            "report (2).txt",
            "report.txt",
            "报告.pdf"
        ],
        "each duplicate is stored under a distinct name-tree key"
    );
    for (name, bytes) in [
        ("report.txt", b"first report\n".as_slice()),
        ("report (2).txt", b"second report\n".as_slice()),
        ("a (2).txt", b"not the embedded one\n".as_slice()),
        ("a.txt", b"hello attachment\n".as_slice()),
    ] {
        let entry = entries
            .iter()
            .find(|(entry_name, _)| entry_name == name)
            .unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(entry.1.as_slice(), bytes, "bytes under {name}");
    }

    close_document(document.document_id).expect("close");
}

#[test]
fn repeated_saves_stay_stable_and_leave_the_source_untouched() {
    let dir = TempDir::new("repeat");
    let fixture = synthetic::embedded_files_pdf();
    let source = PathBuf::from(fixture.path_string());
    let source_bytes = fs::read(&source).expect("read source before");
    let source_mtime = fs::metadata(&source)
        .expect("source metadata")
        .modified()
        .expect("source mtime");
    let output = dir.path().join("output.pdf");
    let added_path = dir.path().join("stable.bin");
    fs::write(&added_path, b"stable bytes").expect("write attachment source");

    let document = open(&source);
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add");
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("first save");
    let first = embedded_entries(&lopdf::Document::load(&output).expect("parse first save"));

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: None,
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("second save");
    let second = embedded_entries(&lopdf::Document::load(&output).expect("parse second save"));

    assert_eq!(
        first, second,
        "repeat saves must not duplicate or change entries"
    );
    assert_eq!(first.len(), 3);
    assert_eq!(
        fs::read(&source).expect("read source after"),
        source_bytes,
        "the Source Document is never written"
    );
    assert_eq!(
        fs::metadata(&source)
            .expect("source metadata after")
            .modified()
            .expect("source mtime after"),
        source_mtime
    );

    close_document(document.document_id).expect("close");
}

#[test]
fn existing_corpus_attachment_survives_saving_an_added_attachment() {
    let dir = TempDir::new("corpus-existing");
    let source = fixture_id_path("structure-metadata");
    let output = dir.path().join("output.pdf");
    let source_document = lopdf::Document::load(&source).expect("parse the structure fixture");
    let source_entries = embedded_entries(&source_document);
    let notes = source_entries
        .iter()
        .find(|(name, _)| name == "notes.txt")
        .expect("the fixture has notes.txt")
        .1
        .clone();

    let added_path = dir.path().join("added.md");
    fs::write(&added_path, b"# added\n").expect("write added file");
    let document = open(&source);
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add");
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save");

    let saved = lopdf::Document::load(&output).expect("parse saved output");
    let entries = embedded_entries(&saved);
    let saved_notes = entries
        .iter()
        .find(|(name, _)| name == "notes.txt")
        .expect("the existing attachment is still in the name tree");
    assert_eq!(
        saved_notes.1, notes,
        "existing attachment bytes are unchanged"
    );
    assert!(entries.iter().any(|(name, _)| name == "added.md"));

    close_document(document.document_id).expect("close");
}

#[test]
fn unrelated_name_trees_survive_attachment_saves() {
    let dir = TempDir::new("dests");
    let fixture = synthetic::embedded_files_pdf();
    let output = dir.path().join("output.pdf");
    let added_path = dir.path().join("tree.txt");
    fs::write(&added_path, b"tree").expect("write attachment");

    let source = lopdf::Document::load(Path::new(&fixture.path_string())).expect("parse source");
    assert_eq!(name_tree_keys(&source, b"Dests"), vec!["chapter-one"]);

    let document = open(Path::new(&fixture.path_string()));
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add");
    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect("save");

    let saved = lopdf::Document::load(&output).expect("parse saved");
    assert_eq!(
        name_tree_keys(&saved, b"Dests"),
        vec!["chapter-one"],
        "an unrelated /Dests name tree must not be replaced"
    );

    close_document(document.document_id).expect("close");
}

#[test]
fn flattened_copy_keeps_attachments() {
    let dir = TempDir::new("flatten-attachments");
    let fixture = synthetic::embedded_files_pdf();
    let destination = dir.path().join("flattened.pdf");
    let added_bytes = b"flatten keeps me";
    let added_path = dir.path().join("flatten.bin");
    fs::write(&added_path, added_bytes).expect("write attachment");

    let document = open(Path::new(&fixture.path_string()));
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add");
    export_flattened_copy(
        document.document_id.clone(),
        path_string(&destination),
        false,
    )
    .expect("export flattened copy");

    let flattened = lopdf::Document::load(&destination).expect("parse flattened copy");
    let entries = embedded_entries(&flattened);
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "flatten.bin", "报告.pdf"]
    );
    let added = entries
        .iter()
        .find(|(name, _)| name == "flatten.bin")
        .expect("added attachment in the flattened copy");
    assert_eq!(added.1.as_slice(), added_bytes);

    close_document(document.document_id).expect("close");
}

#[test]
fn read_only_documents_refuse_attachment_changes() {
    let dir = TempDir::new("read-only-attachments");
    let source = fixture_id_path("encrypted-owner-restrictions");
    let file = dir.path().join("blocked.txt");
    fs::write(&file, b"blocked").expect("write candidate attachment");

    let document = open(&source);
    assert!(document.read_only);
    let add_error = add_attachment(document.document_id.clone(), path_string(&file))
        .expect_err("read-only documents refuse attachment insertion");
    assert_eq!(add_error.code, CommandErrorCode::PermissionDenied);
    let delete_error = delete_attachment(document.document_id.clone(), 1)
        .expect_err("read-only documents refuse attachment deletion");
    assert_eq!(delete_error.code, CommandErrorCode::PermissionDenied);
    assert!(list_attachments(document.document_id.clone())
        .expect("list")
        .is_empty());

    close_document(document.document_id).expect("close");
}

#[test]
fn encrypted_source_save_as_requires_unencrypted_confirmation_and_keeps_attachments() {
    let dir = TempDir::new("encrypted-attachment");
    let source = fixture_id_path("encrypted-user-password");
    let source_bytes = fs::read(&source).expect("read encrypted source before");
    let output = dir.path().join("unencrypted.pdf");
    let added_path = dir.path().join("secret.txt");
    let added_bytes = b"portable attachment\n";
    fs::write(&added_path, added_bytes).expect("write attachment");

    let document = open_document(path_string(&source), Some("pdfx-user".to_owned()))
        .expect("open with the user password");
    assert!(document.encrypted);
    assert!(!document.read_only);
    add_attachment(document.document_id.clone(), path_string(&added_path)).expect("add");

    let error = save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: false,
        },
        &mut |_| {},
    )
    .expect_err("the encrypted source needs explicit confirmation");
    assert_eq!(
        error.code,
        CommandErrorCode::UnencryptedOutputConfirmationRequired
    );
    assert!(!output.exists(), "a refused save writes nothing");

    save_document_with_progress(
        &document.document_id,
        SaveDocumentRequest {
            save_as_path: Some(path_string(&output)),
            confirm_unencrypted_output: true,
        },
        &mut |_| {},
    )
    .expect("save the unencrypted output");
    let saved = lopdf::Document::load(&output).expect("the output is unencrypted and parseable");
    let entries = embedded_entries(&saved);
    let added = entries
        .iter()
        .find(|(name, _)| name == "secret.txt")
        .expect("attachment in the unencrypted output");
    assert_eq!(added.1.as_slice(), added_bytes);
    assert_eq!(
        fs::read(&source).expect("read encrypted source after"),
        source_bytes
    );

    close_document(document.document_id).expect("close");
}

#[test]
fn attachment_file_errors_are_reported_before_touching_the_document() {
    let dir = TempDir::new("file-errors");
    let fixture = synthetic::embedded_files_pdf();
    let document = open(Path::new(&fixture.path_string()));
    let baseline = list_attachments(document.document_id.clone()).expect("list baseline");

    let missing = add_attachment(
        document.document_id.clone(),
        path_string(&dir.path().join("does-not-exist.txt")),
    )
    .expect_err("a missing file is refused");
    assert_eq!(missing.code, CommandErrorCode::FileNotFound);

    let directory = add_attachment(document.document_id.clone(), path_string(dir.path()))
        .expect_err("a directory is refused");
    assert_eq!(directory.code, CommandErrorCode::FileReadFailed);

    let huge = dir.path().join("huge.bin");
    let file = fs::File::create(&huge).expect("create sparse file");
    file.set_len(100 * 1024 * 1024 + 1)
        .expect("size sparse file");
    drop(file);
    let too_large = add_attachment(document.document_id.clone(), path_string(&huge))
        .expect_err("an absurdly large file is refused");
    assert_eq!(too_large.code, CommandErrorCode::AttachmentTooLarge);

    assert_eq!(
        list_attachments(document.document_id.clone()).expect("list after failures"),
        baseline,
        "failed additions leave the attachment list untouched"
    );

    close_document(document.document_id).expect("close");
}

/// The decoded keys of one catalog name tree.
fn name_tree_keys(document: &lopdf::Document, tree: &[u8]) -> Vec<String> {
    let catalog = document.catalog().expect("catalog");
    let names = resolve_dictionary(document, catalog.get(b"Names").expect("catalog /Names"));
    let root = names.get(tree).expect("catalog name tree");
    let mut keys = Vec::new();
    collect_name_keys(document, root, &mut keys, 0);
    let mut keys: Vec<String> = keys
        .iter()
        .map(|key| decode_pdf_text(&key_bytes(key)))
        .collect();
    keys.sort();
    keys
}

fn collect_name_keys(
    document: &lopdf::Document,
    node: &lopdf::Object,
    keys: &mut Vec<lopdf::Object>,
    depth: usize,
) {
    assert!(depth < 32, "name tree cycle or runaway nesting");
    let dictionary = resolve_dictionary(document, node);
    if let Ok(names) = dictionary.get(b"Names") {
        let array = names.as_array().expect("name tree /Names array");
        for pair in array.chunks_exact(2) {
            keys.push(pair[0].clone());
        }
    }
    if let Ok(kids) = dictionary.get(b"Kids") {
        for kid in kids.as_array().expect("name tree /Kids array") {
            collect_name_keys(document, kid, keys, depth + 1);
        }
    }
}

/// Every `(name, bytes)` pair reachable from `/Names /EmbeddedFiles`,
/// traversing `/Kids` and decoding UTF-16BE keys.
fn embedded_entries(document: &lopdf::Document) -> Vec<(String, Vec<u8>)> {
    let catalog = document.catalog().expect("catalog");
    let names = resolve_dictionary(document, catalog.get(b"Names").expect("catalog /Names"));
    let root = names
        .get(b"EmbeddedFiles")
        .expect("catalog /Names /EmbeddedFiles");
    let mut pairs = Vec::new();
    collect_name_pairs(document, root, &mut pairs, 0);

    pairs.sort_by(|(left, _), (right, _)| key_bytes(left).cmp(&key_bytes(right)));
    pairs
        .into_iter()
        .map(|(key, filespec_id)| {
            let name = decode_pdf_text(&key_bytes(&key));
            let filespec = document
                .get_dictionary(filespec_id)
                .expect("filespec dictionary");
            let embedded = filespec
                .get(b"EF")
                .and_then(lopdf::Object::as_dict)
                .expect("filespec /EF");
            let stream_id = embedded
                .get(b"F")
                .and_then(lopdf::Object::as_reference)
                .expect("embedded file stream reference");
            let stream = document
                .get_object(stream_id)
                .and_then(lopdf::Object::as_stream)
                .expect("embedded file stream");
            let bytes = stream
                .decompressed_content()
                .expect("decompress the embedded file stream");
            (name, bytes)
        })
        .collect()
}

fn resolve_dictionary<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> &'a lopdf::Dictionary {
    match object {
        lopdf::Object::Dictionary(dictionary) => dictionary,
        lopdf::Object::Reference(id) => document
            .get_dictionary(*id)
            .expect("dictionary behind reference"),
        other => panic!("expected a dictionary, got {other:?}"),
    }
}

fn collect_name_pairs(
    document: &lopdf::Document,
    node: &lopdf::Object,
    pairs: &mut Vec<(lopdf::Object, lopdf::ObjectId)>,
    depth: usize,
) {
    assert!(depth < 32, "name tree cycle or runaway nesting");
    let dictionary = resolve_dictionary(document, node);
    if let Ok(names) = dictionary.get(b"Names") {
        let array = names.as_array().expect("name tree /Names array");
        for pair in array.chunks_exact(2) {
            let value = pair[1]
                .as_reference()
                .expect("name tree value is a filespec reference");
            pairs.push((pair[0].clone(), value));
        }
    }
    if let Ok(kids) = dictionary.get(b"Kids") {
        for kid in kids.as_array().expect("name tree /Kids array") {
            collect_name_pairs(document, kid, pairs, depth + 1);
        }
    }
}

fn key_bytes(object: &lopdf::Object) -> Vec<u8> {
    match object {
        lopdf::Object::String(bytes, _) => bytes.clone(),
        other => panic!("name tree key is not a string: {other:?}"),
    }
}

fn decode_pdf_text(bytes: &[u8]) -> String {
    match bytes.strip_prefix(&[0xFE, 0xFF]) {
        Some(utf16) => {
            let units: Vec<u16> = utf16
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}
