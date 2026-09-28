# Raw-development platform qualification and performance record

Established 2026-09-28 by lap-c0e (TASK-601, managed continuation of the
legacy coordinating issue lap-404.1). Governing contract: `spec.md`
(acceptance A7/A11/A12, prerequisite P5). This file records what was
measured, on which machine, with which fixture/engine revisions — and,
equally deliberately, what was NOT measured.

## Status: qualified on the Windows host only; release BLOCKED

- Windows reference machine (`windows-thomas-rtx4060`): executed, measured,
  evidence recorded below.
- Linux and macOS reference machines: **do not exist in this development
  environment and are explicitly UNTESTED**. They are declared
  `status: "unavailable"` in `tests/raw-development/platform/platform-manifest.json`
  and keep the fail-closed platform release gate BLOCKED. This is a release
  blocker, not a footnote.
- The provisional warm-preview target is **MISSED** on the measured host
  (details below); the 150 ms threshold stands unchanged and still
  provisional.

## The tooling (repeatable commands)

```powershell
# build + unit tests + full benchmark/stress run + schema validation + gate
powershell -ExecutionPolicy Bypass -File scripts\raw-development\benchmark.ps1 [-RunDir <dir>]

# results document validation only
node scripts/raw-development/benchmark.mjs validate <results.json>

# fail-closed platform release gate (exit 0 released / 1 blocked / 2 failed)
node scripts/raw-development/benchmark.mjs gate
```

Components:

| Path | Role |
| --- | --- |
| `tests/raw-development/platform/` (crate `lap-raw-platform`) | Drives the real `lap_lib::develop` service, the pinned engine, and the real GPU. Workloads: cold decode (fresh child process per fixture), warm slider at 1536 px, settled preview, durable full-resolution export, thumbnail throughput under indexing load, repeated-navigation memory bound, spec-A7 GPU failure modes. RAM via `GlobalMemoryStatusEx`/`GetProcessMemoryInfo`; VRAM via DXGI `IDXGIAdapter3::QueryVideoMemoryInfo` (raw COM, local segment). |
| `tests/raw-development/platform/platform-manifest.json` | Machine/backend manifest: the named Windows host plus the explicitly unavailable Linux/macOS reference machines and the provisional target definition. |
| `scripts/raw-development/benchmark.mjs` | Results-schema validator (recomputes p95 from raw samples, rejects target redefinition) and the fail-closed release gate (every declared machine must be available, executed, hardware-identity-matched, and passing; CI definitions are not execution and are never consulted). |
| `tests/raw-development/platform/schema-gate.test.mjs` | `node --test` suite pinning the validator/gate behavior, including failed/missing-run rejection. |
| `.github/workflows/raw-development.yml` | Workflow **definition** only (`workflow_dispatch`); explicitly not platform evidence. |

Results documents are schema `lap-raw-platform-results/v1`: named
hardware/backend, fixture revisions (sha256), raw sample arrays plus computed
p50/p95, memory series (working set, private commit, VRAM usage/budget), and
per-workload configuration.

## Measured record (2026-09-28, committed revisions)

- Lap host: branch `pebbles-harness/raw-development`, revision `8aaf7ffc80…`
  (full 40-hex in the results document).
- Engine: pinned git dependency
  `de4fdbd76723c76145b477a2b8874226512475f1` (`docs/raw-development/engine-lock.json`).
- Fixtures: committed CC0 corpus, per-run sha256-verified against
  `tests/fixtures/raw-development/corpus-manifest.json`.
- Machine: AMD Ryzen 7 5700X, 47.9 GiB RAM; NVIDIA GeForce RTX 4060
  (7.77 GiB dedicated), wgpu backend **Vulkan**, max texture dimension 32768.
- Command: `powershell -ExecutionPolicy Bypass -File
  scripts\raw-development\benchmark.ps1 -RunDir
  tests\raw-development\runs\lap-c0e\platform-run-final` — all five stages
  PASS (unit tests, build, 317 s run, validation, gate BLOCKED as required).
- Raw measurements: `tests/raw-development/platform/runs/platform-results-windows-thomas-rtx4060.json`
  (machine-local, gitignored; identical copy inside the run directory).

### Measurements (raw samples and computed percentiles in the results JSON)

| Workload | Result | Notes |
| --- | --- | --- |
| Cold decode (fresh process per fixture) | p95 **2305.6 ms**; Bayer CR3 ≈ 721–724 ms; X-Trans RAF ≈ 1884 ms; JPEG-XL linear DNG ≈ 2137 ms; orientation-6 DNG ≈ 2306 ms | full-dimension linear decode, engine defaults |
| Warm slider 1536 px (24 edits) | p50 3876 ms, **p95 3925.6 ms — provisional 150 ms target MISSED** | see attribution below |
| Settled preview 1536 px | p95 ≈ 3912 ms | same host-path cost structure |
| Durable full-resolution export | p95 4644 ms; 5472×3648 PNG, 30.3 MB | committed recipe at revision 1 |
| Thumbnail throughput under indexing load | **0.136 thumbnails/s** (10 thumbnails, 72.9 s) while a background thread executed 47 full-quality indexing decodes | memory series recorded per thumbnail |
| Repeated navigation memory (24 open→preview→close over two real CR3s) | bounded: steady-state private-commit growth **1.5 MB** (bound 256 MiB, provisional) | full per-iteration series recorded |
| GPU failure modes (spec A7) | missing adapter → typed unsupported through the production service; oversized texture (32769×8 vs limit 32768) → typed `TextureTooLarge`; device loss / OOM: **NOT INJECTED** (see honesty notes) | device caps recorded |
| RAM/VRAM counters | sampled throughout; final: 0.67 GiB working set / 1.7 GiB private commit; VRAM usage ≈ 165 MB against a 31.2 GiB local-segment budget during navigation | per-sample series in the results JSON |

### Warm-preview target miss: measured, attributed, NOT redefined

The provisional budget (spec A12: warm 1536-px preview p95 < 150 ms, agreed
final budgets only after the first measured slice) is missed by ~26×. The
harness records `target: { thresholdMs: 150, provisional: true, met: false }`,
and the schema validator rejects any document that moves the threshold.

The integrated stage breakdown attributes the miss precisely:

| Stage of one warm preview render | Measured |
| --- | --- |
| Engine GPU warm re-render (cached input, changed recipe) | **≈ 18–20 ms** |
| Host CPU downscale of the full 5472×3648 linear original (`preview_base`, `image` crate Triangle, per generation) | **≈ 3535 ms** |
| Decode (context) | ≈ 640 ms |
| Buffer copy + RGBA32 conversion + device init (context) | ≈ 120 ms + 500 ms (once) |

The engine's GPU pipeline is comfortably inside the budget; the host preview
path pays a per-generation full-resolution CPU resize before rendering. This
is recorded as the dominant optimization item for the preview pipeline
(host-side caching/downscaling strategy), NOT by redefining the 150 ms
number. The same cost structure explains the thumbnail throughput figure
(every thumbnail pays a full-resolution CPU downscale).

## Gate evidence (fail-closed behavior proven)

`node scripts/raw-development/benchmark.mjs` was exercised against the real
results document and mutated copies (log:
`tests/raw-development/runs/lap-c0e/gate-negative-demo/demo-log.txt`):

| Case | Exit | Expected |
| --- | --- | --- |
| Real results document | 0 (valid) | 0 |
| Provisional target redefined 150→4000 ms | 1, "redefined the provisional target threshold" | reject |
| Export workload marked failed | 1, "workload export failed on this platform" | reject |
| Gate with no executed results for the available machine | 1, "no executed results … a CI workflow definition alone is not platform evidence" | block |
| Gate with real Windows results | 1, blocked: warm-target miss + Linux/macOS UNTESTED | block |
| Malformed results file | 2 (failed) | 2 |

Gate verdict on the real evidence: `windows-thomas-rtx4060: blocked` (target
missed), `linux-reference: untested`, `macos-reference: untested`.

## RED → GREEN

- RED (JS): `node --test tests/raw-development/platform/schema-gate.test.mjs`
  failed with `ERR_MODULE_NOT_FOUND … benchmark.mjs`
  (`tests/raw-development/runs/lap-c0e/red-schema-gate.txt`).
- GREEN (JS): 15/15 tests pass after implementing the validator/gate.
- RED (Rust): `cargo test --manifest-path
  tests/raw-development/platform/Cargo.toml quantile` — 4/4 tests failed with
  `not yet implemented: implement nearest-rank percentile (lap-c0e RED)`
  (`tests/raw-development/runs/lap-c0e/red-quantile.txt`).
- GREEN (Rust): 4/4 pass; the definition matches
  `benchmark.mjs percentile()` so validation can recompute every p95.
- Two counter defects were found and fixed during bring-up (wrong
  `DXGI_ADAPTER_DESC1` layout producing garbage dedicated-memory values;
  wrong `IID_IDXGIAdapter3` producing `E_NOINTERFACE`), each proven by the
  `diag-counters` mode before/after.

## Honest limitations (unresolved release evidence)

- **Linux/macOS: explicitly UNTESTED.** No reference machine exists here; the
  manifest records them as unavailable and the gate stays BLOCKED for them.
  The committed CI workflow is a definition only and was never executed; CI
  definitions alone do not count as platform evidence (spec P5).
- **Device loss and OOM were NOT injected.** Driver-level faults are not
  software-reproducible on this host. The typed engine variants
  (`GpuError::DeviceLost`, `GpuError::OutOfMemory`) exist and are mapped in
  the host, and the checks record this verbatim; real driver-fault
  qualification remains open release work.
- The warm/settled/export/thumbnail budgets other than the provisional
  150 ms preview target are **not yet agreed**; per spec, final budgets are
  to be agreed after this first measured slice, not invented here.
- Measurements are for the Vulkan backend in the offscreen pipeline on this
  machine; the pinned headless baseline path uses Dx12 (see
  `fixtures.md`). No cross-adapter or cross-backend qualification is claimed.
- Thumbnail throughput uses the real session/preview pipeline under a
  background decode load that models indexing; it does not drive the
  packaged app's library indexer.
- The provisional navigation bound (256 MiB steady-state private-commit
  growth) is a harness parameter recorded alongside the full series; it is
  not a product budget.
- `cargo fmt --check` fails on pre-existing Lap files (inherited baseline,
  unchanged by this task); the new harness code is fmt-clean. Machine-local
  artifacts (runs/, target/) are gitignored.
