// Pure model for the native non-AI mask tools (lap-78d).
//
// Coordinate contract (docs/raw-development/schema.md, "Mask geometry"):
// mask geometry lives in the oriented, un-cropped normalized frame. Because
// both the processing output and the oriented frame are normalized, the
// mapping between displayed output coordinates and oriented coordinates is a
// pure crop-offset shift — scale-invariant, no dimensions required.
//
// UI-created masks carry BOTH representations:
//   - `geometry`: the typed, validated payload the engine rasterizes;
//   - `parameters`: the legacy pixel payload regenerated from the geometry,
//     keeping recipes comparable with RapidRAW sidecars.
//
// Transient interaction state (active tool, drag points) stays out of the
// recipe entirely; this module only produces recipe-shaped values.

import type {
    BrushLine,
    CropRect,
    MaskContainer,
    MaskGeometry,
    MaskLocalAdjustments,
    MaskPoint,
    SubMask,
} from '@/composables/useDevelopSession.types';

export type MaskToolKind = 'brush' | 'linear' | 'radial';

/** Documented engine limit: maximum points per stroke line. */
export const MAX_POINTS_PER_LINE = 4096;
/** Consecutive stroke points closer than this (normalized) are dropped. */
export const STROKE_POINT_EPSILON = 1e-4;
/** Default brush diameter, as a fraction of the oriented frame width. */
export const DEFAULT_BRUSH_SIZE = 0.06;
/** Default brush feather (smoothstep fraction, [0, 1]). */
export const DEFAULT_BRUSH_FEATHER = 0.5;
/** Gradient/radial feather used when the user has not chosen one. */
export const DEFAULT_GRADIENT_FEATHER = 0.5;

export interface OrientedPoint {
    x: number;
    y: number;
}

/** Display-space point → oriented frame point (pure crop-offset shift). */
export function outputToOriented(x: number, y: number, crop: CropRect | null): OrientedPoint {
    return { x: x + (crop?.x ?? 0), y: y + (crop?.y ?? 0) };
}

/** Oriented frame point → display-space point (inverse mapping). */
export function orientedToOutput(x: number, y: number, crop: CropRect | null): OrientedPoint {
    return { x: x - (crop?.x ?? 0), y: y - (crop?.y ?? 0) };
}

let counter = 0;

/** Collision-free local id; the engine only requires bounded uniqueness. */
export function newMaskId(prefix: string): string {
    counter = (counter + 1) % 1_000_000;
    return `${prefix}-${Date.now().toString(36)}-${counter}`;
}

function clamp01(value: number): number {
    if (!Number.isFinite(value)) return 0;
    return Math.min(1, Math.max(0, value));
}

/** Regenerates the legacy pixel `parameters` payload from typed geometry. */
export function geometryToLegacy(
    geometry: MaskGeometry,
    orientedWidth: number,
    orientedHeight: number,
): Record<string, unknown> {
    const px = (normalized: number) => normalized * orientedWidth;
    const py = (normalized: number) => normalized * orientedHeight;
    switch (geometry.type) {
        case 'brush':
            return {
                lines: geometry.lines.map((line) => ({
                    tool: line.tool,
                    brushSize: px(line.brushSize),
                    feather: line.feather,
                    points: line.points.map((p) => ({ x: px(p.x), y: py(p.y) })),
                })),
            };
        case 'flow':
            return {
                lines: geometry.lines.map((line) => ({
                    tool: line.tool,
                    brushSize: px(line.brushSize),
                    feather: line.feather,
                    flow: line.flow,
                    points: line.points.map((p) => ({ x: px(p.x), y: py(p.y) })),
                })),
            };
        case 'linear':
            return {
                startX: px(geometry.startX),
                startY: py(geometry.startY),
                endX: px(geometry.endX),
                endY: py(geometry.endY),
                range: px(geometry.range),
            };
        case 'radial':
            return {
                centerX: px(geometry.centerX),
                centerY: py(geometry.centerY),
                radiusX: px(geometry.radiusX),
                radiusY: px(geometry.radiusY),
                rotation: geometry.rotation,
                feather: geometry.feather,
            };
        case 'all':
            return {};
    }
}

function subMaskFor(kind: MaskToolKind, id: string, name: string): SubMask {
    let geometry: MaskGeometry;
    switch (kind) {
        case 'brush':
            geometry = { type: 'brush', lines: [] };
            break;
        case 'linear':
            // Neutral in-frame placeholder; the first drag defines the line.
            geometry = { type: 'linear', startX: 0.25, startY: 0.5, endX: 0.75, endY: 0.5, range: 0.1 };
            break;
        case 'radial':
            geometry = {
                type: 'radial',
                centerX: 0.5,
                centerY: 0.5,
                radiusX: 0.1,
                radiusY: 0.1,
                rotation: 0,
                feather: DEFAULT_GRADIENT_FEATHER,
            };
            break;
    }
    return {
        id,
        name,
        invert: false,
        visible: true,
        opacity: 100,
        mode: 'additive',
        type: kind,
        parameters: {},
        geometry,
    };
}

/**
 * Creates a mask container with one sub-mask of `kind`. The legacy
 * `parameters` payload is regenerated from the typed geometry in pixel
 * units of the oriented frame, keeping the recipe comparable with RapidRAW
 * sidecars. `orientedWidth`/`orientedHeight` default to a unit frame for
 * payload-free uses (tests); the panel passes the open session's frame.
 */
export function createMaskContainer(
    kind: MaskToolKind,
    id: string,
    name: string,
    orientedWidth = 1000,
    orientedHeight = 1000,
): MaskContainer {
    const sub = subMaskFor(kind, `${id}-sub`, name);
    if (sub.geometry) {
        sub.parameters = geometryToLegacy(sub.geometry, orientedWidth, orientedHeight);
    }
    return {
        id,
        name,
        invert: false,
        visible: true,
        opacity: 100,
        adjustments: defaultMaskAdjustments(),
        subMasks: [sub],
        unsupported: {},
    };
}

/** Neutral mask-local adjustments (RapidRAW semantics). */
export function defaultMaskAdjustments(): MaskLocalAdjustments {
    return JSON.parse(JSON.stringify(DEFAULT_MASK_ADJUSTMENTS)) as MaskLocalAdjustments;
}

const DEFAULT_MASK_ADJUSTMENTS: MaskLocalAdjustments = {
    exposure: 0,
    brightness: 0,
    contrast: 0,
    highlights: 0,
    shadows: 0,
    whites: 0,
    blacks: 0,
    toneMapper: 'basic',
    temperature: 0,
    tint: 0,
    vibrance: 0,
    saturation: 0,
    hue: 0,
    colorGrading: {
        balance: 0,
        blending: 50,
        global: { hue: 0, saturation: 0, luminance: 0 },
        shadows: { hue: 0, saturation: 0, luminance: 0 },
        midtones: { hue: 0, saturation: 0, luminance: 0 },
        highlights: { hue: 0, saturation: 0, luminance: 0 },
    },
    hsl: Object.fromEntries(
        ['reds', 'oranges', 'yellows', 'greens', 'aquas', 'blues', 'purples', 'magentas'].map(
            (channel) => [channel, { hue: 0, saturation: 0, luminance: 0 }],
        ),
    ) as MaskLocalAdjustments['hsl'],
    colorCalibration: {
        shadowsTint: 0,
        redHue: 0,
        redSaturation: 0,
        greenHue: 0,
        greenSaturation: 0,
        blueHue: 0,
        blueSaturation: 0,
    },
    clarity: 0,
    structure: 0,
    dehaze: 0,
    'centré': 0,
    sharpness: 0,
    sharpnessThreshold: 15,
    lumaNoiseReduction: 0,
    colorNoiseReduction: 0,
    chromaticAberrationRedCyan: 0,
    chromaticAberrationBlueYellow: 0,
    glowAmount: 0,
    halationAmount: 0,
    flareAmount: 0,
    curves: identityCurves(),
    pointCurves: identityCurves(),
    parametricCurve: defaultParametric(),
    curveMode: 'point',
    sectionVisibility: { basic: true, curves: true, color: true, details: true, effects: true },
};

function identityCurves() {
    return {
        luma: [
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ],
        red: [
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ],
        green: [
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ],
        blue: [
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ],
    };
}

function defaultParametric() {
    const zone = { darks: 0, shadows: 0, highlights: 0, lights: 0, whiteLevel: 0, blackLevel: 0, split1: 25, split2: 50, split3: 75 };
    return { luma: { ...zone }, red: { ...zone }, green: { ...zone }, blue: { ...zone } };
}

/**
 * Appends a brush stroke line to a brush geometry, deduplicating consecutive
 * points and capping the count at the documented engine budget.
 */
export function brushGeometryFromPoints(
    _previous: MaskGeometry,
    points: OrientedPoint[],
    options: { brushSize: number; feather: number; tool: 'brush' | 'eraser' },
): Extract<MaskGeometry, { type: 'brush' }> {
    const deduped: MaskPoint[] = [];
    for (const point of points) {
        const last = deduped[deduped.length - 1];
        if (last && Math.abs(last.x - point.x) < STROKE_POINT_EPSILON && Math.abs(last.y - point.y) < STROKE_POINT_EPSILON) {
            continue;
        }
        deduped.push({ x: clamp01(point.x), y: clamp01(point.y) });
        if (deduped.length >= MAX_POINTS_PER_LINE) break;
    }
    const line: BrushLine = {
        tool: options.tool,
        brushSize: Math.max(options.brushSize, 1e-4),
        feather: Math.min(1, Math.max(0, options.feather)),
        points: deduped,
    };
    return { type: 'brush', lines: [line] };
}

/**
 * Radial gesture: the press point is the center; the drag distance sets the
 * radii anisotropically like the reference initial draw (|dx|, |dy|).
 */
export function radialGeometryFromDrag(
    center: OrientedPoint,
    drag: OrientedPoint,
): Extract<MaskGeometry, { type: 'radial' }> {
    const radiusX = Math.max(Math.abs(drag.x - center.x), 1e-3);
    const radiusY = Math.max(Math.abs(drag.y - center.y), 1e-3);
    return {
        type: 'radial',
        centerX: clamp01(center.x),
        centerY: clamp01(center.y),
        radiusX,
        radiusY,
        rotation: 0,
        feather: DEFAULT_GRADIENT_FEATHER,
    };
}

/**
 * Linear gesture with the reference semantics: the drag defines the falloff
 * range (its length) and the gradient line runs perpendicular to the drag
 * through the anchor at ±20% of the minimum oriented dimension. The
 * computation happens in oriented pixels and normalizes the results.
 */
export function linearGeometryFromDrag(
    anchor: OrientedPoint,
    drag: OrientedPoint,
    orientedWidth: number,
    orientedHeight: number,
): Extract<MaskGeometry, { type: 'linear' }> {
    const anchorPx = { x: anchor.x * orientedWidth, y: anchor.y * orientedHeight };
    const dragPx = { x: drag.x * orientedWidth, y: drag.y * orientedHeight };
    const dx = dragPx.x - anchorPx.x;
    const dy = dragPx.y - anchorPx.y;
    const lengthPx = Math.hypot(dx, dy);
    // Falloff range normalized to the oriented frame width.
    const range = Math.max(lengthPx / orientedWidth, 1e-6);
    // Perpendicular of the drag vector, normalized; falls back to a
    // horizontal line for a degenerate (zero-length) drag.
    const nx = lengthPx > 1e-9 ? -dy / lengthPx : 1;
    const ny = lengthPx > 1e-9 ? dx / lengthPx : 0;
    const half = 0.2 * Math.min(orientedWidth, orientedHeight);
    return {
        type: 'linear',
        startX: clamp01((anchorPx.x - nx * half) / orientedWidth),
        startY: clamp01((anchorPx.y - ny * half) / orientedHeight),
        endX: clamp01((anchorPx.x + nx * half) / orientedWidth),
        endY: clamp01((anchorPx.y + ny * half) / orientedHeight),
        range,
    };
}
