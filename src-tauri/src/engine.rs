//! PDFium library loading and document opening.
//!
//! PDFium initializes once per process, so the binding is held in a process-wide
//! `OnceLock`. Commands share that engine; tests exercise the same path.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use pdfium_render::prelude::{PdfDocument, Pdfium, PdfiumError};

/// A bound PDFium library.
pub struct Engine {
    pdfium: Pdfium,
}

static ENGINE: OnceLock<std::result::Result<Engine, String>> = OnceLock::new();

impl Engine {
    /// Returns the process-wide engine, binding PDFium on first use.
    ///
    /// The Tauri setup hook calls [`Engine::initialize`] with app-aware paths
    /// first; tests and direct command calls resolve the vendored library.
    pub fn shared() -> std::result::Result<&'static Engine, String> {
        let result = ENGINE.get_or_init(|| {
            bind_candidates(&candidate_paths(None)).map_err(|error| format!("{error:#}"))
        });
        match result {
            Ok(engine) => Ok(engine),
            Err(message) => Err(message.clone()),
        }
    }

    /// Binds PDFium from the first working candidate path, including paths
    /// derived from the Tauri app bundle when an app handle is available.
    pub fn initialize(app: Option<&tauri::AppHandle>) -> std::result::Result<(), String> {
        let result = ENGINE.get_or_init(|| {
            bind_candidates(&candidate_paths(app)).map_err(|error| format!("{error:#}"))
        });
        match result {
            Ok(_) => Ok(()),
            Err(message) => Err(message.clone()),
        }
    }

    pub fn pdfium(&self) -> &Pdfium {
        &self.pdfium
    }

    /// Opens an owned byte buffer as a PDF.
    ///
    /// PDFium wraps the buffer without copying it, and `pdfium-render` keeps the
    /// `Vec<u8>` alive inside the returned document. The document therefore owns
    /// its input and can outlive this call; `Engine` is a process-wide static.
    pub fn open_bytes(
        &'static self,
        bytes: Vec<u8>,
        password: Option<&str>,
    ) -> std::result::Result<PdfDocument<'static>, PdfiumError> {
        self.pdfium.load_pdf_from_byte_vec(bytes, password)
    }
}

/// Resolves the PDFium library candidates in priority order.
fn candidate_paths(app: Option<&tauri::AppHandle>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(path) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        paths.push(PathBuf::from(path));
    }

    if let Some(app) = app {
        paths.extend(bundled_paths(app));
    }

    paths.push(vendored_path());
    paths
}

fn library_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

fn vendored_path() -> PathBuf {
    let directory = if cfg!(target_os = "windows") {
        "bin"
    } else {
        "lib"
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join("pdfium")
        .join(directory)
        .join(library_file_name())
}

#[cfg(target_os = "macos")]
fn bundled_paths(app: &tauri::AppHandle) -> Vec<PathBuf> {
    use tauri::Manager;

    let mut paths = Vec::new();
    if let Ok(resources) = app.path().resource_dir() {
        // `tauri-build` copies frameworks to `target/<profile>/../Frameworks`
        // in development and to `Contents/Frameworks` in a bundle; `resource_dir`
        // points at the profile directory or `Contents/Resources` respectively.
        if let Some(parent) = resources.parent() {
            paths.push(parent.join("Frameworks").join(library_file_name()));
        }
        paths.push(resources.join(library_file_name()));
    }
    paths
}

#[cfg(not(target_os = "macos"))]
fn bundled_paths(app: &tauri::AppHandle) -> Vec<PathBuf> {
    use tauri::Manager;

    app.path()
        .resource_dir()
        .map(|directory| vec![directory.join(library_file_name())])
        .unwrap_or_default()
}

fn bind_candidates(paths: &[PathBuf]) -> Result<Engine> {
    let mut attempts = Vec::new();

    for path in paths {
        if !path.exists() {
            attempts.push(format!("{} (not found)", path.display()));
            continue;
        }

        match Pdfium::bind_to_library(path) {
            Ok(bindings) => {
                return Ok(Engine {
                    pdfium: Pdfium::new(bindings),
                });
            }
            Err(error) => attempts.push(format!("{} ({error})", path.display())),
        }
    }

    Err(anyhow!(
        "could not load the PDFium native library. Run `npm run setup:native` from the \
         repository root or set PDFIUM_DYNAMIC_LIB_PATH. Tried: {}",
        attempts.join("; ")
    ))
}
