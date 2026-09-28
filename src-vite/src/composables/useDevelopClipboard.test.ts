import { describe, expect, it } from 'vitest';

// Contract tests for selective recipe copy/paste (lap-62b / TASK-405,
// spec A9 + "Persistence and compatibility"): only selected sections move,
// source-specific geometry never travels, asset identity is never shared,
// and imported payloads are validated before use.
//
// These tests RED against the missing useDevelopClipboard module.

import {
    CLIPBOARD_KIND,
    CLIPBOARD_MAX_BYTES,
    DEVELOP_CLIPBOARD_SCHEMA_VERSION,
    PRESET_HOVER_PREVIEW_DELAY_MS,
    copySections,
    parseClipboardPayload,
    pasteSections,
    serializeClipboard,
} from '@/composables/useDevelopClipboard';
import { DEFAULT_RECIPE, type Recipe } from '@/composables/useDevelopSession.types';

const DIGEST = 'a'.repeat(64);

function recipe(): Recipe {
    return structuredClone(DEFAULT_RECIPE);
}

function validPayloadBody(): Record<string, unknown> {
    return {
        kind: CLIPBOARD_KIND,
        schemaVersion: DEVELOP_CLIPBOARD_SCHEMA_VERSION,
        sections: ['basic'],
        values: { exposure: 0.5 },
    };
}

describe('copySections', () => {
    it('copies only the selected sections', () => {
        const source = recipe();
        source.exposure = 0.7;
        source.contrast = 12;
        source.clarity = 30;

        const payload = copySections(source, ['basic']);

        expect(payload.sections).toEqual(['basic']);
        expect(payload.values.exposure).toBe(0.7);
        expect(payload.values.contrast).toBe(12);
        expect(payload.values).not.toHaveProperty('clarity');
    });

    it('never copies source-specific geometry, per-source lens data or masks', () => {
        const source = recipe();
        source.rotation = 90;
        source.orientationSteps = 2;
        source.flipHorizontal = true;
        source.crop = { x: 0.1, y: 0.1, width: 0.5, height: 0.5 };
        source.lensMaker = 'Acme';
        source.lensDistortionParams = {
            k1: 0, k2: 0, k3: 0, model: 0, tca_vr: 1, tca_vb: 1, vig_k1: 0, vig_k2: 0, vig_k3: 0,
        };

        const payload = copySections(source, ['basic', 'curves', 'color', 'details', 'effects']);

        expect(payload.values).not.toHaveProperty('rotation');
        expect(payload.values).not.toHaveProperty('orientationSteps');
        expect(payload.values).not.toHaveProperty('flipHorizontal');
        expect(payload.values).not.toHaveProperty('crop');
        expect(payload.values).not.toHaveProperty('lensMaker');
        expect(payload.values).not.toHaveProperty('lensDistortionParams');
        expect(payload.values).not.toHaveProperty('masks');
        expect(payload.values).not.toHaveProperty('sectionVisibility');
    });

    it('keeps portable resource URIs but strips machine-local LUT paths', () => {
        const source = recipe();
        source.lutPath = 'C:\\luts\\golden.cube';

        const stripped = copySections(source, ['effects']);
        expect(stripped.values.lutPath).toBeUndefined();

        const portable = recipe();
        portable.lutPath = `resource://lut/${DIGEST}`;
        const kept = copySections(portable, ['effects'], {
            resources: { [`lut/${DIGEST}`]: { algorithm: 'sha256', digest: DIGEST, sizeBytes: 10 } },
        });
        expect(kept.values.lutPath).toBe(`resource://lut/${DIGEST}`);
        expect(kept.resources).toEqual({
            [`lut/${DIGEST}`]: { algorithm: 'sha256', digest: DIGEST, sizeBytes: 10 },
        });
    });

    it('carries no asset identity', () => {
        const payload = copySections(recipe(), ['basic']);
        const serialized = serializeClipboard(payload);
        expect(serialized).not.toContain('assetId');
        expect(serialized).not.toContain('variantId');
        expect(serialized).not.toContain('sourceFingerprint');
        expect(serialized).not.toContain('revision');
    });

    it('rejects empty selections and unknown sections', () => {
        expect(() => copySections(recipe(), [])).toThrow(/at least one section/i);
        expect(() => copySections(recipe(), ['masks' as never])).toThrow(/unknown section/i);
    });
});

describe('parseClipboardPayload', () => {
    it('accepts a serialized payload and pastes only the chosen sections', () => {
        const source = recipe();
        source.exposure = 1.2;
        source.temperature = -20;
        source.clarity = 55;
        const payload = copySections(source, ['basic', 'color']);

        const parsed = parseClipboardPayload(serializeClipboard(payload));
        const target = recipe();
        target.clarity = 99;

        const pasted = pasteSections(parsed, target);
        expect(pasted.exposure).toBe(1.2);
        expect(pasted.temperature).toBe(-20);
        expect(pasted.clarity).toBe(99, 'unselected sections stay untouched');
        expect(pasted).toEqual(pasteSections(payload, target));
    });

    it('rejects future schema versions, wrong kinds and invalid JSON', () => {
        const future = JSON.stringify({ ...validPayloadBody(), schemaVersion: 999 });
        expect(() => parseClipboardPayload(future)).toThrow(/newer/i);

        const wrongKind = JSON.stringify({ ...validPayloadBody(), kind: 'something-else' });
        expect(() => parseClipboardPayload(wrongKind)).toThrow(/kind/i);

        expect(() => parseClipboardPayload('{not json')).toThrow(/json/i);
    });

    it('rejects oversized payloads', () => {
        const big = JSON.stringify({
            ...validPayloadBody(),
            values: { exposure: 0, padding: 'x'.repeat(CLIPBOARD_MAX_BYTES) },
        });
        expect(() => parseClipboardPayload(big)).toThrow(/exceeds/i);
    });

    it('rejects smuggled geometry or identity fields', () => {
        const geometry = JSON.stringify({
            ...validPayloadBody(),
            values: { exposure: 0.1, crop: { x: 0, y: 0, width: 1, height: 1 } },
        });
        expect(() => parseClipboardPayload(geometry)).toThrow(/crop/);

        const identity = JSON.stringify({
            ...validPayloadBody(),
            values: { exposure: 0.1, assetId: '42' },
        });
        expect(() => parseClipboardPayload(identity)).toThrow(/assetId/);

        const unknown = JSON.stringify({
            ...validPayloadBody(),
            values: { exposure: 0.1, showClipping: true },
        });
        expect(() => parseClipboardPayload(unknown)).toThrow(/showClipping/);
    });

    it('rejects out-of-bounds, non-finite and wrongly typed values', () => {
        const cases: Array<Record<string, unknown>> = [
            { exposure: 12 },
            { exposure: Number.NaN },
            { toneMapper: 'wild' },
            { lutIsSceneReferred: 'yes' },
            { curves: { luma: [{ x: 0, y: 0 }] } },
            { parametricCurve: { luma: { darks: 5000 } } },
            { colorGrading: { balance: 5000 } },
            { hsl: { reds: { hue: 0, saturation: 9999, luminance: 0 } } },
            { colorCalibration: { redHue: Number.POSITIVE_INFINITY } },
            { lutPath: 'C:\\luts\\x.cube' },
            { lutSize: 99999 },
        ];
        for (const values of cases) {
            const raw = JSON.stringify({ ...validPayloadBody(), values });
            expect(() => parseClipboardPayload(raw), JSON.stringify(values)).toThrow();
        }
    });

    it('validates referenced resource entries', () => {
        const body = { ...validPayloadBody(), sections: ['effects'] };
        const bad = JSON.stringify({
            ...body,
            values: { lutPath: `resource://lut/${DIGEST}` },
            resources: { [`lut/${DIGEST}`]: { algorithm: 'md5', digest: DIGEST } },
        });
        expect(() => parseClipboardPayload(bad)).toThrow(/algorithm/i);

        const shortDigest = JSON.stringify({
            ...body,
            values: { lutPath: `resource://lut/${DIGEST}` },
            resources: { [`lut/${DIGEST}`]: { algorithm: 'sha256', digest: 'ab' } },
        });
        expect(() => parseClipboardPayload(shortDigest)).toThrow(/digest/i);

        const valid = JSON.stringify({
            ...body,
            values: { lutPath: `resource://lut/${DIGEST}` },
            resources: { [`lut/${DIGEST}`]: { algorithm: 'sha256', digest: DIGEST, sizeBytes: 8 } },
        });
        const parsed = parseClipboardPayload(valid);
        expect(parsed.resources?.[`lut/${DIGEST}`]?.digest).toBe(DIGEST);
    });

    it('requires at least one known section and non-empty sections', () => {
        const empty = JSON.stringify({ ...validPayloadBody(), sections: [] });
        expect(() => parseClipboardPayload(empty)).toThrow(/at least one section/i);

        const unknown = JSON.stringify({ ...validPayloadBody(), sections: ['geometry'] });
        expect(() => parseClipboardPayload(unknown)).toThrow(/unknown section/i);
    });
});

describe('useDevelopClipboard composable', () => {
    it('copies, pastes into a target and clears', async () => {
        const { useDevelopClipboard } = await import('@/composables/useDevelopClipboard');
        const clipboard = useDevelopClipboard();
        expect(clipboard.canPaste.value).toBe(false);

        const source = recipe();
        source.exposure = 0.9;
        clipboard.copy(source, ['basic']);
        expect(clipboard.canPaste.value).toBe(true);

        const target = recipe();
        target.exposure = -1;
        const pasted = clipboard.pasteInto(target);
        expect(pasted?.exposure).toBe(0.9);

        clipboard.clear();
        expect(clipboard.canPaste.value).toBe(false);
        expect(clipboard.pasteInto(recipe())).toBeNull();
    });
});

describe('preset contract constants', () => {
    it('exposes the bounded hover preview delay for fake-timer tests', () => {
        expect(PRESET_HOVER_PREVIEW_DELAY_MS).toBeGreaterThan(0);
        expect(PRESET_HOVER_PREVIEW_DELAY_MS).toBeLessThan(5000);
    });
});
