# Vignetting verification — 2026-09-29

Lap lap-9bc consumes engine rapidraw-3dd at
`bce4801a6f7a8536ba90a629b7999b0133383d6b` on the existing isolated branches.
See [spec.md](spec.md) for behavior and its primary Capture One reference.

## Automated evidence

- RED: model defaults/persistence and invalid-value tests failed; GPU test failed
  because negative EV did not darken edges; UI failed because the section was absent.
- GREEN: three model tests cover defaults, legacy fields, envelope/hash persistence,
  all methods, invalid amount/method, non-finite values even when bypassed.
- Hardware GPU regression covers zero/legacy identity, negative/positive EV,
  unchanged center, ellipse vs circle on rectangular frames, full-frame vs crop
  anchoring for an off-center crop, and ROI/full-frame equality.
- Engine workspace 177 passed, one generator ignored; RapidRAW host 162 passed.
  Both Rust fmt and Clippy gates pass (inherited workspace test warnings).
- Engine frontend 86 tests, typecheck and i18n pass. Existing lint baseline remains
  867 errors/52 warnings; Prettier flags the same 208 files, untouched by Vignetting.
- Lap frontend 280 tests pass. New regressions exercise a complete gesture/Undo,
  three methods, numeric input, bypass, section/global reset, committed envelope
  and selective Effects copy/paste validation. Production Vite build passes.
- Optional Lap typecheck retains the nine old diagnostics tracked by lap-372
  (command-only TS6 deprecation override); no new diagnostic. Vite does not
  typecheck Vue SFCs.
- Lap native debug build and `cargo +1.98.0 test --manifest-path src-tauri/Cargo.toml --locked`
  pass: 205 tests, one optional benchmark ignored. Clippy `--all-targets --locked`
  passes with inherited warnings (205 on the app target). Rust fmt still reports
  inherited differences in untouched files, including `variants.rs`.
- The first native attempt lacked CMake on PATH and failed while configuring
  libheif. Repeating with the documented Visual Studio CMake directory plus
  rustup on PATH completed build, tests and Clippy successfully.
- Contract is copied from committed engine bytes; hashes refreshed. Cargo.lock
  changes only the two engine revision sources, preserving Windows dependencies.

## Native verification

Tested the rebuilt Windows desktop executable using Computer Use on the existing
review copy `review-fuji.RAF` (7752 x 5178, 40.1 MP):

- Opened Vignetting below Effects; initial Amount 0 and Elliptic on Crop preserve
  the existing development settings.
- Dragged to -2.15 EV: edges visibly darken, center retained. One Undo restores
  the entire drag. Dragged to +1.65 EV: edges visibly brighten.
- Selected all three methods through the native dropdown. Circular on Crop and
  Circular agree for this uncropped image; crop differences are verified by the
  controlled hardware GPU regression described above.
- Bypassed and re-enabled the section: baseline appearance returns when bypassed;
  Amount and Method stay retained.
- Entered -1.50 numerically and observed Saved. Closed Lap, confirmed the window
  exited, relaunched the new executable and reopened the section. The enabled
  state, -1.50 EV and Circular method were restored with the corresponding image.
- Used only the Vignetting section reset: Amount 0, Elliptic on Crop, Preview ready
  and Saved confirmed. Existing Light/Color Balance/other edits were preserved.
  Lap remains open with Vignetting expanded.
- The app's latest-input-to-first-preview indicator reported 87-107 ms across
  the editing checks, and 113 ms for reset after restart. These are observed
  samples from a debug build, including canvas upload but excluding monitor
  scanout; they are not a statistical performance benchmark or a speedup claim.

## Limits

Windows only. Crop geometry and ROI are tested on hardware using controlled
pixels; no proprietary Capture One pixel parity. Preview/export share the GPU
path; an exported-file comparison is not part of native verification here.
Vignetting has its own EV controls; legacy Effects vignette remains unchanged.
