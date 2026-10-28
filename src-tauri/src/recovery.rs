//! Recovery files and the recent-files list.
//!
//! While a Document has unsaved edits the editor writes a Recovery File: a
//! complete byte snapshot of the live PDFium state (including every unsaved
//! edit, produced by [`crate::save::serialize_live_document`]) plus a small
//! JSON sidecar describing its provenance. Recovery Files live in a dedicated
//! directory under the application data directory — never next to the Source
//! Document — and writes are atomic, so a crash cannot leave a half-written
//! snapshot. A snapshot write refuses any path that resolves to the Source
//! Document or the current Output Document, mirroring the save pipeline's
//! protection.
//!
//! The snapshot is throttled: the first edit is written after a short quiet
//! period (debounce), and continuous editing still produces a snapshot at
//! least every [`SNAPSHOT_MAX_INTERVAL_MS`]. The clock is injected as a
//! millisecond tick so the schedule is deterministic in tests.
//!
//! Nothing here is uploaded. The sidecar deliberately contains no password,
//! document text or telemetry: only paths, a display name, timestamps, the
//! revision counters and the app version. The snapshot PDF itself is a local
//! copy of the document bytes; for an encrypted Source Document it is an
//! unencrypted copy, which is why a restored Document keeps the
//! "output will be unencrypted" warning.
//!
//! Recent files are persisted as `recent-files.json` in the application data
//! directory. The list is deduplicated by path, capped and pruned of files
//! that no longer exist. A normal start-up only reads the list; it never
//! opens a Document automatically.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::commands::{CommandError, CommandErrorCode, DocumentPermissions, OpenDocument};
use crate::logging;

/// Subdirectory of the application data directory holding Recovery Files.
pub const RECOVERY_DIR_NAME: &str = "recovery";
/// File name of the persisted recent-files list inside the app data directory.
pub const RECENT_FILES_FILE_NAME: &str = "recent-files.json";
/// The recent-files list never grows past this many entries.
pub const MAX_RECENT_FILES: usize = 10;

/// How long editing must be quiet before the first snapshot is written.
pub const SNAPSHOT_DEBOUNCE_MS: u64 = 2_000;
/// While edits keep arriving, a snapshot is forced at this interval anyway.
pub const SNAPSHOT_MAX_INTERVAL_MS: u64 = 30_000;

const SNAPSHOT_SUFFIX: &str = ".pdfx-recovery.pdf";
const METADATA_SUFFIX: &str = ".pdfx-recovery.json";
const METADATA_FORMAT_VERSION: u32 = 1;
const MAX_SESSION_ID_CHARS: usize = 128;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The application data directory, installed once during app setup.
static APP_DATA_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static CLOCK_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Installs the application data directory used by both stores.
///
/// Mirrors [`crate::logging::initialize`]: the first call wins, a second call
/// is harmless, and a failure to create the directory is reported to the
/// caller without preventing the app from starting.
pub fn initialize(directory: impl Into<PathBuf>) -> io::Result<()> {
    let directory = directory.into();
    fs::create_dir_all(&directory)?;
    let _ = APP_DATA_DIRECTORY.set(directory);
    Ok(())
}

/// The installed application data directory, when setup has run.
pub fn app_data_directory() -> Option<&'static Path> {
    APP_DATA_DIRECTORY.get().map(PathBuf::as_path)
}

/// The Recovery File store for the installed app data directory.
pub fn recovery_store() -> Option<RecoveryStore> {
    app_data_directory().map(RecoveryStore::new)
}

/// The recent-files store for the installed app data directory.
pub fn recent_files_store() -> Option<RecentFilesStore> {
    app_data_directory().map(RecentFilesStore::new)
}

/// A monotonic millisecond clock for the snapshot throttle.
pub fn now_ticks() -> u64 {
    CLOCK_ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Wall-clock milliseconds since the Unix epoch, used in persisted metadata.
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

#[derive(Debug)]
pub enum RecoveryError {
    Io(io::Error),
    Serialize(serde_json::Error),
    InvalidSessionId(String),
    /// A recovery path resolved to the Source or Output Document.
    TargetConflict(PathBuf),
    Corrupt(String),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "recovery file I/O failed: {error}"),
            Self::Serialize(error) => {
                write!(formatter, "recovery metadata encoding failed: {error}")
            }
            Self::InvalidSessionId(id) => {
                write!(formatter, "invalid recovery session id: {id:?}")
            }
            Self::TargetConflict(path) => write!(
                formatter,
                "recovery file {} would overwrite a Source or Output Document",
                path.display()
            ),
            Self::Corrupt(message) => write!(formatter, "recovery metadata is corrupt: {message}"),
        }
    }
}

impl std::error::Error for RecoveryError {}

impl From<io::Error> for RecoveryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RecoveryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// What one snapshot tick decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotOutcome {
    pub written: bool,
    pub session_id: Option<String>,
    pub bytes: u64,
}

impl SnapshotOutcome {
    fn skipped() -> Self {
        Self {
            written: false,
            session_id: None,
            bytes: 0,
        }
    }
}

/// Debounce + maximum-interval snapshot schedule for one Document.
///
/// `observe` is called on every tick with the document's current revision; a
/// new revision starts (or restarts) the quiet period. `due` becomes true once
/// the revision has been quiet for [`SNAPSHOT_DEBOUNCE_MS`], or once the
/// document has been continuously dirty for [`SNAPSHOT_MAX_INTERVAL_MS`] even
/// if edits never stop. The continuous-dirty cap also applies before the first
/// snapshot, so a user who types without pausing is still protected.
#[derive(Debug, Default)]
pub struct RecoveryThrottle {
    last_seen_revision: Option<u64>,
    /// When the newest pending revision was first observed.
    changed_at: Option<u64>,
    /// When the current dirty period began (cleared by a write or a clean
    /// document).
    dirty_since: Option<u64>,
}

impl RecoveryThrottle {
    /// Records the current revision. Returns true when it differs from the
    /// revision seen before (i.e. an edit happened since the last tick).
    pub fn observe(&mut self, revision: u64, now: u64) -> bool {
        if self.last_seen_revision == Some(revision) {
            return false;
        }
        self.last_seen_revision = Some(revision);
        self.changed_at = Some(now);
        self.dirty_since.get_or_insert(now);
        true
    }

    /// True when a snapshot is owed at `now`.
    pub fn due(&self, now: u64) -> bool {
        let Some(changed_at) = self.changed_at else {
            return false;
        };
        if now.saturating_sub(changed_at) >= SNAPSHOT_DEBOUNCE_MS {
            return true;
        }
        self.dirty_since
            .is_some_and(|since| now.saturating_sub(since) >= SNAPSHOT_MAX_INTERVAL_MS)
    }

    /// Marks a snapshot as written, ending the pending dirty period.
    pub fn mark_written(&mut self, _now: u64) {
        self.changed_at = None;
        self.dirty_since = None;
    }

    /// Forgets pending state (used when a Document becomes clean).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The JSON sidecar of one Recovery File.
///
/// It must never carry document text, passwords or telemetry. `dirty` and the
/// revisions describe the session that produced the snapshot, so a recovery
/// prompt can explain what was lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySnapshot {
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    pub session_id: String,
    /// Original Source Document path. Provenance only; never written.
    pub source_path: PathBuf,
    /// Output Document that was current when the snapshot was written.
    #[serde(default)]
    pub output_path: Option<PathBuf>,
    pub display_name: String,
    /// The Source Document was encrypted; the snapshot itself is unencrypted.
    #[serde(default)]
    pub source_encrypted: bool,
    #[serde(default)]
    pub source_read_only: bool,
    #[serde(default)]
    pub source_permissions: Option<DocumentPermissions>,
    pub dirty: bool,
    pub revision: u64,
    pub saved_revision: u64,
    pub created_at_millis: u64,
    pub app_version: String,
}

fn default_format_version() -> u32 {
    METADATA_FORMAT_VERSION
}

/// A snapshot read back from disk, ready to open as a Document.
#[derive(Debug, Clone)]
pub struct RestoredSnapshot {
    pub snapshot: RecoverySnapshot,
    pub bytes: Vec<u8>,
}

/// The outcome of scanning the recovery directory.
#[derive(Debug, Clone, Default)]
pub struct RecoveryScan {
    /// Valid snapshots, newest first.
    pub candidates: Vec<RecoverySnapshot>,
    /// Orphan or unreadable snapshot pairs removed by the scan.
    pub discarded: usize,
}

/// Layout of the recovery directory inside the application data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStore {
    directory: PathBuf,
}

impl RecoveryStore {
    /// `app_data_directory` is the application data directory, not the
    /// recovery directory itself.
    pub fn new(app_data_directory: impl AsRef<Path>) -> Self {
        Self {
            directory: app_data_directory.as_ref().join(RECOVERY_DIR_NAME),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn snapshot_path(&self, session_id: &str) -> Result<PathBuf, RecoveryError> {
        validate_session_id(session_id)?;
        Ok(self
            .directory
            .join(format!("{session_id}{SNAPSHOT_SUFFIX}")))
    }

    fn metadata_path(&self, session_id: &str) -> Result<PathBuf, RecoveryError> {
        validate_session_id(session_id)?;
        Ok(self
            .directory
            .join(format!("{session_id}{METADATA_SUFFIX}")))
    }

    /// Writes the snapshot PDF and then its metadata atomically.
    ///
    /// A snapshot path that resolves to the Source Document or the Output
    /// Document is refused before anything is created. Both files are staged
    /// and only then renamed into place, so a failure before the renames leaves
    /// the previous snapshot pair untouched. If the final metadata rename fails
    /// on a first write, the just-written PDF is removed rather than left
    /// without provenance.
    pub fn write_snapshot(
        &self,
        snapshot: &RecoverySnapshot,
        pdf_bytes: &[u8],
    ) -> Result<(), RecoveryError> {
        self.write_snapshot_with(snapshot, pdf_bytes, write_bytes_atomically)
    }

    fn write_snapshot_with<F>(
        &self,
        snapshot: &RecoverySnapshot,
        pdf_bytes: &[u8],
        mut write: F,
    ) -> Result<(), RecoveryError>
    where
        F: FnMut(&Path, &[u8]) -> io::Result<()>,
    {
        let pdf_path = self.snapshot_path(&snapshot.session_id)?;
        let metadata_path = self.metadata_path(&snapshot.session_id)?;

        if crate::target_path::same_file(&pdf_path, &snapshot.source_path)
            || crate::target_path::same_file(&metadata_path, &snapshot.source_path)
            || snapshot.output_path.as_deref().is_some_and(|output| {
                crate::target_path::same_file(&pdf_path, output)
                    || crate::target_path::same_file(&metadata_path, output)
            })
        {
            return Err(RecoveryError::TargetConflict(pdf_path));
        }

        fs::create_dir_all(&self.directory)?;

        let first = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let second = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pdf_staging = self.directory.join(format!(
            ".{}.pdf-stage-{}-{first}.tmp",
            snapshot.session_id,
            std::process::id()
        ));
        let metadata_staging = self.directory.join(format!(
            ".{}.json-stage-{}-{second}.tmp",
            snapshot.session_id,
            std::process::id()
        ));
        let metadata_existed = metadata_path.is_file();

        let write_result = (|| -> Result<(), RecoveryError> {
            write(&pdf_staging, pdf_bytes)?;
            let metadata = serde_json::to_vec_pretty(snapshot)?;
            write(&metadata_staging, &metadata)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&pdf_staging);
            let _ = fs::remove_file(&metadata_staging);
            return Err(error);
        }

        if let Err(error) = fs::rename(&pdf_staging, &pdf_path) {
            let _ = fs::remove_file(&pdf_staging);
            let _ = fs::remove_file(&metadata_staging);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&metadata_staging, &metadata_path) {
            let _ = fs::remove_file(&metadata_staging);
            if !metadata_existed {
                let _ = fs::remove_file(&pdf_path);
            }
            return Err(error.into());
        }

        Ok(())
    }

    /// Reads one snapshot back. `Ok(None)` means the pair does not exist (or
    /// its PDF disappeared); a present but unreadable pair is an error.
    pub fn read_snapshot(
        &self,
        session_id: &str,
    ) -> Result<Option<RestoredSnapshot>, RecoveryError> {
        let metadata_path = self.metadata_path(session_id)?;
        if !metadata_path.is_file() {
            return Ok(None);
        }
        let raw = fs::read(&metadata_path)?;
        let snapshot: RecoverySnapshot = serde_json::from_slice(&raw).map_err(|error| {
            RecoveryError::Corrupt(format!("{}: {error}", metadata_path.display()))
        })?;
        if snapshot.session_id != session_id || snapshot.format_version > METADATA_FORMAT_VERSION {
            return Err(RecoveryError::Corrupt(format!(
                "{} does not describe session {session_id}",
                metadata_path.display()
            )));
        }

        let pdf_path = self.snapshot_path(session_id)?;
        match fs::read(&pdf_path) {
            Ok(bytes) => Ok(Some(RestoredSnapshot { snapshot, bytes })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _ = fs::remove_file(&metadata_path);
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Removes one snapshot pair. Missing files are not an error: a successful
    /// save or a normal close may race the throttled writer.
    pub fn remove_snapshot(&self, session_id: &str) -> Result<(), RecoveryError> {
        let pdf_path = self.snapshot_path(session_id)?;
        let metadata_path = self.metadata_path(session_id)?;
        remove_if_exists(&metadata_path)?;
        remove_if_exists(&pdf_path)?;
        Ok(())
    }

    /// Lists valid snapshots and deliberately removes orphan or unreadable
    /// pairs. A pair is valid only when both the PDF and parseable metadata
    /// exist; anything else cannot be restored and would prompt forever.
    pub fn scan(&self) -> Result<RecoveryScan, RecoveryError> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RecoveryScan::default())
            }
            Err(error) => return Err(error.into()),
        };

        let mut metadata_files = Vec::new();
        let mut pdf_files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(session) = name.strip_suffix(METADATA_SUFFIX) {
                metadata_files.push(session.to_owned());
            } else if let Some(session) = name.strip_suffix(SNAPSHOT_SUFFIX) {
                pdf_files.push(session.to_owned());
            }
        }
        metadata_files.sort();
        pdf_files.sort();

        let mut scan = RecoveryScan::default();
        let mut known = Vec::new();
        for session in metadata_files {
            match self.read_snapshot(&session) {
                Ok(Some(restored)) => {
                    known.push(session);
                    scan.candidates.push(restored.snapshot);
                }
                Ok(None) => {
                    // read_snapshot removed the stale metadata; drop the PDF too.
                    if let Ok(path) = self.snapshot_path(&session) {
                        let _ = fs::remove_file(path);
                    }
                    scan.discarded += 1;
                }
                Err(_) => {
                    if let Ok(path) = self.metadata_path(&session) {
                        let _ = fs::remove_file(path);
                    }
                    if let Ok(path) = self.snapshot_path(&session) {
                        let _ = fs::remove_file(path);
                    }
                    scan.discarded += 1;
                }
            }
        }

        for session in pdf_files {
            if !known.contains(&session) {
                if let Ok(path) = self.snapshot_path(&session) {
                    // A broken pair was already removed while visiting its
                    // metadata; count only files this loop actually removed.
                    if fs::remove_file(path).is_ok() {
                        scan.discarded += 1;
                    }
                }
            }
        }

        scan.candidates.sort_by(|left, right| {
            right
                .created_at_millis
                .cmp(&left.created_at_millis)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(scan)
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_session_id(session_id: &str) -> Result<(), RecoveryError> {
    let valid = !session_id.is_empty()
        && session_id.chars().count() <= MAX_SESSION_ID_CHARS
        && session_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        });
    if valid {
        Ok(())
    } else {
        Err(RecoveryError::InvalidSessionId(session_id.to_owned()))
    }
}

/// Atomic file replacement: a sibling temp file, fsync, then rename. A failed
/// write removes the temp file and leaves an existing target untouched.
fn write_bytes_atomically(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file path", target.display()),
        )
    })?;

    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = directory.join(format!(
        ".{}.pdfx-recovery-{}-{unique}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();

    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    if let Err(error) = fs::rename(&temp_path, target) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    // Best-effort directory fsync (not available on Windows).
    #[cfg(unix)]
    if let Ok(directory_handle) = File::open(directory) {
        let _ = directory_handle.sync_all();
    }

    Ok(())
}

/// One entry of the locally persisted recent-files list.
///
/// Only the path, the display name and the last-opened time are stored; no
/// document content or usage telemetry is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub path: String,
    pub name: String,
    pub last_opened_millis: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentFilesFile {
    version: u32,
    files: Vec<RecentFile>,
}

/// The persisted recent-files list in the application data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFilesStore {
    path: PathBuf,
}

impl RecentFilesStore {
    pub fn new(app_data_directory: impl AsRef<Path>) -> Self {
        Self {
            path: app_data_directory.as_ref().join(RECENT_FILES_FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the raw list, keeping only existing files, deduplicating entries
    /// and applying the cap. A missing or corrupt file yields an empty list.
    pub fn list(&self) -> Vec<RecentFile> {
        let (files, changed) = self.sanitized();
        if changed {
            let _ = self.save(&files);
        }
        files
    }

    /// Loads without touching the filesystem. Used by `record` and `forget`.
    fn sanitized(&self) -> (Vec<RecentFile>, bool) {
        let stored = self.load_raw();
        let total = stored.len();
        let mut files: Vec<RecentFile> = Vec::with_capacity(total.min(MAX_RECENT_FILES));
        for entry in stored {
            if !Path::new(&entry.path).is_file() {
                continue;
            }
            if files
                .iter()
                .any(|known| same_path(&known.path, &entry.path))
            {
                continue;
            }
            files.push(entry);
            if files.len() == MAX_RECENT_FILES {
                break;
            }
        }
        let changed = files.len() != total;
        (files, changed)
    }

    fn load_raw(&self) -> Vec<RecentFile> {
        let Ok(raw) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let Ok(parsed) = serde_json::from_str::<RecentFilesFile>(&raw) else {
            return Vec::new();
        };
        if parsed.version > 1 {
            return Vec::new();
        }
        parsed.files
    }

    /// Records a successfully opened or saved path at the front of the list.
    /// Duplicates (including case and symlink aliases) move to the front.
    pub fn record(&self, path: &str, now_millis: u64) -> Result<Vec<RecentFile>, RecoveryError> {
        let (mut files, _) = self.sanitized();
        files.retain(|entry| !same_path(&entry.path, path));
        files.insert(
            0,
            RecentFile {
                path: path.to_owned(),
                name: file_name(path),
                last_opened_millis: now_millis,
            },
        );
        files.truncate(MAX_RECENT_FILES);
        self.save(&files)?;
        Ok(files)
    }

    /// Removes one path from the list, for example after the file went missing.
    pub fn forget(&self, path: &str) -> Result<Vec<RecentFile>, RecoveryError> {
        let (mut files, _) = self.sanitized();
        files.retain(|entry| !same_path(&entry.path, path));
        self.save(&files)?;
        Ok(files)
    }

    fn save(&self, files: &[RecentFile]) -> Result<(), RecoveryError> {
        let directory = self.path.parent().ok_or_else(|| {
            RecoveryError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "recent-files path has no parent directory",
            ))
        })?;
        fs::create_dir_all(directory)?;
        let raw = serde_json::to_vec_pretty(&RecentFilesFile {
            version: 1,
            files: files.to_vec(),
        })?;
        write_bytes_atomically(&self.path, &raw)?;
        Ok(())
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn same_path(left: &str, right: &str) -> bool {
    crate::target_path::same_file(Path::new(left), Path::new(right))
}

/// Records a successful open or save in the local recent-files list. Missing
/// app data (tests, Vite dev) makes this a no-op; a write failure is logged
/// and never blocks the open or save.
pub fn record_recent_document(path: &Path) {
    let Some(store) = recent_files_store() else {
        return;
    };
    let path = path.to_string_lossy().into_owned();
    if let Err(error) = store.record(&path, unix_millis()) {
        logging::error("recovery.recent.failed", &error.to_string());
    }
}

/// Per-Document recovery tracking, kept next to the PDFium document.
///
/// The store travels with the state so a snapshot written for one application
/// data directory is always cleaned up from that same directory, and tests can
/// use an isolated store without touching the process-global one.
pub struct RecoveryState {
    pub(crate) session_id: String,
    pub(crate) throttle: RecoveryThrottle,
    pub(crate) store: RecoveryStore,
}

impl RecoveryState {
    fn fresh(store: &RecoveryStore) -> Self {
        let unique = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            session_id: format!("s{}-{}-{}", std::process::id(), unix_millis(), unique),
            throttle: RecoveryThrottle::default(),
            store: store.clone(),
        }
    }

    /// Creates the recovery state a restored Document continues to use. The
    /// same session file stays on disk until a save or close removes it, so a
    /// second crash before the next throttled snapshot is still recoverable.
    pub(crate) fn restored(session_id: String, store: RecoveryStore) -> Self {
        Self {
            session_id,
            throttle: RecoveryThrottle::default(),
            store,
        }
    }
}

/// One snapshot tick for an open Document at injected clock `now`, using the
/// installed application data directory.
pub fn maybe_snapshot_at(document_id: &str, now: u64) -> Result<SnapshotOutcome, CommandError> {
    let Some(store) = recovery_store() else {
        return Ok(SnapshotOutcome::skipped());
    };
    maybe_snapshot_in(&store, document_id, now)
}

/// One snapshot tick with an explicit store and injected clock, so the
/// throttle schedule is deterministic in tests.
///
/// A dirty Document observes its current revision and writes when the throttle
/// is due. A clean Document removes any snapshot that belonged to this session
/// (a save or an undo back to the saved revision makes it stale) and resets the
/// schedule.
pub fn maybe_snapshot_in(
    store: &RecoveryStore,
    document_id: &str,
    now: u64,
) -> Result<SnapshotOutcome, CommandError> {
    crate::commands::with_open_document(document_id, |entry| {
        let history = entry.edit.history_view();
        if !history.dirty {
            if let Some(state) = entry.recovery.as_mut() {
                if let Err(error) = state.store.remove_snapshot(&state.session_id) {
                    logging::error("recovery.remove.failed", &error.to_string());
                }
                state.throttle.reset();
            }
            return Ok(SnapshotOutcome::skipped());
        }

        let due = {
            let state = entry
                .recovery
                .get_or_insert_with(|| RecoveryState::fresh(store));
            if state.store != *store {
                state.store = store.clone();
            }
            state.throttle.observe(history.revision, now);
            state.throttle.due(now)
        };
        if !due {
            return Ok(SnapshotOutcome::skipped());
        }

        // Serialization is the expensive step; the caller runs this command
        // off the UI thread. It re-derives every edit and never touches the
        // Source Document.
        let bytes = crate::save::serialize_live_document(entry)?;

        let session_id = entry
            .recovery
            .as_ref()
            .expect("recovery state exists for a dirty document")
            .session_id
            .clone();
        let security = &entry.source_security;
        let snapshot = RecoverySnapshot {
            format_version: METADATA_FORMAT_VERSION,
            session_id: session_id.clone(),
            source_path: entry.source_path.clone(),
            output_path: entry.save.output_path.clone(),
            display_name: file_name(&entry.source_path.to_string_lossy()),
            source_encrypted: security.encrypted,
            source_read_only: security.read_only,
            source_permissions: Some(security.permissions),
            dirty: true,
            revision: history.revision,
            saved_revision: history.saved_revision,
            created_at_millis: unix_millis(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        };

        store
            .write_snapshot(&snapshot, &bytes)
            .map_err(map_recovery_error)?;
        if let Some(state) = entry.recovery.as_mut() {
            state.throttle.mark_written(now);
        }

        logging::event(
            "recovery.snapshot",
            &format!(
                "documentId={document_id} sessionId={} bytes={}",
                snapshot.session_id,
                bytes.len()
            ),
        );

        Ok(SnapshotOutcome {
            written: true,
            session_id: Some(session_id),
            bytes: bytes.len() as u64,
        })
    })
}

/// Discards the Recovery File of a Document that just became clean (save) or
/// is closing normally. The session id is kept so the next edit re-creates the
/// same file name; the throttle starts over.
pub(crate) fn clear_for_entry(entry: &mut OpenDocument) {
    let Some(state) = entry.recovery.as_mut() else {
        return;
    };
    if let Err(error) = state.store.remove_snapshot(&state.session_id) {
        logging::error("recovery.remove.failed", &error.to_string());
    }
    state.throttle.reset();
}

pub(crate) fn map_recovery_error(error: RecoveryError) -> CommandError {
    match error {
        RecoveryError::InvalidSessionId(message) => CommandError::new(
            CommandErrorCode::InvalidArgument,
            RecoveryError::InvalidSessionId(message).to_string(),
        ),
        RecoveryError::TargetConflict(path) => CommandError::new(
            CommandErrorCode::RecoveryFailed,
            RecoveryError::TargetConflict(path).to_string(),
        ),
        other => CommandError::new(CommandErrorCode::RecoveryFailed, other.to_string()),
    }
}

/// The `recovery_snapshot` command: one throttled tick.
///
/// Declared async so serialization and disk I/O never run on the UI thread.
#[tauri::command]
pub async fn recovery_snapshot(document_id: String) -> Result<SnapshotOutcome, CommandError> {
    maybe_snapshot_at(&document_id, now_ticks())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(label: &str) -> PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "pdfx-recovery-unit-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch directory");
        dir
    }

    fn snapshot(session_id: &str, source: &Path) -> RecoverySnapshot {
        RecoverySnapshot {
            format_version: METADATA_FORMAT_VERSION,
            session_id: session_id.to_owned(),
            source_path: source.to_path_buf(),
            output_path: None,
            display_name: "report.pdf".to_owned(),
            source_encrypted: false,
            source_read_only: false,
            source_permissions: None,
            dirty: true,
            revision: 1,
            saved_revision: 0,
            created_at_millis: 1_700_000_000_000,
            app_version: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn the_throttle_debounces_then_caps_a_continuous_edit_stream() {
        let mut throttle = RecoveryThrottle::default();

        assert!(!throttle.due(0), "nothing is owed before an edit");
        throttle.observe(1, 0);
        assert!(!throttle.due(0), "an edit alone does not write immediately");
        assert!(!throttle.due(1_999), "still inside the quiet window");
        assert!(throttle.due(2_000), "the quiet window elapsed");
        throttle.mark_written(2_000);
        assert!(!throttle.due(3_000), "a write starts a new quiet window");

        // Continuous edits: observe a new revision every 500 ms. The debounce
        // never elapses, but the maximum interval forces a snapshot.
        let mut now = 2_000;
        loop {
            now += 500;
            throttle.observe(1 + now / 500, now);
            if throttle.due(now) {
                break;
            }
            assert!(
                now < SNAPSHOT_MAX_INTERVAL_MS * 2,
                "continuous editing must not postpone the snapshot forever"
            );
        }
        assert_eq!(
            now,
            2_500 + SNAPSHOT_MAX_INTERVAL_MS,
            "continuous editing is capped by the maximum interval"
        );
    }

    #[test]
    fn keystrokes_do_not_write_until_the_debounce_elapses() {
        let mut throttle = RecoveryThrottle::default();
        // A burst of edits every 100 ms: no tick inside the burst is due.
        for revision in 1..=19 {
            let now = revision * 100;
            assert!(throttle.observe(revision, now));
            assert!(
                !throttle.due(now),
                "tick {now} must not write on every keystroke"
            );
        }
        assert!(
            !throttle.due(2_000),
            "the quiet period restarted with the newest edit"
        );
        assert!(throttle.due(3_900), "two quiet seconds after the last edit");
    }

    #[test]
    fn a_snapshot_cannot_target_the_source_or_output_document() {
        let dir = scratch_dir("conflict");
        let store = RecoveryStore::new(&dir);
        fs::create_dir_all(store.directory()).expect("create recovery directory");

        let source = store.directory().join("s.pdfx-recovery.pdf");
        fs::write(&source, b"%PDF-1.7 source").expect("write source");

        let error = store
            .write_snapshot(&snapshot("s", &source), b"%PDF-1.7 snapshot")
            .expect_err("writing over the Source Document must be refused");
        assert!(matches!(error, RecoveryError::TargetConflict(_)));
        assert_eq!(
            fs::read(&source).expect("re-read source"),
            b"%PDF-1.7 source",
            "the refused write leaves the Source Document untouched"
        );

        let output = store.directory().join("o.pdfx-recovery.pdf");
        fs::write(&output, b"%PDF-1.7 output").expect("write output");
        let mut meta = snapshot("o", &dir.join("source.pdf"));
        meta.output_path = Some(output.clone());
        let error = store
            .write_snapshot(&meta, b"%PDF-1.7 snapshot")
            .expect_err("writing over the Output Document must be refused");
        assert!(matches!(error, RecoveryError::TargetConflict(_)));
        assert_eq!(
            fs::read(&output).expect("re-read output"),
            b"%PDF-1.7 output"
        );

        // The JSON sidecar is a recovery path too: neither it nor the PDF may
        // resolve to the Source or Output Document.
        let metadata_source = store.directory().join("m.pdfx-recovery.json");
        fs::write(&metadata_source, b"%PDF-1.7 metadata-named source")
            .expect("write source at the metadata path");
        let error = store
            .write_snapshot(&snapshot("m", &metadata_source), b"%PDF-1.7 snapshot")
            .expect_err("writing over a Source at the metadata path must be refused");
        assert!(matches!(error, RecoveryError::TargetConflict(_)));
        assert_eq!(
            fs::read(&metadata_source).expect("re-read metadata-named source"),
            b"%PDF-1.7 metadata-named source"
        );

        let mut metadata_output = snapshot("n", &dir.join("source.pdf"));
        metadata_output.output_path = Some(store.directory().join("n.pdfx-recovery.json"));
        fs::write(
            metadata_output.output_path.as_ref().expect("output path"),
            b"%PDF-1.7 metadata-named output",
        )
        .expect("write output at the metadata path");
        let error = store
            .write_snapshot(&metadata_output, b"%PDF-1.7 snapshot")
            .expect_err("writing over an Output at the metadata path must be refused");
        assert!(matches!(error, RecoveryError::TargetConflict(_)));
        assert_eq!(
            fs::read(metadata_output.output_path.as_ref().expect("output path"))
                .expect("re-read metadata-named output"),
            b"%PDF-1.7 metadata-named output"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_metadata_write_removes_the_new_snapshot_and_keeps_the_old_one() {
        let dir = scratch_dir("metadata-failure");
        let store = RecoveryStore::new(&dir);
        let source = dir.join("input.pdf");
        fs::write(&source, b"%PDF-1.7 source").expect("write source");

        store
            .write_snapshot(&snapshot("session", &source), b"%PDF-1.7 first")
            .expect("write the first snapshot");

        let mut writes = 0;
        let error = store
            .write_snapshot_with(
                &snapshot("session", &source),
                b"%PDF-1.7 second",
                |path, bytes| {
                    writes += 1;
                    if writes == 2 {
                        return Err(io::Error::other("disk full"));
                    }
                    write_bytes_atomically(path, bytes)
                },
            )
            .expect_err("the metadata failure must be reported");
        assert!(matches!(error, RecoveryError::Io(_)));
        let restored = store
            .read_snapshot("session")
            .expect("read snapshot")
            .expect("the previous pair survives");
        assert_eq!(restored.bytes, b"%PDF-1.7 first");

        let leftovers: Vec<_> = fs::read_dir(store.directory())
            .expect("list recovery directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temporary file may remain after a failed write: {leftovers:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scanning_discards_orphans_and_unreadable_pairs() {
        let dir = scratch_dir("scan");
        let store = RecoveryStore::new(&dir);
        let source = dir.join("input.pdf");
        fs::write(&source, b"%PDF-1.7 source").expect("write source");
        store
            .write_snapshot(&snapshot("valid", &source), b"%PDF-1.7 valid")
            .expect("write valid snapshot");

        fs::write(store.directory().join("orphan.pdfx-recovery.pdf"), b"junk")
            .expect("write orphan pdf");
        fs::write(
            store.directory().join("broken.pdfx-recovery.json"),
            b"not json",
        )
        .expect("write broken metadata");
        fs::write(store.directory().join("broken.pdfx-recovery.pdf"), b"junk")
            .expect("write broken pdf");

        let scan = store.scan().expect("scan");
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(scan.candidates[0].session_id, "valid");
        assert_eq!(scan.discarded, 2);
        assert!(!store.directory().join("orphan.pdfx-recovery.pdf").exists());
        assert!(!store.directory().join("broken.pdfx-recovery.json").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn metadata_carries_no_password_or_document_text() {
        let meta = snapshot("session", Path::new("/tmp/input.pdf"));
        let encoded = serde_json::to_string(&meta).expect("encode metadata");
        for forbidden in ["secret", "Confidential", "password", "Hello PDFX"] {
            assert!(
                !encoded.to_lowercase().contains(&forbidden.to_lowercase()),
                "metadata must not contain {forbidden:?}: {encoded}"
            );
        }
    }

    #[test]
    fn recent_files_deduplicate_and_cap_the_list() {
        let dir = scratch_dir("recent");
        let store = RecentFilesStore::new(&dir);
        let first = dir.join("first.pdf");
        fs::write(&first, b"%PDF-1.7").expect("write first");
        let second = dir.join("second.pdf");
        fs::write(&second, b"%PDF-1.7").expect("write second");

        store
            .record(&first.to_string_lossy(), 1)
            .expect("record first");
        store
            .record(&second.to_string_lossy(), 2)
            .expect("record second");
        let files = store
            .record(&first.to_string_lossy(), 3)
            .expect("re-record first");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, first.to_string_lossy());
        assert_eq!(files[0].last_opened_millis, 3);

        for index in 0..(MAX_RECENT_FILES + 3) {
            let path = dir.join(format!("f{index}.pdf"));
            fs::write(&path, b"%PDF-1.7").expect("write file");
            store
                .record(&path.to_string_lossy(), 10 + index as u64)
                .expect("record file");
        }
        let files = RecentFilesStore::new(&dir).list();
        assert_eq!(files.len(), MAX_RECENT_FILES);
        assert_eq!(
            files[0].name,
            format!("f{}.pdf", MAX_RECENT_FILES + 2),
            "the newest entry is first"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_files_survive_a_new_store_and_drop_missing_files() {
        let dir = scratch_dir("recent-restart");
        let existing = dir.join("existing.pdf");
        fs::write(&existing, b"%PDF-1.7").expect("write existing");
        let missing = dir.join("missing.pdf");
        fs::write(&missing, b"%PDF-1.7").expect("write missing");
        RecentFilesStore::new(&dir)
            .record(&existing.to_string_lossy(), 1)
            .expect("record existing");
        RecentFilesStore::new(&dir)
            .record(&missing.to_string_lossy(), 2)
            .expect("record missing");
        fs::remove_file(&missing).expect("remove missing file");

        let files = RecentFilesStore::new(&dir).list();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, existing.to_string_lossy());
        let raw = fs::read_to_string(dir.join(RECENT_FILES_FILE_NAME)).expect("read list");
        assert!(
            !raw.contains("missing.pdf"),
            "the missing entry is removed from the persisted list"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
