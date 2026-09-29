# Recipe schema v1 — validated edit-model contract (lap-3e2 / TASK-201)

Status: implemented in the isolated extraction checkout; Lap consumes it at a
pinned engine revision. Governing contract: `spec.md` (sections *Recipe and
session contract*, *Persistence and compatibility*). Linked engine issue:
`rapidraw-f53`; Lap issue `lap-3e2` (managed continuation of `lap-a35.1`).

| Item | Value |
| --- | --- |
| Crate | `RapidRAW-engine/crates/rapidraw-edit-model` (internal, `publish = false`) |
| Engine branch / revision | `feature/lap-engine-extraction` at `4c8c86e0` (docs commit `f5eb0302`) |
| Schema version | `1` (`SCHEMA_VERSION`; migrations reject anything greater) |
| Dependencies | `serde`, `serde_json` (with `float_roundtrip`), `sha2`, `hex` — no Tauri, React/Vue, filesystem, catalog, or AI dependencies |
| Generated contract | `crates/rapidraw-edit-model/gen/recipe.ts`, `gen/default-recipe.json` (committed, byte-pinned to the generator by tests) |
| Frontend consumption | `src/utils/adjustments.ts` (RapidRAW host) derives `INITIAL_ADJUSTMENTS` and its recipe adapters from the generated module |
| Rust consumption | `src-tauri` depends on the crate; `src-tauri/tests/recipe_model_contract.rs` pins parsing, validation, hashing and migration |

## Envelope (schemaVersion 1)

Serialized camelCase, every field always present:

| Field | Type / constraint |
| --- | --- |
| `schemaVersion` | `1`; readers must refuse larger versions without resetting data |
| `engineVersion` | non-empty string ≤ 64 chars (model/engine version that wrote the envelope) |
| `assetId`, `variantId` | non-empty, ≤ 256 chars, no control characters |
| `revision` | integer ≥ 1; monotonic per asset/variant for optimistic concurrency |
| `sourceFingerprint` | 64 lowercase hex characters (SHA-256 of untouched source bytes) |
| `decode` | effective decode/color settings (below) |
| `recipe` | the ordered adjustment recipe (below) |
| `resources` | ≤ 256 entries; keys ≤ 256 chars; values `{ algorithm: "sha256", digest: 64-hex, sizeBytes? ≤ 1 GiB }` |
| `unsupported` | ≤ 64 entries, ≤ 2 MiB canonical; payloads this schema does not model — preserved, never silently dropped |

### `decode` — effective decode/color settings

Captured so a render never depends on the interpreting application's current
defaults (spec: *RAW interpretation includes global settings…*):

`isRaw` (bool), `fastDemosaic` (bool), `highlightCompression` (f64
[1.01, 64], default 2.5 = engine `raw_highlight_compression`),
`linearRawMode` (`auto` | `gamma` | `skip_calib` | `gamma_skip_calib`,
matching `raw_processing.rs` strings), `rawColorNoiseReduction` ([0, 1],
default 0.5), `rawSharpening` ([0, 1], default 0.35),
`tonemapperOverride` (`basic` | `agx` | null).

## Recipe — persisted render data

All fields from the legacy `INITIAL_ADJUSTMENTS` shape are enumerated,
typed, and bounded. Flat scalars are bounded by the descriptor table
(`descriptors.rs`, `PARAM_DESCRIPTORS`: 52 parameters with min/max/default/
step taken from the RapidRAW adjustment components and modals — e.g.
`exposure` [-5, 5] step 0.01, `saturation` [-100, 100] default 0,
`transformScale` [50, 150] default 100, `lensDistortionAmount` [0, 200]
default 100, `sharpnessThreshold` [0, 80] default 15). Defaults cross-check
against the descriptor table in both directions via tests. Nested structures
are validated structurally:

- Curves (`curves`, `pointCurves`): per channel `luma/red/green/blue`,
  2–32 points, coordinates finite in [0, 255], strictly increasing x, first
  point pinned to x = 0, last to x = 255 (matches the editor).
- `parametricCurve`: darks/shadows/highlights/lights/whiteLevel/blackLevel in
  [-100, 100]; split1/2/3 in [0, 100] (defaults 25/50/75).
- `colorGrading` (balance [-100, 100] default 0, blending [0, 100] default
  50, zone hue in degrees [-100, 360] with negative legacy values retained;
  saturation/luminance in [-100, 100]), `hsl` (8 channels with signed
  component offsets in [-100, 100]),
  `colorCalibration` (7 values in [-100, 100]).
- Effects/grain/vignette/LUT/lens-blur scalars per the descriptor table;
  `lutSize` ≤ 4096; strings bounded (`lutName` 200, `lutPath` 1024,
  `lensBlurDepthMap` 512, lens maker/model 120 chars).
- `lensDistortionParams` (legacy keys `tca_vr` etc. preserved): k*-style
  coefficients in [-10, 10], `model` [0, 100], `tca_vr`/`tca_vb` [0.5, 2].
- `masks`: ≤ 32 containers; each with id (≤ 64), name (≤ 200),
  opacity [0, 100], local adjustments (the tone/color/detail/curve subset;
  mask-local noise reduction accepts [-100, 100] as in the UI), ≤ 16
  sub-masks (opaque `parameters` payload ≤ 64 KiB canonical each), and a
  preserved `unsupported` bucket (≤ 16 entries / 32 KiB per mask).
- `subMasks[].geometry` (lap-78d): typed, validated geometry for the
  supported non-AI kinds `brush`, `flow`, `linear`, `radial`, `all`
  (`rapidraw_edit_model::masks`); `null` for other kinds (AI, luminance/
  color) or unconvertible payloads. Limits: ≤ 256 lines per stroke
  geometry, ≤ 4096 points per line, ≤ 16384 total points; positions and
  lengths finite with |value| ≤ 16 in normalized units; `brushSize`,
  `radiusX`, `radiusY`, `range` > 0; `feather` in [0, 1]; `flow` in
  [0, 100]; radial `rotation` finite degrees. Geometry semantics and
  compositing (invert → opacity → additive/subtractive/intersect in
  sub-mask order, then container invert/opacity) reproduce RapidRAW
  `mask_generation.rs` at revision `5e30bcbb246395d391ba2e9662510641ffe68e6b`.
  A supported kind with `geometry: null` fails render/export explicitly
  naming mask and kind — it is never silently dropped.
- `sectionVisibility`: persisted bypass state for `basic/curves/color/
  details/effects` (`true` = the section applies). Accordion expansion,
  clipping overlays, hover previews and processing flags are **not**
  persisted.
- `sectionOrder`: the persisted operation order; validation requires exactly
  a permutation of the five canonical sections
  (`basic, curves, color, details, effects`).

### UI-only fields stay outside recipes

`showClipping` and `aiPatches` exist only in the legacy host shape. The
Rust schema has no such fields; the frontend adapters reset them explicitly
(`recipeToAdjustments`), and the legacy importer excludes them while
preserving the `aiPatches` payload under `unsupported["legacy.aiPatches"]`.
Inline `lutData` is likewise never recipe data — LUT content is an external
resource referenced through `resources` hashes.

## Geometry — oriented, normalized coordinates

- The oriented image is the source after `orientationSteps` (0..=3)
  clockwise quarter turns and the flip flags; steps 1 and 3 swap
  width/height, exactly like the frontend `getOrientedDimensions`.
- Origin: top-left of the **oriented** image; x right, y down.
- `crop` is normalized to the oriented full image: components in [0, 1],
  width/height > 0, `x + width ≤ 1`, `y + height ≤ 1`.
- Legacy pixel crops (`react-image-crop`, `unit: 'px'`, oriented frame)
  convert via `geometry::crop_to_normalized` / `crop_to_legacy` (pure
  rescale, no rotation). The legacy importer preserves an unconverted
  `crop` under `unsupported["legacyAdjustments.crop"]` until the host
  supplies oriented dimensions — it never guesses.

## Mask geometry — oriented frame and rendering contract (lap-78d)

Mask geometry lives in the **same oriented, un-cropped frame** as `crop`:

- Positions (`points`, `centerX/centerY`, `startX/startY`, `endX/endY`) are
  normalized to the oriented full-frame size (x by width, y by height).
- Lengths (`brushSize` diameter, `radiusX`, `radiusY`, linear `range`) are
  normalized to the oriented full-frame **width**, so circles stay circles
  for any aspect ratio: rasterization converts back to oriented pixels and
  applies the reference per-pixel formulas exactly.
- Crop and orientation never move a mask relative to image content: a mask
  painted at an image feature stays anchored under later crop/rotation/
  flip edits, and survives copy/move/rebuild because it is recipe payload.
- Rendering maps `dst = src_px * scale − crop_px * scale` with the uniform
  scale `output_width / oriented_width`, reproducing the reference bitmap
  pipeline; previews and exports share one rasterizer (`rapidraw_develop::
  masks`) and a bounded bitmap cache (cleared above 50 entries).

## Persisted render data vs transient interaction state

Persisted: every recipe field above plus the envelope identity/resources.
Transient (no schema representation): zoom, clipping display, selected tool,
hover presets, drag state, marquee selection, undo/redo stacks, preview
generations, and processing flags. Undo history is session state per the
spec and is never persisted with the recipe.

## Saturation semantics — explicit conversion, not name mapping

RapidRAW recipe saturation is a deviation value: default **0**, range
[-100, 100]. Lap's legacy `uiStore.activeAdjustments.saturation` is a
CSS-filter value: neutral **100**, range [0, 200]. The conversion is
arithmetic and clamped, defined once in the model and generated for hosts:

```
saturationFromLapLegacy(v) = clamp(v, 0, 200) - 100   // NaN → neutral
saturationToLapLegacy(v)   = clamp(v, -100, 100) + 100
```

Both neutral values map to recipe 0. No field is mapped by name similarity;
other Lap legacy fields (CSS `blur`, `filter` strings) have no recipe
counterpart and are not converted.

## Canonical serialization and hashing

Canonical form: UTF-8 JSON produced through a `serde_json` value roundtrip —
object keys lexicographically sorted, no insignificant whitespace, ryu
number formatting. `content_hash()` is the lowercase hex SHA-256 of those
bytes. The crate enables serde_json's `float_roundtrip` feature so a
serialize → parse → serialize cycle is bit-stable; the seeded property test
(200 random in-bounds recipes) pins hash stability across roundtrips and
the canonical bytes are key-order independent.

## Migrations and legacy import

- `parse_envelope(json)`: current schema only. Unknown envelope fields are
  projected into `unsupported` (prefixed `envelope.`), then the envelope is
  validated.
- `migrate_envelope(json, identity)`: hosts must supply identity
  (`engineVersion`, `assetId`, `variantId`, `sourceFingerprint`) — the model
  never invents identity. Future `schemaVersion` values are rejected with
  `UnsupportedSchema { found, supported_max, preserved }`, where `preserved`
  carries the original parsed payload; callers must keep it visible and must
  never overwrite such data with defaults. Legacy `.rrdata` documents (no
  `schemaVersion`, an `adjustments` object, optionally legacy metadata) are
  imported with explicit key mapping, including the legacy `centré` key;
  unknown fields are preserved under `legacyAdjustments.*`, legacy metadata
  under `legacyMetadata.*`; `showClipping`/`aiPatches` are excluded as
  UI-only (payload preserved); `crop` and inline `lutData` are preserved for
  explicit host conversion (dimensions / resource hashing).
- Import reports (`ImportReport`, `MigrationReport`) list excluded UI fields
  and preserved keys so unsupported content stays visible as a limitation.

## Generated frontend contract

`gen/recipe.ts` contains: `RECIPE_SCHEMA_VERSION`, `SectionId`,
`CANONICAL_SECTION_ORDER`, all interfaces (with the legacy-compatible index
signatures on `Curves`, `ParametricCurve`, `Hsl`, `ColorGrading`,
`SectionVisibility`, `MaskSectionVisibility` so the generated types remain
assignable to the host's structural types), `DEFAULT_RECIPE` (pretty JSON
literal of the Rust default), `RECIPE_PARAM_RANGES` (per descriptor key),
and the saturation conversion helpers. `gen/default-recipe.json` is the
canonical default fixture for non-TS consumers. Both files are committed and
byte-compared to the generator by
`committed_generated_files_match_the_generator`; regenerate with
`cargo test --manifest-path crates/rapidraw-edit-model/Cargo.toml -- --ignored update_generated_files`.
The generated directory is excluded from eslint/prettier (like
`src-tauri/gen`) so formatting tools cannot fight the golden files.

Host adapter (`src/utils/adjustments.ts`):
`INITIAL_ADJUSTMENTS = recipeToAdjustments(DEFAULT_RECIPE)`;
`adjustmentsToRecipe` collects render data and explicitly drops
`aiPatches`/`showClipping`/`lutData`; `recipeToAdjustments` requires oriented
dimensions whenever a recipe carries a crop (explicit error otherwise).

## Verification (2026-09-26, Windows, rustup 1.98.0 MSVC, default features)

RED evidence (stub implementation): `cargo test --manifest-path
crates/rapidraw-edit-model/Cargo.toml` → 8 passed / 24 failed (validation
accepted non-finite/out-of-bounds values; migrations accepted schemaVersion
999; legacy import and TS generation missing). GREEN: same command →
`32 passed / 0 failed` (model_tests) + 3 lib tests.

| Gate (RapidRAW-engine) | Result |
| --- | --- |
| `cargo test --manifest-path crates/rapidraw-edit-model/Cargo.toml` | pass (35 tests, 1 ignored writer) |
| `cargo fmt --manifest-path crates/rapidraw-edit-model/Cargo.toml -- --check` | pass |
| `cargo clippy --manifest-path crates/rapidraw-edit-model/Cargo.toml --all-targets` | pass (0 errors) |
| `cargo test --manifest-path src-tauri/Cargo.toml` | pass (147 tests incl. 5 new contract tests) |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | pass |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` | pass (0 errors, 0 warnings) |
| `npm test` | pass (18 files / 86 tests; baseline 80 + 6 new contract tests) |
| `npm run typecheck` | pass (rootDir widened to `.` so the generated module can be imported) |
| `npm run lint` | fail — inherited: 867 errors / 52 warnings, identical to baseline |
| `npm run format:check` | fail — inherited: 208 files, identical to baseline |
| `npm run i18n:check` | pass (1395 plural resolutions, 13 locales) |
| `git diff --check` | clean |

Build isolation check: the model crate compiles and tests with only
serde/serde_json/sha2/hex — no Tauri, React/Vue, filesystem, catalog or AI
dependencies (verified by `cargo tree`-free inspection of its `Cargo.toml`
and by the standalone crate build itself).

Not claimed here: RAW decode/render parity (spec P4/A6 — needs real
fixtures), GPU behavior, read-only-library storage behavior (later tasks),
and any publication/distribution decision (spec P3).


## Independent Levels (lap-fcd / rapidraw-ae7)

`recipe.levels` is an additive schema-v1 object with neutral defaults for old
recipes. It contains `enabled` (default true) and `rgb`, `red`, `green`, `blue`
channels. Each channel has `inputBlack` / `outputBlack` default 0,
`inputWhite` / `outputWhite` default 255, and `midtone` default 0. Endpoints
are finite 0..255; each white endpoint must exceed its corresponding black by
at least 1. Midtone is finite -1..1; positive brightens. Invalid values are
rejected even when bypassed. The channel object may be partial on read;
missing fields use neutral defaults, matching other additive recipe blocks.

Levels are independent of curves and the five historical section bypasses;
`levels.enabled` bypasses only Levels. The shared GPU pipeline applies RGB,
then individual channels in display-referred space before curves. Neutral
channels are exact no-ops. This applies to preview and export. See
[the Levels specification](levels/spec.md) for mapping, extrapolation,
histogram semantics and UI acceptance. Older engine versions do not render
Levels and must not be used to edit recipes containing it.


### Vignetting (lap-9bc)

Additive schema-v1 `recipe.vignetting`: `{ enabled: true, amount: 0, method: "ellipticOnCrop" }`.
Amount is exposure in EV, finite in [-4, 4]. Methods are `ellipticOnCrop`,
`circularOnCrop`, `circular`; unknown values are rejected. The first two use the
cropped frame; Circular retains the full oriented-frame center and scale using
normalized recipe.crop. The GPU input contract is an already oriented/cropped
texture, matching Lap preview and export. Normalized crops can differ by less
than a pixel from integer crop rounding at very small preview sizes.

The independent enable flag and zero default preserve all prior recipes. The
legacy flat Effects vignette fields keep their original rendering and UI under
Legacy vignette. Global Reset all includes Vignetting; selective Effects
copy/presets carry the complete block, while older payloads without it leave the
target's Vignetting unchanged. No mask-local support. See
[vignetting/spec.md](vignetting/spec.md) for geometry and verification criteria.
