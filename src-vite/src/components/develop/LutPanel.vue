<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useDevelopEditor } from "@/composables/useDevelopEditor";

interface LutEntry {
  id: string;
  name: string;
  cubeSize: number;
  sizeBytes: number;
}
const editor = useDevelopEditor();
const entries = ref<LutEntry[]>([]);
const expanded = ref(false);
const busy = ref(false);
const error = ref("");
const manageOpen = ref(false);
const confirmDelete = ref<LutEntry | null>(null);
const activeId = computed(
  () => editor.recipe.value?.lutPath?.replace(/^resource:\/\//, "") ?? "",
);
const selectedEntry = computed(() =>
  entries.value.find((entry) => entry.id === activeId.value),
);

async function refresh() {
  try {
    entries.value = await invoke<LutEntry[]>("develop_list_luts");
  } catch (cause) {
    error.value = String(cause);
  }
}
onMounted(() => void refresh());

function select(entry: LutEntry | null) {
  if (!editor.recipe.value || busy.value) return;
  try {
    const payload = editor.copyAdjustments();
    payload.values.lutPath = entry ? `resource://${entry.id}` : null;
    payload.values.lutName = entry?.name ?? null;
    payload.values.lutSize = entry?.cubeSize ?? 0;
    if (entry) {
      payload.resources = {
        ...payload.resources,
        [entry.id]: {
          algorithm: "sha256",
          digest: entry.id.slice(4),
          sizeBytes: entry.sizeBytes,
        },
      };
    }
    editor.applyAdjustments(payload);
    error.value = "";
  } catch (cause) {
    error.value = String(cause);
  }
}

async function importLut() {
  const path = await open({
    multiple: false,
    filters: [{ name: "3D LUT", extensions: ["cube", "3dl"] }],
  });
  if (typeof path !== "string") return;
  busy.value = true;
  error.value = "";
  try {
    const entry = await invoke<LutEntry>("develop_import_lut", { path });
    await refresh();
    busy.value = false;
    select(entry);
  } catch (cause) {
    error.value = String(cause);
  } finally {
    busy.value = false;
  }
}

async function removeLut() {
  const entry = confirmDelete.value;
  if (!entry) return;
  busy.value = true;
  error.value = "";
  try {
    await invoke("develop_remove_lut", { id: entry.id });
    confirmDelete.value = null;
    await refresh();
  } catch (cause) {
    error.value = String(cause);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section
    class="border-t border-base-content/5 px-1 pt-2 pb-1"
    data-testid="develop-section-lut"
  >
    <button
      type="button"
      class="w-full text-left text-[11px] font-bold uppercase text-base-content/60"
      :aria-expanded="expanded"
      @click="expanded = !expanded"
    >
      ◈ LUT
    </button>
    <div v-if="expanded" class="pt-2 space-y-2">
      <div class="flex gap-1 items-center">
        <select
          class="select select-xs flex-1 min-w-0"
          data-testid="develop-lut-select"
          :value="activeId"
          :disabled="busy || !editor.session.value"
          aria-label="LUT"
          @change="
            select(
              entries.find(
                (entry) =>
                  entry.id === ($event.target as HTMLSelectElement).value,
              ) ?? null,
            )
          "
        >
          <option value="">None</option>
          <option v-if="activeId && !selectedEntry" :value="activeId">
            {{ editor.recipe.value?.lutName || "Current LUT" }} (removed from
            library)
          </option>
          <option v-for="entry in entries" :key="entry.id" :value="entry.id">
            {{ entry.name }}
          </option>
        </select>
        <button
          type="button"
          class="btn btn-ghost btn-xs"
          title="Import LUT"
          aria-label="Import LUT"
          data-testid="develop-lut-import"
          :disabled="busy"
          @click="importLut"
        >
          <svg
            viewBox="0 0 24 24"
            width="17"
            height="17"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            aria-hidden="true"
          >
            <path d="M12 16V3m0 0L7 8m5-5 5 5M4 16v4h16v-4" />
          </svg>
        </button>
        <button
          type="button"
          class="btn btn-ghost btn-xs"
          title="Manage LUTs"
          aria-label="Manage LUTs"
          data-testid="develop-lut-manage"
          :disabled="busy || !entries.length"
          @click="manageOpen = true"
        >
          ✕
        </button>
      </div>
      <label v-if="activeId" class="flex gap-2 items-center text-xs"
        >Intensity
        <input
          type="range"
          min="0"
          max="100"
          :value="editor.recipe.value?.lutIntensity ?? 100"
          class="range range-primary range-xs flex-1"
          @input="
            editor.setParamLive(
              'lutIntensity',
              Number(($event.target as HTMLInputElement).value),
            )
          "
          @change="editor.endEditTransaction()"
        />
        <span>{{ editor.recipe.value?.lutIntensity ?? 100 }}%</span>
      </label>
      <p v-if="error" role="alert" class="text-xs text-error break-words">
        {{ error }}
      </p>
    </div>
    <div
      v-if="manageOpen"
      role="dialog"
      aria-modal="true"
      aria-label="Manage LUTs"
      class="fixed z-[100] inset-0 flex items-center justify-center bg-black/50"
    >
      <div
        class="rounded-box bg-base-200 border border-base-content/20 p-4 max-w-sm shadow-xl space-y-3"
      >
        <template v-if="confirmDelete">
          <p class="font-semibold">
            Remove {{ confirmDelete.name }} from the LUT library?
          </p>
          <p class="text-xs text-base-content/70">
            Existing images keep their LUT data so their edits remain
            reproducible.
          </p>
          <div class="flex justify-end gap-2">
            <button
              type="button"
              class="btn btn-sm"
              :disabled="busy"
              @click="confirmDelete = null"
            >
              Cancel
            </button>
            <button
              type="button"
              class="btn btn-error btn-sm"
              data-testid="develop-lut-confirm-remove"
              :disabled="busy"
              @click="removeLut"
            >
              Remove
            </button>
          </div>
        </template>
        <template v-else>
          <p class="font-semibold">Manage LUTs</p>
          <div class="max-h-64 overflow-y-auto space-y-1">
            <div
              v-for="entry in entries"
              :key="entry.id"
              class="flex items-center gap-2 text-sm"
            >
              <span class="flex-1 truncate">{{ entry.name }}</span>
              <button
                type="button"
                class="btn btn-ghost btn-xs"
                :aria-label="`Remove ${entry.name}`"
                @click="confirmDelete = entry"
              >
                Remove
              </button>
            </div>
          </div>
          <button type="button" class="btn btn-sm" @click="manageOpen = false">
            Done
          </button>
        </template>
      </div>
    </div>
  </section>
</template>
