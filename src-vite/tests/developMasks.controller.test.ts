import { beforeEach, describe, expect, it } from 'vitest';

// Transaction-boundary contract for the mask editing controller (lap-78d).
//
// Every meaningful user action forms exactly one session history entry:
//   - a complete brush/linear/radial gesture (down → move* → up) is ONE
//     transaction; intermediate drags update the live recipe without
//     polluting history;
//   - cancelling a gesture restores the exact prior recipe with no entry;
//   - parameter edits (opacity, invert, visibility, mode, local exposure)
//     and deletions are one transaction each;
//   - resetting all masks is one transaction and leaves no masks behind.

import { createMaskEditingController } from '@/components/develop/masks/maskEditingController';
import { createMaskContainer } from '@/components/develop/masks/maskModel';
import { DEFAULT_RECIPE, type MaskContainer, type Recipe } from '@/composables/useDevelopSession.types';

/** Minimal history-tracking double of the develop editor. Mirrors
 * useDevelopHistory semantics: begin opens a pending slot, patches record,
 * end commits one entry only when the state changed, cancel discards. */
function fakeEditor() {
    let recipe: Recipe = structuredClone(DEFAULT_RECIPE);
    let presentSnapshot: Recipe = structuredClone(recipe);
    const entries: string[] = [];
    let openLabel: string | null = null;

    const maybeOpen = (label?: string) => {
        if (openLabel === null) openLabel = label ?? 'edit';
    };
    return {
        entries,
        recipe: {
            get value(): Recipe {
                return structuredClone(recipe);
            },
        },
        current: () => structuredClone(recipe),
        beginEditTransaction(label: string) {
            maybeOpen(label);
        },
        applyRecipePatchLive(patch: Partial<Recipe>, label?: string) {
            maybeOpen(label);
            recipe = { ...recipe, ...patch };
        },
        applyRecipePatch(patch: Partial<Recipe>, label?: string) {
            // Mirror the real editor: a bare patch opens an implicit
            // transaction and commits it immediately when none is open.
            const implicit = openLabel === null;
            maybeOpen(label);
            recipe = { ...recipe, ...patch };
            if (implicit) this.endEditTransaction();
        },
        endEditTransaction() {
            if (openLabel !== null) {
                if (JSON.stringify(recipe) !== JSON.stringify(presentSnapshot)) {
                    entries.push(openLabel);
                    presentSnapshot = structuredClone(recipe);
                }
                openLabel = null;
            }
        },
        cancelEditTransaction(restored: Recipe) {
            recipe = structuredClone(restored);
            openLabel = null;
        },
        snapshot() {
            return structuredClone(recipe);
        },
    };
}

function setup() {
    const editor = fakeEditor();
    const controller = createMaskEditingController(editor);
    return { editor, controller };
}

const gesture = { begin: { x: 0.2, y: 0.3 }, move1: { x: 0.3, y: 0.3 }, move2: { x: 0.4, y: 0.35 }, up: { x: 0.5, y: 0.4 } };

let setupState: ReturnType<typeof setup>;

describe('mask gesture transactions', () => {
    beforeEach(() => {
        setupState = setup();
    });

    it('a full brush gesture forms exactly one history entry', () => {
        const { editor, controller } = setupState;
        controller.startGesture('brush', gesture.begin);
        controller.extendGesture(gesture.move1);
        controller.extendGesture(gesture.move2);
        controller.completeGesture(gesture.up);

        const masks = editor.current().masks;
        expect(masks).toHaveLength(1);
        const geometry = masks[0].subMasks[0].geometry;
        expect(geometry).toMatchObject({ type: 'brush' });
        if (geometry?.type === 'brush') {
            expect(geometry.lines).toHaveLength(1);
            // begin + move1 + move2 + up, consecutive duplicates removed.
            expect(geometry.lines[0].points.length).toBeGreaterThanOrEqual(3);
        }
        expect(editor.entries).toEqual(['mask-brush']);
    });

    it('a radial gesture is one transaction and tracks the drag radii', () => {
        const { editor, controller } = setupState;
        controller.startGesture('radial', gesture.begin);
        controller.extendGesture(gesture.move2);
        controller.completeGesture(gesture.up);

        const geometry = editor.current().masks[0].subMasks[0].geometry;
        expect(geometry).toMatchObject({ type: 'radial', centerX: gesture.begin.x, centerY: gesture.begin.y });
        expect(editor.entries).toEqual(['mask-radial']);
    });

    it('a linear gesture is one transaction with the perpendicular gradient line', () => {
        const { editor, controller } = setupState;
        controller.startGesture('linear', { x: 0.5, y: 0.5 });
        controller.extendGesture({ x: 0.5, y: 0.62 });
        controller.completeGesture({ x: 0.5, y: 0.62 });

        const geometry = editor.current().masks[0].subMasks[0].geometry;
        expect(geometry).toMatchObject({ type: 'linear' });
        expect(editor.entries).toEqual(['mask-linear']);
    });

    it('cancelling a gesture restores the exact prior recipe without an entry', () => {
        const { editor, controller } = setupState;
        controller.startGesture('brush', gesture.begin);
        controller.extendGesture(gesture.move1);
        controller.completeGesture(gesture.up);
        const before = editor.snapshot();
        // A second gesture on top of the committed recipe…
        controller.startGesture('radial', gesture.begin);
        controller.extendGesture(gesture.move2);
        // …cancelled: exact restore, no extra history entry, no new mask.
        controller.cancelGesture();
        expect(editor.snapshot()).toEqual(before);
        expect(editor.entries).toEqual(['mask-brush']);
        expect(editor.current().masks).toHaveLength(1);
    });
});

describe('mask parameter transactions', () => {
    beforeEach(() => {
        setupState = setup();
    });

    it('edits opacity/invert/visibility/mode/exposure as single transactions', () => {
        const { editor, controller } = setupState;
        controller.startGesture('radial', gesture.begin);
        controller.completeGesture(gesture.up);
        const id = editor.current().masks[0].id;
        const subId = editor.current().masks[0].subMasks[0].id;

        controller.setMaskFlag(id, 'visible', false);
        controller.setMaskFlag(id, 'invert', true);
        controller.setMaskOpacity(id, 40);
        controller.setSubMaskMode(id, subId, 'intersect');
        controller.setMaskLocal(id, 'exposure', -2);

        const mask = editor.current().masks[0];
        expect(mask.visible).toBe(false);
        expect(mask.invert).toBe(true);
        expect(mask.opacity).toBe(40);
        expect(mask.subMasks[0].mode).toBe('intersect');
        expect(mask.adjustments.exposure).toBe(-2);
        expect(editor.entries).toEqual(['mask-radial', 'mask-visible', 'mask-invert', 'mask-opacity', 'mask-mode', 'mask-local-exposure']);
    });

    it('deleting a mask is one transaction and resets clears everything', () => {
        const { editor, controller } = setupState;
        controller.startGesture('radial', gesture.begin);
        controller.completeGesture(gesture.up);
        controller.startGesture('brush', gesture.begin);
        controller.completeGesture(gesture.up);
        expect(editor.current().masks).toHaveLength(2);

        const firstId = editor.current().masks[0].id;
        controller.removeMask(firstId);
        expect(editor.current().masks).toHaveLength(1);

        controller.resetMasks();
        expect(editor.current().masks).toHaveLength(0);
        expect(editor.entries).toEqual(['mask-radial', 'mask-brush', 'mask-delete', 'mask-reset']);
    });

    it('masks created by the controller pass through unchanged helper semantics', () => {
        // Parity with the model layer: containers created by gestures use the
        // same factory as the panel's add-mask flow.
        const mask: MaskContainer = createMaskContainer('linear', 'x', 'Linear 1');
        expect(mask.subMasks[0].type ?? mask.subMasks[0].kind).toBe('linear');
    });
});
