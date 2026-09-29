# Develop responsiveness review — 2026-09-29

Issue: lap-7cd. Checkout: `C:/Users/Thomas/Desktop/lap`, branch `pebbles-harness/raw-development`.

## Research and chosen approach

Google's [rendering performance guidance](https://web.dev/articles/rendering-performance) and [INP optimization guidance](https://web.dev/articles/optimize-inp) emphasize prompt visual feedback and reducing work on the interaction path. Darktable's [memory/performance documentation](https://docs.darktable.org/usermanual/development/en/special-topics/mem-performance/) describes preview processing and cache tradeoffs. Applied here: show a small draft promptly, reuse prepared image data, preserve full preview quality at rest, and measure the UI path rather than only GPU execution.

The engine already replaces a session's queued preview and cancels its previous in-flight token. A growing FIFO counter would misrepresent this design: per session there is at most one executing preview and one replaceable queued preview. Cancellation is cooperative; already submitted GPU work and an individual CPU resize cannot be forcibly interrupted by this host change.

## Concrete findings and fixes

1. The single prepared-image cache was evicted by the thumbnail session launched after every save. The next slider input repeated full-resolution RAW preparation. Replaced it with a four-entry LRU capped at 64 MiB, keyed by source identity, session, geometry, size and preparation quality. Weak source identities do not keep closed RAW decodes alive.
2. Heavy preparation held the cache mutex. A cache hit could wait behind another session's full-RAW resize. The mutex now protects only lookup/insertion, not preparation.
3. The default single preview worker serialized editor input behind thumbnail preparation. Two bounded workers allow these jobs to prepare concurrently; GPU submission remains serialized by the engine. This is capacity separation, not an absolute priority guarantee across all sessions.
4. Interactive and settled requests both used 1536px. Interactive requests now use 768px (one quarter as many output pixels); after 260ms without input the existing 1536px settled preview is requested. Drafts can downsample an existing prepared full preview. Settled renders never reuse a derived draft, including when callers explicitly request the same size for both qualities. Export behavior is unchanged.
5. Slider input now uses a 32ms latest-value throttle rather than 60ms. New input invalidates older frontend results immediately, even before the next timer dispatch. Additional cancellation checks stop obsolete work before GPU submission and reject it afterwards.
6. While the central Develop canvas owns the image, MediaViewer suppresses its separate session and duplicate canvas update. Closing Develop restores the ordinary viewer path.
7. A visible activity dot and status show updating, ready, waiting/opening or failure. The activity continues between draft and settled preview. The last measured first-preview response is shown in milliseconds. Existing save status remains separate.

## Real Windows computer-use measurements

Same machine, Windows Tauri debug executable, maximized 2560x1392 window, same copied Canon EOS R6 RAW (5496x3670, 20.2 MP), test album `review-20260929-ui`. RTX 4060 Vulkan was established by the preceding renderer investigation. These are warm editing measurements after initial image opening.

The same instrumentation was built into both baseline and optimized app. It timestamps the accepted edit with `performance.now()` and records the first matching frame after canvas `putImageData` at the next animation-frame callback. It includes throttle, scheduling, preparation, GPU work, byte transport and canvas upload. It does **not** measure monitor scanout, exact physical paint, initial RAW decode, or final settled-quality latency. A later settled frame does not overwrite the first-preview measurement. This is not an INP score.

| Build / interaction | First-preview response |
| --- | ---: |
| Baseline slider, exposure 1.42 → 0.79 | 4425 ms |
| Baseline numeric input, 0.79 → 1.20 | 4351 ms |
| Optimized numeric input, 1.20 → 0.79 | 123 ms |
| Optimized numeric input, 0.79 → 1.20 | 88 ms |
| Optimized slider, 1.20 → 0.79 | 103 ms |
| Optimized longer slider drag, 0.79 → 3.02 | 92 ms |
| Restore original test exposure, 3.02 → 1.42 | 91 ms |
| Final build, same slider change as baseline, 1.42 → 0.79 | 106 ms |
| Final build, restore 0.79 → 1.42 | 98 ms |

The final executable also reopened the persisted 1.42 value correctly. Original/developed comparison and closing/reopening the Develop panel were exercised through the Windows UI. Every measured edit subsequently reached `Preview ready` and `Saved`; screenshots confirmed image and histogram changes. This is a small controlled sample, not a percentile benchmark or a guarantee for every recipe. The two baseline results were approximately 4.4 seconds; the five optimized observations were 88–123ms. These real UI measurements supersede using the earlier isolated 19ms GPU-path result as an estimate of slider responsiveness.

## Validation

- RED: latency API absent; interactive request still 1536px; 48→24→48 cache switching evicted full base; same-size settled request reused a derived draft.
- GREEN: frontend suite 262 tests; Rust suite 205 tests, one optional real-RAW benchmark ignored by default.
- Regressions cover stale frame delivery/transfer, continuous input, first-frame timing retained through refinement, draft/settled sizes, source/geometry/session cache isolation, cancelled cache hits, and settled quality after draft.
- Frontend production build and Windows debug executable build succeed. Clippy succeeds with existing repository warnings (205 for the application target). Full repository rustfmt has pre-existing unrelated failures; the changed Rust file is checked separately.
- No push, deployment, engine dependency update or source-image modification. GUI adjustments are made on the existing copied test RAW and its recipe; its exposure is restored to 1.42.

## Remaining performance limits

Cold image opening still decodes the RAW. Geometry/lens changes can still require full-source preparation. Thumbnail generation still performs a separate decode, but its preparation no longer evicts or locks out warm editor inputs. Multi-window contention, large masks, denoise, exports and other hardware need separate measurements. If these remain slow, the next measured candidates are sharing decoded originals across derivative sessions and tile/region processing; adding more queue depth would not address them.
