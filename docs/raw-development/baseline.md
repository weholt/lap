# RAW development baseline gates (TASK-101 / lap-7f5.1)

Established 2026-09-26. Machine-readable companion: `development-manifest.json`.
Governing contract: `spec.md` (SHA256
`49680993969aa54b0265c7671eaa3b172043425bbeb33e69f1855c8293c9284c`, verify with
`certutil -hashfile docs/raw-development/spec.md SHA256`).

Two dedicated checkouts only; `C:/Users/Thomas/Desktop/RapidRAW` stays untouched:

| Repository | Branch | Baseline revision |
| --- | --- | --- |
| `C:/Users/Thomas/Desktop/lap` | `pebbles-harness/raw-development` | `a2d0e01c` (parent `67cda344` = spec-inspected revision) |
| `C:/Users/Thomas/Desktop/RapidRAW-engine` | `feature/lap-engine-extraction` | `5e30bcbb` (identical to spec-inspected revision) |

## Canonical Lap gates

Run from the Lap repository root. Every cargo invocation requires the PATH
prepend below (rustup 1.98 MSVC + VS 2022 cmake; the default PATH resolves cargo
to an incompatible standalone GNU 1.85 toolchain). Never use `--all-features` on
Windows. `scripts\raw_dev_gates.ps1` runs all five with the prepend applied.

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin;$env:PATH"

npm --prefix src-vite run build
npm --prefix src-vite run test        # new: vitest 5.0.2 regression suite (src-vite/tests/)
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

### Results at the baseline revisions

| Gate | Result | Evidence |
| --- | --- | --- |
| `npm --prefix src-vite run build` | pass | built in 4.16s, exit 0 (pre-existing chunk-size warnings only) |
| `npm --prefix src-vite run test` | pass | 2 files / 12 tests, exit 0 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | pass | 20 passed / 0 failed, exit 0 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | **fail (inherited)** | exit 1; pre-existing upstream formatting drift (e.g. `src/t_sqlite.rs`, `src/t_utils.rs`, `src/t_video.rs`) present at `a2d0e01c` before any TASK-101 change |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` | pass | exit 0; 205 warnings / 0 errors, all pre-existing |

Inherited-vs-regression rule for `cargo fmt`: TASK-101 modifies no `.rs` file, so
every fmt diff is upstream drift. Extraction tasks must not add new violations;
reformatting the whole tree is out of scope and stays out of feature diffs.

## Canonical RapidRAW-engine gates

Run from the RapidRAW-engine root with
`$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"` (its `.cargo/config.toml`
guard, recreated per its AGENTS.md, additionally pins `build.rustc`/`rustdoc`):

```powershell
npm test
npm run typecheck
npm run lint
npm run format:check
npm run i18n:check
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

| Gate | Result | Evidence |
| --- | --- | --- |
| `npm test` | pass | 17 files / 80 tests, exit 0 |
| `npm run typecheck` | pass | exit 0 |
| `npm run lint` | **fail (inherited)** | exit 1; 867 errors / 52 warnings — identical counts to the spec's analysis at the same commit |
| `npm run format:check` | **fail (inherited)** | exit 1; 208 files — identical count to the spec's analysis at the same commit |
| `npm run i18n:check` | pass | exit 0 (1395 plural resolutions, 13 locales) |
| `cargo test` | pass | 131 passed / 0 failed across suites, exit 0 |
| `cargo fmt -- --check` | pass | exit 0 |
| `cargo clippy --all-targets` | pass | exit 0; 1 warning / 0 errors |

The stale cached Tauri permission paths that blocked Rust gates in the old
`Workspace/RapidRAW` checkout do not exist in this fresh clone (fresh
`src-tauri/target`); no repair was needed there. Lint/Prettier failures are
inherited at the byte-for-byte same counts as the spec baseline; treat any
increase as a new regression.

## Local dependencies and repairs performed (2026-09-26)

- Lap submodules initialized at their pinned gitlinks: LibRaw `b860248a`,
  libde265 `3cd9fbf1`, libheif `62f1b8c7`, libjpeg-turbo `9217719d`
  (`git submodule update --init`).
- `cmake` is not on the default PATH; Lap's `build.rs` needs it. Repair: prepend
  the VS 2022 Community bundle (`...Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin`,
  cmake 3.30.5-msvc23) — done by the gate script.
- `tauri.conf.json` requires `resources/ffmpeg/*` and `resources/models/*`;
  both directories are gitignored downloads. Repairs run:
  `scripts/download_ffmpeg_sidecar.ps1` and `scripts/download_models.ps1`
  (exact diagnostics without them: `glob pattern resources/ffmpeg/* path not
  found or didn't match any files.`, likewise for `resources/models/*`).
- RapidRAW-engine: recreated the gitignored `.cargo/config.toml` rustc/rustdoc
  pin per its AGENTS.md; ran `npm ci`.

## Harness validation commands

`.agents/pebbles-harness/config.yml` runs `git diff --check` plus the focused
executable gate `npm --prefix src-vite run test`. Cargo gates are intentionally
not harness validation commands: harness shells do not apply the PATH prepend,
so cargo/rustfmt would resolve to the standalone GNU 1.85 toolchain and report
false failures. Use `scripts\raw_dev_gates.ps1` for the full local suite.

## Standing assumptions

- Persistence design (P6, resolved by design per the plan): the authoritative
  first-release store is a writable adjacent `filename.ext.lapedit.json`
  sidecar; a read-only library yields an explicit save failure with the session
  retained for retry or a writable copy. No second recipe authority.
- Build integration (P7, evidence recorded): both hosts compile with rustup
  1.98.0 `x86_64-pc-windows-msvc`, default features only on Windows.
- No RAW fixtures or GPU/parity runs are claimed by this baseline (spec P4/A6
  remain unqualified until real fixtures and hardware runs exist).
