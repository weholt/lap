//! Rollback-switch flag store contract (lap-63f / TASK-602).
//!
//! The switch is a standalone, injectable flag document so it stays
//! independent of app-config.json corruption/recovery: an unattended
//! recovery path can never silently flip the rollback state. Defaults are
//! explicit: a missing file means "develop enabled"; a corrupt or
//! unsupported document is a visible error, never a silent reset.

use crate::common::{tmp_root, tree_snapshot};
use lap_lib::develop::rollback::{RollbackError, RollbackFlag};
use std::fs;

#[test]
fn missing_flag_file_defaults_to_develop_enabled() {
    let dir = tmp_root("flag-default");
    let flag = RollbackFlag::at(dir.join("develop-rollback.json"));
    assert!(!flag.load().unwrap());
}

#[test]
fn flag_persists_across_fresh_instances() {
    let dir = tmp_root("flag-persist");
    let path = dir.join("develop-rollback.json");
    RollbackFlag::at(path.clone()).store(true).unwrap();

    // A fresh process instance re-reads from disk.
    assert!(RollbackFlag::at(path.clone()).load().unwrap());

    RollbackFlag::at(path.clone()).store(false).unwrap();
    assert!(!RollbackFlag::at(path).load().unwrap());
}

#[test]
fn corrupt_flag_document_is_explicit_error_not_silent_default() {
    let dir = tmp_root("flag-corrupt");
    let path = dir.join("develop-rollback.json");
    fs::write(&path, "{ not json at all").unwrap();

    match RollbackFlag::at(path.clone()).load() {
        Err(RollbackError::Corrupt { detail, .. }) => assert!(!detail.is_empty()),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn unsupported_flag_value_type_is_rejected_rather_than_reset() {
    let dir = tmp_root("flag-type");
    let path = dir.join("develop-rollback.json");
    // A future/foreign writer stored a non-boolean value. The reader must
    // reject this explicitly instead of silently treating it as disabled.
    fs::write(&path, r#"{"developRollback": "yes"}"#).unwrap();

    assert!(matches!(
        RollbackFlag::at(path.clone()).load(),
        Err(RollbackError::Corrupt { .. })
    ));

    // The unsupported document is preserved for diagnostics; storing a new
    // value is an explicit operator action and replaces it atomically.
    assert!(fs::read_to_string(&path).unwrap().contains("\"yes\""));
    RollbackFlag::at(path.clone()).store(true).unwrap();
    assert!(RollbackFlag::at(path).load().unwrap());
}

#[test]
fn unknown_keys_are_tolerated_and_never_leak_into_stored_documents() {
    let dir = tmp_root("flag-forward");
    let path = dir.join("develop-rollback.json");
    fs::write(
        &path,
        r#"{"developRollback": true, "futureExtension": {"v": 2}}"#,
    )
    .unwrap();
    let flag = RollbackFlag::at(path.clone());
    assert!(flag.load().unwrap());

    flag.store(false).unwrap();
    let stored = fs::read_to_string(&path).unwrap();
    assert!(stored.contains("developRollback"));
    assert!(!stored.contains("futureExtension"));
}

#[test]
fn storing_the_flag_never_touches_sibling_files() {
    let dir = tmp_root("flag-isolated");
    let source = dir.join("photo.nef");
    fs::write(&source, b"raw-bytes").unwrap();
    let before = tree_snapshot(&dir);
    RollbackFlag::at(dir.join("develop-rollback.json"))
        .store(true)
        .unwrap();
    let after = tree_snapshot(&dir);

    assert_eq!(after.get("photo.nef"), before.get("photo.nef"));
    assert!(after.contains_key("develop-rollback.json"));
    assert_eq!(after.len(), before.len() + 1);
}
