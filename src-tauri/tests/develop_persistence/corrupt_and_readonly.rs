use crate::common::{
    ASSET, VARIANT, clear_readonly, envelope, fingerprint, prev_bytes, repo, set_readonly,
    source_file, tmp_root,
};
use lap_lib::develop::recipe_repository::{RecipeRepoError, RecipeRepository};
use rapidraw_edit_model::SCHEMA_VERSION;
use std::fs;

#[test]
fn malformed_json_sidecar_is_visible_and_never_overwritten() {
    let dir = tmp_root("malformed");
    let src = source_file(&dir);
    let repo = repo();
    let sidecar = RecipeRepository::sidecar_path(&src);
    let garbage: &[u8] = b"{ this is not json ...";
    fs::write(&sidecar, garbage).unwrap();

    let err = repo.load_opt(&src).unwrap_err();
    match err {
        RecipeRepoError::CorruptSidecar { detail, .. } => {
            assert!(!detail.is_empty());
        }
        other => panic!("expected CorruptSidecar, got {other}"),
    }

    let default_env = repo.new_envelope(ASSET, VARIANT, &fingerprint(&src));
    let err = repo.commit_sidecar(&src, 0, default_env).unwrap_err();
    assert!(matches!(err, RecipeRepoError::CorruptSidecar { .. }));
    let err = repo
        .commit_sidecar(&src, 1, envelope(&repo, &src, 0.5))
        .unwrap_err();
    assert!(matches!(err, RecipeRepoError::CorruptSidecar { .. }));

    assert_eq!(fs::read(&sidecar).unwrap(), garbage);
    assert!(prev_bytes(&src).is_none());
}

#[test]
fn future_version_sidecar_is_reported_and_preserved() {
    let dir = tmp_root("future");
    let src = source_file(&dir);
    let repo = repo();
    let sidecar = RecipeRepository::sidecar_path(&src);
    let future_json = format!(
        "{{\"schemaVersion\":999,\"assetId\":\"{ASSET}\",\"variantId\":\"{VARIANT}\",\"revision\":7,\"sourceFingerprint\":\"{}\"}}",
        fingerprint(&src)
    );
    fs::write(&sidecar, future_json.as_bytes()).unwrap();

    match repo.load_opt(&src).unwrap_err() {
        RecipeRepoError::UnsupportedSchema {
            found,
            supported_max,
            preserved,
            ..
        } => {
            assert_eq!(found, Some(999));
            assert_eq!(supported_max, SCHEMA_VERSION);
            assert_eq!(preserved["assetId"], ASSET);
            assert_eq!(preserved["schemaVersion"], 999);
        }
        other => panic!("expected UnsupportedSchema, got {other}"),
    }

    let err = repo
        .commit_sidecar(&src, 7, envelope(&repo, &src, 0.5))
        .unwrap_err();
    assert!(matches!(err, RecipeRepoError::UnsupportedSchema { .. }));

    assert_eq!(fs::read(&sidecar).unwrap(), future_json.as_bytes());
}

#[test]
fn corrupt_sidecar_does_not_block_previous_revision_recovery() {
    let dir = tmp_root("recoverprev");
    let src = source_file(&dir);
    let repo = repo();
    let env1 = envelope(&repo, &src, 0.35);
    repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
    let mut env2 = envelope(&repo, &src, 0.6);
    env2.revision = 2;
    repo.commit_sidecar(&src, 1, env2).unwrap();

    let sidecar = RecipeRepository::sidecar_path(&src);
    fs::write(&sidecar, b"\x00\xff corrupted beyond repair").unwrap();

    assert!(matches!(
        repo.load_opt(&src).unwrap_err(),
        RecipeRepoError::CorruptSidecar { .. }
    ));

    let prev = repo
        .load_previous_opt(&src)
        .unwrap()
        .expect("recoverable prev");
    assert_eq!(prev.revision, 1);
    assert_eq!(prev.recipe.exposure, 0.35);
    assert_eq!(prev_bytes(&src).unwrap(), canonical_of(&env1));
}

fn canonical_of(env: &rapidraw_edit_model::RecipeEnvelope) -> Vec<u8> {
    env.to_canonical_json().unwrap()
}

#[test]
fn read_only_sidecar_file_refuses_replacement_without_data_loss() {
    let dir = tmp_root("readonlyfile");
    let src = source_file(&dir);
    let repo = repo();
    let env1 = envelope(&repo, &src, 0.35);
    repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
    let sidecar = RecipeRepository::sidecar_path(&src);
    let bytes_before = fs::read(&sidecar).unwrap();

    set_readonly(&sidecar);

    let mut env2 = envelope(&repo, &src, 0.6);
    env2.revision = 2;
    let err = repo.commit_sidecar(&src, 1, env2).unwrap_err();
    assert!(
        matches!(err, RecipeRepoError::ReadOnlyDestination { .. }),
        "expected ReadOnlyDestination, got {err}"
    );

    assert_eq!(fs::read(&sidecar).unwrap(), bytes_before);
    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.revision, 1);
    assert_eq!(loaded.recipe.exposure, 0.35);

    clear_readonly(&sidecar);
    let mut env3 = envelope(&repo, &src, 0.6);
    env3.revision = 2;
    let receipt = repo.commit_sidecar(&src, 1, env3).unwrap();
    assert_eq!(receipt.revision, 2);
    assert_eq!(repo.current_revision(&src).unwrap(), Some(2));
}

#[cfg(unix)]
#[test]
fn read_only_directory_refuses_new_sidecar() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp_root("readonlydir");
    let src = source_file(&dir);
    let repo = repo();

    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&dir, perms).unwrap();

    let err = repo
        .commit_sidecar(&src, 0, envelope(&repo, &src, 0.35))
        .unwrap_err();
    let permission_denied = match &err {
        RecipeRepoError::Io { source, .. } => source.kind() == std::io::ErrorKind::PermissionDenied,
        RecipeRepoError::ReadOnlyDestination { .. } => true,
        _ => false,
    };
    assert!(permission_denied, "expected permission failure, got {err}");

    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dir, perms).unwrap();
    assert!(!RecipeRepository::sidecar_path(&src).exists());
}

#[cfg(windows)]
#[test]
fn read_only_directory_target_on_windows_reports_explicit_failure() {
    let dir = tmp_root("readonlywin");
    let src = source_file(&dir);
    let repo = repo();
    let env1 = envelope(&repo, &src, 0.35);
    repo.commit_sidecar(&src, 0, env1.clone()).unwrap();
    set_readonly(&RecipeRepository::sidecar_path(&src));

    let mut env2 = envelope(&repo, &src, 0.6);
    env2.revision = 2;
    let result = repo.commit_sidecar(&src, 1, env2);
    clear_readonly(&RecipeRepository::sidecar_path(&src));
    assert!(result.is_err(), "read-only target must fail on Windows");
    let loaded = repo.load(&src).unwrap();
    assert_eq!(loaded.revision, 1);
}
