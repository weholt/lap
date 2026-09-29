import { describe, it, expect, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { createDevelopSelection } from "./useDevelopSelection";
import { copySections } from "./useDevelopClipboard";
import { DEFAULT_RECIPE } from "./useDevelopSession.types";
function payload(exposure: number) {
  return copySections({ ...DEFAULT_RECIPE, exposure }, ["basic"]);
}

describe("Edit Selected queue", () => {
  it("captures targets, serializes writes and restores each own prior values even when Undo arrives mid-save", async () => {
    const state = createDevelopSelection(() => false);
    const values: Record<number, number> = { 2: -1, 3: 0.5 };
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let calls = 0;
    invoke.mockImplementation(async (_cmd, args) => {
      if (++calls === 1) await gate;
      const before = payload(values[args.assetId]);
      values[args.assetId] = args.payload.values.exposure;
      return { before, revision: calls };
    });
    state.setSelection([1, 2, 3]);
    state.toggle();
    state.changed({ kind: "edit", id: 1, assetId: 1, payload: payload(2) });
    await Promise.resolve();
    state.setSelection([1, 4]);
    state.changed({ kind: "undo", id: 1, assetId: 1, payload: payload(0) });
    expect(calls).toBe(1);
    release();
    await state.idle();
    expect(values).toEqual({ 2: -1, 3: 0.5 });
    expect(state.pending.value).toBe(0);
    state.changed({ kind: "redo", id: 1, assetId: 1, payload: payload(2) });
    await state.idle();
    expect(values).toEqual({ 2: 2, 3: 2 });
  });
  it("never overwrites retained unsaved work, reports failure and retries explicitly", async () => {
    let dirty = true;
    const state = createDevelopSelection((id) => id === 2 && dirty);
    invoke.mockReset();
    invoke.mockResolvedValue({ before: payload(0), revision: 1 });
    state.setSelection([1, 2]);
    state.toggle();
    state.changed({ kind: "edit", id: 2, assetId: 1, payload: payload(1) });
    await state.idle();
    expect(invoke).not.toHaveBeenCalled();
    expect(state.enabled.value).toBe(false);
    expect(state.errors.value[0]).toContain("#2");
    dirty = false;
    state.retry();
    await state.idle();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(state.errors.value).toEqual([]);
  });
  it("does nothing while disabled and rejects oversized or incomplete selections", async () => {
    const state = createDevelopSelection(() => false);
    invoke.mockReset();
    state.setSelection([1]);
    state.toggle();
    state.changed({ kind: "edit", id: 3, assetId: 1, payload: payload(1) });
    await state.idle();
    expect(invoke).not.toHaveBeenCalled();
    state.setSelection(Array.from({ length: 257 }, (_, i) => i + 1));
    state.toggle();
    expect(state.enabled.value).toBe(false);
  });
});
