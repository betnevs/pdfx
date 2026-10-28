//! Path identity for files an operation must never overwrite.
//!
//! Saving and exporting both refuse a target that names the Source Document or
//! the current Output Document. The comparison has to see through symlinks,
//! relative components and the case-insensitive filesystems macOS and Windows
//! use by default, so the canonicalization lives here instead of in each
//! pipeline.

use std::fs;
use std::path::Path;

/// Returns true when `left` and `right` resolve to the same file, including
/// symlink aliases and case variants on case-insensitive platforms.
///
/// Canonical paths resolve symlinks and `..`. A target that does not exist yet
/// has its parent canonicalized and its file name appended, so `../` cannot
/// hide the file and the usual save-dialog result still compares correctly.
pub(crate) fn same_file(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| -> Option<String> {
        let resolved = match fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(_) => {
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let name = path.file_name()?;
                fs::canonicalize(parent).ok()?.join(name)
            }
        };
        let text = resolved.to_string_lossy().into_owned();
        // macOS and Windows filesystems are case-insensitive by default.
        Some(if cfg!(any(target_os = "macos", target_os = "windows")) {
            text.to_lowercase()
        } else {
            text
        })
    };

    match (normalize(left), normalize(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}
