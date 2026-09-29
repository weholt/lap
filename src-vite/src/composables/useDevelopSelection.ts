import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { DevelopClipboardPayload } from "./useDevelopClipboard.types";

export interface AdjustmentChange {
  kind: "edit" | "undo" | "redo";
  id: number;
  assetId: number;
  payload: DevelopClipboardPayload;
}
interface TargetEdit {
  assetId: number;
  before: DevelopClipboardPayload;
  after: DevelopClipboardPayload;
}
interface Applied {
  before: DevelopClipboardPayload;
  revision: number;
}
/** Completed gestures only: one sequential writer, bounded targets/history, no RAW decode.
 * Captured target IDs never change under an asynchronous write. Navigation awaits idle().
 * Undo replays each target's own prior settings under a revision check.
 */
export function createDevelopSelection(hasDirtyState: (id: number) => boolean) {
  const enabled = ref(false);
  const ids = ref<number[]>([]);
  const pending = ref(0);
  const completed = ref(0);
  const total = ref(0);
  const errors = ref<string[]>([]);
  const canRetry = ref(false);
  const count = computed(() => ids.value.length);
  const records = new Map<string, TargetEdit[]>();
  const revisions = new Map<number, number>();
  let tail: Promise<void> = Promise.resolve();
  let failures: Array<() => Promise<void>> = [];
  let failureEpoch = 0;

  function setSelection(selected: number[]) {
    ids.value = [
      ...new Set(selected.filter((id) => Number.isSafeInteger(id) && id > 0)),
    ];
    if (ids.value.length < 2 || ids.value.length > 256) enabled.value = false;
  }
  function toggle() {
    if (count.value >= 2 && count.value <= 256) enabled.value = !enabled.value;
  }
  function enqueue(resolveActions: () => Array<() => Promise<void>>) {
    if (pending.value >= 50) {
      enabled.value = false;
      errors.value = [
        "Too many pending group edits. Wait for saving, then apply the current settings again.",
      ];
      return;
    }
    pending.value++;
    const epoch = ++failureEpoch;
    canRetry.value = false;
    failures = [];
    tail = tail.then(async () => {
      const actions = resolveActions();
      errors.value = [];
      total.value = actions.length;
      completed.value = 0;
      for (const action of actions) {
        try {
          await action();
        } catch (error) {
          errors.value.push(String(error));
          enabled.value = false;
          if (epoch === failureEpoch) failures.push(action);
        } finally {
          completed.value++;
        }
      }
      pending.value--;
      if (epoch === failureEpoch) canRetry.value = failures.length > 0;
    });
  }
  async function apply(
    assetId: number,
    payload: DevelopClipboardPayload,
  ): Promise<Applied> {
    if (hasDirtyState(assetId))
      throw new Error(
        `#${assetId}: unsaved edits must be saved before group editing`,
      );
    try {
      const result = await invoke<Applied>("develop_apply_adjustments", {
        assetId,
        payload,
        expectedRevision: revisions.get(assetId) ?? null,
      });
      revisions.set(assetId, result.revision);
      return result;
    } catch (error) {
      throw new Error(`#${assetId}: ${String(error)}`);
    }
  }
  function changed(change: AdjustmentChange) {
    const key = `${change.assetId}:${change.id}`;
    if (change.kind === "edit") {
      if (!enabled.value) return;
      const targets = ids.value.filter((id) => id !== change.assetId);
      if (!ids.value.includes(change.assetId) || !targets.length) return;
      const entries: TargetEdit[] = [];
      records.set(key, entries);
      // Match the editor's bounded session history.
      if (records.size > 50) records.delete(records.keys().next().value!);
      enqueue(() =>
        targets.map((assetId) => async () => {
          const result = await apply(assetId, change.payload);
          entries.push({
            assetId,
            before: result.before,
            after: change.payload,
          });
        }),
      );
    } else if (records.has(key)) {
      // Resolve entries only after preceding writes finish (Undo can arrive mid-save).
      enqueue(() =>
        (records.get(key) ?? []).map((entry) => async () => {
          await apply(
            entry.assetId,
            change.kind === "undo" ? entry.before : entry.after,
          );
        }),
      );
    }
  }
  async function idle() {
    while (true) {
      const current = tail;
      await current;
      if (current === tail) break;
    }
  }
  function retry() {
    const retryActions = failures;
    enqueue(() => retryActions);
  }
  function clearHistory() {
    records.clear();
    revisions.clear();
    failures = [];
    canRetry.value = false;
  }
  return {
    enabled,
    count,
    ids,
    pending,
    completed,
    total,
    errors,
    canRetry,
    setSelection,
    toggle,
    changed,
    idle,
    retry,
    clearHistory,
  };
}
