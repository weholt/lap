# Small-slice qualification: A1-A7 proven end-to-end

Issued by lap-70c (TASK-306, managed continuation of lap-f19.6) on 2026-09-27.
Governing contract: `docs/raw-development/spec.md` (delivery step 3: "Prove
A1-A7 on this small slice before adding dozens of controls"). Producer
receipts consumed: TASK-102, TASK-303, TASK-304, TASK-305 (SHA-256 verified
against the issue statement before work started; all four MATCH).

## Verdict: PASS

The exposure/white-balance slice satisfies acceptance A1-A7 against real RAW
fixtures on the real Windows GPU backend. One production defect was found by
this gate and fixed with RED→GREEN evidence (below). The distribution release
gate remains BLOCKED by the unresolved P3 licensing decision — that is the
required, honest state and is not a qualification failure.

## What ran (one repeatable command)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\raw-development\qualify-slice.ps1 -RunDir <run-dir>
```

Final full run: `C:\Users\Thomas\Desktop\.wdl-control\lap-raw-development-20260926\runs\f5fe67732b434a61812ed0157fc6cac4\gate-final`
(machine-local; summarized in `gate-summary.json`).

| Stage | Result | Evidence |
| --- | --- | --- |
| 1 pins | PASS | Engine pin `e1035c38aa1150ac350faa3661f26144a922ea91` identical in `engine-lock.json`, `src-tauri/src/develop/sessions.rs` and `src-tauri/Cargo.lock`; engine checkout clean with the pin as ancestor of HEAD; parity + corpus manifests present. |
| 2 rust-layer | PASS | `cargo test --manifest-path src-tauri/Cargo.toml`: 49 + 20 + 25 tests, 0 failed — develop sessions/store/export/cache units plus the develop_persistence integration suite (atomic writes, fault + crash-termination injection at every save boundary, revision CAS, projection reconcile). |
| 3 ui-layer | PASS | `npm --prefix src-vite run test` (via `harness-validation.ps1`): 11 files, 74 tests, 0 failed — DevelopPanel, DevelopExportDialog, useDevelopSession, ImageHistogram renderer contract, uiStore develop state, raw-corpus integrity. |
| 4 e2e-suite | PASS | `tests/raw-development/e2e` scenario binary: all 8 scenarios green (detail below). |
| 5 pinned-parity | PASS | `capture-baselines.ps1 -AllowSourceDrift`: **34/34 headless export cases byte-identical** to the frozen pre-extraction baseline; adapter NVIDIA GeForce RTX 4060, backend Dx12. |
| 6 app-launch | PASS | The actual built `Lap.exe` launched; visible main window verified by Win32 enumeration; 15 s stability hold; graceful WM_CLOSE with exit code 0. |
| 7 release-gate | PASS (BLOCKED as required) | `check-release-gates.ps1` exit 1 — distribution hold in force (spec P3). |

## Revisions and environment

- Lap host: branch `pebbles-harness/raw-development`, parent revision
  `9abe5e7249e08e9e699e544ddd00492f57a280a2` + the commit carrying this file.
- Engine: pinned git dependency at `e1035c38aa1150ac350faa3661f26144a922ea91`
  (isolated checkout `C:/Users/Thomas/Desktop/RapidRAW-engine`, branch
  `feature/lap-engine-extraction`, clean, pin is ancestor of HEAD).
- Fixtures: committed CC0 corpus (`corpus-manifest.json`), per-run SHA-256
  verified before use: Canon EOS R6 CRAW ISO 100 (5472×3648 Bayer, primary),
  orientation-6 linear DNG (5464×8192 transposed), plus synthetic
  gradient/wide/broken DNGs for bounded-cost scenarios and failure cases.
- GPU: NVIDIA GeForce RTX 4060; wgpu backend Vulkan for the offscreen e2e
  pipeline, Dx12 for the pinned-baseline headless capture; max texture
  dimension 32768.
- OS: Windows; toolchain rustup 1.98.0 x86_64-pc-windows-msvc (PATH prepend +
  `build-e2e.cmd` environment handling documented in that script).

## Stage 4 detail: e2e scenarios (`tests/raw-development/e2e`)

All scenarios drive the real `lap_lib::develop` service, the real durable
sidecar store and the pinned engine; the GPU is probed per scenario and a
missing device is a hard failure (never a skip). Fixtures are copied into a
machine-local run directory first — committed fixtures stay byte-immutable.

- **production-wiring** — constructs the develop service EXACTLY like
  `t_cmds::DevelopAppState::service` and runs a durable export through it.
- **a1-source-immutability** (real Bayer CR3): hash → open/decode → GPU
  default preview → commit exposure +0.8 / temperature +35 → close/reopen
  (recipe restored) → GPU adjusted preview (differs from default) → reset to
  default recipe (revision 2) → reopen → full-resolution export 5472×3648 →
  destination rejections (source, sidecar, previous-sidecar: typed
  `DestinationProtected`) → source hash identical after the whole chain.
- **a2-ab-navigation**: A (real CR3, exposure +1.0 / temperature +40) and B
  (gradient, exposure −0.5) commit independently; interleaved GPU previews
  stay asset-scoped; after close/reopen each sidecar restores only its own
  recipe and the previews remain distinct.
- **a3-process-termination**: a child process loops durable commits
  (ACK-per-revision on stdout); the parent hard-kills it with
  `taskkill /F` (TerminateProcess) after ACK 3. The durable sidecar stays
  valid (canonical hash verified), the previous revision stays recoverable,
  startup reconciliation projects exactly the durable revision, the recovered
  asset reopens and GPU-previews, no torn temp artifacts remain. Boundary
  injections at every save point are additionally covered by stage 2's crash
  worker suite.
- **a4-conflict-stale**: two production services over one sidecar commit from
  revision 1 — one wins, the other gets typed `RevisionConflict` and closing
  it preserves the winner; re-submitting generation 5 is typed
  `StaleGeneration`; a gated delayed reply resolves `Cancelled` and never
  replaces the newer generation (engine coalescing proven with the gate held).
- **a5-fullres-export** (real Bayer CR3, >4096 px): committed revision
  exports at exactly the original decoded 5472×3648; explicit `max_edge`
  resize is downscale-only; a fresh service re-export of the same committed
  revision is **byte-identical**; the orientation fixture decodes transposed
  (5464×8192) — full-dimension decode, never an embedded preview.
- **a6-parity**: settled GPU preview (1024 px) vs downsampled export
  (1024 px) under one color transform (engine sRGB-encoded 8-bit via
  `OutputTarget::CpuPixels`), exposure +1.0 / temperature +40 on the real
  CR3. Both capture repeats are **byte-identical between the paths**
  (mean/p99/max abs diff 0, histogram intersection 1.0) — expected exactly
  for the slice's linear controls, which commute with the linear-light
  resampling; frozen tolerances (`tests/raw-development/preview-export-parity.json`,
  schema `lap-raw-preview-parity/v1`) therefore pin near-exact equality
  (mean ≤ 0.25, p99 ≤ 1, max ≤ 2, hist mean ≤ 0.0002, intersection ≥ 0.995)
  and any drift is a regression. The adjusted-state histogram differs from
  the default-state histogram (0.00355 ≥ floor 0.00177), so the histogram
  provably reflects the rendered adjusted state.
- **a7-gpu-failures**: missing adapter (empty backend set) → typed probe
  failure and typed render/export errors with **no artifact written**; a
  recipe referencing a missing LUT resource fails explicitly for both preview
  and durable export (never an un-LUT-ed success); broken/undecodable sources
  produce typed `Decode` errors; the device texture limit is recorded with
  every render within it (engine-side `TextureTooLarge` is the typed guard).
  Driver-level device loss and OOM injection are not software-reproducible on
  this host and are not claimed; their typed error paths exist in the engine
  (`DeviceLost`, OOM variants) and in the host mapping.

## Defect found and fixed (RED→GREEN)

**Defect:** the production constructor
`DevelopService::with_gpu_and_sidecar_store` still installed the pre-TASK-304
`StubExportRenderer` in the engine session manager. Every real
`develop_export_developed` call therefore failed with
`Unsupported("durable export is implemented in a later slice (TASK-304)")` —
the unit tests passed only because they injected the real renderer
themselves. This is exactly the class of integration break the gate exists to
catch.

- **RED:** `lap-raw-e2e.exe production-wiring` →
  `[FAIL] export-through-production-service — production-wiring export
  failed: Err(Unsupported("durable export is implemented in a later slice
  (TASK-304)")))` (evidence snapshot:
  `...\f5fe67732b434a61812ed0157fc6cac4\e2e-red\evidence-production-wiring.json`).
- **Fix:** `src-tauri/src/develop/sessions.rs` — the production constructor
  now installs `super::export::GpuExportRenderer::new()`; the stale stub type
  was removed. No other production file changed.
- **GREEN:** same scenario PASSes (derivative written through the bounded
  engine export queue); the full gate passes, including stage 2's 94-test
  suite over the changed crate.

## Honest limitations

- App-launch evidence proves process/window/lifecycle only. Unattended
  slider interaction in the running GUI is not claimed; spec A10 (interactive
  verification in the main right panel) remains its own task.
- Device-loss/OOM are covered by typed paths, not executed injections.
- Parity/pinned baselines are frozen for this machine and adapter only; no
  cross-platform qualification is claimed (spec P4/P5 untouched).
- The e2e harness cannot run inside the packaged app database; catalog
  projection is covered by the develop_persistence suite and the a3
  reconciliation scenario (in-memory catalog mirror), not by the launched
  app's real library — deliberately, to keep the user's library untouched.
- `cargo fmt --check` fails on pre-existing Lap files (t_sqlite/t_cmds/
  t_utils/…): inherited baseline, not a regression; the one file this task
  changed (`develop/sessions.rs`) is fmt-clean. Untracked local run
  artifacts live under `tests/raw-development/runs/` (gitignored).

## Reproduction

1. `powershell -ExecutionPolicy Bypass -File scripts\raw-development\qualify-slice.ps1`
2. Freeze/compare parity tolerances explicitly:
   `$env:LAP_E2E_PARITY_FREEZE = "1"` once (already frozen in-repo), then
   run `lap-raw-e2e.exe a6-parity` without it for every later compare.
3. Individual scenarios: `lap-raw-e2e.exe <scenario>` (see
   `tests/raw-development/e2e/src/main.rs` for the list).
