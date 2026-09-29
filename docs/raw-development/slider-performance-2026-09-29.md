# Slider performance correction — 29 September 2026

Tracking: `lap-b3d`, branch `pebbles-harness/raw-development`.

## Findings and changes

1. Every adjustment rebuilt the full-resolution lens/geometry/downscale input even though exposure, contrast and other tonal changes do not change those pixels. GPU upload caching did not avoid this CPU work. The renderer now retains one prepared linear-float input (maximum 64 MiB). Identity includes the decoded original, session, geometry/lens hash and requested edge. A weak original reference avoids keeping the full RAW decode alive after session close. Changing source, geometry or size replaces the entry. Larger prepared inputs are rendered but not cached. Export and settled precision are unchanged.
2. The interactive timer was a trailing debounce: every pointer input restarted its 120 ms wait, starving previews until movement stopped. It is now a 60 ms throttle that uses the latest recipe at execution. Backend generation cancellation and bounded scheduling remain active. Settled rendering and the 800 ms durable-save debounce remain unchanged.

## Actual local measurement

Opt-in `develop::sessions::tests::measure_warm_preview` uses real RAW decode and `GpuPreviewRenderer`, changes exposure between six renders after one warm-up, and includes GPU readback. Same 1536-pixel maximum edge, debug profile and source in both runs. Source: copied Fujifilm X-T5 RAF, 7752 x 5178. Device: NVIDIA GeForce RTX 4060, Vulkan. Baseline is the uncached implementation with preparation extracted into a method; optimized run adds caching.

| Run | Raw warm samples (ms) | Median |
| --- | --- | --- |
| Before | 7697.7704, 10779.6061, 7968.8762, 7549.3182, 7737.5830, 7672.7496 | 7717.7 ms |
| After | 128.4339, 70.7325, 19.0244, 18.7484, 18.2127, 18.5554 | 18.9 ms |

This is approximately 408x lower median renderer latency in this small local sample. It is not a measured pointer-to-screen percentile or a cross-platform qualification. Baseline CPU contention from concurrent checks can affect individual samples; the removal of repeated full-resolution work is also verified structurally by exact prepared-buffer reuse. First decode/preparation, geometry edits and cache replacement remain expensive. Existing performance qualification records are historical and have not been silently reclassified as passing.

## Regression coverage

- RED continuous-input test: zero renders during 600 ms of 20 ms inputs. GREEN: bounded periodic renders using recent values; no premature durable commit.
- RED prepared-input test: tonal changes allocate a new base. GREEN: same Arc reused for tonal edit; geometry, size and decoded-source replacement invalidate it; returning to the earlier source misses because only one entry is retained.
- Full frontend suite: 260 passed. Production frontend build passed.
- Full Rust suite: 205 passed, optional real-GPU benchmark ignored by default and separately executed successfully before/after. Clippy passed with existing repository warnings; changed Rust file passes rustfmt and git diff --check passes.
- Native computer-use check: actual slider drags changed Fuji exposure from 0 to 2.43, then 0.84; rendered image/histogram updated and Saved appeared. Restored exposure to 0 afterward. This is functional GUI evidence, not an instrumented end-to-end latency measurement.

To repeat the optional measurement, set `LAP_PREVIEW_BENCH_RAW` to an existing RAW fixture and run the ignored `measure_warm_preview` lib test with `--ignored --nocapture` using the repository's required MSVC/rustup environment. Local raw output: `%TEMP%/lap-perf-before.log` and `%TEMP%/lap-perf-after.log`.
