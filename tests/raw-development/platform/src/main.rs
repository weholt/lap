//! Platform qualification and performance harness for the Lap
//! raw-development extraction (lap-c0e / TASK-601; governing contract
//! `docs/raw-development/spec.md`, acceptance A7/A11/A12, prerequisite P5).
//!
//! Modes:
//!   `full`                        run every benchmark/stress workload on
//!                                 this machine and write the results
//!                                 document (default)
//!   `child-cold-decode <path>`    internal: cold RAW decode in a fresh
//!                                 process, prints one JSON line
//!   `--out <path>`                results document destination
//!   `--machine <id>`              override the manifest machine id
//!
//! The results document is schema `lap-raw-platform-results/v1` and is
//! validated by `scripts/raw-development/benchmark.mjs`. Every measurement
//! carries named hardware/backend identity, fixture revisions and raw
//! samples; the provisional warm-1536px p95 target (150 ms) is reported, and
//! a miss stays visible â€” the target is never redefined.

mod counters;
mod quantile;
mod workloads;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lap_lib::develop::sessions::GpuPreviewRenderer;

use workloads::{ENGINE_REVISION, production_service, utc_now, write_json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repo root")
}

fn os_id() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    }
}

fn git(args: &[&str], cwd: &Path) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Machine {
    id: String,
    cpu: String,
    expected_adapter: Option<String>,
}

/// Resolves this machine from the platform manifest: the first `available`
/// machine whose `os` matches the current platform.
fn resolve_machine(manifest_path: &Path, override_id: Option<&str>) -> Machine {
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).expect("platform manifest"))
            .expect("platform manifest parses");
    let machines = manifest["machines"].as_array().expect("machines array");
    let entry = machines
        .iter()
        .find(|machine| {
            override_id
                .map(|id| machine["id"].as_str() == Some(id))
                .unwrap_or_else(|| {
                    machine["status"].as_str() == Some("available")
                        && machine["os"].as_str() == Some(os_id())
                })
        })
        .unwrap_or_else(|| {
            panic!(
                "no available machine for os '{}' in the platform manifest",
                os_id()
            )
        });
    Machine {
        id: entry["id"].as_str().expect("machine id").to_string(),
        cpu: entry["hardware"]["cpu"]
            .as_str()
            .unwrap_or("unspecified CPU")
            .to_string(),
        expected_adapter: entry["gpu"]["adapter"].as_str().map(str::to_string),
    }
}

fn engine_checkout_matches_pin(engine_root: &Path) -> serde_json::Value {
    let head = git(&["rev-parse", "HEAD"], engine_root).unwrap_or_default();
    let status = git(&["status", "--porcelain"], engine_root).unwrap_or_default();
    serde_json::json!({
        "pin": ENGINE_REVISION,
        "head": head,
        "matches": head == ENGINE_REVISION && status.is_empty(),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Child mode: one cold decode, one JSON line, one small process.
    if let Some(pos) = args.iter().position(|arg| arg == "child-cold-decode") {
        let path = args
            .get(pos + 1)
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("child-cold-decode requires a fixture path"));
        std::process::exit(workloads::child_cold_decode(&path));
    }

    // Diagnostic mode: raw memory/VRAM counters (one JSON object).
    if args.iter().any(|arg| arg == "diag-counters") {
        println!(
            "{}",
            serde_json::to_string_pretty(&counters::sample()).expect("counters serialization")
        );
        return;
    }

    // Diagnostic mode: stage-by-stage breakdown of one preview render.
    if let Some(pos) = args.iter().position(|arg| arg == "diag-render") {
        let fixtures = workloads::load_corpus_fixtures();
        let name = args
            .get(pos + 1)
            .map(String::as_str)
            .unwrap_or("canon-eos-r6-craw-iso100");
        let fixture = fixtures
            .iter()
            .find(|fixture| fixture.rel_path.contains(name))
            .unwrap_or_else(|| panic!("fixture containing '{name}' not found"));
        std::process::exit(workloads::diag_render(fixture));
    }

    let machine_override = args
        .iter()
        .position(|arg| arg == "--machine")
        .and_then(|pos| args.get(pos + 1))
        .map(String::as_str);
    let out_override = args
        .iter()
        .position(|arg| arg == "--out")
        .and_then(|pos| args.get(pos + 1))
        .map(PathBuf::from);

    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("platform-manifest.json");
    let machine = resolve_machine(&manifest_path, machine_override);
    let started = utc_now();
    let overall = std::time::Instant::now();

    println!(
        "== lap-raw-platform: machine '{}' (os {}) ==",
        machine.id,
        os_id()
    );

    // Real GPU required: the platform evidence never runs without one.
    let gpu = GpuPreviewRenderer::new().probe();
    assert!(
        gpu.available,
        "no usable GPU device: {} â€” platform qualification requires real GPU execution and never skips",
        gpu.error.unwrap_or_default()
    );
    if let Some(expected) = &machine.expected_adapter {
        assert_eq!(
            &gpu.adapter_name, expected,
            "detected GPU adapter '{}' does not match the platform manifest ('{expected}'); the results would not be attributable to the named machine",
            gpu.adapter_name
        );
    }
    println!(
        "GPU: {} (backend {}, maxTexture2D {})",
        gpu.adapter_name, gpu.backend, gpu.max_texture_dimension_2d
    );

    // Fixture revisions (sha256-verified against the committed corpus).
    let fixtures = workloads::load_corpus_fixtures();
    println!(
        "fixtures: {} verified against corpus-manifest.json",
        fixtures.len()
    );

    let run_dir = repo_root()
        .join("tests/raw-development/platform/runs")
        .join(format!("{}-full", chrono_compact()));
    fs::create_dir_all(&run_dir).expect("run dir");

    let service = production_service();
    let exe = std::env::current_exe().expect("current exe");
    let primary = fixtures
        .iter()
        .find(|fixture| fixture.rel_path.contains("canon-eos-r6-craw-iso100"))
        .expect("primary Bayer fixture")
        .clone();

    let mut workloads_json = serde_json::Map::new();
    workloads_json.insert(
        "cold-decode".to_string(),
        workloads::cold_decode(&exe, &fixtures),
    );
    let mut warm = workloads::warm_slider_1536(&service, &primary, &run_dir);
    warm["renderBreakdown"] = workloads::render_breakdown(&primary);
    workloads_json.insert("warm-slider-1536".to_string(), warm);
    workloads_json.insert(
        "settled-preview".to_string(),
        workloads::settled_preview(&service, &primary, &run_dir),
    );
    workloads_json.insert(
        "export".to_string(),
        workloads::export_fullres(&service, &primary, &run_dir, 2),
    );
    workloads_json.insert(
        "thumbnail-throughput-indexing".to_string(),
        workloads::thumbnail_throughput_indexing(&service, &fixtures, &run_dir, 10),
    );
    let nav_fixtures = [
        primary.clone(),
        fixtures
            .iter()
            .find(|fixture| fixture.rel_path.contains("iso204800"))
            .expect("highlight-stress fixture")
            .clone(),
    ];
    workloads_json.insert(
        "navigation-memory-bound".to_string(),
        workloads::navigation_memory_bound(&service, &nav_fixtures, &run_dir, 24),
    );
    workloads_json.insert(
        "gpu-failure-modes".to_string(),
        workloads::gpu_failure_modes(&run_dir),
    );

    let counters_final = counters::sample();
    let lap_revision = git(&["rev-parse", "HEAD"], &repo_root()).unwrap_or_default();
    assert_eq!(
        lap_revision.len(),
        40,
        "lap revision must be a 40-hex git revision (got '{lap_revision}')"
    );
    let engine_root = PathBuf::from("C:/Users/Thomas/Desktop/RapidRAW-engine");
    let engine_checkout = engine_checkout_matches_pin(&engine_root);

    let results = serde_json::json!({
        "schema": "lap-raw-platform-results/v1",
        "issue": "lap-c0e",
        "task": "TASK-601",
        "machineId": machine.id,
        "executed": {
            "host": format!("{}/{}", machine.cpu, gpu.adapter_name),
            "command": "lap-raw-platform.exe full",
            "startedUtc": started,
            "finishedUtc": utc_now(),
            "durationSeconds": overall.elapsed().as_secs(),
            "os": os_id(),
            "lapRevision": lap_revision,
            "engineRevision": ENGINE_REVISION,
            "engineCheckout": engine_checkout,
        },
        "hardware": {
            "cpu": machine.cpu,
            "ramBytes": counters_final.total_phys_bytes,
            "memoryLoadPercent": counters_final.memory_load_percent,
        },
        "gpu": {
            "adapter": gpu.adapter_name,
            "backend": gpu.backend,
            "maxTextureDimension2d": gpu.max_texture_dimension_2d,
            "maxBufferSize": gpu.max_buffer_size,
            "vramDedicatedBytes": counters_final.vram_dedicated_bytes,
            "vramAdapter": counters_final.vram_adapter,
        },
        "fixtures": fixtures.iter().map(|fixture| fixture.entry()).collect::<Vec<_>>(),
        "workloads": serde_json::Value::Object(workloads_json),
        "finalMemory": counters_final.series_entry("run-final"),
    });

    let out_path = out_override.unwrap_or_else(|| {
        repo_root()
            .join("tests/raw-development/platform/runs")
            .join(format!("platform-results-{}.json", machine.id))
    });
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).expect("results dir");
    }
    write_json(&out_path, &results);
    // Keep the raw measurements also inside the timestamped run directory.
    write_json(
        &run_dir.join(format!("platform-results-{}.json", machine.id)),
        &results,
    );

    println!(
        "== lap-raw-platform: complete in {}s ==",
        overall.elapsed().as_secs()
    );
    println!("results: {}", out_path.display());
    println!("run dir: {}", run_dir.display());
}

fn chrono_compact() -> String {
    // Compact local run-dir stamp: UTC YYYYmmdd-HHmmss.
    let stamp = utc_now();
    stamp
        .replace(['-', ':'], "")
        .replace("T", "-")
        .trim_end_matches('Z')
        .to_string()
}
