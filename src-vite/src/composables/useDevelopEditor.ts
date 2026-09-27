import { computed, ref, shallowRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';

import { useUIStore } from '@/stores/uiStore';
import { useDevelopSession, type DevelopPreviewState } from './useDevelopSession';
import { useDevelopHistory } from './useDevelopHistory';
import {
    clampParametricValue,
    buildParametricPoints,
    defaultRecipeValue,
    DEVELOP_CONTROL_GROUPS,
    fullResetPatch,
    getRecipeValue,
    normalizeCurvePoints,
    sectionResetPatch,
    setRecipeValue,
    type CurveMode,
} from '@/components/develop/controls';
import {
    type CurvePoint,
    type ExportFormat,
    type ExportReceipt,
    type Recipe,
    type SectionId,
    type ToneMapper,
    DEFAULT_RECIPE,
    RECIPE_PARAM_RANGES,
} from './useDevelopSession.types';

/**
 * Develop editor orchestration (lap-0e9 / TASK-303).
 *
 * Sits between the native Develop panel controls and the IPC session
 * composable (lap-a52). Owns the frontend half of the persistence contract in
 * docs/raw-development/spec.md ("Persistence and compatibility"):
 *   - edits are debounced per asset and awaited on navigation/close;
 *   - save status is idle/pending/saving/saved/conflict/failed and failures
 *     retain the dirty state for an explicit retry;
 *   - per-asset working recipes are strictly separated (A never leaks into B):
 *     switching assets commits A's pending edit first and B starts from B's
 *     committed envelope; failed states are retained per asset id.
 *
 * Preview pixels come from the bounded handle transport via useDevelopSession;
 * this layer only schedules renders and maps errors to explicit UI state.
 */

export type DevelopSaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'conflict' | 'failed';

/** Debounce window before a dirty recipe edit is committed. */
export const DEVELOP_COMMIT_DEBOUNCE_MS = 800;
/** Quick interactive preview tick while controls are being dragged. */
export const DEVELOP_INTERACTIVE_PREVIEW_MS = 120;
/** Settled preview tick after the last edit. */
export const DEVELOP_SETTLED_PREVIEW_MS = 260;

export interface RetainedDevelopState {
    recipe: Recipe;
    saveState: DevelopSaveState;
    lastError: string | null;
}

export interface DevelopEditorFileInput {
    id: number | string;
}

function cloneRecipe(recipe: Recipe): Recipe {
    // Recipes are validated JSON data; a JSON round-trip yields a plain
    // (non-reactive) copy, which structuredClone cannot do across Pinia
    // reactive proxies.
    return JSON.parse(JSON.stringify(recipe)) as Recipe;
}

function isRevisionConflict(error: unknown): boolean {
    return String(error).includes('revision-conflict:');
}

/** Sidecar path mirroring the Rust `RecipeRepository::sidecar_path`. */
export function developSidecarPathFor(sourcePath: string): string {
    const normalized = String(sourcePath || '').replace(/[\\/]+$/, '');
    const lastSlash = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
    const dir = lastSlash >= 0 ? normalized.slice(0, lastSlash + 1) : '';
    const name = lastSlash >= 0 ? normalized.slice(lastSlash + 1) : normalized;
    return `${dir}${name}.lapedit.json`;
}

/** Whether the asset has a committed develop recipe sidecar. */
export async function isDevelopedAssetFile(sourcePath: string): Promise<boolean> {
    if (!sourcePath) return false;
    try {
        return await invoke<boolean>('check_file_exists', {
            filePath: developSidecarPathFor(sourcePath),
        });
    } catch {
        return false;
    }
}

export interface DevelopEditor {
    activeFileId: ReturnType<typeof ref<number | null>>;
    recipe: ReturnType<typeof shallowRef<Recipe | null>>;
    dirty: { value: boolean };
    saveState: ReturnType<typeof ref<DevelopSaveState>>;
    lastError: ReturnType<typeof ref<string | null>>;
    showOriginal: ReturnType<typeof ref<boolean>>;
    rendering: ReturnType<typeof ref<boolean>>;
    previewError: ReturnType<typeof ref<string | null>>;
    openError: ReturnType<typeof ref<string | null>>;
    opening: ReturnType<typeof ref<boolean>>;
    preview: { value: DevelopPreviewState | null };
    session: { value: unknown };
    openAsset(file: DevelopEditorFileInput): Promise<void>;
    setParam(field: string, value: number): void;
    setParamLive(field: string, value: number): void;
    resetParam(field: string): void;
    resetAll(): void;
    /**
     * Transaction control for coherent multi-step gestures: a slider drag or
     * curve point drag calls beginEditTransaction on gesture start, records
     * every intermediate value through the setters, and ends exactly one
     * history transaction on gesture end.
     */
    beginEditTransaction(label: string): void;
    endEditTransaction(): void;
    /** Session-only undo/redo (never persisted; reset on asset switch). */
    undo(): boolean;
    redo(): boolean;
    canUndo: { value: boolean };
    canRedo: { value: boolean };
    historySize: { value: number };
    setSectionVisible(section: SectionId, visible: boolean): void;
    resetSection(section: SectionId): void;
    setToneMapper(mapper: ToneMapper): void;
    setCurveChannelPointsLive(channel: string, points: CurvePoint[]): void;
    setCurveChannelPoints(channel: string, points: CurvePoint[]): void;
    setCurveMode(mode: CurveMode): void;
    setParametricCurveValueLive(channel: string, key: string, value: number): void;
    setParametricCurveValue(channel: string, key: string, value: number): void;
    /**
     * Applies a validated partial recipe update (geometry fields for
     * lap-6bc) as one complete undo transaction.
     */
    applyRecipePatch(patch: Partial<Recipe>, label: string): void;
    /**
     * Applies a validated rrdata import (lap-5c2) as the new working recipe
     * and commits it immediately: the converted recipe replaces the working
     * state, the retained original payload (`unsupported`) merges into the
     * committed envelope via the session's envelope patch, and the durable
     * receipt decides the save state. Retries keep the pending patch.
     */
    applyImportedRecipe(
        recipe: Recipe,
        unsupported: Record<string, unknown>,
    ): Promise<boolean>;
    /**
     * Live gesture variant: opens the transaction when none is open and
     * never closes it (the gesture completion calls endEditTransaction).
     */
    applyRecipePatchLive(patch: Partial<Recipe>, label: string): void;
    /**
     * Cancels the open gesture transaction and restores an exact prior
     * recipe without recording a history entry.
     */
    cancelEditTransaction(restored: Recipe): void;
    flush(options?: { autoRetry?: boolean }): Promise<boolean>;
    flushAsset(assetId: number | string): Promise<boolean>;
    retry(): Promise<boolean>;
    close(options?: { flush?: boolean }): Promise<boolean>;
    hasDirtyStateFor(assetId: number | string | null | undefined): boolean;
    disposeForTests(): Promise<void>;
}

let instance: DevelopEditor | null = null;

export function useDevelopEditor(): DevelopEditor {
    if (!instance) {
        instance = createDevelopEditor();
    }
    return instance;
}

/** Destroys the module singleton (test isolation). */
export function __resetDevelopEditorForTests(): void {
    instance = null;
}

function createDevelopEditor(): DevelopEditor {
    const uiStore = useUIStore();
    const session = useDevelopSession();
    const history = useDevelopHistory();

    const activeFileId = ref<number | null>(null);
    const recipe = shallowRef<Recipe | null>(null);
    const saveState = ref<DevelopSaveState>('idle');
    const lastError = ref<string | null>(null);
    const showOriginal = ref(false);
    const rendering = ref(false);
    const previewError = ref<string | null>(null);
    const openError = ref<string | null>(null);
    const opening = ref(false);

    let commitTimer: ReturnType<typeof setTimeout> | null = null;
    let interactivePreviewTimer: ReturnType<typeof setTimeout> | null = null;
    let settledPreviewTimer: ReturnType<typeof setTimeout> | null = null;
    let commitInFlight: Promise<boolean> | null = null;
    /**
     * Host-managed envelope fields pending the next commit (retained
     * original payload of an rrdata import). Cleared once a commit
     * acknowledges them; kept on failure so retry stays faithful.
     */
    let pendingEnvelopePatch: Record<string, unknown> | null = null;

    function clearTimers() {
        if (commitTimer) { clearTimeout(commitTimer); commitTimer = null; }
        if (interactivePreviewTimer) { clearTimeout(interactivePreviewTimer); interactivePreviewTimer = null; }
        if (settledPreviewTimer) { clearTimeout(settledPreviewTimer); settledPreviewTimer = null; }
    }

    function setSaveState(state: DevelopSaveState, error: string | null = null) {
        saveState.value = state;
        lastError.value = error;
        uiStore.setDevelopSaveState(state, error);
    }

    function markDirty(dirty: boolean) {
        uiStore.setDevelopDirty(dirty);
    }

    function isDirty(): boolean {
        return uiStore.developEditor.dirty;
    }

    function scheduleCommit() {
        if (commitTimer) clearTimeout(commitTimer);
        commitTimer = setTimeout(() => {
            commitTimer = null;
            void commitNow();
        }, DEVELOP_COMMIT_DEBOUNCE_MS);
    }

    async function renderPreview(quality: 'interactive' | 'settled') {
        if (!recipe.value) return;
        rendering.value = true;
        try {
            await session.renderPreview(recipe.value, { quality });
            previewError.value = null;
        } catch (error) {
            previewError.value = String(error);
        } finally {
            rendering.value = false;
        }
    }

    function schedulePreview(kind: 'interactive' | 'settled') {
        const timerKey = kind === 'interactive' ? 'interactive' : 'settled';
        if (timerKey === 'interactive') {
            if (interactivePreviewTimer) clearTimeout(interactivePreviewTimer);
            interactivePreviewTimer = setTimeout(() => {
                interactivePreviewTimer = null;
                void renderPreview('interactive');
            }, DEVELOP_INTERACTIVE_PREVIEW_MS);
        } else {
            if (settledPreviewTimer) clearTimeout(settledPreviewTimer);
            settledPreviewTimer = setTimeout(() => {
                settledPreviewTimer = null;
                void renderPreview('settled');
            }, DEVELOP_SETTLED_PREVIEW_MS);
        }
    }

    async function commitNow(): Promise<boolean> {
        if (commitInFlight) return commitInFlight;
        if (!isDirty() || !recipe.value || !session.session.value) return true;
        setSaveState('saving');
        commitInFlight = (async () => {
            try {
                await session.commitRecipe(
                    cloneRecipe(recipe.value as Recipe),
                    pendingEnvelopePatch ?? undefined,
                );
                pendingEnvelopePatch = null;
                markDirty(false);
                setSaveState('saved');
                uiStore.clearRetainedDevelopState(activeFileId.value ?? 0);
                return true;
            } catch (error) {
                setSaveState(
                    isRevisionConflict(error) ? 'conflict' : 'failed',
                    String(error),
                );
                return false;
            } finally {
                commitInFlight = null;
            }
        })();
        return commitInFlight;
    }

    function retainActiveState() {
        const assetId = activeFileId.value;
        if (!assetId || !recipe.value) return;
        if (!isDirty()) {
            uiStore.clearRetainedDevelopState(assetId);
            return;
        }
        uiStore.retainDevelopState(assetId, {
            recipe: cloneRecipe(recipe.value),
            saveState: saveState.value,
            lastError: lastError.value,
        });
    }

    async function openAsset(file: DevelopEditorFileInput): Promise<void> {
        const assetId = Number(file?.id || 0);
        if (!assetId || !Number.isFinite(assetId)) return;
        if (activeFileId.value === assetId && session.session.value) return;

        openError.value = null;
        // Awaited commit on navigation. An explicitly failed commit is not
        // retried automatically: it is retained per asset for retry instead.
        await flush();
        retainActiveState();

        const previousAssetId = activeFileId.value;
        activeFileId.value = null;
        markDirty(false);
        setSaveState('idle');
        showOriginal.value = false;
        previewError.value = null;
        if (previousAssetId !== null && previousAssetId !== assetId) {
            try {
                await session.closeEditSession();
            } catch {
                // Session cleanup is best-effort; the backend reaps stale sessions.
            }
        }

        opening.value = true;
        try {
            const retained = uiStore.takeRetainedDevelopState(assetId);
            const opened = await session.openEditSession(assetId, 'default');
            activeFileId.value = assetId;
            uiStore.setDevelopActive(assetId);
            recipe.value = retained ? retained.recipe : cloneRecipe(opened.envelope.recipe);
            // Session history restarts per asset: persisted edits never
            // imply persisted undo history.
            history.initialize(recipe.value);
            markDirty(Boolean(retained));
            setSaveState(retained ? retained.saveState : 'idle', retained ? retained.lastError : null);
            if (retained && retained.saveState !== 'failed' && retained.saveState !== 'conflict') {
                scheduleCommit();
            }
            await renderPreview('settled');
        } catch (error) {
            openError.value = String(error);
            throw error;
        } finally {
            opening.value = false;
        }
    }

    /**
     * Replaces the working recipe (undo/redo/reset paths): the cleaned /
     * pending decision compares against the committed envelope recipe so the
     * session only stays dirty when it truly differs, and previews/commits
     * are rescheduled. Never records a history entry by itself.
     */
    function afterRecipeReplaced(next: Recipe) {
        recipe.value = next;
        const matchesCommitted = JSON.stringify(recipe.value) === JSON.stringify(session.session.value && (session.session.value as { envelope: { recipe: Recipe } }).envelope.recipe);
        if (matchesCommitted) {
            if (commitTimer) { clearTimeout(commitTimer); commitTimer = null; }
            markDirty(false);
            setSaveState('idle');
        } else {
            markDirty(true);
            setSaveState('pending');
            scheduleCommit();
        }
        schedulePreview('interactive');
        schedulePreview('settled');
    }

    /** Opens a transaction for a live gesture unless one is already open. */
    function ensureTransaction(label: string) {
        if (history.activeLabel.value === null) {
            history.beginTransaction(label);
        }
    }

    function clampForPath(path: string, value: number): number | null {
        const numeric = Number(value);
        if (!Number.isFinite(numeric)) return null;
        const range = RECIPE_PARAM_RANGES[path];
        if (range) {
            return Math.min(range.max, Math.max(range.min, numeric));
        }
        // Nested descriptors (HSL, grading, calibration) carry the
        // model-validated bounds; unknown paths are rejected outright.
        const descriptor = nestedRangeFor(path);
        if (!descriptor) return null;
        return Math.min(descriptor.max, Math.max(descriptor.min, numeric));
    }

    function nestedRangeFor(path: string): { min: number; max: number; step: number } | null {
        for (const group of DEVELOP_CONTROL_GROUPS) {
            for (const param of group.params) {
                if (param.path === path) return param.range;
            }
        }
        return null;
    }

    /**
     * One live gesture step (slider drag input, curve point drag move).
     * Opens an implicit transaction when none is open, but never closes it:
     * gesture completion closes exactly one transaction via
     * endEditTransaction.
     */
    function setParamLive(field: string, value: number) {
        if (!recipe.value) return;
        const clamped = clampForPath(field, value);
        if (clamped === null) return;
        const current = getRecipeValue(recipe.value, field);
        if (current !== undefined && current === clamped) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        setRecipeValue(next as unknown as Record<string, unknown>, field, clamped);
        ensureTransaction(`edit ${field}`);
        history.record(next);
        afterRecipeReplaced(next);
    }

    /** A complete single edit (keyboard/numeric): exactly one transaction. */
    function setParam(field: string, value: number) {
        const wasOpen = history.activeLabel.value !== null;
        setParamLive(field, value);
        if (!wasOpen) {
            history.endTransaction();
        }
    }

    function applyResetPatch(patch: Record<string, unknown>, label: string) {
        if (!recipe.value) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        const patchClone = JSON.parse(JSON.stringify(patch)) as Record<string, unknown>;
        Object.assign(next as unknown as Record<string, unknown>, patchClone);
        const unchanged = JSON.stringify(next) === JSON.stringify(recipe.value);
        beginEditTransaction(label);
        if (!unchanged) {
            history.record(next);
            afterRecipeReplaced(next);
        }
        endEditTransaction();
    }

    function resetParam(field: string) {
        if (!recipe.value) return;
        let def: number;
        try {
            def = defaultRecipeValue(field);
        } catch {
            return;
        }
        applyResetPatch({ [field]: def }, `reset ${field}`);
    }

    function resetSection(section: SectionId) {
        applyResetPatch(sectionResetPatch(section) as Record<string, unknown>, `reset ${section}`);
    }

    function resetAll() {
        applyResetPatch(fullResetPatch() as Record<string, unknown>, 'reset all');
    }

    function setSectionVisible(section: SectionId, visible: boolean) {
        if (!recipe.value) return;
        if (recipe.value.sectionVisibility[section] === visible) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        next.sectionVisibility = { ...next.sectionVisibility, [section]: visible };
        beginEditTransaction(`bypass ${section}`);
        history.record(next);
        afterRecipeReplaced(next);
        endEditTransaction();
    }

    function setToneMapper(mapper: ToneMapper) {
        if (!recipe.value) return;
        if (recipe.value.toneMapper === mapper) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        next.toneMapper = mapper;
        beginEditTransaction('tone mapper');
        history.record(next);
        afterRecipeReplaced(next);
        endEditTransaction();
    }

    function setCurveChannelPointsLive(channel: string, points: CurvePoint[]) {
        if (!recipe.value) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        next.curves = { ...next.curves, [channel]: normalizeCurvePoints(points) };
        ensureTransaction(`curve ${channel}`);
        history.record(next);
        afterRecipeReplaced(next);
    }

    function setCurveChannelPoints(channel: string, points: CurvePoint[]) {
        setCurveChannelPointsLive(channel, points);
        endEditTransaction();
    }

    function setCurveMode(mode: CurveMode) {
        if (!recipe.value) return;
        const currentMode: CurveMode = recipe.value.curveMode || 'point';
        if (currentMode === mode) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        if (mode === 'parametric') {
            // Reference semantics: switching to parametric stores the current
            // point curves, renders the parametric settings, and flags the
            // mode; switching back restores the stored point curves.
            next.pointCurves = next.curves;
            next.curves = {
                luma: buildParametricPoints(next.parametricCurve.luma),
                red: buildParametricPoints(next.parametricCurve.red),
                green: buildParametricPoints(next.parametricCurve.green),
                blue: buildParametricPoints(next.parametricCurve.blue),
            };
        } else {
            next.curves = next.pointCurves;
        }
        next.curveMode = mode;
        beginEditTransaction('curve mode');
        history.record(next);
        afterRecipeReplaced(next);
        endEditTransaction();
    }

    function setParametricCurveValueLive(channel: string, key: string, value: number) {
        if (!recipe.value) return;
        const clamped = clampParametricValue(key, Number(value));
        if (!Number.isFinite(clamped)) return;
        const currentSettings = recipe.value.parametricCurve[channel];
        if (!currentSettings || currentSettings[key as keyof typeof currentSettings] === clamped) return;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        const settings = { ...next.parametricCurve[channel], [key]: clamped };
        next.parametricCurve = { ...next.parametricCurve, [channel]: settings };
        // The renderer consumes recipe.curves; keep the active channel's
        // rendered curve in sync with the parametric settings.
        next.curves = { ...next.curves, [channel]: buildParametricPoints(settings) };
        ensureTransaction(`parametric ${channel}`);
        history.record(next);
        afterRecipeReplaced(next);
    }

    function setParametricCurveValue(channel: string, key: string, value: number) {
        setParametricCurveValueLive(channel, key, value);
        endEditTransaction();
    }

    /**
     * Merges a validated partial recipe update (geometry fields) into the
     * working recipe. The patch values come from useDevelopGeometry, which
     * owns crop validation and oriented-frame conversions.
     */
    function mergePatch(patch: Partial<Recipe>): Recipe | null {
        if (!recipe.value) return null;
        const next = JSON.parse(JSON.stringify(recipe.value)) as Recipe;
        Object.assign(next as unknown as Record<string, unknown>, patch);
        return next;
    }

    function applyRecipePatchLive(patch: Partial<Recipe>, label: string) {
        const next = mergePatch(patch);
        if (!next) return;
        ensureTransaction(label);
        history.record(next);
        afterRecipeReplaced(next);
    }

    function applyRecipePatch(patch: Partial<Recipe>, label: string) {
        const next = mergePatch(patch);
        if (!next) return;
        const unchanged = JSON.stringify(next) === JSON.stringify(recipe.value);
        beginEditTransaction(label);
        if (!unchanged) {
            history.record(next);
            afterRecipeReplaced(next);
        }
        endEditTransaction();
    }

    /**
     * Applies a validated rrdata import (lap-5c2) and commits it
     * immediately. The converted recipe becomes the working state and the
     * import is a new editing baseline (session history restarts, like
     * opening a different committed state). The retained original payload
     * (`unsupported`: legacy metadata, unknown adjustments, inline LUT/AI
     * data) merges into the committed envelope through the session's
     * envelope patch, so the durable sidecar keeps it for diagnostics and
     * forward compatibility. A failed commit retains the dirty import for
     * an explicit retry, patch included.
     */
    async function applyImportedRecipe(
        imported: Recipe,
        unsupported: Record<string, unknown>,
    ): Promise<boolean> {
        const current = session.session.value as {
            envelope: { unsupported?: Record<string, unknown> };
        } | null;
        if (!current) {
            throw new Error('no open develop session');
        }
        pendingEnvelopePatch = {
            unsupported: {
                ...(current.envelope.unsupported ?? {}),
                ...(unsupported ?? {}),
            },
        };
        const next = cloneRecipe(imported);
        recipe.value = next;
        history.initialize(next);
        markDirty(true);
        setSaveState('pending');
        const ok = await commitNow();
        if (ok) {
            schedulePreview('settled');
        }
        return ok;
    }

    function cancelEditTransaction(restored: Recipe) {
        history.cancelTransaction();
        afterRecipeReplaced(restored);
    }

    function beginEditTransaction(label: string) {
        history.beginTransaction(label);
    }

    function endEditTransaction() {
        history.endTransaction();
    }

    function undo(): boolean {
        if (!recipe.value) return false;
        // Close any open gesture first so undo never interleaves with it.
        history.endTransaction();
        const restored = history.undo();
        if (!restored) return false;
        afterRecipeReplaced(restored);
        return true;
    }

    function redo(): boolean {
        if (!recipe.value) return false;
        history.endTransaction();
        const restored = history.redo();
        if (!restored) return false;
        afterRecipeReplaced(restored);
        return true;
    }

    async function flush(options: { autoRetry?: boolean } = {}): Promise<boolean> {
        clearTimers();
        if (commitInFlight) {
            const settled = await commitInFlight;
            if (!settled) return false;
        }
        if (!isDirty()) return true;
        const state = saveState.value;
        const shouldAttempt = state !== 'failed' && state !== 'conflict' || Boolean(options.autoRetry);
        if (!shouldAttempt) return false;
        return commitNow();
    }

    async function retry(): Promise<boolean> {
        if (!isDirty()) return true;
        const ok = await commitNow();
        if (ok) {
            schedulePreview('settled');
        }
        return ok;
    }

    /**
     * Awaiting flush entry point for save entry points acting on a specific
     * asset (info panel quick save). The active editor asset is flushed with
     * an explicit retry; a retained (non-active) state is retried on a
     * short-lived session so the displayed asset's session stays intact.
     */
    async function flushAsset(assetId: number | string): Promise<boolean> {
        const id = Number(assetId);
        if (!id || !Number.isFinite(id)) return true;
        if (activeFileId.value === id && session.session.value) {
            return flush({ autoRetry: true });
        }
        const retained = uiStore.peekRetainedDevelopState(id);
        if (!retained) return true;
        const tempSession = useDevelopSession();
        try {
            const opened = await tempSession.openEditSession(id, 'default');
            await tempSession.commitRecipe(retained.recipe);
            uiStore.takeRetainedDevelopState(id);
            void opened;
            return true;
        } catch (error) {
            uiStore.retainDevelopState(id, {
                recipe: retained.recipe,
                saveState: isRevisionConflict(error) ? 'conflict' : 'failed',
                lastError: String(error),
            });
            return false;
        } finally {
            try {
                await tempSession.closeEditSession();
            } catch {
                // Best effort cleanup.
            }
        }
    }

    async function close(options: { flush?: boolean } = {}): Promise<boolean> {
        let ok = true;
        if (options.flush !== false) {
            ok = await flush();
            retainActiveState();
        }
        try {
            await session.closeEditSession();
        } catch {
            // Best effort; the backend reaps stale sessions.
        }
        clearTimers();
        activeFileId.value = null;
        uiStore.setDevelopActive(null);
        recipe.value = null;
        history.clear();
        markDirty(false);
        setSaveState('idle');
        showOriginal.value = false;
        return ok;
    }

    function hasDirtyStateFor(assetId: number | string | null | undefined): boolean {
        return uiStore.hasDirtyDevelopState(assetId);
    }

    async function disposeForTests(): Promise<void> {
        clearTimers();
        commitInFlight = null;
        if (session.session.value) {
            try {
                await session.closeEditSession();
            } catch {
                // ignore
            }
        }
        activeFileId.value = null;
        recipe.value = null;
        history.clear();
        markDirty(false);
        setSaveState('idle');
        showOriginal.value = false;
        previewError.value = null;
        openError.value = null;
        uiStore.setDevelopActive(null);
        uiStore.developEditor.retained = {};
        __resetDevelopEditorForTests();
    }

    return {
        activeFileId,
        recipe,
        dirty: computed(() => uiStore.developEditor.dirty),
        saveState,
        lastError,
        showOriginal,
        rendering,
        previewError,
        openError,
        opening,
        preview: session.preview,
        session: session.session as unknown as { value: unknown },
        openAsset,
        setParam,
        setParamLive,
        resetParam,
        resetAll,
        beginEditTransaction,
        endEditTransaction,
        undo,
        redo,
        canUndo: history.canUndo,
        canRedo: history.canRedo,
        historySize: history.size,
        setSectionVisible,
        resetSection,
        setToneMapper,
        setCurveChannelPointsLive,
        setCurveChannelPoints,
        setCurveMode,
        setParametricCurveValueLive,
        setParametricCurveValue,
        applyRecipePatch,
        applyImportedRecipe,
        applyRecipePatchLive,
        cancelEditTransaction,
        flush,
        flushAsset,
        retry,
        close,
        hasDirtyStateFor,
        disposeForTests,
    };
}

// ---------------------------------------------------------------------------
// Developed-asset save routing for the separate editor entry point.
// ---------------------------------------------------------------------------

const DEVELOP_EXPORT_FORMATS: Record<string, ExportFormat> = {
    jpg: 'jpeg',
    jpeg: 'jpeg',
    jfif: 'jpeg',
    png: 'png',
};

export function developExportFormatFor(extension: string): ExportFormat | null {
    return DEVELOP_EXPORT_FORMATS[String(extension || '').toLowerCase()] || null;
}

/**
 * Renders the asset's committed recipe at full resolution into an explicit
 * derivative destination. Reuses the develop editor's already-open session
 * when it belongs to the same asset (so a flush-then-export sequence resolves
 * exactly the revision that was just committed); otherwise opens a
 * short-lived session. The export always resolves an immutable durable
 * revision.
 */
export async function exportDevelopedDerivative(options: {
    assetId: number;
    destination: string;
    format: ExportFormat;
    quality?: number | null;
    maxEdge?: number | null;
}): Promise<ExportReceipt> {
    const editorSession = useDevelopEditor().session as {
        value: { assetId: string | number; variantId: string; revision: number } | null;
    };
    const current = editorSession.value;
    const reusable = Boolean(
        current &&
            String(current.assetId) === String(options.assetId) &&
            current.variantId === 'default',
    );
    const session = useDevelopSession();
    if (!reusable) {
        await session.openEditSession(options.assetId, 'default');
    }
    const opened = reusable
        ? (current as { variantId: string; revision: number })
        : (session.session.value as { variantId: string; revision: number });
    try {
        const completion = await invoke<{ status: string; receipt?: ExportReceipt }>(
            'develop_export_developed',
            {
                assetId: options.assetId,
                variantId: opened.variantId,
                revision: opened.revision,
                destination: options.destination,
                format: options.format,
                quality: options.format === 'jpeg' ? (options.quality ?? 90) : null,
                maxEdge: options.maxEdge ?? null,
                exportJob: crypto.randomUUID(),
            },
        );
        if (completion.status === 'completed' && completion.receipt) {
            return completion.receipt;
        }
        throw new Error('develop derivative export was cancelled; no file was written');
    } finally {
        if (!reusable) {
            try {
                await session.closeEditSession();
            } catch {
                // Best effort cleanup.
            }
        }
    }
}

/**
 * Explicit same-path save acknowledgment for a developed asset: verifies the
 * committed recipe sidecar still loads and reports its durable revision.
 * Never writes pixels to the source path.
 */
export async function verifyDevelopedAssetDurable(assetId: number): Promise<{ revision: number }> {
    const session = useDevelopSession();
    const opened = await session.openEditSession(assetId, 'default');
    try {
        return { revision: opened.revision };
    } finally {
        try {
            await session.closeEditSession();
        } catch {
            // Best effort cleanup.
        }
    }
}
