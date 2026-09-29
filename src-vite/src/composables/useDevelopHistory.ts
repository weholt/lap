import { ref, type Ref } from 'vue';

import { type Recipe } from './useDevelopSession.types';

/**
 * Session-scoped undo/redo history for the develop editor (lap-adc / TASK-401,
 * spec A9).
 *
 * Mirrors the reference editor's undo semantics: the stack lives in memory
 * only, holds a bounded number of states (50) and is reset when another asset
 * is opened or the editor closes. Persisted recipes never imply persisted
 * undo history — callers must not advertise persistence across restarts.
 *
 * One meaningful user action forms exactly one history transaction:
 *   - a slider drag coalesces every intermediate value into one entry
 *     (beginTransaction on gesture start, record per input, endTransaction on
 *     gesture end);
 *   - keyboard/numeric edits are a single implicit transaction;
 *   - reset, section reset and section bypass commits are one transaction.
 */

/** Bounded stack size (reference editor keeps up to 50 in-memory states). */
export const DEVELOP_HISTORY_LIMIT = 50;

interface HistoryEntry {
    id: number;
    label: string;
    recipe: Recipe;
}

export interface DevelopHistory {
    currentId: Readonly<Ref<number>>;
    canUndo: Readonly<Ref<boolean>>;
    canRedo: Readonly<Ref<boolean>>;
    /** Number of committed transaction entries (diagnostics/tests). */
    size: Readonly<Ref<number>>;
    /** Label of the transaction currently being recorded (diagnostics). */
    activeLabel: Readonly<Ref<string | null>>;
    initialize(recipe: Recipe): void;
    beginTransaction(label: string): void;
    /** Records the newest state of the open transaction (coalesced). */
    record(recipe: Recipe): void;
    /** Commits the pending entry; returns false for no-op transactions. */
    endTransaction(force?: boolean): boolean;
    cancelTransaction(): void;
    /** Returns the state to restore, or null when nothing to undo. */
    undo(): Recipe | null;
    /** Returns the state to restore, or null when nothing to redo. */
    redo(): Recipe | null;
    clear(): void;
}

function cloneRecipe(recipe: Recipe): Recipe {
    // Recipes are validated JSON data; a JSON round-trip yields a plain copy
    // immune to reactive proxies (same approach as useDevelopEditor).
    return JSON.parse(JSON.stringify(recipe)) as Recipe;
}

function sameRecipe(a: Recipe | null, b: Recipe | null): boolean {
    if (!a || !b) return false;
    return JSON.stringify(a) === JSON.stringify(b);
}

export function useDevelopHistory(): DevelopHistory {
    // entries[i] is the state AFTER the i-th committed transaction; index
    // points at the present state (entries.length - 1). base is the state
    // the session started from.
    let entries: HistoryEntry[] = [];
    let sequence = 0;
    const currentId = ref(0);
    let base: Recipe | null = null;
    let index = -1;
    let pending: Recipe | null = null;
    let openLabel: string | null = null;

    const canUndo = ref(false);
    const canRedo = ref(false);
    const size = ref(0);
    const activeLabel = ref<string | null>(null);

    function present(): Recipe | null {
        if (index >= 0) return entries[index].recipe;
        return base;
    }

    function sync() {
        currentId.value = index >= 0 ? entries[index].id : 0;
        canUndo.value = index >= 0;
        canRedo.value = index < entries.length - 1;
        size.value = entries.length;
        activeLabel.value = openLabel;
    }

    function initialize(recipe: Recipe): void {
        entries = [];
        base = cloneRecipe(recipe);
        index = -1;
        pending = null;
        openLabel = null;
        sync();
    }

    function beginTransaction(label: string): void {
        if (openLabel !== null) return;
        openLabel = label;
        pending = null;
        sync();
    }

    function record(recipe: Recipe): void {
        // Implicit transaction: a bare update still forms exactly one entry
        // once endTransaction is called.
        if (openLabel === null) {
            openLabel = 'edit';
            sync();
        }
        pending = cloneRecipe(recipe);
    }

    function endTransaction(force = false): boolean {
        if (openLabel === null) return false;
        const label = openLabel;
        openLabel = null;
        const next = pending;
        pending = null;
        sync();
        if (!next || (!force && sameRecipe(next, present()))) {
            return false;
        }
        entries = entries.slice(0, index + 1);
        entries.push({ id: ++sequence, label, recipe: next });
        if (entries.length > DEVELOP_HISTORY_LIMIT) {
            base = entries.shift()!.recipe;
        }
        index = entries.length - 1;
        sync();
        return true;
    }

    function cancelTransaction(): void {
        openLabel = null;
        pending = null;
        sync();
    }

    function undo(): Recipe | null {
        if (index < 0) return null;
        const restore = index >= 1 ? entries[index - 1].recipe : base;
        index -= 1;
        sync();
        return restore ? cloneRecipe(restore) : null;
    }

    function redo(): Recipe | null {
        if (index >= entries.length - 1) return null;
        index += 1;
        sync();
        return cloneRecipe(entries[index].recipe);
    }

    function clear(): void {
        entries = [];
        base = null;
        index = -1;
        pending = null;
        openLabel = null;
        sync();
    }

    return {
        currentId,
        canUndo,
        canRedo,
        size,
        activeLabel,
        initialize,
        beginTransaction,
        record,
        endTransaction,
        cancelTransaction,
        undo,
        redo,
        clear,
    };
}
