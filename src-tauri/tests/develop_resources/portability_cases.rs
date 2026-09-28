//! Portability: recipes reference resources by content hash, copies
//! re-associate without sharing asset identity, and the derivative cache
//! identity participates (lap-62b).

use crate::common::{
    cube_bytes, expected_id, object_path, store, tamper_object, tmp_root, write_bytes,
};
use lap_lib::develop::asset_operations::{fork_sidecar_for_copy, new_copy_asset_id};
use lap_lib::develop::cache::{DerivativeIdentity, QualityTier};
use lap_lib::develop::recipe_repository::RecipeRepository;
use lap_lib::develop::resources::LutFormat;
use lap_lib::develop::resources::{
    ResourceError, attach_lut, detach_lut, lut_resource_id, lut_resource_uri,
    referenced_resource_ids, resource_uri_id,
};
use rapidraw_edit_model::{RecipeEnvelope, ResourceRef, parse_envelope, sha256_hex};
use std::fs;

fn envelope_with_lut(root: &std::path::Path) -> (rapidraw_edit_model::RecipeEnvelope, String) {
    let store = store(root);
    let imported = store
        .import_lut_bytes(&cube_bytes(2), Some("golden"), LutFormat::Cube)
        .unwrap();
    let mut envelope = RecipeEnvelope::new("lap/test", "asset-src", "default", &"a".repeat(64));
    envelope.recipe.exposure = 0.25;
    attach_lut(&mut envelope, &imported);
    (envelope, imported.id)
}

#[test]
fn attach_writes_a_portable_reference_and_survives_canonical_round_trip() {
    let root = tmp_root("attach");
    let (mut envelope, id) = envelope_with_lut(&root);

    let digest = id.strip_prefix("lut/").unwrap();
    assert_eq!(
        envelope.recipe.lut_path.as_deref(),
        Some(lut_resource_uri(&id).as_str()),
        "the recipe references the resource by URI, never by absolute path"
    );
    assert_eq!(envelope.recipe.lut_name.as_deref(), Some("golden"));
    assert_eq!(envelope.recipe.lut_size, 2);
    assert_eq!(
        envelope.resources.get(&id),
        Some(&ResourceRef {
            algorithm: rapidraw_edit_model::ResourceAlgorithm::Sha256,
            digest: digest.to_string(),
            size_bytes: Some(cube_bytes(2).len() as u64),
        })
    );

    // The portable reference survives a canonical serialize -> parse cycle.
    let canonical = envelope.to_canonical_json().unwrap();
    let text = String::from_utf8(canonical).unwrap();
    let reparsed = parse_envelope(&text).expect("round trip parses");
    assert_eq!(reparsed, envelope);
    assert_eq!(referenced_resource_ids(&reparsed), vec![id.clone()]);

    detach_lut(&mut envelope, &id);
    assert!(envelope.recipe.lut_path.is_none());
    assert!(envelope.recipe.lut_name.is_none());
    assert!(
        envelope.resources.is_empty(),
        "detach removes the resource entry"
    );
}

#[test]
fn resolve_recipe_lut_only_resolves_present_portable_references() {
    let root = tmp_root("resolve");
    let store = store(&root);
    let (envelope, id) = envelope_with_lut(&root);

    let lut = store
        .resolve_recipe_lut(&envelope)
        .expect("present resource resolves");
    let lut = lut.expect("a LUT-enabled recipe resolves data");
    assert_eq!(lut.size, 2);

    // A recipe without a LUT resolves to no payload.
    let plain = RecipeEnvelope::new("lap/test", "asset-src", "default", &"a".repeat(64));
    assert!(store.resolve_recipe_lut(&plain).unwrap().is_none());

    // Legacy absolute paths are not portable and fail explicitly.
    let mut legacy = envelope.clone();
    legacy.recipe.lut_path = Some("C:\\luts\\golden.cube".to_string());
    match store.resolve_recipe_lut(&legacy) {
        Err(ResourceError::UnresolvedReference { reference, detail }) => {
            assert!(reference.contains("golden.cube"));
            assert!(!detail.is_empty());
        }
        other => panic!("expected UnresolvedReference, got {other:?}"),
    }

    // A recipe referencing an id absent from the envelope map fails.
    let mut unmapped = envelope.clone();
    unmapped.resources.clear();
    match store.resolve_recipe_lut(&unmapped) {
        Err(ResourceError::UnresolvedReference { .. }) => {}
        other => panic!("expected UnresolvedReference, got {other:?}"),
    }

    // Missing and changed objects surface as their typed outcomes.
    std::fs::remove_file(object_path(&root, &id)).unwrap();
    match store.resolve_recipe_lut(&envelope) {
        Err(ResourceError::Missing { id: missing }) => assert_eq!(missing, id),
        other => panic!("expected Missing, got {other:?}"),
    }
    let reimported = store
        .import_lut_bytes(&cube_bytes(2), None, LutFormat::Cube)
        .unwrap();
    tamper_object(&root, &reimported.id);
    match store.resolve_recipe_lut(&envelope) {
        Err(ResourceError::Changed { id: changed, .. }) => assert_eq!(changed, id),
        other => panic!("expected Changed, got {other:?}"),
    }
}

#[test]
fn limitations_report_lists_observed_resource_problems() {
    let root = tmp_root("limitations");
    let store = store(&root);
    let (mut envelope, id) = envelope_with_lut(&root);

    // A referenced-but-unmapped id and a mapped-but-missing id.
    let ghost_id = format!("lut/{}", sha256_hex(b"ghost"));
    envelope.recipe.lens_blur_depth_map = Some(lut_resource_uri(&ghost_id));
    envelope.resources.insert(
        format!("lut/{}", sha256_hex(b"vanished")),
        ResourceRef {
            algorithm: rapidraw_edit_model::ResourceAlgorithm::Sha256,
            digest: sha256_hex(b"vanished"),
            size_bytes: Some(3),
        },
    );

    let limitations = store.limitations(&envelope);
    let vanished_id = format!("lut/{}", sha256_hex(b"vanished"));
    let kinds: Vec<(&str, String)> = limitations
        .iter()
        .map(|limitation| (limitation.kind, limitation.id.clone()))
        .collect();
    assert!(
        kinds
            .iter()
            .any(|(kind, entry)| *kind == "missing" && *entry == vanished_id),
        "mapped-but-missing resources are reported: {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .any(|(kind, entry)| *kind == "unresolved-reference" && *entry == ghost_id),
        "referenced-but-unmapped resources are reported: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|(_, entry)| *entry == id),
        "present resources are not reported as limitations"
    );

    // Tampering turns the present resource into a "changed" limitation.
    tamper_object(&root, &id);
    let limitations = store.limitations(&envelope);
    assert!(
        limitations
            .iter()
            .any(|limitation| limitation.kind == "changed" && limitation.id == id),
        "changed resources are reported: {limitations:?}"
    );
}

#[test]
fn copied_assets_reassociate_resources_without_sharing_asset_identity() {
    let root = tmp_root("copy");
    let store = store(&root);
    let repo = RecipeRepository::lap_default();

    // Source media + committed sidecar with an attached LUT.
    let source = write_bytes(&root, "photo.raw", b"raw-bytes-original");
    let fingerprint = sha256_hex(b"raw-bytes-original");
    let mut envelope = repo.new_envelope("101", "default", &fingerprint);
    let imported = store
        .import_lut_bytes(&cube_bytes(2), Some("golden"), LutFormat::Cube)
        .unwrap();
    attach_lut(&mut envelope, &imported);
    envelope.recipe.exposure = 0.4;
    repo.commit(&source, 0, envelope, None, None).unwrap();

    // The copy fork (same primitive the catalog copy uses).
    let copy = root.join("copy").join("photo.raw");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::copy(&source, &copy).unwrap();
    let new_asset = new_copy_asset_id();
    let forked = fork_sidecar_for_copy(&source, &copy, &new_asset)
        .expect("copy fork succeeds")
        .expect("a sidecar exists to fork");

    let copied_envelope = repo.load(&copy).unwrap();
    assert_eq!(copied_envelope.asset_id, new_asset);
    assert_ne!(
        copied_envelope.asset_id,
        repo.load(&source).unwrap().asset_id,
        "the copy never shares the source asset identity"
    );
    assert_eq!(
        copied_envelope.resources,
        repo.load(&source).unwrap().resources,
        "content-addressed resources re-associate instead of being duplicated"
    );
    assert_eq!(forked.asset_id, new_asset);

    // The copied recipe resolves its LUT from the SAME store.
    let lut = store
        .resolve_recipe_lut(&copied_envelope)
        .expect("copy resolves resources")
        .expect("copy keeps the LUT reference");
    assert_eq!(lut.size, 2);

    // The original sidecar is untouched.
    let before = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();
    let _ = fs::read_to_string(forked.sidecar_path).unwrap();
    let after = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();
    assert_eq!(
        before, after,
        "fork leaves the source sidecar byte-identical"
    );
}

#[test]
fn derivative_cache_identity_participates_in_resources() {
    let root = tmp_root("cache");
    let (envelope, id) = envelope_with_lut(&root);

    let with_lut =
        DerivativeIdentity::from_envelope(&envelope, 100, 80, QualityTier::Thumbnail).unwrap();
    let mut stripped = envelope.clone();
    detach_lut(&mut stripped, &id);
    let without_lut =
        DerivativeIdentity::from_envelope(&stripped, 100, 80, QualityTier::Thumbnail).unwrap();
    assert_ne!(
        with_lut.key(),
        without_lut.key(),
        "attaching a resource must invalidate previously cached derivatives"
    );

    // A changed resource digest also changes the identity.
    let mut changed = envelope.clone();
    changed.resources.get_mut(&id).unwrap().digest = "e".repeat(64);
    let changed_identity =
        DerivativeIdentity::from_envelope(&changed, 100, 80, QualityTier::Thumbnail).unwrap();
    assert_ne!(with_lut.key(), changed_identity.key());
}

#[test]
fn resource_uri_helpers_round_trip_and_reject_absolute_paths() {
    let id = expected_id(2);
    let uri = lut_resource_uri(&id);
    assert!(uri.starts_with("resource://"));
    assert_eq!(resource_uri_id(&uri), Some(id.as_str()));
    let digest = id.strip_prefix("lut/").unwrap();
    assert_eq!(lut_resource_id(digest), id);

    assert_eq!(resource_uri_id("C:\\luts\\golden.cube"), None);
    assert_eq!(resource_uri_id("resource://lut/not-hex"), None);
    assert_eq!(resource_uri_id("resource://"), None);
    assert_eq!(resource_uri_id("plain-id"), None);
}
