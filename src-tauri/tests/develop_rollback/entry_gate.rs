//! Entry-point gating contract (lap-63f / TASK-602).
//!
//! `ensure_editing_available` is the single backend gate every mutating
//! develop command checks. The disabled message must be explicit about what
//! is retained so a user understands rollback is non-destructive.

use lap_lib::develop::rollback::{ROLLBACK_DISABLED_MESSAGE, ensure_editing_available};

#[test]
fn editing_is_available_when_rollback_is_off() {
    assert!(ensure_editing_available(false).is_ok());
}

#[test]
fn rollback_rejects_with_the_explicit_non_destructive_message() {
    let err = ensure_editing_available(true).unwrap_err();
    assert_eq!(err, ROLLBACK_DISABLED_MESSAGE);
    // The message names the rollback switch and the retained data.
    assert!(err.contains("rollback"));
    assert!(err.contains("sidecar"));
    assert!(err.contains("resource"));
}
