<script setup lang="ts">
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

import type {
    OpenedEditSession,
    Recipe,
} from '@/composables/useDevelopSession.types';

/**
 * Explicit .rrdata import dialog (lap-5c2 / TASK-404; managed continuation
 * of lap-002.4; docs/raw-development/spec.md "Persistence and compatibility").
 *
 * One-way compatibility reader flow: pick a RapidRAW `.rrdata` sidecar, the
 * backend validates it and returns the converted envelope plus a fidelity
 * report, and the user sees every named limitation (unsupported visible
 * effects, missing LUT/mask resources) BEFORE anything is applied. Nothing
 * is written by this dialog: the original rrdata stays untouched and the
 * converted recipe only reaches the durable sidecar through the explicit
 * confirm action (`applyImported` -> editor commit). Declining is always
 * safe; importing never modifies the source document.
 *
 * Note: UI copy is English-only in this slice; localization lands with the
 * Develop panel integration (spec A10 interactive verification).
 */

/** Backend fidelity outcome of `develop_import_rrdata` (serde camelCase). */
interface ImportLimitation {
    kind: string;
    detail: string;
}

interface RrdataImportOutcome {
    envelope: {
        recipe: Recipe;
        unsupported: Record<string, unknown>;
    };
    limitations: ImportLimitation[];
    preservedKeys: string[];
    excludedUiFields: string[];
    migrationApplied: string[];
    original: { sha256: string; byteLen: number };
    faithfulSubset: boolean;
}

const props = defineProps<{
    session: OpenedEditSession | null;
    /** Explicit durable application of the confirmed import. */
    applyImported: (
        recipe: Recipe,
        unsupported: Record<string, unknown>,
    ) => Promise<boolean>;
}>();

const emit = defineEmits<{
    applied: [];
    close: [];
}>();

const importing = ref(false);
const applying = ref(false);
const outcome = ref<RrdataImportOutcome | null>(null);
const sourcePath = ref('');
const sourceVersion = ref<number | null>(null);
const errorMessage = ref<string | null>(null);
const appliedMessage = ref<string | null>(null);

const canConfirm = computed(
    () =>
        !!outcome.value &&
        !applying.value &&
        !importing.value &&
        errorMessage.value === null &&
        appliedMessage.value === null,
);

/** Kind -> short human label for the limitation list. */
function limitationLabel(kind: string): string {
    switch (kind) {
        case 'lens-blur':
            return 'Lens blur will not be rendered';
        case 'lut-resource':
            return 'LUT is preserved but not applied';
        case 'missing-resource':
            return 'Referenced resource is missing';
        case 'masks':
            return 'Local masks are preserved but not rendered';
        case 'ai-patches':
            return 'Generative AI edits are out of scope';
        default:
            return kind;
    }
}

async function chooseRrdata(): Promise<void> {
    errorMessage.value = null;
    appliedMessage.value = null;
    outcome.value = null;
    sourceVersion.value = null;
    const selected = await open({
        title: 'Import RapidRAW .rrdata recipe',
        filters: [{ name: 'RapidRAW recipe (.rrdata)', extensions: ['rrdata', 'json'] }],
        multiple: false,
    });
    if (!selected || typeof selected !== 'string') return;
    const session = props.session;
    if (!session) return;
    importing.value = true;
    sourcePath.value = selected;
    try {
        const result = await invoke<RrdataImportOutcome & { envelope: { schemaVersion?: number } }>(
            'develop_import_rrdata',
            {
                rrdataPath: selected,
                assetId: Number(session.assetId),
                variantId: session.variantId,
                sourceFingerprint: session.sourceFingerprint,
                sourceWidth: session.dimensions[0],
                sourceHeight: session.dimensions[1],
            },
        );
        outcome.value = result;
        sourceVersion.value =
            (result.envelope as { unsupported?: Record<string, unknown> }).unsupported?.[
                'legacyMetadata.version'
            ] != null
                ? Number(
                      (result.envelope as { unsupported: Record<string, unknown> }).unsupported[
                          'legacyMetadata.version'
                      ],
                  )
                : null;
    } catch (err) {
        outcome.value = null;
        errorMessage.value = String(err);
    } finally {
        importing.value = false;
    }
}

async function confirmImport(): Promise<void> {
    if (!outcome.value || !canConfirm.value) return;
    applying.value = true;
    errorMessage.value = null;
    try {
        const ok = await props.applyImported(
            outcome.value.envelope.recipe,
            outcome.value.envelope.unsupported,
        );
        if (ok) {
            appliedMessage.value = 'Imported recipe committed.';
            emit('applied');
        } else {
            errorMessage.value =
                'Import could not be committed; the failed state is retained for retry.';
        }
    } catch (err) {
        errorMessage.value = String(err);
    } finally {
        applying.value = false;
    }
}

function reset(): void {
    outcome.value = null;
    sourcePath.value = '';
    sourceVersion.value = null;
    errorMessage.value = null;
    appliedMessage.value = null;
}
</script>

<template>
    <div
        v-if="props.session"
        class="develop-import-dialog"
        data-testid="develop-import-dialog"
    >
        <div class="develop-import-dialog__panel">
            <header class="develop-import-dialog__header">
                <h2>Import RapidRAW recipe</h2>
                <button
                    type="button"
                    class="develop-import-dialog__close"
                    data-testid="develop-import-close"
                    aria-label="Close"
                    @click="emit('close')"
                >
                    ✕
                </button>
            </header>

            <p class="develop-import-dialog__hint">
                Reads an existing RapidRAW <code>.rrdata</code> sidecar for this
                photo. The original file is never modified. Supported
                parameters are validated before anything is applied; effects
                this version cannot render are listed as limitations instead
                of being silently dropped.
            </p>

            <div class="develop-import-dialog__row">
                <button
                    type="button"
                    class="develop-import-dialog__secondary"
                    data-testid="develop-import-browse"
                    :disabled="importing || applying"
                    @click="chooseRrdata"
                >
                    {{ importing ? 'Reading…' : 'Choose .rrdata file…' }}
                </button>
            </div>

            <p
                v-if="sourcePath"
                class="develop-import-dialog__source"
                data-testid="develop-import-source"
            >
                {{ sourcePath }}
                <span v-if="sourceVersion !== null">
                    · source schema v{{ sourceVersion }}
                </span>
            </p>

            <div
                v-if="outcome"
                class="develop-import-dialog__report"
                data-testid="develop-import-report"
            >
                <p
                    v-if="outcome.faithfulSubset"
                    class="develop-import-dialog__ok"
                    data-testid="develop-import-faithful"
                >
                    All parameters of this document are supported by this
                    version.
                </p>
                <p
                    v-else
                    class="develop-import-dialog__warning"
                    data-testid="develop-import-not-faithful"
                >
                    This import is not a faithful reproduction: the effects
                    below stay in the recipe but are not rendered by this
                    version. Preview and export cannot claim to match
                    RapidRAW for this photo.
                </p>

                <ul
                    v-if="outcome.limitations.length"
                    class="develop-import-dialog__limitations"
                    data-testid="develop-import-limitations"
                >
                    <li
                        v-for="limitation in outcome.limitations"
                        :key="limitation.kind + limitation.detail"
                        class="develop-import-dialog__limitation"
                        :data-testid="`develop-import-limitation-${limitation.kind}`"
                    >
                        <strong>{{ limitationLabel(limitation.kind) }}</strong>
                        — {{ limitation.detail }}
                    </li>
                </ul>

                <p
                    v-if="outcome.preservedKeys.length"
                    class="develop-import-dialog__preserved"
                    data-testid="develop-import-preserved"
                >
                    Preserved from the original document:
                    {{ outcome.preservedKeys.join(', ') }}
                </p>
            </div>

            <div
                class="develop-import-dialog__messages"
                aria-live="polite"
            >
                <p
                    v-if="errorMessage"
                    class="develop-import-dialog__error"
                    data-testid="develop-import-error"
                >
                    {{ errorMessage }}
                </p>
                <p
                    v-if="appliedMessage"
                    class="develop-import-dialog__status"
                    data-testid="develop-import-status"
                >
                    {{ appliedMessage }}
                </p>
            </div>

            <footer class="develop-import-dialog__actions">
                <button
                    type="button"
                    class="develop-import-dialog__secondary"
                    data-testid="develop-import-restart"
                    :disabled="!outcome || applying"
                    @click="reset"
                >
                    Start over
                </button>
                <button
                    type="button"
                    class="develop-import-dialog__primary"
                    data-testid="develop-import-confirm"
                    :disabled="!canConfirm"
                    @click="confirmImport"
                >
                    {{ applying ? 'Applying…' : 'Import recipe' }}
                </button>
            </footer>
        </div>
    </div>
</template>

<style scoped>
.develop-import-dialog {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(0, 0, 0, 0.55);
}

.develop-import-dialog__panel {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    width: min(34rem, 92vw);
    max-height: 88vh;
    overflow-y: auto;
    padding: 1.25rem;
    border-radius: 0.6rem;
    background: var(--color-bg-elevated, #1f1f23);
    color: inherit;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
}

.develop-import-dialog__header {
    display: flex;
    align-items: center;
    justify-content: space-between;
}

.develop-import-dialog__header h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
}

.develop-import-dialog__close {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 0.95rem;
    color: inherit;
    opacity: 0.75;
}

.develop-import-dialog__close:hover {
    opacity: 1;
}

.develop-import-dialog__hint {
    margin: 0;
    font-size: 0.8rem;
    line-height: 1.45;
    opacity: 0.7;
}

.develop-import-dialog__row {
    display: flex;
    gap: 0.4rem;
}

.develop-import-dialog__source {
    margin: 0;
    font-size: 0.75rem;
    opacity: 0.6;
    word-break: break-all;
}

.develop-import-dialog__report {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    font-size: 0.8rem;
}

.develop-import-dialog__ok {
    margin: 0;
    color: #4ade80;
}

.develop-import-dialog__warning {
    margin: 0;
    color: #fbbf24;
}

.develop-import-dialog__limitations {
    margin: 0;
    padding-left: 1.1rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
}

.develop-import-dialog__limitation {
    line-height: 1.4;
}

.develop-import-dialog__preserved {
    margin: 0;
    font-size: 0.72rem;
    opacity: 0.6;
    word-break: break-all;
}

.develop-import-dialog__messages {
    min-height: 1.2rem;
    font-size: 0.8rem;
}

.develop-import-dialog__error {
    margin: 0;
    color: #f87171;
}

.develop-import-dialog__status {
    margin: 0;
    opacity: 0.85;
}

.develop-import-dialog__actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
}

.develop-import-dialog__primary,
.develop-import-dialog__secondary {
    border-radius: 0.4rem;
    padding: 0.45rem 0.9rem;
    font-size: 0.85rem;
    cursor: pointer;
}

.develop-import-dialog__primary {
    background: var(--color-accent, #4f8cff);
    border: 1px solid transparent;
    color: #fff;
}

.develop-import-dialog__primary:disabled {
    opacity: 0.45;
    cursor: not-allowed;
}

.develop-import-dialog__secondary {
    background: transparent;
    border: 1px solid currentColor;
    color: inherit;
    opacity: 0.85;
}

.develop-import-dialog__secondary:disabled {
    opacity: 0.4;
    cursor: not-allowed;
}
</style>
