//! Command-boundary tests for settings persistence and file-association toggling.
//!
//! These call the same functions the `get_settings` / `update_settings` Tauri
//! commands wrap, with a mock file-association platform for the OS side effect.

mod support;

use std::fs;
use std::sync::Mutex;

use pdfx_lib::file_association::{AssociationResult, AssociationStatus, FileAssociationPlatform};
use pdfx_lib::settings::{
    apply_update, default_annotation_author, InterfaceLanguage, Settings, SettingsError,
    SettingsStore, Theme, SETTINGS_FILE_NAME,
};
use support::temp::TempDir;

struct MockPlatform {
    calls: Mutex<Vec<&'static str>>,
    register_result: Result<AssociationResult, String>,
    unregister_result: Result<AssociationResult, String>,
}

impl MockPlatform {
    fn user_action_required() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            register_result: Ok(AssociationResult::user_action_required(
                "请在系统设置中确认默认应用",
            )),
            unregister_result: Ok(AssociationResult::unregistered("已取消文件关联")),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("mock platform lock").clone()
    }
}

impl FileAssociationPlatform for MockPlatform {
    fn register(&self) -> Result<AssociationResult, String> {
        self.calls
            .lock()
            .expect("mock platform lock")
            .push("register");
        self.register_result.clone()
    }

    fn unregister(&self) -> Result<AssociationResult, String> {
        self.calls
            .lock()
            .expect("mock platform lock")
            .push("unregister");
        self.unregister_result.clone()
    }
}

fn store_in(dir: &TempDir) -> SettingsStore {
    SettingsStore::new(dir.path())
}

fn with_theme(theme: Theme) -> Settings {
    Settings {
        theme,
        ..Settings::default()
    }
}

fn directory_entries(dir: &TempDir) -> Vec<String> {
    let mut entries: Vec<String> = fs::read_dir(dir.path())
        .expect("read the temp directory")
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn settings_default_to_chinese_system_theme_and_file_association_off() {
    let settings = Settings::default();

    assert_eq!(settings.language, InterfaceLanguage::ZhCn);
    assert_eq!(settings.theme, Theme::System);
    assert_eq!(settings.annotation_author, default_annotation_author());
    assert!(!settings.file_association_enabled);
    assert_eq!(settings.update_manifest_url, "");
}

#[test]
fn update_manifest_url_survives_a_restart_and_is_trimmed() {
    let dir = TempDir::new("settings-update-url");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();
    let next = Settings {
        update_manifest_url: "  https://example.com/pdfx/latest.json  ".to_owned(),
        ..Settings::default()
    };

    let result = apply_update(&store, &platform, next).expect("apply update");

    assert_eq!(
        result.settings.update_manifest_url,
        "https://example.com/pdfx/latest.json"
    );
    assert_eq!(
        store.load().update_manifest_url,
        "https://example.com/pdfx/latest.json"
    );
}

#[test]
fn invalid_update_manifest_urls_are_rejected_without_writing() {
    let dir = TempDir::new("settings-update-url-invalid");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();

    for url in [
        "not a url",
        "example.com/latest.json",
        "ftp://example.com/latest.json",
        "file:///tmp/latest.json",
        "javascript:alert(1)",
        "https://",
    ] {
        let next = Settings {
            update_manifest_url: url.to_owned(),
            ..Settings::default()
        };

        let error = apply_update(&store, &platform, next).expect_err("url is invalid");
        assert!(
            matches!(error, SettingsError::InvalidArgument(_)),
            "unexpected error for {url}: {error:?}"
        );
    }

    assert!(!store.path().exists());
    assert!(platform.calls().is_empty());
}

#[test]
fn an_empty_update_manifest_url_is_allowed_and_clears_the_setting() {
    let dir = TempDir::new("settings-update-url-cleared");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();
    store
        .save(&Settings {
            update_manifest_url: "https://example.com/pdfx/latest.json".to_owned(),
            ..Settings::default()
        })
        .expect("seed settings");

    let result = apply_update(
        &store,
        &platform,
        Settings {
            update_manifest_url: "   ".to_owned(),
            ..Settings::default()
        },
    )
    .expect("apply update");

    assert_eq!(result.settings.update_manifest_url, "");
    assert_eq!(store.load().update_manifest_url, "");
}

#[test]
fn settings_survive_a_restart_through_a_new_store() {
    let dir = TempDir::new("settings-restart");
    let settings = Settings {
        language: InterfaceLanguage::EnUs,
        theme: Theme::Dark,
        annotation_author: "Alice".to_owned(),
        file_association_enabled: true,
        update_manifest_url: "https://example.com/pdfx/latest.json".to_owned(),
    };

    store_in(&dir).save(&settings).expect("save settings");

    let reloaded = SettingsStore::new(dir.path()).load();
    assert_eq!(reloaded, settings);
}

#[test]
fn settings_file_uses_the_frontend_camel_case_contract() {
    let dir = TempDir::new("settings-json");
    let store = store_in(&dir);
    let settings = Settings {
        language: InterfaceLanguage::EnUs,
        theme: Theme::Dark,
        annotation_author: "Alice".to_owned(),
        file_association_enabled: true,
        update_manifest_url: "https://example.com/pdfx/latest.json".to_owned(),
    };

    store.save(&settings).expect("save settings");

    assert_eq!(
        store.path().file_name().and_then(|name| name.to_str()),
        Some(SETTINGS_FILE_NAME)
    );
    let raw = fs::read_to_string(store.path()).expect("read settings");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("settings is JSON");
    assert_eq!(json["language"], "en-US");
    assert_eq!(json["theme"], "dark");
    assert_eq!(json["annotationAuthor"], "Alice");
    assert_eq!(json["fileAssociationEnabled"], true);
    assert_eq!(
        json["updateManifestUrl"],
        "https://example.com/pdfx/latest.json"
    );
}

#[test]
fn missing_or_corrupt_settings_files_fall_back_to_defaults() {
    let dir = TempDir::new("settings-corrupt");
    let store = store_in(&dir);

    assert_eq!(store.load(), Settings::default());

    fs::write(store.path(), "{ this is not valid json").expect("write corrupt settings");
    assert_eq!(store.load(), Settings::default());
}

#[test]
fn partial_settings_files_fill_missing_fields_from_defaults() {
    let dir = TempDir::new("settings-partial");
    let store = store_in(&dir);
    fs::write(store.path(), r#"{"theme":"light"}"#).expect("write partial settings");

    let loaded = store.load();
    assert_eq!(loaded.theme, Theme::Light);
    assert_eq!(loaded.language, InterfaceLanguage::ZhCn);
    assert_eq!(loaded.annotation_author, default_annotation_author());
    assert!(!loaded.file_association_enabled);
}

#[test]
fn saving_is_atomic_and_leaves_no_temporary_files() {
    let dir = TempDir::new("settings-atomic");
    let store = store_in(&dir);

    store.save(&with_theme(Theme::Light)).expect("first save");
    store.save(&with_theme(Theme::Dark)).expect("second save");

    assert_eq!(directory_entries(&dir), vec![SETTINGS_FILE_NAME.to_owned()]);
    assert_eq!(store.load().theme, Theme::Dark);
}

#[test]
fn updating_other_fields_does_not_touch_the_file_association_platform() {
    let dir = TempDir::new("settings-no-association");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();
    let next = Settings {
        language: InterfaceLanguage::EnUs,
        theme: Theme::Dark,
        annotation_author: "Alice".to_owned(),
        file_association_enabled: false,
        update_manifest_url: String::new(),
    };

    let result = apply_update(&store, &platform, next.clone()).expect("apply update");

    assert_eq!(result.settings, next);
    assert!(result.association.is_none());
    assert!(platform.calls().is_empty());
    assert_eq!(store.load(), next);
}

#[test]
fn enabling_file_association_registers_the_platform_and_persists_the_preference() {
    let dir = TempDir::new("settings-enable-association");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();
    let next = Settings {
        file_association_enabled: true,
        ..Settings::default()
    };

    let result = apply_update(&store, &platform, next).expect("apply update");

    assert_eq!(platform.calls(), vec!["register"]);
    let association = result.association.expect("association result");
    assert_eq!(association.status, AssociationStatus::UserActionRequired);
    assert_eq!(association.instructions, "请在系统设置中确认默认应用");
    assert!(store.load().file_association_enabled);
}

#[test]
fn disabling_file_association_unregisters_the_platform_and_persists_the_preference() {
    let dir = TempDir::new("settings-disable-association");
    let store = store_in(&dir);
    store
        .save(&Settings {
            file_association_enabled: true,
            ..Settings::default()
        })
        .expect("seed settings");
    let platform = MockPlatform::user_action_required();

    let result = apply_update(&store, &platform, Settings::default()).expect("apply update");

    assert_eq!(platform.calls(), vec!["unregister"]);
    let association = result.association.expect("association result");
    assert_eq!(association.status, AssociationStatus::Unregistered);
    assert!(!store.load().file_association_enabled);
}

#[test]
fn an_unchanged_enabled_association_is_not_registered_again() {
    let dir = TempDir::new("settings-association-unchanged");
    let store = store_in(&dir);
    let enabled = Settings {
        file_association_enabled: true,
        ..Settings::default()
    };
    store.save(&enabled).expect("seed settings");
    let platform = MockPlatform::user_action_required();

    let result = apply_update(&store, &platform, enabled).expect("apply update");

    assert!(result.association.is_none());
    assert!(platform.calls().is_empty());
}

#[test]
fn invalid_annotation_authors_are_rejected_without_writing_or_registering() {
    let dir = TempDir::new("settings-invalid-author");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();

    for author in ["   ", "line\nbreak", &"x".repeat(65)] {
        let next = Settings {
            annotation_author: author.to_owned(),
            file_association_enabled: true,
            ..Settings::default()
        };

        let error = apply_update(&store, &platform, next).expect_err("author is invalid");
        assert!(
            matches!(error, SettingsError::InvalidArgument(_)),
            "unexpected error: {error:?}"
        );
    }

    assert!(!store.path().exists());
    assert!(platform.calls().is_empty());
}

#[test]
fn annotation_authors_are_trimmed_before_they_are_persisted() {
    let dir = TempDir::new("settings-author-trim");
    let store = store_in(&dir);
    let platform = MockPlatform::user_action_required();
    let next = Settings {
        annotation_author: "  Alice  ".to_owned(),
        ..Settings::default()
    };

    let result = apply_update(&store, &platform, next).expect("apply update");

    assert_eq!(result.settings.annotation_author, "Alice");
    assert_eq!(store.load().annotation_author, "Alice");
}

#[test]
fn a_platform_failure_keeps_the_preference_and_reports_the_failure() {
    let dir = TempDir::new("settings-platform-failure");
    let store = store_in(&dir);
    let platform = MockPlatform {
        calls: Mutex::new(Vec::new()),
        register_result: Err("lsregister could not run".to_owned()),
        unregister_result: Ok(AssociationResult::unregistered("已取消文件关联")),
    };
    let next = Settings {
        file_association_enabled: true,
        ..Settings::default()
    };

    let result = apply_update(&store, &platform, next).expect("apply update");

    let association = result.association.expect("association result");
    assert_eq!(association.status, AssociationStatus::Failed);
    assert_eq!(
        association.message.as_deref(),
        Some("lsregister could not run")
    );
    assert!(store.load().file_association_enabled);
}
