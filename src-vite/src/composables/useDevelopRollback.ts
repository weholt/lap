import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

/**
 * Frontend contract for the non-destructive develop rollback switch
 * (lap-63f / TASK-602, docs/raw-development/rollback.md).
 *
 * The switch state lives exclusively in the backend flag document
 * (`develop-rollback.json`); this composable is the typed IPC adapter. While
 * the switch is enabled, the Develop panel entry point is disabled and every
 * mutating develop call fails fast with the localized rollback notice —
 * committed sidecars, retained previous revisions, resources and the catalog
 * projection stay untouched, and no recipe is ever flattened into the
 * original media.
 */

export const ROLLBACK_NOTICE_KEY = 'develop.rollbackNotice';

/** IPC name of the backend query command. */
const GET_COMMAND = 'develop_get_rollback';
/** IPC name of the backend persist command. */
const SET_COMMAND = 'develop_set_rollback';

export function useDevelopRollback() {
    /** `null` until the backend state has been read at least once. */
    const active = ref(false);
    const loaded = ref(false);

    /** Reads the persisted switch state from the backend flag document. */
    async function refresh(): Promise<boolean> {
        active.value = await invoke<boolean>(GET_COMMAND);
        loaded.value = true;
        return active.value;
    }

    /** Persists the switch state; local state only moves on success. */
    async function setEnabled(enabled: boolean): Promise<void> {
        await invoke(SET_COMMAND, { enabled });
        active.value = enabled;
        loaded.value = true;
    }

    /**
     * Gate for entry points that start or continue develop editing. Fails
     * closed with the i18n notice key while the switch is on (or before the
     * backend state is known), so callers surface the explicit rollback
     * notice instead of silently editing.
     */
    function ensureEditingAvailable(): void {
        if (active.value || !loaded.value) {
            throw new Error(ROLLBACK_NOTICE_KEY);
        }
    }

    return {
        active,
        loaded,
        noticeKey: ROLLBACK_NOTICE_KEY,
        refresh,
        setEnabled,
        ensureEditingAvailable,
    };
}
