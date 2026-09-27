<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import { IconRestore } from '@/common/icons';

/**
 * One scalar develop control (lap-adc): localized label with double-click
 * reset, reset button, range slider and numeric input.
 *
 * Interaction → transaction mapping (spec A9):
 *   - `live` fires per range `input` while a gesture is running and is
 *     coalesced by the editor into one history transaction;
 *   - `settle` fires on range `change` (gesture end) and closes exactly one
 *     transaction;
 *   - `commit` fires once per keyboard/numeric edit (implicit transaction).
 */
const props = defineProps<{
    labelKey: string;
    min: number;
    max: number;
    step: number;
    value: number;
    testid: string;
    disabled?: boolean;
}>();

const emit = defineEmits<{
    live: [value: number];
    settle: [];
    commit: [value: number];
    reset: [];
}>();

const { t } = useI18n();

const decimals = computed(() => (props.step < 1 ? 2 : 0));

function formatValue(): string {
    return Number(props.value ?? 0).toFixed(decimals.value);
}

function onInput(event: Event) {
    emit('live', Number((event.target as HTMLInputElement).value));
}

function onNumericKeydown(event: KeyboardEvent) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    commitNumeric(event.target as HTMLInputElement);
}

function onNumericChange(event: Event) {
    commitNumeric(event.target as HTMLInputElement);
}

function commitNumeric(target: HTMLInputElement) {
    const raw = target.value.trim();
    const parsed = Number(raw);
    if (raw === '' || !Number.isFinite(parsed)) {
        // Empty or non-numeric input restores the current recipe value.
        target.value = String(props.value);
        return;
    }
    emit('commit', Math.min(props.max, Math.max(props.min, parsed)));
    target.value = String(props.value);
}
</script>

<template>
    <div
        class="group/control border-t border-base-content/5 px-1 py-2 space-y-1"
        :data-testid="`develop-control-${testid}`"
    >
        <div class="flex items-center gap-1 text-xs">
            <span
                class="font-bold uppercase tracking-wide text-base-content/30 mr-auto cursor-pointer select-none"
                :data-testid="`develop-label-${testid}`"
                :title="$t('develop.reset')"
                @dblclick.stop="emit('reset')"
            >{{ $t(labelKey) }}</span>
            <button
                type="button"
                class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
                :data-testid="`develop-reset-${testid}`"
                :title="$t('develop.reset')"
                :aria-label="$t('develop.reset')"
                :disabled="disabled"
                @click.stop="emit('reset')"
            >
                <IconRestore class="w-3 h-3" />
            </button>
        </div>
        <div class="flex items-center gap-2">
            <input
                type="range"
                class="range range-primary range-xs flex-1"
                :min="min"
                :max="max"
                :step="step"
                :value="value"
                :data-testid="`develop-slider-${testid}`"
                :aria-label="$t(labelKey)"
                :disabled="disabled"
                @input="onInput"
                @change="emit('settle')"
            />
            <input
                type="number"
                class="input input-xs input-bordered w-16 text-xs tabular-nums"
                :min="min"
                :max="max"
                :step="step"
                :value="formatValue()"
                :data-testid="`develop-input-${testid}`"
                :aria-label="$t(labelKey)"
                :disabled="disabled"
                @keydown="onNumericKeydown"
                @change="onNumericChange"
            />
        </div>
    </div>
</template>
