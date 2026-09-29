# Levels

## Outcome
Add a Levels section to Lap Develop, inspired by the supplied Capture One reference, with real GPU adjustments and persistent non-destructive edits.

## Scope
Lap owns UI, gestures, undo/save and native verification (lap-fcd). RapidRAW-engine owns the shared recipe and GPU transform (rapidraw-ae7). Global RGB and individual R/G/B channels; input/output black and white endpoints and midpoint. No mask-local Levels, auto-levels or image pickers in this increment. Existing curves remain independent.

## Decisions
Add optional-on-read recipe.levels with neutral defaults and an independent enabled flag. Four channels have inputBlack/inputWhite/outputBlack/outputWhite in 0..255 (white minus black >=1) and midtone in -1..1 (neutral 0). Positive midpoint brightens: its handle moves left from the center of the input interval, with relative position 0.5 - 0.45 * midtone. GPU normalizes input, applies exponent log(0.5)/log(position) within the interval, extrapolates linearly outside, then maps to output. RGB precedes individual channels, after tone mapping/scene LUT and before existing curves. Output clipping remains the renderer's responsibility. This implements documented interaction, not proprietary Capture One pixel parity.

The histogram represents the current rendered output, labelled accordingly, and samples at most 65536 pixels per completed preview; it does not pretend to expose a pre-Levels intermediate buffer. During a gesture existing latest-request-wins preview scheduling stays authoritative. Endpoint ordering is constrained in the UI and rejected by backend validation. Tabs/expansion are transient; all render settings persist. Additive schema-v1 defaults match existing contract evolution; no downgrade to an older engine is supported for editing Levels recipes.

References: [Capture One Levels overview](https://support.captureone.com/hc/en-us/articles/360002602797-The-Levels-tool-overview), [output levels](https://support.captureone.com/hc/en-us/articles/360002603497-Adjusting-output-levels), [manual adjustment](https://support.captureone.com/hc/en-us/articles/360002603397-Manual-adjustment-of-brightness-and-contrast-in-the-Levels-tool).

## Prerequisites
- P1 ready: isolated Lap and engine branches mandated by AGENTS.md; existing generated contract and pinned local git dependency.
- P2 ready: latest-generation preview scheduling and transaction-aware editor from preceding work.
- P3 ready: Windows GPU, MSVC/Rust 1.98 and installed Lap with copied Canon/Fuji RAW fixtures from earlier verification. Live checks still required for this feature.

## Acceptance
- A1 old recipes load neutral Levels; non-neutral values round-trip, invalidate recipe hashes and survive save/reopen. Invalid/non-finite/crossed endpoints rejected even when bypassed.
- A2 real GPU tests verify neutral bit identity, endpoint mapping/extrapolation, midpoint direction, separate channels, composition and bypass; preview/export use this same path.
- A3 native panel offers four tabs, five draggable keyboard-accessible handles and numeric controls, reset channel/all and independent bypass; no changes to curves or other controls.
- A4 one drag is one undo step; rapid input uses existing scheduler; switching channel/closing section ends transactions; disabled session prevents edits.
- A5 live Windows RAW verification demonstrates endpoint/midpoint/channel change, undo/reset and persistence; restore test edits. Full relevant tests and build pass, distinguish inherited gate failures.

## Delivery
RED regressions precede implementation. Commit engine separately and pin exact revision in Lap; regenerate contracts from authority. Build and launch locally, no publication or original RAW writes. Unresolved blockers: none known at authoring. Specification review artifact: this file (wdl-spec-check); implementation and live readiness must be evidenced separately.
