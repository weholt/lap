import { describe, expect, it } from 'vitest';

// Session undo/redo history regressions for the develop editor (lap-adc /
// TASK-401; spec A9).
//
// The history is session-scoped: RapidRAW's undo stack lives in memory only
// and resets when another photo is loaded; persisted edits never mean
// persisted undo history. Slider drags, keyboard edits, resets and section
// bypasses each form exactly one coherent transaction.

import { DEVELOP_HISTORY_LIMIT, useDevelopHistory } from '@/composables/useDevelopHistory';
import { DEFAULT_RECIPE, type Recipe } from '@/composables/useDevelopSession.types';

function recipeWith(patch: Partial<Recipe>): Recipe {
    return { ...structuredClone(DEFAULT_RECIPE), ...patch };
}

describe('useDevelopHistory', () => {
    it('starts empty with no undo or redo available', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));
        expect(history.canUndo.value).toBe(false);
        expect(history.canRedo.value).toBe(false);
        expect(history.size.value).toBe(0);
        expect(history.undo()).toBeNull();
        expect(history.redo()).toBeNull();
    });

    it('coalesces all updates inside one transaction into a single undo step', () => {
        const history = useDevelopHistory();
        const base = recipeWith({});
        history.initialize(base);

        history.beginTransaction('slider drag');
        expect(history.activeLabel.value).toBe('slider drag');
        history.record(recipeWith({ exposure: 0.3 }));
        history.record(recipeWith({ exposure: 0.6 }));
        history.record(recipeWith({ exposure: 1.2 }));
        const committed = history.endTransaction();
        expect(committed).toBe(true);
        expect(history.activeLabel.value).toBeNull();
        expect(history.size.value).toBe(1);

        const restored = history.undo();
        expect(restored).toEqual(base);
        expect(history.canUndo.value).toBe(false);
        expect(history.canRedo.value).toBe(true);

        const redone = history.redo();
        expect(redone).toEqual(recipeWith({ exposure: 1.2 }));
        expect(history.canRedo.value).toBe(false);
    });

    it('keeps one undo step per ended transaction for consecutive edits', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));

        history.beginTransaction('exposure');
        history.record(recipeWith({ exposure: 0.5 }));
        history.endTransaction();
        history.beginTransaction('contrast');
        history.record(recipeWith({ exposure: 0.5, contrast: 20 }));
        history.endTransaction();

        expect(history.size.value).toBe(2);
        expect(history.undo()).toEqual(recipeWith({ exposure: 0.5 }));
        expect(history.undo()).toEqual(recipeWith({}));
        expect(history.canUndo.value).toBe(false);
    });

    it('ignores transactions that do not change the state', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({ exposure: 0.5 }));

        history.beginTransaction('no change');
        history.endTransaction();
        history.beginTransaction('same value');
        history.record(recipeWith({ exposure: 0.5 }));
        history.endTransaction();

        expect(history.size.value).toBe(0);
        expect(history.canUndo.value).toBe(false);
    });

    it('records an update without an explicit transaction as one implicit edit', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));

        history.record(recipeWith({ contrast: 10 }));
        expect(history.size.value).toBe(0);
        const committed = history.endTransaction();
        expect(committed).toBe(true);
        expect(history.size.value).toBe(1);
        expect(history.undo()).toEqual(recipeWith({}));
    });

    it('drops the redo branch when a new transaction is committed after an undo', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));

        history.beginTransaction('a');
        history.record(recipeWith({ exposure: 0.5 }));
        history.endTransaction();
        history.beginTransaction('b');
        history.record(recipeWith({ exposure: 1 }));
        history.endTransaction();
        history.undo();

        expect(history.canRedo.value).toBe(true);
        history.beginTransaction('c');
        history.record(recipeWith({ contrast: 5 }));
        history.endTransaction();

        expect(history.canRedo.value).toBe(false);
        expect(history.size.value).toBe(2);
        expect(history.undo()).toEqual(recipeWith({ exposure: 0.5 }));
    });

    it(`caps the stack at ${DEVELOP_HISTORY_LIMIT} entries and keeps the base reachable`, () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));

        for (let i = 1; i <= DEVELOP_HISTORY_LIMIT + 10; i++) {
            history.beginTransaction(`t${i}`);
            history.record(recipeWith({ exposure: i * 0.01 }));
            history.endTransaction();
        }
        expect(history.size.value).toBe(DEVELOP_HISTORY_LIMIT);

        // The oldest surviving entry is 11; entries 1..10 were dropped.
        for (let i = 0; i < DEVELOP_HISTORY_LIMIT - 2; i++) {
            history.undo();
        }
        // The 49th undo restores the oldest surviving transaction's state.
        expect(history.undo()).toEqual(recipeWith({ exposure: 11 * 0.01 }));
        // The 50th undo steps onto the pre-transaction base state.
        expect(history.undo()).toEqual(recipeWith({}));
        expect(history.canUndo.value).toBe(false);
    });

    it('returns cloned snapshots so callers cannot corrupt the stack', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));
        history.beginTransaction('edit');
        history.record(recipeWith({ exposure: 0.7 }));
        history.endTransaction();

        const restored = history.undo();
        restored!.exposure = 999;
        expect(history.redo()).toEqual(recipeWith({ exposure: 0.7 }));
    });

    it('clears everything for asset switches and session teardown', () => {
        const history = useDevelopHistory();
        history.initialize(recipeWith({}));
        history.beginTransaction('edit');
        history.record(recipeWith({ exposure: 1 }));
        history.endTransaction();

        history.clear();
        expect(history.canUndo.value).toBe(false);
        expect(history.canRedo.value).toBe(false);
        expect(history.size.value).toBe(0);
        expect(history.undo()).toBeNull();

        // After clear, a fresh initialize restarts the session history.
        history.initialize(recipeWith({}));
        expect(history.size.value).toBe(0);
    });
});
