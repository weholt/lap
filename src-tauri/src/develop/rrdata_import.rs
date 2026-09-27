//! Explicit one-way compatibility reader from RapidRAW `.rrdata` documents
//! into Lap recipes (lap-5c2 / TASK-404; managed continuation of lap-002.4).
//!
//! Governing contract: `docs/raw-development/spec.md`, "Persistence and
//! compatibility": import happens through an explicit reader, the original
//! sidecar stays untouched, Lap's envelope is written only after validation,
//! and the original payload is preserved for diagnostics/forward
//! compatibility. Unsupported visible effects (LUT resources, lens blur,
//! visible masks, AI patches) stay discoverable as named limitations and
//! block any claim of a faithful preview/export — never a silent lossy
//! import (spec A9).
//!
//! Source schema, enumerated from the pinned producer code (RapidRAW
//! `5e30bcbb246395d391ba2e9662510641ffe68e6b`,
//! `src-tauri/src/image_processing.rs::ImageMetadata` +
//! `src-tauri/src/exif_processing.rs::load_sidecar`):
//!
//! ```text
//! { "version": u32 (always serialized; default 1),
//!   "rating": u8,
//!   "adjustments": object | null (the legacy adjustment recipe),
//!   "tags": string[] | null, "exif": map | null }
//! ```
//!
//! RapidRAW's own reader silently replaces unreadable/malformed sidecars with
//! defaults; this reader is the opposite: every malformed, future, or
//! out-of-bounds input produces an explicit typed outcome. Parameter
//! semantics and validated bounds come from the pinned `rapidraw-edit-model`
//! legacy migration (`migrate_envelope`), so names are never matched by
//! similarity and values are never clamped into silence.
//!
//! This module performs no writes. Persisting the converted envelope is an
//! explicit host action through [`crate::develop::recipe_repository`].

use std::fs;
use std::path::{Path, PathBuf};

use rapidraw_edit_model::geometry::{LegacyPixelCrop, crop_to_normalized};
use rapidraw_edit_model::migrate::{EnvelopeIdentity, MigrationReport, migrate_envelope};
use rapidraw_edit_model::{ModelError, RecipeEnvelope, sha256_hex};
use serde::Serialize;
use serde_json::Value;

use super::recipe_repository::RecipeRepository;

/// Highest `.rrdata` source schema version this reader accepts. Enumerated
/// from the pinned producer: `ImageMetadata.version` is serialized on every
/// save and defaults to 1; no other version exists in the pinned code.
pub const RRDATA_SOURCE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Errors: every malformed input is a typed, visible outcome
// ---------------------------------------------------------------------------

/// Failures of the explicit compatibility reader. `Corrupt` never falls back
/// to defaults (the pinned producer's reader does; this one must not), and
/// `FutureSourceSchema` carries the full original payload for forward
/// compatibility.
#[derive(Debug)]
pub enum RrdataImportError {
    /// The rrdata file does not exist.
    NotFound { path: PathBuf },
    /// The rrdata file exists but could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not valid UTF-8.
    NotUtf8 { path: PathBuf },
    /// Truncated/invalid JSON, a non-object document, a missing or
    /// non-integer `version`, or a missing/non-object `adjustments` field.
    Corrupt { path: PathBuf, detail: String },
    /// The document declares a source schema version newer than
    /// [`RRDATA_SOURCE_SCHEMA_VERSION`]. The complete original payload is
    /// preserved on the error; nothing is partially imported or reset.
    FutureSourceSchema {
        path: PathBuf,
        found: u32,
        supported_max: u32,
        preserved: Value,
    },
    /// The legacy payload cannot be converted to the oriented normalized
    /// geometry (unknown crop unit, crop outside the oriented frame, ...).
    Conversion { path: PathBuf, detail: String },
    /// The converted recipe/envelope failed pinned model validation: only
    /// validated supported parameters migrate.
    Model {
        path: PathBuf,
        source: rapidraw_edit_model::ModelError,
    },
}

impl std::fmt::Display for RrdataImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RrdataImportError::NotFound { path } => {
                write!(f, "rrdata file not found: {}", path.display())
            }
            RrdataImportError::Io { path, source } => {
                write!(f, "failed to read rrdata file {}: {source}", path.display())
            }
            RrdataImportError::NotUtf8 { path } => {
                write!(f, "rrdata file {} is not valid UTF-8", path.display())
            }
            RrdataImportError::Corrupt { path, detail } => {
                write!(
                    f,
                    "rrdata file {} is corrupt and was left untouched: {detail}",
                    path.display()
                )
            }
            RrdataImportError::FutureSourceSchema {
                path,
                found,
                supported_max,
                ..
            } => write!(
                f,
                "rrdata file {} declares source schema version {found}, newer than the highest supported version {supported_max}; payload preserved, not imported or reset",
                path.display()
            ),
            RrdataImportError::Conversion { path, detail } => write!(
                f,
                "rrdata geometry conversion failed for {}: {detail}",
                path.display()
            ),
            RrdataImportError::Model { path, source } => write!(
                f,
                "rrdata file {} contains parameters the validated recipe rejects: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RrdataImportError {}

// ---------------------------------------------------------------------------
// Documents and outcomes
// ---------------------------------------------------------------------------

/// A validated rrdata document: untouched original bytes, their digest, the
/// parsed payload, and the enumerated source schema version.
#[derive(Debug, Clone)]
pub struct RrdataDocument {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub byte_len: u64,
    pub value: Value,
    pub source_schema_version: u32,
}

/// Diagnostics identity of the imported original payload (spec: "Preserve
/// the original payload for diagnostics/forward compatibility").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalRrdata {
    pub sha256: String,
    pub byte_len: u64,
}

/// One named reason the imported recipe cannot claim a faithful preview or
/// export. Limitations are data: the UI surfaces them and the render path
/// enforces them (LUT-enabled recipes fail rendering explicitly until LUT
/// resources are supported; visible masks are rejected by the host renderer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportLimitation {
    /// Stable kind: `lens-blur`, `lut-resource`, `missing-resource`,
    /// `masks`, or `ai-patches`.
    pub kind: String,
    pub detail: String,
}

/// Result of a validated import: the Lap envelope (identity, source-semantics
/// decode defaults, converted recipe, retained original payload), the named
/// limitations, and the diagnostics identity of the original document.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RrdataImportOutcome {
    pub envelope: RecipeEnvelope,
    pub limitations: Vec<ImportLimitation>,
    /// Keys under which original payload fragments were retained in
    /// `envelope.unsupported` (`legacyMetadata.*`, `legacyAdjustments.*`,
    /// `legacy.*`).
    pub preserved_keys: Vec<String>,
    /// UI-only legacy fields excluded from the recipe (never render data).
    pub excluded_ui_fields: Vec<String>,
    /// Migration steps the pinned model applied (`legacy-rrdata-v0`).
    pub migration_applied: Vec<String>,
    pub original: OriginalRrdata,
    /// True only when no limitation blocks a faithful preview/export claim.
    pub faithful_subset: bool,
}

// ---------------------------------------------------------------------------
// The reader
// ---------------------------------------------------------------------------

/// Reads and validates the rrdata container without converting it. The file
/// is opened read-only and never modified; its exact bytes are hashed so the
/// host can prove immutability (hash before/after).
pub fn read_rrdata(path: &Path) -> Result<RrdataDocument, RrdataImportError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(RrdataImportError::NotFound {
                path: path.to_path_buf(),
            });
        }
        Err(err) => {
            return Err(RrdataImportError::Io {
                path: path.to_path_buf(),
                source: err,
            });
        }
    };

    let text = String::from_utf8(bytes.clone()).map_err(|_| RrdataImportError::NotUtf8 {
        path: path.to_path_buf(),
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|err| RrdataImportError::Corrupt {
        path: path.to_path_buf(),
        detail: format!("invalid JSON: {err}"),
    })?;
    let obj = value
        .as_object()
        .ok_or_else(|| RrdataImportError::Corrupt {
            path: path.to_path_buf(),
            detail: "document is valid JSON but not an ImageMetadata object".to_string(),
        })?;

    // The pinned producer always serializes `version`; a document without it
    // is not a readable rrdata sidecar.
    let version_value = obj
        .get("version")
        .ok_or_else(|| RrdataImportError::Corrupt {
            path: path.to_path_buf(),
            detail: "missing required 'version' field".to_string(),
        })?;
    let version = version_value
        .as_u64()
        .ok_or_else(|| RrdataImportError::Corrupt {
            path: path.to_path_buf(),
            detail: format!("'version' is not an integer: {version_value}"),
        })?;
    if version > u64::from(RRDATA_SOURCE_SCHEMA_VERSION) {
        return Err(RrdataImportError::FutureSourceSchema {
            path: path.to_path_buf(),
            found: u32::try_from(version).unwrap_or(u32::MAX),
            supported_max: RRDATA_SOURCE_SCHEMA_VERSION,
            preserved: value,
        });
    }

    let adjustments = obj
        .get("adjustments")
        .ok_or_else(|| RrdataImportError::Corrupt {
            path: path.to_path_buf(),
            detail: "missing 'adjustments' field".to_string(),
        })?;
    if !adjustments.is_object() {
        return Err(RrdataImportError::Corrupt {
            path: path.to_path_buf(),
            detail: format!(
                "'adjustments' is not an object: {adjustments}; this reader does not substitute defaults for malformed documents"
            ),
        });
    }

    let sha256 = sha256_hex(&bytes);
    let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok(RrdataDocument {
        path: path.to_path_buf(),
        bytes,
        sha256,
        byte_len,
        value,
        source_schema_version: version as u32,
    })
}

/// The one-way compatibility converter. Stateless and pure: `import_document`
/// never touches the filesystem beyond reading the LUT path's existence for
/// fidelity reporting.
#[derive(Debug, Clone)]
pub struct RrdataImporter {
    engine_version: String,
}

impl RrdataImporter {
    pub fn new(engine_version: impl Into<String>) -> Self {
        Self {
            engine_version: engine_version.into(),
        }
    }

    /// Engine identity matching the durable recipe repository's writer tag,
    /// so an imported envelope and its sidecar never disagree about who
    /// produced them.
    pub fn lap_default() -> Self {
        Self::new(RecipeRepository::lap_default().engine_version().to_string())
    }

    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }

    /// Converts a validated document into a Lap envelope plus a fidelity
    /// report.
    ///
    /// `source_dimensions` are the decoded original (unoriented) pixel
    /// dimensions of the target photo; together with the document's
    /// `orientationSteps` they define the oriented frame the legacy pixel
    /// crop lives in (`rapidraw_edit_model::geometry`).
    pub fn import_document(
        &self,
        doc: &RrdataDocument,
        identity: &EnvelopeIdentity,
        source_dimensions: (u64, u64),
    ) -> Result<RrdataImportOutcome, RrdataImportError> {
        let text =
            String::from_utf8(doc.bytes.clone()).map_err(|_| RrdataImportError::NotUtf8 {
                path: doc.path.clone(),
            })?;
        let (mut envelope, report) =
            migrate_envelope(&text, identity).map_err(|err| RrdataImportError::Model {
                path: doc.path.clone(),
                source: err,
            })?;

        self.convert_legacy_crop(doc, &mut envelope, source_dimensions)?;

        let limitations = self.fidelity_report(&envelope, &report);

        // Retention inventory: everything the model kept verbatim under
        // `unsupported`, plus the migration report's own record.
        let mut preserved_keys: Vec<String> = envelope
            .unsupported
            .keys()
            .filter(|key| {
                key.starts_with("legacyMetadata.")
                    || key.starts_with("legacyAdjustments.")
                    || key.starts_with("legacy.")
            })
            .cloned()
            .collect();
        preserved_keys.extend(report.preserved_keys.iter().cloned());
        preserved_keys.sort();
        preserved_keys.dedup();

        let original = OriginalRrdata {
            sha256: doc.sha256.clone(),
            byte_len: doc.byte_len,
        };
        let faithful_subset = limitations.is_empty();
        Ok(RrdataImportOutcome {
            envelope,
            limitations,
            preserved_keys,
            excluded_ui_fields: report.excluded_ui_fields,
            migration_applied: report.applied,
            original,
            faithful_subset,
        })
    }

    /// Legacy pixel crops (`react-image-crop`, `unit: "px"`) live in the
    /// oriented pixel frame; the recipe persists the normalized oriented
    /// rectangle. The original pixel payload stays retained under
    /// `legacyAdjustments.crop` (done by the model migration), so the
    /// conversion stays auditable in both directions.
    fn convert_legacy_crop(
        &self,
        doc: &RrdataDocument,
        envelope: &mut RecipeEnvelope,
        source_dimensions: (u64, u64),
    ) -> Result<(), RrdataImportError> {
        let crop_value = doc.value.get("adjustments").and_then(|adj| adj.get("crop"));
        let crop_obj = match crop_value {
            None | Some(Value::Null) => return Ok(()),
            Some(value @ Value::Object(_)) => value,
            Some(other) => {
                return Err(RrdataImportError::Conversion {
                    path: doc.path.clone(),
                    detail: format!("'crop' is not an object or null: {other}"),
                });
            }
        };

        if let Some(unit) = crop_obj.get("unit") {
            let unit = unit.as_str().ok_or_else(|| RrdataImportError::Conversion {
                path: doc.path.clone(),
                detail: format!("'crop.unit' is not a string: {unit}"),
            })?;
            if unit != "px" {
                return Err(RrdataImportError::Conversion {
                    path: doc.path.clone(),
                    detail: format!(
                        "unsupported crop unit '{unit}'; the legacy producer persists unit \"px\""
                    ),
                });
            }
        }

        let mut numbers = Vec::with_capacity(4);
        for field in ["x", "y", "width", "height"] {
            let value = crop_obj
                .get(field)
                .ok_or_else(|| RrdataImportError::Conversion {
                    path: doc.path.clone(),
                    detail: format!("'crop.{field}' is missing"),
                })?;
            let number = value
                .as_f64()
                .ok_or_else(|| RrdataImportError::Conversion {
                    path: doc.path.clone(),
                    detail: format!("'crop.{field}' is not a number: {value}"),
                })?;
            numbers.push(number);
        }

        let (source_width, source_height) = source_dimensions;
        let oriented = oriented_dimensions(
            (
                u32::try_from(source_width).unwrap_or(u32::MAX),
                u32::try_from(source_height).unwrap_or(u32::MAX),
            ),
            envelope.recipe.orientation_steps,
        );
        let crop = crop_to_normalized(
            LegacyPixelCrop {
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            },
            f64::from(oriented.0),
            f64::from(oriented.1),
        )
        .map_err(|err: ModelError| RrdataImportError::Conversion {
            path: doc.path.clone(),
            detail: err.to_string(),
        })?;
        envelope.recipe.crop = Some(crop);
        Ok(())
    }

    /// Fidelity report: every visible effect this slice cannot render
    /// faithfully becomes a named limitation (spec A9: unsupported imported
    /// effects stay visible and cannot be silently discarded).
    fn fidelity_report(
        &self,
        envelope: &RecipeEnvelope,
        report: &MigrationReport,
    ) -> Vec<ImportLimitation> {
        let mut limitations = Vec::new();
        let recipe = &envelope.recipe;

        if recipe.lens_blur_enabled {
            limitations.push(ImportLimitation {
                kind: "lens-blur".to_string(),
                detail: "lens blur is enabled in the imported recipe; the pinned engine slice renders without it (no depth-map blur pipeline), so previews and exports will not match RapidRAW".to_string(),
            });
        }

        let lut_desired = recipe.lut_path.is_some()
            || recipe.lut_name.is_some()
            || recipe.lut_size > 0
            || envelope.unsupported.contains_key("legacy.lutData");
        if lut_desired {
            if let Some(lut_path) = recipe
                .lut_path
                .as_deref()
                .filter(|p| !Path::new(p).exists())
            {
                limitations.push(ImportLimitation {
                    kind: "missing-resource".to_string(),
                    detail: format!("the referenced LUT file does not exist: {lut_path}"),
                });
            }
            limitations.push(ImportLimitation {
                kind: "lut-resource".to_string(),
                detail: "the imported recipe uses a LUT; LUT resources are not applied by this engine slice and any preview/export attempt fails explicitly instead of rendering un-LUT-ed pixels".to_string(),
            });
        }

        let visible_masks = recipe.masks.iter().filter(|mask| mask.visible).count();
        if visible_masks > 0 {
            limitations.push(ImportLimitation {
                kind: "masks".to_string(),
                detail: format!(
                    "{visible_masks} visible local mask(s) were preserved in the recipe; local adjustments are not rendered by this engine slice and are rejected explicitly at render time"
                ),
            });
        }

        if envelope.unsupported.contains_key("legacy.aiPatches")
            || report
                .preserved_keys
                .iter()
                .any(|key| key == "legacy.aiPatches")
        {
            limitations.push(ImportLimitation {
                kind: "ai-patches".to_string(),
                detail: "generative AI patch edits are preserved verbatim under 'unsupported' but are outside the initial engine scope and are not rendered".to_string(),
            });
        }

        limitations
    }
}

/// Oriented pixel dimensions for `steps` clockwise quarter turns: 1 or 3
/// swap width and height, exactly like the engine's geometry convention.
fn oriented_dimensions(source: (u32, u32), steps: u32) -> (u32, u32) {
    match steps % 4 {
        1 | 3 => (source.1, source.0),
        _ => source,
    }
}
