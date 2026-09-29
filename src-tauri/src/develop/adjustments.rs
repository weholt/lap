//! Portable global adjustment application; no RAW decode, geometry or identity copying.
use super::recipe_repository::{CommitReceipt, RecipeRepository};
use rapidraw_edit_model::{Recipe, RecipeEnvelope, ResourceRef};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, path::Path};

pub const MAX_CLIPBOARD_BYTES: usize = 256 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustmentPayload {
    pub kind: String,
    pub schema_version: u32,
    pub sections: Vec<String>,
    pub values: Map<String, Value>,
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceRef>,
}

pub struct AppliedAdjustments {
    pub before: AdjustmentPayload,
    pub receipt: CommitReceipt,
    pub fingerprint: String,
}

fn fields(section: &str) -> Option<&'static [&'static str]> {
    Some(match section {
        "basic" => &[
            "exposure",
            "brightness",
            "contrast",
            "highlights",
            "shadows",
            "whites",
            "blacks",
            "toneMapper",
            "levels",
        ],
        "curves" => &["curves", "pointCurves", "parametricCurve", "curveMode"],
        "color" => &[
            "temperature",
            "tint",
            "vibrance",
            "saturation",
            "hue",
            "colorGrading",
            "hsl",
            "colorCalibration",
        ],
        "details" => &[
            "clarity",
            "structure",
            "dehaze",
            "centré",
            "sharpness",
            "sharpnessThreshold",
            "lumaNoiseReduction",
            "colorNoiseReduction",
            "chromaticAberrationRedCyan",
            "chromaticAberrationBlueYellow",
        ],
        "effects" => &[
            "glowAmount",
            "halationAmount",
            "flareAmount",
            "grainAmount",
            "grainSize",
            "grainRoughness",
            "vignetteAmount",
            "vignetteMidpoint",
            "vignetteRoundness",
            "vignetteFeather",
            "vignetting",
            "lutIntensity",
            "lutIsSceneReferred",
            "lutName",
            "lutPath",
            "lutSize",
        ],
        _ => return None,
    })
}

impl AdjustmentPayload {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_CLIPBOARD_BYTES {
            return Err("adjustment clipboard is too large".into());
        }
        let payload: Self =
            serde_json::from_str(text).map_err(|e| format!("invalid adjustment clipboard: {e}"))?;
        payload.merge(&Recipe::default())?;
        Ok(payload)
    }

    pub fn merge(&self, recipe: &Recipe) -> Result<Recipe, String> {
        if self.kind != "lap-develop-clipboard"
            || self.schema_version != 1
            || self.sections.is_empty()
            || self.sections.len() > 5
        {
            return Err("unsupported adjustment clipboard kind, version or sections".into());
        }
        let mut allowed = Vec::new();
        for section in &self.sections {
            allowed.extend_from_slice(fields(section).ok_or("unknown adjustment section")?);
        }
        let mut merged = serde_json::to_value(recipe).map_err(|e| e.to_string())?;
        for (key, value) in &self.values {
            if key == "sectionVisibility" {
                let visibility = value.as_object().ok_or("invalid section visibility")?;
                for (section, enabled) in visibility {
                    if !self.sections.contains(section) || !enabled.is_boolean() {
                        return Err("invalid section visibility".into());
                    }
                    merged["sectionVisibility"][section] = enabled.clone();
                }
            } else if allowed.contains(&key.as_str()) {
                merged[key] = value.clone();
            } else {
                return Err(format!(
                    "{key} is not a portable adjustment in the selected sections"
                ));
            }
        }
        if let Some(Value::String(path)) = self.values.get("lutPath") {
            let digest = path
                .strip_prefix("resource://lut/")
                .ok_or("LUT must be a resource URI")?;
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err("invalid LUT digest".into());
            }
        }
        for (id, entry) in &self.resources {
            if id != &format!("lut/{}", entry.digest) {
                return Err("resource identity does not match digest".into());
            }
        }
        let next: Recipe = serde_json::from_value(merged).map_err(|e| e.to_string())?;
        rapidraw_edit_model::validate_recipe(&next).map_err(|e| e.to_string())?;
        Ok(next)
    }

    fn snapshot(&self, envelope: &RecipeEnvelope) -> Result<Self, String> {
        let recipe = serde_json::to_value(&envelope.recipe).map_err(|e| e.to_string())?;
        let mut before = self.clone();
        for (key, value) in &mut before.values {
            if key == "sectionVisibility" {
                for (section, enabled) in value.as_object_mut().ok_or("invalid visibility")? {
                    *enabled = recipe[key][section].clone();
                }
            } else {
                *value = recipe[key].clone();
            }
        }
        // Existing target LUT paths may be local. Undo is internal and must preserve them;
        // such a target is rejected before application rather than storing an unportable undo.
        before.resources = envelope
            .resources
            .iter()
            .filter(|(id, _)| id.starts_with("lut/"))
            .map(|(id, v)| (id.clone(), v.clone()))
            .collect();
        before.merge(&envelope.recipe)?;
        Ok(before)
    }
}

pub fn apply_to_asset(
    repo: &RecipeRepository,
    conn: Option<&Connection>,
    source: &Path,
    asset_id: &str,
    payload: &AdjustmentPayload,
    expected: Option<u64>,
) -> Result<AppliedAdjustments, String> {
    payload.merge(&Recipe::default())?;
    let bytes = std::fs::read(source).map_err(|e| format!("source {}: {e}", source.display()))?;
    let fingerprint = rapidraw_edit_model::sha256_hex(&bytes);
    let durable = repo.load_opt(source).map_err(|e| e.to_string())?;
    let durable_revision = durable.as_ref().map(|e| e.revision).unwrap_or(0);
    let mut envelope =
        durable.unwrap_or_else(|| repo.new_envelope(asset_id, "default", &fingerprint));
    if envelope.asset_id != asset_id || envelope.variant_id != "default" {
        return Err("target recipe identity mismatch".into());
    }
    if envelope.source_fingerprint != fingerprint {
        return Err("target source fingerprint changed".into());
    }
    let expected = expected.unwrap_or(durable_revision);
    let before = payload.snapshot(&envelope)?;
    envelope.recipe = payload.merge(&envelope.recipe)?;
    envelope.resources.extend(payload.resources.clone());
    let receipt = repo
        .commit(source, expected, envelope, conn, None)
        .map_err(|e| e.to_string())?;
    Ok(AppliedAdjustments {
        before,
        receipt,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapidraw_edit_model::{CropRect, sha256_hex};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    fn payload() -> AdjustmentPayload {
        serde_json::from_value(serde_json::json!({
            "kind":"lap-develop-clipboard","schemaVersion":1,"sections":["basic","effects"],
            "values":{"exposure":1.5,"vignetting":{"enabled":true,"amount":-1.0,"method":"circular"}}
        })).unwrap()
    }
    #[test]
    fn copies_and_undoes_adjustments_without_replacing_target_geometry_or_original_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "lap-adjustment-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("target.dng");
        fs::write(&source, b"immutable source test fixture").unwrap();
        let repo = RecipeRepository::lap_default();
        let mut original = repo.new_envelope(
            "12",
            "default",
            &sha256_hex(b"immutable source test fixture"),
        );
        original.recipe.exposure = -0.75;
        original.recipe.crop = Some(CropRect {
            x: 0.1,
            y: 0.2,
            width: 0.7,
            height: 0.6,
        });
        repo.commit(&source, 0, original.clone(), None, None)
            .unwrap();
        let applied = apply_to_asset(&repo, None, &source, "12", &payload(), Some(1)).unwrap();
        let changed = repo.load(&source).unwrap();
        assert_eq!(changed.recipe.exposure, 1.5);
        assert_eq!(changed.recipe.crop, original.recipe.crop);
        assert_eq!(changed.asset_id, "12");
        assert_eq!(changed.recipe.vignetting.amount, -1.0);
        assert!(apply_to_asset(&repo, None, &source, "12", &payload(), Some(1)).is_err());
        apply_to_asset(
            &repo,
            None,
            &source,
            "12",
            &applied.before,
            Some(applied.receipt.revision),
        )
        .unwrap();
        assert_eq!(repo.load(&source).unwrap().recipe, original.recipe);
        assert_eq!(fs::read(&source).unwrap(), b"immutable source test fixture");
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn supports_unedited_targets_and_rejects_smuggled_invalid_or_replaced_sources() {
        let dir = std::env::temp_dir().join(format!(
            "lap-adjustment-new-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("new.dng");
        fs::write(&source, b"original").unwrap();
        let repo = RecipeRepository::lap_default();
        let mut invalid = payload();
        invalid.values.insert("crop".into(), Value::Null);
        assert!(apply_to_asset(&repo, None, &source, "21", &invalid, None).is_err());
        assert!(!RecipeRepository::sidecar_path(&source).exists());
        invalid = payload();
        invalid
            .values
            .insert("exposure".into(), serde_json::json!(900));
        assert!(apply_to_asset(&repo, None, &source, "21", &invalid, None).is_err());
        assert!(!RecipeRepository::sidecar_path(&source).exists());
        let result = apply_to_asset(&repo, None, &source, "21", &payload(), None).unwrap();
        assert_eq!(result.receipt.revision, 1);
        fs::write(&source, b"replacement").unwrap();
        assert!(
            apply_to_asset(&repo, None, &source, "21", &payload(), None)
                .err()
                .unwrap()
                .contains("fingerprint")
        );
        let _ = fs::remove_dir_all(dir);
    }
}
