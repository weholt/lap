import { describe, expect, it } from 'vitest';

// Contract tests for the mask editing model (lap-78d / TASK-501).
//
// The model owns the pure coordinate + payload semantics of native
// brush/linear/radial masks:
//   - mask geometry lives in the oriented, un-cropped normalized frame, so
//     display-space and oriented-space conversions are crop-offset shifts;
//   - UI-created masks carry BOTH the typed `geometry` (authoritative for
//     the engine) and the legacy-shaped `parameters` payload (pixel units)
//     so recipes stay round-trippable with RapidRAW;
//   - stroke payloads stay inside the documented engine limits.

import {
    brushGeometryFromPoints,
    createMaskContainer,
    linearGeometryFromDrag,
    outputToOriented,
    orientedToOutput,
    radialGeometryFromDrag,
    MAX_POINTS_PER_LINE,
    type OrientedPoint,
} from '@/components/develop/masks/maskModel';

describe('mask coordinate mapping', () => {
    it('maps output coordinates to the oriented frame via the crop offset', () => {
        const crop = { x: 0.25, y: 0.125, width: 0.5, height: 0.5 };
        // Normalized units make the mapping a pure offset: output (0,0) is
        // the crop origin in the oriented frame, regardless of scale.
        expect(outputToOriented(0, 0, crop)).toEqual({ x: 0.25, y: 0.125 });
        const mapped = outputToOriented(0.5, 0.5, crop);
        expect(mapped.x).toBeCloseTo(0.75);
        expect(mapped.y).toBeCloseTo(0.625);
    });

    it('is the identity without a crop and round-trips both ways', () => {
        const p: OrientedPoint = { x: 0.3, y: 0.8 };
        expect(outputToOriented(p.x, p.y, null)).toEqual(p);
        const crop = { x: 0.1, y: 0.2, width: 0.7, height: 0.7 };
        const oriented = outputToOriented(p.x, p.y, crop);
        const back = orientedToOutput(oriented.x, oriented.y, crop);
        expect(back.x).toBeCloseTo(p.x);
        expect(back.y).toBeCloseTo(p.y);
    });
});

describe('mask container creation', () => {
    it('creates a visible additive container with typed geometry and legacy payload', () => {
        const mask = createMaskContainer('radial', 'mask-1', 'Radial 1');
        expect(mask.visible).toBe(true);
        expect(mask.invert).toBe(false);
        expect(mask.opacity).toBe(100);
        expect(mask.subMasks).toHaveLength(1);
        const sub = mask.subMasks[0];
        expect(sub.type).toBe('radial');
        expect(sub.mode).toBe('additive');
        expect(sub.geometry).not.toBeNull();
        // The legacy payload is generated from the geometry in pixel units.
        expect(sub.parameters).toMatchObject({ centerX: expect.any(Number) });
    });

    it('brush containers start with an empty stroke list', () => {
        const mask = createMaskContainer('brush', 'mask-2', 'Brush 1');
        expect(mask.subMasks[0].type).toBe('brush');
        const geometry = mask.subMasks[0].geometry;
        expect(geometry).toEqual({ type: 'brush', lines: [] });
        expect((mask.subMasks[0].parameters as { lines: unknown[] }).lines).toEqual([]);
    });
});

describe('gesture geometry builders', () => {
    it('accumulates brush strokes as smoothstep-feathered lines with defaults', () => {
        const geometry = brushGeometryFromPoints(
            { type: 'brush', lines: [] },
            [
                { x: 0.1, y: 0.1 },
                { x: 0.2, y: 0.1 },
            ],
            { brushSize: 0.08, feather: 0.5, tool: 'brush' },
        );
        expect(geometry.type).toBe('brush');
        expect(geometry.lines).toHaveLength(1);
        expect(geometry.lines[0].brushSize).toBeCloseTo(0.08);
        expect(geometry.lines[0].points).toHaveLength(2);
    });

    it('caps brush strokes at the documented engine point budget', () => {
        const points = Array.from({ length: MAX_POINTS_PER_LINE + 500 }, (_, i) => ({
            x: (i % 1000) / 1000,
            y: 0.5,
        }));
        const geometry = brushGeometryFromPoints({ type: 'brush', lines: [] }, points, {
            brushSize: 0.05,
            feather: 0.3,
            tool: 'brush',
        });
        expect(geometry.lines[0].points.length).toBeLessThanOrEqual(MAX_POINTS_PER_LINE);
    });

    it('builds radial geometry from center + drag like the reference tool', () => {
        const gesture = radialGeometryFromDrag({ x: 0.5, y: 0.5 }, { x: 0.6, y: 0.6 });
        expect(gesture.type).toBe('radial');
        if (gesture.type !== 'radial') return;
        expect(gesture.centerX).toBeCloseTo(0.5);
        expect(gesture.centerY).toBeCloseTo(0.5);
        expect(gesture.radiusX).toBeCloseTo(0.1);
        expect(gesture.radiusY).toBeCloseTo(0.1);
        expect(gesture.feather).toBeGreaterThan(0);
    });

    it('builds linear geometry from the perpendicular drag semantics', () => {
        // RapidRAW's initial linear draw: the anchor defines the gradient
        // line through ±perpendicular handles; the drag distance is the
        // falloff range. A downward drag yields a horizontal gradient line
        // through the anchor, spanning ±20% of the minimum oriented
        // dimension, with start/end assigned along the perpendicular vector.
        const gesture = linearGeometryFromDrag({ x: 0.5, y: 0.5 }, { x: 0.5, y: 0.6 }, 1000, 1000);
        expect(gesture.type).toBe('linear');
        if (gesture.type !== 'linear') return;
        expect(gesture.startY).toBeCloseTo(0.5);
        expect(gesture.endY).toBeCloseTo(0.5);
        expect(Math.abs(gesture.endX - gesture.startX)).toBeCloseTo(0.4);
        expect(gesture.range).toBeCloseTo(0.1);
    });
});
