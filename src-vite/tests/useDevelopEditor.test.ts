import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Behavior regressions for the develop editor session orchestration
// (lap-0e9 / TASK-303; managed continuation of lap-f19.3).
//
// Governing contract: docs/raw-development/spec.md ("Persistence and
// compatibility", A2, A10):
//   - edits are debounced per asset and awaited (flushed) on navigation/close;
//   - failed commits retain the dirty state for retry;
//   - a stale revision commit is surfaced as a conflict, never as saved;
//   - asset A's settings never leak into asset B (switch-before-debounce
//     commits A's pending edit before B's session opens, and B's working
//     recipe is exactly B's committed envelope).

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { createPinia, setActivePinia } from 'pinia';
import {
    DEVELOP_COMMIT_DEBOUNCE_MS,
    useDevelopEditor,
} from '@/composables/useDevelopEditor';
import { DEFAULT_RECIPE, RECIPE_SCHEMA_VERSION } from '@/composables/useDevelopSession.types';
import { useUIStore } from '@/stores/uiStore';

// Command-dispatched responses: preview renders may fire at any timer tick, so
// queued replies must be keyed by command, not by call order.
const commandResponses = new Map<string, unknown[]>();

function queueCommand(command: string, response: unknown) {
    if (!commandResponses.has(command)) commandResponses.set(command, []);
    commandResponses.get(command)!.push(response);
}

function lastCommitArgs(): { sessionId: number; expectedRevision: number; envelope: any } | null {
    const calls = invokeMock.mock.calls.filter(([command]) => command === 'develop_commit_recipe');
    if (calls.length === 0) return null;
    return calls[calls.length - 1][1];
}

function envelopeFor(assetId: number, revision: number) {
    return {
        schemaVersion: RECIPE_SCHEMA_VERSION,
        engineVersion: 'lap/0.3.2/rapidraw-edit-model/0.1.0',
        assetId: String(assetId),
        variantId: 'default',
        revision,
        sourceFingerprint: 'f'.repeat(64),
        decode: {},
        recipe: structuredClone(DEFAULT_RECIPE),
        resources: {},
        unsupported: {},
    };
}

function openedSession(assetId: number, revision = 0) {
    return {
        sessionId: 100 + assetId,
        assetId: String(assetId),
        variantId: 'default',
        revision,
        dimensions: [6000, 4000],
        sourceFingerprint: 'f'.repeat(64),
        envelope: envelopeFor(assetId, revision),
    };
}

function receipt(assetId: number, revision: number) {
    return {
        sessionId: 100 + assetId,
        revision,
        contentHash: 'c'.repeat(64),
        sidecarPath: `C:/photos/raw-${assetId}.CR2.lapedit.json`,
        projectionApplied: true,
        projectionError: null,
    };
}

async function openAssetA(editor: ReturnType<typeof useDevelopEditor>) {
    queueCommand('develop_open_edit_session', openedSession(1));
    await editor.openAsset({ id: 1 });
}

describe('useDevelopEditor', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        invokeMock.mockImplementation(async (command: string) => {
            const queue = commandResponses.get(command);
            if (!queue) throw new Error(`unexpected develop invoke: ${command}`);
            const response = queue.shift();
            if (response instanceof Error) throw response;
            return response;
        });
        commandResponses.clear();
        setActivePinia(createPinia());
    });

    afterEach(async () => {
        commandResponses.clear();
        await useDevelopEditor().disposeForTests();
        vi.restoreAllMocks();
    });

    it('marks edits pending and commits them after the debounce window', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);
            queueCommand('develop_commit_recipe', receipt(1, 1));

            editor.setParam('exposure', 0.5);
            expect(editor.saveState.value).toBe('pending');
            expect(editor.dirty.value).toBe(true);

            await vi.advanceTimersByTimeAsync(DEVELOP_COMMIT_DEBOUNCE_MS - 1);
            expect(lastCommitArgs()).toBeNull();

            await vi.advanceTimersByTimeAsync(1);
            expect(lastCommitArgs()).toEqual({
                sessionId: 101,
                expectedRevision: 0,
                envelope: expect.objectContaining({
                    assetId: '1',
                    recipe: expect.objectContaining({ exposure: 0.5 }),
                }),
            });
            expect(editor.saveState.value).toBe('saved');
            expect(editor.dirty.value).toBe(false);
        } finally {
            vi.useRealTimers();
        }
    });

    it('flushes a pending edit when switching assets before the debounce elapses', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);

            editor.setParam('temperature', 30);
            expect(editor.saveState.value).toBe('pending');

            queueCommand('develop_commit_recipe', receipt(1, 1));
            queueCommand('develop_open_edit_session', openedSession(2));

            // Switch to B before the commit debounce fires: A's pending edit
            // must be awaited and committed first.
            const switchPromise = editor.openAsset({ id: 2 });
            await vi.advanceTimersByTimeAsync(0);
            await switchPromise;

            expect(lastCommitArgs()).toEqual({
                sessionId: 101,
                expectedRevision: 0,
                envelope: expect.objectContaining({
                    assetId: '1',
                    recipe: expect.objectContaining({ temperature: 30 }),
                }),
            });

            // B's working recipe is exactly B's envelope: A's settings never leak.
            expect(editor.recipe.value).toEqual(DEFAULT_RECIPE);
            expect(editor.activeFileId.value).toBe(2);
            expect(editor.dirty.value).toBe(false);
        } finally {
            vi.useRealTimers();
        }
    });

    it('flushes on close with an awaited commit and releases the session', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('tint', -20);
        queueCommand('develop_commit_recipe', receipt(1, 1));
        queueCommand('develop_close_edit_session', { sessionId: 101, revision: 1 });

        const ok = await editor.close();
        expect(ok).toBe(true);
        expect(lastCommitArgs()).toEqual({
            sessionId: 101,
            expectedRevision: 0,
            envelope: expect.objectContaining({
                recipe: expect.objectContaining({ tint: -20 }),
            }),
        });
        expect(invokeMock).toHaveBeenCalledWith('develop_close_edit_session', { sessionId: 101 });
        expect(editor.activeFileId.value).toBeNull();
    });

    it('retains the dirty state when the commit fails and retries it successfully', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('exposure', 1.5);

        queueCommand('develop_commit_recipe', new Error('failed to write sidecar: access denied'));
        const firstOk = await editor.flush();
        expect(firstOk).toBe(false);
        expect(editor.saveState.value).toBe('failed');
        expect(editor.dirty.value).toBe(true);
        expect(editor.lastError.value).toContain('access denied');

        queueCommand('develop_commit_recipe', receipt(1, 1));
        const secondOk = await editor.retry();
        expect(secondOk).toBe(true);
        expect(editor.saveState.value).toBe('saved');
        expect(editor.dirty.value).toBe(false);
        expect(lastCommitArgs()).toEqual({
            sessionId: 101,
            expectedRevision: 0,
            envelope: expect.objectContaining({
                recipe: expect.objectContaining({ exposure: 1.5 }),
            }),
        });
    });

    it('surfaces a stale-revision commit as a conflict, not as saved', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('exposure', 0.25);
        queueCommand(
            'develop_commit_recipe',
            new Error('revision-conflict:expected=0,current=4; sidecar changed'),
        );

        const ok = await editor.flush();
        expect(ok).toBe(false);
        expect(editor.saveState.value).toBe('conflict');
        expect(editor.dirty.value).toBe(true);
    });

    it('keeps per-asset state separated: A failed state is retained, B stays clean', async () => {
        const editor = useDevelopEditor();
        const uiStore = useUIStore();
        await openAssetA(editor);

        editor.setParam('exposure', 2);
        queueCommand('develop_commit_recipe', new Error('failed to write sidecar: read-only directory'));
        expect(await editor.flush()).toBe(false);

        // A retains its failed state without auto-retrying on navigation.
        queueCommand('develop_open_edit_session', openedSession(2));
        await editor.openAsset({ id: 2 });
        expect(invokeMock.mock.calls.filter(([c]) => c === 'develop_commit_recipe').length).toBe(1);

        // A's failure is retained for retry, B has no dirty state.
        expect(uiStore.hasDirtyDevelopState(1)).toBe(true);
        expect(uiStore.hasDirtyDevelopState(2)).toBe(false);
        expect(editor.activeFileId.value).toBe(2);
        expect(editor.recipe.value).toEqual(DEFAULT_RECIPE);

        // Returning to A restores the retained dirty recipe and its failure state.
        queueCommand('develop_open_edit_session', openedSession(1));
        await editor.openAsset({ id: 1 });
        expect(editor.recipe.value?.exposure).toBe(2);
        expect(editor.dirty.value).toBe(true);
        expect(editor.saveState.value).toBe('failed');

        // Retry from the restored session commits the retained recipe.
        queueCommand('develop_commit_recipe', receipt(1, 1));
        expect(await editor.retry()).toBe(true);
        expect(uiStore.hasDirtyDevelopState(1)).toBe(false);
    });
});
