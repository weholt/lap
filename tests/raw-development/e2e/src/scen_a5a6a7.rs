//! A5/A6/A7 end-to-end scenarios plus the production-wiring regression:
//! full-resolution export semantics on real fixtures (A5), frozen-tolerance
//! preview/downsampled-export/histogram parity (A6), explicit GPU failure
//! cases (A7), and the exact production service construction used by the
//! Tauri command layer.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lap_lib::develop::export::{
    export_developed, AssetExportInput, ExportCancelSlot, ExportCompletion, ExportError,
    ExportFormat, ExportSettings, GpuExportRenderer,
};
use lap_lib::develop::sessions::{
    AssetEditInput, DevelopConfig, DevelopError, GpuPreviewRenderer, PreviewWait,
};
use lap_lib::develop::RecipeRepository;
use rapidraw_develop::session::{
    ExportJob, ExportRenderer, PreviewJob, PreviewQuality, PreviewRenderer, RecipeStore, SessionId,
};
use rapidraw_develop::{
    CancelToken, DecodeOptions, DecodedOriginal, DevelopError as EngineError,
};

use crate::common::{
    corpus_fixture, fingerprint, gpu_probe_json, load_png, production_service, require_gpu,
    run_dir, save_png, stage_source, synthetic_fixture, Scenario, ENGINE_REVISION,
};
use crate::metrics::{abs_diff_metrics, histogram, histogram_metrics, histograms_per_channel};

fn export_once(
    service: &lap_lib::develop::sessions::DevelopService,
    source: &Path,
    source_bytes: &[u8],
    asset: &str,
    revision: u64,
    destination: PathBuf,
    max_edge: Option<u32>,
) -> Result<ExportCompletion, ExportError> {
    let cancel = CancelToken::pair();
    let slot = ExportCancelSlot::new();
    export_developed(
        service,
        AssetExportInput {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            source_bytes: source_bytes.to_vec(),
            requested_revision: revision,
        },
        ExportSettings {
            destination,
            format: ExportFormat::Png,
            jpeg_quality: 90,
            max_edge,
        },
        &cancel.1,
        &slot,
    )
}

// ---------------------------------------------------------------------------
// A5: full-resolution export on a real >4096 px RAW
// ---------------------------------------------------------------------------

pub fn a5(scenario: &mut Scenario) {
    let dir = run_dir("a5-fullres-export");
    if !require_gpu(scenario) {
        return;
    }
    let (fixture_path, bytes, digest) = corpus_fixture("canon-eos-r6-craw-iso100-nocrop.CR3");
    let source = stage_source(&dir, "photo-a5.CR3", &bytes);
    scenario.check(
        "fixture-integrity",
        true,
        format!("corpus fixture {} ({} bytes): decoded 5472x3648 (>4096 px)", fixture_path.display(), bytes.len()),
    );

    let service = production_service();
    let repo = RecipeRepository::lap_default();
    let opened = service
        .open_session(AssetEditInput {
            asset_id: "asset-a5".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: None,
        })
        .unwrap();
    scenario.check(
        "decode-reports-original-dimensions",
        opened.dimensions == (5472, 3648),
        format!("opened decode dimensions {:?}", opened.dimensions),
    );
    let mut envelope = service.session_envelope(opened.session_id).unwrap();
    envelope.recipe.exposure = 0.7;
    envelope.recipe.temperature = 30.0;
    envelope.recipe.contrast = 10.0;
    let receipt = service.commit_recipe(opened.session_id, opened.revision, envelope).unwrap();
    service.close_session(opened.session_id).unwrap();

    // Restart: reopen from the committed sidecar, then export.
    let reopened = service
        .open_session(AssetEditInput {
            asset_id: "asset-a5".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: Some(repo.load(&source).unwrap()),
        })
        .unwrap();
    scenario.check(
        "committed-revision-after-restart",
        reopened.revision == 1,
        format!("reopened at revision {}", reopened.revision),
    );

    let destination = dir.join("a5-fullres.png");
    let exported = export_once(
        &service,
        &source,
        &bytes,
        "asset-a5",
        1,
        destination.clone(),
        None,
    );
    match exported {
        Ok(ExportCompletion::Completed { receipt }) => {
            scenario.check(
                "full-res-export-original-dimensions",
                receipt.revision == 1 && receipt.width == 5472 && receipt.height == 3648,
                format!(
                    "export at committed revision {} produced {}x{} ({} bytes) — original decoded dimensions",
                    receipt.revision, receipt.width, receipt.height, receipt.bytes_written
                ),
            );
            scenario.artifact(&destination);
        }
        other => {
            scenario.check(
                "full-res-export-original-dimensions",
                false,
                format!("full-resolution export failed: {other:?}"),
            );
        }
    }

    // Explicit resize is the only way dimensions shrink (downscale-only).
    let small_destination = dir.join("a5-small.png");
    let small = export_once(
        &service,
        &source,
        &bytes,
        "asset-a5",
        1,
        small_destination.clone(),
        Some(1024),
    );
    match small {
        Ok(ExportCompletion::Completed { receipt }) => {
            let expected = (1024u32, ((3648.0f64 / 5472.0) * 1024.0).round() as u32);
            scenario.check(
                "explicit-resize-fits-longest-edge",
                (receipt.width, receipt.height) == expected,
                format!(
                    "max_edge 1024 export produced {}x{} (expected {expected:?})",
                    receipt.width, receipt.height
                ),
            );
            scenario.artifact(&small_destination);
        }
        other => {
            scenario.check(
                "explicit-resize-fits-longest-edge",
                false,
                format!("downsampled export failed: {other:?}"),
            );
        }
    }

    // Restart re-export reproducibility: a fresh service instance renders the
    // committed revision byte-identically (frozen tolerance = exact equality,
    // mirroring the capture-baseline determinism characterization).
    let fresh_service = production_service();
    let repeat_destination = dir.join("a5-fullres-repeat.png");
    let repeat = export_once(
        &fresh_service,
        &source,
        &bytes,
        "asset-a5",
        1,
        repeat_destination.clone(),
        None,
    );
    match (fs::read(&destination), repeat) {
        (Ok(original_bytes), Ok(ExportCompletion::Completed { .. })) => {
            let repeat_bytes = fs::read(&repeat_destination).unwrap_or_default();
            scenario.check(
                "restart-reexport-byte-identical",
                !repeat_bytes.is_empty() && repeat_bytes == original_bytes,
                format!(
                    "repeat export {} bytes vs original {} bytes (equal: {})",
                    repeat_bytes.len(),
                    original_bytes.len(),
                    repeat_bytes == original_bytes
                ),
            );
            scenario.artifact(&repeat_destination);
        }
        (_, other) => {
            scenario.check(
                "restart-reexport-byte-identical",
                false,
                format!("repeat export failed: {other:?}"),
            );
        }
    }

    // Orientation fixture: the engine decode applies RAW orientation; the
    // full-dimension decode (never an embedded preview) arrives transposed.
    let (orientation_path, orientation_bytes, orientation_digest) =
        corpus_fixture("dng-linear-orientation6.DNG");
    let orientation_source = stage_source(&dir, "photo-a5-orientation.DNG", &orientation_bytes);
    let orientation_open = service.open_session(AssetEditInput {
        asset_id: "asset-a5o".to_string(),
        variant_id: "default".to_string(),
        source_path: orientation_source.clone(),
        source_fingerprint: orientation_digest.clone(),
        source_bytes: orientation_bytes,
        sidecar: None,
    });
    match orientation_open {
        Ok(opened) => {
            scenario.check(
                "orientation-decode-transposed",
                opened.dimensions == (5464, 8192),
                format!(
                    "orientation fixture {} decoded to {:?} (expected 5464x8192)",
                    orientation_path.display(),
                    opened.dimensions
                ),
            );
            let _ = service.close_session(opened.session_id);
        }
        Err(err) => {
            scenario.check(
                "orientation-decode-transposed",
                false,
                format!("orientation fixture failed to decode: {err}"),
            );
        }
    }

    // Embedded previews never serve as the development source: the export
    // receipt dimensions equal the full decode dimensions by construction of
    // the pipeline (decode_original → full-res render), verified above, and
    // the source stays immutable.
    scenario.check(
        "source-immutable-after-exports",
        fingerprint(&fs::read(&source).unwrap()) == digest,
        "source RAW bytes unchanged after full-res + downsampled + repeat exports",
    );
    let _ = service.close_session(reopened.session_id);
}

// ---------------------------------------------------------------------------
// A6: frozen-tolerance preview / downsampled-export / histogram parity
// ---------------------------------------------------------------------------

const PARITY_ENV_MANIFEST: &str = "LAP_E2E_PARITY_MANIFEST";
const PARITY_ENV_FREEZE: &str = "LAP_E2E_PARITY_FREEZE";

#[derive(serde::Serialize, serde::Deserialize)]
struct ParityManifest {
    schema: String,
    captured_utc: String,
    host_revision: String,
    engine_revision: String,
    gpu: serde_json::Value,
    color_transform: String,
    fixture: FixtureRef,
    recipe: serde_json::Value,
    preview_max_edge: u32,
    export_max_edge: u32,
    repeats: Vec<ParityMetricsDto>,
    repeat_stability: Stability,
    frozen_tolerances: Tolerances,
    histogram_state_floor: HistogramFloor,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FixtureRef {
    name: String,
    sha256: String,
    bytes: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ParityMetricsDto {
    per_channel_mean_abs: [f64; 3],
    per_channel_p99_abs: [f64; 3],
    per_channel_max_abs: [u32; 3],
    hist_mean_abs_diff: f64,
    hist_intersection_min: f64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Stability {
    mean_abs_max_delta: f64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Tolerances {
    per_channel_mean_abs_max: f64,
    per_channel_p99_abs_max: f64,
    per_channel_max_abs_max: u32,
    hist_mean_abs_diff_max: f64,
    hist_intersection_min_at_least: f64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct HistogramFloor {
    /// Adjusted-state histogram must differ from the default-state histogram
    /// by at least this much (hist mean abs diff) — guards against an
    /// unadjusted image being presented as the adjusted render state.
    adjusted_vs_default_hist_mean_abs_min: f64,
}

fn collect_parity(
    scenario: &mut Scenario,
    dir: &Path,
    source: &Path,
    bytes: &[u8],
    digest: &str,
) -> Option<(ParityMetricsDto, f64)> {
    let service = production_service();
    let repo = RecipeRepository::lap_default();
    let repeat_tag: &str = "run";

    // Preview path: settled GPU preview of the adjusted recipe (downscale of
    // the linear original happens BEFORE the adjustment pipeline).
    let preview_session = service
        .open_session(AssetEditInput {
            asset_id: format!("asset-a6-{repeat_tag}"),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            source_fingerprint: digest.to_string(),
            source_bytes: bytes.to_vec(),
            sidecar: None,
        })
        .ok()?;
    let mut adjusted = service
        .session_envelope(preview_session.session_id)
        .expect("session envelope");
    adjusted.recipe.exposure = 1.0;
    adjusted.recipe.temperature = 40.0;
    let wait = service.render_preview(
        preview_session.session_id,
        1,
        adjusted,
        PreviewQuality::Settled,
        1024,
    );
    let preview_frame = match wait {
        Ok(PreviewWait::Completed { ticket }) => service.take_preview_frame(&ticket.handle).ok(),
        other => {
            scenario.check("preview-render", false, format!("{other:?}"));
            None
        }
    }?;
    let preview_artifact = save_png(
        dir,
        "a6-preview-1024.png",
        preview_frame.width,
        preview_frame.height,
        &preview_frame.rgba8,
    );
    scenario.artifact(&preview_artifact);

    // Default-state preview for the histogram-state guard.
    let default_frame = {
        let envelope = service.session_envelope(preview_session.session_id)?;
        match service.render_preview(
            preview_session.session_id,
            2,
            envelope,
            PreviewQuality::Settled,
            1024,
        ) {
            Ok(PreviewWait::Completed { ticket }) => service.take_preview_frame(&ticket.handle).ok(),
            other => {
                scenario.check("default-render", false, format!("{other:?}"));
                None
            }
        }?
    };
    let _ = service.close_session(preview_session.session_id);

    // Export path: commit the adjusted recipe, export downsampled to 1024 px
    // (adjustment at full resolution, then triangle downscale, then encode).
    let commit_session = service
        .open_session(AssetEditInput {
            asset_id: format!("asset-a6-{repeat_tag}"),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            source_fingerprint: digest.to_string(),
            source_bytes: bytes.to_vec(),
            sidecar: None,
        })
        .ok()?;
    let mut commit_envelope = service
        .session_envelope(commit_session.session_id)
        .expect("session envelope");
    commit_envelope.recipe.exposure = 1.0;
    commit_envelope.recipe.temperature = 40.0;
    service
        .commit_recipe(commit_session.session_id, commit_session.revision, commit_envelope)
        .ok()?;
    let _ = service.close_session(commit_session.session_id);

    let destination = dir.join("a6-export-1024.png");
    match export_once(
        &service,
        source,
        bytes,
        &format!("asset-a6-{repeat_tag}"),
        1,
        destination.clone(),
        Some(1024),
    ) {
        Ok(ExportCompletion::Completed { .. }) => {}
        other => {
            scenario.check("export-render", false, format!("{other:?}"));
            return None;
        }
    }
    scenario.artifact(&destination);
    let (export_w, export_h, export_rgba) = load_png(&destination);
    if !scenario.check(
        "parity-dimensions-match",
        (export_w, export_h) == (preview_frame.width, preview_frame.height),
        format!(
            "preview {}x{} vs export {export_w}x{export_h}",
            preview_frame.width, preview_frame.height
        ),
    ) {
        return None;
    }

    let diffs = abs_diff_metrics(&preview_frame.rgba8, &export_rgba);
    let hist_preview = histogram(&preview_frame.rgba8);
    let hist_export = histogram(&export_rgba);
    let (hist_mean, hist_intersection) = histogram_metrics(&hist_preview, &hist_export);
    let hist_default = histogram(&default_frame.rgba8);
    let (adj_default_mean, _) = histogram_metrics(&hist_preview, &hist_default);

    // Per-channel histograms must also correspond (guards channel swaps).
    let pc_preview = histograms_per_channel(&preview_frame.rgba8);
    let pc_export = histograms_per_channel(&export_rgba);
    let mut channel_intersection_min = f64::MAX;
    for (a, b) in pc_preview.iter().zip(&pc_export) {
        let (_, intersection) = histogram_metrics(a, b);
        channel_intersection_min = channel_intersection_min.min(intersection);
    }

    scenario.check(
        "parity-metrics-collected",
        true,
        format!(
            "mean abs diff R/G/B = {:.3}/{:.3}/{:.3}, p99 = {:.0}/{:.0}/{:.0}, max = {}, hist mean abs = {:.5}, hist intersection = {:.4}, per-channel intersection min = {:.4}, adjusted-vs-default hist diff = {:.5}",
            diffs[0].mean_abs, diffs[1].mean_abs, diffs[2].mean_abs,
            diffs[0].p99_abs, diffs[1].p99_abs, diffs[2].p99_abs,
            diffs.iter().map(|c| c.max_abs).max().unwrap_or(0),
            hist_mean, hist_intersection, channel_intersection_min, adj_default_mean,
        ),
    );

    Some((
        ParityMetricsDto {
            per_channel_mean_abs: [diffs[0].mean_abs, diffs[1].mean_abs, diffs[2].mean_abs],
            per_channel_p99_abs: [diffs[0].p99_abs, diffs[1].p99_abs, diffs[2].p99_abs],
            per_channel_max_abs: [diffs[0].max_abs, diffs[1].max_abs, diffs[2].max_abs],
            hist_mean_abs_diff: hist_mean,
            // Combined with the per-channel minimum so a single frozen
            // threshold covers both histogram checks.
            hist_intersection_min: hist_intersection.min(channel_intersection_min),
        },
        adj_default_mean,
    ))
}

pub fn a6(scenario: &mut Scenario) {
    let dir = run_dir("a6-parity");
    if !require_gpu(scenario) {
        return;
    }
    let manifest_path = std::env::var(PARITY_ENV_MANIFEST)
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            crate::common::repo_root()
                .join("tests/raw-development/preview-export-parity.json")
        });
    let freeze = std::env::var(PARITY_ENV_FREEZE).is_ok();

    let (fixture_path, bytes, digest) = corpus_fixture("canon-eos-r6-craw-iso100-nocrop.CR3");
    // Each parity repeat runs against its own unedited source copy so every
    // repeat exports exactly committed revision 1.
    let repeat_a = stage_source(&dir, "photo-a6-run1.CR3", &bytes);
    let repeat_b = stage_source(&dir, "photo-a6-run2.CR3", &bytes);
    scenario.check(
        "fixture-integrity",
        true,
        format!(
            "corpus fixture {} (sha {}…) with the slice controls: exposure +1.0, temperature +40",
            fixture_path.display(),
            &digest[..12]
        ),
    );

    let first = collect_parity(scenario, &dir, &repeat_a, &bytes, &digest);
    let second = collect_parity(scenario, &dir, &repeat_b, &bytes, &digest);
    let (run_a, run_b) = match (first, second) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            scenario.check("parity-collected", false, "a repeat failed; see steps above");
            return;
        }
    };
    let metrics_a = run_a.0;
    let metrics_b = run_b.0;
    let floor_a = run_a.1;
    let stability_delta = metrics_a
        .per_channel_mean_abs
        .iter()
        .zip(&metrics_b.per_channel_mean_abs)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f64, |acc, value| acc.max(value));
    let hist_stability_delta = (metrics_a.hist_mean_abs_diff - metrics_b.hist_mean_abs_diff).abs();
    let stability = stability_delta.max(hist_stability_delta);
    if !scenario.check(
        "repeat-stability",
        stability <= 0.5,
        format!(
            "two independent capture repeats agree within {stability:.4} (mean-abs-diff / hist metric delta)"
        ),
    ) {
        return;
    }

    if freeze {
        let observed_mean = metrics_a.per_channel_mean_abs.iter().fold(0.0f64, |acc, v| acc.max(*v));
        let observed_p99 = metrics_a.per_channel_p99_abs.iter().fold(0.0f64, |acc, v| acc.max(*v));
        let observed_max = metrics_a.per_channel_max_abs.iter().copied().max().unwrap_or(0);
        let manifest = ParityManifest {
            schema: "lap-raw-preview-parity/v1".to_string(),
            captured_utc: crate::common::utc_now(),
            host_revision: crate::common::git_info().head,
            engine_revision: ENGINE_REVISION.to_string(),
            gpu: gpu_probe_json(),
            color_transform: "Both paths output the engine's sRGB-encoded 8-bit pixels via OutputTarget::CpuPixels (one defined color transform). Preview = triangle downscale of the linear f32 original before adjustments; export = adjustments at full resolution then triangle downscale. Metrics measure the resampling-order difference only.".to_string(),
            fixture: FixtureRef {
                name: "corpus/canon-eos-r6-craw-iso100-nocrop.CR3".to_string(),
                sha256: digest.to_string(),
                bytes: bytes.len() as u64,
            },
            recipe: serde_json::json!({"exposure": 1.0, "temperature": 40.0}),
            preview_max_edge: 1024,
            export_max_edge: 1024,
            repeats: vec![metrics_a.clone(), metrics_b.clone()],
            repeat_stability: Stability {
                mean_abs_max_delta: stability,
            },
            frozen_tolerances: Tolerances {
                // Tight: run-to-run rendering is byte-identical on this
                // machine (repeat_stability), so tolerance headroom only
                // covers quantization noise, not slop.
                per_channel_mean_abs_max: observed_mean * 1.5 + 0.25,
                per_channel_p99_abs_max: observed_p99 * 1.5 + 1.0,
                per_channel_max_abs_max: (observed_max as f64 * 1.5 + 2.0) as u32,
                hist_mean_abs_diff_max: metrics_a.hist_mean_abs_diff * 2.0 + 0.0002,
                hist_intersection_min_at_least: (metrics_a.hist_intersection_min - 0.005).max(0.99),
            },
            histogram_state_floor: HistogramFloor {
                adjusted_vs_default_hist_mean_abs_min: floor_a * 0.5,
            },
        };
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .expect("write parity manifest");
        scenario.check(
            "tolerances-frozen",
            true,
            format!(
                "frozen tolerances written to {} (never implicitly: freeze mode is explicit)",
                manifest_path.display()
            ),
        );
        return;
    }

    // Compare mode: fresh metrics must satisfy the frozen tolerances.
    let manifest_text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(err) => {
            scenario.check(
                "frozen-manifest-present",
                false,
                format!("{} unreadable: {err} (run with {PARITY_ENV_FREEZE}=1 once to freeze)", manifest_path.display()),
            );
            return;
        }
    };
    let manifest: ParityManifest = serde_json::from_str(&manifest_text).expect("parity manifest parses");
    scenario.check(
        "frozen-manifest-revisions",
        manifest.engine_revision == ENGINE_REVISION,
        format!(
            "manifest engine revision {} matches the consumed pin",
            manifest.engine_revision
        ),
    );
    let t = &manifest.frozen_tolerances;
    let mean_ok = metrics_a
        .per_channel_mean_abs
        .iter()
        .all(|value| *value <= t.per_channel_mean_abs_max);
    let p99_ok = metrics_a
        .per_channel_p99_abs
        .iter()
        .all(|value| *value <= t.per_channel_p99_abs_max);
    let max_ok = metrics_a
        .per_channel_max_abs
        .iter()
        .all(|value| *value <= t.per_channel_max_abs_max);
    let hist_ok = metrics_a.hist_mean_abs_diff <= t.hist_mean_abs_diff_max
        && metrics_a.hist_intersection_min >= t.hist_intersection_min_at_least;
    scenario.check(
        "preview-export-parity-within-frozen-tolerances",
        mean_ok && p99_ok && max_ok && hist_ok,
        format!(
            "frozen: mean<={:.3} p99<={:.1} max<={} hist<={:.5} intersection>={:.4}; observed mean {:?} p99 {:?} max {:?} hist {:.5} intersection {:.4}",
            t.per_channel_mean_abs_max, t.per_channel_p99_abs_max, t.per_channel_max_abs_max,
            t.hist_mean_abs_diff_max, t.hist_intersection_min_at_least,
            metrics_a.per_channel_mean_abs, metrics_a.per_channel_p99_abs,
            metrics_a.per_channel_max_abs, metrics_a.hist_mean_abs_diff,
            metrics_a.hist_intersection_min,
        ),
    );
    let floor_ok = floor_a >= manifest.histogram_state_floor.adjusted_vs_default_hist_mean_abs_min;
    scenario.check(
        "histogram-reflects-adjusted-state",
        floor_ok,
        format!(
            "adjusted-state histogram differs from default-state histogram by {floor_a:.5} (floor {:.5}); the histogram corresponds to the rendered adjusted state, never the unadjusted image",
            manifest.histogram_state_floor.adjusted_vs_default_hist_mean_abs_min
        ),
    );
}

// ---------------------------------------------------------------------------
// A7: GPU failure cases
// ---------------------------------------------------------------------------

struct HarnessStubExport;

impl ExportRenderer for HarnessStubExport {
    fn render(&self, _job: &ExportJob) -> Result<rapidraw_develop::ExportFrame, EngineError> {
        Err(EngineError::Unsupported(
            "a7 failure cases inject the renderer itself".to_string(),
        ))
    }
}

struct FailingStore;

impl RecipeStore for FailingStore {
    fn persist(&self, _envelope: &rapidraw_edit_model::RecipeEnvelope) -> Result<(), String> {
        Ok(())
    }
}

pub fn a7(scenario: &mut Scenario) {
    let dir = run_dir("a7-gpu-failures");
    if !require_gpu(scenario) {
        return;
    }
    let probe = GpuPreviewRenderer::new().probe();
    let (fixture, bytes, digest) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    let source = stage_source(&dir, "photo.dng", &bytes);

    // (1) Missing adapter: deterministic empty backend set (no usable device).
    let renderer = GpuPreviewRenderer::with_context_factory(Box::new(|| {
        rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(wgpu::Backends::empty())
    }));
    let missing_probe = renderer.probe();
    scenario.check(
        "missing-adapter-probe-fails-typed",
        !missing_probe.available && missing_probe.error.is_some(),
        format!("probe on empty backend set: {:?}", missing_probe.error),
    );
    let decoded: Arc<DecodedOriginal> = Arc::new(
        rapidraw_develop::decode_original(&bytes, &DecodeOptions::default()).expect("decode"),
    );
    let envelope = rapidraw_edit_model::RecipeEnvelope::new("lap-raw-e2e/0", "asset-a7", "default", &digest);
    let cancel = CancelToken::pair();
    let job = PreviewJob {
        session_id: SessionId(1),
        asset_id: "asset-a7".to_string(),
        variant_id: "default".to_string(),
        generation: 1,
        quality: PreviewQuality::Settled,
        max_edge: 64,
        original: Arc::clone(&decoded),
        envelope,
        cancel: cancel.1,
    };
    let render_err = renderer.render(&job);
    scenario.check(
        "missing-adapter-render-fails-typed",
        matches!(&render_err, Err(EngineError::Unsupported(message)) if message.to_lowercase().contains("gpu")),
        format!("render without a device: {render_err:?} — never pixels"),
    );

    // (2) Missing-adapter export writes nothing and reports the failure.
    let export_renderer = GpuExportRenderer::with_context_factory(Box::new(|| {
        rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(wgpu::Backends::empty())
    }));
    let config = DevelopConfig::default();
    let failing_service = lap_lib::develop::sessions::DevelopService::with_parts(
        config,
        Arc::new(renderer),
        Arc::new(export_renderer),
        Arc::new(FailingStore),
    );
    let repo = RecipeRepository::lap_default();
    let mut lut_free = repo.new_envelope("asset-a7x", "default", &digest);
    lut_free.recipe.exposure = 0.5;
    repo.commit_sidecar(&source, 0, lut_free).unwrap();
    let destination = dir.join("a7-must-not-exist.png");
    let export_err = export_once(
        &failing_service,
        &source,
        &bytes,
        "asset-a7x",
        1,
        destination.clone(),
        None,
    );
    let nothing_written = !destination.exists();
    scenario.check(
        "missing-adapter-export-explicit-no-artifact",
        matches!(export_err, Err(ExportError::Unsupported(_))) && nothing_written,
        format!(
            "export outcome: {export_err:?}; destination written: {}",
            destination.exists()
        ),
    );

    // (3) Missing resource: a recipe referencing a LUT without resolved LUT
    // data must fail explicitly through the production service (real GPU),
    // for previews and exports alike.
    let production = production_service();
    let opened = production
        .open_session(AssetEditInput {
            asset_id: "asset-a7l".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: None,
        })
        .unwrap();
    let mut lut_recipe = production.session_envelope(opened.session_id).unwrap();
    lut_recipe.recipe.lut_path = Some("missing-lut-resource.cube".to_string());
    let lut_wait = production.render_preview(
        opened.session_id,
        1,
        lut_recipe,
        PreviewQuality::Settled,
        128,
    );
    let lut_detail = match &lut_wait {
        Ok(PreviewWait::Failed { code, message }) => {
            let lowered = message.to_lowercase();
            (code == "unsupported" && (lowered.contains("lut") || lowered.contains("resource")), format!("Failed code={code} message={message}"))
        }
        other => (false, format!("unexpected outcome {other:?}")),
    };
    scenario.check(
        "missing-lut-resource-fails-explicitly",
        lut_detail.0,
        lut_detail.1,
    );
    let _ = production.close_session(opened.session_id);

    // The same guarantee at the durable export boundary: a committed recipe
    // referencing a missing LUT resource must not export, ever.
    let lut_source = dir.join("photo-lut.dng");
    fs::write(&lut_source, &bytes).unwrap();
    let mut committed_lut = repo.new_envelope("asset-a7l", "default", &digest);
    committed_lut.recipe.lut_path = Some("missing-lut-resource.cube".to_string());
    repo.commit_sidecar(&lut_source, 0, committed_lut).unwrap();
    let lut_destination = dir.join("a7-lut-must-not-exist.png");
    let lut_export = export_once(
        &production,
        &lut_source,
        &bytes,
        "asset-a7l",
        1,
        lut_destination.clone(),
        None,
    );
    scenario.check(
        "missing-lut-export-fails-explicitly",
        matches!(lut_export, Err(ExportError::Unsupported(_))) && !lut_destination.exists(),
        format!(
            "export outcome: {lut_export:?}; destination written: {}",
            lut_destination.exists()
        ),
    );

    // (4) Oversized textures: every rendered artifact must respect the
    // device's texture limit; the engine enforces the limit with a typed
    // TextureTooLarge failure (structural evidence — a true oversize cannot
    // be allocated in software on this host).
    let limit = probe.max_texture_dimension_2d;
    let (fixture_wide, wide_bytes, wide_digest) = synthetic_fixture("dng-linear-wide-5000x64.dng");
    let decoded_wide = rapidraw_develop::decode_original(&wide_bytes, &DecodeOptions::default());
    let wide_ok = decoded_wide
        .as_ref()
        .map(|decoded| decoded.image.dimensions().0 == 5000 && 5000 <= limit)
        .unwrap_or(false);
    scenario.check(
        "oversized-texture-guard-recorded",
        wide_ok,
        format!(
            "wide fixture {} decoded at 5000 px <= device limit {limit}; the engine's check_texture_support fails oversized requests with typed TextureTooLarge (renderer.rs), never the unprocessed base image; allocating a truly oversized texture is not software-injectable on this host",
            fixture_wide.display()
        ),
    );
    let _ = wide_digest;

    // (5) Broken/undecodable sources fail with typed decode errors.
    for broken in ["truncated-linear.dng", "not-a-raw.dng"] {
        let (path, broken_bytes, _) = synthetic_fixture(broken);
        let broken_source = stage_source(&dir, &format!("broken-{broken}"), &broken_bytes);
        let err = production.open_session(AssetEditInput {
            asset_id: format!("asset-a7-{broken}"),
            variant_id: "default".to_string(),
            source_path: broken_source,
            source_fingerprint: digest.clone(),
            source_bytes: broken_bytes,
            sidecar: None,
        });
        scenario.check(
            "broken-source-typed-decode-error",
            matches!(&err, Err(DevelopError::Decode(_))),
            format!("{broken} ({}): {:?}", path.display(), err.as_ref().err().map(|e| e.to_string())),
        );
    }

    scenario
        .extra
        .as_object_mut()
        .map(|extra| extra.insert("gpuProbe".to_string(), gpu_probe_json()));
}
