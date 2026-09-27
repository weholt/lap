# RAW-development release gates and the distribution hold

Established 2026-09-27 (lap-d7f / TASK-103; managed continuation of
lap-7f5.3). Governing contract: `spec.md`; machine-readable inventory:
`provenance.json` (schema `lap-raw-provenance/v1`).

## Status: distribution BLOCKED

The combined-product distribution decision (spec prerequisite **P3**) is
**unresolved**. Local extraction work may continue; **nothing may be
published, distributed, or released**. The release gate below is fail-closed
and currently rejects with exit code 1. That is the correct state.

No agent, harness launch, control run, or environment signal constitutes or
implies distribution approval. Approval exists only when a human authority has
explicitly completed the `distributionDecision` block in `provenance.json`.

## The gate

```powershell
powershell -ExecutionPolicy Bypass -File scripts\raw-development\check-release-gates.ps1
# or directly:
node scripts/raw-development/check-release-gates.mjs
```

Exit codes:

| Code | Verdict    | Meaning |
| ---  | ---------- | ------- |
| 0    | `released` | Every gate passed, including a recorded, evidenced distribution decision. |
| 1    | `blocked`  | Expected today: the decision is absent/unrecorded, provenance is incomplete, notices are missing, or unknowns remain. |
| 2    | `failed`   | `provenance.json` is unreadable or has an unsupported schema. |

Gates, in evaluation order:

1. `schema` — the inventory parses and declares `lap-raw-provenance/v1`.
2. `inventory-completeness` — every required component category
   (RapidRAW AGPL-3.0 source, Lap GPL-3.0-or-later application, extracted
   engine crates, WGSL shaders, rawler fork, film LUTs, lens data, Lap native
   submodules, optional AI models, optional ffmpeg sidecar, optional ONNX
   Runtime) carries a source revision and either identified license evidence
   or an explicit unknown with an explanatory note.
3. `notice-preservation` — every notice file recorded in
   `provenance.json` (`notices[]`) still exists in its repository (Lap
   `LICENSE`, engine `LICENSE`, engine `resources/licenses/*.txt`,
   `SPEKTRAFILM_LICENSE.txt`, submodule license files). A missing notice
   blocks release; `--skip-notice-checks` exists only for fixture tests.
4. `unknowns-resolved` — no component may have a non-identified license and
   `openQuestions` must be empty. Recording an approval does **not** bypass
   this gate: the technical qualification requirements stay in force.
5. `distribution-decision` — fail-closed approval check. `approved` status is
   only recognized when `decidedBy`, `decidedAt`, `scope`, and at least one
   `evidence` entry (`kind` + `reference`) are all present. Anything else
   keeps the verdict `blocked`.

The checker never reads environment variables or harness state; the `env`
option of `evaluateReleaseGates` exists so tests can prove the verdict is
invariant under hostile environment signals.

## How an authorized decision is recorded later

Only after the human licensing decision (P3) is made:

1. Resolve the underlying unknowns first. The `unknowns-resolved` gate cannot
   be switched off: every `openQuestions` entry and every non-identified
   component license must be resolved with real evidence (not by deleting the
   entries). Weakening the technical gates is not a permitted way to record a
   decision.
2. Complete `distributionDecision` in `docs/raw-development/provenance.json`:
   - `status: "approved"`,
   - `decidedBy`: named human/organizational authority,
   - `decidedAt`: ISO-8601 timestamp of the decision,
   - `scope`: what exactly is approved (e.g. "internal distribution only",
     "public release of the combined product"),
   - `evidence`: one or more `{ kind, reference }` entries pointing at the
     decision record (tracker issue, signed memo, license counsel opinion).
3. Re-run the gate; it must exit 0. Record the run in the tracker with the
   same evidence references.
4. Any later change to bundled resources, dependencies, or extraction scope
   requires updating `provenance.json` and re-running the gate; if new
   unknowns appear, the gate blocks again automatically.

A decision recorded without the required fields is treated exactly like a
missing decision: the release stays blocked. This is deliberate — the gate
distinguishes "someone wrote the word approved" from "an evidenced decision
exists".

## Coverage notes (kept honest)

- `provenance.json` inventories the **extraction-relevant** code and bundled
  resources plus Lap's native submodules and machine-local optional artifacts.
  The transitive npm/cargo dependency trees are **not** inventoried
  (`full-dependency-audit` open question; spec delivery step 6).
- Film LUT bytes were verified byte-identical (SHA-256, 2026-09-27) to the
  reference RapidRAW checkout at `5e30bcbb`; the SPEKTRAFILM license explicitly
  keeps the LUTs outside GPLv3 (CC BY-SA 4.0).
- The extracted crates deliberately assert no license field (see
  `crates/rapidraw-edit-model/src/lib.rs`); the inventory records this instead
  of claiming a license.
- Model weights, ffmpeg sidecar binaries, and ONNX Runtime artifacts are
  recorded as unknown-provenance optional dependencies; they are outside the
  initial extraction scope and must not be redistributed while unknown.
