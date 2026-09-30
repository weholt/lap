import { computed, shallowRef, type ComputedRef, type ShallowRef } from 'vue';

import {
    type ClipboardResourceRef,
    type DevelopClipboardPayload,
    type DevelopClipboardSection,
} from './useDevelopClipboard.types';
import {
    CANONICAL_SECTION_ORDER,
    RECIPE_PARAM_RANGES,
    type Recipe,
} from './useDevelopSession.types';

/**
 * Selective recipe copy/paste (lap-62b / TASK-405, spec A9).
 *
 * `copySections` snapshots only the selected adjustment sections into a
 * validated payload; `pasteSections` merges such a payload into a target
 * recipe, touching nothing else. The payload never carries source-specific
 * geometry, per-source lens data, masks, or asset identity, and LUT
 * references travel only as content-addressed `resource://` URIs so recipes
 * stay portable across assets and machines.
 *
 * `parseClipboardPayload` validates imported payloads (bounded size, known
 * sections, no smuggled fields, model bounds) before they are allowed to
 * reach a recipe — imported preset payloads are validated, not trusted.
 *
 * The backend re-validates every committed envelope; this layer exists so
 * invalid or non-portable payloads are rejected at the boundary with an
 * explicit error instead of silently changing a recipe.
 */

export const CLIPBOARD_KIND = 'lap-develop-clipboard';
export const DEVELOP_CLIPBOARD_SCHEMA_VERSION = 1;
/** Bounded payload size for imported clipboard/preset JSON. */
export const CLIPBOARD_MAX_BYTES = 256 * 1024;
/**
 * Hover delay before a preset preview renders (spec A9: hover is transient;
 * previews below this delay never happen at all). Shared contract constant
 * so component tests can drive it with fake timers.
 */
export const PRESET_HOVER_PREVIEW_DELAY_MS = 400;

const RESOURCE_URI_PATTERN = /^resource:\/\/lut\/[0-9a-f]{64}$/;

/** Recipe fields per copyable section (schema.md "Recipe — persisted render
 * data"). Geometry, per-source lens data, lens blur and masks
 * are deliberately absent: they are source-specific or non-portable.
 * Per-section bypass flags travel only for selected sections. */
export const SECTION_FIELDS: Record<
    DevelopClipboardSection,
    readonly (keyof Recipe & string)[]
> = {
    basic: [
        'exposure', 'brightness', 'contrast', 'highlights', 'shadows', 'whites', 'blacks',
        'toneMapper', 'levels',
    ],
    curves: ['curves', 'pointCurves', 'parametricCurve', 'curveMode'],
    color: [
        'temperature', 'tint', 'vibrance', 'saturation', 'hue',
        'colorGrading', 'hsl', 'colorCalibration',
        'blackWhiteEnabled', 'blackWhiteMix',
    ],
    details: [
        'clarity', 'structure', 'dehaze', 'centré', 'sharpness', 'sharpnessThreshold',
        'lumaNoiseReduction', 'colorNoiseReduction',
        'chromaticAberrationRedCyan', 'chromaticAberrationBlueYellow',
    ],
    effects: [
        'glowAmount', 'halationAmount', 'flareAmount',
        'grainAmount', 'grainSize', 'grainRoughness',
        'vignetteAmount', 'vignetteMidpoint', 'vignetteRoundness', 'vignetteFeather', 'vignetting',
        'lutIntensity', 'lutIsSceneReferred', 'lutName', 'lutPath', 'lutSize',
    ],
};

function clone<T>(value: T): T {
    return JSON.parse(JSON.stringify(value)) as T;
}

function canonicalSections(selected: readonly string[]): DevelopClipboardSection[] {
    return CANONICAL_SECTION_ORDER.filter((section) => selected.includes(section));
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Whether a LUT reference is portable (content-addressed URI). */
export function isPortableLutReference(value: unknown): value is string {
    return typeof value === 'string' && RESOURCE_URI_PATTERN.test(value);
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/**
 * Snapshots the selected sections of `recipe` into a validated payload.
 * Absolute LUT paths are stripped (machine-local, never shared); portable
 * resource URIs are kept and their envelope resource entries travel along
 * when the source envelope's `resources` map is supplied.
 */
export function copySections(
    recipe: Recipe,
    sections: readonly DevelopClipboardSection[],
    options: { resources?: Record<string, ClipboardResourceRef> } = {},
): DevelopClipboardPayload {
    const unique = [...new Set(sections)];
    if (unique.length === 0) {
        throw new Error('selective copy requires at least one section');
    }
    for (const section of unique) {
        if (!(section in SECTION_FIELDS)) {
            throw new Error(`unknown section '${String(section)}'`);
        }
    }

    const values: Record<string, unknown> = {};
    for (const field of new Set(unique.flatMap((section) => SECTION_FIELDS[section]))) {
        if (recipe[field] !== undefined) {
            values[field] = clone(recipe[field]);
        }
    }

    values.sectionVisibility = Object.fromEntries(unique.map(section => [section, recipe.sectionVisibility[section]]));

    const resources: Record<string, ClipboardResourceRef> | undefined = isPortableLutReference(
        values.lutPath,
    )
        ? (() => {
            const id = values.lutPath.slice('resource://'.length);
            const entry = options.resources?.[id];
            return entry ? { [id]: clone(entry) } : undefined;
        })()
        : undefined;
    if (values.lutPath !== null && !isPortableLutReference(values.lutPath)) {
        // Machine-local absolute/relative paths must never travel.
        delete values.lutPath;
    }

    const payload: DevelopClipboardPayload = {
        kind: CLIPBOARD_KIND,
        schemaVersion: DEVELOP_CLIPBOARD_SCHEMA_VERSION,
        sections: canonicalSections(unique),
        values: values as Partial<Recipe>,
    };
    if (resources) {
        payload.resources = resources;
    }
    return payload;
}

// ---------------------------------------------------------------------------
// Validation (imported payloads)
// ---------------------------------------------------------------------------

function fail(message: string): never {
    throw new Error(`invalid develop clipboard payload: ${message}`);
}

function checkFiniteNumber(field: string, value: unknown, min: number, max: number): number {
    if (typeof value !== 'number' || !Number.isFinite(value)) {
        fail(`${field} must be a finite number`);
    }
    if (value < min || value > max) {
        fail(`${field} ${value} is outside the model bounds [${min}, ${max}]`);
    }
    return value;
}

function checkFlatParam(field: string, value: unknown): void {
    const range = RECIPE_PARAM_RANGES[field];
    if (!range) {
        fail(`unknown parameter '${field}'`);
    }
    checkFiniteNumber(field, value, range.min, range.max);
}

function checkCurvePoints(field: string, value: unknown): void {
    if (!isPlainObject(value)) fail(`${field} must be an object`);
    for (const channel of ['luma', 'red', 'green', 'blue']) {
        const points = value[channel];
        if (!Array.isArray(points)) fail(`${field}.${channel} must be an array of points`);
        if (points.length < 2 || points.length > 32) {
            fail(`${field}.${channel} must hold 2..32 points`);
        }
        let previousX = -1;
        points.forEach((point: unknown, index: number) => {
            if (!isPlainObject(point)) fail(`${field}.${channel}[${index}] must be a point`);
            const x = checkFiniteNumber(`${field}.${channel}[${index}].x`, point.x, 0, 255);
            checkFiniteNumber(`${field}.${channel}[${index}].y`, point.y, 0, 255);
            if (x <= previousX) fail(`${field}.${channel} x values must strictly increase`);
            previousX = x;
        });
        if (points[0] && (points[0] as Record<string, unknown>).x !== 0) {
            fail(`${field}.${channel} first point must be pinned to x = 0`);
        }
        if (
            points[points.length - 1] &&
            (points[points.length - 1] as Record<string, unknown>).x !== 255
        ) {
            fail(`${field}.${channel} last point must be pinned to x = 255`);
        }
    }
}

function checkParametricCurve(value: unknown): void {
    if (!isPlainObject(value)) fail('parametricCurve must be an object');
    for (const channel of ['luma', 'red', 'green', 'blue']) {
        const settings = value[channel];
        if (!isPlainObject(settings)) fail(`parametricCurve.${channel} must be an object`);
        for (const key of ['darks', 'shadows', 'highlights', 'lights', 'whiteLevel', 'blackLevel']) {
            checkFiniteNumber(`parametricCurve.${channel}.${key}`, settings[key], -100, 100);
        }
        for (const key of ['split1', 'split2', 'split3']) {
            checkFiniteNumber(`parametricCurve.${channel}.${key}`, settings[key], 0, 100);
        }
    }
}

function checkHueSatLum(field: string, value: unknown): void {
    if (!isPlainObject(value)) fail(`${field} must be an object`);
    for (const key of ['hue', 'saturation', 'luminance']) {
        checkFiniteNumber(`${field}.${key}`, value[key], -100, 100);
    }
}

function checkColorGrading(value: unknown): void {
    if (!isPlainObject(value)) fail('colorGrading must be an object');
    checkFiniteNumber('colorGrading.balance', value.balance, -100, 100);
    checkFiniteNumber('colorGrading.blending', value.blending, 0, 100);
    for (const zone of ['global', 'shadows', 'midtones', 'highlights']) {
        const settings = value[zone];
        if (!isPlainObject(settings)) fail(`colorGrading.${zone} must be an object`);
        checkFiniteNumber(`colorGrading.${zone}.hue`, settings.hue, 0, 360);
        checkFiniteNumber(`colorGrading.${zone}.saturation`, settings.saturation, 0, 100);
        checkFiniteNumber(`colorGrading.${zone}.luminance`, settings.luminance, -100, 100);
    }
}

function checkHsl(value: unknown): void {
    if (!isPlainObject(value)) fail('hsl must be an object');
    for (const channel of [
        'reds', 'oranges', 'yellows', 'greens', 'aquas', 'blues', 'purples', 'magentas',
    ]) {
        checkHueSatLum(`hsl.${channel}`, value[channel]);
    }
}

function checkColorCalibration(value: unknown): void {
    if (!isPlainObject(value)) fail('colorCalibration must be an object');
    for (const key of [
        'shadowsTint', 'redHue', 'redSaturation', 'greenHue', 'greenSaturation',
        'blueHue', 'blueSaturation',
    ]) {
        checkFiniteNumber(`colorCalibration.${key}`, value[key], -100, 100);
    }
}

function checkFieldValue(field: string, value: unknown): void {
    if (field === 'blackWhiteEnabled') {
        if (typeof value !== 'boolean') fail('blackWhiteEnabled must be a boolean');
        return;
    }
    if (field === 'blackWhiteMix') {
        if (!Array.isArray(value) || value.length !== 8) fail('blackWhiteMix must contain eight values');
        value.forEach((mix, index) => checkFiniteNumber(`blackWhiteMix[${index}]`, mix, -100, 100));
        return;
    }
    if (field === 'levels') {
        if (!isPlainObject(value) || typeof value.enabled !== 'boolean') fail('invalid Levels');
        for (const channel of ['rgb', 'red', 'green', 'blue']) {
            const v = value[channel];
            if (!isPlainObject(v)) fail(`invalid levels.${channel}`);
            for (const key of ['inputBlack', 'inputWhite', 'outputBlack', 'outputWhite']) checkFiniteNumber(`levels.${channel}.${key}`, v[key], 0, 255);
            checkFiniteNumber(`levels.${channel}.midtone`, v.midtone, -1, 1);
            if (Number(v.inputWhite) - Number(v.inputBlack) < 1) fail('Levels input endpoints must increase');
            if (Number(v.outputWhite) - Number(v.outputBlack) < 1) fail('Levels output endpoints must not decrease');
        }
        return;
    }
    if (field === 'vignetting') {
        if (!isPlainObject(value)) fail('vignetting must be an object');
        checkFiniteNumber('vignetting.amount', value.amount, -4, 4);
        if (typeof value.enabled !== 'boolean') fail('vignetting.enabled must be a boolean');
        if (typeof value.method !== 'string' || !['ellipticOnCrop', 'circularOnCrop', 'circular'].includes(value.method)) fail('invalid vignetting method');
        return;
    }
    if (field === 'toneMapper') {
        if (value !== 'basic' && value !== 'agx') fail(`toneMapper '${String(value)}' is invalid`);
        return;
    }
    if (field === 'curveMode') {
        if (value !== 'point' && value !== 'parametric') {
            fail(`curveMode '${String(value)}' is invalid`);
        }
        return;
    }
    if (field === 'lutIsSceneReferred') {
        if (typeof value !== 'boolean') fail('lutIsSceneReferred must be a boolean');
        return;
    }
    if (field === 'lutName') {
        if (value !== null && (typeof value !== 'string' || value.length === 0 || value.length > 200)) {
            fail('lutName must be null or a string of 1..200 characters');
        }
        return;
    }
    if (field === 'lutPath') {
        if (value !== null && !isPortableLutReference(value)) {
            fail(
                'lutPath must be null or a portable resource://lut/<64-hex> URI; machine-local paths are never shared',
            );
        }
        return;
    }
    if (field === 'lutSize') {
        if (!Number.isInteger(value)) fail('lutSize must be an integer');
        checkFiniteNumber(field, value, 0, 4096);
        return;
    }
    if (field === 'curves' || field === 'pointCurves') {
        checkCurvePoints(field, value);
        return;
    }
    if (field === 'parametricCurve') {
        checkParametricCurve(value);
        return;
    }
    if (field === 'colorGrading') {
        checkColorGrading(value);
        return;
    }
    if (field === 'hsl') {
        checkHsl(value);
        return;
    }
    if (field === 'colorCalibration') {
        checkColorCalibration(value);
        return;
    }
    checkFlatParam(field, value);
}

function checkResourceEntry(field: string, entry: unknown): void {
    if (!isPlainObject(entry)) fail(`${field} must be an object`);
    if (entry.algorithm !== 'sha256') fail(`${field}.algorithm must be 'sha256'`);
    if (typeof entry.digest !== 'string' || !/^[0-9a-f]{64}$/.test(entry.digest)) {
        fail(`${field}.digest must be a 64-hex sha256 digest`);
    }
    if (
        entry.sizeBytes !== undefined &&
        (typeof entry.sizeBytes !== 'number' || !Number.isInteger(entry.sizeBytes) ||
            entry.sizeBytes <= 0 || entry.sizeBytes > 1024 * 1024 * 1024)
    ) {
        fail(`${field}.sizeBytes must be a positive integer within 1 GiB`);
    }
}

/**
 * Parses and validates an imported clipboard/preset payload. Anything but a
 * bounded, current-schema, known-section, smuggle-free payload of in-bounds
 * values is rejected with an explicit error.
 */
export function parseClipboardPayload(
    raw: string | unknown,
    options: { maxBytes?: number } = {},
): DevelopClipboardPayload {
    let value: unknown = raw;
    if (typeof raw === 'string') {
        const maxBytes = options.maxBytes ?? CLIPBOARD_MAX_BYTES;
        const byteLength = new TextEncoder().encode(raw).length;
        if (byteLength > maxBytes) {
            fail(`payload of ${byteLength} bytes exceeds the ${maxBytes} byte bound`);
        }
        try {
            value = JSON.parse(raw);
        } catch {
            fail('payload is not valid JSON');
        }
    }
    if (!isPlainObject(value)) fail('payload must be a JSON object');

    if (value.kind !== CLIPBOARD_KIND) {
        fail(`unknown kind '${String(value.kind)}'`);
    }
    if (value.schemaVersion === undefined) fail('missing schemaVersion');
    if (typeof value.schemaVersion === 'number' && value.schemaVersion > DEVELOP_CLIPBOARD_SCHEMA_VERSION) {
        fail(
            `payload schemaVersion ${value.schemaVersion} is newer than the supported version ${DEVELOP_CLIPBOARD_SCHEMA_VERSION}`,
        );
    }
    if (value.schemaVersion !== DEVELOP_CLIPBOARD_SCHEMA_VERSION) {
        fail(`unsupported schemaVersion ${String(value.schemaVersion)}`);
    }

    if (!Array.isArray(value.sections) || value.sections.length === 0) {
        fail('payload must select at least one section');
    }
    for (const section of value.sections) {
        if (typeof section !== 'string' || !(section in SECTION_FIELDS)) {
            fail(`unknown section '${String(section)}'`);
        }
    }

    if (!isPlainObject(value.values)) fail('values must be an object');
    const allowed = new Set<string>(
        value.sections.flatMap((section: string) => SECTION_FIELDS[section as DevelopClipboardSection]),
    );
    for (const [field, fieldValue] of Object.entries(value.values)) {
        if (field === 'sectionVisibility') {
            if (!isPlainObject(fieldValue)) fail('sectionVisibility must be an object');
            for (const [section, enabled] of Object.entries(fieldValue)) {
                if (!value.sections.includes(section) || typeof enabled !== 'boolean') fail('invalid sectionVisibility');
            }
            continue;
        }
        if (!allowed.has(field)) {
            fail(
                `field '${field}' is not part of the selected sections (source-specific geometry, per-source data and asset identity are never copyable)`,
            );
        }
        checkFieldValue(field, fieldValue);
    }

    let resources: Record<string, ClipboardResourceRef> | undefined;
    if (value.resources !== undefined) {
        if (!isPlainObject(value.resources)) fail('resources must be an object');
        const keys = Object.keys(value.resources);
        if (keys.length > 256) fail('resources must hold at most 256 entries');
        resources = {};
        for (const [id, entry] of Object.entries(value.resources)) {
            if (id.length > 256 || !id.startsWith('lut/')) {
                fail(`resource id '${id}' is not a well-formed lut/ id`);
            }
            checkResourceEntry(`resources.${id}`, entry);
            resources[id] = entry as ClipboardResourceRef;
        }
    }

    const payload: DevelopClipboardPayload = {
        kind: CLIPBOARD_KIND,
        schemaVersion: DEVELOP_CLIPBOARD_SCHEMA_VERSION,
        sections: canonicalSections(value.sections as string[]),
        values: clone(value.values) as Partial<Recipe>,
    };
    if (resources) {
        payload.resources = resources;
    }
    return payload;
}

/** Serializes a payload for transport/storage. */
export function serializeClipboard(payload: DevelopClipboardPayload): string {
    return JSON.stringify(payload);
}

// ---------------------------------------------------------------------------
// Paste
// ---------------------------------------------------------------------------

/** Deep-clones the payload's values as a partial-recipe patch (the exact
 * unit an editor applies as ONE transaction). */
export function pastePatch(payload: DevelopClipboardPayload): Partial<Recipe> {
    return clone(payload.values);
}

/**
 * Merges a validated payload into `target`, changing ONLY the copied
 * sections' fields. Geometry, masks, identity and unselected sections of the
 * target are returned untouched.
 */
export function pasteSections(payload: DevelopClipboardPayload, target: Recipe): Recipe {
    if (payload.kind !== CLIPBOARD_KIND || payload.schemaVersion !== DEVELOP_CLIPBOARD_SCHEMA_VERSION) {
        fail('payload is not a current develop clipboard payload');
    }
    if (!Array.isArray(payload.sections) || payload.sections.length === 0) {
        fail('payload must select at least one section');
    }
    const next = clone(target);
    Object.assign(next as unknown as Record<string, unknown>, pastePatch(payload));
    if (payload.values.sectionVisibility) {
        next.sectionVisibility = { ...target.sectionVisibility, ...payload.values.sectionVisibility };
    }
    return next;
}

// ---------------------------------------------------------------------------
// Composable
// ---------------------------------------------------------------------------

export interface DevelopClipboard {
    clipboard: Readonly<ShallowRef<DevelopClipboardPayload | null>>;
    canPaste: Readonly<ComputedRef<boolean>>;
    copy(
        recipe: Recipe,
        sections: readonly DevelopClipboardSection[],
        options?: { resources?: Record<string, ClipboardResourceRef> },
    ): DevelopClipboardPayload;
    /** Returns the target with the clipboard merged in, or null when empty. */
    pasteInto(target: Recipe): Recipe | null;
    clear(): void;
}

/**
 * Module-instance clipboard for selective copy/paste. The editor applies
 * `pasteInto` results through its one-transaction recipe patch API, so a
 * paste is exactly one undo/redo entry.
 */
export function useDevelopClipboard(): DevelopClipboard {
    const clipboard = shallowRef<DevelopClipboardPayload | null>(null);
    const canPaste = computed(() => clipboard.value !== null);

    function copy(
        recipe: Recipe,
        sections: readonly DevelopClipboardSection[],
        options: { resources?: Record<string, ClipboardResourceRef> } = {},
    ): DevelopClipboardPayload {
        clipboard.value = copySections(recipe, sections, options);
        return clipboard.value;
    }

    function pasteInto(target: Recipe): Recipe | null {
        if (!clipboard.value) return null;
        return pasteSections(clipboard.value, target);
    }

    function clear(): void {
        clipboard.value = null;
    }

    return { clipboard, canPaste, copy, pasteInto, clear };
}
