//! User-initiated update check.
//!
//! ADR-0009: updates are semi-automatic. PDFX never checks on start-up or in
//! the background, never downloads or installs anything, and sends no
//! telemetry. The only network activity is the single HTTP GET the
//! `check_for_updates` command performs after the user clicks "检查更新", and
//! the system browser is only opened when the user clicks "打开下载页".
//!
//! The request carries no cookies, credentials, document data or usage data:
//! it is a plain `GET` for a public JSON manifest, with a finite end-to-end
//! timeout and a small redirect budget.

use std::fmt;
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use ureq::Agent;
use url::Url;

/// Longest manifest address the settings page accepts, in characters.
pub const MAX_MANIFEST_URL_CHARS: usize = 2048;

/// End-to-end budget for one update check, including DNS and the body read.
pub const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirect hops the check follows before failing.
pub const UPDATE_CHECK_MAX_REDIRECTS: u32 = 3;

/// Largest manifest body the check accepts. A version manifest is tiny; the
/// limit keeps a misbehaving server from exhausting memory.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// The JSON manifest published at the user-configured address.
///
/// Shape: `{ "version": "x.y.z", "notes": "...", "downloadUrl": "https://..." }`.
/// `notes` is optional; unknown fields are ignored so the manifest can grow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub download_url: String,
}

/// Whether the manifest describes a newer release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpdateAvailable,
    UpToDate,
}

/// The result of a successful `check_for_updates` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub status: UpdateStatus,
    pub current_version: String,
    /// The version the manifest advertises.
    pub version: String,
    pub notes: Option<String>,
    pub download_url: String,
}

#[derive(Debug)]
pub enum UpdateError {
    /// No manifest address is configured; no request was made.
    NotConfigured,
    /// The configured manifest address is not an absolute http(s) URL.
    InvalidManifestUrl(String),
    /// The manifest's download page is not an absolute http(s) URL.
    InvalidDownloadUrl(String),
    /// Network, timeout or non-success HTTP status. Retryable.
    Http(String),
    /// The response was not a valid manifest. Not retryable without a fix.
    ManifestInvalid(String),
    /// The operating system could not open the download page.
    OpenFailed(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => formatter.write_str("update manifest address is not configured"),
            Self::InvalidManifestUrl(message)
            | Self::InvalidDownloadUrl(message)
            | Self::Http(message)
            | Self::ManifestInvalid(message)
            | Self::OpenFailed(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for UpdateError {}

/// Parses an absolute `http`/`https` URL with a host.
///
/// Other schemes are rejected so the update check can never read local files
/// and the download button can never hand the system opener a `file:` or
/// `javascript:` URL. Embedded credentials are rejected as well.
fn parse_http_url(raw: &str, label: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| {
        format!("{label}必须是完整的 http 或 https 链接，例如 https://example.com/latest.json。")
    })?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{label}只支持 http 或 https 链接。"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(format!("{label}缺少主机名。"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label}不能包含用户名或密码。"));
    }

    Ok(url)
}

/// Validates the address of the version manifest.
pub fn validate_manifest_url(raw: &str) -> Result<Url, String> {
    parse_http_url(raw, "更新地址")
}

/// Validates the download page from a version manifest before it is handed to
/// the system browser.
pub fn validate_download_url(raw: &str) -> Result<Url, String> {
    parse_http_url(raw, "下载页地址")
}

/// The system-browser boundary, injectable so tests never launch a browser.
pub trait DownloadPageOpener {
    fn open(&self, url: &str) -> Result<(), String>;
}

/// Opens URLs with the operating system's default browser.
///
/// The `open` crate (MIT) uses `ShellExecuteW` on Windows (through the
/// `shellexecute-on-windows` feature) and the `open` command on macOS, so the
/// URL never passes through a shell. The URL is validated here first, so the
/// system launcher only ever sees an absolute http(s) address.
pub struct SystemOpener;

impl DownloadPageOpener for SystemOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        open::that_detached(url)
            .map_err(|error| format!("could not open the download page: {error}"))
    }
}

/// Validates the manifest's download page and hands it to the system browser.
///
/// Opening happens only when the user clicks "打开下载页"; a launcher failure
/// is returned to the caller instead of being swallowed.
pub fn open_download_page(opener: &dyn DownloadPageOpener, url: &str) -> Result<(), UpdateError> {
    let url = validate_download_url(url).map_err(UpdateError::InvalidDownloadUrl)?;
    opener.open(url.as_str()).map_err(UpdateError::OpenFailed)
}

/// Performs the update check with one explicit GET.
///
/// `current_version` is the running application's version; it is a parameter
/// so tests can pin the comparison independently of the build.
pub fn check_for_updates(
    manifest_url: &str,
    current_version: &str,
) -> Result<UpdateCheckResult, UpdateError> {
    UpdateChecker::new().check(manifest_url, current_version)
}

/// A reusable update checker. Constructing one performs no I/O; only
/// [`UpdateChecker::check`] opens a connection.
#[derive(Clone)]
pub struct UpdateChecker {
    agent: Agent,
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateChecker {
    pub fn new() -> Self {
        Self::with_timeout(UPDATE_CHECK_TIMEOUT)
    }

    /// Builds a checker with a custom end-to-end budget. Used by tests to
    /// exercise the timeout path without waiting for the production budget.
    pub fn with_timeout(timeout: Duration) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(timeout))
            .max_redirects(UPDATE_CHECK_MAX_REDIRECTS)
            .user_agent(concat!("PDFX/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }

    pub fn check(
        &self,
        manifest_url: &str,
        current_version: &str,
    ) -> Result<UpdateCheckResult, UpdateError> {
        let manifest_url = manifest_url.trim();
        if manifest_url.is_empty() {
            return Err(UpdateError::NotConfigured);
        }
        let url = validate_manifest_url(manifest_url).map_err(UpdateError::InvalidManifestUrl)?;

        let current = Version::parse(current_version).map_err(|_| {
            UpdateError::ManifestInvalid(format!("应用版本号 {current_version:?} 不是 x.y.z 格式"))
        })?;

        let mut response = self
            .agent
            .get(url.as_str())
            .header("Accept", "application/json")
            .call()
            .map_err(map_http_error)?;

        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_MANIFEST_BYTES)
            .read_to_string()
            .map_err(map_body_error)?;

        let manifest: UpdateManifest = serde_json::from_str(&body)
            .map_err(|error| UpdateError::ManifestInvalid(format!("无法解析版本信息：{error}")))?;

        let version = Version::parse(manifest.version.trim()).map_err(|_| {
            UpdateError::ManifestInvalid(format!("版本号 {:?} 不是 x.y.z 格式", manifest.version))
        })?;
        validate_download_url(&manifest.download_url).map_err(UpdateError::InvalidDownloadUrl)?;

        let status = if version > current {
            UpdateStatus::UpdateAvailable
        } else {
            UpdateStatus::UpToDate
        };

        Ok(UpdateCheckResult {
            status,
            current_version: current.to_string(),
            version: version.to_string(),
            notes: manifest.notes.filter(|notes| !notes.trim().is_empty()),
            download_url: manifest.download_url,
        })
    }
}

fn map_http_error(error: ureq::Error) -> UpdateError {
    match error {
        ureq::Error::StatusCode(code) => UpdateError::Http(format!("更新服务器返回 HTTP {code}")),
        ureq::Error::Timeout(_) => UpdateError::Http("更新检查超时".to_owned()),
        ureq::Error::HostNotFound => UpdateError::Http("找不到更新服务器".to_owned()),
        ureq::Error::TooManyRedirects => UpdateError::Http("更新服务器重定向次数过多".to_owned()),
        other => UpdateError::Http(format!("无法连接更新服务器：{other}")),
    }
}

fn map_body_error(error: ureq::Error) -> UpdateError {
    match error {
        ureq::Error::BodyExceedsLimit(limit) => {
            UpdateError::ManifestInvalid(format!("版本信息超过 {limit} 字节上限"))
        }
        ureq::Error::Timeout(_) => UpdateError::Http("更新检查超时".to_owned()),
        other => UpdateError::ManifestInvalid(format!("无法读取版本信息：{other}")),
    }
}
