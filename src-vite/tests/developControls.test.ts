import { existsSync, readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// Descriptor/default parity regressions for the develop control layer
// (lap-adc / TASK-401; spec A6/A9/A10).
//
// The Vue panel never hand-copies numeric semantics: flat scalar controls are
// generated from the pinned rapidraw-edit-model contract (RECIPE_PARAM_RANGES,
// DEFAULT_RECIPE). Nested sliders (HSL, color grading, calibration,
// parametric curve) mirror the model's validated bounds in
// crates/rapidraw-edit-model/src/validate.rs of the pinned engine revision
// e1035c38aa1150ac350faa3661f26144a922ea91. LUT and lens-blur controls
// (resource/AI dependent) stay outside the first-release global recipe
// (spec section 2), as do geometry/section-null parameters (separate
// increments).

import {
    CANONICAL_SECTION_ORDER,
    DEFAULT_RECIPE,
    RECIPE_PARAM_RANGES,
    type Recipe,
} from '@/composables/useDevelopSession.types';
import {
    BASIC_TONE_MAPPER_CONTROL,
    COLOR_CALIBRATION_PARAMS,
    CURVE_CHANNELS,
    DEVELOP_CONTROL_GROUPS,
    DEVELOP_SECTIONS,
    HSL_CHANNELS,
    HSL_COMPONENTS,
    PARAMETRIC_CURVE_SLIDERS,
    buildParametricPoints,
    defaultRecipeValue,
    getRecipeValue,
    inScopeParamKeysBySection,
    sectionResetPatch,
} from '@/components/develop/controls';

function allGroupParams() {
    return DEVELOP_CONTROL_GROUPS.flatMap((group) =>
        group.params.map((param) => ({ group, param })),
    );
}

describe('develop control descriptors', () => {
    it('declares exactly the canonical engine sections in engine order', () => {
        expect(DEVELOP_SECTIONS.map((s) => s.id)).toEqual(CANONICAL_SECTION_ORDER);
        expect(DEVELOP_SECTIONS.map((s) => s.visibilityKey)).toEqual(CANONICAL_SECTION_ORDER);
        for (const section of DEVELOP_SECTIONS) {
            expect(typeof section.labelKey).toBe('string');
        }
    });

    it('maps every in-scope generated descriptor to exactly one control group with identical ranges', () => {
        const inScope = inScopeParamKeysBySection();
        for (const section of ['basic', 'color', 'details', 'effects'] as const) {
            const groupKeys = DEVELOP_CONTROL_GROUPS
                .filter((group) => group.section === section)
                .flatMap((group) => group.params.map((param) => param.path))
                .filter((path) => !path.includes('.'));
            expect([...groupKeys].sort()).toEqual([...inScope[section]].sort());
        }

        for (const { param } of allGroupParams()) {
            // Every param path (flat or nested) must resolve in the contract.
            expect(getRecipeValue(DEFAULT_RECIPE, param.path), param.path).toBeDefined();
            if (param.path.includes('.')) continue;
            const range = RECIPE_PARAM_RANGES[param.path];
            expect(range, `param ${param.path} must exist in the generated engine table`).toBeTruthy();
            expect(param.range).toEqual({ min: range.min, max: range.max, step: range.step });
            expect(defaultRecipeValue(param.path)).toBe(
                DEFAULT_RECIPE[param.path as keyof Recipe] as number,
            );
        }
    });

    it('keeps LUT, lens-blur and section-null parameters outside the first-release panel scope', () => {
        const covered = new Set(allGroupParams().map(({ param }) => param.path));
        for (const [key, range] of Object.entries(RECIPE_PARAM_RANGES)) {
            const inScope = Boolean(range.section) && !key.startsWith('lensBlur') && key !== 'lutIntensity';
            expect(covered.has(key), `param ${key}`).toBe(inScope);
        }
    });

    it('bounds nested sliders to the model-validated ranges (validate.rs), not legacy UI guesses', () => {
        const byPath = new Map(allGroupParams().map(({ param }) => [param.path, param]));
        // Grading uses absolute hue degrees through 360; negative legacy hues
        // remain valid. HSL offsets and calibration retain their signed bounds.
        for (const zone of ['global', 'shadows', 'midtones', 'highlights']) {
            for (const component of HSL_COMPONENTS) {
                const param = byPath.get(`colorGrading.${zone}.${component}`);
                expect(param, `colorGrading.${zone}.${component}`).toBeTruthy();
                expect(param!.range).toEqual({ min: -100, max: component === 'hue' ? 360 : 100, step: 1 });
            }
        }
        for (const channel of HSL_CHANNELS) {
            for (const component of HSL_COMPONENTS) {
                const param = byPath.get(`hsl.${channel}.${component}`);
                expect(param, `hsl.${channel}.${component}`).toBeTruthy();
                expect(param!.range).toEqual({ min: -100, max: 100, step: 1 });
            }
        }
        for (const param of COLOR_CALIBRATION_PARAMS) {
            expect(param.range).toEqual({ min: -100, max: 100, step: 1 });
        }
        const blending = byPath.get('colorGrading.blending');
        expect(blending!.range).toEqual({ min: 0, max: 100, step: 1 });
        const balance = byPath.get('colorGrading.balance');
        expect(balance!.range).toEqual({ min: -100, max: 100, step: 1 });
    });

    it('declares the model channels for HSL, grading, curves, calibration and tone mapping', () => {
        expect(HSL_CHANNELS).toEqual([
            'reds', 'oranges', 'yellows', 'greens', 'aquas', 'blues', 'purples', 'magentas',
        ]);
        expect(HSL_COMPONENTS).toEqual(['hue', 'saturation', 'luminance']);
        expect(CURVE_CHANNELS).toEqual(['luma', 'red', 'green', 'blue']);
        expect(COLOR_CALIBRATION_PARAMS.map((param) => param.path)).toEqual([
            'colorCalibration.shadowsTint',
            'colorCalibration.redHue',
            'colorCalibration.redSaturation',
            'colorCalibration.greenHue',
            'colorCalibration.greenSaturation',
            'colorCalibration.blueHue',
            'colorCalibration.blueSaturation',
        ]);
        expect(BASIC_TONE_MAPPER_CONTROL).toMatchObject({ path: 'toneMapper' });
        expect(BASIC_TONE_MAPPER_CONTROL.options.map((option) => option.value)).toEqual([
            'basic',
            'agx',
        ]);
    });

    it('builds section reset patches from the same descriptors (reset/section-reset consistency)', () => {
        const inScope = inScopeParamKeysBySection();

        const basic = sectionResetPatch('basic');
        expect(Object.keys(basic).sort()).toEqual([...inScope.basic, 'toneMapper'].sort());
        expect(basic).toMatchObject({ exposure: 0, contrast: 0, toneMapper: 'basic' });

        const color = sectionResetPatch('color');
        expect(Object.keys(color).sort()).toEqual([
            ...inScope.color,
            'hsl',
            'colorGrading',
            'colorCalibration',
        ].sort());
        expect(color).toMatchObject({
            temperature: DEFAULT_RECIPE.temperature,
            tint: DEFAULT_RECIPE.tint,
            saturation: DEFAULT_RECIPE.saturation,
            hsl: DEFAULT_RECIPE.hsl,
            colorGrading: DEFAULT_RECIPE.colorGrading,
            colorCalibration: DEFAULT_RECIPE.colorCalibration,
        });

        expect(sectionResetPatch('curves')).toEqual({
            curves: DEFAULT_RECIPE.curves,
            pointCurves: DEFAULT_RECIPE.pointCurves,
            parametricCurve: DEFAULT_RECIPE.parametricCurve,
            curveMode: DEFAULT_RECIPE.curveMode,
        });

        const details = sectionResetPatch('details');
        expect(Object.keys(details).sort()).toEqual([...inScope.details].sort());
        expect(details).toMatchObject({ clarity: 0, sharpness: 0, sharpnessThreshold: 15 });

        const effects = sectionResetPatch('effects');
        expect(Object.keys(effects).sort()).toEqual([...inScope.effects].sort());
        expect(effects).toMatchObject({ grainAmount: 0, vignetteAmount: 0, glowAmount: 0 });
    });
});

describe('parametric curve point generation (reference math)', () => {
    it('reproduces the identity diagonal for neutral parametric settings', () => {
        const points = buildParametricPoints(DEFAULT_RECIPE.parametricCurve.luma);
        expect(points).toEqual([
            { x: 0, y: 0 },
            { x: 31.875, y: 31.875 },
            { x: 63.75, y: 63.75 },
            { x: 127.5, y: 127.5 },
            { x: 191.25, y: 191.25 },
            { x: 223.125, y: 223.125 },
            { x: 255, y: 255 },
        ]);
    });

    it('matches the reference displacement formula for an extreme slider value', () => {
        const points = buildParametricPoints({ ...DEFAULT_RECIPE.parametricCurve.luma, darks: 100 });
        // The midtones node averages the darks and lights responses:
        // response(v=1, x=0.5) = tanh(1.2) * 0.35 * sqrt(0.5), lights = 0.
        const response = Math.tanh(1.2) * 0.35 * Math.sqrt(0.5);
        const expectedMid = 0.5 + (response + 0) / 2;
        expect(points[3].x).toBeCloseTo(127.5, 9);
        expect(points[3].y).toBeCloseTo(expectedMid * 255, 6);
    });

    it('applies black/white level offsets and clamps results to [0, 255]', () => {
        const black = buildParametricPoints({ ...DEFAULT_RECIPE.parametricCurve.luma, blackLevel: 20 });
        expect(black[0].y).toBe(20);
        const white = buildParametricPoints({ ...DEFAULT_RECIPE.parametricCurve.luma, whiteLevel: -30 });
        expect(white[6].y).toBe(225);
        const clamped = buildParametricPoints({
            ...DEFAULT_RECIPE.parametricCurve.luma,
            blackLevel: 100,
            whiteLevel: -100,
        });
        for (const point of clamped) {
            expect(point.y).toBeGreaterThanOrEqual(0);
            expect(point.y).toBeLessThanOrEqual(255);
        }
    });

    it('declares the reference slider ranges for the parametric parameters', () => {
        expect(PARAMETRIC_CURVE_SLIDERS).toEqual([
            { key: 'whiteLevel', min: -100, max: 0 },
            { key: 'highlights', min: -100, max: 100 },
            { key: 'lights', min: -100, max: 100 },
            { key: 'darks', min: -100, max: 100 },
            { key: 'shadows', min: -100, max: 100 },
            { key: 'blackLevel', min: 0, max: 100 },
        ]);
    });
});

describe('nested recipe paths', () => {
    it('reads nested values and defaults without name-based guessing', () => {
        expect(getRecipeValue(DEFAULT_RECIPE, 'hsl.reds.hue')).toBe(0);
        expect(getRecipeValue(DEFAULT_RECIPE, 'colorGrading.blending')).toBe(50);
        expect(getRecipeValue(DEFAULT_RECIPE, 'colorCalibration.blueSaturation')).toBe(0);
        expect(defaultRecipeValue('hsl.magentas.luminance')).toBe(0);
        expect(defaultRecipeValue('grainSize')).toBe(25);
        expect(defaultRecipeValue('vignetteMidpoint')).toBe(50);
        expect(defaultRecipeValue('sharpnessThreshold')).toBe(15);
    });
});

describe('frozen fixture preset coverage', () => {
    const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
    const presetsDir = path.join(repoRoot, 'tests/fixtures/raw-development/presets');

    it('exposes every frozen per-adjustment/combined preset parameter with the exact descriptor range', () => {
        // The frozen fixture outputs were captured per-adjustment and in
        // representative combinations at the pinned engine revision
        // (tests/fixtures/raw-development/baselines/capture-manifest.json).
        // The pixel comparisons run in the engine's corpus gate; this Lap-side
        // gate proves the panel exposes exactly that parameter vocabulary with
        // identical numeric semantics, so the same recipes reach the renderer.
        expect(existsSync(presetsDir)).toBe(true);
        const exposed = new Map(allGroupParams().map(({ param }) => [param.path, param]));
        const files = readdirSync(presetsDir).filter(
            (f) => f.endsWith('.json') && f !== 'presets-manifest.json',
        );
        expect(files.length).toBeGreaterThanOrEqual(10);

        const coveredKeys = new Set<string>();
        for (const file of files) {
            const preset = JSON.parse(readFileSync(path.join(presetsDir, file), 'utf8')) as Record<string, number>;
            for (const [key, value] of Object.entries(preset)) {
                coveredKeys.add(key);
                const param = exposed.get(key);
                expect(param, `preset ${file}: ${key} must be panel-exposed`).toBeTruthy();
                expect(value, `preset ${file}: ${key}`).toBeGreaterThanOrEqual(param!.range.min);
                expect(value, `preset ${file}: ${key}`).toBeLessThanOrEqual(param!.range.max);
            }
        }

        // Every per-adjustment kind frozen in the baseline manifest.
        for (const required of [
            'exposure', 'contrast', 'highlights', 'shadows', 'saturation',
            'temperature', 'clarity', 'sharpness',
        ]) {
            expect(coveredKeys.has(required), required).toBe(true);
        }
    });
});
