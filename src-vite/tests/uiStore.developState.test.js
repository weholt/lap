import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

// Regression coverage for the develop editor state in the UI store
// (lap-0e9 / TASK-303). The develop mode must stay fully separated from the
// legacy info-panel adjustment flow: develop dirty state never triggers the
// legacy "unsaved changes" modal, and retained per-asset develop state never
// leaks between assets.

import { useUIStore } from '@/stores/uiStore';

describe('uiStore develop editor state', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('tracks dirty develop state for the active asset only', () => {
    const uiStore = useUIStore();

    expect(uiStore.hasDirtyDevelopState(7)).toBe(false);

    uiStore.setDevelopActive(7);
    uiStore.setDevelopDirty(true);
    expect(uiStore.hasDirtyDevelopState(7)).toBe(true);
    expect(uiStore.hasDirtyDevelopState(8)).toBe(false);
    expect(uiStore.hasDirtyDevelopState(null)).toBe(false);
    expect(uiStore.hasDirtyDevelopState(undefined)).toBe(false);

    uiStore.setDevelopDirty(false);
    expect(uiStore.hasDirtyDevelopState(7)).toBe(false);
  });

  it('keeps retained develop state isolated per asset', () => {
    const uiStore = useUIStore();

    uiStore.retainDevelopState(7, { recipe: { exposure: 1 }, saveState: 'failed', lastError: 'x' });
    uiStore.retainDevelopState(8, { recipe: { exposure: 2 }, saveState: 'conflict', lastError: 'y' });

    expect(uiStore.hasDirtyDevelopState(7)).toBe(true);
    expect(uiStore.hasDirtyDevelopState(8)).toBe(true);
    expect(uiStore.hasDirtyDevelopState(9)).toBe(false);

    const retained = uiStore.takeRetainedDevelopState(7);
    expect(retained).toEqual({ recipe: { exposure: 1 }, saveState: 'failed', lastError: 'x' });
    expect(uiStore.hasDirtyDevelopState(7)).toBe(false);
    expect(uiStore.hasDirtyDevelopState(8)).toBe(true);

    expect(uiStore.peekRetainedDevelopState(8)?.saveState).toBe('conflict');
    uiStore.clearRetainedDevelopState(8);
    expect(uiStore.hasDirtyDevelopState(8)).toBe(false);
  });

  it('never mixes develop state into the legacy info-panel adjustment check', () => {
    const uiStore = useUIStore();
    const fileInfo = { file_path: 'C:/photos/a.CR2', width: 100, height: 100 };

    uiStore.setDevelopActive(7);
    uiStore.setDevelopDirty(true);
    uiStore.setDevelopSaveState('failed', 'read-only directory');

    expect(uiStore.hasActiveChanges(fileInfo)).toBe(false);

    uiStore.setActiveAdjustments('C:/photos/a.CR2', { brightness: 5 });
    expect(uiStore.hasActiveChanges(fileInfo)).toBe(true);

    uiStore.clearActiveAdjustments();
    expect(uiStore.hasActiveChanges(fileInfo)).toBe(false);
    // Develop state is untouched by the legacy flow.
    expect(uiStore.hasDirtyDevelopState(7)).toBe(true);
  });
});
