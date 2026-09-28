# A10 interactive verification checklist

Established 2026-09-28 (lap-63f / TASK-602; managed continuation of
lap-404.2). Governing contract: `spec.md` A10: "Develop controls work in
Lap's main right panel with central preview, keyboard/numeric input,
localization, original comparison, save status, and explicit derivative
export. Information/selection/duplicate workflows still work. Requires
interactive verification."

Spec A10 requires a human at the real application. This document is the
reproducible checklist plus an honest record of what has actually been
observed. Automated proxies are listed separately and never substitute for
the interactive items.

## How to run

```powershell
# 1. Build the real application (default features only on Windows).
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo build --manifest-path src-tauri/Cargo.toml

# 2. Frontend dev build or packaged frontend (the exe loads src-vite/dist).
npm --prefix src-vite install   # once
npm --prefix src-vite run build

# 3. Launch the application.
.\src-tauri\target\debug\Lap.exe
```

Use a library folder containing the RAW corpus fixtures (see
`docs/raw-development/fixtures.md`). Record observed results and screenshots
in `tests/raw-development/runs/a10-interactive/<date>/` and cross-link them
from the tracker issue.

## Checklist

| # | Step | Expected (spec ref) | Observed |
| --- | --- | --- | --- |
| K1 | Select an asset, open the Develop panel from the main right-panel toolbar | Panel opens beside information/selection/duplicate modes; central filmstrip/preview shows the rendered image (A10) | pending |
| K2 | Drag exposure; then edit the same control via keyboard arrows and by typing a number | Each meaningful action forms one undo transaction; numeric edits commit on Enter; ranges match the generated descriptors (A9/A10) | pending |
| K3 | Switch UI language (all 9 locales) | Every develop label, aria-label, bypass/reset tooltip and save status is localized (A10) | pending |
| K4 | Click "View original" | The central preview and histogram switch to the untouched original; toggling back re-renders the developed state (A10) | pending |
| K5 | Watch the save status during and after an edit | saving → saved appears; killing the sidecar write path shows failed with retry and retains the dirty session (A3/A10) | pending |
| K6 | Export explicitly | Derivative is written only to the chosen destination; source bytes stay identical (A1/A10) | pending |
| K7 | Check information panel, selection mode, duplicate review | All still function with the Develop panel available (A10) | pending |
| K8 | Enable the rollback switch (`develop-rollback.json` → `true`), restart | Develop toolbar entry disabled with rollback notice; panel (if open) shows the notice only; sidecars/resources unchanged (rollback.md) | pending |
| K9 | Disable the rollback switch and reopen Develop | Editing resumes; the earlier recipe and revision stream are intact (rollback.md) | pending |

## Automated proxies actually executed (2026-09-28, lap-63f)

These prove the underlying contracts without a human at the window and are
recorded in the tracker issue with exact commands:

- Keyboard/numeric edit semantics, localization of labels, original
  comparison, save status and retry: `npm --prefix src-vite run test`
  (25 files / 231 tests, including `tests/DevelopPanel.test.ts` which
  mounts the real panel component: keyboard/numeric input, 9-locale
  labels, save status/retry, histogram source switching).
- Information/selection/duplicate modes remain untouched by this task's
  diff (no changes outside develop-scoped files; verified by diff
  inspection).
- Full editing pipeline against real RAW fixtures on the real GPU:
  `tests/raw-development/e2e` scenarios (A1-A7).
- Application lifecycle (process start, window visible, 15 s stability,
  graceful exit): `scripts/raw-development/launch-app-slice.ps1` — proves
  launch only; it explicitly does not drive interactive GUI editing.

## Honest limitation

Until a human executes K1-K9 and records observations/screenshots, spec A10
remains **not fully verified**. The automated evidence above reduces but
does not remove that gap; no agent output may claim otherwise.
