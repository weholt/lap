use crate::common::{
    CRASH_EXIT_OK, canonical_bytes, catalog_db, envelope, prev_bytes, repo, sidecar_bytes,
    source_file, spawn_crash_worker, temp_siblings, tmp_root,
};
use lap_lib::develop::recipe_repository::{
    FaultAction, FaultInjection, FaultPoint, RecipeRepoError, RecipeRepository,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

const OLD_KEPT_POINTS: [FaultPoint; 5] = [
    FaultPoint::BeforeTempWrite,
    FaultPoint::AfterTempWrite,
    FaultPoint::BeforeFlush,
    FaultPoint::AfterFlush,
    FaultPoint::BeforeReplace,
];

const NEW_DURABLE_POINTS: [FaultPoint; 4] = [
    FaultPoint::AfterReplace,
    FaultPoint::BeforeAck,
    FaultPoint::BeforeProjection,
    FaultPoint::AfterProjection,
];

#[test]
fn commit_creates_durable_sidecar_and_acknowledges() {
    let dir = tmp_root("happy");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let env = envelope(&repo, &src, 0.35);

    let receipt = repo
        .commit(&src, 0, env.clone(), Some(&conn), None)
        .unwrap();

    assert_eq!(receipt.revision, 1);
    assert_eq!(receipt.previous_revision, None);
    assert!(receipt.projection_applied);
    assert_eq!(receipt.projection_error, None);
    assert_eq!(receipt.sidecar_path, RecipeRepository::sidecar_path(&src));
    assert_eq!(sidecar_bytes(&src), canonical_bytes(&env));
    assert_eq!(env.content_hash().unwrap(), receipt.content_hash);

    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.revision, 1);
    assert_eq!(loaded.recipe.exposure, 0.35);
    assert_eq!(loaded.asset_id, "asset-A");
    assert_eq!(loaded.variant_id, "default");

    assert!(prev_bytes(&src).is_none());
    assert!(temp_siblings(&src).is_empty());
}

#[test]
fn commit_retains_recoverable_previous_revision() {
    let dir = tmp_root("prev");
    let src = source_file(&dir);
    let repo = repo();
    let env1 = envelope(&repo, &src, 0.35);
    repo.commit_sidecar(&src, 0, env1.clone()).unwrap();

    let mut env2 = envelope(&repo, &src, 0.6);
    env2.revision = 2;
    let receipt = repo.commit_sidecar(&src, 1, env2.clone()).unwrap();
    assert_eq!(receipt.revision, 2);
    assert_eq!(receipt.previous_revision, Some(1));
    assert_eq!(receipt.content_hash, env2.content_hash().unwrap());

    let prev = repo.load_previous_opt(&src).unwrap().expect("prev sidecar");
    assert_eq!(prev.revision, 1);
    assert_eq!(prev.recipe.exposure, 0.35);
    assert_eq!(prev_bytes(&src).unwrap(), canonical_bytes(&env1));

    let current = repo.load(&src).unwrap();
    assert_eq!(current.revision, 2);
    assert_eq!(current.recipe.exposure, 0.6);
    assert_eq!(sidecar_bytes(&src), canonical_bytes(&env2));
}

#[test]
fn fault_error_at_each_boundary_preserves_or_advances_atomically() {
    for point in OLD_KEPT_POINTS {
        let dir = tmp_root(&format!("err_old_{}", point.as_str()));
        let src = source_file(&dir);
        let repo = repo();
        let env1 = envelope(&repo, &src, 0.35);
        repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
        let env2 = envelope(&repo, &src, 0.6);

        let err = repo
            .commit(
                &src,
                1,
                env2.clone(),
                None,
                Some(FaultInjection {
                    point,
                    action: FaultAction::Error("injected".to_string()),
                }),
            )
            .unwrap_err();
        assert!(
            matches!(err, RecipeRepoError::Injected { .. }),
            "point {}: expected injected error, got {err}",
            point.as_str()
        );

        assert_eq!(
            sidecar_bytes(&src),
            canonical_bytes(&env1),
            "point {}: sidecar must stay at the previous revision",
            point.as_str()
        );
        let loaded = repo.load(&src).unwrap();
        assert_eq!(loaded.revision, 1);
        assert_eq!(loaded.recipe.exposure, 0.35);

        if point == FaultPoint::BeforeReplace {
            assert_eq!(
                prev_bytes(&src).unwrap(),
                canonical_bytes(&env1),
                "point BeforeReplace: previous revision must be retained"
            );
        } else {
            assert!(
                prev_bytes(&src).is_none(),
                "point {}: previous revision must not exist yet",
                point.as_str()
            );
        }
        assert!(
            temp_siblings(&src).is_empty(),
            "point {}: failed commits must clean their temp sibling",
            point.as_str()
        );
    }

    for point in NEW_DURABLE_POINTS {
        let dir = tmp_root(&format!("err_new_{}", point.as_str()));
        let src = source_file(&dir);
        let repo = repo();
        let env1 = envelope(&repo, &src, 0.35);
        repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
        let mut env2 = envelope(&repo, &src, 0.6);
        env2.revision = 2;

        let err = repo
            .commit(
                &src,
                1,
                env2.clone(),
                None,
                Some(FaultInjection {
                    point,
                    action: FaultAction::Error("injected".to_string()),
                }),
            )
            .unwrap_err();
        assert!(matches!(err, RecipeRepoError::Injected { .. }));

        assert_eq!(
            sidecar_bytes(&src),
            canonical_bytes(&env2),
            "point {}: sidecar write is durable even though the acknowledgement was lost",
            point.as_str()
        );
        let loaded = repo.load(&src).unwrap();
        assert_eq!(loaded.revision, 2);
        assert_eq!(loaded.recipe.exposure, 0.6);
        assert_eq!(prev_bytes(&src).unwrap(), canonical_bytes(&env1));
    }
}

#[test]
fn crash_termination_at_boundaries_keeps_sidecar_recoverable() {
    let keep_points = [
        FaultPoint::AfterTempWrite,
        FaultPoint::AfterFlush,
        FaultPoint::BeforeReplace,
    ];
    for point in keep_points {
        let dir = tmp_root(&format!("crash_old_{}", point.as_str()));
        let src = source_file(&dir);
        let repo = repo();
        let env1 = envelope(&repo, &src, 0.35);
        repo.commit_sidecar(&src, 0, env1.clone()).unwrap();

        let status = spawn_crash_worker(point, &dir, 1, "asset-A");
        assert_eq!(
            status.code(),
            Some(CRASH_EXIT_OK),
            "point {}: worker must abort at the boundary",
            point.as_str()
        );

        assert_eq!(
            sidecar_bytes(&src),
            canonical_bytes(&env1),
            "point {}: crash before replace must keep the previous sidecar",
            point.as_str()
        );
        let loaded = repo.load(&src).unwrap();
        assert_eq!(loaded.revision, 1);
        assert_eq!(loaded.recipe.exposure, 0.35);
        if point == FaultPoint::BeforeReplace {
            assert_eq!(
                prev_bytes(&src).unwrap(),
                canonical_bytes(&env1),
                "point BeforeReplace: prev already updated, still recoverable"
            );
        } else {
            assert!(prev_bytes(&src).is_none());
        }
    }

    let durable_points = [FaultPoint::AfterReplace, FaultPoint::BeforeAck];
    for point in durable_points {
        let dir = tmp_root(&format!("crash_new_{}", point.as_str()));
        let src = source_file(&dir);
        let repo = repo();
        let env1 = envelope(&repo, &src, 0.35);
        repo.commit_sidecar(&src, 0, env1.clone()).unwrap();

        let status = spawn_crash_worker(point, &dir, 1, "asset-A");
        assert_eq!(status.code(), Some(CRASH_EXIT_OK));

        let mut env2 = envelope(&repo, &src, 0.6);
        env2.revision = 2;
        assert_eq!(
            sidecar_bytes(&src),
            canonical_bytes(&env2),
            "point {}: crashed-after-replace sidecar must hold the complete new envelope",
            point.as_str()
        );
        let loaded = repo.load(&src).unwrap();
        assert_eq!(loaded.revision, 2);
        assert_eq!(prev_bytes(&src).unwrap(), canonical_bytes(&env1));
        assert!(
            temp_siblings(&src).is_empty(),
            "point {}: replace consumed its temp file",
            point.as_str()
        );
    }
}

#[test]
fn concurrent_readers_never_observe_torn_sidecar() {
    let dir = tmp_root("torn");
    let src = source_file(&dir);
    let repo = repo();
    repo.commit_sidecar(&src, 0, envelope(&repo, &src, 0.35))
        .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    for reader_id in 0..2usize {
        let stop = Arc::clone(&stop);
        let src = src.clone();
        readers.push(thread::spawn(move || {
            let repo = RecipeRepository::lap_default();
            let mut last_seen = 0u64;
            let mut observations = 0u64;
            while !stop.load(Ordering::Relaxed) {
                match repo.load_opt(&src) {
                    Ok(None) => {}
                    Ok(Some(env)) => {
                        observations += 1;
                        assert!(
                            env.revision >= last_seen,
                            "reader {reader_id}: revision went backwards"
                        );
                        last_seen = env.revision;
                        let _ = env.recipe.exposure;
                    }
                    Err(err) => panic!("reader {reader_id}: sidecar was torn: {err}"),
                }
            }
            observations
        }));
    }

    for i in 2..=25u64 {
        let mut attempts = 0;
        loop {
            attempts += 1;
            let current = repo.current_revision(&src).unwrap().unwrap_or(0);
            let mut env = envelope(&repo, &src, 0.35 + (i as f64) * 0.001);
            env.revision = current + 1;
            match repo.commit_sidecar(&src, current, env) {
                Ok(receipt) => {
                    assert_eq!(receipt.revision, i);
                    break;
                }
                Err(RecipeRepoError::RevisionConflict { .. }) => {
                    assert!(attempts < 50, "writer starved by conflicts");
                }
                Err(err) => panic!("unexpected write failure: {err}"),
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    for reader in readers {
        assert!(reader.join().unwrap() > 0, "reader made no observations");
    }
    assert_eq!(repo.current_revision(&src).unwrap(), Some(25));
    assert!(temp_siblings(&src).is_empty());
}
