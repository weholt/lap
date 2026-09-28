//! Lens-profile resources: versioned identity, provenance, explicit
//! missing/unsupported outcomes, and cache-identity participation (lap-d52).

use crate::common::{lensfun_xml, lensfun_xml_digest, object_path, store, tamper_object, tmp_root};
use lap_lib::develop::resources::{
    ResourceError, ResourceStatus, attach_lens_profile, detach_lens_profile, detach_lut,
};
use rapidraw_edit_model::{LensCorrectionMode, ResourceAlgorithm};

fn attached_envelope(
    root: &std::path::Path,
    tag: &str,
) -> (
    lap_lib::develop::resources::ResourceStore,
    rapidraw_edit_model::RecipeEnvelope,
    String,
) {
    let store = store(&root.join(tag));
    let imported = store
        .import_lens_profile_bytes(
            &lensfun_xml(),
            Some("lensfun snapshot"),
            Some("lensfun 2020-01-01"),
        )
        .expect("profile imports");
    let mut envelope = rapidraw_edit_model::RecipeEnvelope::new(
        "lap-test/0",
        "asset-a",
        "default",
        &"a".repeat(64),
    );
    envelope.recipe.lens_correction_mode = LensCorrectionMode::Auto;
    attach_lens_profile(
        &mut envelope,
        &imported,
        "TestCorp Optics",
        "Test 24-70mm f/2.8",
    );
    (store, envelope, imported.id)
}

#[test]
fn lens_profile_import_is_content_addressed_and_versioned() {
    let root = tmp_root("lens-deterministic");
    let store = store(&root);

    let first = store
        .import_lens_profile_bytes(&lensfun_xml(), Some("snapshot"), Some("lensfun 2020-01-01"))
        .expect("first import");
    let second = store
        .import_lens_profile_bytes(&lensfun_xml(), None, None)
        .expect("second import");

    let digest = lensfun_xml_digest();
    assert_eq!(first.id, format!("lens/{digest}"));
    assert_eq!(first.id, second.id, "identical bytes import to the same id");
    assert_eq!(first.digest, digest);
    assert_eq!(first.lens_count, 3, "fixture carries three lenses");
    assert_eq!(first.camera_count, 1);
    assert_eq!(first.version.as_deref(), Some("lensfun 2020-01-01"));
    assert!(first.newly_stored);
    assert!(!second.newly_stored);
    assert!(object_path(&root, &first.id).is_file());

    match store.status(&first.id).unwrap() {
        ResourceStatus::Present { digest: found, .. } => assert_eq!(found, digest),
        other => panic!("expected Present, got {other:?}"),
    }

    match store.import_lens_profile_bytes(b"<not-xml", None, None) {
        Err(ResourceError::Corrupt { detail }) => {
            assert!(!detail.is_empty());
        }
        other => panic!("expected a corrupt outcome, got {other:?}"),
    }
}

#[test]
fn lens_database_listing_matching_and_resolution_work_from_the_store() {
    let root = tmp_root("lens-db");
    let (store, _envelope, id) = attached_envelope(&root, "db");

    let database = store.lens_database(&id).expect("database parses");
    let makers = store.lens_makers(&id).expect("makers list");
    assert_eq!(
        makers,
        vec![
            "LegacyCorp".to_string(),
            "OtherCorp".to_string(),
            "TestCorp Optics".to_string()
        ]
    );

    let matched = store
        .find_best_lens_match(&id, "testcorp", "Test 24-70mm f/2.8")
        .expect("match query succeeds")
        .expect("match found");
    assert_eq!(matched.0, "TestCorp Optics");

    let (params, _notices) = store
        .resolve_lens_params(
            &id,
            "TestCorp Optics",
            "Test 24-70mm f/2.8",
            37.0,
            None,
            None,
        )
        .expect("params resolve");
    assert!(
        (params.k1 - (-0.015)).abs() < 1e-6,
        "mid-focal interpolation"
    );
}

#[test]
fn select_lens_profile_writes_provenance_params_and_resource_entry() {
    let root = tmp_root("lens-select");
    let store = store(&root);
    let imported = store
        .import_lens_profile_bytes(&lensfun_xml(), None, Some("lensfun 2020-01-01"))
        .unwrap();
    let mut envelope = rapidraw_edit_model::RecipeEnvelope::new(
        "lap-test/0",
        "asset-a",
        "default",
        &"a".repeat(64),
    );

    let (params, notices) = store
        .select_lens_profile(
            &mut envelope,
            &lap_lib::develop::resources::LensSelection {
                profile_id: &imported.id,
                maker: "TestCorp Optics",
                model: "Test 24-70mm f/2.8",
                version: "lensfun 2020-01-01",
                focal_length: 24.0,
                aperture: Some(2.8),
                distance: None,
            },
        )
        .expect("selection succeeds");

    // The linear-TCA selection carries no capability notices.
    assert!(notices.is_empty(), "notices: {notices:?}");
    assert!((params.vig_k1 - (-0.2)).abs() < 1e-6);

    let profile = envelope
        .recipe
        .lens_profile
        .as_ref()
        .expect("provenance set");
    assert_eq!(profile.uri, format!("resource://{}", imported.id));
    assert_eq!(profile.maker, "TestCorp Optics");
    assert_eq!(profile.model, "Test 24-70mm f/2.8");
    assert_eq!(profile.version, "lensfun 2020-01-01");
    assert_eq!(profile.sha256, imported.digest);
    assert_eq!(
        envelope
            .recipe
            .lens_distortion_params
            .as_ref()
            .expect("params set")
            .vig_k1,
        params.vig_k1
    );
    let entry = envelope.resources.get(&imported.id).expect("map entry");
    assert_eq!(entry.algorithm, ResourceAlgorithm::Sha256);
    assert_eq!(entry.digest, imported.digest);
    envelope.validate().expect("selected envelope validates");
}

#[test]
fn unsupported_distortion_models_fail_selection_explicitly() {
    let root = tmp_root("lens-unsupported");
    let store = store(&root);
    let imported = store
        .import_lens_profile_bytes(&lensfun_xml(), None, None)
        .unwrap();
    let mut envelope = rapidraw_edit_model::RecipeEnvelope::new(
        "lap-test/0",
        "asset-a",
        "default",
        &"a".repeat(64),
    );

    match store.select_lens_profile(
        &mut envelope,
        &lap_lib::develop::resources::LensSelection {
            profile_id: &imported.id,
            maker: "LegacyCorp",
            model: "Legacy 50mm f/2",
            version: "unversioned",
            focal_length: 50.0,
            aperture: None,
            distance: None,
        },
    ) {
        Err(ResourceError::UnsupportedLens { detail }) => {
            assert!(
                detail.contains("ptbrown") && detail.contains("Legacy 50mm"),
                "error must name the lens and model: {detail}"
            );
        }
        other => panic!("expected an explicit unsupported-lens error, got {other:?}"),
    }
    assert!(
        envelope.recipe.lens_profile.is_none(),
        "a failed selection must not write partial provenance"
    );

    match store.select_lens_profile(
        &mut envelope,
        &lap_lib::develop::resources::LensSelection {
            profile_id: &imported.id,
            maker: "TestCorp Optics",
            model: "Does Not Exist",
            version: "unversioned",
            focal_length: 24.0,
            aperture: None,
            distance: None,
        },
    ) {
        Err(ResourceError::UnsupportedLens { detail }) => {
            assert!(detail.contains("no lens profile found"), "{detail}");
        }
        other => panic!("expected profile-not-found, got {other:?}"),
    }
}

#[test]
fn missing_or_changed_profile_objects_fail_resolution_explicitly() {
    let root = tmp_root("lens-resolve");
    let (store, mut envelope, id) = attached_envelope(&root, "resolve");

    // Present: verification returns the parsed database.
    let database = store
        .resolve_recipe_lens_profile(&envelope)
        .expect("resolves");
    assert!(database.is_some());

    // Missing: the object was pruned/moved away.
    let object = {
        let root = store.root().to_path_buf();
        let digest = id.strip_prefix("lens/").unwrap();
        root.join("objects").join(&digest[..2]).join(digest)
    };
    std::fs::remove_file(&object).unwrap();
    match store.resolve_recipe_lens_profile(&envelope) {
        Err(ResourceError::Missing { id: missing }) => assert_eq!(missing, id),
        other => panic!("expected Missing, got {other:?}"),
    }

    // Changed: same-length tampered content.
    let reimported = store
        .import_lens_profile_bytes(&lensfun_xml(), None, None)
        .unwrap();
    assert_eq!(reimported.id, id);
    tamper_object(store.root(), &id);
    match store.resolve_recipe_lens_profile(&envelope) {
        Err(ResourceError::Changed { id: changed, .. }) => assert_eq!(changed, id),
        other => panic!("expected Changed, got {other:?}"),
    }

    // Unmapped: the recipe references the profile but the envelope lost the
    // resource-map entry.
    tamper_object(store.root(), &id);
    envelope.resources.remove(&id);
    match store.resolve_recipe_lens_profile(&envelope) {
        Err(ResourceError::UnresolvedReference { reference, .. }) => {
            assert!(reference.contains(&id), "{reference}");
        }
        other => panic!("expected UnresolvedReference, got {other:?}"),
    }

    // A recipe without a profile resolves to nothing.
    let plain = rapidraw_edit_model::RecipeEnvelope::new(
        "lap-test/0",
        "asset-a",
        "default",
        &"a".repeat(64),
    );
    assert!(store.resolve_recipe_lens_profile(&plain).unwrap().is_none());
}

#[test]
fn lens_profiles_participate_in_limitations_and_referenced_ids() {
    let root = tmp_root("lens-limits");
    let (store, envelope, id) = attached_envelope(&root, "limits");

    let referenced = lap_lib::develop::resources::referenced_resource_ids(&envelope);
    assert!(
        referenced.contains(&id),
        "the lens profile id is a referenced resource: {referenced:?}"
    );

    assert!(
        store.limitations(&envelope).is_empty(),
        "present profile is clean"
    );
    let object = {
        let root = store.root().to_path_buf();
        let digest = id.strip_prefix("lens/").unwrap();
        root.join("objects").join(&digest[..2]).join(digest)
    };
    std::fs::remove_file(&object).unwrap();
    let limitations = store.limitations(&envelope);
    assert!(
        limitations
            .iter()
            .any(|limitation| limitation.id == id && limitation.kind == "missing"),
        "a missing profile is a visible limitation: {limitations:?}"
    );
}

#[test]
fn detach_clears_provenance_and_the_map_entry() {
    let root = tmp_root("lens-detach");
    let (store, mut envelope, id) = attached_envelope(&root, "detach");

    detach_lens_profile(&mut envelope, &id);
    assert!(envelope.recipe.lens_profile.is_none());
    assert!(envelope.recipe.lens_maker.is_none());
    assert!(envelope.recipe.lens_model.is_none());
    assert!(envelope.recipe.lens_distortion_params.is_none());
    assert!(!envelope.resources.contains_key(&id));
    envelope.validate().expect("detached envelope validates");

    // The store still serves the object; only the association is gone.
    store.lens_database(&id).expect("object remains");

    // Unrelated resources (a LUT) survive a lens detach.
    use lap_lib::develop::resources::{LutFormat, attach_lut};
    let lut = store
        .import_lut_bytes(&crate::common::cube_bytes(2), None, LutFormat::Cube)
        .unwrap();
    attach_lut(&mut envelope, &lut);
    detach_lens_profile(&mut envelope, &id);
    assert!(envelope.recipe.lut_path.is_some(), "LUT reference survives");
    detach_lut(&mut envelope, &lut.id);
}

#[test]
fn derivative_cache_identity_includes_the_lens_profile() {
    use lap_lib::develop::cache::{DerivativeIdentity, QualityTier};

    let root = tmp_root("lens-cache");
    let (_store, envelope, id) = attached_envelope(&root, "cache");

    let identity = |envelope: &rapidraw_edit_model::RecipeEnvelope| {
        DerivativeIdentity::from_envelope(envelope, 1600, 1067, QualityTier::Settled)
            .expect("identity builds")
            .key()
    };

    let with_profile = identity(&envelope);

    let mut without = envelope.clone();
    without.recipe.lens_profile = None;
    without.resources.remove(&id);
    let without_profile = identity(&without);

    assert_ne!(
        with_profile, without_profile,
        "cache identity must split on the lens profile"
    );
}
