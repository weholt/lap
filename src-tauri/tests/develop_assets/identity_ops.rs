//! Rename/move/copy identity contracts for grouped-asset develop operations
//! (lap-487; spec A8: "Rename, move, copy ... preserve or correctly
//! re-associate edits").

use crate::common;

use common::{
    Catalog, commit_recipe, envelope_of, fingerprint, prev_sidecar_bytes, repo, sidecar_bytes,
    sidecar_of, write_source,
};
use lap_lib::develop::asset_operations::{
    AdoptionOutcome, OperationError, OperationJournal, adopt_catalog_identity, companion_move,
    execute_companion_moves, fork_sidecar_for_copy, migrate_companion_projection,
    new_copy_asset_id,
};
use std::fs;

#[test]
fn rename_moves_recipe_sidecar_and_retains_identity() {
    let catalog = Catalog::open("rename_identity", &["library"], &[(0, 100, "photo.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-rename");
    let revision = commit_recipe(&raw, "100", 0.4);
    // The acknowledged sidecar is projected in the catalog before the rename.
    crate::common::repo()
        .reconcile(&catalog.conn, &raw)
        .unwrap();
    assert_eq!(catalog.projection_file_id(&sidecar_of(&raw)), Some(100));

    let target = dir.join("vacation.nef");
    let companion = companion_move(&raw, &target).expect("sidecar companion expected");
    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &[companion]).unwrap();
    journal.rename(&raw, &target).unwrap();

    assert!(!sidecar_of(&raw).exists(), "old sidecar must not remain");
    assert!(
        sidecar_of(&target).exists(),
        "sidecar must follow the rename"
    );

    let envelope = envelope_of(&target);
    assert_eq!(envelope.asset_id, "100", "rename must retain identity");
    assert_eq!(envelope.revision, revision);
    assert_eq!(envelope.recipe.exposure, 0.4);
    assert_eq!(envelope.source_fingerprint, fingerprint(&target));

    // Projection rows follow the new sidecar path.
    migrate_companion_projection(&catalog.conn, &sidecar_of(&raw), &sidecar_of(&target)).unwrap();
    assert_eq!(catalog.projection_file_id(&sidecar_of(&target)), Some(100));
    assert_eq!(catalog.projection_rows(&sidecar_of(&raw)), 0);
}

#[test]
fn move_into_subfolder_moves_sidecar_coherently() {
    let catalog = Catalog::open(
        "move_identity",
        &["library", "library/sub"],
        &[(0, 100, "photo.nef")],
    );
    let dir = catalog.folder(0);
    let sub = catalog.folder(1);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-move");
    commit_recipe(&raw, "100", -0.2);

    let target = sub.join("photo.nef");
    let companion = companion_move(&raw, &target).expect("sidecar companion expected");
    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &[companion]).unwrap();
    journal.rename(&raw, &target).unwrap();

    assert!(sidecar_of(&target).exists());
    assert!(!sidecar_of(&raw).exists());
    let envelope = envelope_of(&target);
    assert_eq!(envelope.asset_id, "100");
    assert_eq!(envelope.recipe.exposure, -0.2);
}

#[test]
fn copy_forks_asset_identity_and_keeps_original_untouched() {
    let catalog = Catalog::open(
        "copy_fork",
        &["library", "library/copy"],
        &[(0, 100, "photo.nef")],
    );
    let dir = catalog.folder(0);
    let dest = catalog.folder(1);
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-copy");
    let revision = commit_recipe(&raw, "100", 0.9);
    let original_sidecar_before = sidecar_bytes(&raw);

    let copied = dest.join("photo.nef");
    fs::copy(&raw, &copied).unwrap();
    let _forked = fork_sidecar_for_copy(&raw, &copied, &new_copy_asset_id())
        .unwrap()
        .expect("source has a sidecar, so the copy must fork one");

    let copy_envelope = envelope_of(&copied);
    assert_ne!(
        copy_envelope.asset_id, "100",
        "copy must create a new asset identity"
    );
    assert!(copy_envelope.asset_id.starts_with("copied-"));
    assert_eq!(copy_envelope.revision, revision, "same initial recipe");
    assert_eq!(copy_envelope.recipe.exposure, 0.9);
    assert_eq!(copy_envelope.source_fingerprint, fingerprint(&copied));

    assert_eq!(
        sidecar_bytes(&raw),
        original_sidecar_before,
        "the original sidecar must stay untouched"
    );
    assert_eq!(envelope_of(&raw).asset_id, "100");
}

#[test]
fn adopt_rekeys_forked_copy_to_catalog_identity_preserving_revision() {
    let dir = common::tmp_root("adopt_copy");
    let raw = write_source(&dir, "photo.nef", b"raw-bytes-adopt");
    let revision = commit_recipe(&raw, "copied-abc", 0.7);

    let outcome = adopt_catalog_identity(&raw, "200").unwrap();
    match outcome {
        AdoptionOutcome::Adopted {
            previous_asset_id,
            revision: adopted_revision,
        } => {
            assert_eq!(previous_asset_id, "copied-abc");
            assert_eq!(adopted_revision, revision, "adoption preserves revision");
        }
        other => panic!("expected adoption, got {other:?}"),
    }

    let envelope = envelope_of(&raw);
    assert_eq!(envelope.asset_id, "200");
    assert_eq!(envelope.recipe.exposure, 0.7);

    // The previous revision is retained for recovery, still under the old id.
    let previous = prev_sidecar_bytes(&raw).expect("previous sidecar retained");
    let text = String::from_utf8(previous).unwrap();
    assert!(text.contains("copied-abc"));

    // A second adoption to the same id is a no-op.
    assert!(matches!(
        adopt_catalog_identity(&raw, "200").unwrap(),
        AdoptionOutcome::Unchanged { .. }
    ));
}

#[test]
fn adoption_refuses_replaced_media_and_leaves_the_sidecar_untouched() {
    let dir = common::tmp_root("adopt_replaced");
    let raw = write_source(&dir, "photo.nef", b"original-raw-bytes");
    commit_recipe(&raw, "100", 0.3);
    let sidecar_before = sidecar_bytes(&raw);

    // The media at the same path is replaced with different bytes.
    fs::write(&raw, b"replacement-bytes").unwrap();

    let error = adopt_catalog_identity(&raw, "100").unwrap_err();
    assert!(
        matches!(error, OperationError::SourceReplaced { .. }),
        "replacement must be detected, got {error:?}"
    );
    assert_eq!(
        sidecar_bytes(&raw),
        sidecar_before,
        "a refused adoption must not touch the durable sidecar"
    );
}

#[test]
fn companion_move_is_absent_without_a_sidecar() {
    let dir = common::tmp_root("companion_absent");
    let raw = write_source(&dir, "photo.nef", b"unedited-raw");
    let target = dir.join("renamed.nef");
    assert!(companion_move(&raw, &target).is_none());

    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &[]).unwrap();
    assert!(journal.is_empty());
    let summary = journal.rollback();
    assert_eq!(summary.reverted, 0);
    assert!(summary.failed.is_empty());
}

#[test]
fn partial_companion_failure_rolls_back_completed_moves() {
    let dir = common::tmp_root("companion_partial");
    let first = write_source(&dir, "one.nef", b"first");
    let second = write_source(&dir, "two.nef", b"second");
    commit_recipe(&first, "10", 0.1);
    commit_recipe(&second, "20", 0.2);

    let first_target = dir.join("one-moved.nef");
    let second_target = dir.join("two-moved.nef");
    // The second companion's destination already exists: the group operation
    // must abort and restore the first completed companion move.
    let _ = repo(); // keep repo in scope parity with other tests
    let conflicting = sidecar_of(&second_target);
    fs::write(&conflicting, b"stale destination sidecar").unwrap();

    let moves = [
        companion_move(&first, &first_target).unwrap(),
        companion_move(&second, &second_target).unwrap(),
    ];
    let mut journal = OperationJournal::new();
    let error = execute_companion_moves(&mut journal, &moves).unwrap_err();
    assert!(
        matches!(error, OperationError::DestinationExists { .. }),
        "explicit conflict required, got {error:?}"
    );

    let summary = journal.rollback();
    assert_eq!(
        summary.reverted, 1,
        "the first completed rename is reverted"
    );
    assert!(summary.failed.is_empty());
    assert!(sidecar_of(&first).exists(), "data loss is not acceptable");
    assert!(!sidecar_of(&first_target).exists());
    assert!(sidecar_of(&second).exists());
}

#[test]
fn unicode_paths_survive_rename_and_copy_fork() {
    let catalog = Catalog::open("unicode_ops", &["bibliothèque"], &[(0, 100, "café ☕.nef")]);
    let dir = catalog.folder(0);
    let raw = write_source(&dir, "café ☕.nef", b"unicode-raw-bytes");
    let revision = commit_recipe(&raw, "100", 0.5);

    let renamed = dir.join("żółć ✔.nef");
    let companion = companion_move(&raw, &renamed).expect("unicode sidecar companion expected");
    let mut journal = OperationJournal::new();
    execute_companion_moves(&mut journal, &[companion]).unwrap();
    journal.rename(&raw, &renamed).unwrap();

    assert!(sidecar_of(&renamed).exists());
    let envelope = envelope_of(&renamed);
    assert_eq!(envelope.asset_id, "100");
    assert_eq!(envelope.revision, revision);
    assert_eq!(envelope.recipe.exposure, 0.5);

    let copied = dir.join("kopiëren ☕.nef");
    fs::copy(&renamed, &copied).unwrap();
    let forked = fork_sidecar_for_copy(&renamed, &copied, &new_copy_asset_id())
        .unwrap()
        .expect("fork expected");
    assert!(forked.sidecar_path.exists());
    assert_ne!(envelope_of(&copied).asset_id, "100");
    assert_eq!(
        envelope_of(&copied).source_fingerprint,
        fingerprint(&copied)
    );
}

#[test]
fn identity_is_refused_for_missing_media() {
    let dir = common::tmp_root("adopt_missing_media");
    let raw = write_source(&dir, "photo.nef", b"to-be-deleted");
    commit_recipe(&raw, "100", 0.1);
    let sidecar_before = sidecar_bytes(&raw);
    fs::remove_file(&raw).unwrap();

    let error = adopt_catalog_identity(&raw, "100").unwrap_err();
    assert!(matches!(error, OperationError::SourceMissing { .. }));
    assert_eq!(sidecar_bytes(&raw), sidecar_before, "sidecar preserved");
}

#[test]
fn companion_target_conflicts_are_detected_before_any_rename() {
    let dir = common::tmp_root("companion_preflight");
    let raw = write_source(&dir, "photo.nef", b"preflight");
    commit_recipe(&raw, "100", 0.0);
    let target = dir.join("renamed.nef");
    // Destination sidecar exists even though the destination media does not.
    fs::write(sidecar_of(&target), b"occupied").unwrap();

    let companion = companion_move(&raw, &target).unwrap();
    let mut journal = OperationJournal::new();
    let error = execute_companion_moves(&mut journal, &[companion]).unwrap_err();
    assert!(matches!(error, OperationError::DestinationExists { .. }));
    assert!(
        sidecar_of(&raw).exists(),
        "preflight failure must leave the source untouched"
    );
    assert!(journal.is_empty());
}

#[test]
fn adopt_preserves_resources_and_unsupported_payload() {
    let dir = common::tmp_root("adopt_resources");
    let raw = write_source(&dir, "photo.nef", b"resource-raw");
    let repo = repo();
    let mut envelope = repo.new_envelope("copied-res", "default", &fingerprint(&raw));
    envelope.recipe.exposure = 0.8;
    envelope.unsupported.insert(
        "legacyEffect".to_string(),
        serde_json::json!({"kind": "unknown"}),
    );
    let revision = repo.current_revision(&raw).unwrap().unwrap_or(0);
    repo.commit(&raw, revision, envelope, None, None).unwrap();

    adopt_catalog_identity(&raw, "300").unwrap();
    let adopted = envelope_of(&raw);
    assert_eq!(adopted.asset_id, "300");
    assert_eq!(adopted.recipe.exposure, 0.8);
    assert!(
        adopted.unsupported.contains_key("legacyEffect"),
        "unsupported payload must survive identity adoption"
    );
}

/// Whole-folder copies carry verbatim sidecars; each must be re-keyed to a
/// fresh unique identity so no two assets share one identity.
#[test]
fn copied_folder_sidecars_get_fresh_identities() {
    let source = common::tmp_root("folder_copy_source");
    let raw_one = write_source(&source, "one.nef", b"folder-copy-one");
    let raw_two = write_source(&source, "two.nef", b"folder-copy-two");
    commit_recipe(&raw_one, "10", 0.1);
    commit_recipe(&raw_two, "20", 0.2);

    let destination = common::tmp_root("folder_copy_dest");
    for entry in [
        "one.nef",
        "one.nef.lapedit.json",
        "two.nef",
        "two.nef.lapedit.json",
    ] {
        fs::copy(source.join(entry), destination.join(entry)).unwrap();
    }

    let forked =
        lap_lib::develop::asset_operations::fork_copied_folder_sidecars(&destination).unwrap();
    assert_eq!(forked, 2);

    let one = envelope_of(&destination.join("one.nef"));
    let two = envelope_of(&destination.join("two.nef"));
    assert_ne!(one.asset_id, "10");
    assert_ne!(two.asset_id, "20");
    assert_ne!(one.asset_id, two.asset_id, "forks must be unique");
    assert_eq!(one.recipe.exposure, 0.1);
    assert_eq!(two.recipe.exposure, 0.2);
    // The source sidecars are untouched.
    assert_eq!(envelope_of(&raw_one).asset_id, "10");
}

/// Replace-policy removals are staged through backups: rollback restores the
/// removed sidecar, commit removes the backup cleanly.
#[test]
fn journal_staged_removal_restores_on_rollback_and_cleans_on_commit() {
    let dir = common::tmp_root("journal_removal");
    let raw = write_source(&dir, "photo.nef", b"journal-remove");
    commit_recipe(&raw, "100", 0.65);
    let sidecar = sidecar_of(&raw);

    let mut journal = OperationJournal::new();
    journal.remove_file(&sidecar).unwrap();
    assert!(!sidecar.exists(), "the staged removal moved the sidecar");
    let summary = journal.rollback();
    assert_eq!(summary.reverted, 1);
    assert!(summary.failed.is_empty());
    assert!(sidecar.exists(), "rollback restores the removed sidecar");

    let mut journal = OperationJournal::new();
    journal.remove_file(&sidecar).unwrap();
    let summary = journal.commit();
    assert!(summary.failed.is_empty());
    assert!(!sidecar.exists());
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("lap-removed"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "commit must clean staged backups, found {leftovers:?}"
    );
}
