//! Shared fixtures for the develop-resources suite: bounded scratch
//! directories and deterministic identity/validation LUT payloads.

use lap_lib::develop::resources::{ResourceStore, ResourceStoreLimits};
use rapidraw_edit_model::sha256_hex;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn tmp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lap_develop_resources_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(bytes).unwrap();
    path
}

/// A valid 2x2x2 identity-ish cube: every entry is deterministic so the
/// expected digest is stable across runs.
pub fn cube_bytes(size: u32) -> Vec<u8> {
    let mut text = String::new();
    text.push_str("# identity test cube\n");
    text.push_str(&format!("LUT_3D_SIZE {size}\n"));
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let r = f64::from(x) / f64::from(size - 1);
                let g = f64::from(y) / f64::from(size - 1);
                let b = f64::from(z) / f64::from(size - 1);
                text.push_str(&format!("{r:.6} {g:.6} {b:.6}\n"));
            }
        }
    }
    text.into_bytes()
}

/// Reinterprets the cube as a header-less 3DL document (one triplet per
/// line, no LUT_3D_SIZE header).
pub fn three_dl_bytes(size: u32) -> Vec<u8> {
    let cube = cube_bytes(size);
    let text = String::from_utf8(cube).unwrap();
    let data: Vec<&str> = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("LUT_3D_SIZE")
        })
        .collect();
    data.join("\n").into_bytes()
}

pub fn cube_digest(size: u32) -> String {
    sha256_hex(&cube_bytes(size))
}

pub fn expected_id(size: u32) -> String {
    format!("lut/{}", cube_digest(size))
}

pub fn store(root: &Path) -> ResourceStore {
    ResourceStore::open(root).expect("resource store opens")
}

pub fn bounded_store(root: &Path, max_bytes: u64, max_edge: u32) -> ResourceStore {
    ResourceStore::with_limits(
        root,
        ResourceStoreLimits {
            max_resource_bytes: max_bytes,
            max_lut_edge: max_edge,
            max_cached_luts: 4,
        },
    )
    .expect("bounded resource store opens")
}

/// Walks `<root>/objects` and returns every stored object path.
pub fn stored_objects(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let prefix = root.join("objects");
    let mut stack = vec![prefix];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Flips every byte of the object file for `id` without changing its length
/// (a "same size, different content" tamper).
pub fn tamper_object(root: &Path, id: &str) {
    let path = object_path(root, id);
    let bytes = fs::read(&path).unwrap();
    let flipped: Vec<u8> = bytes.iter().map(|b| !b).collect();
    fs::write(&path, flipped).unwrap();
}

pub fn object_path(root: &Path, id: &str) -> PathBuf {
    let digest = id
        .strip_prefix("lut/")
        .or_else(|| id.strip_prefix("lens/"))
        .expect("resource id");
    root.join("objects").join(&digest[..2]).join(digest)
}

// ---------------------------------------------------------------------------
// Lens profile fixtures (lap-d52)
// ---------------------------------------------------------------------------

/// A deterministic minimal lensfun database: one camera, three lenses —
/// the main zoom (poly3 distortion, linear TCA, pa vignetting at two
/// apertures), a ptlens prime, and a lens with an unsupported distortion
/// model (ptbrown) for explicit-error coverage.
pub fn lensfun_xml() -> Vec<u8> {
    let mut text = String::new();
    text.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<lensdatabase>\n");
    text.push_str("  <camera>\n    <maker>TestCorp</maker>\n    <model>TestBody X1</model>\n    <mount>TestMount</mount>\n    <cropfactor>1.5</cropfactor>\n  </camera>\n");
    text.push_str("  <lens>\n");
    text.push_str("    <maker>TestCorp</maker>\n");
    text.push_str("    <maker lang=\"en\">TestCorp Optics</maker>\n");
    text.push_str("    <model>Test 24-70mm f/2.8</model>\n");
    text.push_str("    <mount>TestMount</mount>\n");
    text.push_str("    <cropfactor>1.5</cropfactor>\n");
    text.push_str("    <calibration>\n");
    text.push_str(
        "      <distortion model=\"poly3\" focal=\"24\" k1=\"-0.01\" k2=\"0.005\" k3=\"0.001\"/>\n",
    );
    text.push_str(
        "      <distortion model=\"poly3\" focal=\"50\" k1=\"-0.02\" k2=\"0.01\" k3=\"0.002\"/>\n",
    );
    text.push_str("      <tca model=\"linear\" focal=\"24\" vr=\"1.0004\" vb=\"0.9998\"/>\n");
    text.push_str("      <vignetting model=\"pa\" focal=\"24\" aperture=\"2.8\" distance=\"10\" k1=\"-0.2\" k2=\"0.05\" k3=\"0.01\"/>\n");
    text.push_str("      <vignetting model=\"pa\" focal=\"24\" aperture=\"8\" distance=\"10\" k1=\"-0.05\" k2=\"0.01\" k3=\"0.0\"/>\n");
    text.push_str("      <vignetting model=\"pa\" focal=\"50\" aperture=\"2.8\" distance=\"10\" k1=\"-0.3\" k2=\"0.1\" k3=\"0.02\"/>\n");
    text.push_str("    </calibration>\n");
    text.push_str("  </lens>\n");
    text.push_str("  <lens>\n");
    text.push_str("    <maker>OtherCorp</maker>\n");
    text.push_str("    <model>Other 35mm f/1.4</model>\n");
    text.push_str("    <mount>TestMount</mount>\n");
    text.push_str("    <cropfactor>1.0</cropfactor>\n");
    text.push_str("    <calibration>\n");
    text.push_str(
        "      <distortion model=\"ptlens\" focal=\"35\" a=\"0.012\" b=\"-0.021\" c=\"0.008\"/>\n",
    );
    text.push_str("    </calibration>\n");
    text.push_str("  </lens>\n");
    text.push_str("  <lens>\n");
    text.push_str("    <maker>LegacyCorp</maker>\n");
    text.push_str("    <model>Legacy 50mm f/2</model>\n");
    text.push_str("    <mount>LegacyMount</mount>\n");
    text.push_str("    <calibration>\n");
    text.push_str("      <distortion model=\"ptbrown\" focal=\"50\" k1=\"-0.01\" k2=\"0.002\"/>\n");
    text.push_str("    </calibration>\n");
    text.push_str("  </lens>\n");
    text.push_str("</lensdatabase>\n");
    text.into_bytes()
}

pub fn lensfun_xml_digest() -> String {
    sha256_hex(&lensfun_xml())
}
