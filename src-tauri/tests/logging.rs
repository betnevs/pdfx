//! Tests for local rolling logs and user-initiated diagnostic export.

mod support;

use std::fs;

use pdfx_lib::logging::{build_diagnostic_report, export_diagnostics, Logger};
use support::temp::TempDir;

fn logger_in(dir: &TempDir, max_bytes: u64, max_files: usize) -> Logger {
    Logger::new(dir.path(), max_bytes, max_files).expect("create logger")
}

#[test]
fn log_lines_carry_utc_timestamps_levels_and_events() {
    let dir = TempDir::new("logging-lines");
    let logger = logger_in(&dir, 1_000_000, 3);

    logger
        .log_event("document.open", "path=\"/tmp/a.pdf\" pages=3")
        .expect("log open");
    logger
        .log_error("document.open.failed", "code=CorruptDocument")
        .expect("log failure");

    let contents =
        fs::read_to_string(dir.path().join("pdfx.log")).expect("read the current log file");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2, "each event occupies exactly one line");

    let timestamp = lines[0].split(' ').next().expect("timestamp field");
    assert_eq!(timestamp.len(), 24, "timestamp: {timestamp}");
    assert_eq!(&timestamp[4..5], "-");
    assert_eq!(&timestamp[7..8], "-");
    assert_eq!(&timestamp[10..11], "T");
    assert_eq!(&timestamp[13..14], ":");
    assert_eq!(&timestamp[16..17], ":");
    assert_eq!(&timestamp[19..20], ".");
    assert!(timestamp.ends_with('Z'));
    assert!(timestamp.starts_with("20"), "timestamp: {timestamp}");

    assert!(
        lines[0].contains(" INFO document.open path=\"/tmp/a.pdf\" pages=3"),
        "line: {}",
        lines[0]
    );
    assert!(
        lines[1].contains(" ERROR document.open.failed code=CorruptDocument"),
        "line: {}",
        lines[1]
    );
}

#[test]
fn log_details_are_flattened_to_one_line() {
    let dir = TempDir::new("logging-sanitize");
    let logger = logger_in(&dir, 1_000_000, 3);

    logger
        .log_error("document.save.failed", "first line\nsecond line\rthird")
        .expect("log failure");

    let contents =
        fs::read_to_string(dir.path().join("pdfx.log")).expect("read the current log file");
    assert_eq!(contents.lines().count(), 1);
    assert!(contents.contains("first line\\nsecond line\\rthird"));
}

#[test]
fn logs_roll_over_and_keep_a_bounded_number_of_files() {
    let dir = TempDir::new("logging-rotation");
    let logger = logger_in(&dir, 220, 2);

    for index in 0..40 {
        logger
            .log_event("test.rotate", &format!("entry={index:02} padding=xxxxxxxx"))
            .expect("log entry");
    }

    let files = logger.log_files();
    assert_eq!(files.len(), 3, "two archives plus the current file");
    for path in &files {
        assert!(path.exists(), "missing log file {}", path.display());
    }

    let combined: String = files
        .iter()
        .map(|path| fs::read_to_string(path).expect("read log file"))
        .collect();
    assert!(combined.contains("entry=39"), "newest entry is kept");
    assert!(!combined.contains("entry=00"), "oldest entries are dropped");
}

#[test]
fn the_logger_creates_missing_directories() {
    let dir = TempDir::new("logging-directories");
    let nested = dir.path().join("nested").join("logs");

    let logger = Logger::new(&nested, 10_000, 2).expect("create logger");

    logger.log_event("app.start", "version=0.1.0").expect("log");
    assert!(nested.join("pdfx.log").exists());
}

#[test]
fn a_diagnostic_report_contains_version_platform_and_log_contents() {
    let dir = TempDir::new("logging-report");
    let logger = logger_in(&dir, 1_000_000, 3);
    logger
        .log_event("document.open", "path=\"/tmp/report.pdf\"")
        .expect("log open");

    let report = build_diagnostic_report(Some(&logger), "PDFX", "9.9.9");

    assert!(report.contents.contains("PDFX 9.9.9"));
    assert!(report.contents.contains(std::env::consts::OS));
    assert!(report.contents.contains(std::env::consts::ARCH));
    assert!(report.contents.contains("document.open"));
    assert!(report.contents.contains("pdfx.log"));
    assert_eq!(report.log_files, 1);
}

#[test]
fn a_diagnostic_report_without_logs_is_still_useful() {
    let report = build_diagnostic_report(None, "PDFX", "0.1.0");

    assert!(report.contents.contains("PDFX 0.1.0"));
    assert!(report.contents.contains("no log entries"));
    assert_eq!(report.log_files, 0);
}

#[test]
fn diagnostic_export_writes_a_copy_to_the_chosen_path() {
    let dir = TempDir::new("logging-export");
    let destination = dir.path().join("exports").join("pdfx-diagnostics.txt");

    let export = export_diagnostics(&destination, "PDFX", "0.1.0").expect("export diagnostics");

    assert_eq!(export.path, destination);
    assert!(export.bytes > 0);
    assert_eq!(export.log_files, 0);
    let contents = fs::read_to_string(&export.path).expect("read the exported report");
    assert!(contents.contains("PDFX 0.1.0"));
    assert!(contents.contains(std::env::consts::OS));
}
