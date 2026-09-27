import { computed, ref, shallowRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';

import { useUIStore } from '@/stores/uiStore';
import { useDevelopSession, type DevelopPreviewState } from './useDevelopSession';
import {
    type ExportFormat,
    type ExportReceipt,
    type Recipe,
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
    resetParam(field: string): void;
    resetAll(): void;
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
                await session.commitRecipe(cloneRecipe(recipe.value as Recipe));
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

    function setParam(field: string, value: number) {
        if (!recipe.value) return;
        const range = RECIPE_PARAM_RANGES[field];
        let next = Number(value);
        if (!Number.isFinite(next)) return;
        if (range) {
            next = Math.min(range.max, Math.max(range.min, next));
        }
        const current = Number((recipe.value as Record<string, unknown>)[field]);
        if (Number.isFinite(current) && current === next) return;
        recipe.value = { ...recipe.value, [field]: next };
        markDirty(true);
        setSaveState('pending');
        scheduleCommit();
        schedulePreview('interactive');
        schedulePreview('settled');
    }

    function applyReset(fields: string[]) {
        if (!recipe.value) return;
        const next: Record<string, unknown> = { ...recipe.value };
        let changed = false;
        for (const field of fields) {
            if (next[field] !== DEFAULT_RECIPE[field as keyof Recipe]) {
                next[field] = DEFAULT_RECIPE[field as keyof Recipe];
                changed = true;
            }
        }
        if (!changed) return;
        recipe.value = next as Recipe;
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

    function resetParam(field: string) {
        applyReset([field]);
    }

    function resetAll() {
        applyReset(Object.keys(RECIPE_PARAM_RANGES));
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
        resetParam,
        resetAll,
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
 * derivative destination. Opens a short-lived session so the export resolves
 * exactly the durable sidecar revision.
 */
export async function exportDevelopedDerivative(options: {
    assetId: number;
    destination: string;
    format: ExportFormat;
    quality?: number | null;
    maxEdge?: number | null;
}): Promise<ExportReceipt> {
    const session = useDevelopSession();
    const opened = await session.openEditSession(options.assetId, 'default');
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
        try {
            await session.closeEditSession();
        } catch {
            // Best effort cleanup.
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
