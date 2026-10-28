//! Atomic replacement of exported files.
//!
//! Both image export and flattened-copy export write to a same-directory
//! temporary file first, fsync it and rename it over the target. A full disk,
//! an I/O error or an interrupted process therefore cannot leave a truncated
//! file at the target, and a failure removes the temporary file. The rename is
//! atomic because the temporary file lives on the target's filesystem.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::commands::{CommandError, CommandErrorCode};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `target` atomically.
///
/// The target's directory must already exist. On failure the previous target
/// (if any) is untouched and no temporary file is left behind.
pub(crate) fn write_atomically(target: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    write_atomically_with(target, |file| file.write_all(bytes))
}

/// The write step is a parameter so tests can inject a disk-full failure after
/// partial bytes without a real full filesystem.
fn write_atomically_with<F>(target: &Path, write: F) -> Result<(), CommandError>
where
    F: FnOnce(&mut fs::File) -> std::io::Result<()>,
{
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("{} is not a file path", target.display()),
            )
        })?;

    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = directory.join(format!(
        ".{}.pdfx-export-{}-{unique}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let mut temp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|error| {
            write_failure(
                "could not create the temporary export file",
                &temp_path,
                error,
            )
        })?;

    let write_result = write(&mut temp).and_then(|()| temp.sync_all());
    drop(temp);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(write_failure(
            "could not write the temporary export file",
            &temp_path,
            error,
        ));
    }

    if let Err(error) = fs::rename(&temp_path, target) {
        let _ = fs::remove_file(&temp_path);
        return Err(write_failure(
            "could not replace the export target",
            target,
            error,
        ));
    }

    // Make the rename durable. Directory fsync is not available on Windows and
    // is best-effort elsewhere: the file is already in place.
    #[cfg(unix)]
    if let Ok(directory_handle) = fs::File::open(directory) {
        let _ = directory_handle.sync_all();
    }

    Ok(())
}

fn write_failure(context: &str, path: &Path, error: std::io::Error) -> CommandError {
    let code = match error.kind() {
        std::io::ErrorKind::PermissionDenied => CommandErrorCode::PermissionDenied,
        _ => CommandErrorCode::FileWriteFailed,
    };
    CommandError::new(code, format!("{context} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A private scratch directory for the write mechanism's unit tests.
    fn scratch_dir(label: &str) -> std::path::PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("pdfx-{label}-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    fn temporary_files(directory: &Path) -> Vec<String> {
        fs::read_dir(directory)
            .expect("list the scratch directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".pdfx-export-"))
            .collect()
    }

    fn disk_full() -> std::io::Error {
        std::io::Error::other("no space left on device")
    }

    #[test]
    fn a_failed_write_leaves_no_target_and_no_temporary_file() {
        let dir = scratch_dir("export-atomic-fail");
        let target = dir.join("page.png");

        let error = write_atomically_with(&target, |file| {
            file.write_all(b"only part of the file")?;
            Err(disk_full())
        })
        .expect_err("the injected write failure must be reported");

        assert_eq!(error.code, CommandErrorCode::FileWriteFailed);
        assert!(
            !target.exists(),
            "a failed export must not leave a partial file at the target"
        );
        assert!(
            temporary_files(&dir).is_empty(),
            "a failed export must remove its temporary file"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_overwrite_keeps_the_previous_target_byte_identical() {
        let dir = scratch_dir("export-atomic-overwrite");
        let target = dir.join("page.png");
        fs::write(&target, b"the file that was already there").expect("write the previous file");

        let error = write_atomically_with(&target, |file| {
            file.write_all(b"partial replacement")?;
            Err(disk_full())
        })
        .expect_err("the injected write failure must be reported");

        assert_eq!(error.code, CommandErrorCode::FileWriteFailed);
        assert_eq!(
            fs::read(&target).expect("re-read the previous file"),
            b"the file that was already there",
            "the rename never happened, so the previous file is untouched"
        );
        assert!(
            temporary_files(&dir).is_empty(),
            "a failed export must remove its temporary file"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_successful_write_replaces_the_target_and_leaves_no_temporary_file() {
        let dir = scratch_dir("export-atomic-success");
        let target = dir.join("page.png");

        write_atomically(&target, b"complete file").expect("write the file atomically");

        assert_eq!(
            fs::read(&target).expect("read the written file"),
            b"complete file"
        );
        assert!(
            temporary_files(&dir).is_empty(),
            "the temporary file is renamed into place"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
