// Mask editing controller (lap-78d): native brush/linear/radial tools with
// explicit transaction boundaries.
//
// The controller translates pointer gestures and panel actions into recipe
// patches through the develop editor's history API:
//   - `startGesture` opens ONE history transaction and creates the mask
//     container;
//   - `extendGesture` updates the working geometry live (no history churn —
//     the editor's live API coalesces into the open transaction);
//   - `completeGesture` closes exactly one transaction;
//   - `cancelGesture` restores the exact prior recipe without an entry.
//
// Gesture coordinates arrive in display/output normalized space together
// with the recipe crop so they can be mapped into the oriented frame; all
// persisted geometry is oriented-normalized (schema contract).

import {
    DEFAULT_BRUSH_FEATHER,
    DEFAULT_BRUSH_SIZE,
    brushGeometryFromPoints,
    createMaskContainer,
    geometryToLegacy,
    linearGeometryFromDrag,
    newMaskId,
    outputToOriented,
    radialGeometryFromDrag,
    type MaskToolKind,
    type OrientedPoint,
} from '@/components/develop/masks/maskModel';
import type { CropRect, MaskContainer, MaskGeometry, MaskLocalAdjustments, Recipe, SubMaskMode } from '@/composables/useDevelopSession.types';

/** Minimal editor surface the controller needs (useDevelopEditor-compatible). */
export interface MaskEditorLike {
    recipe: { value: Recipe | null | undefined };
    beginEditTransaction(label: string): void;
    endEditTransaction(): void;
    applyRecipePatch(patch: Partial<Recipe>, label: string): void;
    applyRecipePatchLive(patch: Partial<Recipe>, label: string): void;
    cancelEditTransaction(restored: Recipe): void;
}

interface OpenGesture {
    kind: MaskToolKind;
    maskId: string;
    /** Recipe captured before the gesture, for exact cancel/restore. */
    restored: Recipe;
    points: OrientedPoint[];
    /** Oriented frame dims for the linear perpendicular computation. */
    orientedWidth: number;
    orientedHeight: number;
}

export type OrientedFrame = { readonly width: number; readonly height: number };

export function createMaskEditingController(
    editor: MaskEditorLike,
    orientedFrame: OrientedFrame | (() => OrientedFrame) = () => ({ width: 1000, height: 1000 }),
) {
    let gesture: OpenGesture | null = null;

    function frame(): OrientedFrame {
        return typeof orientedFrame === 'function' ? orientedFrame() : orientedFrame;
    }

    function currentCrop(): CropRect | null {
        return editor.recipe.value?.crop ?? null;
    }

    function toOriented(point: OrientedPoint): OrientedPoint {
        return outputToOriented(point.x, point.y, currentCrop());
    }

    function patchMasks(masks: MaskContainer[], label: string, live: boolean) {
        const patch = { masks };
        if (live) {
            editor.applyRecipePatchLive(patch, label);
        } else {
            editor.applyRecipePatch(patch, label);
        }
    }

    function replaceGeometry(maskId: string, geometry: MaskGeometry, live: boolean) {
        const recipe = editor.recipe.value;
        if (!recipe) return;
        const masks = structuredClone(recipe.masks) as MaskContainer[];
        const mask = masks.find((m) => m.id === maskId);
        if (!mask) return;
        const sub = mask.subMasks[0];
        sub.geometry = geometry;
        // Keep the legacy pixel payload in sync with the typed geometry.
        sub.parameters = geometryToLegacy(geometry, frame().width, frame().height);
        patchMasks(masks, `mask-${geometry.type}`, live);
    }

    function geometryFor(kind: MaskToolKind, oriented: OrientedPoint[], dims: { width: number; height: number }): MaskGeometry | null {
        const mask = editor.recipe.value?.masks.find((m) => m.id === gesture?.maskId);
        const existing = mask?.subMasks[0].geometry ?? null;
        switch (kind) {
            case 'brush':
                return brushGeometryFromPoints(
                    existing ?? { type: 'brush', lines: [] },
                    oriented,
                    { brushSize: DEFAULT_BRUSH_SIZE, feather: DEFAULT_BRUSH_FEATHER, tool: 'brush' },
                );
            case 'radial':
                if (oriented.length < 2) return null;
                return radialGeometryFromDrag(oriented[0], oriented[oriented.length - 1]);
            case 'linear':
                if (oriented.length < 2) return null;
                return linearGeometryFromDrag(oriented[0], oriented[oriented.length - 1], dims.width, dims.height);
        }
    }

    return {
        /** Whether a gesture transaction is currently open. */
        get gestureActive(): boolean {
            return gesture !== null;
        },

        /**
         * Starts a mask gesture: opens one history transaction and adds the
         * mask container. `point` is display-normalized; `orientedWidth`/
         * `orientedHeight` are the oriented full-frame pixel dimensions.
         */
        startGesture(kind: MaskToolKind, point: OrientedPoint, orientedWidth = 0, orientedHeight = 0): void {
            if (gesture) this.cancelGesture();
            const recipe = editor.recipe.value;
            if (!recipe) return;
            editor.beginEditTransaction(`mask-${kind}`);
            const id = newMaskId(`mask-${kind}`);
            const restored = structuredClone(recipe) as Recipe;
            const container = createMaskContainer(kind, id, '');
            container.name = `${kind === 'brush' ? 'Brush' : kind === 'radial' ? 'Radial' : 'Linear'} ${recipe.masks.length + 1}`;
            const masks = [...(structuredClone(recipe.masks) as MaskContainer[]), container];
            gesture = {
                kind,
                maskId: id,
                restored,
                points: [toOriented(point)],
                orientedWidth: orientedWidth || frame().width,
                orientedHeight: orientedHeight || frame().height,
            };
            patchMasks(masks, `mask-${kind}`, true);
        },

        /** Records a drag move into the open gesture's live geometry. */
        extendGesture(point: OrientedPoint): void {
            if (!gesture) return;
            gesture.points.push(toOriented(point));
            const geometry = geometryFor(gesture.kind, gesture.points, {
                width: gesture.orientedWidth,
                height: gesture.orientedHeight,
            });
            if (geometry) replaceGeometry(gesture.maskId, geometry, true);
        },

        /** Completes the gesture: final geometry, one committed transaction. */
        completeGesture(point?: OrientedPoint): void {
            if (!gesture) return;
            if (point) gesture.points.push(toOriented(point));
            const geometry = geometryFor(gesture.kind, gesture.points, {
                width: gesture.orientedWidth,
                height: gesture.orientedHeight,
            });
            if (geometry) replaceGeometry(gesture.maskId, geometry, true);
            gesture = null;
            editor.endEditTransaction();
        },

        /** Cancels the gesture, restoring the exact prior recipe. */
        cancelGesture(): void {
            if (!gesture) return;
            const restored = gesture.restored;
            const maskId = gesture.maskId;
            gesture = null;
            editor.cancelEditTransaction(restored);
            void maskId;
        },

        /** One-transaction scalar edit on a mask container. */
        setMaskFlag(maskId: string, flag: 'visible' | 'invert', value: boolean): void {
            this.editMask(maskId, (mask) => {
                mask[flag] = value;
            }, `mask-${flag}`);
        },

        setMaskOpacity(maskId: string, opacity: number): void {
            this.editMask(maskId, (mask) => {
                mask.opacity = opacity;
            }, 'mask-opacity');
        },

        /** One-transaction edit of a mask-local adjustment value. */
        setMaskLocal(maskId: string, field: keyof MaskLocalAdjustments, value: number): void {
            this.editMask(maskId, (mask) => {
                (mask.adjustments[field] as number) = value;
            }, `mask-local-${String(field)}`);
        },

        setSubMaskMode(maskId: string, subMaskId: string, mode: SubMaskMode): void {
            this.editMask(maskId, (mask) => {
                const sub = mask.subMasks.find((s) => s.id === subMaskId);
                if (sub) sub.mode = mode;
            }, 'mask-mode');
        },

        setSubMaskGeometry(maskId: string, geometry: MaskGeometry): void {
            this.editMask(maskId, (mask) => {
                mask.subMasks[0].geometry = geometry;
            }, `mask-${geometry.type}`);
        },

        removeMask(maskId: string): void {
            const recipe = editor.recipe.value;
            if (!recipe) return;
            const masks = (structuredClone(recipe.masks) as MaskContainer[]).filter((m) => m.id !== maskId);
            patchMasks(masks, 'mask-delete', false);
        },

        /** Reset-all for masks: one transaction, no containers remain. */
        resetMasks(): void {
            patchMasks([], 'mask-reset', false);
        },

        /** Shared single-transaction mask edit helper. */
        editMask(maskId: string, mutate: (mask: MaskContainer) => void, label: string): void {
            const recipe = editor.recipe.value;
            if (!recipe) return;
            const masks = structuredClone(recipe.masks) as MaskContainer[];
            const mask = masks.find((m) => m.id === maskId);
            if (!mask) return;
            mutate(mask);
            patchMasks(masks, label, false);
        },
    };
}

export type MaskEditingController = ReturnType<typeof createMaskEditingController>;
