//! Best-effort operating-system file-association registration.
//!
//! Modern macOS and Windows 10+ deliberately prevent an application from
//! silently taking over a file type. This module registers PDFX as an available
//! PDF handler and reports honestly when the operating system still requires the
//! user to confirm the default application; it never claims a default it does
//! not have. The real platform implementations are only exercised by the
//! ignored manual test.

use serde::{Deserialize, Serialize};

/// How far the operating system let the registration attempt get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationStatus {
    /// PDFX is registered and is known to be the default PDF handler.
    Registered,
    /// PDFX is registered as an available handler; the user must confirm the
    /// default in the operating system's default-apps UI.
    UserActionRequired,
    /// PDFX's handler registration has been removed.
    Unregistered,
    /// This build or platform cannot register a handler (for example a
    /// development build without an application bundle).
    Unsupported,
    /// The registration attempt itself failed.
    Failed,
}

/// The outcome of a register or unregister attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociationResult {
    pub status: AssociationStatus,
    /// User-facing guidance for what to do next. Empty when nothing is needed.
    pub instructions: String,
    /// Technical detail for a failed attempt.
    pub message: Option<String>,
}

impl AssociationResult {
    pub fn registered(instructions: impl Into<String>) -> Self {
        Self {
            status: AssociationStatus::Registered,
            instructions: instructions.into(),
            message: None,
        }
    }

    pub fn user_action_required(instructions: impl Into<String>) -> Self {
        Self {
            status: AssociationStatus::UserActionRequired,
            instructions: instructions.into(),
            message: None,
        }
    }

    pub fn unregistered(instructions: impl Into<String>) -> Self {
        Self {
            status: AssociationStatus::Unregistered,
            instructions: instructions.into(),
            message: None,
        }
    }

    pub fn unsupported(instructions: impl Into<String>) -> Self {
        Self {
            status: AssociationStatus::Unsupported,
            instructions: instructions.into(),
            message: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            status: AssociationStatus::Failed,
            instructions: String::new(),
            message: Some(message.into()),
        }
    }
}

/// The platform side of the file-association toggle.
///
/// Tests inject a mock implementation; production code selects the real one
/// through [`platform_for`].
pub trait FileAssociationPlatform: Send + Sync {
    fn register(&self) -> Result<AssociationResult, String>;
    fn unregister(&self) -> Result<AssociationResult, String>;
}

/// Platforms without a file-association implementation.
pub struct UnsupportedPlatform;

impl FileAssociationPlatform for UnsupportedPlatform {
    fn register(&self) -> Result<AssociationResult, String> {
        Ok(AssociationResult::unsupported(
            "当前平台无法自动注册文件关联。请在操作系统的默认应用设置中手动选择 PDFX。",
        ))
    }

    fn unregister(&self) -> Result<AssociationResult, String> {
        Ok(AssociationResult::unsupported(
            "当前平台没有可移除的 PDFX 文件关联。",
        ))
    }
}

/// Selects the implementation for the running operating system.
pub fn platform_for(app: &tauri::AppHandle) -> Box<dyn FileAssociationPlatform> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacFileAssociation::for_app(app))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        Box::new(WindowsFileAssociation::for_current_executable())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        Box::new(UnsupportedPlatform)
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacFileAssociation;

#[cfg(target_os = "windows")]
pub use windows::WindowsFileAssociation;

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tauri::Manager;

    use super::{AssociationResult, FileAssociationPlatform};

    const LSREGISTER: &str =
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

    /// macOS registration through LaunchServices.
    ///
    /// Registering the bundle makes PDFX available as a PDF handler. macOS
    /// requires the user to choose the default handler, so a successful
    /// registration reports [`AssociationResult::user_action_required`].
    pub struct MacFileAssociation {
        bundle: Option<PathBuf>,
    }

    impl MacFileAssociation {
        pub fn new(bundle: Option<PathBuf>) -> Self {
            Self { bundle }
        }

        pub fn for_app(app: &tauri::AppHandle) -> Self {
            Self::new(app_bundle(app))
        }
    }

    /// Resolves `PDFX.app` from the bundled resources directory. Development
    /// builds run from `target/...` and have no application bundle.
    fn app_bundle(app: &tauri::AppHandle) -> Option<PathBuf> {
        let resources = app.path().resource_dir().ok()?;
        let bundle = resources.parent()?.parent()?;
        (bundle.extension().and_then(|extension| extension.to_str()) == Some("app"))
            .then(|| bundle.to_path_buf())
    }

    fn run_lsregister(flag: &str, bundle: &Path) -> Result<(), String> {
        let output = Command::new(LSREGISTER)
            .arg(flag)
            .arg(bundle)
            .output()
            .map_err(|error| format!("could not run lsregister: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "lsregister {flag} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    impl FileAssociationPlatform for MacFileAssociation {
        fn register(&self) -> Result<AssociationResult, String> {
            let Some(bundle) = &self.bundle else {
                return Ok(AssociationResult::unsupported(
                    "macOS 上，当前构建不是已安装的应用包，无法注册文件关联。请安装 PDFX 后，在“访达”中右键任意 PDF → 显示简介 → 打开方式 → 选择 PDFX → 全部更改。",
                ));
            };

            run_lsregister("-f", bundle)?;

            Ok(AssociationResult::user_action_required(
                "PDFX 已注册为可选的 PDF 打开方式。macOS 不允许应用自动更改默认应用：请右键任意 PDF → 显示简介 → 打开方式 → 选择 PDFX → 全部更改。",
            ))
        }

        fn unregister(&self) -> Result<AssociationResult, String> {
            let Some(bundle) = &self.bundle else {
                return Ok(AssociationResult::unsupported(
                    "当前构建没有可移除的 PDFX 文件关联。",
                ));
            };

            run_lsregister("-u", bundle)?;

            Ok(AssociationResult::unregistered(
                "已从系统注册中移除 PDFX。如果它仍是默认 PDF 应用，请在 PDF 的“显示简介”中改回其他应用。",
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_installed_bundle_is_the_only_registerable_form() {
            let platform = MacFileAssociation::new(None);
            let result = platform.register().expect("register without a bundle");

            assert_eq!(result.status, super::super::AssociationStatus::Unsupported);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{AssociationResult, FileAssociationPlatform};

    const DOCUMENT_PROG_ID: &str = "PDFX.Document";
    const DOCUMENT_CLASS_KEY: &str = r"HKCU\Software\Classes\PDFX.Document";
    const PDF_OPEN_WITH_KEY: &str = r"HKCU\Software\Classes\.pdf\OpenWithProgids";

    /// Windows registration through per-user registry entries.
    ///
    /// PDFX is added to the `.pdf` Open-with list. Windows 10+ protects the
    /// user's default-app choice with a hash, so writing the default is ignored;
    /// the user must confirm PDFX in Settings → Apps → Default apps.
    pub struct WindowsFileAssociation {
        executable: PathBuf,
    }

    impl WindowsFileAssociation {
        pub fn new(executable: PathBuf) -> Self {
            Self { executable }
        }

        pub fn for_current_executable() -> Self {
            Self::new(std::env::current_exe().unwrap_or_default())
        }
    }

    fn run_registry(args: &[String]) -> Result<(), String> {
        let output = Command::new("reg")
            .args(args)
            .output()
            .map_err(|error| format!("could not run reg.exe: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "reg.exe {} failed: {}",
                args.get(1).map(String::as_str).unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn registry_key_exists(key: &str) -> bool {
        Command::new("reg")
            .args(["query", key])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn registry_value_exists(key: &str, value: &str) -> bool {
        Command::new("reg")
            .args(["query", key, "/v", value])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// The `reg.exe` invocations that make PDFX an available PDF handler.
    /// The `.pdf` default value is intentionally untouched.
    fn register_commands(executable: &Path) -> Vec<Vec<String>> {
        let executable = executable.display().to_string();
        let open_command = format!("\"{executable}\" \"%1\"");
        let icon = format!("\"{executable}\",0");
        let class_key = DOCUMENT_CLASS_KEY.to_owned();
        let icon_key = format!("{DOCUMENT_CLASS_KEY}\\DefaultIcon");
        let command_key = format!("{DOCUMENT_CLASS_KEY}\\shell\\open\\command");
        let open_with_key = PDF_OPEN_WITH_KEY.to_owned();
        let prog_id = DOCUMENT_PROG_ID.to_owned();

        vec![
            vec![
                "add".to_owned(),
                class_key.clone(),
                "/ve".to_owned(),
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                "PDFX PDF 文档".to_owned(),
                "/f".to_owned(),
            ],
            vec![
                "add".to_owned(),
                class_key,
                "/v".to_owned(),
                "FriendlyTypeName".to_owned(),
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                "PDFX PDF 文档".to_owned(),
                "/f".to_owned(),
            ],
            vec![
                "add".to_owned(),
                icon_key,
                "/ve".to_owned(),
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                icon,
                "/f".to_owned(),
            ],
            vec![
                "add".to_owned(),
                command_key,
                "/ve".to_owned(),
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                open_command,
                "/f".to_owned(),
            ],
            vec![
                "add".to_owned(),
                open_with_key,
                "/v".to_owned(),
                prog_id,
                "/t".to_owned(),
                "REG_SZ".to_owned(),
                "/d".to_owned(),
                String::new(),
                "/f".to_owned(),
            ],
        ]
    }

    fn unregister_commands() -> Vec<Vec<String>> {
        vec![
            strings(&["delete", DOCUMENT_CLASS_KEY, "/f"]),
            strings(&["delete", PDF_OPEN_WITH_KEY, "/v", DOCUMENT_PROG_ID, "/f"]),
        ]
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    impl FileAssociationPlatform for WindowsFileAssociation {
        fn register(&self) -> Result<AssociationResult, String> {
            if self.executable.as_os_str().is_empty() {
                return Err("could not locate the PDFX executable".to_owned());
            }

            for command in register_commands(&self.executable) {
                run_registry(&command)?;
            }

            Ok(AssociationResult::user_action_required(
                "PDFX 已加入“打开方式”列表。Windows 不允许应用自动更改默认应用：请打开 设置 → 应用 → 默认应用 → 按文件类型选择 .pdf → 选择 PDFX。",
            ))
        }

        fn unregister(&self) -> Result<AssociationResult, String> {
            if registry_key_exists(DOCUMENT_CLASS_KEY) {
                run_registry(&strings(&["delete", DOCUMENT_CLASS_KEY, "/f"]))?;
            }
            if registry_value_exists(PDF_OPEN_WITH_KEY, DOCUMENT_PROG_ID) {
                run_registry(&strings(&[
                    "delete",
                    PDF_OPEN_WITH_KEY,
                    "/v",
                    DOCUMENT_PROG_ID,
                    "/f",
                ]))?;
            }

            Ok(AssociationResult::unregistered(
                "已移除 PDFX 的打开方式注册。如果它仍是默认 PDF 应用，请在 设置 → 应用 → 默认应用 中改回其他应用。",
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn registration_adds_an_open_with_entry_without_claiming_the_default() {
            let commands = register_commands(Path::new(r"C:\Program Files\PDFX\PDFX.exe"));

            assert_eq!(commands.len(), 5);
            assert_eq!(commands[0][1], DOCUMENT_CLASS_KEY);
            assert_eq!(
                commands[3][6],
                "\"C:\\Program Files\\PDFX\\PDFX.exe\" \"%1\""
            );
            assert_eq!(commands[4][1], PDF_OPEN_WITH_KEY);
            assert!(!commands.iter().any(|command| {
                command.len() >= 2 && command[1] == r"HKCU\Software\Classes\.pdf"
            }));
        }

        #[test]
        fn unregistration_only_removes_pdfx_entries() {
            let commands = unregister_commands();

            assert_eq!(commands[0], strings(&["delete", DOCUMENT_CLASS_KEY, "/f"]));
            assert_eq!(
                commands[1],
                strings(&["delete", PDF_OPEN_WITH_KEY, "/v", DOCUMENT_PROG_ID, "/f"])
            );
        }
    }
}
