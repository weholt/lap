# Color Balance — 2026-09-29

Implemented for lap-eb3; engine validation fix rapidraw-0be.

## Interaction

Develop has a separate **Color Balance** section below Color. Master, 3-Way,
Shadow, Midtone and Highlight tabs edit the existing `colorGrading` recipe.
The center puck controls hue and saturation, the rim changes hue only, and
curved side rails control saturation and lightness independently. Master
lightness is disabled. Numeric fields, arrow keys, Shift for larger keyboard
steps, per-wheel reset and whole-tool reset are available. Double-click inside
the wheel neutralizes tint while preserving hue/lightness. Each drag is one
undo transaction. 3-Way and individual tabs share exactly the same values.
Small panels stack the three wheels so their pointer targets remain usable.

The interaction is based on Capture One's official
[Adjusting color balance](https://support.captureone.com/hc/en-us/articles/360002594937-Adjusting-color-balance)
and [tool overview](https://support.captureone.com/hc/en-us/articles/360002594857-The-Color-Balance-tool-overview).
Rendering remains the existing RapidRAW GPU grading algorithm; this does not
claim pixel-identical Capture One output, Capture One preset compatibility or
proprietary tonal response. Tonal range balance/blending remain available under
Tonal ranges. The engine's Color section bypass also bypasses Color Balance;
the panel reports that state.

## Compatibility and responsiveness

The shared model incorrectly treated grading hue as an HSL channel offset,
rejecting blue/cyan/magenta values above 100 degrees. Engine commit
`e754dea8f0ecefc85da7f518ed412567576d85bf` accepts grading hues through 360,
including masks, while retaining the previously valid negative hue and signed
saturation/lightness values. Opening a legacy recipe or changing tabs does not
rewrite it. HSL offset bounds are unchanged. No schema, shader or source pixel
changes. Lap consumes this exact local revision with its existing transitive
Windows dependency locks retained.

A wheel update changes hue/saturation together in one recipe update, using
existing throttled latest-generation previews, stable preview sizing, debounced
sidecars and session-scoped undo. No extra render queue or synchronous disk work
is added to pointer handling.

## Automated validation

- Lap frontend: 271 tests pass; production Vite build passes.
- Wheel regressions exercise full-circle/clamped dragging, independent hue and
  side rails, pointer cancellation, keyboard/numeric input, Master lightness and
  cleanup. Panel tests check atomic undo/redo, shared tabs, saved recipe payload,
  reset isolation, legacy preservation and all nine locale bundles.
- Engine model: 53 tests pass (one generator ignored); host Rust: 162 pass.
  Rust host fmt/clippy pass; new regression tests pass clippy with warnings denied.
- Engine frontend tests, typecheck and i18n check pass. Untouched baseline lint
  reports 867 errors/52 warnings; Prettier reports 208 existing files. These were
  not reformatted as part of the model validation fix.

## Native app verification

Computer-use testing on Windows with the built Lap executable, real
`review-fuji.RAF` (Fujifilm X-T5, 7752 x 5178) in the existing review album:

- Master puck drag from neutral to hue 222.7 / saturation 56.7 visibly changed
  the RAW preview and histogram; Saved appeared with no validation error.
  One Undo restored both values to zero and disabled Undo, proving one gesture.
- In 3-Way, a Shadow drag produced hue 213.6 / saturation 40.3; the right arc
  raised luminance to 39.2 while preserving hue and saturation. The separate
  Shadow tab showed those same values. The left arc reduced saturation to 25.8
  while preserving hue 213.6 and luminance 39.2.
- Closed and restarted the app after Saved. The Shadow tab restored exactly
  213.6 / 25.8 / 39.2 from disk. Per-wheel reset restored the original neutral
  shadow settings. Master, Midtone and Highlight remained neutral; existing
  exposure/brightness/curves and the other photo were preserved.
- Reported input-to-presented-preview latency was 84 ms for the Master drag,
  100 ms for the Shadow drag, 99 ms for lightness and 101 ms for saturation;
  reset after reopening was 115 ms. This is app instrumentation through canvas
  upload and the next animation frame, not an external physical-display timing.
- The app is left open with Color Balance expanded on Master.

Lap Rust integration suite: 205 tests pass, one optional benchmark ignored.
The initial attempt while Lap was open hit a Windows executable lock; the full
suite passed with the app closed before the persistence-reopen check.

Lap clippy --all-targets passes with existing warnings (205 in the application target).
