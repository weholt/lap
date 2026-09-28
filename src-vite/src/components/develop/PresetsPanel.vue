<script lang="ts">
// Preset hover/apply contract surface is re-exported so hosts and tests
// consume one import (lap-62b / TASK-405, spec A9).
import { PRESET_HOVER_PREVIEW_DELAY_MS } from '@/composables/useDevelopClipboard';

export { PRESET_HOVER_PREVIEW_DELAY_MS };
export type { DevelopPreset } from '@/composables/useDevelopClipboard.types';
</script>

<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue';

import {
    pasteSections,
    PRESET_HOVER_PREVIEW_DELAY_MS,
} from '@/composables/useDevelopClipboard';
import type { DevelopPreset } from '@/composables/useDevelopClipboard.types';
import type { Recipe } from '@/composables/useDevelopSession.types';

/**
 * Reusable-presets panel (lap-62b / TASK-405; spec A9 "Preset hover is
 * transient, cancel restores prior state and applying a preset commits one
 * transaction").
 *
 * Transaction contract, enforced here and proven with fake-timer component
 * tests:
 *   - Hovering renders nothing until `PRESET_HOVER_PREVIEW_DELAY_MS` has
 *     elapsed; leaving earlier never emits anything at all.
 *   - A fired hover emits `preview` ONCE with the merged recipe. The panel
 *     never writes to stores and never invokes IPC, so hovering can never
 *     save (schema.md: hover presets are transient, not persisted).
 *   - Leaving after a preview emits `preview-cancel` once, carrying the
 *     EXACT prior recipe for the host to restore.
 *   - Clicking emits `apply` exactly once with the merged recipe; the host
 *     applies it through its one-transaction patch API. An active hover
 *     preview is superseded silently (the applied recipe replaces it).
 *
 * The panel is deliberately presentational: preset payloads must already be
 * validated (parseClipboardPayload), and merging uses the same
 * pasteSections primitive as selective paste.
 */

const props = defineProps<{
    /** Validated reusable presets. */
    presets: DevelopPreset[];
    /** The editor's current working recipe; null disables all interaction. */
    recipe: Recipe | null;
}>();

const emit = defineEmits<{
    /** Transient preview: [merged recipe, preset id]. */
    preview: [recipe: Recipe, presetId: string];
    /** Hover cancelled: [exact prior recipe to restore]. */
    'preview-cancel': [restored: Recipe];
    /** One-transaction apply: [merged recipe, preset id]. */
    apply: [recipe: Recipe, presetId: string];
}>();

const hoveringId = ref<string | null>(null);
let hoverTimer: ReturnType<typeof setTimeout> | null = null;
let pendingRestore: Recipe | null = null;

function clone(recipe: Recipe): Recipe {
    return JSON.parse(JSON.stringify(recipe)) as Recipe;
}

function clearTimer(): void {
    if (hoverTimer !== null) {
        clearTimeout(hoverTimer);
        hoverTimer = null;
    }
}

/** Ends the hover state; emits preview-cancel when a preview was showing. */
function cancelActivePreview(): void {
    clearTimer();
    if (hoveringId.value !== null && pendingRestore) {
        emit('preview-cancel', pendingRestore);
    }
    hoveringId.value = null;
    pendingRestore = null;
}

function enter(preset: DevelopPreset): void {
    if (!props.recipe) return;
    cancelActivePreview();
    hoverTimer = setTimeout(() => {
        hoverTimer = null;
        if (!props.recipe) return;
        pendingRestore = clone(props.recipe);
        hoveringId.value = preset.id;
        emit('preview', pasteSections(preset.payload, props.recipe), preset.id);
    }, PRESET_HOVER_PREVIEW_DELAY_MS);
}

function leave(): void {
    cancelActivePreview();
}

function apply(preset: DevelopPreset): void {
    // A pending (never-fired) hover simply never happened.
    clearTimer();
    // An active preview is superseded by the apply, not cancelled: the
    // applied recipe replaces the displayed transient state.
    hoveringId.value = null;
    pendingRestore = null;
    if (!props.recipe) return;
    emit('apply', pasteSections(preset.payload, props.recipe), preset.id);
}

/** Test/diagnostic hook: reset the hover state machine without events. */
function resetHover(): void {
    clearTimer();
    hoveringId.value = null;
    pendingRestore = null;
}

onBeforeUnmount(resetHover);

defineExpose({ resetHover });
</script>

<template>
    <div class="flex flex-col gap-1" data-testid="presets-panel">
        <button
            v-for="preset in presets"
            :key="preset.id"
            type="button"
            class="btn btn-ghost btn-sm justify-start text-left min-w-0"
            :data-testid="`preset-${preset.id}`"
            :class="{ 'btn-active': hoveringId === preset.id }"
            :title="preset.name"
            :aria-label="preset.name"
            @mouseenter="enter(preset)"
            @mouseleave="leave()"
            @click="apply(preset)"
        >
            <span class="truncate">{{ preset.name }}</span>
        </button>
    </div>
</template>
