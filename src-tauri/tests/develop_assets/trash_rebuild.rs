//! Trash/restore coherence, external rescan reconciliation and catalog
//! rebuild re-association (lap-487; spec A8).

use crate::common;

use common::{Catalog, commit_recipe, envelope_of, repo, sidecar_of, write_source};
use lap_lib::develop::asset_operations::{
    AdoptionOutcome, ReconcileFinding, adopt_catalog_identity, delete_companion_permanently,
    delete_companion_projection, reconcile_folder, trash_companion,
};
use std::fs;

#[test]
fn trash_companion_removes_sidecar_and_projection_rows() {
    let catalog = Catalog::open("trash_group", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-trash");
    commit_recipe(&raw, "100", 0.35);

    // Simulate the acknowledged projection state.
    repo().reconcile(&catalog.conn, &raw).unwrap();
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(100));
    let durable_sidecar = sidecar_bytes_of(&raw);

    // The media was already trashed by the grouped delete; the companion goes
    // to the same trash so an OS restore brings the group back together.
    fs::remove_file(&raw).unwrap();
    let trashed = trash_companion(&raw).unwrap();
    assert!(trashed, "sidecar expected in trash");
    assert!(!sidecar_of(&raw).exists());

    delete_companion_projection(&catalog.conn, &[sidecar_of(&raw)]).unwrap();
    assert_eq!(catalog.projection_rows(&sidecar_of(&raw)), 0);

    // Simulate an OS "Put Back" restore of both members: the recipe
    // re-associates from the restored sidecar.
    fs::write(&raw, b"raw-bytes-trash").unwrap();
    fs::write(sidecar_of(&raw), durable_sidecar).unwrap();
    repo().reconcile(&catalog.conn, &raw).unwrap();
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(100));
    assert_eq!(envelope_of(&raw).recipe.exposure, 0.35);
}

#[test]
fn permanent_delete_removes_companion_without_trash() {
    let dir = common::tmp_root("permanent_delete");
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-permanent");
    commit_recipe(&raw, "100", 0.0);
    fs::remove_file(&raw).unwrap();

    let deleted = delete_companion_permanently(&raw).unwrap();
    assert!(deleted);
    assert!(!sidecar_of(&raw).exists());

    // A second call reports that nothing was present.
    assert!(!delete_companion_permanently(&raw).unwrap());
}

#[test]
fn catalog_rebuild_reassociates_edits_from_sidecars() {
    let catalog = Catalog::open("rebuild", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-rebuild");
    commit_recipe(&raw, "100", 0.45);
    repo().reconcile(&catalog.conn, &raw).unwrap();
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(100));

    // Delete the catalog and rebuild it: the file comes back with a NEW row id.
    catalog
        .conn
        .execute_batch("DELETE FROM adevelop_recipes; DELETE FROM afiles; DELETE FROM afolders;")
        .unwrap();
    catalog
        .conn
        .execute(
            "INSERT INTO afolders (id, album_id, path) VALUES (50, 1, ?1)",
            rusqlite::params![dir.to_string_lossy()],
        )
        .unwrap();
    catalog
        .conn
        .execute(
            "INSERT INTO afiles (id, folder_id, name) VALUES (900, 50, ?1)",
            rusqlite::params!["photo.nef"],
        )
        .unwrap();

    let summary = reconcile_folder(&catalog.conn, &repo(), &dir);
    assert_eq!(summary.projected, 1, "sidecar re-projects after rebuild");
    assert!(
        summary.findings.is_empty(),
        "no ambiguity or missing-media findings expected: {:?}",
        summary.findings
    );
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(900));

    // The sidecar's stale asset id re-keys to the rebuilt catalog id.
    match adopt_catalog_identity(&raw, "900").unwrap() {
        AdoptionOutcome::Adopted {
            previous_asset_id, ..
        } => {
            assert_eq!(previous_asset_id, "100");
        }
        other => panic!("expected adoption after rebuild, got {other:?}"),
    }
    assert_eq!(envelope_of(&raw).recipe.exposure, 0.45);
}

#[test]
fn reconcile_surfaces_missing_media_without_losing_the_sidecar() {
    let catalog = Catalog::open("missing_media", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-missing");
    commit_recipe(&raw, "100", 0.15);

    // External removal of the media; the sidecar remains on disk.
    fs::remove_file(&raw).unwrap();

    let summary = reconcile_folder(&catalog.conn, &repo(), &dir);
    assert_eq!(summary.sidecars_seen, 1);
    assert!(
        summary
            .findings
            .iter()
            .any(|(path, finding)| path == &sidecar_of(&raw)
                && matches!(finding, ReconcileFinding::MediaMissing)),
        "missing media must be surfaced: {:?}",
        summary.findings
    );
    assert!(
        sidecar_of(&raw).exists(),
        "the sidecar is durable until the user decides otherwise"
    );
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), None);

    // Media returns (external move back): reconcile re-associates unambiguously.
    fs::write(&raw, b"raw-bytes-missing").unwrap();
    let summary = reconcile_folder(&catalog.conn, &repo(), &dir);
    assert_eq!(summary.projected, 1);
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(100));
}

#[test]
fn reconcile_surfaces_ambiguous_catalog_matches_without_arbitrary_association() {
    // Catalog rows hold two folder spellings that differ only by case (both
    // historically imported). The physical sidecar is queried through a third
    // case spelling, so the exact lookup misses and the case-insensitive
    // fallback matches two rows: the strict resolver must surface the
    // ambiguity instead of attaching the recipe to an arbitrary row.
    let catalog = Catalog::open("ambiguous", &["docs"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-ambiguous");
    commit_recipe(&raw, "100", 0.2);

    let variant_parent = dir.parent().unwrap().join("DOCS");
    catalog
        .conn
        .execute(
            "INSERT INTO afolders (id, album_id, path) VALUES (40, 1, ?1)",
            rusqlite::params![variant_parent.to_string_lossy()],
        )
        .unwrap();
    catalog
        .conn
        .execute(
            "INSERT INTO afiles (id, folder_id, name) VALUES (800, 40, ?1)",
            rusqlite::params!["photo.nef"],
        )
        .unwrap();

    // A third case spelling of the same physical directory.
    let queried_dir = dir.parent().unwrap().join("Docs");
    let queried_raw = queried_dir.join("photo.nef");
    assert_ne!(
        queried_dir.to_string_lossy(),
        dir.to_string_lossy(),
        "the queried path must differ textually from the catalog row"
    );

    let summary = reconcile_folder(&catalog.conn, &repo(), &queried_dir);
    assert_eq!(summary.sidecars_seen, 1);
    assert!(
        summary
            .findings
            .iter()
            .any(|(_, finding)| matches!(finding, ReconcileFinding::AmbiguousCatalogMatch { .. })),
        "ambiguity must be surfaced: {:?}",
        summary.findings
    );
    assert_eq!(
        catalog.projection_file_id(&sidecar_of(&queried_raw)),
        None,
        "an ambiguous match must not associate to an arbitrary row"
    );
    assert!(
        sidecar_of(&raw).exists(),
        "the sidecar stays untouched for explicit resolution"
    );
}

#[test]
fn per_path_projection_cleanup_removes_rows_without_sidecars() {
    let catalog = Catalog::open("sidecar_gone", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-gone");
    commit_recipe(&raw, "100", 0.05);
    repo().reconcile(&catalog.conn, &raw).unwrap();
    assert_eq!(catalog.projection_rows(&sidecar_of(&raw)), 1);

    fs::remove_file(sidecar_of(&raw)).unwrap();
    // A walk that sees no sidecar cannot clean the orphan row; explicit
    // per-path maintenance (used by the delete commands) does.
    delete_companion_projection(&catalog.conn, &[sidecar_of(&raw)]).unwrap();
    assert_eq!(catalog.projection_rows(&sidecar_of(&raw)), 0);
}

#[test]
fn reconciliation_projection_matches_commit_projection_facts() {
    // The projection written by reconcile carries the same durable facts the
    // commit path projects, so either order produces the same row.
    let catalog = Catalog::open("projection_parity", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-parity");
    let revision = commit_recipe(&raw, "100", 0.75);
    let envelope = envelope_of(&raw);

    reconcile_folder(&catalog.conn, &repo(), &dir);
    let (row_revision, row_fingerprint, row_hash, row_edited): (i64, String, String, i64) = catalog
        .conn
        .query_row(
            "SELECT revision, source_fingerprint, content_hash, is_edited
             FROM adevelop_recipes WHERE sidecar_path = ?1",
            rusqlite::params![sidecar_of(&raw).to_string_lossy()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(row_revision, revision as i64);
    assert_eq!(row_fingerprint, envelope.source_fingerprint);
    assert_eq!(row_hash, envelope.content_hash().unwrap());
    assert_eq!(row_edited, 1);
}

fn sidecar_bytes_of(member: &std::path::Path) -> Vec<u8> {
    fs::read(sidecar_of(member)).unwrap()
}
