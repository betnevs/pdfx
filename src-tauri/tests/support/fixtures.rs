use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct FixtureManifest {
    pub version: u32,
    pub fixtures: Vec<Fixture>,
}

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub category: String,
    pub path: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub owner_password: Option<String>,
    pub page_count: u32,
    pub valid: bool,
    pub description: String,
}

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
}

pub fn load_manifest() -> FixtureManifest {
    let path = fixtures_dir().join("manifest.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()))
}

pub fn fixture_path(fixture: &Fixture) -> PathBuf {
    fixtures_dir().join(&fixture.path)
}

pub fn fixture<'a>(manifest: &'a FixtureManifest, category: &str) -> Option<&'a Fixture> {
    manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.category == category)
}
