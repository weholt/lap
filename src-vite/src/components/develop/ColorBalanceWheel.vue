<script setup lang="ts">
import { computed, onBeforeUnmount, ref, useId } from 'vue';
import { useI18n } from 'vue-i18n';
import type { HueSatLum } from '@/composables/useDevelopSession.types';

const props = defineProps<{ value: HueSatLum; zone: string; label: string; master?: boolean; disabled?: boolean }>();
const emit = defineEmits<{ live: [value: HueSatLum]; settle: []; commit: [value: HueSatLum]; reset: [] }>();
const { t } = useI18n();
const svg = ref<SVGSVGElement>();
const id = useId();
type Control = 'puck' | 'hue' | 'saturation' | 'luminance';
let gesture: { control: Control; pointer: number; start: HueSatLum } | null = null;
const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));
const wrap = (h: number) => ((h % 360) + 360) % 360;
const rounded = (n: number) => Math.round(n * 10) / 10;
function point(angle: number, radius: number) {
    const r = angle * Math.PI / 180;
    return { x: 160 + Math.cos(r) * radius, y: 120 - Math.sin(r) * radius };
}
function arc(from: number, to: number, radius: number) {
    const a = point(from, radius), b = point(to, radius);
    return `M ${a.x} ${a.y} A ${radius} ${radius} 0 0 0 ${b.x} ${b.y}`;
}
const ring = Array.from({ length: 90 }, (_, i) => ({ d: arc(i * 4, i * 4 + 4.5, 100), color: `hsl(${i * 4} 100% 50%)` }));
const hue = computed(() => wrap(props.value.hue));
const tint = computed(() => `hsl(${hue.value} 100% 60%)`);
// Signed legacy saturation is retained until explicitly edited.
const puck = computed(() => point(hue.value, props.value.saturation));
const hueTick = computed(() => ({ a: point(hue.value, 101), b: point(hue.value, 109) }));
const satMin = computed(() => props.value.saturation < 0 ? -100 : 0);
const satAngle = computed(() => 220 - (props.value.saturation - satMin.value) / (100 - satMin.value) * 80);
function tick(angle: number) { return { a: point(angle, 131), b: point(angle, 141) }; }
const satTick = computed(() => tick(satAngle.value));
const lumTick = computed(() => tick(props.value.luminance * 0.4));
const valuesText = computed(() => `${t('develop.hslComponents.hue')} ${rounded(hue.value)}°, ${t('develop.hslComponents.saturation')} ${rounded(props.value.saturation)}%`);

function publish(patch: Partial<HueSatLum>, live: boolean) {
    if (props.disabled) return;
    const value = { ...props.value, ...patch };
    if (value.hue === props.value.hue && value.saturation === props.value.saturation && value.luminance === props.value.luminance) return;
    if (live) emit('live', value); else emit('commit', value);
}
function move(e: PointerEvent) {
    if (!gesture || gesture.pointer !== e.pointerId || !svg.value) return;
    const rect = svg.value.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const x = (e.clientX - rect.left) * 320 / rect.width - 160;
    const y = 120 - (e.clientY - rect.top) * 240 / rect.height;
    const angle = wrap(Math.atan2(y, x) * 180 / Math.PI);
    const control = gesture.control;
    if (control === 'puck') {
        const saturation = rounded(clamp(Math.hypot(x, y), 0, 100));
        publish({ hue: saturation < 0.5 ? props.value.hue : wrap(rounded(angle)), saturation }, true);
    } else if (control === 'hue') {
        publish({ hue: wrap(rounded(angle)) }, true);
    } else if (control === 'saturation') {
        // Project onto the left arc, clamping beyond its endpoints.
        const a = Math.atan2(y, -Math.abs(x)) * 180 / Math.PI;
        const leftAngle = a < 0 ? a + 360 : a;
        publish({ saturation: rounded(satMin.value + clamp((220 - leftAngle) / 80, 0, 1) * (100 - satMin.value)) }, true);
    } else {
        const a = Math.atan2(y, Math.abs(x)) * 180 / Math.PI;
        publish({ luminance: rounded(clamp(a / 0.4, -100, 100)) }, true);
    }
}
function start(e: PointerEvent, control: Control) {
    if (props.disabled || e.button !== 0 || gesture || (props.master && control === 'luminance')) return;
    e.preventDefault();
    (e.currentTarget as SVGElement).focus();
    gesture = { control, pointer: e.pointerId, start: { ...props.value } };
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
function key(e: KeyboardEvent, control: Control) {
    if (props.disabled || (props.master && control === 'luminance')) return;
    if (e.key === 'Escape' && gesture) {
        e.preventDefault();
        emit('live', gesture.start);
        finish();
        return;
    }
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return;
    e.preventDefault();
    const amount = (e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -1 : 1) * (e.shiftKey ? 10 : 1);
    const component = control === 'puck' ? (['ArrowLeft', 'ArrowRight'].includes(e.key) ? 'hue' : 'saturation') : control;
    const min = component === 'luminance' ? -100 : component === 'saturation' ? satMin.value : 0;
    const max = component === 'hue' ? 360 : 100;
    const value = e.key === 'Home' ? min : e.key === 'End' ? max : props.value[component] + amount;
    publish({ [component]: component === 'hue' ? wrap(value) : clamp(value, min, max) }, false);
}
function numericKey(e: KeyboardEvent, component: keyof HueSatLum) {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    numeric(e, component);
}
function numeric(e: Event, component: keyof HueSatLum) {
    const target = e.target as HTMLInputElement;
    const value = target.value.trim() === '' ? NaN : Number(target.value);
    if (Number.isFinite(value)) {
        const min = component === 'hue' ? 0 : component === 'saturation' ? satMin.value : -100;
        publish({ [component]: clamp(value, min, component === 'hue' ? 360 : 100) }, false);
    }
    target.value = String(component === 'hue' ? hue.value : props.value[component]);
}
onBeforeUnmount(() => finish());
</script>

<template>
    <div class="balance-wheel" :data-testid="`develop-balance-wheel-${zone}`">
        <svg ref="svg" viewBox="0 0 320 240" :aria-label="label" role="group"
            @pointermove="move" @pointerup="finish" @pointercancel="finish" @lostpointercapture="finish">
            <defs>
                <linearGradient :id="`${id}-sat`" x1="0" y1="1" x2="0" y2="0">
                    <stop offset="0" stop-color="#ddd" /><stop offset="1" :stop-color="tint" />
                </linearGradient>
                <linearGradient :id="`${id}-lum`" x1="0" y1="1" x2="0" y2="0">
                    <stop offset="0" stop-color="#111" /><stop offset="1" stop-color="#eee" />
                </linearGradient>
            </defs>
            <g aria-hidden="true" fill="none" stroke-width="3">
                <path v-for="(segment, index) in ring" :key="index" :d="segment.d" :stroke="segment.color" />
                <line :x1="hueTick.a.x" :y1="hueTick.a.y" :x2="hueTick.b.x" :y2="hueTick.b.y" :stroke="tint" />
                <path :d="arc(140, 220, 136)" :stroke="`url(#${id}-sat)`" />
                <path :d="arc(-40, 40, 136)" :stroke="master ? '#555' : `url(#${id}-lum)`" :opacity="master ? 0.4 : 1" />
                <line :x1="satTick.a.x" :y1="satTick.a.y" :x2="satTick.b.x" :y2="satTick.b.y" stroke="#eee" />
                <line v-if="!master" :x1="lumTick.a.x" :y1="lumTick.a.y" :x2="lumTick.b.x" :y2="lumTick.b.y" stroke="#eee" />
                <circle cx="160" cy="120" r="1" stroke="currentColor" opacity="0.4" />
                <circle :cx="puck.x" :cy="puck.y" r="6" :fill="Math.abs(value.saturation) > 0 ? tint : 'none'" stroke="#ddd" stroke-width="1.3" />
            </g>
            <circle class="wheel-hit" data-control="puck" cx="160" cy="120" r="92" fill="transparent"
                role="slider" :tabindex="disabled ? -1 : 0" :aria-label="`${label}: ${t('develop.colorBalance.tint')}`"
                :aria-valuemin="0" :aria-valuemax="100" :aria-valuenow="value.saturation" :aria-valuetext="valuesText"
                :aria-disabled="disabled || false" :aria-description="t('develop.colorBalance.help')"
                @pointerdown="start($event, 'puck')" @keydown.stop="key($event, 'puck')"
                @dblclick.stop="publish({ saturation: 0 }, false)" />
            <circle class="wheel-hit" data-control="hue" cx="160" cy="120" r="100" fill="none" stroke="transparent" stroke-width="16"
                role="slider" :tabindex="disabled ? -1 : 0" :aria-label="`${label}: ${t('develop.hslComponents.hue')}`"
                aria-valuemin="0" aria-valuemax="360" :aria-valuenow="hue" :aria-disabled="disabled || false"
                @pointerdown="start($event, 'hue')" @keydown.stop="key($event, 'hue')" />
            <path class="wheel-hit" data-control="saturation" :d="arc(140, 220, 136)" fill="none" stroke="transparent" stroke-width="20"
                role="slider" :tabindex="disabled ? -1 : 0" :aria-label="`${label}: ${t('develop.hslComponents.saturation')}`"
                :aria-valuemin="satMin" aria-valuemax="100" :aria-valuenow="value.saturation" :aria-disabled="disabled || false"
                @pointerdown="start($event, 'saturation')" @keydown.stop="key($event, 'saturation')" />
            <path class="wheel-hit" data-control="luminance" :d="arc(-40, 40, 136)" fill="none" stroke="transparent" stroke-width="20"
                role="slider" :tabindex="master || disabled ? -1 : 0" :aria-label="`${label}: ${t('develop.hslComponents.luminance')}`"
                aria-valuemin="-100" aria-valuemax="100" :aria-valuenow="value.luminance" :aria-disabled="master || disabled || false"
                @pointerdown="start($event, 'luminance')" @keydown.stop="key($event, 'luminance')" />
        </svg>
        <div class="balance-values">
            <label v-for="component in (['hue', 'saturation', 'luminance'] as const)" :key="component">
                <span>{{ t(`develop.hslComponents.${component}`) }}</span>
                <input type="number" step="0.1" :min="component === 'luminance' ? -100 : component === 'saturation' ? satMin : 0"
                    :max="component === 'hue' ? 360 : 100" :value="rounded(component === 'hue' ? hue : value[component])"
                    :disabled="disabled || (master && component === 'luminance')"
                    :aria-label="`${label}: ${t(`develop.hslComponents.${component}`)}`"
                    :data-testid="`develop-input-grading-${zone}-${component}`"
                    @keydown.stop="numericKey($event, component)" @change="numeric($event, component)" />
            </label>
        </div>
    </div>
</template>

<style scoped>
.balance-wheel { min-width: 0; }
svg { display: block; width: 100%; height: auto; touch-action: none; user-select: none; }
.wheel-hit { cursor: crosshair; outline: none; }
.wheel-hit:focus-visible { stroke: var(--color-primary, #ff7bc8); stroke-width: 2; stroke-dasharray: 3 3; }
.wheel-hit[aria-disabled='true'] { cursor: default; }
.balance-values { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 7px; }
.balance-values label { display: flex; flex-direction: column; gap: 3px; min-width: 0; font-size: 10px; opacity: .8; }
.balance-values input { width: 100%; min-width: 0; background: transparent; border: 1px solid #8885; border-radius: 5px; padding: 4px; font-size: 11px; font-variant-numeric: tabular-nums; }
.balance-values input:focus-visible { outline: 1px solid var(--color-primary, #ff7bc8); }
.balance-values input:disabled { opacity: .35; }
</style>
