//! IPC adapter for Lap sidecar revisions (zero means no durable sidecar).
use rapidraw_edit_model::RecipeEnvelope;

pub fn parse_session_envelope(mut value: serde_json::Value) -> Result<RecipeEnvelope, String> {
    // The IPC contract uses durable sidecar revisions, including zero for a
    // fresh asset. The shared engine starts at one. Translate only here;
    // never relax the durable model/sidecar validator to accept revision zero.
    let revision = value
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "invalid develop envelope: revision must be an unsigned integer".to_string()
        })?;
    let engine_revision = revision
        .checked_add(1)
        .ok_or_else(|| "invalid develop envelope: revision overflow".to_string())?;
    value["revision"] = engine_revision.into();
    rapidraw_edit_model::migrate::parse_envelope_value(value)
        .map_err(|e| format!("invalid develop envelope: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wire(revision: u64) -> serde_json::Value {
        let mut value = serde_json::to_value(RecipeEnvelope::new(
            "lap/test",
            "42",
            "default",
            &"f".repeat(64),
        ))
        .unwrap();
        value["revision"] = revision.into();
        value
    }
    #[test]
    fn fresh_ipc_envelope_maps_to_engine_revision_one() {
        assert_eq!(parse_session_envelope(wire(0)).unwrap().revision, 1);
    }
    #[test]
    fn persisted_ipc_revision_maps_exactly_once() {
        assert_eq!(parse_session_envelope(wire(5)).unwrap().revision, 6);
    }
    #[test]
    fn rejects_invalid_revisions_and_recipes() {
        for bad in [
            serde_json::Value::Null,
            (-1).into(),
            "0".into(),
            u64::MAX.into(),
        ] {
            let mut value = wire(0);
            value["revision"] = bad;
            assert!(parse_session_envelope(value).is_err());
        }
        let mut value = wire(0);
        value["recipe"]["exposure"] = 999.into();
        assert!(parse_session_envelope(value).is_err());
    }
}
