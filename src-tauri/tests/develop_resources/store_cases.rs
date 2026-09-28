//! Store behavior: deterministic content addressing, bounded imports and
//! explicit missing/changed/corrupt/oversized outcomes (lap-62b).

use crate::common::{
    bounded_store, cube_bytes, cube_digest, expected_id, object_path, store, stored_objects,
    tamper_object, three_dl_bytes, tmp_root, write_bytes,
};
use lap_lib::develop::resources::{LutFormat, ResourceError, ResourceStatus};

#[test]
fn import_is_content_addressed_and_deterministic() {
    let root = tmp_root("deterministic");
    let store = store(&root);

    let first = store
        .import_lut_bytes(&cube_bytes(2), Some("first"), LutFormat::Cube)
        .expect("first import succeeds");
    let second = store
        .import_lut_bytes(&cube_bytes(2), Some("second"), LutFormat::Cube)
        .expect("second import succeeds");

    assert_eq!(
        first.id,
        expected_id(2),
        "id derives from the content digest"
    );
    assert_eq!(first.id, second.id, "same bytes import to the same id");
    assert_eq!(first.digest, cube_digest(2));
    assert_eq!(first.size_bytes, cube_bytes(2).len() as u64);
    assert_eq!(first.cube_size, 2);
    assert!(first.newly_stored, "first import stores the object");
    assert!(
        !second.newly_stored,
        "re-importing identical content must not write a second object"
    );
    assert_eq!(stored_objects(&root).len(), 1, "exactly one object stored");
    assert!(object_path(&root, &first.id).is_file());
    let resource = first.resource();
    assert_eq!(resource.digest, first.digest);
    assert_eq!(resource.size_bytes, Some(first.size_bytes));
}

#[test]
fn import_over_oversized_limits_is_rejected() {
    let root = tmp_root("oversized");
    // Byte budget: a 65-byte payload against a 64-byte budget.
    let byte_bounded = bounded_store(&root, 64, 256);
    let too_big_bytes = vec![b'x'; 65];
    match byte_bounded.import_lut_bytes(&too_big_bytes, None, LutFormat::Cube) {
        Err(ResourceError::Oversized { detail }) => {
            assert!(
                detail.contains("byte"),
                "detail mentions the byte bound: {detail}"
            );
        }
        other => panic!("expected an oversized-file rejection, got {other:?}"),
    }

    // Edge budget: a valid 8-edge cube against a 4-edge bound (the byte
    // budget is generous so the EDGE bound is what trips).
    let edge_bounded = bounded_store(&root, 1024 * 1024, 4);
    let big_edge = cube_bytes(8);
    match edge_bounded.import_lut_bytes(&big_edge, None, LutFormat::Cube) {
        Err(ResourceError::Oversized { detail }) => {
            assert!(
                detail.contains("edge"),
                "detail mentions the cube edge bound: {detail}"
            );
        }
        other => panic!("expected an oversized-cube rejection, got {other:?}"),
    }
}

#[test]
fn corrupt_cubes_are_rejected_explicitly() {
    let root = tmp_root("corrupt");
    let store = store(&root);

    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "missing size header",
            b"# only comments\n0.0 0.0 0.0\n".to_vec(),
            "LUT_3D_SIZE",
        ),
        (
            "wrong entry count",
            b"LUT_3D_SIZE 2\n0.0 0.0 0.0\n".to_vec(),
            "mismatch",
        ),
        (
            "non-finite value",
            {
                let mut text = String::from_utf8(cube_bytes(2)).unwrap();
                text = text.replacen("0.000000", "nan", 1);
                text.into_bytes()
            },
            "finite",
        ),
        (
            "data before the size header",
            b"0.1 0.1 0.1\nLUT_3D_SIZE 2\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n"
                .to_vec(),
            "before",
        ),
        (
            "malformed size value",
            b"LUT_3D_SIZE two\n".to_vec(),
            "LUT_3D_SIZE",
        ),
    ];
    for (label, bytes, needle) in cases {
        match store.import_lut_bytes(&bytes, None, LutFormat::Cube) {
            Err(ResourceError::Corrupt { detail }) => assert!(
                detail.to_lowercase().contains(&needle.to_lowercase()),
                "{label}: detail '{detail}' should mention '{needle}'"
            ),
            other => panic!("{label}: expected a corrupt outcome, got {other:?}"),
        }
    }
    assert!(
        stored_objects(&root).is_empty(),
        "rejected imports store nothing"
    );
}

#[test]
fn unsupported_formats_are_rejected_explicitly() {
    let root = tmp_root("format");
    let store = store(&root);
    let path = write_bytes(&root, "not-a-lut.txt", b"hello");
    match store.import_lut_file(&path) {
        Err(ResourceError::UnsupportedFormat { extension, .. }) => {
            assert_eq!(extension, "txt");
        }
        other => panic!("expected an unsupported-format rejection, got {other:?}"),
    }
}

#[test]
fn status_reports_present_missing_and_changed() {
    let root = tmp_root("status");
    let store = store(&root);
    let imported = store
        .import_lut_bytes(&cube_bytes(2), None, LutFormat::Cube)
        .unwrap();

    match store.status(&imported.id).unwrap() {
        ResourceStatus::Present { digest, size_bytes } => {
            assert_eq!(digest, imported.digest);
            assert_eq!(size_bytes, imported.size_bytes);
        }
        other => panic!("expected Present, got {other:?}"),
    }

    // Missing: the object file was removed (store pruned / moved away).
    std::fs::remove_file(object_path(&root, &imported.id)).unwrap();
    match store.status(&imported.id).unwrap() {
        ResourceStatus::Missing => {}
        other => panic!("expected Missing, got {other:?}"),
    }

    // Changed: the object was replaced with same-length different content.
    let reimported = store
        .import_lut_bytes(&cube_bytes(2), None, LutFormat::Cube)
        .unwrap();
    tamper_object(&root, &reimported.id);
    match store.status(&reimported.id).unwrap() {
        ResourceStatus::Changed {
            expected_digest,
            found_digest,
            ..
        } => {
            assert_eq!(expected_digest, reimported.digest);
            assert_ne!(found_digest, expected_digest);
            assert_eq!(found_digest.len(), 64);
        }
        other => panic!("expected Changed, got {other:?}"),
    }

    // Malformed ids never touch the filesystem.
    match store.status("bogus") {
        Err(ResourceError::InvalidId { id }) => assert_eq!(id, "bogus"),
        other => panic!("expected InvalidId, got {other:?}"),
    }
}

#[test]
fn three_dl_documents_parse_with_cube_count_validation() {
    let root = tmp_root("threedl");
    let store = store(&root);

    let imported = store
        .import_lut_bytes(&three_dl_bytes(2), Some("grade"), LutFormat::ThreeDl)
        .expect("valid 3dl imports");
    assert_eq!(imported.cube_size, 2);
    assert_eq!(imported.name.as_deref(), Some("grade"));

    let wrong = {
        let mut text = String::from_utf8(three_dl_bytes(2)).unwrap();
        text.push_str("0.5 0.5 0.5\n");
        text.into_bytes()
    };
    match store.import_lut_bytes(&wrong, None, LutFormat::ThreeDl) {
        Err(ResourceError::Corrupt { detail }) => {
            assert!(
                detail.contains("cube"),
                "detail mentions cube count: {detail}"
            );
        }
        other => panic!("expected a corrupt 3dl rejection, got {other:?}"),
    }
}

#[test]
fn load_lut_returns_the_engine_payload() {
    let root = tmp_root("load");
    let store = store(&root);
    let imported = store
        .import_lut_bytes(&cube_bytes(2), None, LutFormat::Cube)
        .unwrap();

    let lut = store.load_lut(&imported.id).expect("present LUT loads");
    assert_eq!(lut.size, 2);
    assert_eq!(lut.data.len(), (2usize * 2 * 2) * 3);
    for value in &lut.data {
        assert!(value.is_finite(), "LUT payload values are finite");
    }

    std::fs::remove_file(object_path(&root, &imported.id)).unwrap();
    match store.load_lut(&imported.id) {
        Err(ResourceError::Missing { id }) => assert_eq!(id, imported.id),
        other => panic!("expected Missing, got {other:?}"),
    }
}
