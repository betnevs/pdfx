//! Command-boundary tests for the user-initiated update check.
//!
//! Every test drives the same functions the `check_for_updates` command wraps.
//! Network tests talk to a local HTTP server on 127.0.0.1; nothing reaches the
//! internet.

mod support;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use pdfx_lib::file_association::{AssociationResult, FileAssociationPlatform};
use pdfx_lib::settings::{apply_update, Settings, SettingsStore};
use pdfx_lib::updater::{
    check_for_updates, open_download_page, DownloadPageOpener, UpdateChecker, UpdateError,
    UpdateStatus,
};
use support::http::{RedirectTarget, Response, TestServer};
use support::temp::TempDir;

struct NoopPlatform;

impl FileAssociationPlatform for NoopPlatform {
    fn register(&self) -> Result<AssociationResult, String> {
        Ok(AssociationResult::user_action_required(""))
    }

    fn unregister(&self) -> Result<AssociationResult, String> {
        Ok(AssociationResult::unregistered(""))
    }
}

const NEWER_MANIFEST: &str = r#"{
  "version": "0.2.0",
  "notes": "新增批量导出。",
  "downloadUrl": "https://example.com/pdfx/download"
}"#;

fn server_with(body: impl Into<String>) -> TestServer {
    TestServer::start(Response::Status {
        code: 200,
        body: body.into(),
    })
}

#[test]
fn a_newer_manifest_reports_an_available_update() {
    let server = server_with(NEWER_MANIFEST);

    let result =
        check_for_updates(&server.url("/latest.json"), "0.1.0").expect("update check succeeds");

    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(result.current_version, "0.1.0");
    assert_eq!(result.version, "0.2.0");
    assert_eq!(result.notes.as_deref(), Some("新增批量导出。"));
    assert_eq!(result.download_url, "https://example.com/pdfx/download");
    assert_eq!(server.request_count(), 1);
}

#[test]
fn the_same_or_an_older_manifest_reports_that_the_app_is_current() {
    for version in ["0.1.0", "0.0.9"] {
        let server = server_with(format!(
            r#"{{"version":"{version}","downloadUrl":"https://example.com/pdfx/download"}}"#
        ));

        let result =
            check_for_updates(&server.url("/latest.json"), "0.1.0").expect("update check succeeds");

        assert_eq!(
            result.status,
            UpdateStatus::UpToDate,
            "version {version} must not be reported as newer"
        );
        assert_eq!(result.version, version);
        assert_eq!(result.notes, None);
    }
}

#[test]
fn a_manifest_without_notes_is_valid() {
    let server =
        server_with(r#"{"version":"1.0.0","downloadUrl":"https://example.com/pdfx/download"}"#);

    let result =
        check_for_updates(&server.url("/latest.json"), "0.1.0").expect("update check succeeds");

    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(result.notes, None);
}

#[test]
fn server_errors_are_retryable_failures_not_manifests() {
    let server = TestServer::start(Response::Status {
        code: 500,
        body: "boom".to_owned(),
    });

    let error = check_for_updates(&server.url("/latest.json"), "0.1.0")
        .expect_err("a 500 response is a failure");

    assert!(
        matches!(error, UpdateError::Http(_)),
        "unexpected error: {error:?}"
    );
    assert_eq!(server.request_count(), 1);
}

#[test]
fn malformed_json_is_reported_as_an_invalid_manifest() {
    let server = server_with("{ this is not json");

    let error = check_for_updates(&server.url("/latest.json"), "0.1.0")
        .expect_err("malformed JSON is not a manifest");

    assert!(
        matches!(error, UpdateError::ManifestInvalid(_)),
        "unexpected error: {error:?}"
    );
}

#[test]
fn manifests_with_invalid_versions_or_download_pages_are_rejected() {
    for body in [
        r#"{"version":"not-a-version","downloadUrl":"https://example.com/download"}"#,
        r#"{"version":"1.0.0","downloadUrl":"file:///tmp/pdfx.dmg"}"#,
        r#"{"version":"1.0.0","downloadUrl":"javascript:alert(1)"}"#,
        r#"{"version":"1.0.0"}"#,
    ] {
        let server = server_with(body);

        let error = check_for_updates(&server.url("/latest.json"), "0.1.0")
            .expect_err("manifest is invalid");

        assert!(
            matches!(
                error,
                UpdateError::ManifestInvalid(_) | UpdateError::InvalidDownloadUrl(_)
            ),
            "unexpected error for {body}: {error:?}"
        );
    }
}

#[test]
fn oversized_manifests_are_rejected_without_buffering_them_whole() {
    let body = format!(
        r#"{{"version":"1.0.0","downloadUrl":"https://example.com/download","notes":"{}"}}"#,
        "x".repeat(70 * 1024)
    );
    let server = server_with(body);

    let error = check_for_updates(&server.url("/latest.json"), "0.1.0")
        .expect_err("an oversized manifest is invalid");

    assert!(
        matches!(error, UpdateError::ManifestInvalid(_)),
        "unexpected error: {error:?}"
    );
}

#[test]
fn a_non_http_manifest_address_is_rejected_before_connecting() {
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/latest.json",
        "not a url",
    ] {
        let error = check_for_updates(url, "0.1.0").expect_err("url is not fetchable");
        assert!(
            matches!(error, UpdateError::InvalidManifestUrl(_)),
            "unexpected error for {url}: {error:?}"
        );
    }
}

#[test]
fn the_check_times_out_instead_of_hanging() {
    let server = TestServer::start(Response::Stall(Duration::from_millis(500)));
    let checker = UpdateChecker::with_timeout(Duration::from_millis(100));

    let started = Instant::now();
    let error = checker
        .check(&server.url("/latest.json"), "0.1.0")
        .expect_err("a stalled server must time out");

    assert!(
        matches!(error, UpdateError::Http(_)),
        "unexpected error: {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the timeout must be bounded, took {:?}",
        started.elapsed()
    );
}

#[test]
fn one_redirect_is_followed() {
    let target = server_with(NEWER_MANIFEST);
    let redirect = TestServer::start(Response::Redirect {
        location: RedirectTarget::Fixed(target.url("/latest.json")),
    });

    let result =
        check_for_updates(&redirect.url("/latest.json"), "0.1.0").expect("redirect is followed");

    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(target.request_count(), 1);
}

#[test]
fn redirect_loops_stop_after_a_few_hops() {
    let server = TestServer::start(Response::Redirect {
        location: RedirectTarget::SelfAddress,
    });

    let started = Instant::now();
    let error = check_for_updates(&server.url("/latest.json"), "0.1.0")
        .expect_err("a redirect loop must fail");

    assert!(
        matches!(error, UpdateError::Http(_)),
        "unexpected error: {error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let requests = server.request_count();
    assert!(
        (2..=6).contains(&requests),
        "the redirect budget must be small, saw {requests} requests"
    );
}

#[test]
fn an_unset_address_is_not_checked() {
    let error = check_for_updates("   ", "0.1.0").expect_err("unset is not checkable");

    assert!(
        matches!(error, UpdateError::NotConfigured),
        "unexpected error: {error:?}"
    );
}

struct RecordingOpener {
    opened: Mutex<Vec<String>>,
    result: Result<(), String>,
}

impl RecordingOpener {
    fn new(result: Result<(), String>) -> Self {
        Self {
            opened: Mutex::new(Vec::new()),
            result,
        }
    }

    fn opened(&self) -> Vec<String> {
        self.opened.lock().expect("opener lock").clone()
    }
}

impl DownloadPageOpener for RecordingOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        self.opened
            .lock()
            .expect("opener lock")
            .push(url.to_owned());
        self.result.clone()
    }
}

#[test]
fn the_download_page_is_handed_to_the_system_browser() {
    let opener = RecordingOpener::new(Ok(()));

    open_download_page(&opener, "https://example.com/pdfx/download")
        .expect("the system opener succeeds");

    assert_eq!(
        opener.opened(),
        vec!["https://example.com/pdfx/download".to_owned()]
    );
}

#[test]
fn unsafe_download_addresses_never_reach_the_system_browser() {
    let opener = RecordingOpener::new(Ok(()));

    for url in [
        "file:///tmp/pdfx.dmg",
        "javascript:alert(1)",
        "not a url",
        "",
        "https://user:secret@example.com/download",
    ] {
        let error = open_download_page(&opener, url).expect_err("url is unsafe");
        assert!(
            matches!(error, UpdateError::InvalidDownloadUrl(_)),
            "unexpected error for {url}: {error:?}"
        );
    }

    assert!(
        opener.opened().is_empty(),
        "the system browser must only receive validated http(s) URLs"
    );
}

#[test]
fn a_failing_system_opener_is_reported_instead_of_silently_ignored() {
    let opener = RecordingOpener::new(Err("no browser registered".to_owned()));

    let error = open_download_page(&opener, "https://example.com/pdfx/download")
        .expect_err("the failure must reach the caller");

    match error {
        UpdateError::OpenFailed(message) => assert_eq!(message, "no browser registered"),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn loading_settings_never_connects_and_the_check_uses_the_persisted_address() {
    let server = server_with(NEWER_MANIFEST);
    let dir = TempDir::new("update-check-settings");
    let store = SettingsStore::new(dir.path());
    apply_update(
        &store,
        &NoopPlatform,
        Settings {
            update_manifest_url: server.url("/latest.json"),
            ..Settings::default()
        },
    )
    .expect("persist the manifest address");

    let settings = store.load();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        server.request_count(),
        0,
        "loading settings must not touch the network"
    );

    let result =
        check_for_updates(&settings.update_manifest_url, "0.1.0").expect("update check succeeds");
    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(server.request_count(), 1);
}
