//! Command-boundary tests for encrypted PDFs and permission restrictions.
//!
//! These exercise the same `open_document` function the Tauri IPC layer exposes,
//! so the frontend and the tests share one seam. They require the pinned PDFium
//! library (`npm run setup:native`).

mod support;

use pdfx_lib::commands::{
    close_document, open_document, render_page, CommandErrorCode, DocumentPermissions, DocumentView,
};
use support::fixtures;

fn fixture_path(id: &str) -> String {
    let manifest = fixtures::load_manifest();
    let fixture = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.id == id)
        .unwrap_or_else(|| panic!("fixture '{id}' is missing from the manifest"));
    fixtures::fixture_path(fixture)
        .to_string_lossy()
        .into_owned()
}

fn unrestricted_permissions() -> DocumentPermissions {
    DocumentPermissions {
        can_modify_content: true,
        can_annotate: true,
        can_assemble: true,
        can_fill_forms: true,
        can_print: true,
        can_copy_content: true,
    }
}

#[test]
fn unencrypted_documents_report_full_permissions_and_are_editable() {
    let view = open_document(fixture_path("text-latin"), None).expect("open text fixture");

    assert!(!view.encrypted, "plain PDF must not report encryption");
    assert!(!view.read_only, "plain PDF must be editable");
    assert_eq!(view.permissions, unrestricted_permissions());

    close_document(view.document_id).expect("document should close");
}

#[test]
fn owner_restricted_document_is_read_only_and_reports_each_denied_capability() {
    let view = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("the empty user password should open the restricted fixture");

    assert!(view.encrypted, "the restricted document is still encrypted");
    assert!(
        view.read_only,
        "restrictions that deny every editing capability must report read-only"
    );
    assert_eq!(
        view.permissions,
        DocumentPermissions {
            can_modify_content: false,
            can_annotate: false,
            can_assemble: false,
            can_fill_forms: false,
            // The fixture is R2/V1: printing is allowed and has no quality split.
            can_print: true,
            can_copy_content: false,
        }
    );

    close_document(view.document_id).expect("document should close");
}

#[test]
fn user_password_document_reports_which_password_step_failed() {
    let path = fixture_path("encrypted-user-password");

    let error = open_document(path.clone(), None).expect_err("a missing password cannot open");
    assert_eq!(error.code, CommandErrorCode::PasswordRequired);

    let error =
        open_document(path.clone(), Some(String::new())).expect_err("an empty password is wrong");
    assert_eq!(error.code, CommandErrorCode::InvalidPassword);

    let error =
        open_document(path, Some("not-the-password".to_owned())).expect_err("a wrong password");
    assert_eq!(error.code, CommandErrorCode::InvalidPassword);
}

#[test]
fn user_password_document_opens_with_the_user_password_and_stays_editable() {
    let view = open_document(
        fixture_path("encrypted-user-password"),
        Some("pdfx-user".to_owned()),
    )
    .expect("the documented user password should open the fixture");

    assert!(view.encrypted, "the fixture must report its encryption");
    assert!(
        !view.read_only,
        "an unrestricted encrypted document is editable"
    );
    assert_eq!(view.permissions, unrestricted_permissions());

    close_document(view.document_id).expect("document should close");
}

#[test]
fn owner_password_lifts_permission_restrictions() {
    let view = open_document(
        fixture_path("encrypted-owner-restrictions"),
        Some("pdfx-owner".to_owned()),
    )
    .expect("the owner password should open the restricted fixture");

    assert!(
        view.encrypted,
        "opening with the owner password keeps encryption"
    );
    assert!(
        !view.read_only,
        "owner authentication must grant the editing capabilities"
    );
    assert_eq!(view.permissions, unrestricted_permissions());

    close_document(view.document_id).expect("document should close");
}

#[test]
fn wrong_owner_password_is_rejected_as_invalid() {
    let error = open_document(
        fixture_path("encrypted-owner-restrictions"),
        Some("wrong-owner-password".to_owned()),
    )
    .expect_err("a wrong owner password cannot open the document");

    assert_eq!(error.code, CommandErrorCode::InvalidPassword);
}

#[test]
fn read_only_documents_still_render_for_viewing() {
    let view = open_document(fixture_path("encrypted-owner-restrictions"), None)
        .expect("the restricted fixture opens for viewing");

    let image = render_page(view.document_id.clone(), 0, 1.0)
        .expect("permission restrictions limit editing, not viewing");

    assert_eq!((image.width, image.height), (612, 792));
    assert!(!image.png_base64.is_empty());

    close_document(view.document_id).expect("document should close");
}

/// Pins the JSON shape the frontend consumes. The Tauri IPC layer serializes
/// these structs, so a renamed field would otherwise only surface in the UI.
#[test]
fn document_view_serializes_encryption_and_permissions_for_the_frontend() {
    let view = DocumentView {
        document_id: "doc-1".to_owned(),
        name: "secret.pdf".to_owned(),
        path: "/tmp/secret.pdf".to_owned(),
        page_count: 1,
        page_width: 612.0,
        page_height: 792.0,
        encrypted: true,
        read_only: true,
        permissions: DocumentPermissions {
            can_modify_content: false,
            can_annotate: false,
            can_assemble: false,
            can_fill_forms: false,
            can_print: true,
            can_copy_content: false,
        },
        output_path: None,
        output_encrypted: false,
        recovered: false,
        signature: None,
    };

    let json = serde_json::to_value(view).expect("DocumentView must serialize");

    assert_eq!(json["encrypted"], true);
    assert_eq!(json["readOnly"], true);
    assert_eq!(json["outputPath"], serde_json::Value::Null);
    assert_eq!(json["outputEncrypted"], false);
    assert_eq!(json["recovered"], false);
    assert_eq!(json["signature"], serde_json::Value::Null);
    assert_eq!(json["permissions"]["canModifyContent"], false);
    assert_eq!(json["permissions"]["canAnnotate"], false);
    assert_eq!(json["permissions"]["canAssemble"], false);
    assert_eq!(json["permissions"]["canFillForms"], false);
    assert_eq!(json["permissions"]["canPrint"], true);
    assert_eq!(json["permissions"]["canCopyContent"], false);
}
