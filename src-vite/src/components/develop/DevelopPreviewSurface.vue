<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import type { DevelopPreviewState } from '@/composables/useDevelopSession';
import DevelopMaskOverlay from '@/components/develop/masks/DevelopMaskOverlay.vue';

const props = defineProps<{ frame: DevelopPreviewState | null }>();
const emit = defineEmits<{
    presented: [frame: DevelopPreviewState];
    error: [message: string];
}>();
const viewport = ref<HTMLDivElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const available = ref({ width: 0, height: 0 });
let observer: ResizeObserver | undefined;
let presentation = 0;

// The display box is fitted to the viewport, never to the bitmap's intrinsic
// pixel size. Draft/full frames therefore replace pixels in the same place.
const imageStyle = computed(() => {
    const frame = props.frame;
    if (!frame || frame.width <= 0 || frame.height <= 0) return { width: '0px', height: '0px' };
    const scale = Math.min(available.value.width / frame.width, available.value.height / frame.height);
    return { width: `${frame.width * scale}px`, height: `${frame.height * scale}px` };
});

onMounted(() => {
    if (!viewport.value) return;
    const rect = viewport.value.getBoundingClientRect();
    available.value = { width: rect.width, height: rect.height };
    observer = new ResizeObserver(([entry]) => {
        if (entry) available.value = { width: entry.contentRect.width, height: entry.contentRect.height };
    });
    observer.observe(viewport.value);
});
onBeforeUnmount(() => {
    observer?.disconnect();
    cancelAnimationFrame(presentation);
});

watch(() => props.frame, async (frame) => {
    await nextTick();
    const target = canvas.value;
    if (!target || !frame || frame !== props.frame) return;
    const context = target.getContext('2d');
    if (!context) { emit('error', 'canvas 2d context unavailable'); return; }
    // Resize and upload synchronously. There is no blank frame between the two,
    // and explicit CSS dimensions prevent bitmap allocation from changing layout.
    if (target.width !== frame.width) target.width = frame.width;
    if (target.height !== frame.height) target.height = frame.height;
    context.putImageData(new ImageData(new Uint8ClampedArray(frame.bytes), frame.width, frame.height), 0, 0);
    cancelAnimationFrame(presentation);
    presentation = requestAnimationFrame(() => {
        if (frame === props.frame) emit('presented', frame);
    });
}, { immediate: true });
</script>

<template>
    <div ref="viewport" class="absolute inset-0 flex items-center justify-center overflow-hidden">
        <div v-if="frame" class="relative shrink-0" :style="imageStyle" data-testid="develop-preview-image-box">
            <canvas ref="canvas" class="block w-full h-full"></canvas>
            <!-- The mask coordinates follow the fitted image, excluding letterboxing. -->
            <DevelopMaskOverlay :target="canvas" />
        </div>
    </div>
</template>
