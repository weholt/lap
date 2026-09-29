# Adjustment actions — lap-ffb

## Behavior

Copy writes a bounded, versioned JSON adjustment payload to the operating-system
text clipboard on an explicit click. Apply validates that clipboard and updates
the current image as one undo transaction. Levels, Color Balance, Vignetting and
selected section bypass flags travel with tone, curves, color, detail and effects.
Target crop/orientation, masks, lens profiles, decode settings and identity remain
per-image. Older clipboard payloads without the new fields remain accepted.

Edit Selected is a session toggle for 2–256 selected images. Its highlighted button
shows the count; an active notice names the scope. The Develop panel stays visible
during Ctrl/Shift selection. Enabling alone does not change photos. Each completed
gesture copies the current global settings to the captured selection. Apply also
synchronizes targets when the current image already matches the clipboard.

The active preview keeps its existing latest-input scheduler. Background targets
receive one sequential sidecar update each, without RAW decode for the settings
write. Thumbnail refresh uses the existing acknowledged-commit path. The writer
queues completed gestures, never pointer samples, with a 50-gesture upper bound.
Progress and per-image failures are visible; errors turn off Edit Selected. Retry
retries only the latest operation's failed items; newer operations supersede retry.
Navigation waits for accepted writes before opening another session. Undo restores
each successfully updated target's own previous settings, even if Undo was clicked
during a write; redo uses the same target IDs. History remains session-only.

## Persistence and validation

Every target is fingerprinted; replacement media, identity mismatch and revision
conflicts are rejected. Existing target geometry, resources and unsupported data
are retained. Unedited images create their first sidecar at revision 1. Retained
unsaved target edits are protected rather than silently overwritten. Group writes
reject missing LUT resources; machine-local LUTs must be imported as portable resources
before copying. The backend enforces the editing rollback switch.

## Automated verification

RED: the new toolbar test failed because no buttons existed. Clipboard tests
failed on a valid grading hue of 275 and absent bypass state. Persistence tests
failed before implementation, then passed for exact per-target undo, original-byte
and crop preservation, CAS conflicts, new targets and rejection before writing.

Final Windows checks (2026-09-29/30):

- `npm --prefix src-vite test`: 288 passed across 31 files, including clipboard,
  toolbar, per-target undo/redo, completed-gesture propagation, unchanged-active
  Apply, failure/retry protection and the navigation barrier.
- `npm --prefix src-vite run build`: passed.
- `cargo +1.98.0 test --manifest-path src-tauri/Cargo.toml --locked`: 207 passed,
  one optional benchmark ignored.
- `cargo +1.98.0 build --manifest-path src-tauri/Cargo.toml --locked`: passed;
  the rebuilt native executable was launched for the final panel-retention check.
- `cargo +1.98.0 clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked`:
  passed with the existing 205 warnings. Repository-wide rustfmt and TypeScript
  checks remain non-green from inherited issues; TypeScript has six diagnostics
  (down from nine), tracked by lap-372. New Rust module and command block were
  rustfmt formatted; no unrelated mass-formatting changes were made.
- `git diff --check`: passed.

## Native acceptance

Used Computer Use against the native Windows Tauri app and two existing RAW test
copies (Canon CR3 and Fuji RAF), with the original recipes saved as a read-only
comparison snapshot before interaction.

1. Copy Fuji adjustments to the Windows clipboard, select Canon, click Apply:
   Exposure changes from 4.21 to 1.04 and Brightness from 2.52 to -0.94. Sidecar
   comparisons confirm copied curves, grading, Levels, Vignetting and bypass.
   The displayed thumbnail updates. One Undo restores Canon's prior adjustments.
2. Select both images and enable Edit Selected: the button highlights and shows
   the count. Changing Exposure to 1.25 persists the same global settings on both
   images. Undo restores Canon to 4.21 and Fuji to 1.04, each with its own prior
   complete settings. Redo applies 1.25 to both; a second Undo restores them again.
3. Turn Edit Selected off while both remain selected. Drag Canon Exposure to 0.64:
   only Canon is written; Fuji remains at 1.04 with an unchanged revision. Undo
   restores Canon again.
4. Native testing exposed that entering multi-select hid Develop. The toolbar,
   Shift/range and group entry paths now preserve it. After rebuilding/restarting,
   entering toolbar multi-select and clicking both thumbnails keeps Develop open
   and updates the button count from 0 through 1 to 2.
5. All pre-existing recipe fields match the saved comparison after Undo. Canon's
   older sidecar also gains the engine's neutral Levels/Vignetting defaults during
   normal serialization. Revisions advance normally; no sidecar rollback is used.
   The original RAW SHA-256 hashes remain unchanged:
   - Canon: `4775df76433b1e5844bc7a42b95d93efc8438c0a4cd9681e120e23fe3e3007e8`
   - Fuji: `cddd7ae0c43f9280876e5fdcbdf8878718972ebd3d034d8affc8cfc6752d33dc`

The new executable is left running with Develop visible and Edit Selected off.

## Scope

Lap host changes only; the existing pinned engine revision is unchanged. Tested on
Windows. Source metadata and image-local geometry are intentionally not part of the
portable adjustment clipboard. This is not a claim of Capture One file-format
compatibility or cross-platform clipboard qualification.
