//! Local rolling logs and user-initiated diagnostic export.
//!
//! Logs stay on this device. There is no network code here and no automatic
//! upload: a diagnostic report is only written when the user chooses a path.
//! The rolling file lives under the application log directory, keeps a bounded
//! number of archives, and flattens multi-line details so every event occupies
//! exactly one line.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_FILE_NAME: &str = "pdfx.log";
pub const DEFAULT_MAX_BYTES: u64 = 1_000_000;
pub const DEFAULT_MAX_FILES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Error,
}

impl LogLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Error => "ERROR",
        }
    }
}

/// A rolling log file in one directory.
#[derive(Debug)]
pub struct Logger {
    directory: PathBuf,
    max_bytes: u64,
    max_files: usize,
}

impl Logger {
    pub fn new(
        directory: impl Into<PathBuf>,
        max_bytes: u64,
        max_files: usize,
    ) -> io::Result<Self> {
        if max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_bytes must be greater than zero",
            ));
        }
        if max_files == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_files must be greater than zero",
            ));
        }

        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            max_bytes,
            max_files,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn log_event(&self, event: &str, detail: &str) -> io::Result<()> {
        self.write(LogLevel::Info, event, detail)
    }

    pub fn log_error(&self, event: &str, detail: &str) -> io::Result<()> {
        self.write(LogLevel::Error, event, detail)
    }

    fn write(&self, level: LogLevel, event: &str, detail: &str) -> io::Result<()> {
        let line = format_line(level, event, detail, SystemTime::now());
        let current = self.current_path();
        let current_len = fs::metadata(&current).map(|meta| meta.len()).unwrap_or(0);

        // `max_bytes` is a soft cap: a single oversized line still gets written
        // so evidence is never dropped.
        if current_len > 0 && current_len + line.len() as u64 > self.max_bytes {
            self.rotate()?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current)?;
        file.write_all(line.as_bytes())
    }

    fn rotate(&self) -> io::Result<()> {
        let current = self.current_path();
        if !current.exists() {
            return Ok(());
        }

        let _ = fs::remove_file(self.archive_path(self.max_files));
        for index in (1..self.max_files).rev() {
            let source = self.archive_path(index);
            if source.exists() {
                let target = self.archive_path(index + 1);
                let _ = fs::remove_file(&target);
                fs::rename(&source, &target)?;
            }
        }

        let first = self.archive_path(1);
        let _ = fs::remove_file(&first);
        fs::rename(&current, first)
    }

    /// Existing log files from oldest to newest.
    pub fn log_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for index in (1..=self.max_files).rev() {
            let path = self.archive_path(index);
            if path.exists() {
                files.push(path);
            }
        }
        let current = self.current_path();
        if current.exists() {
            files.push(current);
        }
        files
    }

    fn current_path(&self) -> PathBuf {
        self.directory.join(LOG_FILE_NAME)
    }

    fn archive_path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("{LOG_FILE_NAME}.{index}"))
    }
}

fn format_line(level: LogLevel, event: &str, detail: &str, time: SystemTime) -> String {
    let timestamp = format_timestamp(time);
    let event = sanitize(event);
    let detail = sanitize(detail);
    if detail.is_empty() {
        format!("{timestamp} {} {event}\n", level.label())
    } else {
        format!("{timestamp} {} {event} {detail}\n", level.label())
    }
}

/// Keeps one event per line; a newline in a detail becomes a literal `\n`.
fn sanitize(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn format_timestamp(time: SystemTime) -> String {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let seconds = millis / 1_000;
    let milliseconds = millis % 1_000;
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milliseconds:03}Z")
}

/// Howard Hinnant's days-to-civil date algorithm, with day 0 = 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };

    (year, month as u32, day)
}

/// A diagnostic report ready to be written to a user-chosen path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsReport {
    pub contents: String,
    pub log_files: u32,
}

/// The result of a completed diagnostic export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsExport {
    pub path: PathBuf,
    pub bytes: u64,
    pub log_files: u32,
}

/// Builds a plain-text report from version and platform information plus the
/// contents of every retained log file. No settings or document contents are
/// included.
pub fn build_diagnostic_report(
    logger: Option<&Logger>,
    app_name: &str,
    app_version: &str,
) -> DiagnosticsReport {
    let mut contents = String::new();
    contents.push_str(&format!("{app_name} diagnostics\n"));
    contents.push_str(&format!(
        "generated: {}\n",
        format_timestamp(SystemTime::now())
    ));
    contents.push_str(&format!("app: {app_name} {app_version}\n"));
    contents.push_str(&format!(
        "platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    contents.push('\n');

    let files = logger.map(Logger::log_files).unwrap_or_default();
    if files.is_empty() {
        contents.push_str("logs: no log entries yet\n");
    } else {
        for path in &files {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(LOG_FILE_NAME);
            contents.push_str(&format!("--- {name} ---\n"));
            match fs::read_to_string(path) {
                Ok(log) => {
                    contents.push_str(&log);
                    if !log.ends_with('\n') {
                        contents.push('\n');
                    }
                }
                Err(error) => contents.push_str(&format!("<unreadable: {error}>\n")),
            }
        }
    }

    DiagnosticsReport {
        contents,
        log_files: files.len() as u32,
    }
}

/// Writes a diagnostic report to `destination`, creating its parent directory
/// when needed. This is the only path logs ever take off the device, and the
/// user chooses it.
pub fn export_diagnostics(
    destination: &Path,
    app_name: &str,
    app_version: &str,
) -> io::Result<DiagnosticsExport> {
    let report = match GLOBAL.get() {
        Some(logger) => {
            let logger = logger
                .lock()
                .map_err(|_| io::Error::other("log lock poisoned"))?;
            build_diagnostic_report(Some(&logger), app_name, app_version)
        }
        None => build_diagnostic_report(None, app_name, app_version),
    };

    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(destination, report.contents.as_bytes())?;

    event(
        "diagnostics.export",
        &format!("path={}", destination.display()),
    );

    Ok(DiagnosticsExport {
        path: destination.to_path_buf(),
        bytes: report.contents.len() as u64,
        log_files: report.log_files,
    })
}

static GLOBAL: OnceLock<Mutex<Logger>> = OnceLock::new();

/// Installs the process-wide logger. The app calls this once during setup; a
/// second call is ignored. Failure to open a log directory is not fatal.
pub fn initialize(directory: impl Into<PathBuf>) -> io::Result<()> {
    if GLOBAL.get().is_some() {
        return Ok(());
    }
    let logger = Logger::new(directory, DEFAULT_MAX_BYTES, DEFAULT_MAX_FILES)?;
    let _ = GLOBAL.set(Mutex::new(logger));
    Ok(())
}

/// Records an informational event. A missing or poisoned logger is ignored: the
/// application keeps working even when logging is unavailable.
pub fn event(event: &str, detail: &str) {
    if let Some(logger) = GLOBAL.get() {
        if let Ok(logger) = logger.lock() {
            let _ = logger.log_event(event, detail);
        }
    }
}

/// Records an error event. See [`event`].
pub fn error(event: &str, detail: &str) {
    if let Some(logger) = GLOBAL.get() {
        if let Ok(logger) = logger.lock() {
            let _ = logger.log_error(event, detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(millis: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(millis)
    }

    #[test]
    fn timestamps_render_as_utc_iso8601() {
        assert_eq!(format_timestamp(at(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_timestamp(at(1_704_067_200_000)),
            "2024-01-01T00:00:00.000Z"
        );
        assert_eq!(
            format_timestamp(at(1_700_000_000_123)),
            "2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn lines_are_flattened_and_empty_details_leave_no_trailing_space() {
        assert_eq!(
            format_line(LogLevel::Info, "app.start", "", at(0)),
            "1970-01-01T00:00:00.000Z INFO app.start\n"
        );
        assert_eq!(
            format_line(LogLevel::Error, "parse", "a\nb\rc", at(0)),
            "1970-01-01T00:00:00.000Z ERROR parse a\\nb\\rc\n"
        );
    }
}
