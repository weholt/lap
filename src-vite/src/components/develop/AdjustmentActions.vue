<script setup lang="ts">
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useI18n } from "vue-i18n";
import { useDevelopEditor } from "@/composables/useDevelopEditor";
import {
  parseClipboardPayload,
  serializeClipboard,
} from "@/composables/useDevelopClipboard";
const { t } = useI18n();
const editor = useDevelopEditor();
const selection = editor.selection;
const busy = ref(false);
const error = ref("");
const message = ref("");
const unavailable = computed(
  () => !editor.session.value || editor.opening.value || busy.value,
);
async function copy() {
  if (unavailable.value) return;
  busy.value = true;
  error.value = "";
  message.value = "";
  try {
    editor.endEditTransaction();
    const text = serializeClipboard(editor.copyAdjustments());
    await invoke("develop_write_adjustment_clipboard", { text });
    message.value = t("develop.actions.copied");
  } catch (err) {
    error.value = String(err);
  } finally {
    busy.value = false;
  }
}
async function apply() {
  if (unavailable.value) return;
  const id = editor.activeFileId.value;
  busy.value = true;
  error.value = "";
  message.value = "";
  try {
    const text = await invoke<string>("develop_read_adjustment_clipboard");
    const payload = parseClipboardPayload(text);
    if (id !== editor.activeFileId.value || editor.opening.value)
      throw new Error(t("develop.actions.changedImage"));
    editor.applyAdjustments(payload);
    message.value = t("develop.actions.applied");
  } catch (err) {
    error.value = String(err);
  } finally {
    busy.value = false;
  }
}
</script>
<template>
  <div
    class="shrink-0 px-2 pb-2 border-b border-base-content/10"
    data-testid="develop-adjustment-actions"
  >
    <div class="flex items-stretch gap-1">
      <button
        class="btn btn-ghost btn-xs h-auto py-1 flex-1 flex-col gap-0.5"
        :disabled="unavailable"
        :title="t('develop.actions.copyHint')"
        :aria-label="t('develop.actions.copy')"
        data-testid="develop-copy-adjustments"
        @click.stop="copy"
      >
        <svg
          width="22"
          height="22"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
        >
          <path d="M13 3h8v8l-3-3-11 11-4-4L14 6z" />
        </svg>
        <span>{{ t("develop.actions.copy") }}</span>
      </button>
      <button
        class="btn btn-ghost btn-xs h-auto py-1 flex-1 flex-col gap-0.5"
        :disabled="unavailable"
        :title="t('develop.actions.applyHint')"
        :aria-label="t('develop.actions.apply')"
        data-testid="develop-apply-adjustments"
        @click.stop="apply"
      >
        <svg
          width="22"
          height="22"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
        >
          <path d="M11 21H3v-8l3 3L17 5l4 4L10 18z" />
        </svg>
        <span>{{ t("develop.actions.apply") }}</span>
      </button>
      <button
        class="btn btn-ghost btn-xs h-auto py-1 flex-[2] flex-col gap-0.5"
        :class="
          selection.enabled.value
            ? 'text-warning bg-warning/10 ring-1 ring-warning/50'
            : ''
        "
        :disabled="
          unavailable ||
          selection.count.value < 2 ||
          selection.count.value > 256 ||
          !selection.ids.value.includes(editor.activeFileId.value ?? 0)
        "
        :title="t('develop.actions.selectedHint')"
        :aria-pressed="String(selection.enabled.value)"
        data-testid="develop-edit-selected"
        @click.stop="selection.toggle"
      >
        <svg
          width="26"
          height="22"
          viewBox="0 0 28 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          aria-hidden="true"
        >
          <path d="M9 3h16v13M5 7h16v13" />
          <path d="M1 11h16v12H1z" />
        </svg>
        <span
          >{{ t("develop.actions.editSelected") }} ({{
            selection.count.value
          }})</span
        >
      </button>
    </div>
    <p
      v-if="selection.enabled.value"
      class="text-[10px] text-warning pt-1"
      role="status"
    >
      {{ t("develop.actions.active", { count: selection.count.value }) }}
    </p>
    <p
      v-if="selection.pending.value"
      class="text-xs text-primary pt-1"
      role="status"
    >
      {{
        t("develop.actions.progress", {
          done: selection.completed.value,
          total: selection.total.value,
          pending: selection.pending.value,
        })
      }}
    </p>
    <p v-else-if="message" class="text-[10px] text-success pt-1" role="status">
      {{ message }}
    </p>
    <p
      v-if="error"
      class="text-xs text-error break-words pt-1"
      role="alert"
      data-testid="develop-adjustment-error"
    >
      {{ error }}
    </p>
    <div
      v-if="selection.errors.value.length"
      class="text-xs text-error break-words pt-1"
      role="alert"
    >
      <p>{{ t("develop.actions.failed") }}</p>
      <p v-for="item in selection.errors.value" :key="item">{{ item }}</p>
      <button
        v-if="selection.canRetry.value && !selection.pending.value"
        class="btn btn-xs"
        @click.stop="selection.retry"
      >
        {{ t("develop.save.retry") }}
      </button>
    </div>
  </div>
</template>
