import { computed, type Ref } from "vue";

import { useDevelopEditor } from "./useDevelopEditor";
import { geometryResetPatch } from "@/components/develop/controls";
import type { CropRect, Recipe } from "./useDevelopSession.types";

// Oriented geometry for the develop recipe (lap-6bc / TASK-402).
//
// Single source of truth for Lap's crop/rotate/flip interactions. The
// conversions mirror the pinned engine crates exactly (engine revision
// e1035c38aa1150ac350faa3661f26144a922ea91):
//   - rapidraw-edit-model src/geometry.rs: oriented, normalized crops
//     (components in [0, 1], x + width <= 1, y + height <= 1) and the
//     legacy pixel-crop rescale;
//   - rapidraw-develop src/geometry.rs: the renderers' orientation order —
//     quarter turns first, then the horizontal and the vertical flip.
//
// Semantics chosen for Lap (documented, tested): the crop rectangle follows
// the image content through rotation/flip so a crop always refers to the
// same original (decoded) pixels; a locked aspect ratio is transformed with
// the frame (reciprocal across odd turns) so the lock keeps describing the
// same region. UI-only drag state never enters the recipe.

export interface OrientationState {
  steps: number;
  flipHorizontal: boolean;
  flipVertical: boolean;
}

export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LegacyPixelCrop {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Longest frame edge of the panel crop editor in CSS pixels. */
export const GEOMETRY_FRAME_MAX_EDGE = 200;

/** Minimum crop size as a fraction of the oriented frame (empty-crop guard). */
export const MIN_CROP_SIZE = 0.05;

const SNAP = 1e6;

/** Snaps a geometry component to 1e-6 to keep recipes free of float drift. */
function snap(value: number): number {
  return Math.round(value * SNAP) / SNAP;
}

export function normalizeSteps(steps: number): number {
  return (((Number(steps) || 0) % 4) + 4) % 4;
}

export function rotateStepsRight(steps: number): number {
  return (normalizeSteps(steps) + 1) % 4;
}

export function rotateStepsLeft(steps: number): number {
  return (normalizeSteps(steps) + 3) % 4;
}

/** Dimensions of the oriented image (odd quarter turns swap width/height). */
export function orientedDimensions(
  sourceWidth: number,
  sourceHeight: number,
  steps: number,
): { width: number; height: number } {
  const swap = normalizeSteps(steps) % 2 === 1;
  return swap
    ? { width: sourceHeight, height: sourceWidth }
    : { width: sourceWidth, height: sourceHeight };
}

/**
 * Recipe contract for a crop: finite components, positive size and inside the
 * oriented unit frame. Returns an error message, or null when valid.
 */
export function validateCropRect(
  crop: CropRect | null | undefined,
): string | null {
  if (!crop) return "crop is missing";
  for (const [name, value] of Object.entries(crop) as [string, number][]) {
    if (!Number.isFinite(value)) return `crop.${name} is not finite`;
  }
  if (crop.width <= 0 || crop.height <= 0)
    return "crop.width/height must be > 0";
  if (
    crop.x < 0 ||
    crop.y < 0 ||
    crop.x + crop.width > 1 ||
    crop.y + crop.height > 1
  ) {
    return "crop exceeds the oriented [0,1]x[0,1] frame";
  }
  return null;
}

/** Whether the crop covers the oriented frame (and can be stored as null). */
export function isFullFrameCrop(crop: CropRect | null | undefined): boolean {
  if (!crop) return true;
  return (
    snapEq(crop.x, 0) &&
    snapEq(crop.y, 0) &&
    snapEq(crop.width, 1) &&
    snapEq(crop.height, 1)
  );
}

function snapEq(a: number, b: number): boolean {
  return Math.abs(a - b) <= 1e-5;
}

/** Canonical recipe value for a user-facing crop: null when it is the frame. */
function canonicalCrop(crop: CropRect | null): CropRect | null {
  if (!crop) return null;
  if (isFullFrameCrop(crop)) return null;
  return crop;
}

/**
 * Legacy oriented pixel crop -> normalized recipe rect. Pure rescale, no
 * rotation (engine `crop_to_normalized`); throws on invalid input.
 */
export function cropToNormalized(
  legacy: LegacyPixelCrop,
  orientedWidth: number,
  orientedHeight: number,
): CropRect {
  if (!(orientedWidth > 0) || !(orientedHeight > 0)) {
    throw new Error(
      `invalid oriented dimensions ${orientedWidth}x${orientedHeight}`,
    );
  }
  for (const [name, value] of Object.entries(legacy) as [string, number][]) {
    if (!Number.isFinite(value))
      throw new Error(`legacy crop field ${name} is not finite`);
  }
  if (legacy.width <= 0 || legacy.height <= 0) {
    throw new Error("legacy crop width/height must be positive");
  }
  const crop: CropRect = {
    x: snap(legacy.x / orientedWidth),
    y: snap(legacy.y / orientedHeight),
    width: snap(legacy.width / orientedWidth),
    height: snap(legacy.height / orientedHeight),
  };
  const error = validateCropRect(crop);
  if (error) throw new Error(error);
  return crop;
}

/** Normalized recipe rect -> legacy oriented pixels (engine `crop_to_legacy`). */
export function cropToLegacy(
  crop: CropRect,
  orientedWidth: number,
  orientedHeight: number,
): LegacyPixelCrop {
  const error = validateCropRect(crop);
  if (error) throw new Error(error);
  return {
    x: crop.x * orientedWidth,
    y: crop.y * orientedHeight,
    width: crop.width * orientedWidth,
    height: crop.height * orientedHeight,
  };
}

/**
 * Map one pixel of the oriented frame (after quarter turns AND flips) back to
 * its original decoded pixel. Mirrors the engine `oriented_to_source_pixel`
 * composition: invert the flips in the oriented frame, then invert the turns.
 */
export function orientedToSourcePixel(
  x: number,
  y: number,
  sourceWidth: number,
  sourceHeight: number,
  state: OrientationState,
): { x: number; y: number } | null {
  const steps = normalizeSteps(state.steps);
  const { width, height } = orientedDimensions(
    sourceWidth,
    sourceHeight,
    steps,
  );
  if (x < 0 || y < 0 || x >= width || y >= height) return null;

  let px = x;
  let py = y;
  if (state.flipHorizontal) px = width - 1 - px;
  if (state.flipVertical) py = height - 1 - py;

  switch (steps) {
    case 0:
      return { x: px, y: py };
    case 1:
      return { x: py, y: sourceHeight - 1 - px };
    case 2:
      return { x: sourceWidth - 1 - px, y: sourceHeight - 1 - py };
    case 3:
      return { x: sourceWidth - 1 - py, y: px };
    default:
      return null;
  }
}

/**
 * Convert a normalized oriented-frame crop into a pixel rectangle in the
 * original decoded pixels, valid for every orientation combination. Returns
 * null for invalid crops.
 */
export function cropToSourceRect(
  crop: CropRect | null,
  sourceWidth: number,
  sourceHeight: number,
  state: OrientationState,
): PixelRect | null {
  if (sourceWidth <= 0 || sourceHeight <= 0) return null;
  if (isFullFrameCrop(crop)) {
    return { x: 0, y: 0, width: sourceWidth, height: sourceHeight };
  }
  const error = validateCropRect(crop);
  if (error) return null;

  const steps = normalizeSteps(state.steps);
  const { width: orientedW, height: orientedH } = orientedDimensions(
    sourceWidth,
    sourceHeight,
    steps,
  );
  const x0 = Math.min(Math.round(crop!.x * orientedW), orientedW - 1);
  const y0 = Math.min(Math.round(crop!.y * orientedH), orientedH - 1);
  const x1 = Math.min(
    Math.max(Math.round((crop!.x + crop!.width) * orientedW), 1),
    orientedW,
  );
  const y1 = Math.min(
    Math.max(Math.round((crop!.y + crop!.height) * orientedH), 1),
    orientedH,
  );

  let minX = Number.MAX_SAFE_INTEGER;
  let minY = Number.MAX_SAFE_INTEGER;
  let maxX = -1;
  let maxY = -1;
  for (const [cx, cy] of [
    [x0, y0],
    [x1 - 1, y0],
    [x0, y1 - 1],
    [x1 - 1, y1 - 1],
  ]) {
    const source = orientedToSourcePixel(
      cx,
      cy,
      sourceWidth,
      sourceHeight,
      state,
    );
    if (!source) return null;
    minX = Math.min(minX, source.x);
    minY = Math.min(minY, source.y);
    maxX = Math.max(maxX, source.x);
    maxY = Math.max(maxY, source.y);
  }
  return { x: minX, y: minY, width: maxX - minX + 1, height: maxY - minY + 1 };
}

// Normalized-frame operators (unit square; frame-independent). Rotating the
// IMAGE clockwise by one quarter turn moves a crop to:
//   x' = 1 - y - h, y' = x, w' = h, h' = w
// and a horizontal flip to x' = 1 - x - w.

function rotateCropRight(crop: CropRect): CropRect {
  return snapCrop({
    x: 1 - crop.y - crop.height,
    y: crop.x,
    width: crop.height,
    height: crop.width,
  });
}

function flipCropH(crop: CropRect): CropRect {
  return snapCrop({
    x: 1 - crop.x - crop.width,
    y: crop.y,
    width: crop.width,
    height: crop.height,
  });
}

function flipCropV(crop: CropRect): CropRect {
  return snapCrop({
    x: crop.x,
    y: 1 - crop.y - crop.height,
    width: crop.width,
    height: crop.height,
  });
}

function snapCrop(crop: CropRect): CropRect {
  let x = snap(crop.x);
  let y = snap(crop.y);
  let width = snap(crop.width);
  let height = snap(crop.height);
  if (x < 0) x = 0;
  if (y < 0) y = 0;
  if (x + width > 1) width = snap(1 - x);
  if (y + height > 1) height = snap(1 - y);
  return { x, y, width, height };
}

/**
 * Map a crop between two oriented frames so it covers the same original
 * pixels. Returns null when the input is invalid.
 */
export function cropBetweenFrames(
  crop: CropRect | null,
  from: OrientationState,
  to: OrientationState,
): CropRect | null {
  if (!crop) return null;
  const error = validateCropRect(crop);
  if (error) return null;

  // from-oriented frame -> source. The render applies (turns then flips);
  // the inverse therefore undoes the flips FIRST, then the turns
  // (unit-square ops).
  let current: CropRect = { ...crop };
  if (from.flipVertical) current = flipCropV(current);
  if (from.flipHorizontal) current = flipCropH(current);
  for (let i = 0; i < normalizeSteps(from.steps); i++) {
    for (let r = 0; r < 3; r++) current = rotateCropRight(current);
  }

  // source -> to-oriented frame (render order: turns first, then flips).
  for (let i = 0; i < normalizeSteps(to.steps); i++)
    current = rotateCropRight(current);
  if (to.flipVertical) current = flipCropV(current);
  if (to.flipHorizontal) current = flipCropH(current);

  const resultError = validateCropRect(current);
  if (resultError) return null;
  return canonicalCrop(current);
}

/** EXIF Orientation 1..8 as the turn/flip state the renderers apply. */
export function exifOrientationState(exif: number): OrientationState {
  switch (exif) {
    case 1:
      return { steps: 0, flipHorizontal: false, flipVertical: false };
    case 2:
      return { steps: 0, flipHorizontal: true, flipVertical: false };
    case 3:
      return { steps: 2, flipHorizontal: false, flipVertical: false };
    case 4:
      return { steps: 0, flipHorizontal: false, flipVertical: true };
    case 5:
      return { steps: 1, flipHorizontal: true, flipVertical: false };
    case 6:
      return { steps: 1, flipHorizontal: false, flipVertical: false };
    case 7:
      return { steps: 3, flipHorizontal: true, flipVertical: false };
    case 8:
      return { steps: 3, flipHorizontal: false, flipVertical: false };
    default:
      throw new Error(`unsupported EXIF orientation ${exif}`);
  }
}

/** The largest crop with the given pixel width/height ratio inside the frame. */
export function largestAspectCrop(
  aspect: number,
  orientedWidth: number,
  orientedHeight: number,
): CropRect | null {
  if (!(aspect > 0) || orientedWidth <= 0 || orientedHeight <= 0) return null;
  // Crops are frame fractions; the aspect is a pixel ratio.
  const fractionRatio = aspect / (orientedWidth / orientedHeight);
  let width: number;
  let height: number;
  if (fractionRatio >= 1) {
    width = 1;
    height = 1 / fractionRatio;
  } else {
    height = 1;
    width = fractionRatio;
  }
  return snapCrop({
    x: (1 - width) / 2,
    y: (1 - height) / 2,
    width,
    height,
  });
}

/**
 * Re-fit a crop to an aspect ratio (pixel ratio) around its current center,
 * never growing beyond its current extent, clamped into the frame
 * (aspect-lock select behavior).
 */
export function constrainCropToAspect(
  crop: CropRect,
  aspect: number,
  orientedWidth: number,
  orientedHeight: number,
): CropRect | null {
  if (!(aspect > 0)) return null;
  const error = validateCropRect(crop);
  if (error) return null;

  const fractionRatio = aspect / (orientedWidth / orientedHeight);
  let width: number;
  let height: number;
  if (fractionRatio >= 1) {
    width = 1;
    height = 1 / fractionRatio;
  } else {
    height = 1;
    width = fractionRatio;
  }

  const scale = Math.min(1, crop.width / width, crop.height / height);
  width *= scale;
  height *= scale;

  const centerX = crop.x + crop.width / 2;
  const centerY = crop.y + crop.height / 2;
  const x = Math.min(1 - width, Math.max(0, centerX - width / 2));
  const y = Math.min(1 - height, Math.max(0, centerY - height / 2));
  const fitted = snapCrop({ x, y, width, height });
  const fittedError = validateCropRect(fitted);
  return fittedError ? null : fitted;
}

// ---------------------------------------------------------------------------
// Crop-editor gesture math (frame fractions; used by GeometryControls).
// ---------------------------------------------------------------------------

export type CropGestureMode =
  | "move"
  | "nw"
  | "n"
  | "ne"
  | "e"
  | "se"
  | "s"
  | "sw"
  | "w";

/**
 * Applies one pointer/keyboard gesture delta (in frame fractions) to a start
 * crop and returns the clamped, always-valid result: the rectangle never
 * leaves the oriented frame and never degenerates below MIN_CROP_SIZE
 * (empty/out-of-range crops are rejected by construction). `start` may be
 * null, meaning the full frame.
 */
export function clampCropPointerGesture(
  start: CropRect | null,
  mode: CropGestureMode,
  dx: number,
  dy: number,
  aspect: number | null,
  orientedWidth: number,
  orientedHeight: number,
): CropRect | null {
  const base: CropRect = start ?? { x: 0, y: 0, width: 1, height: 1 };
  if (validateCropRect(base) && !isFullFrameCrop(base)) return null;
  if (orientedWidth <= 0 || orientedHeight <= 0) return null;

  let x = base.x;
  let y = base.y;
  let width = base.width;
  let height = base.height;

  switch (mode) {
    case "move":
      x += dx;
      y += dy;
      break;
    case "e":
      width += dx;
      break;
    case "s":
      height += dy;
      break;
    case "se":
      width += dx;
      height += dy;
      break;
    case "w":
      x += dx;
      width -= dx;
      break;
    case "n":
      y += dy;
      height -= dy;
      break;
    case "sw":
      x += dx;
      width -= dx;
      height += dy;
      break;
    case "nw":
      x += dx;
      width -= dx;
      y += dy;
      height -= dy;
      break;
    case "ne":
      width += dx;
      y += dy;
      height -= dy;
      break;
  }

  if (mode === "move") {
    // Moving slides the rectangle; its size never changes.
    x = Math.min(Math.max(x, 0), 1 - width);
    y = Math.min(Math.max(y, 0), 1 - height);
  } else {
    // Resizing keeps the anchor corner: the position is clamped into the
    // frame and the size is limited by the remaining frame span, so the
    // rectangle never leaves the frame and never degenerates below
    // MIN_CROP_SIZE (empty/out-of-range crops are rejected by design).
    x = Math.min(Math.max(x, 0), 1 - MIN_CROP_SIZE);
    y = Math.min(Math.max(y, 0), 1 - MIN_CROP_SIZE);
    width = Math.max(MIN_CROP_SIZE, Math.min(width, 1 - x));
    height = Math.max(MIN_CROP_SIZE, Math.min(height, 1 - y));
  }

  let fitted = snapCrop({ x, y, width, height });
  if (aspect && aspect > 0) {
    const constrained = constrainCropToAspect(
      fitted,
      aspect,
      orientedWidth,
      orientedHeight,
    );
    if (constrained) fitted = constrained;
  }
  const error = validateCropRect(fitted);
  return error ? null : fitted;
}

/** Percentage-box style for a crop inside the oriented frame element. */
export function cropBoxStyle(crop: CropRect): Record<string, string> {
  return {
    left: `${crop.x * 100}%`,
    top: `${crop.y * 100}%`,
    width: `${crop.width * 100}%`,
    height: `${crop.height * 100}%`,
  };
}

/** Fixed aspect choices for the select (labels are universal ratio text). */
export function frameAspectOptions(): Array<{ value: string; label: string }> {
  const ratios: Array<[string, number]> = [
    ["1:1", 1],
    ["4:3", 4 / 3],
    ["3:2", 3 / 2],
    ["16:9", 16 / 9],
    ["16:10", 16 / 10],
    ["2:1", 2],
    ["3:4", 3 / 4],
    ["2:3", 2 / 3],
    ["9:16", 9 / 16],
    ["10:16", 10 / 16],
    ["1:2", 1 / 2],
  ];
  return ratios.map(([label, ratio]) => ({
    value: String(Math.round(ratio * 10000) / 10000),
    label,
  }));
}

// ---------------------------------------------------------------------------
// Composable: geometry state and recipe-backed actions over useDevelopEditor.
// ---------------------------------------------------------------------------

export interface DevelopGeometry {
  sourceDimensions: Ref<{ width: number; height: number }>;
  orientationSteps: Ref<number>;
  flipHorizontal: Ref<boolean>;
  flipVertical: Ref<boolean>;
  crop: Ref<CropRect | null>;
  aspectRatio: Ref<number | null>;
  oriented: Ref<{ width: number; height: number }>;
  frameSize: Ref<{ width: number; height: number }>;
  rotateLeft(): void;
  rotateRight(): void;
  toggleFlipHorizontal(): void;
  toggleFlipVertical(): void;
  setAspect(aspect: number | null): void;
  setCropLive(crop: CropRect | null): boolean;
  setCrop(crop: CropRect | null): boolean;
  beginCropGesture(label?: string): void;
  endCropGesture(): void;
  snapshot(): Recipe | null;
  cancelCropGesture(snapshot: Recipe): void;
  resetGeometry(): void;
}

export function useDevelopGeometry(): DevelopGeometry {
  const develop = useDevelopEditor();

  const sourceDimensions = computed(() => {
    const session = develop.session.value as {
      dimensions?: [number, number];
    } | null;
    const dims = session?.dimensions;
    return {
      width: Number(dims?.[0] || 0),
      height: Number(dims?.[1] || 0),
    };
  });

  const orientationSteps = computed(() =>
    normalizeSteps(develop.recipe.value?.orientationSteps ?? 0),
  );
  const flipHorizontal = computed(() =>
    Boolean(develop.recipe.value?.flipHorizontal),
  );
  const flipVertical = computed(() =>
    Boolean(develop.recipe.value?.flipVertical),
  );
  const crop = computed(() => develop.recipe.value?.crop ?? null);
  const aspectRatio = computed(() => develop.recipe.value?.aspectRatio ?? null);

  const oriented = computed(() =>
    orientedDimensions(
      sourceDimensions.value.width,
      sourceDimensions.value.height,
      orientationSteps.value,
    ),
  );

  const frameSize = computed(() => {
    const { width, height } = oriented.value;
    if (width <= 0 || height <= 0)
      return {
        width: GEOMETRY_FRAME_MAX_EDGE,
        height: GEOMETRY_FRAME_MAX_EDGE,
      };
    const ratio = width / height;
    if (ratio >= 1) {
      return {
        width: GEOMETRY_FRAME_MAX_EDGE,
        height: Math.max(1, Math.round(GEOMETRY_FRAME_MAX_EDGE / ratio)),
      };
    }
    return {
      width: Math.max(1, Math.round(GEOMETRY_FRAME_MAX_EDGE * ratio)),
      height: GEOMETRY_FRAME_MAX_EDGE,
    };
  });

  const state = computed<OrientationState>(() => ({
    steps: orientationSteps.value,
    flipHorizontal: flipHorizontal.value,
    flipVertical: flipVertical.value,
  }));

  function patch(next: Partial<Recipe>, label: string) {
    develop.applyRecipePatch(next, label);
  }

  function patchLive(next: Partial<Recipe>, label: string) {
    develop.applyRecipePatchLive(next, label);
  }

  function orientationPatch(to: OrientationState): Partial<Recipe> {
    const nextCrop = cropBetweenFrames(crop.value, state.value, to);
    // The aspect lock describes the selected region: it turns with the
    // frame (reciprocal across an odd number of quarter turns).
    const parityChanged =
      normalizeSteps(to.steps) % 2 !== normalizeSteps(state.value.steps) % 2;
    const currentAspect = aspectRatio.value;
    const nextAspect =
      currentAspect && parityChanged ? snap(1 / currentAspect) : currentAspect;
    const result: Partial<Recipe> = {
      orientationSteps: normalizeSteps(to.steps),
      flipHorizontal: to.flipHorizontal,
      flipVertical: to.flipVertical,
    };
    if (nextCrop !== null || crop.value !== null) result.crop = nextCrop;
    if (nextAspect !== null) result.aspectRatio = nextAspect;
    return result;
  }

  function rotateRight() {
    const to: OrientationState = {
      steps: rotateStepsRight(orientationSteps.value),
      flipHorizontal: flipHorizontal.value,
      flipVertical: flipVertical.value,
    };
    patch(orientationPatch(to), "rotate right");
  }

  function rotateLeft() {
    const to: OrientationState = {
      steps: rotateStepsLeft(orientationSteps.value),
      flipHorizontal: flipHorizontal.value,
      flipVertical: flipVertical.value,
    };
    patch(orientationPatch(to), "rotate left");
  }

  function toggleFlip(axis: "flipHorizontal" | "flipVertical") {
    const to: OrientationState = { ...state.value, [axis]: !state.value[axis] };
    patch(
      orientationPatch(to),
      axis === "flipHorizontal" ? "flip horizontal" : "flip vertical",
    );
  }

  function setAspect(aspect: number | null) {
    const next: Partial<Recipe> = { aspectRatio: aspect };
    if (aspect && crop.value) {
      const constrained = constrainCropToAspect(
        crop.value,
        aspect,
        oriented.value.width,
        oriented.value.height,
      );
      if (constrained) next.crop = canonicalCrop(constrained);
    }
    patch(next, "aspect lock");
  }

  function normalized(cropValue: CropRect | null): CropRect | null {
    if (!cropValue) return null;
    const error = validateCropRect(cropValue);
    if (error) return null;
    return canonicalCrop(cropValue);
  }

  function setCropLive(cropValue: CropRect | null): boolean {
    const next = normalized(cropValue);
    if (cropValue && !next) return false;
    patchLive({ crop: next }, "crop drag");
    return true;
  }

  function setCrop(cropValue: CropRect | null): boolean {
    const next = normalized(cropValue);
    if (cropValue && !next) return false;
    patch({ crop: next }, "crop");
    return true;
  }

  function beginCropGesture(label = "crop drag") {
    develop.beginEditTransaction(label);
  }

  function endCropGesture() {
    develop.endEditTransaction();
  }

  function snapshot(): Recipe | null {
    const current = develop.recipe.value;
    return current ? (JSON.parse(JSON.stringify(current)) as Recipe) : null;
  }

  function cancelCropGesture(restored: Recipe) {
    develop.cancelEditTransaction(restored);
  }

  function resetGeometry() {
    patch(geometryResetPatch(), "reset geometry");
  }

  return {
    sourceDimensions,
    orientationSteps,
    flipHorizontal,
    flipVertical,
    crop,
    aspectRatio,
    oriented,
    frameSize,
    rotateLeft,
    rotateRight,
    toggleFlipHorizontal: () => toggleFlip("flipHorizontal"),
    toggleFlipVertical: () => toggleFlip("flipVertical"),
    setAspect,
    setCropLive,
    setCrop,
    beginCropGesture,
    endCropGesture,
    snapshot,
    cancelCropGesture,
    resetGeometry,
  };
}
