<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue';
import { useI18n } from 'vue-i18n';

import { useDevelopEditor } from '@/composables/useDevelopEditor';
import {
    PARAMETRIC_CURVE_SLIDERS,
    buildParametricPoints,
    clampParametricValue,
    curveSvgPath,
} from '@/components/develop/controls';
import type { CurvePoint } from '@/composables/useDevelopSession.types';

/**
 * Point/parametric curve editor (lap-adc). Mirrors the reference editor's
 * interaction semantics: per-channel point curves with draggable/focusable
 * points (one undo transaction per gesture or keystroke), and a parametric
 * mode whose sliders re-render the active channel's curve. The renderer
 * consumes recipe.curves; mode switches swap curves/pointCurves exactly like
 * the reference implementation.
 */
const editor = useDevelopEditor();
const { t } = useI18n();

type CurveChannel = 'luma' | 'red' | 'green' | 'blue';

const CHANNELS: CurveChannel[] = ['luma', 'red', 'green', 'blue'];
const SNAP_THRESHOLD = 5;

const activeChannel = ref<CurveChannel>('luma');
const selectedPoint = ref<number | null>(null);
const surfaceRef = ref<HTMLElement | null>(null);
let dragIndex: number | null = null;

const mode = computed(() => editor.recipe.value?.curveMode || 'point');
const points = computed<CurvePoint[]>(
    () => editor.recipe.value?.curves?.[activeChannel.value] || [
        { x: 0, y: 0 },
        { x: 255, y: 255 },
    ],
);
const parametricSettings = computed(() => editor.recipe.value?.parametricCurve?.[activeChannel.value]);
const displayPoints = computed<CurvePoint[]>(() =>
    mode.value === 'parametric' && parametricSettings.value
        ? buildParametricPoints(parametricSettings.value)
        : points.value,
);
const path = computed(() => curveSvgPath(displayPoints.value));

function channelLabelKey(channel: CurveChannel) {
    return `develop.curve.channels.${channel}`;
}

function pointAria(index: number): string {
    return t('develop.curve.pointAria', {
        index: index + 1,
        total: points.value.length,
        channel: t(channelLabelKey(activeChannel.value)),
    });
}

function pointLeft(point: CurvePoint): string {
    return `${(Math.min(255, Math.max(0, point.x)) / 255) * 100}%`;
}

function pointTop(point: CurvePoint): string {
    return `${(1 - Math.min(255, Math.max(0, point.y)) / 255) * 100}%`;
}

function movePoint(index: number, dx: number, dy: number) {
    const pts = points.value.map((p) => ({ ...p }));
    const point = pts[index];
    if (!point) return;
    const minX = index === 0 ? 0 : pts[index - 1].x + 0.01;
    const maxX = index === pts.length - 1 ? 255 : pts[index + 1].x - 0.01;
    pts[index] = {
        x: Math.min(maxX, Math.max(minX, point.x + dx)),
        y: Math.min(255, Math.max(0, point.y + dy)),
    };
    selectedPoint.value = index;
    // One committed transaction per keystroke.
    editor.setCurveChannelPoints(activeChannel.value, pts);
}

function removePoint(index: number) {
    const pts = points.value.map((p) => ({ ...p }));
    if (index <= 0 || index >= pts.length - 1) return;
    pts.splice(index, 1);
    selectedPoint.value = null;
    editor.setCurveChannelPoints(activeChannel.value, pts);
}

function onPointKeydown(event: KeyboardEvent, index: number) {
    const step = event.shiftKey ? 10 : 1;
    switch (event.key) {
        case 'ArrowLeft': movePoint(index, -step, 0); break;
        case 'ArrowRight': movePoint(index, step, 0); break;
        case 'ArrowUp': movePoint(index, 0, step); break;
        case 'ArrowDown': movePoint(index, 0, -step); break;
        case 'Delete':
        case 'Backspace': removePoint(index); break;
        default: return;
    }
    event.preventDefault();
    event.stopPropagation();
}

function onPointPointerDown(event: PointerEvent, index: number) {
    if (mode.value === 'parametric') return;
    if (event.button !== 0) return;
    dragIndex = index;
    selectedPoint.value = index;
    editor.beginEditTransaction(`curve ${activeChannel.value} drag`);
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
}

function onSurfacePointerDown(event: PointerEvent) {
    if (mode.value === 'parametric') return;
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (target && target.closest('[data-curve-point]')) return;
    const container = surfaceRef.value;
    if (!container) return;
    const rect = container.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const x = Math.min(255, Math.max(0, ((event.clientX - rect.left) / rect.width) * 255));
    const y = Math.min(255, Math.max(0, 255 - ((event.clientY - rect.top) / rect.height) * 255));
    const pts = [...points.value, { x, y }].sort((a, b) => a.x - b.x);
    const newIndex = pts.findIndex((p) => p.x === x && p.y === y);
    dragIndex = newIndex;
    selectedPoint.value = newIndex;
    editor.beginEditTransaction(`curve ${activeChannel.value} drag`);
    editor.setCurveChannelPointsLive(activeChannel.value, pts);
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
}

function onPointerMove(event: PointerEvent) {
    if (dragIndex === null) return;
    const container = surfaceRef.value;
    if (!container) return;
    const rect = container.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    let x = ((event.clientX - rect.left) / rect.width) * 255;
    const y = 255 - ((event.clientY - rect.top) / rect.height) * 255;
    if (x < SNAP_THRESHOLD) x = 0;
    if (x > 255 - SNAP_THRESHOLD) x = 255;
    const pts = points.value.map((p) => ({ ...p }));
    const index = dragIndex;
    const point = pts[index];
    if (!point) return;
    const minX = index === 0 ? 0 : pts[index - 1].x + 0.01;
    const maxX = index === pts.length - 1 ? 255 : pts[index + 1].x - 0.01;
    pts[index] = {
        x: Math.min(maxX, Math.max(minX, Math.min(255, Math.max(0, x)))),
        y: Math.min(255, Math.max(0, y)),
    };
    editor.setCurveChannelPointsLive(activeChannel.value, pts);
}

function onPointerUp() {
    if (dragIndex === null) return;
    dragIndex = null;
    editor.endEditTransaction();
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
}

function onParamLive(key: string, event: Event) {
    const value = clampParametricValue(key, Number((event.target as HTMLInputElement).value));
    editor.setParametricCurveValueLive(activeChannel.value, key, value);
}

function onParamCommit(key: string, event: Event) {
    const value = clampParametricValue(key, Number((event.target as HTMLInputElement).value));
    editor.setParametricCurveValue(activeChannel.value, key, value);
}

onBeforeUnmount(() => {
    if (dragIndex !== null) {
        dragIndex = null;
        editor.endEditTransaction();
    }
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
});
</script>

<template>
    <div class="px-1 py-2 space-y-2" data-testid="develop-curve-editor">
        <!-- Mode + channel selectors -->
        <div class="flex items-center gap-1">
            <button
                type="button"
                class="btn btn-ghost btn-xs"
                :class="mode === 'point' ? 'text-primary' : 'text-base-content/50'"
                data-testid="develop-curve-mode-point"
                :aria-label="$t('develop.curve.pointLabel')"
                :aria-pressed="mode === 'point' ? 'true' : 'false'"
                :title="$t('develop.curve.pointLabel')"
                @click.stop="editor.setCurveMode('point')"
            >{{ $t('develop.curve.point') }}</button>
            <button
                type="button"
                class="btn btn-ghost btn-xs"
                :class="mode === 'parametric' ? 'text-primary' : 'text-base-content/50'"
                data-testid="develop-curve-mode-parametric"
                :aria-label="$t('develop.curve.paramLabel')"
                :aria-pressed="mode === 'parametric' ? 'true' : 'false'"
                :title="$t('develop.curve.paramLabel')"
                @click.stop="editor.setCurveMode('parametric')"
            >{{ $t('develop.curve.parametric') }}</button>
            <div class="ml-auto flex items-center gap-0.5">
                <button
                    v-for="channel in CHANNELS"
                    :key="channel"
                    type="button"
                    class="btn btn-ghost btn-xs uppercase"
                    :class="activeChannel === channel ? 'text-primary' : 'text-base-content/50'"
                    :data-testid="`develop-curve-channel-${channel}`"
                    :aria-label="$t(channelLabelKey(channel))"
                    :aria-pressed="activeChannel === channel ? 'true' : 'false'"
                    @click.stop="activeChannel = channel; selectedPoint = null"
                >{{ $t(channelLabelKey(channel)) }}</button>
            </div>
        </div>

        <!-- Curve surface -->
        <div
            ref="surfaceRef"
            class="relative w-full aspect-square rounded-box bg-base-100/60 border border-base-content/10 select-none touch-none"
            data-testid="develop-curve-surface"
            @pointerdown="onSurfacePointerDown"
        >
            <svg viewBox="0 0 255 255" class="absolute inset-0 h-full w-full" preserveAspectRatio="none">
                <line x1="0" y1="255" x2="255" y2="0" class="text-base-content/15" stroke="currentColor" stroke-width="1" stroke-dasharray="4 4" />
                <path :d="path" fill="none" class="text-primary" stroke="currentColor" stroke-width="1.5" />
            </svg>
            <template v-if="mode === 'point'">
                <button
                    v-for="(point, index) in points"
                    :key="`${activeChannel}-${index}`"
                    type="button"
                    role="slider"
                    class="absolute w-3 h-3 -ml-1.5 -mt-1.5 rounded-full border border-primary bg-primary/80 hover:bg-primary focus:outline focus:outline-2 focus:outline-primary/60"
                    :class="selectedPoint === index ? 'ring-2 ring-primary/60' : ''"
                    :style="{ left: pointLeft(point), top: pointTop(point) }"
                    :data-testid="`develop-curve-point-${index}`"
                    :data-curve-point="true"
                    :aria-label="pointAria(index)"
                    :aria-valuemin="0"
                    :aria-valuemax="255"
                    :aria-valuenow="Math.round(point.x)"
                    tabindex="0"
                    @pointerdown="onPointPointerDown($event, index)"
                    @focus="selectedPoint = index"
                    @keydown="onPointKeydown($event, index)"
                ></button>
            </template>
        </div>
        <div class="text-[10px] text-base-content/40">{{ $t('develop.curve.undoHint') }}</div>

        <!-- Parametric sliders -->
        <div v-if="mode === 'parametric'" class="space-y-0">
            <div
                v-for="slider in PARAMETRIC_CURVE_SLIDERS"
                :key="slider.key"
                class="border-t border-base-content/5 px-1 py-2 space-y-1"
                :data-testid="`develop-curve-param-control-${slider.key}`"
            >
                <div class="flex items-center gap-1 text-xs">
                    <span class="font-bold uppercase tracking-wide text-base-content/30 mr-auto">
                        {{ $t(`develop.curve.params.${slider.key}`) }}
                    </span>
                </div>
                <div class="flex items-center gap-2">
                    <input
                        type="range"
                        class="range range-primary range-xs flex-1"
                        :min="slider.min"
                        :max="slider.max"
                        step="1"
                        :value="parametricSettings?.[slider.key] ?? 0"
                        :data-testid="`develop-curve-param-${slider.key}`"
                        :aria-label="$t(`develop.curve.params.${slider.key}`)"
                        @input="onParamLive(slider.key, $event)"
                        @change="editor.endEditTransaction()"
                    />
                    <input
                        type="number"
                        class="input input-xs input-bordered w-16 text-xs tabular-nums"
                        :min="slider.min"
                        :max="slider.max"
                        step="1"
                        :value="Math.round(parametricSettings?.[slider.key] ?? 0)"
                        :data-testid="`develop-curve-param-input-${slider.key}`"
                        :aria-label="$t(`develop.curve.params.${slider.key}`)"
                        @change="onParamCommit(slider.key, $event)"
                    />
                </div>
            </div>
        </div>
    </div>
</template>
