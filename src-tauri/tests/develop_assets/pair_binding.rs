//! Same-basename RAW/JPEG pair binding (lap-487; spec: "Resolve RAW/JPEG
//! pairs to the explicitly selected RAW member; never attach its recipe to
//! whichever companion is displayed").

use crate::common;

use common::{Catalog, commit_recipe, envelope_of, sidecar_of, write_source};
use lap_lib::develop::asset_operations::{
    OperationJournal, companion_move, execute_companion_moves, migrate_companion_projection,
};

/// Group rename of a RAW+JPEG pair where only the RAW was ever developed:
/// the recipe must follow the RAW member, never the JPEG companion.
#[test]
fn pair_rename_binds_recipe_to_the_selected_raw_member() {
    let catalog = Catalog::open(
        "pair_raw_only",
        &["library"],
        &[(0, 100, "photo.nef"), (0, 101, "photo.jpg")],
    );
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-member-bytes");
    let jpeg = write_source(&dir, "photo.jpg", b"jpeg-member-bytes");
    commit_recipe(&raw, "100", 0.6);
    crate::common::repo()
        .reconcile(&catalog.conn, &raw)
        .unwrap();

    // Grouped rename: both members move to a new stem (like Lap's grouped
    // rename does for Live Photo / RAW+JPEG components).
    let raw_target = dir.join("trip.nef");
    let jpeg_target = dir.join("trip.jpg");
    let moves: Vec<_> = [
        companion_move(&raw, &raw_target),
        companion_move(&jpeg, &jpeg_target),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert_eq!(
        moves.len(),
        1,
        "only the developed RAW member contributes a companion move"
    );
    assert_eq!(moves[0].to, sidecar_of(&raw_target));

    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &moves).unwrap();
    journal.rename(&raw, &raw_target).unwrap();
    journal.rename(&jpeg, &jpeg_target).unwrap();
    migrate_companion_projection(&catalog.conn, &sidecar_of(&raw), &sidecar_of(&raw_target))
        .unwrap();

    // The recipe landed on the RAW member only.
    assert!(sidecar_of(&raw_target).exists());
    assert!(
        !sidecar_of(&jpeg_target).exists(),
        "the JPEG companion must not inherit the RAW recipe"
    );
    assert_eq!(envelope_of(&raw_target).asset_id, "100");
    assert_eq!(
        catalog.projection_file_id(&sidecar_of(&raw_target)),
        Some(100)
    );
}

/// Both members developed independently: each sidecar keeps following its own
/// member through a grouped rename.
#[test]
fn pair_with_two_recipes_keeps_each_member_binding() {
    let catalog = Catalog::open(
        "pair_both",
        &["library"],
        &[(0, 100, "photo.nef"), (0, 101, "photo.jpg")],
    );
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-member-bytes");
    let jpeg = write_source(&dir, "photo.jpg", b"jpeg-member-bytes");
    commit_recipe(&raw, "100", 0.6);
    commit_recipe(&jpeg, "101", -0.4);

    let raw_target = dir.join("trip.nef");
    let jpeg_target = dir.join("trip.jpg");
    let moves = [
        companion_move(&raw, &raw_target).unwrap(),
        companion_move(&jpeg, &jpeg_target).unwrap(),
    ];
    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &moves).unwrap();
    journal.rename(&raw, &raw_target).unwrap();
    journal.rename(&jpeg, &jpeg_target).unwrap();

    let raw_recipe = envelope_of(&raw_target);
    let jpeg_recipe = envelope_of(&jpeg_target);
    assert_eq!(raw_recipe.asset_id, "100");
    assert_eq!(raw_recipe.recipe.exposure, 0.6);
    assert_eq!(jpeg_recipe.asset_id, "101", "member bindings must not swap");
    assert_eq!(jpeg_recipe.recipe.exposure, -0.4);
}

/// A sidecar moved next to different same-named media is never adopted:
/// fingerprint revalidation surfaces the mismatch explicitly.
#[test]
fn sidecar_moved_onto_foreign_same_named_media_is_refused() {
    let source_dir = common::tmp_root("pair_foreign_source");
    let dest_dir = common::tmp_root("pair_foreign_dest");
    let raw = write_source(&source_dir, "photo.nef", b"edited-raw-bytes");
    commit_recipe(&raw, "100", 0.25);

    // External sidecar-only move next to a different photo with the same name.
    let foreign = write_source(&dest_dir, "photo.nef", b"different-photo-bytes");
    std::fs::rename(sidecar_of(&raw), sidecar_of(&foreign)).unwrap();

    let error =
        lap_lib::develop::asset_operations::adopt_catalog_identity(&foreign, "100").unwrap_err();
    assert!(
        matches!(
            error,
            lap_lib::develop::asset_operations::OperationError::SourceReplaced { .. }
        ),
        "foreign association must be refused, got {error:?}"
    );
    assert!(
        sidecar_of(&foreign).exists(),
        "the sidecar itself is preserved for manual recovery"
    );
}
