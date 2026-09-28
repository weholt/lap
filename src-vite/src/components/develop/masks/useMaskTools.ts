// Shared mask-tool state for the Develop panel and the central preview
// overlay (lap-78d).
//
// The controller wraps the develop editor singleton (transaction boundaries
// live there); the active tool and gesture state are UI-only and never
// enter the recipe. There is one controller per app because the editor is a
// singleton — matching useDevelopEditor's pattern.

import { ref } from 'vue';

import {
    createMaskEditingController,
    type MaskEditingController,
} from '@/components/develop/masks/maskEditingController';
import type { MaskToolKind } from '@/components/develop/masks/maskModel';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import type { CropRect, MaskContainer, Recipe } from '@/composables/useDevelopSession.types';

let instance: MaskTools | null = null;

export interface MaskTools {
    controller: MaskEditingController;
    /** UI-only: the tool armed in the panel ('' = none). Never persisted. */
    activeTool: ReturnType<typeof ref<MaskToolKind | ''>>;
    armTool(kind: MaskToolKind | ''): void;
    /** Oriented full-frame pixel dimensions for the open session. */
    readonly orientedFrame: { readonly width: number; readonly height: number };
    currentCrop(): CropRect | null;
    masks(): MaskContainer[];
}

/** Oriented frame of the currently open develop session (pixels). */
function computeOrientedFrame(editor: ReturnType<typeof useDevelopEditor>): {
    width: number;
    height: number;
} {
    const session = editor.session.value as
        | { dimensions?: [number, number]; envelope?: { recipe?: { orientationSteps?: number } } }
        | null;
    const dims = session?.dimensions;
    let width = Number(dims?.[0] ?? 0);
    let height = Number(dims?.[1] ?? 0);
    const steps = Number(session?.envelope?.recipe?.orientationSteps ?? 0) % 4;
    if (steps === 1 || steps === 3) {
        [width, height] = [height, width];
    }
    return { width: width || 1000, height: height || 1000 };
}

export function useMaskTools(): MaskTools {
    if (!instance) {
        const editor = useDevelopEditor();
        // The frame getter recomputes per access (cheap) so asset switches
        // are always reflected.
        const controller = createMaskEditingController(editor, () => computeOrientedFrame(editor));
        instance = {
            controller,
            activeTool: ref<MaskToolKind | ''>(''),
            armTool(kind: MaskToolKind | '') {
                instance!.activeTool.value = kind;
            },
            get orientedFrame() {
                return computeOrientedFrame(editor);
            },
            currentCrop(): CropRect | null {
                return (editor.recipe.value as Recipe | null | undefined)?.crop ?? null;
            },
            masks(): MaskContainer[] {
                return (editor.recipe.value as Recipe | null | undefined)?.masks ?? [];
            },
        };
    }
    return instance;
}

/** Destroys the module singleton (test isolation). */
export function __resetMaskToolsForTests(): void {
    instance = null;
}
