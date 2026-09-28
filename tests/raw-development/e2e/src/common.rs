//! Shared scenario plumbing for the A1-A7 small-slice qualification harness
//! (lap-70c / TASK-306; governing contract `docs/raw-development/spec.md`).
//!
//! Everything in this harness drives the REAL application library
//! (`lap_lib::develop`) and the pinned engine revision it consumes — no
//! scenario may substitute a mock for a layer under qualification. GPU
//! absence is a hard failure, never a skip.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lap_lib::develop::sessions::{
    DevelopConfig, DevelopService, GpuPreviewRenderer, SidecarBackedStore,
};
use lap_lib::develop::RecipeRepository;
use rapidraw_edit_model::sha256_hex;

pub const ENGINE_REVISION: &str = "de4fdbd76723c76145b477a2b8874226512475f1";

// ---------------------------------------------------------------------------
// Repo / fixture layout
// ---------------------------------------------------------------------------

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("lap repository root")
}

pub fn fixture_root() -> PathBuf {
    repo_root().join("tests/fixtures/raw-development")
}

/// Returns the corpus fixture path after verifying its sha256 against the
/// committed corpus manifest (provenance + integrity gate per run).
pub fn corpus_fixture(name: &str) -> (PathBuf, Vec<u8>, String) {
    let path = fixture_root().join("corpus").join(name);
    let bytes = fs::read(&path).unwrap_or_else(|err| panic!("read corpus fixture {name}: {err}"));
    let digest = sha256_hex(&bytes);
    verify_corpus_manifest(name, &digest, bytes.len());
    (path, bytes, digest)
}

pub fn synthetic_fixture(name: &str) -> (PathBuf, Vec<u8>, String) {
    let path = fixture_root().join("synthetic").join(name);
    let bytes = fs::read(&path).unwrap_or_else(|err| panic!("read synthetic fixture {name}: {err}"));
    let digest = sha256_hex(&bytes);
    (path, bytes, digest)
}

/// Reads `corpus-manifest.json` and asserts the fixture's sha256, size and
/// CC0 licensing evidence are intact before any scenario touches it.
fn verify_corpus_manifest(name: &str, digest: &str, len: usize) {
    let manifest_path = fixture_root().join("corpus-manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&manifest_path).expect("corpus manifest readable"),
    )
    .expect("corpus manifest parses");
    let entry = manifest["files"]
        .as_array()
        .expect("manifest files array")
        .iter()
        .find(|entry| entry["path"].as_str() == Some(&format!("corpus/{name}")))
        .unwrap_or_else(|| panic!("corpus manifest entry for {name}"));
    assert_eq!(
        entry["sha256"].as_str(),
        Some(digest),
        "fixture {name} does not match its committed corpus manifest hash"
    );
    assert_eq!(
        entry["bytes"].as_u64(),
        Some(len as u64),
        "fixture {name} byte length drifted from the manifest"
    );
    assert_eq!(
        entry["license"].as_str(),
        Some("CC0-1.0"),
        "fixture {name} must stay CC0-licensed"
    );
}

/// Copies fixture bytes into the run work directory: the committed fixtures
/// are immutable, and sidecars must never be written next to them.
pub fn stage_source(work_dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = work_dir.join(name);
    fs::write(&path, bytes).expect("stage source copy");
    path
}

// ---------------------------------------------------------------------------
// Evidence collection
// ---------------------------------------------------------------------------

pub struct Step {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub elapsed_ms: u128,
}

pub struct Scenario {
    pub name: String,
    pub started_utc: String,
    pub steps: Vec<Step>,
    pub artifacts: Vec<String>,
    pub extra: serde_json::Value,
}

impl Scenario {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            started_utc: utc_now(),
            steps: Vec::new(),
            artifacts: Vec::new(),
            extra: serde_json::json!({}),
        }
    }

    /// Records one pass/fail step. Returns false on failure so callers can
    /// bail out of dependent work while the evidence keeps accumulating.
    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) -> bool {
        let detail = detail.into();
        println!(
            "  [{}] {} — {}",
            if ok { "PASS" } else { "FAIL" },
            name,
            detail
        );
        self.steps.push(Step {
            name: name.to_string(),
            ok,
            detail,
            elapsed_ms: 0,
        });
        ok
    }

    pub fn timed<F: FnOnce(&mut Self) -> O, O>(&mut self, name: &str, f: F) -> Option<O> {
        let start = Instant::now();
        let outcome = f(self);
        let elapsed = start.elapsed().as_millis();
        let last_ok = self.steps.last().map(|s| s.ok).unwrap_or(true);
        if let Some(last) = self.steps.last_mut() {
            last.elapsed_ms = elapsed;
        }
        println!(
            "  [{}] {} ({} ms) — {}",
            if last_ok { "PASS" } else { "FAIL" },
            name,
            elapsed,
            self.steps.last().map(|s| s.detail.as_str()).unwrap_or("")
        );
        if last_ok {
            Some(outcome)
        } else {
            None
        }
    }

    pub fn record_step(&mut self, name: &str, ok: bool, detail: String) -> bool {
        self.check(name, ok, detail)
    }

    pub fn artifact(&mut self, path: &Path) {
        self.artifacts.push(path.display().to_string());
    }

    pub fn passed(&self) -> bool {
        self.steps.iter().all(|step| step.ok)
    }
}

pub fn utc_now() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    // Compact UTC timestamp without external crates: seconds precision.
    let secs = now.as_secs();
    let days = secs / 86400;
    let (year, month, day) = civil_from_days(days as i64);
    let rem = secs % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The per-invocation evidence root: `$LAP_E2E_RUN_DIR` when the gate script
/// pins one, otherwise a timestamped directory under
/// `tests/raw-development/runs/` (gitignored).
pub fn run_base() -> PathBuf {
    std::env::var("LAP_E2E_RUN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            repo_root()
                .join("tests/raw-development/runs")
                .join(format!(
                    "{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                ))
        })
}

/// Creates (or reuses) the per-scenario work directory under the run base.
pub fn run_dir(scenario: &str) -> PathBuf {
    let base = run_base().join(scenario);
    fs::create_dir_all(&base).expect("run dir");
    base
}

/// Writes the scenario evidence JSON and returns its path.
pub fn write_evidence(scenario: &Scenario, dir: &Path, ok: bool) -> PathBuf {
    let git = git_info();
    let gpu = gpu_probe_json();
    let evidence = serde_json::json!({
        "schema": "lap-raw-e2e-evidence/v1",
        "scenario": scenario.name,
        "startedUtc": scenario.started_utc,
        "finishedUtc": utc_now(),
        "outcome": if ok { "pass" } else { "fail" },
        "host": {
            "repository": repo_root(),
            "branch": git.branch,
            "revision": git.head,
            "dirtyPaths": git.dirty,
            "lapLibExposed": ["develop", "t_migration"],
        },
        "engine": {
            "pinnedRevision": ENGINE_REVISION,
            "checkoutMatchesPin": engine_checkout_matches_pin(),
        },
        "gpu": gpu,
        "steps": scenario.steps.iter().map(|s| serde_json::json!({
            "name": s.name, "ok": s.ok, "detail": s.detail, "elapsedMs": s.elapsed_ms,
        })).collect::<Vec<_>>(),
        "artifacts": scenario.artifacts,
        "extra": scenario.extra,
    });
    let path = dir.join(format!("evidence-{}.json", scenario.name));
    let mut file = fs::File::create(&path).expect("evidence file");
    file.write_all(serde_json::to_vec_pretty(&evidence).unwrap().as_slice())
        .unwrap();
    file.flush().unwrap();
    println!("  evidence: {}", path.display());
    path
}

pub struct GitInfo {
    pub head: String,
    pub branch: String,
    pub dirty: Vec<String>,
}

pub fn git_info() -> GitInfo {
    let root = repo_root();
    let run = |args: &[&str]| -> String {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let head = run(&["rev-parse", "HEAD"]);
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = run(&["status", "--porcelain"])
        .lines()
        .map(|line| line.to_string())
        .filter(|line| !line.contains(".pebbles/events.jsonl"))
        .collect();
    GitInfo {
        head,
        branch,
        dirty,
    }
}

pub fn engine_checkout_matches_pin() -> bool {
    let engine_root = PathBuf::from("C:/Users/Thomas/Desktop/RapidRAW-engine");
    let run = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&engine_root)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let head = run(&["rev-parse", "HEAD"]).unwrap_or_default();
    let status = run(&["status", "--porcelain"]).unwrap_or_default();
    head == ENGINE_REVISION && status.is_empty()
}

/// GPU capability report of the production preview renderer. A missing device
/// fails qualification; it never downgrades a scenario to "skipped".
pub fn gpu_probe_json() -> serde_json::Value {
    let report = GpuPreviewRenderer::new().probe();
    serde_json::json!({
        "available": report.available,
        "error": report.error,
        "adapterName": report.adapter_name,
        "backend": report.backend,
        "maxTextureDimension2d": report.max_texture_dimension_2d,
        "maxBufferSize": report.max_buffer_size,
    })
}

pub fn require_gpu(scenario: &mut Scenario) -> bool {
    let report = GpuPreviewRenderer::new().probe();
    scenario.check(
        "gpu-device-present",
        report.available,
        if report.available {
            format!(
                "adapter '{}' backend '{}' maxTexture2D {}",
                report.adapter_name, report.backend, report.max_texture_dimension_2d
            )
        } else {
            format!(
                "no usable offscreen device: {} — the qualification gate cannot skip GPU scenarios",
                report.error.unwrap_or_default()
            )
        },
    )
}

// ---------------------------------------------------------------------------
// Production-shaped service construction
// ---------------------------------------------------------------------------

/// Builds the develop service exactly like the production Tauri command layer
/// (`t_cmds::DevelopAppState::service`): real GPU preview renderer, durable
/// sidecar store, default bounds, no catalog connection factory (the e2e
/// harness has no app database; projection behavior is covered by the
/// develop_persistence integration suite and by the a3 scenario's own
/// in-memory catalog).
pub fn production_service() -> DevelopService {
    let store = Arc::new(SidecarBackedStore::new(RecipeRepository::lap_default()));
    DevelopService::with_gpu_and_sidecar_store(
        DevelopConfig::default(),
        Arc::new(GpuPreviewRenderer::new()),
        store,
    )
}

pub fn fingerprint(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Saves an RGBA8 frame as a PNG artifact for manual inspection.
pub fn save_png(dir: &Path, name: &str, width: u32, height: u32, rgba8: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let buffer = image::RgbaImage::from_raw(width, height, rgba8.to_vec())
        .expect("frame buffer matches dimensions");
    buffer
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write PNG artifact");
    path
}

/// Decodes a PNG artifact back into (width, height, rgba8).
pub fn load_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let image = image::open(path).expect("read PNG artifact");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    (width, height, rgba.into_raw())
}
