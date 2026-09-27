# RAW development regression fixtures (lap-7f5.2 / lap-d13, TASK-102)

Created 2026-09-26/27. Machine-readable sources of truth live under
`tests/fixtures/raw-development/`; this file documents what exists, why it is
licensed for use, and how the pinned baselines were captured. Governing
contract: `spec.md` (prerequisite P4, acceptance A5/A6).

## Layout

```text
tests/fixtures/raw-development/
  corpus-manifest.json     real RAW corpus: provenance, license, checksums, dims
  corpus/                  CC0 real camera RAW files (+ one derived variant)
  synthetic/               deterministic generated DNGs + synthetic-manifest.json
  presets/                 adjustment override JSONs + presets-manifest.json
  baselines/               capture-manifest.json (frozen pre-extraction outputs)
scripts/raw-development/
  acquire-corpus.ps1       pinned-list download + sha256 verification
  generate-synthetic-fixtures.mjs  byte-deterministic synthetic DNG generator
  make-orientation-variant.mjs     TIFF Orientation=6 derivation
  png-info.mjs             PNG dimension probe used by the capture
  capture-baselines.ps1    baseline capture / compare tool (see below)
```

## Real corpus

| Fixture | Categories | Decoded |
| --- | --- | --- |
| `corpus/canon-eos-r6-craw-iso100-nocrop.CR3` | bayer, fullres | 5472x3648 |
| `corpus/fujifilm-xt5-lossy-iso125.RAF` | xtrans, fullres | 7728x5152 |
| `corpus/dng-jpegxl-lossy-16bit-linear-tiles.DNG` | linear-dng | 8192x5464 |
| `corpus/canon-eos-r6-craw-iso204800-nocrop.CR3` | highlight-stress, bayer, fullres | 5472x3648 |
| `corpus/dng-linear-orientation6.DNG` | orientation, linear-dng (derived) | 5464x8192 (transposed) |

Coverage satisfies P4: Bayer, X-Trans, linear DNG, orientation, highlight
stress, and multiple RAWs larger than 4096 px (A5). The orientation fixture is
byte-identical to the dnglab linear DNG except the 2-byte inline TIFF
Orientation value (1 -> 6); the engine applies it, producing transposed output
dimensions.

### Licensing and permitted use

- Sources: `https://rawdb.dnglab.org` (dnglab RawDB). Every entry records its
  `sourceUrl`, the RawDB API license field, and notes dnglab's
  `CONTRIBUTE_SAMPLES.md`, which requires contributed sample sets to be CC0
  with contributor copyright ownership. License for every file: **CC0-1.0**.
- The derived orientation variant adds no authorship (2-byte value change of a
  CC0 file).
- No private photo library data is committed. Acquisition used a pinned file
  list with sha256 verification against the RawDB API records and a hard byte
  budget (83,305,796 of 209,715,200 bytes used).
- Re-acquire reproducibly with `scripts/raw-development/acquire-corpus.ps1`
  (`-SkipDownload` re-verifies local bytes only). Re-runs rewrite the corpus
  manifest BOM-free and carry over engine-verified `decodedDimensions` for
  byte-identical fixtures, so re-verification never breaks the JSON gates.

## Synthetic suite

`generate-synthetic-fixtures.mjs` emits byte-for-byte deterministic DNGs (no
timestamps or randomness) in two separate groups:

- **numerical**: 64x48 linear gradients, highlight-clipped frame, 5000x64 wide
  gradient (>4096 px), 8x2 boundary constants, and `wrong-tiff-magic.dng`.
  The wrong-magic file documents *measured* engine behavior: the pinned
  revision tolerates TIFF magic 43 and renders byte-identically to the
  gradient fixture; the baseline pins that behavior so a future decoder must
  change it explicitly, never silently.
- **failure**: truncated header, header-only, ASCII text, and zero-byte files.
  The engine must reject each with an explicit error (exit 1), never fall
  through silently.

Synthetic fixtures complement the real corpus for numerical/failure tests;
they do not establish camera-RAW parity (spec P4).

## Pinned pre-extraction baselines

`baselines/capture-manifest.json` (schema `lap-raw-baseline/v1`) freezes the
pinned RapidRAW renderer's headless export results:

- **Engine pin**: captured against engine commit `1e072132` with all
  render-relevant sources (`raw_processing.rs`, `image_processing.rs`,
  `gpu_processing.rs`, `export_processing.rs`, `adjustment_utils.rs`,
  `image_loader.rs`) verified unchanged from pre-extraction render baseline
  `5e30bcbb` and individually sha256-hashed in the manifest.
- **GPU evidence**: NVIDIA GeForce RTX 4060, wgpu backend `Dx12`; a
  `processed (ROI: ...) on GPU in` log line is asserted for every render case
  (spec A6 requires real GPU runs).
- **Decode options**: effective values for highlight compression (2.5),
  linear raw mode (auto), preprocessing color NR (0.5) and sharpening (0.35),
  preprocessing applied to non-RAWs (false), backend (dx12). Keys absent from
  the host profile fall back to engine code defaults, re-verified against the
  pinned engine source at capture time (the engine normalizes settings.json
  on exit, which would otherwise read as spurious drift).
- **Color spaces**: working = linear scene-referred f32 (rawler RawDevelop,
  calibration on, sRGB step removed); output = display-referred sRGB-encoded
  PNG.
- **Case matrix**: 34 cases = default + combined-tone + combined-color for
  every real fixture; the primary Bayer fixture additionally carries 8
  per-adjustment presets (exposure, contrast, highlights, shadows, saturation,
  temperature, sharpness, clarity) and combined-full; every synthetic
  numerical fixture at default; every synthetic failure fixture as an
  expected-failure case (nonzero exit + logged error evidence, no render
  checksum).

### Determinism characterization and frozen tolerance

Each capture invocation runs every case twice (`-Repeat 2`, sha256 compared
in-run), and complete independent invocations were compared against the frozen
manifest (compare mode). All runs were byte-identical: **result =
deterministic, frozen tolerance = exact checksum equality (0)**. The script
aborts rather than picking a hash if any repeat diverges; tolerances must then
be derived from deliberate pixel-diff characterization, never guessed.

### Capture and regression workflow

```powershell
# capture (writes manifests; baseline absent, or explicit -Update)
powershell -ExecutionPolicy Bypass -File scripts\raw-development\capture-baselines.ps1 [-Update]

# compare against the frozen baseline (exit 1 on any drift = regression signal)
powershell -ExecutionPolicy Bypass -File scripts\raw-development\capture-baselines.ps1
```

Baselines are never regenerated implicitly: overwriting the frozen manifest
always requires the explicit `-Update` flag after an intentional, reviewed
engine change. Missing fixture rights or unavailable GPU execution block
baseline work entirely (issue-level constraint).

## Gates

- Lap: `npm --prefix src-vite run test` — `src-vite/tests/raw-corpus.test.js`
  verifies corpus checksums/provenance/coverage, synthetic determinism
  manifests, preset integrity, and the frozen baseline structure.
- Engine twin (rapidraw-39c): `cargo test --manifest-path
  src-tauri/Cargo.toml --test raw_corpus_gates` mirrors the same gates and
  additionally fails if any render-relevant engine source no longer matches
  the hashes pinned in the baseline manifest.

## Honest limitations

- Baselines are pinned for this machine and adapter (RTX 4060 / Dx12) only;
  backend/precision differences on other hosts are expected to need their own
  frozen tolerances. No cross-platform qualification is claimed.
- Editor preview and histogram behavior of the pinned revision is recorded in
  the manifest's `defaults` section as a contract; it is not exercised by the
  headless capture path.
- Small generated images complement the real corpus but do not establish
  camera-RAW parity (spec P4).
