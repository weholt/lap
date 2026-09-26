import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it } from 'vitest';

import { useUIStore } from '@/stores/uiStore';

// Pins the CURRENT temporary-adjustment contract of Lap's editor
// (docs/raw-development/spec.md, "Recipe/defaults"): one transient
// activeAdjustments object, saturation default 100, no durable per-photo
// recipe. The extraction must replace this consciously, not accidentally.

const fileInfo = { file_path: 'C:/photos/raw.CR2', width: 6000, height: 4000 };

describe('uiStore activeAdjustments', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it('starts with neutral defaults and saturation 100', () => {
    const store = useUIStore();
    expect(store.activeAdjustments).toEqual({
      filePath: null,
      brightness: 0,
      contrast: 0,
      saturation: 100,
      hue: 0,
      blur: 0,
      filter: null,
      resize: null,
    });
  });

  it('reports no active changes for default adjustments', () => {
    const store = useUIStore();
    store.setActiveAdjustments(fileInfo.file_path, {});
    expect(store.hasActiveChanges(fileInfo)).toBe(false);
  });

  it('reports changes when an adjustment leaves its default', () => {
    const store = useUIStore();
    store.setActiveAdjustments(fileInfo.file_path, { brightness: 10 });
    expect(store.hasActiveChanges(fileInfo)).toBe(true);

    store.clearActiveAdjustments();
    store.setActiveAdjustments(fileInfo.file_path, { saturation: 101 });
    expect(store.hasActiveChanges(fileInfo)).toBe(true);
  });

  it('ignores adjustments staged for a different file', () => {
    const store = useUIStore();
    store.setActiveAdjustments('C:/photos/other.JPG', { brightness: 50 });
    expect(store.hasActiveChanges(fileInfo)).toBe(false);
  });

  it('detects only resizes that change the rounded target dimensions', () => {
    const store = useUIStore();
    store.setActiveAdjustments(fileInfo.file_path, {
      resize: { width: 6000.4, height: 3999.6 },
    });
    expect(store.hasActiveChanges(fileInfo)).toBe(false);

    store.clearActiveAdjustments();
    store.setActiveAdjustments(fileInfo.file_path, {
      resize: { width: 2048, height: 1365 },
    });
    expect(store.hasActiveChanges(fileInfo)).toBe(true);
  });

  it('resets to the documented defaults on clearActiveAdjustments', () => {
    const store = useUIStore();
    store.setActiveAdjustments(fileInfo.file_path, {
      brightness: 33,
      filter: 'grayscale',
      resize: { width: 100, height: 100 },
    });
    store.clearActiveAdjustments();
    expect(store.activeAdjustments.filePath).toBeNull();
    expect(store.activeAdjustments.brightness).toBe(0);
    expect(store.activeAdjustments.saturation).toBe(100);
    expect(store.activeAdjustments.filter).toBeNull();
    expect(store.activeAdjustments.resize).toBeNull();
  });
});
