//! A1/A2 end-to-end scenarios: source immutability through the full
//! edit → restart → reset → export chain, and per-asset recipe isolation
//! across A/B navigation with restart (spec acceptance A1/A2).

use std::fs;
use std::path::PathBuf;

use lap_lib::develop::export::{
    export_developed, AssetExportInput, ExportCancelSlot, ExportCompletion, ExportError,
    ExportFormat, ExportSettings,
};
use lap_lib::develop::sessions::{AssetEditInput, PreviewWait};
use lap_lib::develop::RecipeRepository;
use rapidraw_develop::session::PreviewQuality;
use rapidraw_develop::CancelToken;

use crate::common::{
    corpus_fixture, fingerprint, production_service, require_gpu, run_dir, save_png,
    stage_source, synthetic_fixture, Scenario,
};

fn completed_frame(
    scenario: &mut Scenario,
    service: &lap_lib::develop::sessions::DevelopService,
    session_id: u64,
    generation: u64,
    max_edge: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    let envelope = service.session_envelope(session_id)?;
    let wait = service.render_preview(session_id, generation, envelope, PreviewQuality::Settled, max_edge);
    match wait {
        Ok(PreviewWait::Completed { ticket }) => {
            let frame = service.take_preview_frame(&ticket.handle).ok()?;
            Some((frame.width, frame.height, frame.rgba8))
        }
        other => {
            scenario.check(
                "preview-completed",
                false,
                format!("generation {generation} did not complete: {other:?}"),
            );
            None
        }
    }
}

pub fn run(scenario: &mut Scenario) {
    let dir = run_dir("a1-source-immutability");
    if !require_gpu(scenario) {
        return;
    }
    let (fixture_path, bytes, digest) = corpus_fixture("canon-eos-r6-craw-iso100-nocrop.CR3");
    scenario.check(
        "fixture-integrity",
        true,
        format!(
            "corpus fixture {} ({} bytes, sha256 {digest})",
            fixture_path.display(),
            bytes.len()
        ),
    );
    let source = stage_source(&dir, "photo.CR3", &bytes);
    scenario.check(
        "source-hash-before",
        fingerprint(&fs::read(&source).unwrap()) == digest,
        "staged source copy matches the corpus hash before any edit",
    );

    let service = production_service();

    // ---- open + default preview (real GPU render of the real decode)
    let opened = service.open_session(AssetEditInput {
        asset_id: "asset-a1".to_string(),
        variant_id: "default".to_string(),
        source_path: source.clone(),
        source_fingerprint: digest.clone(),
        source_bytes: bytes.clone(),
        sidecar: None,
    });
    if !scenario.check(
        "open-fresh-session",
        opened.is_ok(),
        format!("open: {:?}", opened.as_ref().map(|o| (o.session_id, o.revision, o.dimensions)).map_err(|e| e.to_string())),
    ) {
        return;
    }
    let opened = opened.unwrap();
    scenario.check(
        "fresh-revision-zero",
        opened.revision == 0,
        format!("fresh asset reports sidecar revision {}", opened.revision),
    );
    let default_frame = scenario
        .timed("default-preview-render", |s| {
            completed_frame(s, &service, opened.session_id, 1, 512)
        })
        .flatten();
    let default_frame = match default_frame {
        Some(frame) => frame,
        None => return,
    };
    let artifact = save_png(&dir, "a1-default-preview.png", default_frame.0, default_frame.1, &default_frame.2);
    scenario.artifact(&artifact);

    // ---- edit exposure + white balance, commit
    let mut edited = service.session_envelope(opened.session_id).unwrap();
    edited.recipe.exposure = 0.8;
    edited.recipe.temperature = 35.0;
    let receipt = scenario.timed("commit-exposure-wb", |_| {
        service.commit_recipe(opened.session_id, opened.revision, edited)
    });
    let receipt = match receipt {
        Some(Ok(receipt)) => {
            scenario.check(
                "commit-acknowledged",
                receipt.revision == 1 && receipt.content_hash.is_some(),
                format!("revision {} content-hash {:?} sidecar {:?} projection-applied {}",
                    receipt.revision, receipt.content_hash.as_deref().map(|h| &h[..h.len().min(12)]),
                    receipt.sidecar_path.as_ref().map(|p| p.display().to_string()), receipt.projection_applied),
            );
            Some(receipt)
        }
        other => {
            scenario.check("commit-acknowledged", false, format!("commit failed: {other:?}"));
            None
        }
    };
    let Some(_) = receipt else { return };

    // ---- restart: close and reopen from the durable sidecar
    let closed = service.close_session(opened.session_id);
    scenario.check("close-before-restart", closed.is_ok(), format!("{closed:?}"));
    let repo = RecipeRepository::lap_default();
    let reopened = service.open_session(AssetEditInput {
        asset_id: "asset-a1".to_string(),
        variant_id: "default".to_string(),
        source_path: source.clone(),
        source_fingerprint: digest.clone(),
        source_bytes: bytes.clone(),
        sidecar: Some(repo.load(&source).unwrap()),
    });
    if !scenario.check("reopen-after-restart", reopened.is_ok(), format!("{:?}", reopened.as_ref().err())) {
        return;
    }
    let reopened = reopened.unwrap();
    let restored = repo.load(&source).unwrap();
    scenario.check(
        "recipe-restored-after-restart",
        reopened.revision == 1
            && restored.recipe.exposure == 0.8
            && restored.recipe.temperature == 35.0,
        format!(
            "reopened revision {} restored exposure {} temperature {}",
            reopened.revision, restored.recipe.exposure, restored.recipe.temperature
        ),
    );

    let adjusted_frame = scenario
        .timed("adjusted-preview-render", |s| {
            completed_frame(s, &service, reopened.session_id, 1, 512)
        })
        .flatten();
    let adjusted_frame = match adjusted_frame {
        Some(frame) => frame,
        None => return,
    };
    let artifact = save_png(&dir, "a1-adjusted-preview.png", adjusted_frame.0, adjusted_frame.1, &adjusted_frame.2);
    scenario.artifact(&artifact);
    scenario.check(
        "adjusted-preview-differs",
        adjusted_frame.2 != default_frame.2,
        "the settled preview reflects the committed adjustment (not the unadjusted image)",
    );

    // ---- reset to the default recipe
    let default_envelope = repo.new_envelope("asset-a1", "default", &digest);
    let reset = service.commit_recipe(reopened.session_id, reopened.revision, default_envelope);
    scenario.check(
        "reset-commit",
        matches!(&reset, Ok(receipt) if receipt.revision == 2),
        format!("reset commit: {reset:?}"),
    );
    service.close_session(reopened.session_id).unwrap();
    let reopened2 = service.open_session(AssetEditInput {
        asset_id: "asset-a1".to_string(),
        variant_id: "default".to_string(),
        source_path: source.clone(),
        source_fingerprint: digest.clone(),
        source_bytes: bytes.clone(),
        sidecar: Some(repo.load(&source).unwrap()),
    })
    .unwrap();
    let after_reset = repo.load(&source).unwrap();
    scenario.check(
        "reset-restored-default",
        reopened2.revision == 2 && after_reset.recipe.exposure == 0.0 && after_reset.recipe.temperature == 0.0,
        format!(
            "after reset: revision {}, exposure {}, temperature {}",
            reopened2.revision, after_reset.recipe.exposure, after_reset.recipe.temperature
        ),
    );

    // ---- export the committed revision at full resolution (real GPU)
    let cancel = CancelToken::pair();
    let slot = ExportCancelSlot::new();
    let destination = dir.join("a1-export-rev2.png");
    let exported = scenario.timed("full-res-export", |_| {
        export_developed(
            &service,
            AssetExportInput {
                asset_id: "asset-a1".to_string(),
                variant_id: "default".to_string(),
                source_path: source.clone(),
                source_bytes: bytes.clone(),
                requested_revision: 2,
            },
            ExportSettings {
                destination: destination.clone(),
                format: ExportFormat::Png,
                jpeg_quality: 90,
                max_edge: None,
            },
            &cancel.1,
            &slot,
        )
    });
    match exported {
        Some(Ok(ExportCompletion::Completed { receipt })) => {
            scenario.check(
                "export-completed",
                receipt.revision == 2
                    && receipt.width == 5472
                    && receipt.height == 3648
                    && receipt.bytes_written > 0,
                format!(
                    "derivative {} ({}x{}, {} bytes)",
                    receipt.destination.display(),
                    receipt.width,
                    receipt.height,
                    receipt.bytes_written
                ),
            );
            scenario.artifact(&destination);
        }
        other => {
            scenario.check(
                "export-completed",
                false,
                format!("full-resolution export failed: {other:?}"),
            );
        }
    }

    // ---- backend rejects source/sidecar export destinations (spec A1)
    for (label, protected) in [
        ("source-path-destination", source.clone()),
        (
            "sidecar-path-destination",
            RecipeRepository::sidecar_path(&source),
        ),
        (
            "previous-sidecar-destination",
            RecipeRepository::previous_sidecar_path(&source),
        ),
    ] {
        let rejected = export_developed(
            &service,
            AssetExportInput {
                asset_id: "asset-a1".to_string(),
                variant_id: "default".to_string(),
                source_path: source.clone(),
                source_bytes: bytes.clone(),
                requested_revision: 2,
            },
            ExportSettings {
                destination: protected.clone(),
                format: ExportFormat::Png,
                jpeg_quality: 90,
                max_edge: None,
            },
            &cancel.1,
            &slot,
        );
        let detail = match &rejected {
            Err(ExportError::DestinationProtected { .. }) => {
                "typed DestinationProtected".to_string()
            }
            other => format!("unexpected outcome: {other:?}"),
        };
        scenario.check(
            "export-destination-protected",
            matches!(&rejected, Err(ExportError::DestinationProtected { .. })),
            format!("{label}: {detail}"),
        );
    }

    // ---- source immutability after edit, restart, reset and export
    let bytes_after = fs::read(&source).unwrap();
    let fixture_after = fs::read(&fixture_path).unwrap();
    scenario.check(
        "source-hash-after-chain",
        fingerprint(&bytes_after) == digest && fingerprint(&fixture_after) == digest,
        "source RAW bytes unchanged through edit, restart, reset and export",
    );
    let _ = service.close_session(reopened2.session_id);
}

pub fn a2(scenario: &mut Scenario) {
    let dir = run_dir("a2-ab-navigation");
    if !require_gpu(scenario) {
        return;
    }
    let (fixture_a, bytes_a, digest_a) = corpus_fixture("canon-eos-r6-craw-iso100-nocrop.CR3");
    let (fixture_b, bytes_b, digest_b) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    scenario.check(
        "fixture-integrity",
        !fixture_a.as_os_str().is_empty() && !fixture_b.as_os_str().is_empty(),
        format!(
            "A = {} (sha {}…), B = {} (sha {}…)",
            fixture_a.display(),
            &digest_a[..12],
            fixture_b.display(),
            &digest_b[..12]
        ),
    );
    let source_a = stage_source(&dir, "photo-a.CR3", &bytes_a);
    let source_b = stage_source(&dir, "photo-b.dng", &bytes_b);
    let service = production_service();
    let repo = RecipeRepository::lap_default();

    let mut open = |scenario: &mut Scenario,
                    asset: &str,
                    source: &PathBuf,
                    bytes: &[u8],
                    digest: &str|
     -> Option<u64> {
        let opened = service.open_session(AssetEditInput {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.to_string(),
            source_bytes: bytes.to_vec(),
            sidecar: None,
        });
        match opened {
            Ok(opened) => {
                scenario.check(
                    "open-session",
                    true,
                    format!("{asset}: session {} revision {}", opened.session_id, opened.revision),
                );
                Some(opened.session_id)
            }
            Err(err) => {
                scenario.check("open-session", false, format!("{asset}: {err}"));
                None
            }
        }
    };

    // Edit A: exposure +1.0, temperature +40. Edit B: exposure -0.5.
    let session_a = open(scenario, "asset-a2a", &source_a, &bytes_a, &digest_a);
    let mut envelope_a = session_a.and_then(|sid| service.session_envelope(sid));
    if let Some(envelope) = envelope_a.as_mut() {
        envelope.recipe.exposure = 1.0;
        envelope.recipe.temperature = 40.0;
    }
    let commit_a = session_a
        .zip(envelope_a)
        .map(|(sid, envelope)| service.commit_recipe(sid, 0, envelope));
    scenario.check(
        "commit-a",
        matches!(&commit_a, Some(Ok(receipt)) if receipt.revision == 1),
        format!("A commit: {commit_a:?}"),
    );

    let session_b = open(scenario, "asset-a2b", &source_b, &bytes_b, &digest_b);
    let mut envelope_b = session_b.and_then(|sid| service.session_envelope(sid));
    if let Some(envelope) = envelope_b.as_mut() {
        envelope.recipe.exposure = -0.5;
    }
    let commit_b = session_b
        .zip(envelope_b)
        .map(|(sid, envelope)| service.commit_recipe(sid, 0, envelope));
    scenario.check(
        "commit-b",
        matches!(&commit_b, Some(Ok(receipt)) if receipt.revision == 1),
        format!("B commit: {commit_b:?}"),
    );

    // Interleaved previews while both sessions are open.
    let (mut sid_a, mut sid_b) = (None, None);
    if let (Some(a), Some(b)) = (session_a, session_b) {
        let frame_a = completed_frame(scenario, &service, a, 1, 512);
        let frame_b = completed_frame(scenario, &service, b, 1, 512);
        if let (Some(fa), Some(fb)) = (frame_a, frame_b) {
            let artifact_a = save_png(&dir, "a2-a-preview.png", fa.0, fa.1, &fa.2);
            let artifact_b = save_png(&dir, "a2-b-preview.png", fb.0, fb.1, &fb.2);
            scenario.artifact(&artifact_a);
            scenario.artifact(&artifact_b);
            scenario.check(
                "ab-previews-distinct",
                fa.2 != fb.2,
                "A's and B's previews differ; no cross-asset leakage",
            );
        }
        sid_a = Some(a);
        sid_b = Some(b);
    }
    if let Some(sid) = sid_a {
        service.close_session(sid).unwrap();
    }
    if let Some(sid) = sid_b {
        service.close_session(sid).unwrap();
    }

    // Restart: reopen both from sidecars; each asset restores only its own recipe.
    let reopened_a = service.open_session(AssetEditInput {
        asset_id: "asset-a2a".to_string(),
        variant_id: "default".to_string(),
        source_path: source_a.clone(),
        source_fingerprint: digest_a.clone(),
        source_bytes: bytes_a.clone(),
        sidecar: Some(repo.load(&source_a).unwrap()),
    });
    let reopened_b = service.open_session(AssetEditInput {
        asset_id: "asset-a2b".to_string(),
        variant_id: "default".to_string(),
        source_path: source_b.clone(),
        source_fingerprint: digest_b.clone(),
        source_bytes: bytes_b.clone(),
        sidecar: Some(repo.load(&source_b).unwrap()),
    });
    match (reopened_a, reopened_b) {
        (Ok(a), Ok(b)) => {
            let durable_a = repo.load(&source_a).unwrap();
            let durable_b = repo.load(&source_b).unwrap();
            scenario.check(
                "ab-restart-restore",
                durable_a.recipe.exposure == 1.0
                    && durable_a.recipe.temperature == 40.0
                    && durable_b.recipe.exposure == -0.5
                    && durable_b.recipe.temperature == 0.0
                    && a.revision == 1
                    && b.revision == 1,
                format!(
                    "A (exposure {}, temperature {}) and B (exposure {}) restored independently",
                    durable_a.recipe.exposure, durable_a.recipe.temperature, durable_b.recipe.exposure
                ),
            );
            let frame_a = completed_frame(scenario, &service, a.session_id, 1, 512);
            let frame_b = completed_frame(scenario, &service, b.session_id, 1, 512);
            if let (Some(fa), Some(fb)) = (frame_a, frame_b) {
                scenario.check(
                    "ab-restart-previews-distinct",
                    fa.2 != fb.2,
                    "restarted A/B previews still reflect their own recipes",
                );
            }
            service.close_session(a.session_id).unwrap();
            service.close_session(b.session_id).unwrap();
        }
        (a, b) => {
            scenario.check(
                "ab-restart-restore",
                false,
                format!("reopen failed: a={a:?} b={b:?}"),
            );
        }
    }

    // Source hashes unchanged (spec A1 in the A/B chain).
    scenario.check(
        "source-hashes-unchanged",
        fingerprint(&fs::read(&source_a).unwrap()) == digest_a
            && fingerprint(&fs::read(&source_b).unwrap()) == digest_b,
        "both source RAWs unchanged after A/B navigation and restart",
    );
}


