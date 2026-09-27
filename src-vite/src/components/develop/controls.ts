import {
    type CurvePoint,
    type CurveMode,
    type ParametricCurveSettings,
    type Recipe,
    type SectionId,
    DEFAULT_RECIPE,
    RECIPE_PARAM_RANGES,
} from '@/composables/useDevelopSession.types';

// Control descriptor layer for the native Develop panel (lap-adc / TASK-401).
//
// Generated from the authoritative shared model (pinned rapidraw-edit-model
// revision, see docs/raw-development/engine-lock.json):
//   - flat scalar ranges come verbatim from the generated RECIPE_PARAM_RANGES
//     table; nothing here hand-copies numeric semantics;
//   - nested ranges (HSL, color grading, calibration, parametric curve) mirror
//     the model's validated bounds in
//     crates/rapidraw-edit-model/src/validate.rs at that revision. Where the
//     legacy React UI used different visual ranges (e.g. a 0..360 grading
//     hue wheel), the shared model wins.
// LUT and lens-blur parameters are resource/AI dependent and stay outside the
// first-release global recipe (spec section 2); section-null parameters
// (geometry, lens correction) belong to later increments.

export interface ParamDescriptor {
    /** Dotted path into the recipe (e.g. "hsl.reds.hue", "contrast"). */
    path: string;
    range: { min: number; max: number; step: number };
    labelKey: string;
}

export interface ControlGroup {
    id: string;
    section: SectionId;
    labelKey: string;
    params: ParamDescriptor[];
    /**
     * When set, the group's recipe values live under one nested object and a
     * section reset replaces that whole object with its default instead of
     * assigning the dotted param paths individually.
     */
    resetObjectPath?: 'hsl' | 'colorGrading' | 'colorCalibration';
}

export interface DevelopSectionDescriptor {
    id: SectionId;
    /** recipe.sectionVisibility key toggled by the bypass control. */
    visibilityKey: SectionId;
    labelKey: string;
}

// ---------------------------------------------------------------------------
// Sections (canonical engine order)
// ---------------------------------------------------------------------------

export const DEVELOP_SECTIONS: DevelopSectionDescriptor[] = [
    { id: 'basic', visibilityKey: 'basic', labelKey: 'develop.sections.basic' },
    { id: 'curves', visibilityKey: 'curves', labelKey: 'develop.sections.curves' },
    { id: 'color', visibilityKey: 'color', labelKey: 'develop.sections.color' },
    { id: 'details', visibilityKey: 'details', labelKey: 'develop.sections.details' },
    { id: 'effects', visibilityKey: 'effects', labelKey: 'develop.sections.effects' },
];

// ---------------------------------------------------------------------------
// Nested model bounds (rapidraw-edit-model validate.rs)
// ---------------------------------------------------------------------------

const HSL_COMPONENT_RANGE = { min: -100, max: 100, step: 1 } as const;

export const HSL_CHANNELS = [
    'reds',
    'oranges',
    'yellows',
    'greens',
    'aquas',
    'blues',
    'purples',
    'magentas',
] as const;

export const HSL_COMPONENTS = ['hue', 'saturation', 'luminance'] as const;

export const COLOR_GRADING_ZONES = ['global', 'shadows', 'midtones', 'highlights'] as const;

export const COLOR_CALIBRATION_PARAMS: ParamDescriptor[] = [
    { path: 'colorCalibration.shadowsTint', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.shadowsTint' },
    { path: 'colorCalibration.redHue', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.hue' },
    { path: 'colorCalibration.redSaturation', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.saturation' },
    { path: 'colorCalibration.greenHue', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.hue' },
    { path: 'colorCalibration.greenSaturation', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.saturation' },
    { path: 'colorCalibration.blueHue', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.hue' },
    { path: 'colorCalibration.blueSaturation', range: HSL_COMPONENT_RANGE, labelKey: 'develop.calibration.saturation' },
];

/**
 * Point/parametric curve channels (model `Curves`/`ParametricCurve` keys).
 */
export const CURVE_CHANNELS = ['luma', 'red', 'green', 'blue'] as const;

/**
 * Reference slider ranges for the parametric curve parameters (per channel).
 * Bounds match rapidraw-edit-model validate.rs (-100..=100) with the
 * reference presentation split for the black/white levels.
 */
export const PARAMETRIC_CURVE_SLIDERS: Array<{
    key: keyof Pick<
        ParametricCurveSettings,
        'whiteLevel' | 'highlights' | 'lights' | 'darks' | 'shadows' | 'blackLevel'
    >;
    min: number;
    max: number;
}> = [
    { key: 'whiteLevel', min: -100, max: 0 },
    { key: 'highlights', min: -100, max: 100 },
    { key: 'lights', min: -100, max: 100 },
    { key: 'darks', min: -100, max: 100 },
    { key: 'shadows', min: -100, max: 100 },
    { key: 'blackLevel', min: 0, max: 100 },
];

/**
 * Model-validated clamp for a single parametric setting (validate.rs:
 * darks/shadows/lights/highlights/whiteLevel/blackLevel -100..=100,
 * splits 0..=100).
 */
export function clampParametricValue(key: string, value: number): number {
    if (key === 'split1' || key === 'split2' || key === 'split3') {
        return Math.min(100, Math.max(0, value));
    }
    if (key === 'whiteLevel') {
        return Math.min(0, Math.max(-100, value));
    }
    if (key === 'blackLevel') {
        return Math.min(100, Math.max(0, value));
    }
    return Math.min(100, Math.max(-100, value));
}

/**
 * Builds the parametric curve's 7 control points from its settings.
 * Reference math from RapidRAW's curve editor at the pinned source revision
 * (tanh compression, headroom sqrt, split interpolation); the model stores
 * the same points the renderer consumes through recipe.curves.
 */
export function buildParametricPoints(settings: ParametricCurveSettings): CurvePoint[] {
    const vH = settings.highlights / 100;
    const vL = settings.lights / 100;
    const vD = settings.darks / 100;
    const vS = settings.shadows / 100;

    const blackYOffset = settings.blackLevel;
    const whiteYOffset = settings.whiteLevel;

    const s1 = settings.split1 / 100;
    const s2 = settings.split2 / 100;
    const s3 = settings.split3 / 100;

    const xH = (s3 + 1) / 2;
    const xS = s1 / 2;
    const xs = [0, xS, s1, s2, s3, xH, 1];

    const SLIDER_GAIN = 1.2;
    const MAX_DISPLACEMENT = 0.35;

    const response = (v: number, x: number): number => {
        const headroom = v >= 0 ? 1 - x : x;
        const compressedHeadroom = Math.sqrt(headroom);
        const sigmoid = Math.tanh(v * SLIDER_GAIN);
        return sigmoid * MAX_DISPLACEMENT * compressedHeadroom;
    };

    const ys = [
        0,
        xS + response(vS, xS),
        s1 + (response(vS, s1) + response(vD, s1)) / 2,
        s2 + (response(vD, s2) + response(vL, s2)) / 2,
        s3 + (response(vL, s3) + response(vH, s3)) / 2,
        xH + response(vH, xH),
        1,
    ];

    const clamp = (v: number) => Math.max(0, Math.min(1, v));

    const points = xs.map((x, i) => ({
        x: x * 255,
        y: clamp(ys[i]) * 255,
    }));

    if (points.length >= 2) {
        points[0].y = Math.max(0, Math.min(255, points[0].y + blackYOffset));
        const lastIndex = points.length - 1;
        points[lastIndex].y = Math.max(0, Math.min(255, points[lastIndex].y + whiteYOffset));
    }

    return points;
}

// ---------------------------------------------------------------------------
// Tone mapper select (basic section)
// ---------------------------------------------------------------------------

export const BASIC_TONE_MAPPER_CONTROL = {
    path: 'toneMapper',
    labelKey: 'develop.controls.toneMapper',
    options: [
        { value: 'basic' as const, labelKey: 'develop.toneMapper.basic' },
        { value: 'agx' as const, labelKey: 'develop.toneMapper.agx' },
    ],
};

// ---------------------------------------------------------------------------
// Scalar control groups
// ---------------------------------------------------------------------------

function flatParam(path: string, labelKey: string): ParamDescriptor {
    const range = RECIPE_PARAM_RANGES[path];
    if (!range) {
        throw new Error(`develop controls: ${path} is missing from the generated engine table`);
    }
    return { path, range: { min: range.min, max: range.max, step: range.step }, labelKey };
}

function nestedParam(path: string, range: { min: number; max: number; step: number }, labelKey: string): ParamDescriptor {
    return { path, range: { ...range }, labelKey };
}

function hslGroupParams(): ParamDescriptor[] {
    const params: ParamDescriptor[] = [];
    for (const channel of HSL_CHANNELS) {
        for (const component of HSL_COMPONENTS) {
            params.push(
                nestedParam(`hsl.${channel}.${component}`, HSL_COMPONENT_RANGE, `develop.hslComponents.${component}`),
            );
        }
    }
    return params;
}

function gradingGroupParams(): ParamDescriptor[] {
    const params: ParamDescriptor[] = [];
    for (const zone of COLOR_GRADING_ZONES) {
        for (const component of HSL_COMPONENTS) {
            params.push(
                nestedParam(
                    `colorGrading.${zone}.${component}`,
                    HSL_COMPONENT_RANGE,
                    `develop.hslComponents.${component}`,
                ),
            );
        }
    }
    params.push(nestedParam('colorGrading.balance', HSL_COMPONENT_RANGE, 'develop.controls.balance'));
    params.push(nestedParam('colorGrading.blending', { min: 0, max: 100, step: 1 }, 'develop.controls.blending'));
    return params;
}

export const DEVELOP_CONTROL_GROUPS: ControlGroup[] = [
    {
        id: 'tone',
        section: 'basic',
        labelKey: 'develop.groups.tone',
        params: [
            flatParam('exposure', 'develop.exposure'),
            flatParam('brightness', 'develop.controls.brightness'),
            flatParam('contrast', 'develop.controls.contrast'),
            flatParam('highlights', 'develop.controls.highlights'),
            flatParam('shadows', 'develop.controls.shadows'),
            flatParam('whites', 'develop.controls.whites'),
            flatParam('blacks', 'develop.controls.blacks'),
        ],
    },
    {
        id: 'whiteBalance',
        section: 'color',
        labelKey: 'develop.whiteBalance',
        params: [
            flatParam('temperature', 'develop.temperature'),
            flatParam('tint', 'develop.tint'),
        ],
    },
    {
        id: 'color',
        section: 'color',
        labelKey: 'develop.groups.color',
        params: [
            flatParam('vibrance', 'develop.controls.vibrance'),
            flatParam('saturation', 'develop.controls.saturation'),
            flatParam('hue', 'develop.controls.hue'),
        ],
    },
    {
        id: 'hsl',
        section: 'color',
        labelKey: 'develop.groups.hsl',
        resetObjectPath: 'hsl',
        params: hslGroupParams(),
    },
    {
        id: 'colorGrading',
        section: 'color',
        labelKey: 'develop.groups.colorGrading',
        resetObjectPath: 'colorGrading',
        params: gradingGroupParams(),
    },
    {
        id: 'calibration',
        section: 'color',
        labelKey: 'develop.groups.calibration',
        resetObjectPath: 'colorCalibration',
        params: COLOR_CALIBRATION_PARAMS,
    },
    {
        id: 'detail',
        section: 'details',
        labelKey: 'develop.groups.detail',
        params: [
            flatParam('clarity', 'develop.controls.clarity'),
            flatParam('structure', 'develop.controls.structure'),
            flatParam('dehaze', 'develop.controls.dehaze'),
            flatParam('centré', 'develop.controls.center'),
        ],
    },
    {
        id: 'sharpening',
        section: 'details',
        labelKey: 'develop.groups.sharpening',
        params: [
            flatParam('sharpness', 'develop.controls.sharpness'),
            flatParam('sharpnessThreshold', 'develop.controls.sharpnessThreshold'),
            flatParam('lumaNoiseReduction', 'develop.controls.lumaNoiseReduction'),
            flatParam('colorNoiseReduction', 'develop.controls.colorNoiseReduction'),
        ],
    },
    {
        id: 'chromaticAberration',
        section: 'details',
        labelKey: 'develop.groups.chromaticAberration',
        params: [
            flatParam('chromaticAberrationRedCyan', 'develop.controls.chromaticAberrationRedCyan'),
            flatParam('chromaticAberrationBlueYellow', 'develop.controls.chromaticAberrationBlueYellow'),
        ],
    },
    {
        id: 'grain',
        section: 'effects',
        labelKey: 'develop.groups.grain',
        params: [
            flatParam('grainAmount', 'develop.controls.grainAmount'),
            flatParam('grainSize', 'develop.controls.grainSize'),
            flatParam('grainRoughness', 'develop.controls.grainRoughness'),
        ],
    },
    {
        id: 'vignette',
        section: 'effects',
        labelKey: 'develop.groups.vignette',
        params: [
            flatParam('vignetteAmount', 'develop.controls.vignetteAmount'),
            flatParam('vignetteMidpoint', 'develop.controls.vignetteMidpoint'),
            flatParam('vignetteRoundness', 'develop.controls.vignetteRoundness'),
            flatParam('vignetteFeather', 'develop.controls.vignetteFeather'),
        ],
    },
    {
        id: 'light',
        section: 'effects',
        labelKey: 'develop.groups.light',
        params: [
            flatParam('glowAmount', 'develop.controls.glowAmount'),
            flatParam('halationAmount', 'develop.controls.halationAmount'),
            flatParam('flareAmount', 'develop.controls.flareAmount'),
        ],
    },
];

// ---------------------------------------------------------------------------
// Coverage helpers
// ---------------------------------------------------------------------------

/**
 * The generated engine descriptor keys the first-release panel controls,
 * grouped by section. LUT/lens-blur (resource/AI dependent) and section-null
 * parameters (geometry, lens correction) are excluded.
 */
export function inScopeParamKeysBySection(): Record<'basic' | 'color' | 'details' | 'effects', string[]> {
    const result: Record<'basic' | 'color' | 'details' | 'effects', string[]> = {
        basic: [],
        color: [],
        details: [],
        effects: [],
    };
    for (const [key, range] of Object.entries(RECIPE_PARAM_RANGES)) {
        if (!range.section || range.section === 'curves') continue;
        if (key.startsWith('lensBlur') || key === 'lutIntensity') continue;
        result[range.section].push(key);
    }
    for (const keys of Object.values(result)) {
        keys.sort();
    }
    return result;
}

/**
 * Reset patch for one section, built from the same descriptors the controls
 * use (reset/section-reset consistency, spec A9).
 */
export function sectionResetPatch(section: SectionId): Partial<Recipe> {
    if (section === 'curves') {
        return {
            curves: structuredClone(DEFAULT_RECIPE.curves),
            pointCurves: structuredClone(DEFAULT_RECIPE.pointCurves),
            parametricCurve: structuredClone(DEFAULT_RECIPE.parametricCurve),
            curveMode: DEFAULT_RECIPE.curveMode,
        };
    }
    const patch: Record<string, unknown> = {};
    if (section === 'basic') {
        patch.toneMapper = DEFAULT_RECIPE.toneMapper;
    }
    for (const group of DEVELOP_CONTROL_GROUPS) {
        if (group.section !== section) continue;
        if (group.resetObjectPath) {
            patch[group.resetObjectPath] = structuredClone(
                DEFAULT_RECIPE[group.resetObjectPath as keyof Recipe],
            );
            continue;
        }
        for (const param of group.params) {
            patch[param.path] = defaultRecipeValue(param.path);
        }
    }
    return patch as Partial<Recipe>;
}

/** The union of all section resets (spec A9: reset/section-reset consistency). */
export function fullResetPatch(): Partial<Recipe> {
    const patch: Record<string, unknown> = {};
    for (const section of DEVELOP_SECTIONS) {
        Object.assign(patch, sectionResetPatch(section.id));
    }
    return patch as Partial<Recipe>;
}

// ---------------------------------------------------------------------------
// Recipe path helpers
// ---------------------------------------------------------------------------

export function getRecipeValue(recipe: Recipe, path: string): number | undefined {
    let node: unknown = recipe;
    for (const part of path.split('.')) {
        if (node === null || typeof node !== 'object') return undefined;
        node = (node as Record<string, unknown>)[part];
    }
    return typeof node === 'number' ? node : undefined;
}

export function setRecipeValue(recipe: Record<string, unknown>, path: string, value: number): void {
    const parts = path.split('.');
    let node: Record<string, unknown> = recipe;
    for (let i = 0; i < parts.length - 1; i++) {
        const next = node[parts[i]];
        if (next === null || typeof next !== 'object') {
            throw new Error(`develop controls: unknown recipe path ${path}`);
        }
        node = next as Record<string, unknown>;
    }
    node[parts[parts.length - 1]] = value;
}

export function defaultRecipeValue(path: string): number {
    const value = getRecipeValue(DEFAULT_RECIPE, path);
    if (value === undefined) {
        throw new Error(`develop controls: no default for recipe path ${path}`);
    }
    return value;
}

/**
 * Normalizes and validates a point curve for the model (validate.rs):
 * coordinates clamped to [0, 255], first point pinned to x=0, last to x=255,
 * x strictly increasing (duplicate-x interior points are dropped).
 */
export function normalizeCurvePoints(input: CurvePoint[]): CurvePoint[] {
    const finite = input.filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y));
    const clamped = finite
        .map((p) => ({ x: Math.min(255, Math.max(0, p.x)), y: Math.min(255, Math.max(0, p.y)) }))
        .sort((a, b) => a.x - b.x);
    const points: CurvePoint[] = [];
    for (const point of clamped) {
        if (points.length > 0 && point.x <= points[points.length - 1].x) {
            if (points.length === 1 && point.x === 0) {
                // Keep the later of two zero-x points as the first point.
                points[0] = point;
            }
            continue;
        }
        points.push(point);
    }
    if (points.length === 0) {
        return [
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ];
    }
    points[0] = { x: 0, y: points[0].y };
    points[points.length - 1] = { x: 255, y: points[points.length - 1].y };
    if (points.length === 1) {
        points.unshift({ x: 0, y: 0 });
    }
    return points;
}

/**
 * Monotone cubic (Fritsch-Carlson) SVG path for a point curve, ported from
 * the RapidRAW reference curve editor at the pinned source revision.
 * Coordinates are y-inverted for the SVG viewport (255 - y).
 */
export function curveSvgPath(points: CurvePoint[]): string {
    if (points.length < 2) return '';

    const n = points.length;
    const deltas: number[] = [];
    const ms: number[] = [];

    for (let i = 0; i < n - 1; i++) {
        const dx = points[i + 1].x - points[i].x;
        const dy = points[i + 1].y - points[i].y;
        if (dx === 0) {
            deltas.push(dy > 0 ? 1e6 : dy < 0 ? -1e6 : 0);
        } else {
            deltas.push(dy / dx);
        }
    }

    ms.push(deltas[0]);
    for (let i = 1; i < n - 1; i++) {
        if (deltas[i - 1] * deltas[i] <= 0) {
            ms.push(0);
        } else {
            ms.push((deltas[i - 1] + deltas[i]) / 2);
        }
    }
    ms.push(deltas[n - 2]);

    for (let i = 0; i < n - 1; i++) {
        if (deltas[i] === 0) {
            ms[i] = 0;
            ms[i + 1] = 0;
            continue;
        }
        const alpha = ms[i] / deltas[i];
        const beta = ms[i + 1] / deltas[i];
        const tau = alpha * alpha + beta * beta;
        if (tau > 9) {
            const scale = 3.0 / Math.sqrt(tau);
            ms[i] = scale * alpha * deltas[i];
            ms[i + 1] = scale * beta * deltas[i];
        }
    }

    let path = '';
    if (points[0].x > 0) {
        path += `M 0 ${255 - points[0].y} L ${points[0].x} ${255 - points[0].y}`;
    } else {
        path += `M ${points[0].x} ${255 - points[0].y}`;
    }

    for (let i = 0; i < n - 1; i++) {
        const p0 = points[i];
        const p1 = points[i + 1];
        const m0 = ms[i];
        const m1 = ms[i + 1];
        const dx = p1.x - p0.x;
        const cp1x = p0.x + dx / 3.0;
        const cp1y = p0.y + (m0 * dx) / 3.0;
        const cp2x = p1.x - dx / 3.0;
        const cp2y = p1.y - (m1 * dx) / 3.0;
        path += ` C ${cp1x.toFixed(2)} ${255 - Number(cp1y.toFixed(2))}, ${cp2x.toFixed(2)} ${
            255 - Number(cp2y.toFixed(2))
        }, ${p1.x} ${255 - p1.y}`;
    }

    if (points[n - 1].x < 255) {
        path += ` L 255 ${255 - points[n - 1].y}`;
    }
    return path;
}

/** Curve mode type re-export for component props. */
export type { CurveMode };
