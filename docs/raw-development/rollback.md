# Develop feature rollback switch

Established 2026-09-28 (lap-63f / TASK-602; managed continuation of
lap-404.2). Governing contract: `spec.md` ("Persistence and compatibility").

## What it is

The rollback switch is a **non-destructive recovery lever** for the new
Develop feature. While enabled:

- Every mutating develop command rejects with the explicit message
  `ROLLBACK_DISABLED_MESSAGE` before any session, sidecar or export write
  (`develop_open_edit_session`, `develop_commit_recipe`,
  `develop_export_developed`, `develop_batch_apply_recipes`,
  `develop_batch_export`, `develop_create_virtual_copy`,
  `develop_reset_variant`, `develop_delete_variant`,
  `develop_import_rrdata`).
- The frontend disables the Develop toolbar entry with the localized notice
  (`develop.rollbackNotice`, all nine locales) and an already-open Develop
  panel collapses to the notice, opens no new session, and renders no
  editing controls (fail-closed while the switch state is unreadable).

## What it never does

- It never deletes, rewrites or "flattens" recipes into the original media.
  Committed sidecars (`*.lapedit.json`), retained previous revisions
  (`*.prev`), virtual-copy sidecars, content-addressed resources and the
  `adevelop_recipes` catalog projection all stay byte-identical while the
  switch is toggled.
- It never writes `.rrdata` or any export artifact.
- It is not a data migration in either direction. Disabling the switch
  resumes editing on the same per-asset/variant CAS revision stream; the
  next commit keeps the retained-previous backup behavior.

## Storage and authority

The flag is a standalone document at `<app-data>/develop-rollback.json`:

```json
{ "developRollback": false }
```

It is deliberately independent of `app-config.json` so config corruption or
the config recovery path can never silently flip the switch. Semantics:

- Missing file → `false` (develop enabled).
- Invalid JSON or a non-boolean `developRollback` value → explicit
  `RollbackError::Corrupt`; the document is preserved on disk and every
  gated command rejects with the readability error (fail closed, never a
  silent reset). Unknown keys are tolerated for forward compatibility and
  never leak into stored documents.
- Writes go through temp-sibling + flush + atomic replace.

## IPC

| Command | Behavior |
| --- | --- |
| `develop_get_rollback` | Returns the persisted switch state. |
| `develop_set_rollback(enabled)` | Persists atomically, returns the stored state. Only this document changes. |

Both commands surface read/write failures as explicit strings.

## Test evidence

`src-tauri/tests/develop_rollback/` (14 cases) covers: flag defaults,
persistence across fresh instances, corrupt/unsupported documents rejected
without reset, unknown-key tolerance, flag isolation, entry-gate message
contract, the full non-destructive scenario (commit → rollback → hash
identical tree → gate rejections → projection retained → disable → editing
resumes at the same revision stream with recoverable backup), and the
migration scenario table (upgrade backup byte-identity, corrupt and
future-schema rejection with preserved payloads, source immutability via
SHA-256 at every step).

Component coverage: `src-vite/tests/DevelopPanel.test.ts` ("shows the
rollback notice...", "fails closed with the rollback notice...") and
`src-vite/src/composables/useDevelopRollback.test.ts`.
