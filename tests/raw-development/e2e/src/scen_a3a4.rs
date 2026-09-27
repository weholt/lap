//! A3/A4 end-to-end scenarios: hard process termination during durable saves
//! with recovery of the last acknowledged revision plus startup catalog
//! reconciliation (A3), and two-window revision conflicts with stale preview
//! rejection (A4).

use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lap_lib::develop::sessions::{
    AssetEditInput, DevelopConfig, DevelopError, DevelopService, PreviewWait,
};
use lap_lib::develop::RecipeRepository;
use rapidraw_edit_model::RecipeEnvelope;
use rapidraw_develop::session::{
    ExportJob, ExportRenderer, PreviewFrame, PreviewJob, PreviewRenderer, PreviewQuality,
    RecipeStore, SessionManagerConfig,
};
use rapidraw_develop::DevelopError as EngineError;

use crate::common::{
    fingerprint, production_service, require_gpu, run_dir, stage_source, synthetic_fixture,
    Scenario,
};

// ---------------------------------------------------------------------------
// A3: process termination during saves
// ---------------------------------------------------------------------------

const CRASH_LOOP_ENV: &str = "LAP_E2E_CRASH_LOOP_DIR";
const CRASH_LOOP_ROUNDS: u64 = 25;

/// Child mode: durably commits rounds of recipes, acknowledging each with an
/// `ACK <rev>` line on stdout. The parent hard-kills the process mid-loop.
pub fn crash_loop_child(work_dir: &Path) -> i32 {
    let source = work_dir.join("photo.dng");
    let repo = RecipeRepository::lap_default();
    let bytes = fs::read(&source).expect("child: staged source");
    let fp = fingerprint(&bytes);
    for revision in 1..=CRASH_LOOP_ROUNDS {
        let mut envelope = repo.new_envelope("asset-a3", "default", &fp);
        envelope.recipe.exposure = revision as f64 * 0.1;
        envelope.recipe.temperature = revision as f64;
        repo.commit_sidecar(&source, revision - 1, envelope)
            .unwrap_or_else(|err| panic!("child: commit {revision} failed: {err}"));
        println!("ACK {revision}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::thread::sleep(Duration::from_millis(25));
    }
    0
}

pub fn a3(scenario: &mut Scenario) {
    let dir = run_dir("a3-process-termination");
    let (fixture, bytes, digest) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    scenario.check(
        "fixture-integrity",
        true,
        format!("real decodable DNG {} (sha {}â€¦)", fixture.display(), &digest[..12]),
    );
    let source = stage_source(&dir, "photo.dng", &bytes);

    // Spawn the child commit loop and hard-kill it after ACK 3.
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&exe)
        .env(CRASH_LOOP_ENV, dir.display().to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn commit-loop child");
    let pid = child.id();
    let stdout = child.stdout.take().expect("child stdout");
    // The reader must stay alive until AFTER the hard kill: dropping it
    // closes the pipe and the child would die of a broken-stdout panic
    // before taskkill can terminate it.
    let mut reader = std::io::BufReader::new(stdout);
    let mut last_ack = 0u64;
    let mut acked_line = String::new();
    loop {
        acked_line.clear();
        match reader.read_line(&mut acked_line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(rest) = acked_line.trim().strip_prefix("ACK ") {
                    last_ack = rest.trim().parse().unwrap_or(last_ack);
                    if last_ack >= 3 {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    let kill = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .expect("run taskkill");
    let status = child.wait();
    drop(reader);
    scenario.check(
        "child-hard-killed",
        kill.success() && status.is_ok(),
        format!(
            "taskkill /F /PID {pid} (TerminateProcess semantics: no cleanup, no in-process handlers), last acknowledged revision {last_ack}, wait: {status:?}"
        ),
    );

    // The last acknowledged sidecar stays readable and valid.
    let repo = RecipeRepository::lap_default();
    let durable = repo.load(&source);
    match durable {
        Ok(envelope) => {
            let acknowledged_ok = envelope.revision >= last_ack && last_ack >= 3;
            let content_ok = envelope.content_hash().is_ok();
            let recipe_ok = (envelope.recipe.exposure - envelope.revision as f64 * 0.1).abs() < 1e-9;
            scenario.check(
                "last-acknowledged-sidecar-readable",
                acknowledged_ok && content_ok && recipe_ok,
                format!(
                    "durable revision {} (last ack {last_ack}), canonical hash ok: {content_ok}, recipe matches revision: {recipe_ok}",
                    envelope.revision
                ),
            );
            if envelope.revision >= 2 {
                let previous = repo.load_previous_opt(&source);
                scenario.check(
                    "recoverable-previous-revision",
                    matches!(&previous, Ok(Some(prev)) if prev.revision == envelope.revision - 1),
                    format!(
                        "previous sidecar revision: {:?}",
                        previous.as_ref().map(|prev| prev.as_ref().map(|envelope| envelope.revision))
                    ),
                );
            }
            let durable_revision = envelope.revision;
            let durable_exposure = envelope.recipe.exposure;

            // Startup catalog reconciliation: the child only wrote sidecars
            // (commit_sidecar, no catalog), so the projection lags. Reconcile
            // must project exactly the durable revision.
            let conn = catalog_db(&dir, "photo.dng");
            let outcome = repo.reconcile(&conn, &source);
            let projected_ok = matches!(&outcome, Ok(lap_lib::develop::recipe_repository::ReconcileOutcome::Projected { revision }) if *revision == durable_revision);
            scenario.check(
                "startup-reconciles-catalog-lag",
                projected_ok,
                format!("reconcile outcome: {outcome:?} (durable revision {durable_revision})"),
            );
            let row = RecipeRepository::projection_row(
                &conn,
                &RecipeRepository::sidecar_path(&source),
                "default",
            );
            scenario.check(
                "projection-row-matches-sidecar",
                matches!(&row, Ok(Some(row)) if row.revision == durable_revision),
                format!("projection row: {row:?}"),
            );

            // The recovered asset remains editable: reopen + GPU preview of
            // the recovered recipe.
            if require_gpu(scenario) {
                let service = production_service();
                let reopened = service.open_session(AssetEditInput {
                    asset_id: "asset-a3".to_string(),
                    variant_id: "default".to_string(),
                    source_path: source.clone(),
                    source_fingerprint: digest.clone(),
                    source_bytes: fs::read(&source).unwrap(),
                    sidecar: Some(repo.load(&source).unwrap()),
                });
                match reopened {
                    Ok(opened) => {
                        scenario.check(
                            "recovered-asset-reopenable",
                            opened.revision == durable_revision,
                            format!(
                                "reopened at revision {} (exposure {durable_exposure}); session {}",
                                opened.revision, opened.session_id
                            ),
                        );
                        let envelope = service.session_envelope(opened.session_id).unwrap();
                        let wait = service.render_preview(
                            opened.session_id,
                            1,
                            envelope,
                            PreviewQuality::Settled,
                            256,
                        );
                        scenario.check(
                            "recovered-asset-previews",
                            matches!(&wait, Ok(PreviewWait::Completed { .. })),
                            format!("preview of recovered state: {wait:?}"),
                        );
                        let _ = service.close_session(opened.session_id);
                    }
                    Err(err) => {
                        scenario.check(
                            "recovered-asset-reopenable",
                            false,
                            format!("reopen failed: {err}"),
                        );
                    }
                }
            }

            // Kill/ack consistency across the surviving sidecar history: the
            // sidecar must never be torn (load succeeded above) and the temp
            // siblings must be absent after a hard kill.
            let temporaries: Vec<PathBuf> = fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .map(|name| {
                            let name = name.to_string_lossy();
                            name.ends_with(".tmp") || name.contains(".tmp.")
                        })
                        .unwrap_or(false)
                })
                .collect();
            scenario.check(
                "no-torn-temp-artifacts",
                temporaries.is_empty(),
                format!(
                    "temp siblings after hard kill: {} ({temporaries:?})",
                    temporaries.len()
                ),
            );
        }
        Err(err) => {
            scenario.check(
                "last-acknowledged-sidecar-readable",
                false,
                format!("durable sidecar unreadable after hard kill: {err}"),
            );
        }
    }
}

/// Minimal catalog mirror of the application schema (albums/afolders/afiles
/// plus the develop projection), used by the a3 startup reconciliation check.
fn catalog_db(dir: &Path, source_name: &str) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE albums (id INTEGER PRIMARY KEY, path TEXT NOT NULL);
         CREATE TABLE afolders (
             id INTEGER PRIMARY KEY,
             album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
             path TEXT NOT NULL
         );
         CREATE TABLE afiles (
             id INTEGER PRIMARY KEY,
             folder_id INTEGER NOT NULL REFERENCES afolders(id) ON DELETE CASCADE,
             name TEXT NOT NULL
         );",
    )
    .unwrap();
    let dir_str = dir.to_string_lossy().to_string();
    conn.execute("INSERT INTO albums (id, path) VALUES (1, ?1)", [&dir_str])
        .unwrap();
    conn.execute(
        "INSERT INTO afolders (id, album_id, path) VALUES (10, 1, ?1)",
        [&dir_str],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO afiles (id, folder_id, name) VALUES (100, 10, ?1)",
        [source_name],
    )
    .unwrap();
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    conn
}

// ---------------------------------------------------------------------------
// Minimal gate-dance probe (bisection for the a4 hang)
// ---------------------------------------------------------------------------

pub fn probe_gate(scenario: &mut Scenario) {
    let dir = run_dir("probe-gate");
    let (fixture, bytes, digest) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    let source = stage_source(&dir, "photo.dng", &bytes);
    let (gate, senders) = {
        let (tx, rx) = mpsc::channel();
        (
            GateRenderer {
                gates: Mutex::new(std::collections::HashMap::from([(1u64, rx)])),
            },
            std::collections::HashMap::from([(1u64, tx)]),
        )
    };
    let config = DevelopConfig {
        sessions: SessionManagerConfig {
            max_sessions: 4,
            preview_workers: 1,
            export_workers: 1,
            max_queued_previews: 8,
            max_queued_exports: 2,
            max_cached_preview_results: 2,
        },
        max_preview_edge: 4096,
        max_cached_preview_bytes: 1024 * 1024,
    };
    eprintln!("probe: with_parts");
    let gated = Arc::new(DevelopService::with_parts(
        config,
        Arc::new(gate),
        Arc::new(HarnessStubExport),
        Arc::new(NullStore),
    ));
    eprintln!("probe: open_session");
    let session = gated
        .open_session(AssetEditInput {
            asset_id: "asset-probe".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: None,
        })
        .unwrap();
    eprintln!("probe: spawn gen1");
    let blocked = {
        let gated = Arc::clone(&gated);
        let session_id = session.session_id;
        let envelope = gated.session_envelope(session_id).unwrap();
        std::thread::spawn(move || {
            let outcome = gated.render_preview(session_id, 1, envelope, PreviewQuality::Settled, 48);
            eprintln!("probe: gen1 resolved {outcome:?}");
            outcome
        })
    };
    std::thread::sleep(Duration::from_millis(150));
    eprintln!("probe: gen2 submit (gate still held; engine must coalesce gen1)");
    let newer = gated.render_preview(
        session.session_id,
        2,
        gated.session_envelope(session.session_id).unwrap(),
        PreviewQuality::Settled,
        48,
    );
    eprintln!("probe: gen2 resolved");
    let _ = senders.get(&1).unwrap().send(());
    let blocked = blocked.join().unwrap();
    scenario.check(
        "probe-gate-dance",
        matches!(newer, Ok(PreviewWait::Completed { .. }))
            && matches!(blocked, Ok(PreviewWait::Cancelled)),
        format!("gen2: {newer:?}; gen1: {blocked:?}"),
    );
    let _ = gated.close_session(session.session_id);
}

// ---------------------------------------------------------------------------
// A4: two-window conflict + stale preview rejection
// ---------------------------------------------------------------------------

/// Preview renderer with per-generation gates (mirrors the service unit
/// double): a gated generation blocks until released, simulating a delayed
/// render reply. Used only for the delayed-reply discipline check.
struct GateRenderer {
    gates: Mutex<std::collections::HashMap<u64, mpsc::Receiver<()>>>,
}

impl PreviewRenderer for GateRenderer {
    fn render(&self, job: &PreviewJob) -> Result<PreviewFrame, EngineError> {
        let rx = self.gates.lock().unwrap().remove(&job.generation);
        if let Some(rx) = rx {
            loop {
                // Cooperative cancellation must interrupt a gated render:
                // without this check the loop only ever exits on gate
                // release, and a coalescing newer generation would wait
                // forever behind the single preview worker.
                job.cancel.check()?;
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
        job.cancel.check()?;
        Ok(PreviewFrame {
            width: 2,
            height: 2,
            rgba8: vec![job.generation as u8, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
        })
    }
}

struct NullStore;

impl RecipeStore for NullStore {
    fn persist(&self, _envelope: &RecipeEnvelope) -> Result<(), String> {
        Ok(())
    }
}

/// Explicit refusal double: the gated A4 service never needs durable export.
struct HarnessStubExport;

impl ExportRenderer for HarnessStubExport {
    fn render(&self, _job: &ExportJob) -> Result<rapidraw_develop::ExportFrame, EngineError> {
        Err(EngineError::Unsupported(
            "export is not under test in the A4 generation-discipline scenario".to_string(),
        ))
    }
}

pub fn a4(scenario: &mut Scenario) {
    let dir = run_dir("a4-conflict-stale");
    if !require_gpu(scenario) {
        return;
    }
    let (fixture, bytes, digest) = synthetic_fixture("dng-linear-gradient-64x48.dng");
    let source = stage_source(&dir, "photo.dng", &bytes);
    scenario.check(
        "fixture-integrity",
        true,
        format!("real decodable DNG {} (sha {}â€¦)", fixture.display(), &digest[..12]),
    );

    let repo = RecipeRepository::lap_default();
    let mut seeded = repo.new_envelope("asset-a4", "default", &digest);
    seeded.recipe.exposure = 0.3;
    repo.commit_sidecar(&source, 0, seeded).unwrap();

    let open_input = || AssetEditInput {
        asset_id: "asset-a4".to_string(),
        variant_id: "default".to_string(),
        source_path: source.clone(),
        source_fingerprint: digest.clone(),
        source_bytes: bytes.clone(),
        sidecar: Some(repo.load(&source).unwrap()),
    };

    // Two windows: two independent service instances over the same durable
    // sidecar, exactly like two application windows/processes.
    let window_a = production_service();
    let window_b = production_service();
    let opened_a = window_a.open_session(open_input());
    let opened_b = window_b.open_session(open_input());
    if !scenario.check(
        "two-windows-open",
        matches!((&opened_a, &opened_b), (Ok(a), Ok(b)) if a.revision == 1 && b.revision == 1),
        format!(
            "window A: {:?}, window B: {:?}",
            opened_a.as_ref().map(|o| (o.session_id, o.revision)),
            opened_b.as_ref().map(|o| (o.session_id, o.revision))
        ),
    ) {
        return;
    }
    let session_a = opened_a.unwrap();
    let session_b = opened_b.unwrap();

    // Both commit from revision 1: exactly one succeeds.
    let mut envelope_a = window_a.session_envelope(session_a.session_id).unwrap();
    envelope_a.recipe.exposure = 1.0;
    let commit_a = window_a.commit_recipe(session_a.session_id, session_a.revision, envelope_a);
    let mut envelope_b = window_b.session_envelope(session_b.session_id).unwrap();
    envelope_b.recipe.exposure = 2.0;
    let commit_b = window_b.commit_recipe(session_b.session_id, session_b.revision, envelope_b);
    let conflict_typed = matches!(
        &commit_b,
        Err(DevelopError::RevisionConflict { expected: 1, current: 2, .. })
    );
    scenario.check(
        "stale-window-commit-conflicts",
        matches!(&commit_a, Ok(receipt) if receipt.revision == 2) && conflict_typed,
        format!(
            "window A: {:?}; window B: {:?} (typed conflict: {conflict_typed})",
            commit_a.as_ref().map(|r| r.revision),
            commit_b.as_ref().err().map(|e| e.to_string())
        ),
    );
    let durable = repo.load(&source).unwrap();
    scenario.check(
        "durable-sidecar-advanced-once",
        durable.revision == 2 && durable.recipe.exposure == 1.0,
        format!(
            "durable revision {} exposure {}",
            durable.revision, durable.recipe.exposure
        ),
    );

    // The conflicting window's in-memory session must not have adopted the
    // winning recipe silently: closing it must not overwrite the sidecar.
    let closed_b = window_b.close_session(session_b.session_id);
    let durable_after_close = repo.load(&source).unwrap();
    scenario.check(
        "loser-close-preserves-winner",
        closed_b.is_ok() && durable_after_close.revision == 2 && durable_after_close.recipe.exposure == 1.0,
        format!(
            "close of stale window: {closed_b:?}; durable still revision {} exposure {}",
            durable_after_close.revision, durable_after_close.recipe.exposure
        ),
    );

    // Stale generation rejection on the winning window (real GPU renderer).
    let completed = window_a.render_preview(
        session_a.session_id,
        5,
        window_a.session_envelope(session_a.session_id).unwrap(),
        PreviewQuality::Settled,
        256,
    );
    scenario.check(
        "generation-5-renders",
        matches!(&completed, Ok(PreviewWait::Completed { .. })),
        format!("generation 5: {completed:?}"),
    );
    let stale = window_a.render_preview(
        session_a.session_id,
        5,
        window_a.session_envelope(session_a.session_id).unwrap(),
        PreviewQuality::Settled,
        256,
    );
    scenario.check(
        "stale-generation-rejected",
        matches!(&stale, Err(DevelopError::StaleGeneration { requested: 5, .. })),
        format!("re-submitted generation 5: {:?} (typed: {:?})", stale.as_ref().err().map(|e| e.to_string()), stale.as_ref().err()),
    );

    // Delayed reply can never replace a newer generation (gate renderer;
    // generation discipline is renderer-agnostic, the real GPU path above).
    let (gate, senders) = {
        let (tx, rx) = mpsc::channel();
        (
            GateRenderer {
                gates: Mutex::new(std::collections::HashMap::from([(1u64, rx)])),
            },
            std::collections::HashMap::from([(1u64, tx)]),
        )
    };
    let config = DevelopConfig {
        sessions: SessionManagerConfig {
            max_sessions: 4,
            preview_workers: 1,
            export_workers: 1,
            max_queued_previews: 8,
            max_queued_exports: 2,
            max_cached_preview_results: 2,
        },
        max_preview_edge: 4096,
        max_cached_preview_bytes: 1024 * 1024,
    };
    let gated = Arc::new(DevelopService::with_parts(
        config,
        Arc::new(gate),
        Arc::new(HarnessStubExport),
        Arc::new(NullStore),
    ));
    let gated_session = gated
        .open_session(AssetEditInput {
            asset_id: "asset-a4g".to_string(),
            variant_id: "default".to_string(),
            source_path: source.clone(),
            source_fingerprint: digest.clone(),
            source_bytes: bytes.clone(),
            sidecar: None,
        })
        .unwrap();
    let blocked = {
        let gated = Arc::clone(&gated);
        let session_id = gated_session.session_id;
        let envelope = gated.session_envelope(session_id).unwrap();
        std::thread::spawn(move || {
            let outcome = gated.render_preview(session_id, 1, envelope, PreviewQuality::Settled, 64);
            outcome
        })
    };
    std::thread::sleep(Duration::from_millis(150));
    let newer = gated.render_preview(
        gated_session.session_id,
        2,
        gated.session_envelope(gated_session.session_id).unwrap(),
        PreviewQuality::Settled,
        64,
    );
    let newer_ok = matches!(&newer, Ok(PreviewWait::Completed { ticket }) if ticket.generation == 2);
    let _ = senders.get(&1).unwrap().send(());
    let blocked_outcome = blocked.join().unwrap();
    scenario.check(
        "delayed-reply-never-overwrites",
        newer_ok && matches!(blocked_outcome, Ok(PreviewWait::Cancelled)),
        format!(
            "newer generation 2 completed: {newer_ok}; delayed generation 1 resolved to {:?}",
            blocked_outcome.as_ref()
        ),
    );
    let _ = gated.close_session(gated_session.session_id);
    let _ = window_a.close_session(session_a.session_id);
}

