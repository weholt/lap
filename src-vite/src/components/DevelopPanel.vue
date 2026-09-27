<script setup lang="ts">
import { computed, onBeforeUnmount, watch } from 'vue';
import { useI18n } from 'vue-i18n';

import {
    useDevelopEditor,
} from '@/composables/useDevelopEditor';
import {
    RECIPE_PARAM_RANGES,
    type Recipe,
} from '@/composables/useDevelopSession.types';
import TButton from '@/components/TButton.vue';
import { IconClose, IconRestore } from '@/common/icons';

/**
 * Native Develop panel (lap-0e9 / TASK-303; spec A10).
 *
 * Exposure and white-balance controls built directly from the generated
 * engine descriptors (RECIPE_PARAM_RANGES). All edits go through the
 * useDevelopEditor orchestration: debounced recipe commits with
 * saving/saved/conflict/failed status, awaited flush on navigation/close,
 * retained dirty state with explicit retry, per-control and global reset,
 * and an explicit original comparison toggle. This panel never writes
 * pixels to the source file.
 */

const props = defineProps<{
    file: Record<string, any> | null;
}>();

const emit = defineEmits<{
    close: [];
}>();

const { t } = useI18n();
const develop = useDevelopEditor();

const CONTROL_FIELDS = ['exposure', 'temperature', 'tint'] as const;
type ControlField = (typeof CONTROL_FIELDS)[number];

const CONTROL_LABEL_KEYS: Record<ControlField, string> = {
    exposure: 'develop.exposure',
    temperature: 'develop.temperature',
    tint: 'develop.tint',
};

const controlValues = computed(() => {
    const recipe = develop.recipe.value;
    const values: Record<ControlField, number> = { exposure: 0, temperature: 0, tint: 0 };
    if (!recipe) return values;
    for (const field of CONTROL_FIELDS) {
        values[field] = Number(recipe[field as keyof Recipe] ?? 0);
    }
    return values;
});

function rangeFor(field: ControlField) {
    return RECIPE_PARAM_RANGES[field];
}

function labelKey(field: ControlField) {
    return CONTROL_LABEL_KEYS[field];
}

function formatValue(field: ControlField): string {
    const range = rangeFor(field);
    const value = controlValues.value[field];
    const decimals = range.step < 1 ? 2 : 0;
    return value.toFixed(decimals);
}

function onSliderInput(field: ControlField, event: Event) {
    const target = event.target as HTMLInputElement;
    develop.setParam(field, Number(target.value));
}

function onNumericKeydown(field: ControlField, event: KeyboardEvent) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    commitNumericInput(field, event.target as HTMLInputElement);
}

function onNumericChange(field: ControlField, event: Event) {
    commitNumericInput(field, event.target as HTMLInputElement);
}

function commitNumericInput(field: ControlField, target: HTMLInputElement) {
    const raw = target.value.trim();
    const range = rangeFor(field);
    if (raw === '') {
        // Empty input restores the current value without touching the recipe.
        target.value = String(controlValues.value[field]);
        return;
    }
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) {
        // Reject non-numeric input: keep the current recipe value.
        target.value = String(controlValues.value[field]);
        return;
    }
    develop.setParam(field, parsed);
    target.value = String(controlValues.value[field]);
}

function resetField(field: ControlField) {
    develop.resetParam(field);
}

function resetAll() {
    develop.resetAll();
}

function toggleOriginal() {
    develop.showOriginal.value = !develop.showOriginal.value;
}

const statusLabel = computed(() => {
    const state = develop.saveState.value;
    if (state === 'idle' || state === 'pending') {
        return state === 'pending' ? t('develop.save.pending') : t('develop.save.idle');
    }
    return t(`develop.save.${state}`);
});

const showRetry = computed(
    () => develop.saveState.value === 'failed' || develop.saveState.value === 'conflict',
);

async function retryCommit() {
    await develop.retry();
}

function requestClose() {
    emit('close');
}

watch(
    () => Number(props.file?.id || 0),
    async (fileId) => {
        if (!fileId) return;
        try {
            await develop.openAsset({ id: fileId });
        } catch {
            // Open failures are surfaced through develop.openError.
        }
    },
    { immediate: true },
);

onBeforeUnmount(() => {
    // Awaited commits happen through the panel-mode switching paths; the
    // composable keeps per-asset state if this panel unmounts unexpectedly.
    void develop.flush().catch(() => {});
});
</script>

<template>
    <div class="w-full h-full rounded-box bg-base-200 flex flex-col overflow-hidden" data-testid="develop-panel">
        <!-- Header & Close -->
        <div class="my-2 px-2 flex items-center w-full shrink-0">
            <div class="flex-1 pl-1">
                <span class="text-sm font-semibold text-primary/70">{{ $t('develop.title') }}</span>
            </div>
            <div class="flex items-center gap-1">
                <TButton
                    :icon="IconClose"
                    :tooltip="$t('msgbox.close')"
                    :buttonSize="'small'"
                    @click.stop="requestClose"
                />
            </div>
        </div>

        <div v-if="file" class="mb-2 px-2 flex-1 overflow-y-auto overflow-x-hidden flex flex-col gap-1">
            <div
                v-if="develop.openError.value"
                class="px-2 py-1.5 rounded-box bg-error/10 text-error text-xs break-words"
                data-testid="develop-open-error"
            >{{ develop.openError.value }}</div>

            <!-- Controls -->
            <template v-for="field in CONTROL_FIELDS" :key="field">
                <div
                    v-if="field === 'temperature'"
                    class="border-t border-base-content/5 px-1 pt-2 pb-0.5"
                >
                    <span class="font-bold uppercase text-[11px] tracking-wide text-base-content/40">
                        {{ $t('develop.whiteBalance') }}
                    </span>
                </div>
                <div
                    class="group/control border-t border-base-content/5 px-1 py-2 space-y-1"
                    :data-testid="`develop-control-${field}`"
                >
                    <div class="flex items-center gap-1 text-xs">
                        <span
                            class="font-bold uppercase tracking-wide text-base-content/30 mr-auto cursor-pointer select-none"
                            :data-testid="`develop-label-${field}`"
                            :title="$t('develop.reset')"
                            @dblclick.stop="resetField(field)"
                        >{{ $t(labelKey(field)) }}</span>
                        <button
                            type="button"
                            class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
                            :data-testid="`develop-reset-${field}`"
                            :title="$t('develop.reset')"
                            :aria-label="$t('develop.reset')"
                            @click.stop="resetField(field)"
                        >
                            <IconRestore class="w-3 h-3" />
                        </button>
                    </div>
                    <div class="flex items-center gap-2">
                        <input
                            type="range"
                            class="range range-primary range-xs flex-1"
                            :min="rangeFor(field).min"
                            :max="rangeFor(field).max"
                            :step="rangeFor(field).step"
                            :value="controlValues[field]"
                            :data-testid="`develop-slider-${field}`"
                            :aria-label="$t(labelKey(field))"
                            @input="onSliderInput(field, $event)"
                        />
                        <input
                            type="number"
                            class="input input-xs input-bordered w-16 text-xs tabular-nums"
                            :min="rangeFor(field).min"
                            :max="rangeFor(field).max"
                            :step="rangeFor(field).step"
                            :value="formatValue(field)"
                            :data-testid="`develop-input-${field}`"
                            :aria-label="$t(labelKey(field))"
                            @keydown="onNumericKeydown(field, $event)"
                            @change="onNumericChange(field, $event)"
                        />
                    </div>
                </div>
            </template>

            <!-- Reset all -->
            <div class="border-t border-base-content/5 px-1 py-2">
                <button
                    type="button"
                    class="btn btn-ghost btn-xs w-full text-base-content/60 hover:text-base-content"
                    data-testid="develop-reset-all"
                    @click.stop="resetAll"
                >
                    <IconRestore class="w-3 h-3" />
                    {{ $t('develop.resetAll') }}
                </button>
            </div>
        </div>

        <!-- Footer: original comparison + save status -->
        <div
            v-if="file"
            class="px-2 pb-2 pt-1 border-t border-base-content/5 shrink-0 flex flex-col gap-1"
        >
            <button
                type="button"
                class="btn btn-ghost btn-xs justify-start text-base-content/70 hover:text-base-content"
                data-testid="develop-view-original"
                :aria-pressed="develop.showOriginal.value ? 'true' : 'false'"
                @click.stop="toggleOriginal"
            >
                {{ develop.showOriginal.value ? $t('develop.viewDeveloped') : $t('develop.viewOriginal') }}
            </button>

            <div class="flex items-center gap-2">
                <span
                    class="text-xs flex-1"
                    :class="{
                        'text-base-content/50': develop.saveState.value === 'idle',
                        'text-warning': develop.saveState.value === 'pending' || develop.saveState.value === 'saving',
                        'text-success': develop.saveState.value === 'saved',
                        'text-error': develop.saveState.value === 'failed' || develop.saveState.value === 'conflict',
                    }"
                    data-testid="develop-save-status"
                    :title="develop.lastError.value || ''"
                >{{ statusLabel }}</span>
                <button
                    v-if="showRetry"
                    type="button"
                    class="btn btn-xs btn-warning"
                    data-testid="develop-retry"
                    @click.stop="retryCommit"
                >{{ $t('develop.retry') }}</button>
            </div>
            <div
                v-if="develop.lastError.value && showRetry"
                class="text-[10px] text-error/80 break-words"
                data-testid="develop-save-error"
            >{{ develop.lastError.value }}</div>
        </div>
    </div>
</template>
