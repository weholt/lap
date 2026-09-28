<script setup lang="ts">
// Pointer overlay for the native mask tools over the develop central
// preview (lap-78d).
//
// The overlay mirrors the displayed preview canvas box. Pointer events are
// normalized against that box (output space) and mapped into the oriented,
// un-cropped frame by the controller (pure crop-offset shift). A full
// gesture — pointerdown → moves → pointerup — forms exactly one history
// transaction; Escape cancels with an exact restore. The overlay draws
// simple shape outlines for feedback; rendering itself happens in the
// engine, never here.

import { computed, onBeforeUnmount, onMounted, ref } from 'vue';

import { orientedToOutput } from '@/components/develop/masks/maskModel';
import { useMaskTools } from '@/components/develop/masks/useMaskTools';

const props = defineProps<{
    /** The displayed preview canvas; the overlay matches its box. */
    target: HTMLCanvasElement | null;
}>();

const tools = useMaskTools();
const rootRef = ref<HTMLDivElement | null>(null);
const box = ref<{ left: number; top: number; width: number; height: number } | null>(null);
const livePoints = ref<Array<{ x: number; y: number }>>([]);

const enabled = computed(() => tools.activeTool.value !== '' && props.target !== null);

function recomputeBox() {
    const canvas = props.target;
    if (!canvas) {
        box.value = null;
        return;
    }
    const rect = canvas.getBoundingClientRect();
    box.value = { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

function normalize(event: PointerEvent): { x: number; y: number } {
    const b = box.value;
    if (!b || b.width === 0 || b.height === 0) return { x: 0, y: 0 };
    return {
        x: (event.clientX - b.left) / b.width,
        y: (event.clientY - b.top) / b.height,
    };
}

function onPointerDown(event: PointerEvent) {
    if (!enabled.value) return;
    recomputeBox();
    const point = normalize(event);
    (event.target as HTMLElement).setPointerCapture(event.pointerId);
    tools.controller.startGesture(tools.activeTool.value as 'brush' | 'linear' | 'radial', point);
    livePoints.value = [point];
}

function onPointerMove(event: PointerEvent) {
    if (!tools.controller.gestureActive) return;
    const point = normalize(event);
    tools.controller.extendGesture(point);
    livePoints.value = [...livePoints.value, point];
}

function onPointerUp(event: PointerEvent) {
    if (!tools.controller.gestureActive) return;
    tools.controller.completeGesture(normalize(event));
    livePoints.value = [];
    tools.armTool('');
}

function onPointerCancel() {
    if (!tools.controller.gestureActive) return;
    tools.controller.cancelGesture();
    livePoints.value = [];
    tools.armTool('');
}

function onKeyDown(event: KeyboardEvent) {
    if (event.key === 'Escape') onPointerCancel();
}

/** Outline geometry for the last visible mask, in output space. */
const outlines = computed(() => {
    const crop = tools.currentCrop();
    const toOutput = (x: number, y: number) => orientedToOutput(x, y, crop);
    const shapes: Array<{ kind: string; points: string; ellipse?: { cx: number; cy: number; rx: number; ry: number } }> = [];
    for (const mask of tools.masks()) {
        if (!mask.visible) continue;
        const sub = mask.subMasks[0];
        const geometry = sub?.geometry;
        if (!geometry) continue;
        if (geometry.type === 'radial') {
            const c = toOutput(geometry.centerX, geometry.centerY);
            shapes.push({
                kind: 'radial',
                points: '',
                ellipse: { cx: c.x, cy: c.y, rx: geometry.radiusX, ry: geometry.radiusY },
            });
        } else if (geometry.type === 'linear') {
            const a = toOutput(geometry.startX, geometry.startY);
            const b = toOutput(geometry.endX, geometry.endY);
            shapes.push({ kind: 'linear', points: `${a.x},${a.y} ${b.x},${b.y}` });
        } else if (geometry.type === 'brush') {
            const last = geometry.lines[geometry.lines.length - 1];
            if (last) {
                shapes.push({
                    kind: 'brush',
                    points: last.points.map((p) => {
                        const o = toOutput(p.x, p.y);
                        return `${o.x},${o.y}`;
                    }).join(' '),
                });
            }
        }
    }
    return shapes;
});

const viewBox = computed(() => `0 0 1 1`);

onMounted(() => {
    window.addEventListener('resize', recomputeBox);
    recomputeBox();
});

onBeforeUnmount(() => {
    window.removeEventListener('resize', recomputeBox);
});
</script>

<template>
    <div
        v-if="enabled"
        ref="rootRef"
        class="absolute inset-0 z-30 touch-none"
        :style="box ? { cursor: 'crosshair' } : {}"
        data-testid="develop-mask-overlay"
        @pointerdown.prevent="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerCancel"
        @keydown="onKeyDown"
        tabindex="-1"
    >
        <svg class="absolute inset-0 w-full h-full pointer-events-none" :viewBox="viewBox" preserveAspectRatio="none">
            <g v-for="(shape, i) in outlines" :key="i" stroke="rgba(255,0,0,0.7)" fill="none" stroke-width="0.002">
                <ellipse
                    v-if="shape.ellipse"
                    :cx="shape.ellipse.cx"
                    :cy="shape.ellipse.cy"
                    :rx="shape.ellipse.rx"
                    :ry="shape.ellipse.ry"
                />
                <polyline v-else :points="shape.points" />
            </g>
            <polyline
                v-if="livePoints.length > 1"
                :points="livePoints.map((p) => `${p.x},${p.y}`).join(' ')"
                stroke="rgba(255,0,0,0.9)"
                fill="none"
                stroke-width="0.002"
            />
        </svg>
    </div>
</template>
