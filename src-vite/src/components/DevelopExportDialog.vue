<script setup lang="ts">
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';

import type {
    CommitReceipt,
    ExportCompletion,
    ExportFormat,
    ExportReceipt,
    OpenedEditSession,
    Recipe,
} from '@/composables/useDevelopSession.types';

/**
 * Derivative export dialog (lap-7ae / TASK-304, docs/raw-development/spec.md
 * `export_developed`).
 *
 * Commit and derivative export are **separate actions**: "Commit" durably
 * persists the edited recipe without writing any pixels; "Export derivative"
 * flushes uncommitted edits first (the backend then resolves exactly the
 * acknowledged committed revision — a stale request fails with an explicit
 * revision mismatch instead of silently exporting "latest") and writes a
 * full-resolution derivative to an explicit destination. Source paths and
 * their aliases are rejected by the Rust backend before any write, and the
 * derivative is written atomically.
 *
 * Note: UI copy is English-only in this slice; localization lands with the
 * Develop panel integration (spec A10 interactive verification).
 */

const props = defineProps<{
    session: OpenedEditSession | null;
    /** The edited recipe in the develop panel (may be dirtier than committed). */
    recipe: Recipe;
    /** True when `recipe` differs from the committed envelope. */
    dirty: boolean;
    /** Durable commit through the develop session (flush-before-export). */
    commitRecipe: (recipe: Recipe) => Promise<CommitReceipt>;
}>();

const emit = defineEmits<{
    committed: [receipt: CommitReceipt];
    close: [];
}>();

const destination = ref('');
const format = ref<ExportFormat>('png');
const jpegQuality = ref(90);
/** Full resolution exports the original decoded dimensions (spec A5). */
const useFullResolution = ref(true);
const maxEdgeInput = ref(4096);
const committing = ref(false);
const exporting = ref(false);
const status = ref<string | null>(null);
const errorMessage = ref<string | null>(null);
const lastReceipt = ref<ExportReceipt | null>(null);

let activeExportJob = '';

const canExport = computed(
    () =>
        !!props.session &&
        destination.value.trim().length > 0 &&
        !exporting.value &&
        !committing.value,
);

function defaultName(): string {
    const stem = props.session ? `asset-${props.session.assetId}-developed` : 'developed';
    return `${stem}.${format.value === 'png' ? 'png' : 'jpg'}`;
}

async function chooseDestination(): Promise<void> {
    const selected = await save({
        title: 'Export developed derivative',
        defaultPath: defaultName(),
        filters: [
            {
                name: format.value === 'png' ? 'PNG image' : 'JPEG image',
                extensions: [format.value === 'png' ? 'png' : 'jpg'],
            },
        ],
    });
    if (selected) {
        destination.value = selected;
        errorMessage.value = null;
    }
}

/**
 * Separate action: durably commit the edited recipe. No pixels are written;
 * this is the explicit flush the panel can also use on navigation.
 */
async function commit(): Promise<void> {
    if (!props.session || committing.value) return;
    committing.value = true;
    errorMessage.value = null;
    try {
        const receipt = await props.commitRecipe(props.recipe);
        emit('committed', receipt);
        status.value = `Committed revision ${receipt.revision}.`;
    } catch (err) {
        errorMessage.value = String(err);
    } finally {
        committing.value = false;
    }
}

/**
 * Flush-before-export: dirty edits are committed first so the export
 * resolves exactly the revision the backend will acknowledge.
 */
async function resolveRevisionForExport(): Promise<number | null> {
    const current = props.session;
    if (!current) return null;
    if (!props.dirty) {
        return current.revision;
    }
    const receipt = await props.commitRecipe(props.recipe);
    emit('committed', receipt);
    return receipt.revision;
}

async function exportDerivative(): Promise<void> {
    const current = props.session;
    if (!current || !canExport.value) return;
    exporting.value = true;
    errorMessage.value = null;
    status.value = null;
    lastReceipt.value = null;
    activeExportJob = crypto.randomUUID();
    try {
        const revision = await resolveRevisionForExport();
        if (revision === null) return;
        const completion = await invoke<ExportCompletion>('develop_export_developed', {
            assetId: Number(current.assetId),
            variantId: current.variantId,
            revision,
            destination: destination.value.trim(),
            format: format.value,
            quality: format.value === 'jpeg' ? jpegQuality.value : null,
            maxEdge: useFullResolution.value ? null : maxEdgeInput.value,
            exportJob: activeExportJob,
        });
        if (completion.status === 'completed') {
            lastReceipt.value = completion.receipt;
            status.value = `Exported ${completion.receipt.width}x${completion.receipt.height} (${completion.receipt.bytesWritten} bytes) at revision ${completion.receipt.revision}.`;
        } else {
            status.value = 'Export cancelled; no file was written.';
        }
    } catch (err) {
        errorMessage.value = String(err);
    } finally {
        exporting.value = false;
        activeExportJob = '';
    }
}

/** Cooperative cancellation: the backend leaves no partial file behind. */
async function cancelExport(): Promise<void> {
    if (!activeExportJob) return;
    await invoke('develop_cancel_export', { exportJob: activeExportJob });
}
</script>

<template>
    <div
        v-if="props.session"
        class="develop-export-dialog"
        data-testid="develop-export-dialog"
    >
        <div class="develop-export-dialog__panel">
            <header class="develop-export-dialog__header">
                <h2>Export derivative</h2>
                <button
                    type="button"
                    class="develop-export-dialog__close"
                    data-testid="develop-export-close"
                    aria-label="Close"
                    @click="emit('close')"
                >
                    ✕
                </button>
            </header>

            <p class="develop-export-dialog__hint">
                Exporting writes a full-resolution derivative of the committed
                recipe to a separate file. The original photo and its recipe
                are never modified. Destination paths pointing at the source
                photo are rejected.
            </p>

            <label class="develop-export-dialog__field">
                <span>Destination</span>
                <div class="develop-export-dialog__row">
                    <input
                        v-model="destination"
                        type="text"
                        data-testid="develop-export-destination"
                        placeholder="Choose an export destination"
                        spellcheck="false"
                    />
                    <button
                        type="button"
                        data-testid="develop-export-browse"
                        @click="chooseDestination"
                    >
                        Browse…
                    </button>
                </div>
            </label>

            <label class="develop-export-dialog__field">
                <span>Format</span>
                <select
                    v-model="format"
                    data-testid="develop-export-format"
                >
                    <option value="png">PNG</option>
                    <option value="jpeg">JPEG</option>
                </select>
            </label>

            <label
                v-if="format === 'jpeg'"
                class="develop-export-dialog__field"
            >
                <span>JPEG quality</span>
                <input
                    v-model.number="jpegQuality"
                    type="number"
                    min="1"
                    max="100"
                    data-testid="develop-export-quality"
                />
            </label>

            <div class="develop-export-dialog__field">
                <label class="develop-export-dialog__checkbox">
                    <input
                        v-model="useFullResolution"
                        type="checkbox"
                        data-testid="develop-export-fullres"
                    />
                    <span>Original dimensions (full resolution)</span>
                </label>
                <label
                    v-if="!useFullResolution"
                    class="develop-export-dialog__field"
                >
                    <span>Longest edge (pixels)</span>
                    <input
                        v-model.number="maxEdgeInput"
                        type="number"
                        min="1"
                        data-testid="develop-export-maxedge"
                    />
                </label>
            </div>

            <div
                class="develop-export-dialog__messages"
                aria-live="polite"
            >
                <p
                    v-if="errorMessage"
                    class="develop-export-dialog__error"
                    data-testid="develop-export-error"
                >
                    {{ errorMessage }}
                </p>
                <p
                    v-if="status"
                    class="develop-export-dialog__status"
                    data-testid="develop-export-status"
                >
                    {{ status }}
                </p>
                <p
                    v-if="lastReceipt"
                    class="develop-export-dialog__receipt"
                    data-testid="develop-export-receipt"
                >
                    Revision {{ lastReceipt.revision }} ·
                    {{ lastReceipt.width }}×{{ lastReceipt.height }} ·
                    {{ lastReceipt.format.toUpperCase() }}
                </p>
            </div>

            <footer class="develop-export-dialog__actions">
                <button
                    type="button"
                    class="develop-export-dialog__secondary"
                    data-testid="develop-export-commit"
                    :disabled="!props.session || committing || exporting"
                    @click="commit"
                >
                    {{ committing ? 'Committing…' : 'Commit changes' }}
                </button>
                <button
                    v-if="exporting"
                    type="button"
                    class="develop-export-dialog__secondary"
                    data-testid="develop-export-cancel"
                    @click="cancelExport"
                >
                    Cancel export
                </button>
                <button
                    type="button"
                    class="develop-export-dialog__primary"
                    data-testid="develop-export-start"
                    :disabled="!canExport"
                    @click="exportDerivative"
                >
                    {{ exporting ? 'Exporting…' : 'Export derivative' }}
                </button>
            </footer>
        </div>
    </div>
</template>

<style scoped>
.develop-export-dialog {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.55);
}

.develop-export-dialog__panel {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    width: min(30rem, 92vw);
    padding: 1.25rem;
    border-radius: 0.6rem;
    background: var(--color-bg-elevated, #1f1f23);
    color: inherit;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
}

.develop-export-dialog__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
}

.develop-export-dialog__header h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
}

.develop-export-dialog__close {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 0.95rem;
    color: inherit;
    opacity: 0.75;
}

.develop-export-dialog__close:hover {
    opacity: 1;
}

.develop-export-dialog__hint {
    margin: 0;
    font-size: 0.8rem;
    line-height: 1.45;
    opacity: 0.7;
}

.develop-export-dialog__field {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.82rem;
}

.develop-export-dialog__field > span {
    opacity: 0.75;
}

.develop-export-dialog__row {
    display: flex;
    gap: 0.4rem;
}

.develop-export-dialog__row input[type='text'] {
    flex: 1;
}

.develop-export-dialog__checkbox {
    display: flex;
    align-items: center;
    gap: 0.4rem;
}

.develop-export-dialog__messages {
    min-height: 1.2rem;
    font-size: 0.8rem;
}

.develop-export-dialog__error {
    margin: 0;
    color: #f87171;
}

.develop-export-dialog__status {
    margin: 0;
    opacity: 0.85;
}

.develop-export-dialog__receipt {
    margin: 0.15rem 0 0;
    opacity: 0.6;
}

.develop-export-dialog__actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
}

.develop-export-dialog__primary,
.develop-export-dialog__secondary {
    border-radius: 0.4rem;
    padding: 0.45rem 0.9rem;
    font-size: 0.85rem;
    cursor: pointer;
}

.develop-export-dialog__primary {
    background: var(--color-accent, #4f8cff);
    border: 1px solid transparent;
    color: #fff;
}

.develop-export-dialog__primary:disabled {
    opacity: 0.45;
    cursor: not-allowed;
}

.develop-export-dialog__secondary {
    background: transparent;
    border: 1px solid currentColor;
    color: inherit;
    opacity: 0.85;
}

.develop-export-dialog__secondary:disabled {
    opacity: 0.4;
    cursor: not-allowed;
}
</style>
