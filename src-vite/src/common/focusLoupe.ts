import { ref } from 'vue';
import { config } from '@/common/config';

/** Shared across the toolbar and every image pane, including compare view. */
export const focusLoupeEnabled = ref(false);

/** 100% is one photo pixel per screen pixel. */
export const FOCUS_LOUPE_ZOOMS = [100, 150, 200, 400, 800] as const;

export function toggleFocusLoupe() {
  focusLoupeEnabled.value = !focusLoupeEnabled.value;
}

export function focusLoupeZoomPercent() {
  const value = Number(config.settings.focusLoupeZoom);
  return (FOCUS_LOUPE_ZOOMS as readonly number[]).includes(value) ? value : 100;
}
