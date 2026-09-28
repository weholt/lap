import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Contract tests for the develop rollback-switch composable (lap-63f /
// TASK-602, managed continuation of lap-404.2).
//
// Governing contract: docs/raw-development/spec.md ("Persistence and
// compatibility") and docs/raw-development/rollback.md. The composable is
// the typed IPC adapter for the non-destructive rollback switch:
//   - the switch state comes exclusively from the backend flag document;
//   - enabling/disabling persists through `develop_set_rollback`;
//   - `ensureEditingAvailable` rejects with the localized rollback notice
//     while the switch is on, so the Develop entry point cannot open and
//     mutating calls fail fast before any sidecar write.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import {
    ROLLBACK_NOTICE_KEY,
    useDevelopRollback,
} from '@/composables/useDevelopRollback';

describe('useDevelopRollback', () => {
    beforeEach(() => {
        invokeMock.mockReset();
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('loads the persisted switch state from the backend', async () => {
        invokeMock.mockResolvedValueOnce(true);
        const rollback = useDevelopRollback();

        await rollback.refresh();

        expect(invokeMock).toHaveBeenCalledWith('develop_get_rollback');
        expect(rollback.active.value).toBe(true);
        expect(rollback.loaded.value).toBe(true);
    });

    it('starts unloaded and treats an unknown state as unavailable for editing', async () => {
        const rollback = useDevelopRollback();

        expect(rollback.loaded.value).toBe(false);
        expect(rollback.active.value).toBe(false);
        // Before the backend state arrives the gate must fail closed with
        // the i18n notice key, never silently allow editing.
        expect(() => rollback.ensureEditingAvailable()).toThrowError(
            ROLLBACK_NOTICE_KEY,
        );
    });

    it('persists enable/disable and reflects the new state', async () => {
        invokeMock.mockResolvedValueOnce(false); // initial refresh
        const rollback = useDevelopRollback();
        await rollback.refresh();

        invokeMock.mockResolvedValueOnce(undefined);
        await rollback.setEnabled(true);
        expect(invokeMock).toHaveBeenCalledWith('develop_set_rollback', {
            enabled: true,
        });
        expect(rollback.active.value).toBe(true);

        invokeMock.mockResolvedValueOnce(undefined);
        await rollback.setEnabled(false);
        expect(rollback.active.value).toBe(false);
    });

    it('propagates backend failures as explicit errors instead of flipping state', async () => {
        invokeMock.mockRejectedValueOnce(new Error('config write failed'));
        const rollback = useDevelopRollback();

        await expect(rollback.setEnabled(true)).rejects.toThrowError(
            'config write failed',
        );
        expect(rollback.active.value).toBe(false);
    });

    it('ensureEditingAvailable rejects with the notice key while rolled back', async () => {
        invokeMock.mockResolvedValueOnce(true);
        const rollback = useDevelopRollback();
        await rollback.refresh();

        expect(() => rollback.ensureEditingAvailable()).toThrowError(
            ROLLBACK_NOTICE_KEY,
        );
        expect(rollback.noticeKey).toBe(ROLLBACK_NOTICE_KEY);
    });
});
