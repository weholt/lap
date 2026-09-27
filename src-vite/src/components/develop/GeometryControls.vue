<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  IconClose,
  IconFlipHorizontal,
  IconFlipVertical,
  IconRestore,
  IconRotateLeft,
  IconRotateRight,
} from "@/common/icons";
import { useDevelopEditor } from "@/composables/useDevelopEditor";
import {
  clampCropPointerGesture,
  cropBoxStyle,
  frameAspectOptions,
  useDevelopGeometry,
  type CropGestureMode,
} from "@/composables/useDevelopGeometry";
import type { CropRect, Recipe } from "@/composables/useDevelopSession.types";

/**
 * Crop / rotate / flip interactions in oriented coordinates (lap-6bc /
 * TASK-402, spec A5/A6/A9/A10).
 *
 * Every control edits the validated recipe through useDevelopGeometry:
 * quarter turns and flips move the crop rect with the image so the same
 * original pixels stay selected, the aspect lock is a pixel ratio over the
 * oriented frame, and each meaningful gesture forms exactly one undo
 * transaction with explicit commit/cancel. The component owns no geometry
 * math and never touches the source file.
 */

const { t } = useI18n();
const develop = useDevelopEditor();
const geometry = useDevelopGeometry();

const aspectValue = computed(() => {
  const aspect = geometry.aspectRatio.value;
  return aspect === null ? "free" : aspectOptionKey(aspect);
});

const aspectOptions = computed(() => frameAspectOptions());

function aspectOptionKey(aspect: number): string {
  return String(Math.round(aspect * 10000) / 10000);
}

function onAspectChange(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  if (value === "free") {
    geometry.setAspect(null);
    return;
  }
  if (value === "original") {
    const { width, height } = geometry.oriented.value;
    if (width > 0 && height > 0) geometry.setAspect(width / height);
    return;
  }
  const aspect = Number(value);
  if (Number.isFinite(aspect) && aspect > 0) geometry.setAspect(aspect);
}

// --------------------------------------------------------------------------
// Crop gestures: pointer drags on the box/handles and the Escape/Enter keys.
// --------------------------------------------------------------------------

type Gesture = {
  mode: CropGestureMode;
  crop: CropRect;
  clientX: number;
  clientY: number;
};

const gesture = ref<Gesture | null>(null);
const gestureSnapshot = ref<Recipe | null>(null);

function startGesture(mode: CropGestureMode, event: PointerEvent) {
  if (event.button !== 0 || gesture.value) return;
  const current = geometry.crop.value;
  // No crop yet: the drag starts from the full frame.
  const base: CropRect = current ?? { x: 0, y: 0, width: 1, height: 1 };
  gesture.value = {
    mode,
    crop: { ...base },
    clientX: event.clientX,
    clientY: event.clientY,
  };
  gestureSnapshot.value = geometry.snapshot();
  geometry.beginCropGesture();
  window.addEventListener("pointermove", onWindowPointerMove);
  window.addEventListener("pointerup", onWindowPointerUp);
  window.addEventListener("keydown", onGestureKeydown, true);
}

function onWindowPointerMove(event: PointerEvent) {
  const current = gesture.value;
  if (!current) return;
  const frame = geometry.frameSize.value;
  if (frame.width <= 0 || frame.height <= 0) return;
  const dx = (event.clientX - current.clientX) / frame.width;
  const dy = (event.clientY - current.clientY) / frame.height;
  const next = clampCropPointerGesture(
    current.crop,
    current.mode,
    dx,
    dy,
    geometry.aspectRatio.value,
    geometry.oriented.value.width,
    geometry.oriented.value.height,
  );
  if (next) geometry.setCropLive(next);
  event.preventDefault();
}

function onWindowPointerUp() {
  commitGesture();
}

function onGestureKeydown(event: KeyboardEvent) {
  if (!gesture.value) return;
  if (event.key === "Escape") {
    cancelGesture();
    event.preventDefault();
    event.stopPropagation();
  } else if (event.key === "Enter") {
    commitGesture();
    event.preventDefault();
    event.stopPropagation();
  }
}

function commitGesture() {
  if (!gesture.value) return;
  gesture.value = null;
  gestureSnapshot.value = null;
  geometry.endCropGesture();
  removeGestureListeners();
}

function cancelGesture() {
  if (!gesture.value) return;
  gesture.value = null;
  const restored = gestureSnapshot.value;
  gestureSnapshot.value = null;
  if (restored) geometry.cancelCropGesture(restored);
  removeGestureListeners();
}

function removeGestureListeners() {
  window.removeEventListener("pointermove", onWindowPointerMove);
  window.removeEventListener("pointerup", onWindowPointerUp);
  window.removeEventListener("keydown", onGestureKeydown, true);
}

function onHandlePointerDown(mode: CropGestureMode, event: PointerEvent) {
  event.preventDefault();
  event.stopPropagation();
  startGesture(mode, event);
}

function onCropBoxPointerDown(event: PointerEvent) {
  const target = event.target as HTMLElement | null;
  if (target && target.closest("[data-crop-handle]")) return;
  onHandlePointerDown("move", event);
}

// Keyboard moves on the frame: one complete transaction per keystroke.
function onFrameKeydown(event: KeyboardEvent) {
  const step = event.shiftKey ? 0.05 : 0.01;
  let dx = 0;
  let dy = 0;
  switch (event.key) {
    case "ArrowLeft":
      dx = -step;
      break;
    case "ArrowRight":
      dx = step;
      break;
    case "ArrowUp":
      dy = -step;
      break;
    case "ArrowDown":
      dy = step;
      break;
    default:
      return;
  }
  const current = geometry.crop.value ?? { x: 0, y: 0, width: 1, height: 1 };
  const next = clampCropPointerGesture(
    current,
    "move",
    dx,
    dy,
    geometry.aspectRatio.value,
    geometry.oriented.value.width,
    geometry.oriented.value.height,
  );
  if (next) geometry.setCrop(next);
  event.preventDefault();
  event.stopPropagation();
}

function removeCrop() {
  if (gesture.value) return;
  geometry.setCrop(null);
}

const boxCrop = computed<CropRect>(
  () => geometry.crop.value ?? { x: 0, y: 0, width: 1, height: 1 },
);
const cropStyle = computed(() => cropBoxStyle(boxCrop.value));
const cropHandles = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;

const handleClass: Record<(typeof cropHandles)[number], string> = {
  nw: "-top-1 -left-1 cursor-nwse-resize",
  n: "-top-1 left-1/2 -translate-x-1/2 cursor-ns-resize",
  ne: "-top-1 -right-1 cursor-nesw-resize",
  e: "top-1/2 -right-1 -translate-y-1/2 cursor-ew-resize",
  se: "-bottom-1 -right-1 cursor-nwse-resize",
  s: "-bottom-1 left-1/2 -translate-x-1/2 cursor-ns-resize",
  sw: "-bottom-1 -left-1 cursor-nesw-resize",
  w: "top-1/2 -left-1 -translate-y-1/2 cursor-ew-resize",
};

onBeforeUnmount(() => {
  if (gesture.value) commitGesture();
  removeGestureListeners();
});
</script>

<template>
  <div
    class="px-1 py-2 space-y-2"
    data-testid="develop-geometry-root"
    data-bypassed="false"
  >
    <div class="flex items-center gap-0.5">
      <span
        class="flex-1 min-w-0 text-[10px] font-bold uppercase tracking-wide text-base-content/40 truncate"
        data-testid="develop-geometry-title"
        >{{ $t("develop.geometry.title") }}</span
      >
      <button
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-rotate-left"
        :aria-label="$t('develop.geometry.rotateLeft')"
        :title="$t('develop.geometry.rotateLeft')"
        @click.stop="geometry.rotateLeft()"
      >
        <IconRotateLeft class="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-rotate-right"
        :aria-label="$t('develop.geometry.rotateRight')"
        :title="$t('develop.geometry.rotateRight')"
        @click.stop="geometry.rotateRight()"
      >
        <IconRotateRight class="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-flip-horizontal"
        :aria-label="$t('develop.geometry.flipHorizontal')"
        :title="$t('develop.geometry.flipHorizontal')"
        @click.stop="geometry.toggleFlipHorizontal()"
      >
        <IconFlipHorizontal class="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-flip-vertical"
        :aria-label="$t('develop.geometry.flipVertical')"
        :title="$t('develop.geometry.flipVertical')"
        @click.stop="geometry.toggleFlipVertical()"
      >
        <IconFlipVertical class="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-reset"
        :aria-label="$t('develop.geometry.reset')"
        :title="$t('develop.geometry.reset')"
        @click.stop="geometry.resetGeometry()"
      >
        <IconRestore class="w-3.5 h-3.5" />
      </button>
    </div>

    <!-- Aspect lock over the oriented frame -->
    <div class="flex items-center gap-1">
      <label
        class="text-[10px] font-bold uppercase tracking-wide text-base-content/40 shrink-0"
        for="develop-geometry-aspect"
        >{{ $t("develop.geometry.aspect") }}</label
      >
      <select
        id="develop-geometry-aspect"
        class="select select-xs flex-1 min-w-0"
        data-testid="develop-geometry-aspect"
        :value="aspectValue"
        :aria-label="$t('develop.geometry.aspect')"
        @change="onAspectChange"
      >
        <option value="free">{{ $t("develop.geometry.aspectFree") }}</option>
        <option value="original">
          {{ $t("develop.geometry.aspectOriginal") }}
        </option>
        <option
          v-for="option in aspectOptions"
          :key="option.value"
          :value="option.value"
        >
          {{ option.label }}
        </option>
      </select>
      <button
        v-if="geometry.crop.value"
        type="button"
        class="btn btn-ghost btn-xs text-base-content/50 hover:text-base-content"
        data-testid="develop-geometry-crop-remove"
        :aria-label="$t('develop.geometry.cropRemove')"
        :title="$t('develop.geometry.cropRemove')"
        @click.stop="removeCrop"
      >
        <IconClose class="w-3.5 h-3.5" />
      </button>
    </div>

    <!-- Oriented frame with the crop rectangle -->
    <div class="flex justify-center">
      <div
        class="relative select-none touch-none"
        data-testid="develop-geometry-crop"
        tabindex="0"
        role="application"
        :aria-label="$t('develop.geometry.cropAria')"
        :style="{
          width: `${geometry.frameSize.value.width}px`,
          height: `${geometry.frameSize.value.height}px`,
        }"
        @keydown="onFrameKeydown"
      >
        <div
          class="absolute inset-0 rounded-box border border-base-content/10 bg-base-300/40"
        ></div>
        <div
          class="absolute rounded-sm border border-primary cursor-move"
          :class="
            geometry.crop.value ? '' : 'border-dashed border-base-content/30'
          "
          data-testid="develop-geometry-crop-box"
          :style="cropStyle"
          @pointerdown="onCropBoxPointerDown"
        >
          <div
            v-for="handle in cropHandles"
            :key="handle"
            class="absolute w-2 h-2 bg-primary border border-base-100"
            :data-crop-handle="handle"
            :data-testid="`develop-geometry-handle-${handle}`"
            :class="handleClass[handle]"
            @pointerdown="onHandlePointerDown(handle, $event)"
          ></div>
        </div>
      </div>
    </div>
    <div class="text-[10px] leading-4 text-base-content/40">
      {{ $t("develop.geometry.cropHint") }}
    </div>
  </div>
</template>
