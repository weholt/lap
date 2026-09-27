//! End-to-end A1-A7 small-slice qualification harness for the Lap
//! raw-development slice (lap-70c / TASK-306).
//!
//! Every scenario drives the real application library (`lap_lib::develop`)
//! and the pinned engine revision against the licensed RAW corpus on the real
//! Windows GPU backend. GPU absence fails the gate; nothing is skipped and no
//! layer under qualification is replaced by a mock.

mod common;
mod metrics;
mod scen_a1a2;
mod scen_a3a4;
mod scen_a5a6a7;

use common::{require_gpu, run_dir, stage_source, synthetic_fixture, Scenario};
use lap_lib::develop::export::{
    export_developed, AssetExportInput, ExportCancelSlot, ExportCompletion, ExportFormat,
    ExportSettings,
};
use lap_lib::develop::sessions::AssetEditInput;
use lap_lib::develop::RecipeRepository;
use rapidraw_develop::CancelToken;

/// Production-wiring regression: builds the develop service EXACTLY like the
/// Tauri command layer (`t_cmds::DevelopAppState::service` via
/// `DevelopService::with_gpu_and_sidecar_store`) and runs a full durable
/// export through it. This is the scenario that pins the production export
/// wiring; it failed RED before the stub renderer was replaced (see
/// docs/raw-development/slice-qualification.md).
fn wiring(scenario: &mut Scenario) {
    let dir = run_dir("production-wiring-export");
    if !require_gpu(scenario) {
        return;
    }
    let (fixture, bytes, digest) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    let source = stage_source(&dir, "photo.dng", &bytes);
    scenario.check(
        "fixture-integrity",
        true,
        format!("fixture {} (sha {}…)", fixture.display(), &digest[..12]),
    );

    let service = common::production_service();
    let opened = service
        .open_session(AssetEditInput {
            asset_id: "asset-wiring".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: None,
        })
        .unwrap();
    let mut envelope = service.session_envelope(opened.session_id).unwrap();
    envelope.recipe.exposure = 0.6;
    let commit = service.commit_recipe(opened.session_id, opened.revision, envelope);
    if !scenario.check(
        "commit-through-production-service",
        matches!(&commit, Ok(receipt) if receipt.revision == 1),
        format!("commit: {commit:?}"),
    ) {
        return;
    }
    let _ = service.close_session(opened.session_id);

    let cancel = CancelToken::pair();
    let slot = ExportCancelSlot::new();
    let destination = dir.join("wiring-export.png");
    let exported = export_developed(
        &service,
        AssetExportInput {
            asset_id: "asset-wiring".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_bytes: bytes.clone(),
            requested_revision: 1,
        },
        ExportSettings {
            destination: destination.clone(),
            format: ExportFormat::Png,
            jpeg_quality: 90,
            max_edge: None,
        },
        &cancel.1,
        &slot,
    );
    match exported {
        Ok(ExportCompletion::Completed { receipt }) => {
            scenario.check(
                "export-through-production-service",
                receipt.revision == 1 && receipt.bytes_written > 0,
                format!(
                    "durable export through the exact production service construction wrote {} ({} bytes)",
                    receipt.destination.display(),
                    receipt.bytes_written
                ),
            );
            scenario.artifact(&destination);
        }
        other => {
            scenario.check(
                "export-through-production-service",
                false,
                format!("production-wiring export failed: {other:?}"),
            );
        }
    }
}

fn run_scenario(name: &str) -> bool {
    let mut scenario = Scenario::new(name);
    // A hard panic in one scenario must not take down the whole gate run.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name {
        "production-wiring" => wiring(&mut scenario),
        "probe-gate" => scen_a3a4::probe_gate(&mut scenario),
        "a1-source-immutability" => scen_a1a2::run(&mut scenario),
        "a2-ab-navigation" => scen_a1a2::a2(&mut scenario),
        "a3-process-termination" => scen_a3a4::a3(&mut scenario),
        "a4-conflict-stale" => scen_a3a4::a4(&mut scenario),
        "a5-fullres-export" => scen_a5a6a7::a5(&mut scenario),
        "a6-parity" => scen_a5a6a7::a6(&mut scenario),
        "a7-gpu-failures" => scen_a5a6a7::a7(&mut scenario),
        other => {
            scenario.check("scenario-known", false, format!("unknown scenario '{other}'"));
        }
    }));
    let panicked = outcome.is_err();
    if panicked {
        scenario.check(
            "scenario-completed",
            false,
            "scenario panicked (see stderr for the panic payload)".to_string(),
        );
    }
    let ok = scenario.passed() && !panicked;
    let dir = common::run_base();
    common::write_evidence(&scenario, &dir, ok);
    println!(
        "== scenario {name}: {} ({} steps, {} failed) ==",
        if ok { "PASS" } else { "FAIL" },
        scenario.steps.len(),
        scenario.steps.iter().filter(|s| !s.ok).count()
    );
    ok
}

fn main() {
    // a3 child mode: durable commit loop that the parent hard-kills.
    if let Ok(work_dir) = std::env::var("LAP_E2E_CRASH_LOOP_DIR") {
        let code = scen_a3a4::crash_loop_child(std::path::Path::new(&work_dir));
        std::process::exit(code);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let requested = if args.is_empty() {
        vec![
            "production-wiring".to_string(),
            "a1-source-immutability".to_string(),
            "a2-ab-navigation".to_string(),
            "a3-process-termination".to_string(),
            "a4-conflict-stale".to_string(),
            "a5-fullres-export".to_string(),
            "a6-parity".to_string(),
            "a7-gpu-failures".to_string(),
        ]
    } else {
        args
    };

    let mut results = Vec::new();
    for name in &requested {
        println!("== scenario {name}: starting ==");
        let ok = run_scenario(name);
        results.push((name.clone(), ok));
    }

    let mut summary = std::collections::BTreeMap::new();
    for (name, ok) in &results {
        summary.insert(name.clone(), serde_json::json!(ok));
    }
    let all_ok = results.iter().all(|(_, ok)| *ok);
    let summary_value = serde_json::json!({
        "schema": "lap-raw-e2e-summary/v1",
        "outcome": if all_ok { "pass" } else { "fail" },
        "scenarios": summary,
        "host": {
            "revision": common::git_info().head,
            "branch": common::git_info().branch,
        },
        "engineRevision": common::ENGINE_REVISION,
        "engineCheckoutMatchesPin": common::engine_checkout_matches_pin(),
        "gpu": common::gpu_probe_json(),
        "finishedUtc": common::utc_now(),
    });
    let summary_path = common::run_base().join("summary.json");
    std::fs::write(&summary_path, serde_json::to_vec_pretty(&summary_value).unwrap())
        .expect("write summary");
    println!("summary: {}", summary_path.display());
    println!(
        "== e2e gate: {} ==",
        if all_ok { "PASS" } else { "FAIL" }
    );
    std::process::exit(if all_ok { 0 } else { 1 });
}
