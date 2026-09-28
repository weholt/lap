//! Migration / upgrade / corrupt / future-schema scenario table with
//! recipe/source hash comparison (lap-63f / TASK-602 acceptance scenario).
//!
//! These consolidate the acceptance scenarios in one place: an application
//! upgrade commit must back up the previous recipe revision; older readers
//! must reject corrupt and future-schema documents explicitly while every
//! original byte stays untouched.

use crate::common::{catalog_db, envelope, fingerprint, repo, source_file, tmp_root};
use lap_lib::develop::recipe_repository::{RecipeRepoError, RecipeRepository};
use rapidraw_edit_model::SCHEMA_VERSION;
use std::fs;

#[test]
fn upgrade_commit_backs_up_the_previous_recipe_revision() {
    let dir = tmp_root("migration-upgrade");
    let source = source_file(&dir);
    let source_sha = fingerprint(&source);
    let repo = repo();
    let conn = catalog_db();

    let first = repo
        .commit(
            &source,
            0,
            envelope(&repo, &source, 0.25),
            Some(&conn),
            None,
        )
        .unwrap();
    let first_bytes = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

    // The "upgrade": a newer application version rewrites the recipe at the
    // current schema. The previous revision must remain recoverable.
    let second = repo
        .commit(
            &source,
            1,
            envelope(&repo, &source, 0.75),
            Some(&conn),
            None,
        )
        .unwrap();
    assert_eq!(second.previous_revision, Some(1));

    let backup_bytes = fs::read(RecipeRepository::previous_sidecar_path(&source)).unwrap();
    assert_eq!(backup_bytes, first_bytes, "backup must be byte-identical");

    let restored = repo.load_previous_opt(&source).unwrap().unwrap();
    assert_eq!(restored.revision, first.revision);
    assert_eq!(restored.schema_version, SCHEMA_VERSION);
    assert!((restored.recipe.exposure - 0.25).abs() < f64::EPSILON);
    assert_eq!(fingerprint(&source), source_sha);
}

#[test]
fn corrupt_recipe_is_rejected_without_resetting_or_rewriting() {
    let dir = tmp_root("migration-corrupt");
    let source = source_file(&dir);
    let sidecar = RecipeRepository::sidecar_path(&source);
    let garbage = b"{ broken json for lap-63f".to_vec();
    fs::write(&sidecar, &garbage).unwrap();
    let source_sha = fingerprint(&source);

    assert!(matches!(
        repo().load_opt(&source),
        Err(RecipeRepoError::CorruptSidecar { .. })
    ));

    // Explicitly untouched: same bytes, no backup invention, no reset.
    assert_eq!(fs::read(&sidecar).unwrap(), garbage);
    assert!(!RecipeRepository::previous_sidecar_path(&source).exists());
    assert_eq!(fingerprint(&source), source_sha);
}

#[test]
fn future_schema_is_rejected_with_payload_preserved_for_older_versions() {
    let dir = tmp_root("migration-future");
    let source = source_file(&dir);
    let sidecar = RecipeRepository::sidecar_path(&source);
    let future_version = SCHEMA_VERSION + 1;
    let future_json = format!(
        "{{\"schemaVersion\":{future_version},\"assetId\":\"asset-A\",\"variantId\":\"default\",\"revision\":4,\"sourceFingerprint\":\"{}\",\"exposure\":1}}",
        fingerprint(&source)
    );
    fs::write(&sidecar, future_json.as_bytes()).unwrap();
    let source_sha = fingerprint(&source);

    match repo().load_opt(&source) {
        Err(RecipeRepoError::UnsupportedSchema {
            found,
            supported_max,
            preserved,
            ..
        }) => {
            assert_eq!(found, Some(future_version));
            assert_eq!(supported_max, SCHEMA_VERSION);
            assert_eq!(preserved["variantId"], "default");
            assert_eq!(preserved["revision"], 4);
        }
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }

    // The future document survives untouched so a newer version can read it.
    assert_eq!(fs::read(&sidecar).unwrap(), future_json.as_bytes());
    assert_eq!(fingerprint(&source), source_sha);
}

#[test]
fn recipe_and_source_hashes_survive_a_full_scenario_round_trip() {
    let dir = tmp_root("migration-hashes");
    let source = source_file(&dir);
    let repo = repo();
    let source_sha = fingerprint(&source);

    let receipt = repo
        .commit(&source, 0, envelope(&repo, &source, 0.5), None, None)
        .unwrap();
    let sidecar_bytes = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

    let reloaded = repo.load(&source).unwrap();
    assert_eq!(reloaded.content_hash().unwrap(), receipt.content_hash);
    assert_eq!(
        sha256_of(&sidecar_bytes),
        sha256_of(&fs::read(RecipeRepository::sidecar_path(&source)).unwrap())
    );
    assert_eq!(fingerprint(&source), source_sha);
}

fn sha256_of(bytes: &[u8]) -> String {
    rapidraw_edit_model::sha256_hex(bytes)
}
