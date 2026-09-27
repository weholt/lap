import { describe, expect, it } from "vitest";

// Geometry regressions for the develop recipe's oriented coordinate system
// (lap-6bc / TASK-402; managed continuation of lap-002.2).
//
// Governing contracts:
//   - docs/raw-development/schema.md "Geometry — oriented, normalized
//     coordinates": origin at the top-left of the oriented image, crop
//     components in [0, 1] with x + width <= 1 and y + height <= 1.
//   - rapidraw-edit-model src/geometry.rs (crop_to_normalized / crop_to_legacy
//     at engine revision e1035c38aa1150ac350faa3661f26144a922ea91).
//   - rapidraw-develop src/geometry.rs: the quarter-turn + flip composition the
//     renderers apply (rotation first, then horizontal then vertical flips).
//
// The Lap-side conversions must agree with those semantics for EVERY
// orientation combination, so a crop always refers to the same original
// (decoded) pixels no matter how the image is turned or mirrored.

import {
  constrainCropToAspect,
  cropBetweenFrames,
  cropToLegacy,
  cropToNormalized,
  cropToSourceRect,
  exifOrientationState,
  isFullFrameCrop,
  largestAspectCrop,
  orientedDimensions,
  orientedToSourcePixel,
  rotateStepsLeft,
  rotateStepsRight,
  validateCropRect,
  type OrientationState,
} from "@/composables/useDevelopGeometry";
import type { CropRect } from "@/composables/useDevelopSession.types";

const FRAME: CropRect = { x: 0.1, y: 0.2, width: 0.5, height: 0.3 };

function snapEq(a: number, b: number, epsilon = 1e-6): boolean {
  return Math.abs(a - b) <= epsilon;
}

function sameCrop(a: CropRect | null, b: CropRect | null): boolean {
  if (!a || !b) return a === b;
  return (
    snapEq(a.x, b.x) &&
    snapEq(a.y, b.y) &&
    snapEq(a.width, b.width) &&
    snapEq(a.height, b.height)
  );
}

function allStates(): OrientationState[] {
  const states: OrientationState[] = [];
  for (const steps of [0, 1, 2, 3]) {
    for (const flipHorizontal of [false, true]) {
      for (const flipVertical of [false, true]) {
        states.push({ steps, flipHorizontal, flipVertical });
      }
    }
  }
  return states;
}

describe("oriented dimensions and step arithmetic", () => {
  it("swaps width/height only for odd quarter turns (engine oriented_dimensions)", () => {
    expect(orientedDimensions(6000, 4000, 0)).toEqual({
      width: 6000,
      height: 4000,
    });
    expect(orientedDimensions(6000, 4000, 1)).toEqual({
      width: 4000,
      height: 6000,
    });
    expect(orientedDimensions(6000, 4000, 2)).toEqual({
      width: 6000,
      height: 4000,
    });
    expect(orientedDimensions(6000, 4000, 3)).toEqual({
      width: 4000,
      height: 6000,
    });
  });

  it("wraps quarter-turn steps in both directions", () => {
    expect(rotateStepsRight(0)).toBe(1);
    expect(rotateStepsRight(3)).toBe(0);
    expect(rotateStepsLeft(0)).toBe(3);
    expect(rotateStepsLeft(1)).toBe(0);
    expect(rotateStepsLeft(rotateStepsRight(2))).toBe(2);
  });
});

describe("crop rectangle validation (empty/out-of-range rejection)", () => {
  it("accepts in-frame rectangles including the full frame and tiny crops", () => {
    expect(validateCropRect({ x: 0, y: 0, width: 1, height: 1 })).toBeNull();
    expect(validateCropRect(FRAME)).toBeNull();
    expect(
      validateCropRect({ x: 0.999, y: 0.999, width: 0.001, height: 0.001 }),
    ).toBeNull();
  });

  it("rejects null, degenerate, non-finite and out-of-frame crops", () => {
    expect(validateCropRect(null)).not.toBeNull();
    expect(validateCropRect(undefined)).not.toBeNull();
    expect(
      validateCropRect({ x: 0.1, y: 0.1, width: 0, height: 0.5 }),
    ).not.toBeNull();
    expect(
      validateCropRect({ x: 0.1, y: 0.1, width: 0.5, height: -1 }),
    ).not.toBeNull();
    expect(
      validateCropRect({ x: -0.01, y: 0, width: 0.5, height: 0.5 }),
    ).not.toBeNull();
    expect(
      validateCropRect({ x: 0, y: 0, width: 0.5, height: 1.01 }),
    ).not.toBeNull();
    expect(
      validateCropRect({ x: 0.5, y: 0.5, width: 0.6, height: 0.1 }),
    ).not.toBeNull();
    expect(
      validateCropRect({ x: Number.NaN, y: 0, width: 0.5, height: 0.5 }),
    ).not.toBeNull();
    expect(
      validateCropRect({
        x: 0,
        y: 0,
        width: Number.POSITIVE_INFINITY,
        height: 1,
      }),
    ).not.toBeNull();
  });

  it("rejects non-finite and out-of-frame rectangles in normalized-conversion", () => {
    expect(() =>
      cropToNormalized({ x: 1100, y: 0, width: 400, height: 600 }, 1200, 600),
    ).toThrow();
    expect(() =>
      cropToNormalized({ x: 0, y: 0, width: 0, height: 100 }, 1200, 600),
    ).toThrow();
    expect(() =>
      cropToNormalized(
        { x: Number.NaN, y: 0, width: 100, height: 100 },
        1200,
        600,
      ),
    ).toThrow();
    expect(() =>
      cropToNormalized({ x: 0, y: 0, width: 100, height: 100 }, 0, 600),
    ).toThrow();
  });

  it("mirrors the engine crop_to_normalized / crop_to_legacy values", () => {
    const normalized = cropToNormalized(
      { x: 120, y: 60, width: 600, height: 300 },
      1200,
      600,
    );
    expect(normalized).toEqual({ x: 0.1, y: 0.1, width: 0.5, height: 0.5 });
    const legacy = cropToLegacy(normalized, 1200, 600);
    expect(legacy).toEqual({ x: 120, y: 60, width: 600, height: 300 });
  });
});

describe("oriented → original pixel conversions", () => {
  // Reference inverse formulas ported from rapidraw-develop geometry.rs
  // (oriented_to_source_pixel) for the pure quarter-turn part.
  function referenceStepInverse(
    x: number,
    y: number,
    sourceW: number,
    sourceH: number,
    steps: number,
  ): { x: number; y: number } {
    const w = sourceW;
    const h = sourceH;
    switch (steps) {
      case 0:
        return { x, y };
      case 1: // Rotate90 CW: oriented (x, y) came from source (y, h-1-x)
        return { x: y, y: h - 1 - x };
      case 2: // Rotate180
        return { x: w - 1 - x, y: h - 1 - y };
      case 3: // Rotate270: oriented (x, y) came from source (w-1-y, x)
        return { x: w - 1 - y, y: x };
      default:
        throw new Error("bad steps");
    }
  }

  it("maps pixels through the reference quarter-turn formulas", () => {
    for (const steps of [0, 1, 2, 3]) {
      const state: OrientationState = {
        steps,
        flipHorizontal: false,
        flipVertical: false,
      };
      const { width, height } = orientedDimensions(40, 30, steps);
      for (const px of [0, 17, width - 1]) {
        for (const py of [0, 11, height - 1]) {
          const mapped = orientedToSourcePixel(px, py, 40, 30, state);
          const reference = referenceStepInverse(px, py, 40, 30, steps);
          expect(mapped, `steps=${steps} (${px},${py})`).toEqual(reference);
        }
      }
    }
  });

  it("rejects pixels outside the oriented frame", () => {
    const plain: OrientationState = {
      steps: 0,
      flipHorizontal: false,
      flipVertical: false,
    };
    const turned: OrientationState = {
      steps: 1,
      flipHorizontal: false,
      flipVertical: false,
    };
    expect(orientedToSourcePixel(40, 0, 40, 30, plain)).toBeNull();
    expect(orientedToSourcePixel(0, 30, 40, 30, plain)).toBeNull();
    expect(orientedToSourcePixel(30, 40, 40, 30, turned)).toBeNull();
  });

  it("maps corners of a crop to the same source pixels through flips and turns", () => {
    const sourceW = 4000;
    const sourceH = 6000;
    for (const state of allStates()) {
      const rect = cropToSourceRect(FRAME, sourceW, sourceH, state);
      expect(rect, JSON.stringify(state)).not.toBeNull();
      const { width: ow, height: oh } = orientedDimensions(
        sourceW,
        sourceH,
        state.steps,
      );
      const x0 = Math.round(FRAME.x * ow);
      const y0 = Math.round(FRAME.y * oh);
      const x1 = Math.round((FRAME.x + FRAME.width) * ow) - 1;
      const y1 = Math.round((FRAME.y + FRAME.height) * oh) - 1;
      for (const [cx, cy] of [
        [x0, y0],
        [x1, y0],
        [x0, y1],
        [x1, y1],
      ]) {
        const pixel = orientedToSourcePixel(cx, cy, sourceW, sourceH, state);
        expect(
          pixel,
          `${JSON.stringify(state)} corner ${cx},${cy}`,
        ).not.toBeNull();
        expect(pixel!.x).toBeGreaterThanOrEqual(rect!.x);
        expect(pixel!.x).toBeLessThanOrEqual(rect!.x + rect!.width - 1);
        expect(pixel!.y).toBeGreaterThanOrEqual(rect!.y);
        expect(pixel!.y).toBeLessThanOrEqual(rect!.y + rect!.height - 1);
      }
    }
  });

  it("returns the full source for a full-frame crop in every state", () => {
    for (const state of allStates()) {
      const rect = cropToSourceRect(
        { x: 0, y: 0, width: 1, height: 1 },
        123,
        45,
        state,
      );
      expect(rect).toEqual({ x: 0, y: 0, width: 123, height: 45 });
      expect(cropToSourceRect(null, 123, 45, state)).toEqual({
        x: 0,
        y: 0,
        width: 123,
        height: 45,
      });
    }
  });

  it("rejects out-of-frame crops on conversion to source pixels", () => {
    expect(
      cropToSourceRect({ x: 0, y: 0, width: 1.5, height: 1 }, 100, 100, {
        steps: 0,
        flipHorizontal: false,
        flipVertical: false,
      }),
    ).toBeNull();
  });
});

describe("crop invariance across orientation combinations", () => {
  it("keeps the same source pixels when rotating right through all 16 states", () => {
    const sourceW = 2000;
    const sourceH = 3000;
    for (const start of allStates()) {
      const before = cropToSourceRect(FRAME, sourceW, sourceH, start);
      const next: OrientationState = {
        steps: rotateStepsRight(start.steps) as OrientationState["steps"],
        flipHorizontal: start.flipHorizontal,
        flipVertical: start.flipVertical,
      };
      const moved = cropBetweenFrames(FRAME, start, next);
      expect(moved, JSON.stringify({ start, next })).not.toBeNull();
      expect(validateCropRect(moved)).toBeNull();
      const after = cropToSourceRect(moved, sourceW, sourceH, next);
      expect(after).toEqual(before);
    }
  });

  it("keeps the same source pixels when toggling flips in all 16 states", () => {
    const sourceW = 2000;
    const sourceH = 3000;
    for (const start of allStates()) {
      const before = cropToSourceRect(FRAME, sourceW, sourceH, start);
      for (const axis of ["flipHorizontal", "flipVertical"] as const) {
        const next: OrientationState = { ...start, [axis]: !start[axis] };
        const moved = cropBetweenFrames(FRAME, start, next);
        expect(moved, JSON.stringify({ start, axis })).not.toBeNull();
        const after = cropToSourceRect(moved, sourceW, sourceH, next);
        expect(after).toEqual(before);
        // Round trip restores the exact crop.
        expect(sameCrop(cropBetweenFrames(moved!, next, start), FRAME)).toBe(
          true,
        );
      }
    }
  });

  it("round-trips an arbitrary state change exactly", () => {
    for (const from of allStates()) {
      for (const to of allStates()) {
        const there = cropBetweenFrames(FRAME, from, to);
        expect(
          there,
          `${JSON.stringify(from)} -> ${JSON.stringify(to)}`,
        ).not.toBeNull();
        const back = cropBetweenFrames(there!, to, from);
        expect(sameCrop(back, FRAME)).toBe(true);
      }
    }
  });

  it("treats a full-frame crop as invariant and detects full-frame crops", () => {
    const full: CropRect = { x: 0, y: 0, width: 1, height: 1 };
    expect(isFullFrameCrop(full)).toBe(true);
    expect(isFullFrameCrop(null)).toBe(true);
    expect(isFullFrameCrop({ x: 0, y: 0, width: 0.999999, height: 1 })).toBe(
      true,
    );
    expect(isFullFrameCrop(FRAME)).toBe(false);
    for (const to of allStates()) {
      const mapped = cropBetweenFrames(
        full,
        { steps: 0, flipHorizontal: false, flipVertical: false },
        to,
      );
      // null is the canonical representation of the full frame.
      expect(mapped === null || sameCrop(mapped, full)).toBe(true);
    }
  });
});

describe("EXIF orientation mapping", () => {
  // EXIF Orientation 1..8 expressed as quarter turns + flips in the render
  // order the engine applies (rotation first, then horizontal, then vertical).
  const EXPECTED: Record<number, OrientationState> = {
    1: { steps: 0, flipHorizontal: false, flipVertical: false },
    2: { steps: 0, flipHorizontal: true, flipVertical: false },
    3: { steps: 2, flipHorizontal: false, flipVertical: false },
    4: { steps: 0, flipHorizontal: false, flipVertical: true },
    5: { steps: 1, flipHorizontal: true, flipVertical: false }, // Transpose
    6: { steps: 1, flipHorizontal: false, flipVertical: false }, // Rotate 90 CW
    7: { steps: 3, flipHorizontal: true, flipVertical: false }, // Transverse
    8: { steps: 3, flipHorizontal: false, flipVertical: false }, // Rotate 270 CW
  };

  it("maps all eight EXIF orientations to turn/flip states", () => {
    for (const exif of [1, 2, 3, 4, 5, 6, 7, 8]) {
      expect(exifOrientationState(exif)).toEqual(EXPECTED[exif]);
    }
    expect(() => exifOrientationState(0)).toThrow();
    expect(() => exifOrientationState(9)).toThrow();
  });

  it("swaps dimensions exactly for the transposing EXIF orientations", () => {
    for (const exif of [5, 6, 7, 8]) {
      expect(
        orientedDimensions(300, 200, exifOrientationState(exif).steps),
      ).toEqual({
        width: 200,
        height: 300,
      });
    }
    for (const exif of [1, 2, 3, 4]) {
      expect(
        orientedDimensions(300, 200, exifOrientationState(exif).steps),
      ).toEqual({
        width: 300,
        height: 200,
      });
    }
  });

  it("keeps crop conversions consistent for every EXIF orientation", () => {
    const sourceW = 800;
    const sourceH = 600;
    for (const exif of [1, 2, 3, 4, 5, 6, 7, 8]) {
      const state = exifOrientationState(exif);
      const identity: OrientationState = {
        steps: 0,
        flipHorizontal: false,
        flipVertical: false,
      };
      const moved = cropBetweenFrames(FRAME, identity, state);
      const fromIdentity = cropToSourceRect(FRAME, sourceW, sourceH, identity);
      const fromExif = cropToSourceRect(moved, sourceW, sourceH, state);
      expect(fromExif, `exif ${exif}`).toEqual(fromIdentity);
    }
  });
});

describe("aspect locks", () => {
  it("builds the largest centered aspect crop inside the frame", () => {
    // 1000x800 frame: a 2:1 crop spans the full width and 500px height.
    const wide = largestAspectCrop(2, 1000, 800);
    expect(wide).toEqual({ x: 0, y: 0.1875, width: 1, height: 0.625 });
    expect((wide!.width * 1000) / (wide!.height * 800)).toBeCloseTo(2, 6);

    // A 1:2 crop spans the full height and 400px width.
    const tall = largestAspectCrop(0.5, 1000, 800);
    expect(tall).toEqual({ x: 0.3, y: 0, width: 0.4, height: 1 });
    expect((tall!.width * 1000) / (tall!.height * 800)).toBeCloseTo(0.5, 6);

    const square = largestAspectCrop(1, 1000, 800);
    expect(square).toEqual({ x: 0.1, y: 0, width: 0.8, height: 1 });
    expect((square!.width * 1000) / (square!.height * 800)).toBeCloseTo(1, 6);
  });

  it("keeps the crop center and ratio while constraining to an aspect", () => {
    const center = {
      x: FRAME.x + FRAME.width / 2,
      y: FRAME.y + FRAME.height / 2,
    };
    const constrained = constrainCropToAspect(FRAME, 1, 1000, 1000);
    expect(constrained.width / constrained.height).toBeCloseTo(1, 6);
    expect(constrained.x + constrained.width / 2).toBeCloseTo(center.x, 6);
    expect(constrained.y + constrained.height / 2).toBeCloseTo(center.y, 6);
    expect(validateCropRect(constrained)).toBeNull();
  });

  it("clamps constrained crops to stay inside the frame near the edges", () => {
    const corner: CropRect = { x: 0.8, y: 0.8, width: 0.2, height: 0.2 };
    const constrained = constrainCropToAspect(corner, 1, 1000, 1000);
    expect(validateCropRect(constrained)).toBeNull();
    expect(constrained.x + constrained.width).toBeLessThanOrEqual(1);
    expect(constrained.y + constrained.height).toBeLessThanOrEqual(1);
  });
});
