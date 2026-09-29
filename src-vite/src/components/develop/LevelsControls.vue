<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import type { LevelsChannel } from '@/composables/useDevelopSession.types';
import { LEVELS_KEYS, changeLevels, levelsBounds, levelsPosition, levelsFromPosition, levelsHistogram, type LevelsKey, type LevelsColor, type LevelsPixels } from './levels';

const props = defineProps<{ value: LevelsChannel; channel: LevelsColor; pixels?: LevelsPixels | null; disabled?: boolean }>();
const emit = defineEmits<{ live: [value: LevelsChannel]; commit: [value: LevelsChannel]; settle: [] }>();
const { t } = useI18n();
const svg = ref<SVGSVGElement>();
let gesture: { key: LevelsKey; pointer: number; start: LevelsChannel } | null = null;
const histogram = computed(() => levelsHistogram(props.pixels));
const paths = computed(() => {
    const bins = histogram.value;
    const max = Math.max(1, ...bins.red, ...bins.green, ...bins.blue);
    return (['red', 'green', 'blue'] as const).map(channel => ({ channel,
        d: Array.from(bins[channel], (n, i) => `${i ? 'L' : 'M'}${12 + i} ${126 - Math.sqrt(n / max) * 102}`).join(' '),
    }));
});
const visiblePaths = computed(() => paths.value.filter(p => props.channel === 'rgb' || props.channel === p.channel));
const outputKeys: LevelsKey[] = ['outputBlack', 'outputWhite'];
const inputKeys: LevelsKey[] = ['inputBlack', 'midtone', 'inputWhite'];
const label = (key: LevelsKey) => t(`develop.levels.${key}`);
const isOutput = (key: LevelsKey) => key.startsWith('output');
const position = (key: LevelsKey) => 12 + levelsPosition(props.value, key);
function publish(value: LevelsChannel, live: boolean) {
    if (props.disabled || LEVELS_KEYS.every(key => props.value[key] === value[key])) return;
    if (live) emit('live', value); else emit('commit', value);
}
function numeric(e: Event, key: LevelsKey) {
    const input = e.target as HTMLInputElement;
    const value = input.value.trim() ? Number(input.value) : NaN;
    const next = changeLevels(props.value, key, value);
    publish(next, false);
    input.value = String(next[key]);
}
function move(e: PointerEvent) {
    if (!gesture || gesture.pointer !== e.pointerId || !svg.value) return;
    const box = svg.value.getBoundingClientRect();
    if (!box.width) return;
    publish(levelsFromPosition(props.value, gesture.key, (e.clientX - box.left) * 280 / box.width - 12), true);
}
function start(e: PointerEvent, key: LevelsKey) {
    if (props.disabled || e.button !== 0 || gesture) return;
    e.preventDefault();
    (e.currentTarget as SVGElement).focus();
    gesture = { key, pointer: e.pointerId, start: { ...props.value } };
    svg.value?.setPointerCapture?.(e.pointerId);
    move(e);
}
function finish(e?: PointerEvent) {
    if (!gesture || (e && gesture.pointer !== e.pointerId)) return;
    const pointer = gesture.pointer;
    gesture = null;
    if (svg.value?.hasPointerCapture?.(pointer)) svg.value.releasePointerCapture(pointer);
    emit('settle');
}
function cancel(e?: PointerEvent) {
    if (!gesture || (e && gesture.pointer !== e.pointerId)) return;
    emit('live', gesture.start);
    finish(e);
}
function keydown(e: KeyboardEvent, key: LevelsKey) {
    if (props.disabled) return;
    if (e.key === 'Escape' && gesture) { e.preventDefault(); cancel(); return; }
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
    e.preventDefault();
    const [min, max] = levelsBounds(props.value, key);
    let delta = (e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -1 : 1) * (e.shiftKey ? 10 : 1);
    if (key === 'midtone') delta *= -0.01; // Left on the midpoint lightens.
    const value = e.key === 'Home' ? min : e.key === 'End' ? max : props.value[key] + delta;
    publish(changeLevels(props.value, key, value), false);
}
onBeforeUnmount(() => finish());
</script>

<template>
    <div class="levels-controls" :class="{ disabled }" data-testid="levels-controls">
        <div class="levels-numbers output-numbers">
            <label v-for="key in outputKeys" :key="key"><span>{{ label(key) }}</span>
                <input type="number" :data-testid="`levels-number-${key}`" :aria-label="label(key)" :value="value[key]"
                    :min="levelsBounds(value, key)[0]" :max="levelsBounds(value, key)[1]" step="1" :disabled="disabled"
                    @change="numeric($event, key)" @keydown.stop @keydown.enter="($event.target as HTMLInputElement).blur()" />
            </label>
        </div>
        <svg ref="svg" viewBox="0 0 280 150" class="levels-chart" data-testid="levels-chart" :aria-label="t('develop.levels.histogram')"
            @pointermove="move" @pointerup="finish" @pointercancel="cancel" @lostpointercapture="finish">
            <rect x="12" y="22" width="255" height="104" class="chart-background" />
            <path v-for="x in [76,140,204]" :key="x" :d="`M${x} 22V126`" class="grid-line" />
            <path v-for="y in [48,74,100,126]" :key="y" :d="`M12 ${y}H267`" class="grid-line" />
            <path v-for="path in visiblePaths" :key="path.channel" :d="path.d" fill="none" :class="`histogram-${path.channel}`" stroke-width="1.2" />
            <g v-for="key in LEVELS_KEYS" :key="key" class="level-handle" :class="key" role="slider"
                :tabindex="disabled ? -1 : 0" :aria-disabled="!!disabled" :aria-label="label(key)" aria-orientation="horizontal"
                :aria-valuemin="levelsBounds(value, key)[0]" :aria-valuemax="levelsBounds(value, key)[1]" :aria-valuenow="value[key]"
                :data-testid="`levels-handle-${key}`" :transform="`translate(${position(key)},0)`"
                @pointerdown="start($event, key)" @keydown.stop="keydown($event, key)">
                <line y1="22" y2="126" class="handle-guide" />
                <rect x="-11" :y="isOutput(key) ? 0 : 126" width="22" height="24" fill="transparent" />
                <path v-if="isOutput(key)" d="M-5 6H5V15L0 22L-5 15Z" class="handle-knob" />
                <path v-else d="M0 126L5 133V140H-5V133Z" class="handle-knob" />
            </g>
        </svg>
        <div class="levels-numbers input-numbers">
            <label v-for="key in inputKeys" :key="key"><span>{{ label(key) }}</span>
                <input type="number" :data-testid="`levels-number-${key}`" :aria-label="label(key)" :value="value[key]"
                    :min="levelsBounds(value, key)[0]" :max="levelsBounds(value, key)[1]" :step="key === 'midtone' ? 0.01 : 1" :disabled="disabled"
                    @change="numeric($event, key)" @keydown.stop @keydown.enter="($event.target as HTMLInputElement).blur()" />
            </label>
        </div>
        <p class="histogram-label">{{ pixels ? t('develop.levels.histogram') : t('develop.levels.waiting') }}</p>
    </div>
</template>

<style scoped>
.levels-chart { width: 100%; display: block; touch-action: none; user-select: none; overflow: visible; }
.chart-background { fill: #0003; }
.grid-line { stroke: #8883; stroke-width: .6; }
.histogram-red { stroke: #ff777d; }
.histogram-green { stroke: #86ce90; }
.histogram-blue { stroke: #81aaff; }
.level-handle { cursor: ew-resize; outline: none; }
.handle-guide { stroke: #bbb8; stroke-width: .7; pointer-events: none; }
.handle-knob { fill: #242424; stroke: #ccc; stroke-width: 1.2; }
.inputWhite .handle-knob, .outputWhite .handle-knob { fill: #fff; }
.midtone .handle-knob { fill: #888; }
.level-handle:focus-visible .handle-knob, .level-handle:hover .handle-knob { stroke: var(--color-primary, #ff7bc8); stroke-width: 2; }
.levels-numbers { display: flex; justify-content: space-between; gap: 4px; }
.levels-numbers label { display: flex; flex-direction: column; gap: 3px; max-width: 32%; font-size: 9px; color: color-mix(in srgb, currentColor 65%, transparent); }
.output-numbers label:last-child, .input-numbers label:last-child { text-align: right; }
.input-numbers label:nth-child(2) { text-align: center; }
.levels-numbers input { width: 100%; max-width: 65px; padding: 3px 4px; font-size: 12px; text-align: inherit; color: var(--color-base-content); background: #0003; border: 1px solid #8884; border-radius: 3px; font-variant-numeric: tabular-nums; }
.levels-numbers input:focus-visible { outline: 1px solid var(--color-primary, #ff7bc8); }
.histogram-label { font-size: 9px; opacity: .5; margin-top: 8px; text-align: center; }
.disabled { opacity: .5; }
.disabled .level-handle { cursor: default; }
</style>
