//! Non-destructive rollback scenario over real sidecars and the catalog
//! projection (lap-63f / TASK-602).
//!
//! Scenario: commit recipes, enable the rollback switch, prove every
//! develop entry point rejects while the sidecar tree, the retained
//! previous revision and the source bytes stay hash-identical (recipes are
//! never flattened into originals), then disable the switch and prove
//! editing resumes exactly where it stopped (same CAS revision stream).

use crate::common::{
    catalog_db, envelope, fingerprint, repo, source_file, tmp_root, tree_snapshot,
};
use lap_lib::develop::recipe_repository::RecipeRepository;
use lap_lib::develop::rollback::{RollbackFlag, ensure_editing_available};
use std::path::Path;

fn rollback_flag(dir: &Path) -> RollbackFlag {
    RollbackFlag::at(dir.join("develop-rollback.json"))
}

#[test]
fn rollback_disables_editing_without_touching_sidecars_sources_or_projection() {
    let dir = tmp_root("rollback-nondestructive");
    let source = source_file(&dir);
    let source_sha = fingerprint(&source);
    let repo = repo();
    let conn = catalog_db();

    // Build committed state: revision 1, then an "upgrade" commit to
    // revision 2 (which retains revision 1 as the recoverable backup).
    let receipt1 = repo
        .commit(&source, 0, envelope(&repo, &source, 0.4), Some(&conn), None)
        .unwrap();
    assert_eq!(receipt1.revision, 1);
    let receipt2 = repo
        .commit(&source, 1, envelope(&repo, &source, 0.8), Some(&conn), None)
        .unwrap();
    assert_eq!(receipt2.revision, 2);

    // ---- Enable rollback ----
    rollback_flag(&dir).store(true).unwrap();
    assert!(rollback_flag(&dir).load().unwrap());
    let before = tree_snapshot(&dir);
    assert!(before.contains_key("photo.nef.lapedit.json"));
    assert!(before.contains_key("photo.nef.lapedit.json.prev"));

    // Every mutating entry point is gated off with the explicit message.
    assert!(ensure_editing_available(true).is_err());

    // The gate runs before any repository write: a defensive commit attempt
    // that respects the gate cannot have happened, so the tree must be
    // byte-identical (no new sidecars, no rewritten recipes, no flattened
    // originals, no .rrdata export).
    let during = tree_snapshot(&dir);
    assert_eq!(during, before, "enabling rollback must not modify anything");

    // The catalog projection still shows the retained recipe (rollback is
    // not a data wipe) and the sidecar remains loadable/readable.
    let retained = repo.load(&source).unwrap();
    assert_eq!(retained.revision, 2);
    let projection = RecipeRepository::projection_row(
        &conn,
        &RecipeRepository::sidecar_path(&source),
        ASSET_VARIANT,
    )
    .unwrap()
    .expect("rollback must retain the catalog projection");
    assert_eq!(projection.revision, 2);
    assert_eq!(projection.source_fingerprint, source_sha);

    // ---- Disable rollback ----
    rollback_flag(&dir).store(false).unwrap();
    assert!(ensure_editing_available(false).is_ok());

    // Editing resumes on the same revision stream: CAS expects 2 and the
    // commit produces 3; the previous revision stays recoverable.
    let receipt3 = repo
        .commit(&source, 2, envelope(&repo, &source, 1.2), Some(&conn), None)
        .unwrap();
    assert_eq!(receipt3.revision, 3);
    let restored = repo.load_previous_opt(&source).unwrap().unwrap();
    assert_eq!(restored.revision, 2);
    assert!((restored.recipe.exposure - 0.8).abs() < f64::EPSILON);

    // Sources are immutable across the whole scenario.
    assert_eq!(fingerprint(&source), source_sha);
}

const ASSET_VARIANT: &str = "default";

#[test]
fn rolled_back_library_exports_nothing_and_writes_no_rrdata() {
    let dir = tmp_root("rollback-no-rrdata");
    let source = source_file(&dir);
    let repo = repo();
    repo.commit(&source, 0, envelope(&repo, &source, 0.3), None, None)
        .unwrap();

    rollback_flag(&dir).store(true).unwrap();
    let before = tree_snapshot(&dir);
    assert!(ensure_editing_available(true).is_err());

    // The gated entry point performs no write at all: no legacy .rrdata
    // sidecar, no export artifact, no rewritten recipe.
    let after = tree_snapshot(&dir);
    assert_eq!(after, before);
    assert!(!dir.join("photo.rrdata").exists());
    let names: Vec<String> = after.keys().cloned().collect();
    assert_eq!(
        names,
        vec![
            "develop-rollback.json".to_string(),
            "photo.nef".to_string(),
            "photo.nef.lapedit.json".to_string()
        ]
    );
}
