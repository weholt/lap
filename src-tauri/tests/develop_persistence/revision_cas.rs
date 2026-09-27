use crate::common::{
    ASSET, VARIANT, canonical_bytes, envelope, repo, sidecar_bytes, source_file, tmp_root,
};
use lap_lib::develop::recipe_repository::{RecipeRepoError, RecipeRepository};
use rapidraw_edit_model::sha256_hex;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

#[test]
fn stale_expected_revision_is_rejected_without_writes() {
    let dir = tmp_root("stale");
    let src = source_file(&dir);
    let repo = repo();
    let env1 = envelope(&repo, &src, 0.35);
    let first = repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
    assert_eq!(first.revision, 1);
    let bytes_before = sidecar_bytes(&src);

    for stale in [0u64, 5, u64::MAX - 1] {
        let err = repo
            .commit_sidecar(&src, stale, envelope(&repo, &src, 0.9))
            .unwrap_err();
        match err {
            RecipeRepoError::RevisionConflict {
                expected, current, ..
            } => {
                assert_eq!(expected, stale);
                assert_eq!(current, Some(1));
            }
            other => panic!("stale {stale}: expected conflict, got {other}"),
        }
        assert_eq!(
            sidecar_bytes(&src),
            bytes_before,
            "rejected commits must not write"
        );
    }

    assert_eq!(repo.current_revision(&src).unwrap(), Some(1));
    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.recipe.exposure, 0.35);
}

#[test]
fn two_windows_commit_from_same_revision_one_succeeds() {
    let dir = tmp_root("twowin");
    let src = source_file(&dir);
    let repo = repo();
    repo.commit_sidecar(&src, 0, envelope(&repo, &src, 0.35))
        .unwrap();

    let window_a = RecipeRepository::lap_default();
    let window_b = RecipeRepository::lap_default();
    let mut env_a = envelope(&window_a, &src, 0.9);
    env_a.revision = 2;
    let mut env_b = envelope(&window_b, &src, -0.9);
    env_b.revision = 2;

    let src_a = src.clone();
    let a = thread::spawn(move || window_a.commit_sidecar(&src_a, 1, env_a));
    let result_b = window_b.commit_sidecar(&src, 1, env_b);
    let result_a = a.join().unwrap();

    let (ok_receipt, conflict_current) = match (&result_a, &result_b) {
        (Ok(receipt), Err(RecipeRepoError::RevisionConflict { current, .. }))
        | (Err(RecipeRepoError::RevisionConflict { current, .. }), Ok(receipt)) => {
            (receipt.clone(), *current)
        }
        _ => panic!("expected exactly one success and one conflict: {result_a:?} / {result_b:?}"),
    };
    assert_eq!(ok_receipt.revision, 2);
    assert_eq!(conflict_current, Some(2));

    let winner_env = if result_a.is_ok() {
        let mut e = envelope(&repo, &src, 0.9);
        e.revision = 2;
        e
    } else {
        let mut e = envelope(&repo, &src, -0.9);
        e.revision = 2;
        e
    };
    assert_eq!(sidecar_bytes(&src), canonical_bytes(&winner_env));
    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.recipe.exposure, winner_env.recipe.exposure);
    assert_eq!(loaded.content_hash().unwrap(), ok_receipt.content_hash);
}

#[test]
fn writes_serialize_per_asset_and_variant() {
    let dir = tmp_root("serialize");
    let src = source_file(&dir);
    let repo = Arc::new(repo());

    let successes = Arc::new(AtomicU64::new(0));
    let mut writers = Vec::new();
    for writer_id in 0..4u64 {
        let repo = Arc::clone(&repo);
        let src = src.clone();
        let successes = Arc::clone(&successes);
        writers.push(thread::spawn(move || {
            let mut committed = 0u64;
            while committed < 10 {
                let attempts = committed;
                let current = repo.current_revision(&src).unwrap().unwrap_or(0);
                let mut env =
                    repo.new_envelope(ASSET, VARIANT, &sha256_hex(&fs::read(&src).unwrap()));
                env.recipe.exposure = 0.1 * (writer_id as f64) + 0.001 * (committed as f64);
                env.revision = current + 1;
                match repo.commit_sidecar(&src, current, env) {
                    Ok(receipt) => {
                        assert_eq!(receipt.revision, current + 1);
                        assert!(receipt.revision > attempts);
                        committed += 1;
                        successes.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(RecipeRepoError::RevisionConflict { .. }) => {}
                    Err(err) => panic!("writer {writer_id}: unexpected failure: {err}"),
                }
            }
        }));
    }
    for writer in writers {
        writer.join().unwrap();
    }

    assert_eq!(successes.load(Ordering::Relaxed), 40);
    assert_eq!(repo.current_revision(&src).unwrap(), Some(40));
    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.revision, 40);
    assert!(temp_siblings_clean(&dir));
}

fn temp_siblings_clean(dir: &PathBuf) -> bool {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
}

#[test]
fn identity_and_fingerprint_mismatches_are_rejected() {
    let dir = tmp_root("identity");
    let src = source_file(&dir);
    let repo = repo();
    repo.commit_sidecar(&src, 0, envelope(&repo, &src, 0.35))
        .unwrap();
    let bytes_before = sidecar_bytes(&src);

    let mut other_asset = repo.new_envelope("asset-B", VARIANT, &fingerprint_of(&src));
    other_asset.revision = 2;
    other_asset.recipe.exposure = 0.9;
    let err = repo.commit_sidecar(&src, 1, other_asset).unwrap_err();
    match err {
        RecipeRepoError::IdentityMismatch { .. } => {}
        other => panic!("expected IdentityMismatch, got {other}"),
    }

    let mut other_variant = repo.new_envelope(ASSET, "grain", &fingerprint_of(&src));
    other_variant.revision = 2;
    let err = repo.commit_sidecar(&src, 1, other_variant).unwrap_err();
    match err {
        RecipeRepoError::IdentityMismatch { .. } => {}
        other => panic!("expected IdentityMismatch, got {other}"),
    }

    let mut other_source = repo.new_envelope(ASSET, VARIANT, &sha256_hex(b"different raw bytes"));
    other_source.revision = 2;
    other_source.recipe.exposure = 0.9;
    let err = repo.commit_sidecar(&src, 1, other_source).unwrap_err();
    match err {
        RecipeRepoError::SourceFingerprintMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, fingerprint_of(&src));
            assert_eq!(found, sha256_hex(b"different raw bytes"));
        }
        other => panic!("expected SourceFingerprintMismatch, got {other}"),
    }

    assert_eq!(sidecar_bytes(&src), bytes_before);
    assert_eq!(repo.current_revision(&src).unwrap(), Some(1));
}

fn fingerprint_of(source: &std::path::Path) -> String {
    sha256_hex(&fs::read(source).unwrap())
}
