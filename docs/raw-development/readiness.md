# Final readiness assessment: A1-A12 and P1-P7 (lap-da3 / TASK-603)

Issued 2026-09-28 by lap-da3 (managed continuation of the legacy coordinating
issue lap-404.3; legacy history preserved). Governing contract:
`docs/raw-development/spec.md` (SHA-256 `49680993969aa54b0265c7671eaa3b17204
3425bbeb33e69f1855c8293c9284c`, re-verified 2026-09-28 with
`certutil -hashfile docs/raw-development/spec.md SHA256`). Machine-readable
companion: `docs/raw-development/readiness.json` (schema
`lap-raw-readiness/v1`); enforcement: `scripts/raw-development/check-release-gates.*`.

## Verdict: release BLOCKED — assessment COMPLETE and accurate

This is a finite local assessment. It classifies delivery evidence against
every original acceptance criterion; it grants no distribution, publication,
push, or main-branch merge permission. **BLOCKED release status is the honest
result and remains an unresolved product requirement — never a waiver.** The
fail-closed checker rejects the current evidence set with exit 1, which is the
expected assessment outcome, not a passing release check.

| Classification | Items |
| --- | --- |
| passed | A1, A2, A3, A4, A5, A6, A8, A9, A11; P1, P2, P4, P6 |
| failed | A12 (provisional warm-preview budget missed ~26x on the measured host) |
| blocked | A7 (device-loss/OOM not injected), A10 (interactive checklist pending); P3 (distribution decision), P5 (Linux/macOS coverage), P7 (packaging/non-Windows integration) |
| untested | (none — every entry carries executed evidence or an explicit gap record) |

## Revisions this assessment audited

- Lap host: branch `pebbles-harness/raw-development`, parent revision
  `9615d36e40f1637ec6453e45700903675ac744e4` + the commit carrying this file.
- Engine: consumed pin `de4fdbd76723c76145b477a2b8874226512475f1`, identical
  across `engine-lock.json`, `src-tauri/src/develop/sessions.rs`
  (`ENGINE_GIT_REVISION`), `src-tauri/Cargo.lock`, and `src-tauri/Cargo.toml`.
  Engine checkout HEAD is `73ed5e87` = pin + tracker-event lines only
  (`git diff --stat` shows `.pebbles/events.jsonl` alone), so both hosts build
  the same engine source content. This is honestly recorded as
  `checkoutHeadMatchesPin: false` in `readiness.json`.
- Render baseline: `5e30bcbb246395d391ba2e9662510641ffe68e6b` (unchanged).
- Producer receipts re-hashed with certutil: TASK-103
  `8126171e…3e3b5` and TASK-602 `7cccd8c2…09752` — both MATCH the accepted
  receipt SHA-256 values in the issue statement.

### Resource/schema hash verification and a corrected inventory entry

All `engine-lock.json` hashes were re-derived on 2026-09-28 from committed
bytes (`git show <rev>:<path>` via cmd-level redirection — PowerShell stream
redirection mangles bytes and was deliberately not used), cross-checked
byte-identical against the cargo-resolved checkout under a fresh CARGO_HOME.

**Finding (lap-da3):** the `gen/recipe.ts` and `gen/default-recipe.json` hashes
recorded by lap-d52 (TASK-502) did not match the committed content at the pin
in either LF or CRLF form, and no automated gate validated them. This
assessment corrected both entries in `engine-lock.json` (verified twice plus
against the cargo checkout: `7c1cb13c…` and `bceb1e7a…`), recorded the
correction in `engine-lock.json` (`hashCorrection`) and in
`readiness.json` (`revisions.resourceSchemaVerification.corrected`). Every
other entry (3 shaders, workspace manifest/lock, both crate manifests) re-
verified unchanged. With the correction, dependency/provenance inventories
reflect the actual builds.

## Acceptance items (exact evidence in readiness.json)

- **A1 passed** — e2e `a1-source-immutability`: source SHA-256 identical
  through edit/restart/reset/full-res export; typed `DestinationProtected` for
  source/sidecar/alias destinations; hardlink/case/canonical alias rejection
  (lap-7ae); rollback migration asserts source immutability (rollback.md).
- **A2 passed** — e2e `a2-ab-navigation` + `useDevelopEditor`
  switch-before-debounce flush + persistence restore suites.
- **A3 passed** — e2e `a3-process-termination` (hard `taskkill /F` mid-commit),
  fault/crash injection at every save boundary (`develop_persistence`),
  corrupt/future-schema rejection with preserved payloads, explicit
  read-only failure, orphan temp cleanup at startup (lap-63f RED→GREEN).
- **A4 passed** — e2e `a4-conflict-stale` (typed `RevisionConflict`,
  `StaleGeneration`, cancelled delayed reply), engine session coalescing,
  stale-contribution rejection in the derivative cache.
- **A5 passed** — e2e `a5-fullres-export` (5472×3648 original dimensions,
  byte-identical re-export, orientation-6 transposed decode; embedded
  previews never serve as the development source).
- **A6 passed (single-host scope)** — 34/34 headless export cases
  byte-identical to the frozen pre-extraction baseline, re-frozen at the final
  pin by lap-63f; frozen exact tolerances; preview vs downsampled export
  byte-identical under one color transform with corresponding histogram
  (`preview-export-parity.json`). Real CC0 fixtures; GPU runs asserted.
  Scope note: one machine/adapter only; extending tolerances across
  backends/platforms is release work under P5 (blocker B2).
- **A7 blocked** — missing adapter, oversized texture (`TextureTooLarge`),
  missing resources, broken decodes are executed typed failures; device loss
  and OOM were **not injected** (typed paths exist only) — blocker B5.
- **A8 passed** — journaled rename/move/copy/trash companions, copy identity
  fork, fingerprint replacement detection, RAW/JPEG pair binding, Unicode
  paths, missing media, ambiguity surfacing, catalog rebuild re-association
  (lap-487, lap-952). Recorded limits: OS recycle-bin restore is simulated;
  no UI-level file-operation e2e.
- **A9 passed** — reset, 50-entry session undo/redo, one transaction per
  gesture, persisted section bypass, oriented crop/orientation with mask
  anchoring, preset hover/cancel/apply (hovering never saves), selective
  copy/paste, imported unsupported effects stay visible as named limitations
  (lap-adc, lap-6bc, lap-62b, lap-78d, lap-5c2).
- **A10 blocked** — interactive checklist K1-K9 entirely pending
  (`interactive-verification.md`); automated proxies (231 vitest incl. the
  mounted DevelopPanel, launch lifecycle evidence) reduce but do not remove
  the gap — blocker B4. Never classified as passed.
- **A11 passed** — fresh-cache standalone consumer build at the final pin
  (2m56s, no AI models/credentials/application window) with correct decodes of
  the synthetic gradient (64×48), orientation-6 DNG (5464×8192), and Bayer CR3
  (5472×3648); both hosts verified on the same tested revision (pin equality +
  HEAD delta = tracker events only).
- **A12 failed** — measured Windows host (RTX 4060, Vulkan): warm 1536 px
  preview p95 **3925.6 ms vs the provisional 150 ms target — MISSED ~26x**
  (attribution: ~3535 ms host CPU full-res downscale per generation; engine
  warm re-render ~18–20 ms); cold decode p95 2305.6 ms; export p95 4644 ms;
  navigation memory bounded. Final budgets remain unagreed by design until the
  measured slice is optimized and re-agreed — blocker B3. Linux/macOS untested
  — blocker B2.

## Prerequisites

- **P1 passed** (spec source map, pinned revisions), **P2 passed** (branch
  descends from spec-inspected `67cda344`; pre-existing checkout untouched),
  **P4 passed** (CC0 corpus: Bayer, X-Trans, linear DNG, orientation,
  highlight stress, >4096 px; frozen baselines), **P6 passed** (writable
  adjacent sidecar adopted; explicit read-only failure with retained session).
- **P3 blocked** — `distributionDecision` holds with no deciding authority and
  8 release-blocking `openQuestions` (the combined-product decision itself plus
  the 7 technical unknowns of blocker B7).
- **P5 blocked** — Windows executed and measured; Linux/macOS reference
  machines do not exist here and are declared unavailable; the committed CI
  workflow is an unexecuted definition and never counts as evidence (B2).
- **P7 blocked** — Windows dev-build integration is evidenced; packaging
  (`tauri bundle`) was never exercised and non-Windows integration is
  unverified (B6).

## Task inventory and tracker honesty

All 25 plan tasks (TASK-101…TASK-603) have executed, evidenced managed issues
(listed per key in `readiness.json.tasks`). Legacy imported issues remain
intentionally open as preserved history; **tracker drain is not equated with
product qualification anywhere in this assessment** — classifications derive
solely from the linked evidence.

## Fail-closed checker behavior (RED→GREEN)

- RED (before implementation): `npm --prefix src-vite run test -- --run
  tests/readiness-gates.test.js` → **15/15 failed** (no readiness gates; no
  `readiness.json`); log: `tests/raw-development/runs/lap-da3/red-readiness-gates.txt`.
- GREEN (after implementation): full vitest suite → **26 files / 246 tests
  passed**, including the 15 readiness-gate tests: omitted A6/A10/A12 entries
  rejected; A6/A10/A12 `passed` without `gpu-fixture-parity` /
  `interactive-verification` / `platform-measurement` evidence rejected;
  skipped task (`TASK-403` → `skipped`) rejected; `ready` decision while P3 is
  unresolved or blockers exist rejected; malformed readiness document → exit 2;
  a fully-evidenced synthetic fixture with an approved decision releases with
  exit 0.
- Real evidence set: `node scripts/raw-development/check-release-gates.mjs` →
  **exit 1 (blocked)**; gates: provenance `unknowns-resolved` and
  `distribution-decision` failed (expected); all readiness gates passed —
  i.e. the assessment itself is complete and the blocked decision is recorded
  consistently. Retained output: `tests/raw-development/runs/lap-da3/gate-real-run.json`.
  The PowerShell wrapper propagates the same exit code.

## Remaining release blockers (all required; none waivable)

| ID | Blocker | Resolution step |
| --- | --- | --- |
| B1 | Combined-product distribution decision unresolved (P3) | Human licensing authority completes `distributionDecision` in `provenance.json` (approved + decidedBy/decidedAt/scope/evidence); crate license fields added; see `release-gates.md`. |
| B2 | Linux/macOS platform coverage absent (P5; A6 tolerance scope) | Provision declared reference machines; run `scripts/raw-development/benchmark.ps1` on each; freeze per-backend tolerances; platform gate must reflect executed passing runs. |
| B3 | Warm-preview budget missed; final budgets unagreed (A12) | Implement/measure host preview-base caching/downscaling; re-run the benchmark; agree final budgets per spec without redefining the provisional threshold. |
| B4 | A10 interactive verification not executed | Human executes K1-K9 and records observations; re-classify A10 on that evidence. |
| B5 | Device-loss/OOM driver-fault evidence missing (A7) | Execute driver-level fault qualification on a reference machine; record typed outcomes. |
| B6 | Packaging / non-Windows build integration unexercised (P7) | Run packaging builds per platform; reconcile artifact inventories with `provenance.json`. |
| B7 | Component licensing unknowns (lensfun data, shader audit, model weights, ffmpeg sidecar, ONNX Runtime, crate license fields, full dependency audit) | Resolve each `openQuestion` with primary-source evidence; the `unknowns-resolved` gate passes only at zero open questions. |

## Local handoff

1. Reproduce the assessment verdict:
   `powershell -ExecutionPolicy Bypass -File scripts\raw-development\check-release-gates.ps1`
   (expect exit 1 while blockers stand).
2. Reproduce the test gate: `npm --prefix src-vite run test` (246 tests).
3. Do not edit `.pebbles/events.jsonl` by hand; do not treat a future green
   tracker as readiness — re-run this checker and re-classify on evidence.
4. Publication remains disabled: no release, publication, push, or main-branch
   merge has occurred or is authorized by this assessment.
