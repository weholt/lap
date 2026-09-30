<script setup lang="ts">
import { computed, ref } from "vue";
import { useDevelopEditor } from "@/composables/useDevelopEditor";
import type { Recipe } from "@/composables/useDevelopSession.types";
import DevelopSliderControl from "./DevelopSliderControl.vue";

const editor = useDevelopEditor();
const expanded = ref(false);
const channels = [
  "reds",
  "oranges",
  "yellows",
  "greens",
  "aquas",
  "blues",
  "purples",
  "magentas",
] as const;
const mix = computed(
  () => editor.recipe.value?.blackWhiteMix ?? [0, 0, 0, 0, 0, 0, 0, 0],
);
function setMix(index: number, value: number, live: boolean) {
  const next = [...mix.value] as Recipe["blackWhiteMix"];
  next[index] = value;
  if (live)
    editor.applyRecipePatchLive({ blackWhiteMix: next }, "black and white mix");
  else editor.applyRecipePatch({ blackWhiteMix: next }, "black and white mix");
}
function reset() {
  editor.applyRecipePatch(
    { blackWhiteEnabled: false, blackWhiteMix: [0, 0, 0, 0, 0, 0, 0, 0] },
    "reset black and white",
  );
}
</script>

<template>
  <section
    class="border-t border-base-content/5"
    data-testid="develop-section-black-white"
  >
    <div class="flex items-center gap-1 px-1 pt-2 pb-1">
      <svg
        width="14"
        height="14"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="9" />
        <path d="M12 3a9 9 0 0 1 0 18Z" fill="currentColor" stroke="none" />
      </svg>
      <button
        type="button"
        class="flex-1 text-left text-[11px] font-bold uppercase text-base-content/60"
        :aria-expanded="expanded"
        data-testid="develop-section-toggle-black-white"
        @click="expanded = !expanded"
      >
        Black and White
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-xs"
        title="Reset Black and White"
        aria-label="Reset Black and White"
        @click="reset"
      >
        ↶
      </button>
    </div>
    <div v-show="expanded" class="px-1 pb-2">
      <label class="flex items-center gap-2 text-xs mb-2">
        <input
          type="checkbox"
          class="checkbox checkbox-primary checkbox-xs"
          data-testid="develop-black-white-enabled"
          :checked="editor.recipe.value?.blackWhiteEnabled ?? false"
          @change="
            editor.applyRecipePatch(
              {
                blackWhiteEnabled: ($event.target as HTMLInputElement).checked,
              },
              'black and white conversion',
            )
          "
        />
        Convert to black and white
      </label>
      <div :class="{ 'opacity-50': !editor.recipe.value?.blackWhiteEnabled }">
        <DevelopSliderControl
          v-for="(channel, index) in channels"
          :key="channel"
          :label-key="`develop.hslChannels.${channel}`"
          :min="-100"
          :max="100"
          :step="1"
          :value="mix[index]"
          :testid="`develop-bw-${channel}`"
          @live="setMix(index, $event, true)"
          @settle="editor.endEditTransaction()"
          @commit="setMix(index, $event, false)"
          @reset="setMix(index, 0, false)"
        />
      </div>
    </div>
  </section>
</template>
