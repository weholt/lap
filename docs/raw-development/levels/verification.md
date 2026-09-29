# Levels verification — 2026-09-29

Lap lap-fcd consumes engine rapidraw-ae7 at
`271c8d3ff11be4c6bec085d15ca8ea8b377c0ea4` on the existing isolated feature branches.
See [spec.md](spec.md) for behavior and primary Capture One references.

## Automated evidence

- RED: 3 model tests failed (missing defaults/round-trip and ignored invalid
  endpoints); hardware GPU test failed (input 20 remained 20 instead of 15);
  panel test failed because Levels did not exist.
- GREEN: 4 model tests and 2 hardware GPU tests pass. The latter verify neutral
  bit identity, RGB input/output mapping and extrapolation, midpoint direction,
  separate R/G/B channels, bypass and composition with existing curves.
- Engine workspace: full suite 171 passed, 1 generator ignored before the final
  additional persistence/non-finite and RGB/curve-composition tests; those added
  tests then passed in the focused final run. RapidRAW host: 162 tests pass.
- Engine Rust fmt passes for workspace and host. Clippy passes; inherited test
  warnings remain. Both changed test targets pass with `-D warnings`.
- Engine frontend: 86 tests, typecheck and i18n pass. Existing lint baseline
  remains 867 errors/52 warnings and Prettier 208 files, unchanged by Levels.
- Lap frontend: full suite 277 passed; the subsequently added atomic drag/save
  regression also passed (2 Levels panel tests in that run). Vite build passes.
- Lap native debug build passes. Full Rust suite: 205 passed, 1 optional benchmark
  ignored. Clippy passes with inherited application warnings (205 in app target).
- Lap Rust fmt check still reports pre-existing formatting in untouched variants.rs, t_migration.rs and other baseline files; no formatting sweep was included.
- Generated TypeScript body and all engine resource hashes match committed bytes.
  Cargo.lock changes only the two engine source revisions; Windows dependencies
  remain pinned exactly as before.
- Optional Lap `tsc --noEmit`: blocked by existing TS6 config deprecations. With
  command-only `--ignoreDeprecations 6.0`, 9 pre-existing source diagnostics remain,
  including the already undefined RecipeEnvelope alias. Tracked separately as
  lap-372; no Levels-specific diagnostic. Vite does not typecheck Vue SFCs.

## Native interaction evidence

Windows computer-use against the built Lap.exe on the existing copied
`review-fuji.RAF` (Fujifilm X-T5, 7752 x 5178). Existing Light and Color Balance
adjustments were retained; tests modify only the newly neutral Levels block.

- Drag input black 0 to 29: visible increased black clipping; Saved; 115 ms
  reported input-to-first-preview. One Undo restored 0 and disabled Undo,
  demonstrating that the complete drag was one transaction.
- Drag output white 255 to 233: highlights darkened, input white stayed 255.
- Drag midpoint left to +0.41: midtones brightened, output white stayed 233.
- Drag input white 255 to 230: separate input range adjustment; 89 ms reported.
- Disable Levels: pre-Levels look returned and values remained 233/+0.41/230;
  explicit bypass indication appeared. Re-enable restored adjusted look.
- Select Red: independent neutral fields and red histogram appeared while RGB
  settings remained active. Native numeric control accepted Red output white 210.

- Restarted the built native app after Saved: RGB output white 233, midpoint +0.41 and input white 230 reappeared; Red output white 210 also persisted.
- Reset only Levels using its section reset: RGB and Red returned to 0/255 endpoints and midpoint 0; Saved and Preview ready confirmed. Existing Light and Color Balance adjustments were preserved. Lap remains open on the expanded RGB Levels panel.

Timing uses the application's instrumentation through canvas upload and the
next animation frame, not physical display scanout. No extra decode or preview
queue is added by Levels. Sampling for its histogram is bounded to 65536 pixels
per rendered frame and independent of handle movements.

## Scope

Global Levels only. No auto adjustment, eyedropper image sampling, mask-local
Levels or addition to the existing selective clipboard/preset section chooser.
No proprietary Capture One numerical parity is asserted. Shared GPU rendering
covers preview and export; native verification here focuses on preview and
persistence rather than a separate exported-file comparison.
