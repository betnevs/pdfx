//! Locally persisted application settings.
//!
//! Settings live in `settings.json` under the app config directory. Writes are
//! atomic (temp file + rename) so a crash cannot leave a half-written file, and
//! a missing or corrupt file falls back to defaults instead of blocking start-up.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::file_association::{AssociationResult, FileAssociationPlatform};
use crate::updater;

pub const SETTINGS_FILE_NAME: &str = "settings.json";
pub const DEFAULT_ANNOTATION_AUTHOR: &str = "PDFX 用户";
const MAX_ANNOTATION_AUTHOR_CHARS: usize = 64;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Interface language choices offered by the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InterfaceLanguage {
    #[default]
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}

/// The application theme. `System` follows the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Everything the settings page exposes.
///
/// `annotation_author` is the contract later annotation tickets use when they
/// create a new annotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub language: InterfaceLanguage,
    pub theme: Theme,
    pub annotation_author: String,
    pub file_association_enabled: bool,
    /// Address of the version manifest the user-initiated update check reads.
    /// Empty means the check is not configured and no request is made.
    pub update_manifest_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: InterfaceLanguage::default(),
            theme: Theme::default(),
            annotation_author: DEFAULT_ANNOTATION_AUTHOR.to_owned(),
            // File associations stay off until the user opts in.
            file_association_enabled: false,
            update_manifest_url: String::new(),
        }
    }
}

impl Settings {
    /// Trims and validates user-entered text before it is persisted.
    fn normalized(self) -> Result<Self, SettingsError> {
        let annotation_author = self.annotation_author.trim();
        if annotation_author.is_empty() {
            return Err(SettingsError::InvalidArgument(
                "Annotation 显示名不能为空。".to_owned(),
            ));
        }
        if annotation_author.chars().count() > MAX_ANNOTATION_AUTHOR_CHARS {
            return Err(SettingsError::InvalidArgument(format!(
                "Annotation 显示名不能超过 {MAX_ANNOTATION_AUTHOR_CHARS} 个字符。"
            )));
        }
        if annotation_author.chars().any(char::is_control) {
            return Err(SettingsError::InvalidArgument(
                "Annotation 显示名不能包含换行或控制字符。".to_owned(),
            ));
        }

        let update_manifest_url = self.update_manifest_url.trim();
        if update_manifest_url.chars().count() > updater::MAX_MANIFEST_URL_CHARS {
            return Err(SettingsError::InvalidArgument(format!(
                "更新地址不能超过 {} 个字符。",
                updater::MAX_MANIFEST_URL_CHARS
            )));
        }
        if !update_manifest_url.is_empty() {
            updater::validate_manifest_url(update_manifest_url)
                .map_err(SettingsError::InvalidArgument)?;
        }

        Ok(Self {
            annotation_author: annotation_author.to_owned(),
            update_manifest_url: update_manifest_url.to_owned(),
            ..self
        })
    }
}

#[derive(Debug)]
pub enum SettingsError {
    InvalidArgument(String),
    Io(io::Error),
    Serialize(serde_json::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "could not access the settings file: {error}"),
            Self::Serialize(error) => {
                write!(formatter, "could not encode the settings file: {error}")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<io::Error> for SettingsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SettingsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// The persisted settings file.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// `config_dir` is the application config directory, not the file itself.
    pub fn new(config_dir: impl AsRef<Path>) -> Self {
        Self {
            path: config_dir.as_ref().join(SETTINGS_FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads settings, falling back to defaults when the file is missing or
    /// unreadable. A corrupt file must never prevent the app from starting.
    pub fn load(&self) -> Settings {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persists settings atomically: write a sibling temp file, sync it, then
    /// rename it over `settings.json`.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let raw = serde_json::to_vec_pretty(settings)?;
        let directory = self.path.parent().ok_or_else(|| {
            SettingsError::InvalidArgument("settings path has no parent directory".to_owned())
        })?;
        fs::create_dir_all(directory)?;

        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = directory.join(format!(
            ".{SETTINGS_FILE_NAME}.{}-{unique}.tmp",
            std::process::id()
        ));

        let write_result = (|| -> Result<(), io::Error> {
            let mut file = File::create(&temp_path)?;
            file.write_all(&raw)?;
            file.write_all(b"\n")?;
            file.sync_all()
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(error.into());
        }

        if let Err(error) = fs::rename(&temp_path, &self.path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error.into());
        }

        Ok(())
    }
}

/// The result of an `update_settings` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdateResult {
    pub settings: Settings,
    /// Present when this update changed the file-association preference.
    pub association: Option<AssociationResult>,
}

/// The behavior of the `update_settings` command, decoupled from the Tauri app
/// handle so tests can inject a platform implementation.
///
/// The preference is persisted even when the operating system still requires
/// the user to confirm the default app: the toggle means "register PDFX as a
/// handler", not "PDFX silently became the default".
pub fn apply_update(
    store: &SettingsStore,
    platform: &dyn FileAssociationPlatform,
    update: Settings,
) -> Result<SettingsUpdateResult, SettingsError> {
    let current = store.load();
    let settings = update.normalized()?;
    store.save(&settings)?;

    let association = if settings.file_association_enabled != current.file_association_enabled {
        let outcome = if settings.file_association_enabled {
            platform.register()
        } else {
            platform.unregister()
        };
        Some(match outcome {
            Ok(result) => result,
            Err(message) => AssociationResult::failed(message),
        })
    } else {
        None
    };

    Ok(SettingsUpdateResult {
        settings,
        association,
    })
}
