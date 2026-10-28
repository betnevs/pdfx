mod support;

use support::fixtures;

const REQUIRED_CATEGORIES: [&str; 8] = [
    "text",
    "scanned",
    "encrypted",
    "form",
    "structure",
    "large",
    "rotated",
    "malformed",
];

#[test]
fn manifest_covers_every_required_fixture_category() {
    let manifest = fixtures::load_manifest();
    assert_eq!(manifest.version, 1, "unsupported fixture manifest version");

    for category in REQUIRED_CATEGORIES {
        assert!(
            manifest.fixtures.iter().any(|f| f.category == category),
            "fixture manifest is missing the '{category}' category"
        );
    }

    for fixture in &manifest.fixtures {
        assert!(!fixture.id.is_empty(), "fixture id must not be empty");
        assert!(fixture.page_count >= 1, "{} has no pages", fixture.id);
        assert!(
            !fixture.description.is_empty(),
            "{} has no description",
            fixture.id
        );
    }
}

#[test]
fn every_fixture_points_to_a_pdf_file() {
    let manifest = fixtures::load_manifest();

    for fixture in &manifest.fixtures {
        let path = fixtures::fixture_path(fixture);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));

        assert!(!bytes.is_empty(), "{} is empty", path.display());
        assert!(
            bytes.starts_with(b"%PDF-"),
            "{} does not start with a PDF header",
            path.display()
        );
        if fixture.valid {
            assert!(
                bytes.windows(5).any(|window| window == b"%%EOF"),
                "valid fixture {} is missing its %%EOF marker",
                path.display()
            );
        }
    }
}

#[test]
fn the_loader_finds_the_encrypted_fixture_by_category() {
    let manifest = fixtures::load_manifest();
    let encrypted = fixtures::fixture(&manifest, "encrypted").expect("encrypted fixture");

    assert!(
        encrypted.password.is_some(),
        "encrypted fixture must declare a user password"
    );
    assert!(encrypted.valid);
}

#[test]
fn encrypted_fixtures_cover_user_and_owner_password_cases() {
    let manifest = fixtures::load_manifest();
    let encrypted: Vec<_> = manifest
        .fixtures
        .iter()
        .filter(|fixture| fixture.category == "encrypted")
        .collect();

    assert!(
        encrypted
            .iter()
            .any(|fixture| fixture.password.as_deref() == Some("pdfx-user")),
        "missing encrypted fixture that requires a non-empty user password"
    );
    assert!(
        encrypted.iter().any(|fixture| {
            fixture.password.as_deref() == Some("")
                && fixture.owner_password.as_deref() == Some("pdfx-owner")
        }),
        "missing encrypted fixture that restricts permissions with an owner password"
    );
}
