<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { parseClipboardPayload } from "@/composables/useDevelopClipboard";
import type { DevelopPreset } from "@/composables/useDevelopClipboard.types";
import { useDevelopEditor } from "@/composables/useDevelopEditor";

const STORAGE_KEY = "lap.develop.presets.v1";
const editor = useDevelopEditor();
const expanded = ref(false);
const presets = ref<DevelopPreset[]>([]);
const name = ref("");
const selectedId = ref("");
const error = ref("");
const message = ref("");
const confirmDelete = ref(false);
const selected = computed(() =>
  presets.value.find((preset) => preset.id === selectedId.value),
);
const canApply = computed(
  () => !!selected.value && !!editor.session.value && !editor.opening.value,
);
const canApplySelected = computed(
  () =>
    canApply.value &&
    editor.selection.count.value > 1 &&
    editor.selection.ids.value.includes(editor.activeFileId.value ?? 0),
);

function load() {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(STORAGE_KEY) || "[]",
    );
    if (!Array.isArray(value)) throw new Error("Invalid preset library");
    presets.value = value.slice(0, 200).map((candidate: any) => ({
      id: String(candidate.id),
      name: String(candidate.name).slice(0, 100),
      payload: parseClipboardPayload(candidate.payload),
    }));
  } catch (cause) {
    error.value = `Could not load presets: ${String(cause)}`;
  }
}
function persist() {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(presets.value));
}
onMounted(load);

function save() {
  const label = name.value.trim();
  if (!label || !editor.session.value) return;
  try {
    editor.endEditTransaction();
    const preset = {
      id: crypto.randomUUID(),
      name: label.slice(0, 100),
      payload: editor.copyAdjustments(),
    };
    presets.value = [...presets.value, preset];
    persist();
    selectedId.value = preset.id;
    name.value = "";
    error.value = "";
    message.value = `Saved ${preset.name}`;
  } catch (cause) {
    error.value = String(cause);
  }
}
function apply(toSelection: boolean) {
  if (
    !selected.value ||
    !canApply.value ||
    (toSelection && !canApplySelected.value)
  )
    return;
  try {
    editor.endEditTransaction();
    const wasEnabled = editor.selection.enabled.value;
    editor.selection.enabled.value = toSelection;
    try {
      editor.applyAdjustments(selected.value.payload);
    } finally {
      editor.selection.enabled.value = wasEnabled;
    }
    error.value = "";
    message.value = toSelection
      ? `Applied to ${editor.selection.count.value} selected images`
      : `Applied ${selected.value.name}`;
  } catch (cause) {
    error.value = String(cause);
  }
}
function remove() {
  if (!selected.value) return;
  try {
    presets.value = presets.value.filter(
      (preset) => preset.id !== selectedId.value,
    );
    persist();
    selectedId.value = "";
    confirmDelete.value = false;
    error.value = "";
  } catch (cause) {
    error.value = String(cause);
  }
}
</script>

<template>
  <section
    class="border-t border-base-content/5 px-1 pt-2 pb-1"
    data-testid="develop-section-presets"
  >
    <button
      type="button"
      class="w-full text-left text-[11px] font-bold uppercase text-base-content/60"
      :aria-expanded="expanded"
      @click="expanded = !expanded"
    >
      ▤ Presets
    </button>
    <div v-if="expanded" class="space-y-2 pt-2">
      <div class="flex gap-1">
        <input
          v-model="name"
          type="text"
          maxlength="100"
          class="input input-xs flex-1 min-w-0"
          placeholder="Preset name"
          aria-label="Preset name"
          data-testid="develop-preset-name"
          @keydown.enter="save"
        />
        <button
          type="button"
          class="btn btn-primary btn-xs"
          data-testid="develop-preset-save"
          :disabled="!name.trim() || !editor.session.value"
          @click="save"
        >
          Save
        </button>
      </div>
      <div class="flex gap-1">
        <select
          v-model="selectedId"
          class="select select-xs flex-1 min-w-0"
          aria-label="Preset"
          data-testid="develop-preset-select"
        >
          <option value="">Choose a preset</option>
          <option v-for="preset in presets" :key="preset.id" :value="preset.id">
            {{ preset.name }}
          </option>
        </select>
        <button
          type="button"
          class="btn btn-ghost btn-xs"
          title="Delete preset"
          aria-label="Delete preset"
          :disabled="!selected"
          @click="confirmDelete = true"
        >
          ✕
        </button>
      </div>
      <div class="flex gap-1">
        <button
          type="button"
          class="btn btn-primary btn-xs flex-1"
          data-testid="develop-preset-apply"
          :disabled="!canApply"
          @click="apply(false)"
        >
          Apply
        </button>
        <button
          type="button"
          class="btn btn-outline btn-xs flex-1"
          data-testid="develop-preset-apply-selected"
          :disabled="!canApplySelected"
          @click="apply(true)"
        >
          Apply to selected ({{ editor.selection.count.value }})
        </button>
      </div>
      <p v-if="message" role="status" class="text-xs text-success">
        {{ message }}
      </p>
      <p v-if="error" role="alert" class="text-xs text-error break-words">
        {{ error }}
      </p>
    </div>
    <div
      v-if="confirmDelete"
      role="dialog"
      aria-modal="true"
      aria-label="Delete preset"
      class="fixed z-[100] inset-0 flex items-center justify-center bg-black/50"
    >
      <div
        class="rounded-box bg-base-200 border border-base-content/20 p-4 max-w-sm shadow-xl space-y-3"
      >
        <p>Delete preset {{ selected?.name }}?</p>
        <div class="flex justify-end gap-2">
          <button
            type="button"
            class="btn btn-sm"
            @click="confirmDelete = false"
          >
            Cancel
          </button>
          <button type="button" class="btn btn-error btn-sm" @click="remove">
            Delete
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
