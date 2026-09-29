# Vignetting

## Outcome
Dedicated Vignetting section in Lap Develop, matching the supplied Amount/Method layout and offering live non-destructive edge exposure adjustment.

## Scope
Lap lap-9bc owns UI, history, saving and native verification. Engine rapidraw-3dd owns the additive model and shared GPU implementation. Existing Effects vignette remains available and unchanged for old recipes. No lens correction or mask-local changes.

## Decisions
New independent `vignetting` block: enabled=true, amount=0 EV (-4..4), method=ellipticOnCrop. Other methods: circularOnCrop and circular. Ellipse follows both cropped-frame dimensions; crop circle uses half the longer cropped dimension; Circular stays centered on the full oriented frame, reconstructed from normalized crop. Exposure multiplier is 2^(amount * smoothstep(0.25,1,radius)); apply in linear light before tone mapping, after the legacy vignette. Zero/bypass is an exact no-op. The three named methods are behaviorally inspired, not a proprietary pixel match.

Preview/export receive already cropped textures and the same recipe crop, so the shared shader handles both consistently; absolute coordinates also preserve ROI/tile behavior. This adds constant GPU arithmetic and no image decode or extra preview queue. Old vignette controls stay under Effects with an explicit legacy label. Vignetting is included in selective Effects copy/presets and global Reset all; it has its own bypass/reset.

Source: [Capture One 11 user guide, p.247](https://downloads.captureone.pro/cf2a94ed-be82-44d8-b65a-c9bfea606654/English/Capture%20One%2011%20User%20Guide.pdf), documenting crop-relative choices and +/-4 EV. Formula/feather are our implementation choices.

## Prerequisites
- P1 ready: isolated Lap and engine branches and local commit permission in both AGENTS.md files.
- P2 ready: shared GPU pipeline, crop-before-render contract, latest-request-wins scheduler and transactional editor already present.
- P3 ready: Windows MSVC/Rust 1.98, GPU and copied Fuji RAW native test fixture used for Levels.

## Acceptance
- A1 defaults preserve old images; amount/method validate, persist and participate in canonical hashes. Non-finite/out-of-range/unknown methods rejected.
- A2 hardware tests verify neutral/legacy preservation, EV sign, unchanged center, ellipse/circle difference, off-center crop anchoring and ROI parity.
- A3 native panel exposes three methods and Amount, one drag/undo, reset and bypass; existing recipe sections preserved.
- A4 build and relevant gates run with inherited failures distinguished; native RAW interaction and restart persistence verified and test edits restored.

## Delivery
RED then GREEN, engine commit then exact Lap pin/generated contract. Build and launch locally, no publication. No known blockers at authoring; spec.md is the artifact for wdl-spec-check, not a claim of review completion. Record implementation and native evidence in verification.md.
