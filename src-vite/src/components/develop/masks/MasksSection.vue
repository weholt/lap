<script setup lang="ts">
// Mask tools section for the Develop panel (lap-78d).
//
// Native non-AI masks (brush / linear / radial) through the shared engine:
//   - arming a tool routes the pointer gesture to the central preview
//     overlay; the finished gesture commits exactly one history transaction;
//   - parameter edits (visibility, invert, opacity, combine mode, local
//     exposure) are one transaction each through the editor's history;
//   - masks containing kinds this engine cannot render are surfaced as
//     explicit limitations, never silently dropped (the render path fails
//     naming mask and kind).
// Tool selection and drag state are UI-only and never persisted.

import { computed } from 'vue';
import { useI18n } from 'vue-i18n';

import { useMaskTools } from '@/components/develop/masks/useMaskTools';
import type { MaskContainer } from '@/composables/useDevelopSession.types';
import TButton from '@/components/TButton.vue';
import { IconClose } from '@/common/icons';

const props = defineProps<{
    disabled?: boolean;
}>();

const { t } = useI18n();
const tools = useMaskTools();

const masks = computed<MaskContainer[]>(() => tools.masks());
const activeTool = computed(() => tools.activeTool.value);

const TOOL_BUTTONS: Array<{ kind: 'brush' | 'linear' | 'radial'; labelKey: string; testid: string }> = [
    { kind: 'brush', labelKey: 'develop.masks.brush', testid: 'develop-mask-tool-brush' },
    { kind: 'linear', labelKey: 'develop.masks.linear', testid: 'develop-mask-tool-linear' },
    { kind: 'radial', labelKey: 'develop.masks.radial', testid: 'develop-mask-tool-radial' },
];

const LOCAL_FIELDS: Array<{ field: 'exposure' | 'contrast' | 'saturation' | 'temperature' | 'tint'; labelKey: string; min: number; max: number; step: number }> = [
    { field: 'exposure', labelKey: 'develop.exposure', min: -5, max: 5, step: 0.01 },
    { field: 'contrast', labelKey: 'develop.masks.localContrast', min: -100, max: 100, step: 1 },
    { field: 'saturation', labelKey: 'develop.masks.localSaturation', min: -100, max: 100, step: 1 },
    { field: 'temperature', labelKey: 'develop.temperature', min: -100, max: 100, step: 1 },
    { field: 'tint', labelKey: 'develop.tint', min: -100, max: 100, step: 1 },
];

const MODES: Array<{ value: 'additive' | 'subtractive' | 'intersect'; labelKey: string }> = [
    { value: 'additive', labelKey: 'develop.masks.modeAdd' },
    { value: 'subtractive', labelKey: 'develop.masks.modeSubtract' },
    { value: 'intersect', labelKey: 'develop.masks.modeIntersect' },
];

function onArmTool(kind: 'brush' | 'linear' | 'radial') {
    tools.armTool(tools.activeTool.value === kind ? '' : kind);
}

function maskLabel(index: number, mask: MaskContainer): string {
    return mask.name || `${t('develop.masks.mask')} ${index + 1}`;
}

function unsupportedKinds(mask: MaskContainer): string[] {
    return mask.subMasks
        .filter((sub) => sub.visible && sub.geometry === null)
        .map((sub) => sub.type);
}

function onOpacity(mask: MaskContainer, event: Event) {
    tools.controller.setMaskOpacity(mask.id, Number((event.target as HTMLInputElement).value));
}

function onLocal(mask: MaskContainer, field: (typeof LOCAL_FIELDS)[number]['field'], event: Event) {
    tools.controller.setMaskLocal(mask.id, field, Number((event.target as HTMLInputElement).value));
}

function onMode(mask: MaskContainer, event: Event) {
    const sub = mask.subMasks[0];
    if (sub) {
        tools.controller.setSubMaskMode(mask.id, sub.id, (event.target as HTMLSelectElement).value as 'additive' | 'subtractive' | 'intersect');
    }
}
</script>

<template>
    <div class="px-1 pt-2" data-testid="develop-masks-section">
        <div class="flex items-center gap-1 mb-1">
            <div class="text-[10px] font-bold uppercase tracking-wide text-base-content/40 flex-1">
                {{ $t('develop.masks.title') }}
            </div>
            <button
                v-for="tool in TOOL_BUTTONS"
                :key="tool.kind"
                type="button"
                class="btn btn-ghost btn-xs"
                :class="activeTool === tool.kind ? 'text-primary' : 'text-base-content/50'"
                :data-testid="tool.testid"
                :aria-pressed="activeTool === tool.kind ? 'true' : 'false'"
                :disabled="props.disabled"
                @click.stop="onArmTool(tool.kind)"
            >{{ $t(tool.labelKey) }}</button>
        </div>

        <div
            v-if="activeTool"
            class="px-2 py-1 mb-1 rounded-box bg-info/10 text-info text-[10px]"
            data-testid="develop-mask-tool-hint"
        >{{ $t('develop.masks.toolHint') }}</div>

        <div v-if="masks.length === 0" class="px-2 py-1 text-[10px] text-base-content/30">
            {{ $t('develop.masks.empty') }}
        </div>

        <div
            v-for="(mask, index) in masks"
            :key="mask.id"
            class="mb-1 rounded-box bg-base-300/40"
            :data-testid="`develop-mask-${index}`"
        >
            <div class="flex items-center gap-1 px-2 pt-1">
                <input
                    type="checkbox"
                    class="checkbox checkbox-xs"
                    :data-testid="`develop-mask-${index}-visible`"
                    :checked="mask.visible"
                    @change="tools.controller.setMaskFlag(mask.id, 'visible', ($event.target as HTMLInputElement).checked)"
                />
                <span class="text-xs flex-1 truncate" :class="{ 'line-through opacity-50': !mask.visible }">
                    {{ maskLabel(index, mask) }}
                </span>
                <TButton
                    :icon="IconClose"
                    :tooltip="$t('develop.masks.delete')"
                    :buttonSize="'small'"
                    :data-testid="`develop-mask-${index}-delete`"
                    @click.stop="tools.controller.removeMask(mask.id)"
                />
            </div>

            <div v-if="unsupportedKinds(mask).length" class="px-2 py-1 text-[10px] text-error/80">
                {{ $t('develop.masks.unsupported', { kinds: unsupportedKinds(mask).join(', ') }) }}
            </div>

            <div class="px-2 py-1 grid grid-cols-2 gap-x-2 gap-y-0.5 items-center">
                <label class="text-[10px] text-base-content/40">{{ $t('develop.masks.opacity') }}</label>
                <input
                    type="range"
                    min="0"
                    max="100"
                    step="1"
                    class="range range-xs"
                    :data-testid="`develop-mask-${index}-opacity`"
                    :value="mask.opacity"
                    @change="onOpacity(mask, $event)"
                />
                <label class="text-[10px] text-base-content/40">{{ $t('develop.masks.mode') }}</label>
                <select
                    class="select select-xs"
                    :data-testid="`develop-mask-${index}-mode`"
                    :value="mask.subMasks[0]?.mode ?? 'additive'"
                    @change="onMode(mask, $event)"
                >
                    <option v-for="mode in MODES" :key="mode.value" :value="mode.value">
                        {{ $t(mode.labelKey) }}
                    </option>
                </select>
                <label class="text-[10px] text-base-content/40">{{ $t('develop.masks.invert') }}</label>
                <input
                    type="checkbox"
                    class="checkbox checkbox-xs justify-self-start"
                    :data-testid="`develop-mask-${index}-invert`"
                    :checked="mask.invert"
                    @change="tools.controller.setMaskFlag(mask.id, 'invert', ($event.target as HTMLInputElement).checked)"
                />
            </div>

            <div class="px-2 pb-1 grid grid-cols-2 gap-x-2 gap-y-0.5 items-center">
                <template v-for="local in LOCAL_FIELDS" :key="local.field">
                    <label class="text-[10px] text-base-content/40">{{ $t(local.labelKey) }}</label>
                    <input
                        type="range"
                        :min="local.min"
                        :max="local.max"
                        :step="local.step"
                        class="range range-xs"
                        :data-testid="`develop-mask-${index}-local-${local.field}`"
                        :value="mask.adjustments[local.field]"
                        @change="onLocal(mask, local.field, $event)"
                    />
                </template>
            </div>
        </div>

        <button
            v-if="masks.length > 0"
            type="button"
            class="btn btn-ghost btn-xs w-full text-base-content/50 hover:text-base-content"
            data-testid="develop-masks-reset"
            :disabled="props.disabled"
            @click.stop="tools.controller.resetMasks()"
        >{{ $t('develop.masks.resetAll') }}</button>
    </div>
</template>
