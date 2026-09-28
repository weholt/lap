// Lap host contracts for selective recipe copy/paste and reusable presets
// (lap-62b / TASK-405; governing contract docs/raw-development/spec.md A9,
// schema rules in docs/raw-development/schema.md).
//
// A clipboard/preset payload carries ONLY validated, portable recipe values:
//   - the explicitly selected adjustment sections, nothing else;
//   - never source-specific geometry (crop/orientation/transform) or
//     per-source lens data;
//   - never asset identity (assetId/variantId/revisions/fingerprints);
//   - LUT references only as content-addressed `resource://lut/<64-hex>`
//     URIs — machine-local absolute paths are never shared.
//
// The authoritative semantic validation happens in the Rust host when a
// recipe is committed; the parser in useDevelopClipboard.ts is the
// defense-in-depth validation for imported payloads before they can reach a
// recipe.

import type { Recipe, SectionId } from './useDevelopSession.types';

/** Sections of the recipe that can be copied/pasted selectively. */
export type DevelopClipboardSection = SectionId;

/** Envelope `resources` entry shape carried alongside copied LUT references. */
export interface ClipboardResourceRef {
    algorithm: 'sha256';
    digest: string;
    sizeBytes?: number;
}

/**
 * Validated clipboard payload. `values` contains exactly the fields of the
 * selected sections (see SECTION_FIELDS); anything outside them is rejected
 * by the parser, so geometry and identity cannot be smuggled in.
 */
export interface DevelopClipboardPayload {
    kind: 'lap-develop-clipboard';
    schemaVersion: 1;
    sections: DevelopClipboardSection[];
    values: Partial<Recipe>;
    resources?: Record<string, ClipboardResourceRef>;
}

/** A named, reusable validated payload (a user-facing preset). */
export interface DevelopPreset {
    id: string;
    name: string;
    payload: DevelopClipboardPayload;
}
