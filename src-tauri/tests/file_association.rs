//! Platform boundary tests for file-association registration.
//!
//! The real OS calls are deliberately kept behind an ignored manual test: the
//! automated tests cover the honest "the OS still needs the user" outcomes and
//! the platform command construction.

use pdfx_lib::file_association::{AssociationStatus, FileAssociationPlatform, UnsupportedPlatform};

#[test]
fn unsupported_platforms_report_that_association_cannot_be_automated() {
    let platform = UnsupportedPlatform;

    let registration = platform
        .register()
        .expect("register on an unsupported platform");
    assert_eq!(registration.status, AssociationStatus::Unsupported);
    assert!(!registration.instructions.is_empty());

    let removal = platform
        .unregister()
        .expect("unregister on an unsupported platform");
    assert_eq!(removal.status, AssociationStatus::Unsupported);
}

#[cfg(target_os = "macos")]
#[test]
fn a_macos_build_without_an_app_bundle_cannot_register() {
    use pdfx_lib::file_association::MacFileAssociation;

    let platform = MacFileAssociation::new(None);

    let registration = platform.register().expect("register without a bundle");
    assert_eq!(registration.status, AssociationStatus::Unsupported);
    assert!(registration.instructions.contains("macOS"));
}

/// Manual verification of the real platform registration.
///
/// Run on the packaged app (macOS) or a Windows build with:
/// `cargo test --test file_association -- --ignored --nocapture`
#[test]
#[ignore = "manually registers PDFX with the real operating system"]
fn manual_real_platform_registration_round_trip() {
    #[cfg(target_os = "macos")]
    {
        use pdfx_lib::file_association::MacFileAssociation;
        use std::path::PathBuf;

        let Some(bundle) = std::env::var_os("PDFX_APP_BUNDLE").map(PathBuf::from) else {
            eprintln!(
                "skipping: set PDFX_APP_BUNDLE to a packaged PDFX.app to exercise real registration"
            );
            return;
        };
        let platform = MacFileAssociation::new(Some(bundle));
        let registration = platform.register().expect("register with LaunchServices");
        eprintln!("macOS registration: {registration:?}");
        let removal = platform
            .unregister()
            .expect("unregister from LaunchServices");
        eprintln!("macOS removal: {removal:?}");
    }

    #[cfg(target_os = "windows")]
    {
        use pdfx_lib::file_association::WindowsFileAssociation;

        let platform = WindowsFileAssociation::for_current_executable();
        let registration = platform.register().expect("register the per-user ProgId");
        eprintln!("Windows registration: {registration:?}");
        let removal = platform.unregister().expect("remove the per-user ProgId");
        eprintln!("Windows removal: {removal:?}");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        eprintln!("skipping: real registration is only implemented for macOS and Windows");
    }
}
