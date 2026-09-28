//! Virtual-copy (variant) lifecycle integration tests (lap-952 / TASK-503).
//!
//! Spec refs A1/A3/A8/A9: virtual copies keep their own variant identity and
//! revision while referring to the same immutable source bytes; copy/reset/
//! delete operations preserve every other variant, the projection rows of
//! other variants, and shared content-addressed resources through a catalog
//! rebuild.

use lap_lib::develop::recipe_repository::{
    ProjectionRow, RecipeRepoError, RecipeRepository, is_edited_envelope,
};
use lap_lib::develop::variants::{self, DEFAULT_VARIANT_ID, VariantError};
use rapidraw_edit_model::{ResourceAlgorithm, ResourceRef, sha256_hex};
use rusqlite::Connection;
use std::fs;
use std::path::Path;

use crate::common;

use common::{Catalog, fingerprint, repo, write_source};

/// The authoritative asset identity supplied by the host (catalog file id).
const ASSET: &str = "9001";

/// Commits an edited recipe sidecar (exposure set) for `source` and returns
/// the durable revision.
fn commit_default(source: &Path, asset_id: &str, exposure: f64) -> u64 {
    let repository = repo();
    let current = repository.current_revision(source).unwrap().unwrap_or(0);
    let mut envelope = match repository.load_opt(source).unwrap() {
        Some(existing) => existing,
        None => repository.new_envelope(asset_id, DEFAULT_VARIANT_ID, &fingerprint(source)),
    };
    envelope.recipe.exposure = exposure;
    repository
        .commit(source, current, envelope, None, None)
        .unwrap()
        .revision
}

/// Advances a variant's recipe by `exposure`, returning the new revision.
fn bump_variant(source: &Path, variant_id: &str, exposure: f64) -> u64 {
    let repository = repo();
    let envelope = repository
        .load_variant_opt(source, variant_id)
        .unwrap()
        .unwrap();
    let receipt = repository
        .commit_variant(
            source,
            variant_id,
            envelope.revision,
            {
                let mut bumped = envelope;
                bumped.recipe.exposure = exposure;
                bumped
            },
            None,
            None,
        )
        .unwrap();
    receipt.revision
}

fn projection_row(conn: &Connection, sidecar: &Path, variant_id: &str) -> Option<ProjectionRow> {
    RecipeRepository::projection_row(conn, sidecar, variant_id).unwrap()
}

// ---------------------------------------------------------------------------
// Identity and revision independence
// ---------------------------------------------------------------------------

#[test]
fn virtual_copy_gets_own_identity_and_revision_for_the_same_source() {
    let dir = common::tmp_root("vc-identity");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v1");
    let source_hash = fingerprint(&source);
    let source_bytes_before = fs::read(&source).unwrap();
    let primary_revision = commit_default(&source, "101", 0.5);
    let primary_sidecar_before = fs::read(common::sidecar_of(&source)).unwrap();

    let envelope =
        variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();

    assert!(
        envelope.variant_id.starts_with("vc-"),
        "generated virtual copy ids must be distinguishable: {}",
        envelope.variant_id
    );
    assert_ne!(
        envelope.variant_id,
        DEFAULT_VARIANT_ID.to_string(),
        "the copy must have its own variant identity"
    );
    assert_eq!(envelope.asset_id, ASSET);
    assert_eq!(envelope.source_fingerprint, source_hash);
    assert_eq!(
        envelope.revision, 1,
        "a fresh virtual copy starts at revision 1"
    );
    assert_eq!(
        envelope.recipe.exposure, 0.5,
        "the copy inherits the copied recipe"
    );

    // The copied sidecar lives beside the primary one and its name round-trips
    // to the media source and variant identity.
    let copy_sidecar = variants::variant_sidecar_path(&source, &envelope.variant_id);
    assert!(copy_sidecar.exists());
    assert_eq!(
        variants::variant_sidecar_source(&copy_sidecar).unwrap(),
        (source.clone(), envelope.variant_id.clone())
    );
    let reloaded = repo().load_opt(&source).unwrap().unwrap();
    assert_eq!(reloaded.variant_id, DEFAULT_VARIANT_ID);

    // The primary sidecar and the source bytes are untouched (A1).
    assert_eq!(
        fs::read(common::sidecar_of(&source)).unwrap(),
        primary_sidecar_before,
        "creating a virtual copy must not rewrite the primary sidecar"
    );
    assert_eq!(fs::read(&source).unwrap(), source_bytes_before);
    assert_eq!(
        repo().current_revision(&source).unwrap(),
        Some(primary_revision)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn variant_revisions_advance_independently_with_typed_conflicts() {
    let dir = common::tmp_root("vc-revisions");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v2");
    let default_revision = commit_default(&source, "202", 0.1);
    let copy = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();

    // Advancing the copy must not advance the default variant.
    assert_eq!(bump_variant(&source, &copy.variant_id, 0.9), 2);
    assert_eq!(
        repo().current_revision(&source).unwrap(),
        Some(default_revision),
        "the default variant revision must be independent"
    );
    let copy_revision_now = repo()
        .current_variant_revision(&source, &copy.variant_id)
        .unwrap()
        .unwrap();
    assert_eq!(copy_revision_now, 2);

    // Stale expected revision on the copy is a typed conflict (two-window CAS).
    let stale = repo()
        .load_variant_opt(&source, &copy.variant_id)
        .unwrap()
        .unwrap();
    let err = repo()
        .commit_variant(&source, &copy.variant_id, 1, stale, None, None)
        .expect_err("a stale revision must be rejected");
    assert!(
        matches!(
            err,
            RecipeRepoError::RevisionConflict {
                expected: 1,
                current: Some(2),
                ..
            }
        ),
        "expected a revision conflict, got {err:?}"
    );

    // Cross-variant identity swaps are rejected against the copy sidecar.
    let primary = repo().load(&source).unwrap();
    let err = repo()
        .commit_variant(&source, &copy.variant_id, 2, primary, None, None)
        .expect_err("identity swaps must be rejected");
    assert!(
        matches!(err, RecipeRepoError::IdentityMismatch { .. }),
        "expected an identity mismatch, got {err:?}"
    );
    assert_eq!(
        repo()
            .current_variant_revision(&source, &copy.variant_id)
            .unwrap(),
        Some(2),
        "rejected commits must not change the durable revision"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Preserve other variants and shared resources
// ---------------------------------------------------------------------------

#[test]
fn reset_clears_one_variant_and_preserves_the_others() {
    let dir = common::tmp_root("vc-reset");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v3");
    commit_default(&source, "303", 0.7);
    let copy_a = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();
    let copy_b = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();

    let default_sidecar_before = fs::read(common::sidecar_of(&source)).unwrap();
    let copy_a_sidecar = variants::variant_sidecar_path(&source, &copy_a.variant_id);
    let copy_a_before = fs::read(&copy_a_sidecar).unwrap();

    let bumped_revision = bump_variant(&source, &copy_b.variant_id, -0.4);

    let receipt =
        variants::reset_variant(&repo(), None, &source, &copy_b.variant_id, bumped_revision)
            .unwrap();

    assert_eq!(receipt.revision, bumped_revision + 1);
    let reset_envelope = repo()
        .load_variant_opt(&source, &copy_b.variant_id)
        .unwrap()
        .unwrap();
    assert_eq!(reset_envelope.variant_id, copy_b.variant_id);
    assert_eq!(reset_envelope.asset_id, ASSET);
    assert_eq!(
        reset_envelope.recipe.exposure, 0.0,
        "reset clears the recipe"
    );
    assert!(
        !is_edited_envelope(&reset_envelope),
        "a reset variant is no longer edited"
    );

    // Other variants and the primary sidecar are byte-identical.
    assert_eq!(fs::read(&copy_a_sidecar).unwrap(), copy_a_before);
    assert_eq!(
        fs::read(common::sidecar_of(&source)).unwrap(),
        default_sidecar_before
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn delete_variant_preserves_other_sidecars_projection_rows_and_resources() {
    let dir = common::tmp_root("vc-delete");
    let catalog = Catalog::open("vc-delete-catalog", &["lib"], &[(0, 401, "photo.arw")]);
    let folder = catalog.folder(0);
    let source = folder.join("photo.arw");
    write_source(&folder, "photo.arw", b"raw-bytes-v4");

    commit_default(&source, "401", 0.2);
    let copy_a =
        variants::create_virtual_copy(&repo(), Some(&catalog.conn), &source, ASSET, None, None)
            .unwrap();
    let copy_b =
        variants::create_virtual_copy(&repo(), Some(&catalog.conn), &source, ASSET, None, None)
            .unwrap();

    // Seed one shared resource reference on the default variant so the store
    // object is referenced by the default (virtual copies inherit references
    // when created from an edited variant).
    let mut default_envelope = repo().load(&source).unwrap();
    default_envelope.resources.insert(
        "lut/shared".to_string(),
        ResourceRef {
            algorithm: ResourceAlgorithm::Sha256,
            digest: "a".repeat(64),
            size_bytes: Some(3),
        },
    );
    let default_revision = repo().current_revision(&source).unwrap().unwrap();
    repo()
        .commit(
            &source,
            default_revision,
            default_envelope,
            Some(&catalog.conn),
            None,
        )
        .unwrap();

    let copy_b_sidecar = variants::variant_sidecar_path(&source, &copy_b.variant_id);
    assert!(copy_b_sidecar.exists());
    assert!(projection_row(&catalog.conn, &copy_b_sidecar, &copy_b.variant_id).is_some());

    let store_object = dir.join("resource-store").join("lut").join("a".repeat(64));
    fs::create_dir_all(store_object.parent().unwrap()).unwrap();
    fs::write(&store_object, b"lut").unwrap();

    variants::delete_variant(Some(&catalog.conn), &source, &copy_b.variant_id).unwrap();

    assert!(
        !copy_b_sidecar.exists(),
        "the deleted variant's sidecar must be removed"
    );
    assert!(
        projection_row(&catalog.conn, &copy_b_sidecar, &copy_b.variant_id).is_none(),
        "the deleted variant's projection row must be removed"
    );

    // Everything else survives.
    assert!(variants::variant_sidecar_path(&source, &copy_a.variant_id).exists());
    assert!(
        projection_row(
            &catalog.conn,
            &common::sidecar_of(&source),
            DEFAULT_VARIANT_ID
        )
        .is_some()
    );
    assert!(
        projection_row(
            &catalog.conn,
            &variants::variant_sidecar_path(&source, &copy_a.variant_id),
            &copy_a.variant_id
        )
        .is_some()
    );
    let default_now = repo().load(&source).unwrap();
    assert!(
        default_now.resources.contains_key("lut/shared"),
        "deleting one variant must not garbage-collect shared resource references"
    );
    assert!(
        store_object.exists(),
        "deleting a variant must never delete shared resource store objects"
    );

    // Deleting the default variant is refused explicitly.
    let err = variants::delete_variant(Some(&catalog.conn), &source, DEFAULT_VARIANT_ID)
        .expect_err("the primary variant cannot be deleted");
    assert!(
        matches!(err, VariantError::PrimaryVariantProtected),
        "expected PrimaryVariantProtected, got {err:?}"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(catalog.folder(0).parent().unwrap());
}

// ---------------------------------------------------------------------------
// Creation conflicts and input validation
// ---------------------------------------------------------------------------

#[test]
fn invalid_or_conflicting_copy_requests_fail_explicitly() {
    let dir = common::tmp_root("vc-conflicts");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v5");
    commit_default(&source, "505", 0.3);

    let first =
        variants::create_virtual_copy(&repo(), None, &source, ASSET, None, Some("vc-custom"))
            .unwrap();
    assert_eq!(first.variant_id, "vc-custom");
    let err = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, Some("vc-custom"))
        .expect_err("a duplicate virtual copy id must be rejected");
    assert!(
        matches!(err, VariantError::SidecarExists { .. }),
        "expected SidecarExists, got {err:?}"
    );

    for bad in [
        "",
        "default",
        "../escape",
        "a/b",
        "a\\b",
        "a b",
        "a.b",
        &"x".repeat(65),
    ] {
        let err = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, Some(bad))
            .expect_err("invalid ids must be rejected");
        assert!(
            matches!(err, VariantError::InvalidVariantId(_)),
            "expected InvalidVariantId for {bad:?}, got {err:?}"
        );
    }

    // Copying from a virtual variant that does not exist fails explicitly.
    let err =
        variants::create_virtual_copy(&repo(), None, &source, ASSET, Some("vc-missing"), None)
            .expect_err("copying from a missing variant must fail");
    assert!(
        matches!(err, VariantError::SidecarMissing { .. }),
        "expected SidecarMissing, got {err:?}"
    );

    // An asset without any sidecar can still get a virtual copy (fresh edit).
    let fresh = write_source(&dir, "fresh.arw", b"raw-bytes-v6");
    let fresh_copy =
        variants::create_virtual_copy(&repo(), None, &fresh, "707", None, None).unwrap();
    assert_eq!(fresh_copy.revision, 1);
    assert_eq!(
        fresh_copy.recipe.exposure, 0.0,
        "no source recipe means a default recipe"
    );
    assert!(
        !common::sidecar_of(&fresh).exists(),
        "the primary sidecar must stay absent"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Catalog rebuild
// ---------------------------------------------------------------------------

#[test]
fn catalog_rebuild_restores_variant_projection_rows() {
    let catalog = Catalog::open("vc-rebuild-catalog", &["lib"], &[(0, 601, "photo.arw")]);
    let folder = catalog.folder(0);
    let source = folder.join("photo.arw");
    write_source(&folder, "photo.arw", b"raw-bytes-v7");

    commit_default(&source, "601", 0.6);
    let copy_a =
        variants::create_virtual_copy(&repo(), Some(&catalog.conn), &source, ASSET, None, None)
            .unwrap();
    let copy_b =
        variants::create_virtual_copy(&repo(), Some(&catalog.conn), &source, ASSET, None, None)
            .unwrap();

    // Wipe the rebuildable projection entirely (catalog rebuild) and restore
    // it from the durable sidecars.
    catalog
        .conn
        .execute("DELETE FROM adevelop_recipes", [])
        .unwrap();

    let repository = repo();
    repository
        .reconcile_albums_at_startup(&catalog.conn)
        .unwrap();
    let summary = variants::reconcile_albums_at_startup(&catalog.conn);
    assert!(
        summary.errors.is_empty(),
        "reconcile errors: {:?}",
        summary.errors
    );
    assert_eq!(
        summary.projected, 2,
        "both virtual copies must be projected"
    );

    for copy in [&copy_a, &copy_b] {
        let sidecar = variants::variant_sidecar_path(&source, &copy.variant_id);
        let row = projection_row(&catalog.conn, &sidecar, &copy.variant_id)
            .expect("variant row must be restored from its sidecar");
        assert_eq!(row.revision, copy.revision);
        assert_eq!(
            row.file_id,
            Some(601),
            "the variant row must re-associate with its file"
        );
        assert_eq!(row.source_fingerprint, fingerprint(&source));
    }
    assert!(
        projection_row(
            &catalog.conn,
            &common::sidecar_of(&source),
            DEFAULT_VARIANT_ID
        )
        .is_some()
    );

    // An externally deleted variant sidecar has its stale projection row
    // removed by reconcile without touching the other variants.
    let copy_b_sidecar = variants::variant_sidecar_path(&source, &copy_b.variant_id);
    fs::remove_file(&copy_b_sidecar).unwrap();
    let summary = variants::reconcile_folder(&catalog.conn, &repository, &folder);
    assert_eq!(summary.removed_missing, 1, "the stale row must be reported");
    assert!(
        projection_row(&catalog.conn, &copy_b_sidecar, &copy_b.variant_id).is_none(),
        "the stale row must be removed"
    );
    assert!(
        projection_row(
            &catalog.conn,
            &variants::variant_sidecar_path(&source, &copy_a.variant_id),
            &copy_a.variant_id
        )
        .is_some()
    );

    let _ = fs::remove_dir_all(folder.parent().unwrap());
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[test]
fn list_variants_reports_default_first_with_lifecycle_facts() {
    let dir = common::tmp_root("vc-list");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v8");
    let revision = commit_default(&source, "808", 0.4);
    let copy = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();

    let list = variants::list_variants(&repo(), &source).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].variant_id, DEFAULT_VARIANT_ID);
    assert!(!list[0].is_virtual_copy);
    assert!(list[0].is_edited, "exposure 0.4 counts as edited");
    assert_eq!(list[0].revision, revision);
    assert_eq!(list[1].variant_id, copy.variant_id);
    assert!(list[1].is_virtual_copy);
    assert!(list[1].is_edited, "the copy inherited the edited recipe");
    assert_eq!(list[1].revision, 1);
    for entry in &list {
        assert!(!entry.content_hash.is_empty());
    }

    // An unedited asset lists just the implicit default variant.
    let fresh = write_source(&dir, "fresh.arw", b"raw-bytes-v9");
    let fresh_list = variants::list_variants(&repo(), &fresh).unwrap();
    assert_eq!(fresh_list.len(), 1);
    assert_eq!(fresh_list[0].variant_id, DEFAULT_VARIANT_ID);
    assert!(!fresh_list[0].is_edited);
    assert_eq!(fresh_list[0].revision, 0, "no sidecar means revision 0");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn source_hash_stays_identical_through_the_full_lifecycle() {
    let dir = common::tmp_root("vc-hash");
    let source = write_source(&dir, "photo.arw", b"raw-bytes-v10");
    let before = sha256_hex(&fs::read(&source).unwrap());

    commit_default(&source, "909", 0.8);
    let copy = variants::create_virtual_copy(&repo(), None, &source, ASSET, None, None).unwrap();
    let bumped = bump_variant(&source, &copy.variant_id, -1.2);
    variants::reset_variant(&repo(), None, &source, &copy.variant_id, bumped).unwrap();
    let bumped_again = bump_variant(&source, &copy.variant_id, 2.0);
    let _ = bumped_again;
    variants::delete_variant(None, &source, &copy.variant_id).unwrap();

    assert_eq!(
        sha256_hex(&fs::read(&source).unwrap()),
        before,
        "A1: source bytes are immutable"
    );

    let _ = fs::remove_dir_all(&dir);
}
