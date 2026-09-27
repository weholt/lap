use lap_lib::develop::rrdata_import::{RrdataImportError, RrdataImporter};
use rapidraw_edit_model::migrate::EnvelopeIdentity;
use rapidraw_edit_model::sha256_hex;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ASSET: &str = "asset-A";
pub const VARIANT: &str = "default";
/// The pinned producer's default source schema version (RapidRAW
/// `ImageMetadata.version`, revision 5e30bcbb246395d391ba2e9662510641ffe68e6b).
pub const SOURCE_SCHEMA_VERSION: u32 = 1;

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/rrdata")
}

pub fn fixture(name: &str) -> PathBuf {
    fixture_dir().join(name)
}

pub fn tmp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lap_rrdata_import_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Decoded original (unoriented) dimensions used for crop conversions in the
/// tests; the edited fixture's pixel crop is authored against 4000x3000.
pub fn source_dimensions() -> (u64, u64) {
    (4000, 3000)
}

pub fn fingerprint() -> String {
    "a".repeat(64)
}

pub fn engine_version() -> String {
    "lap/test/rapidraw-edit-model/0.1.0".to_string()
}

pub fn identity() -> EnvelopeIdentity {
    EnvelopeIdentity {
        engine_version: engine_version(),
        asset_id: ASSET.to_string(),
        variant_id: VARIANT.to_string(),
        source_fingerprint: fingerprint(),
    }
}

pub fn importer() -> RrdataImporter {
    RrdataImporter::new(engine_version())
}

pub fn file_sha256(path: &Path) -> String {
    sha256_hex(&fs::read(path).unwrap())
}

pub fn expect_corrupt(err: RrdataImportError, needle: &str) {
    match err {
        RrdataImportError::Corrupt { detail, .. } => {
            assert!(
                detail.to_lowercase().contains(&needle.to_lowercase()),
                "corrupt-document detail '{detail}' should mention '{needle}'"
            );
        }
        other => panic!("expected a corrupt-document outcome mentioning '{needle}', got {other:?}"),
    }
}

/// Copies a fixture into a scratch directory so hash-before/hash-after
/// assertions never read through a path another test could touch.
pub fn scratch_copy(tag: &str, fixture_name: &str) -> PathBuf {
    let dir = tmp_root(tag);
    let destination = dir.join(fixture_name);
    fs::copy(fixture(fixture_name), &destination).unwrap();
    destination
}
