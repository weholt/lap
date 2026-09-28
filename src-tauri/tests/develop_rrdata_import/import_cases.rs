use crate::common::*;
use lap_lib::develop::recipe_repository::{RecipeRepository, is_edited_envelope};
use lap_lib::develop::rrdata_import::{RrdataImportError, read_rrdata};
use rapidraw_edit_model::masks::MaskGeometry;
use rapidraw_edit_model::migrate::EnvelopeIdentity;
use rapidraw_edit_model::types::EffectiveDecodeSettings;
use std::fs;

// Acceptance: imported defaults use SOURCE semantics (RapidRAW neutrals such
// as saturation 0), identity is stable (host asset/variant/fingerprint), and
// the rrdata payload that is not modeled is retained, never dropped.
#[test]
fn default_document_imports_with_source_semantics() {
    let doc = read_rrdata(&fixture("default-v1.rrdata.json")).unwrap();
    assert_eq!(doc.source_schema_version, SOURCE_SCHEMA_VERSION);

    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();

    let recipe = &outcome.envelope.recipe;
    assert_eq!(recipe.exposure, 0.0);
    // RapidRAW neutral saturation is 0, not Lap's legacy CSS-filter 100.
    assert_eq!(recipe.saturation, 0.0);
    assert_eq!(recipe.contrast, 0.0);
    assert_eq!(recipe.grain_size, 25.0);
    assert_eq!(recipe.vignette_feather, 50.0);
    assert_eq!(recipe.sharpness_threshold, 15.0);
    assert_eq!(recipe.curves.luma.len(), 2);
    assert!(recipe.crop.is_none());
    assert!(recipe.masks.is_empty());

    // rrdata carries no decode settings: the pinned producer's defaults are
    // recorded so the render never depends on the host's current defaults.
    assert_eq!(outcome.envelope.decode, EffectiveDecodeSettings::default());
    assert!((outcome.envelope.decode.highlight_compression - 2.5).abs() < f64::EPSILON);

    // Stable identity comes from the host, not from file paths.
    assert_eq!(outcome.envelope.schema_version, 1);
    assert_eq!(outcome.envelope.asset_id, ASSET);
    assert_eq!(outcome.envelope.variant_id, VARIANT);
    assert_eq!(outcome.envelope.source_fingerprint, fingerprint());
    assert_eq!(outcome.envelope.engine_version, engine_version());

    // Original payload retained: legacy metadata keys survive verbatim.
    assert!(
        outcome
            .envelope
            .unsupported
            .contains_key("legacyMetadata.version")
    );
    assert!(
        outcome
            .envelope
            .unsupported
            .contains_key("legacyMetadata.rating")
    );

    assert!(
        outcome.faithful_subset,
        "default document has no limitations"
    );
    assert!(outcome.limitations.is_empty());
}

#[test]
fn edited_document_migrates_supported_parameters_and_preserves_the_rest() {
    let path = scratch_copy("edited", "edited-v1.rrdata.json");
    let before = file_sha256(&path);
    let doc = read_rrdata(&path).unwrap();

    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();

    let recipe = &outcome.envelope.recipe;
    assert_eq!(recipe.exposure, 0.5);
    assert_eq!(recipe.contrast, 15.0);
    assert_eq!(recipe.temperature, -20.0);
    assert_eq!(recipe.tint, 5.0);
    assert_eq!(recipe.vibrance, 25.0);
    assert_eq!(recipe.saturation, -30.0);
    assert_eq!(recipe.clarity, 12.0);
    assert_eq!(recipe.highlights, -10.0);
    assert_eq!(recipe.grain_amount, 40.0);
    assert_eq!(recipe.vignette_amount, -25.0);
    assert_eq!(recipe.orientation_steps, 1);
    assert!(
        !recipe.section_visibility.effects,
        "section bypass migrates"
    );
    assert_eq!(recipe.curves.luma.len(), 3);
    assert_eq!(recipe.curves.luma[1].x, 128.0);
    assert_eq!(recipe.curves.luma[1].y, 140.0);

    // Geometry: legacy pixel crop (oriented frame) -> normalized CropRect.
    // orientationSteps 1 swaps 4000x3000 to oriented 3000x4000:
    // x 600/3000=0.2, y 400/4000=0.1, w 2400/3000=0.8, h 1600/4000=0.4.
    let crop = recipe
        .crop
        .expect("pixel crop converts to the oriented frame");
    assert!((crop.x - 0.2).abs() < 1e-9);
    assert!((crop.y - 0.1).abs() < 1e-9);
    assert!((crop.width - 0.8).abs() < 1e-9);
    assert!((crop.height - 0.4).abs() < 1e-9);

    // Original payload retained for diagnostics: legacy crop, unknown keys,
    // and legacy metadata survive verbatim in `unsupported`.
    let original_crop = outcome
        .envelope
        .unsupported
        .get("legacyAdjustments.crop")
        .expect("legacy pixel crop retained");
    assert_eq!(original_crop["x"], 600.0);
    assert_eq!(original_crop["width"], 2400.0);
    assert!(
        outcome
            .envelope
            .unsupported
            .contains_key("legacyAdjustments.futureUnknownSlider")
    );
    assert_eq!(outcome.envelope.unsupported["legacyMetadata.rating"], 4);
    assert_eq!(
        outcome.envelope.unsupported["legacyMetadata.exif"]["Make"],
        "Canon"
    );
    assert!(
        outcome
            .preserved_keys
            .iter()
            .any(|key| key == "legacyAdjustments.futureUnknownSlider")
    );

    // UI-only state is excluded, listed, and never becomes recipe data.
    assert!(
        outcome
            .excluded_ui_fields
            .iter()
            .any(|key| key == "showClipping")
    );

    assert!(
        outcome.faithful_subset,
        "edited document has no visible unsupported effect"
    );
    assert!(outcome.limitations.is_empty());

    // The original rrdata stays byte-identical.
    assert_eq!(file_sha256(&path), before);
    assert_eq!(doc.sha256, before);
    assert_eq!(doc.byte_len, fs::metadata(&path).unwrap().len());
}

#[test]
fn partial_document_fills_source_defaults() {
    let doc = read_rrdata(&fixture("partial-v1.rrdata.json")).unwrap();
    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();
    assert_eq!(outcome.envelope.recipe.exposure, 0.75);
    assert_eq!(outcome.envelope.recipe.highlights, -30.0);
    assert_eq!(outcome.envelope.recipe.saturation, 0.0);
    assert_eq!(outcome.envelope.recipe.contrast, 0.0);
    assert!(outcome.faithful_subset);
}

#[test]
fn invalid_documents_produce_explicit_outcomes() {
    let cases: &[(&str, &str)] = &[
        ("corrupt.rrdata.json", "json"),
        ("missing-version.rrdata.json", "version"),
        ("not-an-object.rrdata.json", "object"),
    ];
    for (name, needle) in cases {
        let err = read_rrdata(&fixture(name)).unwrap_err();
        expect_corrupt(err, needle);
    }

    let missing = read_rrdata(&fixture("no-such-document.rrdata.json")).unwrap_err();
    assert!(
        matches!(missing, RrdataImportError::NotFound { .. }),
        "missing rrdata is an explicit NotFound, got {missing:?}"
    );
}

#[test]
fn future_source_schema_is_rejected_with_payload_preserved() {
    let path = scratch_copy("future", "future-version.rrdata.json");
    let before = file_sha256(&path);
    // The version gate fires at read time: a future document never becomes a
    // convertible RrdataDocument.
    let err = read_rrdata(&path).unwrap_err();
    match err {
        RrdataImportError::FutureSourceSchema {
            found,
            supported_max,
            preserved,
            ..
        } => {
            assert_eq!(found, 99);
            assert_eq!(supported_max, SOURCE_SCHEMA_VERSION);
            // Forward compatibility: the whole original payload survives on
            // the error for diagnostics; nothing is partially imported.
            assert_eq!(preserved["version"], 99);
            assert_eq!(preserved["adjustments"]["exposure"], 0.5);
            assert_eq!(
                preserved["adjustments"]["someFutureControl"]["nested"],
                true
            );
            assert_eq!(preserved["tags"][0], "from-the-future");
        }
        other => panic!("expected FutureSourceSchema, got {other:?}"),
    }
    assert_eq!(file_sha256(&path), before, "original rrdata untouched");
}

#[test]
fn missing_lut_resource_is_reported_explicitly() {
    let doc = read_rrdata(&fixture("missing-lut-v1.rrdata.json")).unwrap();
    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();

    // Parameters still migrate; nothing is silently dropped...
    let recipe = &outcome.envelope.recipe;
    assert_eq!(recipe.lut_name.as_deref(), Some("Missing profile"));
    assert_eq!(recipe.lut_intensity, 70.0);
    assert_eq!(recipe.lut_size, 33);

    // ...but the absent resource is named explicitly and blocks a faithful
    // preview/export claim.
    assert!(!outcome.faithful_subset);
    let missing = outcome
        .limitations
        .iter()
        .find(|limitation| limitation.kind == "missing-resource")
        .expect("missing LUT resource must be a named limitation");
    assert!(
        missing.detail.contains("graded-look.cube"),
        "{}",
        missing.detail
    );
    assert!(
        outcome
            .limitations
            .iter()
            .any(|limitation| limitation.kind == "lut-resource")
    );
}

#[test]
fn unsupported_visible_effects_stay_discoverable() {
    let doc = read_rrdata(&fixture("unsupported-effects-v1.rrdata.json")).unwrap();
    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();

    // Retained, never dropped:
    assert!(outcome.envelope.recipe.lens_blur_enabled);
    assert_eq!(
        outcome.envelope.recipe.lens_blur_depth_map.as_deref(),
        Some("C:/Users/editor/depth-maps/photo.depth.png")
    );
    assert_eq!(outcome.envelope.recipe.masks.len(), 1);
    let mask = &outcome.envelope.recipe.masks[0];
    assert!(mask.visible);
    assert!((mask.adjustments.exposure - 0.3).abs() < 1e-9);
    assert_eq!(mask.sub_masks[0].kind, "brush");
    assert!(mask.sub_masks[0].parameters.is_object());
    assert!(
        outcome
            .envelope
            .unsupported
            .contains_key("legacy.aiPatches")
    );

    // ...and every unsupported visible effect is a named limitation.
    let kinds: Vec<&str> = outcome
        .limitations
        .iter()
        .map(|limitation| limitation.kind.as_str())
        .collect();
    assert!(kinds.contains(&"lens-blur"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"lut-resource"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"masks"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"ai-patches"), "kinds: {kinds:?}");
    assert!(!outcome.faithful_subset);
}

#[test]
fn supported_non_ai_masks_import_with_typed_geometry_and_no_limitation() {
    // Brush/linear/radial geometry in the legacy pixel payload converts into
    // the validated oriented normalized `geometry` field. Such masks are
    // rendered by the engine, so they are NOT import limitations anymore
    // (spec A9: limitations exist for genuinely unsupported effects only).
    let doc = read_rrdata(&fixture("masked-supported-v1.rrdata.json")).unwrap();
    let outcome = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap();

    assert_eq!(outcome.envelope.recipe.masks.len(), 1);
    let mask = &outcome.envelope.recipe.masks[0];
    assert_eq!(mask.sub_masks.len(), 2);
    let radial = &mask.sub_masks[0];
    assert_eq!(radial.kind, "radial");
    let geometry = radial.geometry.as_ref().expect("radial geometry converts");
    match geometry {
        MaskGeometry::Radial {
            center_x,
            center_y,
            radius_x,
            radius_y,
            rotation,
            feather,
        } => {
            // Oriented frame is 4000x3000 (source, no rotation).
            assert!((center_x - 0.5).abs() < 1e-9);
            assert!((center_y - 0.5).abs() < 1e-9);
            assert!((radius_x - 800.0 / 4000.0).abs() < 1e-9);
            assert!((radius_y - 600.0 / 4000.0).abs() < 1e-9);
            assert_eq!((*rotation, *feather), (0.0, 0.5));
        }
        other => panic!("expected radial geometry, got {other:?}"),
    }
    let linear = &mask.sub_masks[1];
    assert_eq!(linear.kind, "linear");
    assert!(linear.geometry.is_some(), "linear geometry converts");
    // The original payload stays preserved for diagnostics/round trips.
    assert!(linear.parameters.is_object());

    let kinds: Vec<&str> = outcome
        .limitations
        .iter()
        .map(|limitation| limitation.kind.as_str())
        .collect();
    assert!(
        !kinds.contains(&"masks"),
        "supported masks are rendered, not limitations: {kinds:?}"
    );
    assert!(
        outcome.faithful_subset,
        "no unsupported effects in this document"
    );
}

#[test]
fn out_of_bounds_values_fail_validation_without_writes() {
    let path = scratch_copy("bounds", "out-of-bounds-v1.rrdata.json");
    let before = file_sha256(&path);
    let doc = read_rrdata(&path).unwrap();

    let err = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap_err();
    match err {
        RrdataImportError::Model { source, .. } => {
            let message = source.to_string();
            assert!(
                message.contains("exposure") && message.contains("bounds"),
                "validation error should name the offending parameter: {message}"
            );
        }
        other => panic!("expected a model validation outcome, got {other:?}"),
    }
    assert_eq!(file_sha256(&path), before, "original rrdata untouched");
}

#[test]
fn non_pixel_crop_units_are_rejected_explicitly() {
    let dir = tmp_root("crop-unit");
    let path = dir.join("percent-crop.rrdata.json");
    fs::write(
        &path,
        r#"{"version":1,"rating":0,"tags":null,"adjustments":{"exposure":0.2,"crop":{"unit":"%","x":10,"y":10,"width":50,"height":50}}}"#,
    )
    .unwrap();
    let doc = read_rrdata(&path).unwrap();
    let err = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap_err();
    match err {
        RrdataImportError::Conversion { detail, .. } => {
            assert!(detail.contains("unit"), "{detail}");
        }
        other => panic!("expected a conversion outcome, got {other:?}"),
    }
}

#[test]
fn crop_outside_the_oriented_frame_is_rejected() {
    let dir = tmp_root("crop-frame");
    let path = dir.join("oversized-crop.rrdata.json");
    fs::write(
        &path,
        r#"{"version":1,"rating":0,"tags":null,"adjustments":{"exposure":0.2,"orientationSteps":0,"crop":{"unit":"px","x":3500,"y":0,"width":1000,"height":3000}}}"#,
    )
    .unwrap();
    let doc = read_rrdata(&path).unwrap();
    let err = importer()
        .import_document(&doc, &identity(), source_dimensions())
        .unwrap_err();
    assert!(
        matches!(err, RrdataImportError::Conversion { .. }),
        "crop past the 4000px frame must fail explicitly, got {err:?}"
    );
}

#[test]
fn converted_envelope_commits_durably_and_retains_original_payload() {
    let dir = tmp_root("commit");
    let source = dir.join("photo.nef");
    fs::write(&source, b"raw-bytes-for-lap-5c2-import-tests").unwrap();
    let source_fingerprint = rapidraw_edit_model::sha256_hex(&fs::read(&source).unwrap());
    let local_identity = EnvelopeIdentity {
        source_fingerprint,
        ..identity()
    };

    let doc = read_rrdata(&fixture("edited-v1.rrdata.json")).unwrap();
    let outcome = importer()
        .import_document(&doc, &local_identity, source_dimensions())
        .unwrap();

    let repo = RecipeRepository::lap_default();
    let receipt = repo
        .commit(&source, 0, outcome.envelope.clone(), None, None)
        .unwrap();
    assert_eq!(receipt.revision, 1);

    let reloaded = repo.load(&source).unwrap();
    assert_eq!(reloaded.recipe.exposure, 0.5);
    assert!((reloaded.recipe.crop.as_ref().unwrap().width - 0.8).abs() < 1e-9);
    assert!(
        reloaded
            .unsupported
            .contains_key("legacyAdjustments.futureUnknownSlider")
    );
    assert!(reloaded.unsupported.contains_key("legacyMetadata.exif"));
    assert!(is_edited_envelope(&reloaded));
}

#[test]
fn importer_reports_the_original_payload_identity() {
    let doc = read_rrdata(&fixture("default-v1.rrdata.json")).unwrap();
    assert_eq!(doc.sha256, file_sha256(&fixture("default-v1.rrdata.json")));
    assert_eq!(
        doc.byte_len,
        fs::metadata(fixture("default-v1.rrdata.json"))
            .unwrap()
            .len()
    );
    assert_eq!(doc.path, fixture("default-v1.rrdata.json"));
}
