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
import { buildParametricPoints } from '@/components/develop/controls';
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
    it('reports latency only for the latest input actually drawn', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        vi.spyOn(performance, 'now').mockReturnValue(100);
        editor.setParamLive('exposure', 1);
        const frame = { handle: 'x', width: 1, height: 1, generation: 5, bytes: new ArrayBuffer(4), inputAt: 100, quality: 'interactive' as const };
        editor.preview.value = frame;
        vi.mocked(performance.now).mockReturnValue(150);
        editor.markPreviewPresented(frame);
        expect(editor.previewLatencyMs.value).toBe(50);
        const settled = { ...frame, quality: 'settled' as const };
        editor.preview.value = settled;
        vi.mocked(performance.now).mockReturnValue(400);
        editor.markPreviewPresented(settled);
        expect(editor.previewLatencyMs.value).toBe(50);
        expect(editor.presentedQuality.value).toBe('settled');
        editor.setParamLive('exposure', 2);
        vi.mocked(performance.now).mockReturnValue(200);
        editor.markPreviewPresented(frame);
        expect(editor.previewLatencyMs.value).toBe(50);
    });

    it('renders while input keeps arriving faster than the preview interval', async () => {
        vi.useFakeTimers();
        const editor = useDevelopEditor();
        await openAssetA(editor);
        invokeMock.mockClear();
        for (let i = 1; i <= 30; i++) {
            editor.setParamLive('exposure', i / 10);
            await vi.advanceTimersByTimeAsync(20);
        }
        const calls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'develop_render_preview');
        expect(calls.length).toBeGreaterThanOrEqual(3);
        expect(calls.length).toBeLessThanOrEqual(20);
        expect(calls.at(-1)![1].envelope.recipe.exposure).toBeGreaterThan(2);
        expect(invokeMock.mock.calls.filter(([cmd]) => cmd === 'develop_commit_recipe')).toHaveLength(0);
    });

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

    it('saves edits made while an earlier commit is still in flight', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        let release!: (value: unknown) => void;
        queueCommand('develop_commit_recipe', new Promise(resolve => { release = resolve; }));
        queueCommand('develop_commit_recipe', receipt(1, 2));
        editor.setParam('exposure', 1);
        const saving = editor.flush();
        editor.setParam('exposure', 2);
        release(receipt(1, 1));
        await saving;
        await editor.flush();
        expect(lastCommitArgs()?.envelope.recipe.exposure).toBe(2);
        expect(lastCommitArgs()?.expectedRevision).toBe(1);
        expect(editor.dirty.value).toBe(false);
    });

    it('preserves failed import metadata per asset across navigation and retry', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        queueCommand('develop_commit_recipe', new Error('disk read only'));
        const imported = structuredClone(DEFAULT_RECIPE);
        imported.exposure = 1;
        expect(await editor.applyImportedRecipe(imported, { originalPayload: 'keep me' })).toBe(false);
        queueCommand('develop_open_edit_session', openedSession(2));
        await editor.openAsset({id: 2});
        editor.setParam('exposure', 2);
        queueCommand('develop_commit_recipe', receipt(2, 1));
        await editor.flush();
        expect(lastCommitArgs()?.envelope.unsupported).not.toHaveProperty('originalPayload');
        queueCommand('develop_open_edit_session', openedSession(1));
        await editor.openAsset({id: 1});
        queueCommand('develop_commit_recipe', receipt(1, 1));
        expect(await editor.retry()).toBe(true);
        expect(lastCommitArgs()?.envelope.unsupported.originalPayload).toBe('keep me');
    });

    it('keeps retained state if reopening an asset fails', async () => {
        const editor = useDevelopEditor();
        const store = useUIStore();
        const recipe = structuredClone(DEFAULT_RECIPE);
        recipe.exposure = 2;
        store.retainDevelopState(1, {recipe, saveState: 'failed', lastError: 'read only'});
        queueCommand('develop_open_edit_session', new Error('decode failed'));
        await expect(editor.openAsset({id: 1})).rejects.toThrow('decode failed');
        expect(store.peekRetainedDevelopState(1)?.recipe.exposure).toBe(2);
    });

    it('resets nested color controls without adding dotted recipe keys', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        editor.setParam('colorCalibration.redHue', 25);
        editor.resetParam('colorCalibration.redHue');
        expect(editor.recipe.value?.colorCalibration.redHue).toBe(0);
        expect(editor.recipe.value).not.toHaveProperty(['colorCalibration.redHue']);
    });

    it('serializes rapid navigation and ends on the most recently requested asset', async () => {
        const editor = useDevelopEditor();
        let release!: (value: unknown) => void;
        queueCommand('develop_open_edit_session', new Promise(resolve => { release = resolve; }));
        queueCommand('develop_open_edit_session', openedSession(2));
        const first = editor.openAsset({id: 1});
        const second = editor.openAsset({id: 2});
        await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('develop_open_edit_session', {assetId: 1, variantId: 'default'}));
        expect(invokeMock.mock.calls.filter(([name]) => name === 'develop_open_edit_session')).toHaveLength(1);
        release(openedSession(1));
        await Promise.all([first, second]);
        expect(editor.activeFileId.value).toBe(2);
    });

    it('keeps the latest preview error when an older request completes', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);
            let release!: (value: unknown) => void;
            queueCommand('develop_render_preview', new Promise(resolve => { release = resolve; }));
            queueCommand('develop_render_preview', new Error('new GPU failure'));
            editor.setParam('exposure', 1);
            await vi.advanceTimersByTimeAsync(260);
            expect(editor.previewError.value).toContain('new GPU failure');
            release({status: 'cancelled'});
            await vi.advanceTimersByTimeAsync(0);
            expect(editor.previewError.value).toContain('new GPU failure');
        } finally { vi.useRealTimers(); }
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

describe('useDevelopEditor transactions, undo/redo and section controls (lap-adc)', () => {
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

    it('groups a slider drag into a single undo transaction', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.beginEditTransaction('exposure drag');
        editor.setParam('exposure', 0.3);
        editor.setParam('exposure', 0.6);
        editor.setParam('exposure', 1.2);
        editor.endEditTransaction();

        expect(editor.historySize.value).toBe(1);
        expect(editor.recipe.value?.exposure).toBe(1.2);
        expect(editor.canUndo.value).toBe(true);

        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.exposure).toBe(0);
        expect(editor.canUndo.value).toBe(false);
        expect(editor.canRedo.value).toBe(true);

        expect(editor.redo()).toBe(true);
        expect(editor.recipe.value?.exposure).toBe(1.2);
        expect(editor.canRedo.value).toBe(false);
    });

    it('keeps one undo step per keyboard/numeric edit without an open drag transaction', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('contrast', 25);
        editor.setParam('grainAmount', 40);
        expect(editor.historySize.value).toBe(2);

        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.contrast).toBe(25);
        expect(editor.recipe.value?.grainAmount).toBe(0);
        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.contrast).toBe(0);
    });

    it('undoing back to the committed recipe marks the session clean again', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        queueCommand('develop_commit_recipe', receipt(1, 1));
        editor.setParam('exposure', 0.5);
        expect(await editor.flush()).toBe(true);
        expect(editor.saveState.value).toBe('saved');

        editor.setParam('exposure', 1.5);
        expect(editor.dirty.value).toBe(true);
        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.exposure).toBe(0.5);
        expect(editor.dirty.value).toBe(false);
        expect(editor.saveState.value).toBe('idle');

        expect(editor.redo()).toBe(true);
        expect(editor.recipe.value?.exposure).toBe(1.5);
        expect(editor.dirty.value).toBe(true);
        expect(editor.saveState.value).toBe('pending');
    });

    it('records a section bypass toggle as a single transaction', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setSectionVisible('effects', false);
        expect(editor.recipe.value?.sectionVisibility.effects).toBe(false);
        expect(editor.historySize.value).toBe(1);

        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.sectionVisibility.effects).toBe(true);
        expect(editor.redo()).toBe(true);
        expect(editor.recipe.value?.sectionVisibility.effects).toBe(false);
    });

    it('resets one section in a single transaction and leaves other sections intact', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('exposure', 1);
        editor.setParam('grainAmount', 40);
        editor.resetSection('basic');

        expect(editor.recipe.value?.exposure).toBe(0);
        expect(editor.recipe.value?.grainAmount).toBe(40);
        // One transaction: the drag-like sequence above produced two entries,
        // the section reset adds exactly one more.
        expect(editor.historySize.value).toBe(3);

        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.exposure).toBe(1);
        expect(editor.recipe.value?.grainAmount).toBe(40);
    });

    it('resets the curves section back to identity curves', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setCurveChannelPoints('luma', [{ x: 0, y: 0 }, { x: 128, y: 60 }, { x: 255, y: 255 }]);
        editor.resetSection('curves');

        expect(editor.recipe.value?.curves.luma).toEqual(DEFAULT_RECIPE.curves.luma);
        expect(editor.historySize.value).toBe(2);
        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.curves.luma).toEqual([
            { x: 0, y: 0 },
            { x: 128, y: 60 },
            { x: 255, y: 255 },
        ]);
    });

    it('resets all panel sections as one transaction (union of section resets)', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParam('exposure', 1);
        editor.setParam('temperature', -30);
        editor.setParam('clarity', 20);
        editor.setParam('grainAmount', 50);
        editor.setCurveChannelPoints('luma', [{ x: 0, y: 0 }, { x: 64, y: 40 }, { x: 255, y: 255 }]);
        editor.resetAll();

        expect(editor.recipe.value).toEqual(DEFAULT_RECIPE);
        expect(editor.historySize.value).toBe(6);
        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.grainAmount).toBe(50);
        expect(editor.recipe.value?.curves.luma).toEqual([
            { x: 0, y: 0 },
            { x: 64, y: 40 },
            { x: 255, y: 255 },
        ]);
    });

    it('clears the undo history when another asset is opened (session scope)', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        editor.setParam('exposure', 1);
        expect(editor.canUndo.value).toBe(true);

        queueCommand('develop_open_edit_session', openedSession(2));
        await editor.openAsset({ id: 2 });

        expect(editor.canUndo.value).toBe(false);
        expect(editor.canRedo.value).toBe(false);
        expect(editor.historySize.value).toBe(0);
    });

    it('validates curve points before applying them to the recipe', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setCurveChannelPoints('luma', [
            { x: 255, y: 999 },
            { x: -5, y: -10 },
            { x: 128, y: 60 },
            { x: 128, y: 0 },
        ]);

        const points = editor.recipe.value?.curves.luma ?? [];
        expect(points[0]).toEqual({ x: 0, y: 0 });
        expect(points[points.length - 1]).toEqual({ x: 255, y: 255 });
        for (let i = 1; i < points.length; i++) {
            expect(points[i].x).toBeGreaterThan(points[i - 1].x);
        }
        for (const point of points) {
            expect(point.x).toBeGreaterThanOrEqual(0);
            expect(point.x).toBeLessThanOrEqual(255);
            expect(point.y).toBeGreaterThanOrEqual(0);
            expect(point.y).toBeLessThanOrEqual(255);
        }
    });

    it('switching curve modes swaps curves/pointCurves exactly like the engine contract', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setCurveChannelPoints('luma', [{ x: 0, y: 0 }, { x: 128, y: 80 }, { x: 255, y: 255 }]);
        editor.setCurveMode('parametric');

        expect(editor.recipe.value?.curveMode).toBe('parametric');
        expect(editor.recipe.value?.pointCurves.luma).toEqual([
            { x: 0, y: 0 },
            { x: 128, y: 80 },
            { x: 255, y: 255 },
        ]);
        expect(editor.recipe.value?.curves.luma).toEqual(
            buildParametricPoints(DEFAULT_RECIPE.parametricCurve.luma),
        );

        editor.setParametricCurveValue('luma', 'darks', 100);
        const response = Math.tanh(1.2) * 0.35 * Math.sqrt(0.5);
        const expectedMid = 0.5 + (response + 0) / 2;
        expect(editor.recipe.value?.curves.luma[3].y).toBeCloseTo(expectedMid * 255, 6);
        expect(editor.recipe.value?.parametricCurve.luma.darks).toBe(100);

        editor.setCurveMode('point');
        expect(editor.recipe.value?.curveMode).toBe('point');
        expect(editor.recipe.value?.curves.luma).toEqual([
            { x: 0, y: 0 },
            { x: 128, y: 80 },
            { x: 255, y: 255 },
        ]);
    });

    it('clamps parametric curve values to the model bounds', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.setParametricCurveValue('luma', 'whiteLevel', -150);
        expect(editor.recipe.value?.parametricCurve.luma.whiteLevel).toBe(-100);
        editor.setParametricCurveValue('luma', 'blackLevel', 80);
        expect(editor.recipe.value?.parametricCurve.luma.blackLevel).toBe(80);
        editor.setParametricCurveValue('luma', 'darks', 500);
        expect(editor.recipe.value?.parametricCurve.luma.darks).toBe(100);
    });

    it('does not resurrect undo history after close (no persistence across restarts)', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);
        editor.setParam('exposure', 1);
        expect(editor.canUndo.value).toBe(true);

        queueCommand('develop_close_edit_session', { sessionId: 101, revision: 0 });
        await editor.close();

        expect(editor.canUndo.value).toBe(false);
        expect(editor.historySize.value).toBe(0);
    });

    // -----------------------------------------------------------------------
    // Geometry patches (lap-6bc / TASK-402): crop/orientation edits are
    // recipe edits like any other — undoable, resettable, committed through
    // the same debounced durable save.
    // -----------------------------------------------------------------------

    it('applies a geometry patch as exactly one transaction with dirty/pending state', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.applyRecipePatch(
            {
                orientationSteps: 1,
                flipHorizontal: true,
                crop: { x: 0.1, y: 0.2, width: 0.5, height: 0.4 },
                aspectRatio: 1.25,
            },
            'rotate',
        );

        expect(editor.recipe.value?.orientationSteps).toBe(1);
        expect(editor.recipe.value?.flipHorizontal).toBe(true);
        expect(editor.recipe.value?.crop).toEqual({ x: 0.1, y: 0.2, width: 0.5, height: 0.4 });
        expect(editor.historySize.value).toBe(1);
        expect(editor.dirty.value).toBe(true);
        expect(editor.saveState.value).toBe('pending');
        expect(editor.undo()).toBe(true);
        expect(editor.recipe.value?.orientationSteps).toBe(0);
        expect(editor.recipe.value?.crop).toBeNull();
    });

    it('coalesces live geometry gesture steps into the open transaction', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.beginEditTransaction('crop drag');
        editor.applyRecipePatchLive({ crop: { x: 0.1, y: 0.1, width: 0.5, height: 0.5 } }, 'crop drag');
        editor.applyRecipePatchLive({ crop: { x: 0.2, y: 0.2, width: 0.4, height: 0.4 } }, 'crop drag');
        expect(editor.historySize.value).toBe(0);
        editor.endEditTransaction();

        expect(editor.historySize.value).toBe(1);
        expect(editor.recipe.value?.crop).toEqual({ x: 0.2, y: 0.2, width: 0.4, height: 0.4 });
    });

    it('cancels an open gesture transaction and restores the exact prior recipe', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        const before = editor.recipe.value;
        editor.beginEditTransaction('crop drag');
        editor.applyRecipePatchLive({ crop: { x: 0.3, y: 0.3, width: 0.2, height: 0.2 } }, 'crop drag');
        expect(editor.recipe.value?.crop).not.toBeNull();

        editor.cancelEditTransaction(before!);
        expect(editor.recipe.value?.crop).toBeNull();
        expect(editor.historySize.value).toBe(0);
        expect(editor.dirty.value).toBe(false);
        expect(editor.saveState.value).toBe('idle');
    });

    it('commits a geometry patch through the debounced durable save', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);
            queueCommand('develop_commit_recipe', receipt(1, 1));

            editor.applyRecipePatch({ crop: { x: 0, y: 0, width: 0.5, height: 0.5 } }, 'crop');
            await vi.advanceTimersByTimeAsync(DEVELOP_COMMIT_DEBOUNCE_MS);

            expect(lastCommitArgs()?.envelope.recipe).toEqual(
                expect.objectContaining({ crop: { x: 0, y: 0, width: 0.5, height: 0.5 } }),
            );
            expect(editor.saveState.value).toBe('saved');
        } finally {
            vi.useRealTimers();
        }
    });

    it('includes geometry defaults in reset-all (geometry participates in reset)', async () => {
        const editor = useDevelopEditor();
        await openAssetA(editor);

        editor.applyRecipePatch(
            {
                orientationSteps: 2,
                flipHorizontal: true,
                crop: { x: 0.1, y: 0.1, width: 0.5, height: 0.5 },
                aspectRatio: 1,
            },
            'geometry',
        );
        editor.resetAll();

        expect(editor.recipe.value?.orientationSteps).toBe(0);
        expect(editor.recipe.value?.flipHorizontal).toBe(false);
        expect(editor.recipe.value?.crop).toBeNull();
        expect(editor.recipe.value?.aspectRatio).toBeNull();
    });

    it('applies an imported rrdata recipe with an immediate commit carrying the retained payload', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);
            queueCommand('develop_commit_recipe', receipt(1, 1));

            const imported = structuredClone(DEFAULT_RECIPE);
            imported.exposure = 0.75;
            const ok = await editor.applyImportedRecipe(imported, {
                'legacyMetadata.rating': 3,
                'legacyAdjustments.futureUnknownSlider': 9,
            });

            expect(ok).toBe(true);
            expect(editor.recipe.value?.exposure).toBe(0.75);
            expect(editor.saveState.value).toBe('saved');
            expect(editor.dirty.value).toBe(false);

            const commit = lastCommitArgs();
            expect(commit?.envelope.recipe.exposure).toBe(0.75);
            expect(commit?.envelope.unsupported['legacyMetadata.rating']).toBe(3);
            expect(commit?.envelope.unsupported['legacyAdjustments.futureUnknownSlider']).toBe(9);

            // The retained payload lives in the session envelope now: later
            // ordinary edits keep committing it (diagnostics retention).
            queueCommand('develop_commit_recipe', receipt(1, 2));
            editor.setParam('exposure', 0.25);
            await vi.advanceTimersByTimeAsync(DEVELOP_COMMIT_DEBOUNCE_MS + 50);
            const later = lastCommitArgs();
            expect(later?.envelope.recipe.exposure).toBe(0.25);
            expect(later?.envelope.unsupported['legacyMetadata.rating']).toBe(3);
        } finally {
            vi.useRealTimers();
        }
    });

    it('retains the pending import patch when the import commit fails and retries with it', async () => {
        vi.useFakeTimers();
        try {
            const editor = useDevelopEditor();
            await openAssetA(editor);
            queueCommand('develop_commit_recipe', new Error('disk full'));

            const imported = structuredClone(DEFAULT_RECIPE);
            imported.exposure = 0.4;
            const ok = await editor.applyImportedRecipe(imported, {
                'legacyMetadata.rating': 5,
            });

            expect(ok).toBe(false);
            expect(editor.saveState.value).toBe('failed');
            expect(editor.dirty.value).toBe(true);
            expect(editor.recipe.value?.exposure).toBe(0.4);

            queueCommand('develop_commit_recipe', receipt(1, 1));
            const retried = await editor.retry();
            expect(retried).toBe(true);
            const commit = lastCommitArgs();
            expect(commit?.envelope.recipe.exposure).toBe(0.4);
            expect(commit?.envelope.unsupported['legacyMetadata.rating']).toBe(5);
        } finally {
            vi.useRealTimers();
        }
    });
});
