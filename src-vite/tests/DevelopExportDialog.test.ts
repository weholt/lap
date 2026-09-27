import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';

// Contract tests for the derivative export dialog (lap-7ae / TASK-304).
//
// Governing contract: docs/raw-development/spec.md `export_developed`.
// The UI half of the contract pinned here:
//   - commit and derivative export are separate actions;
//   - the export action flushes dirty edits BEFORE exporting and pins the
//     acknowledged committed revision (a clean session exports its committed
//     revision without re-committing);
//   - backend failures (e.g. revision mismatch, protected destination) are
//     surfaced visibly instead of being swallowed;
//   - an in-flight export can be cancelled cooperatively by id.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

const saveMock = vi.fn();

vi.mock('@tauri-apps/plugin-dialog', () => ({
    save: (...args: unknown[]) => saveMock(...args),
}));

import DevelopExportDialog from '@/components/DevelopExportDialog.vue';
import {
    DEFAULT_RECIPE,
    RECIPE_SCHEMA_VERSION,
    type OpenedEditSession,
} from '@/composables/useDevelopSession.types';

type InvokeArgs = Record<string, unknown>;

function openedSession(overrides: Partial<OpenedEditSession> = {}): OpenedEditSession {
    return {
        sessionId: 11,
        assetId: '42',
        variantId: 'default',
        revision: 3,
        dimensions: [6000, 4000],
        sourceFingerprint: 'f'.repeat(64),
        envelope: {
            schemaVersion: RECIPE_SCHEMA_VERSION,
            engineVersion: 'lap/0.3.2/rapidraw-edit-model/0.1.0',
            assetId: '42',
            variantId: 'default',
            revision: 3,
            sourceFingerprint: 'f'.repeat(64),
            decode: {},
            recipe: structuredClone(DEFAULT_RECIPE),
            resources: {},
            unsupported: {},
        },
        ...overrides,
    } as OpenedEditSession;
}

function completedReceipt(overrides: Record<string, unknown> = {}) {
    return {
        assetId: '42',
        variantId: 'default',
        revision: 4,
        destination: 'D:/exports/developed.png',
        format: 'png',
        width: 6000,
        height: 4000,
        bytesWritten: 123456,
        sourceFingerprint: 'f'.repeat(64),
        contentHash: 'c'.repeat(64),
        ...overrides,
    };
}

const commitRecipeMock = vi.fn();

async function mountDialog(
    props: {
        session?: OpenedEditSession | null;
        dirty?: boolean;
    } = {},
) {
    return mount(DevelopExportDialog, {
        props: {
            session: props.session ?? openedSession(),
            recipe: structuredClone(DEFAULT_RECIPE),
            dirty: props.dirty ?? false,
            commitRecipe: commitRecipeMock,
        },
    });
}

function destinationInput(wrapper: ReturnType<typeof mount>) {
    return wrapper.get('[data-testid="develop-export-destination"]');
}

async function readyDialog(props: { dirty?: boolean } = {}) {
    const wrapper = await mountDialog(props);
    await destinationInput(wrapper).setValue('D:/exports/developed.png');
    return wrapper;
}

describe('DevelopExportDialog', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        saveMock.mockReset();
        commitRecipeMock.mockReset();
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('renders nothing without an open develop session', () => {
        const wrapper = mount(DevelopExportDialog, {
            props: {
                session: null,
                recipe: structuredClone(DEFAULT_RECIPE),
                dirty: false,
                commitRecipe: commitRecipeMock,
            },
        });
        expect(wrapper.find('[data-testid="develop-export-dialog"]').exists()).toBe(false);
    });

    it('disables exporting until a destination is set', async () => {
        const wrapper = await mountDialog();
        const start = wrapper.get('[data-testid="develop-export-start"]');
        expect((start.element as HTMLButtonElement).disabled).toBe(true);
        await destinationInput(wrapper).setValue('D:/exports/out.png');
        expect((start.element as HTMLButtonElement).disabled).toBe(false);
    });

    it('commits without exporting when the commit action is used', async () => {
        commitRecipeMock.mockResolvedValueOnce({ revision: 4 });
        const wrapper = await readyDialog({ dirty: true });

        await wrapper.get('[data-testid="develop-export-commit"]').trigger('click');
        await flushPromises();

        expect(commitRecipeMock).toHaveBeenCalledTimes(1);
        expect(invokeMock).not.toHaveBeenCalledWith(
            'develop_export_developed',
            expect.anything(),
        );
        expect(wrapper.emitted('committed')).toEqual([[{ revision: 4 }]]);
        expect(wrapper.get('[data-testid="develop-export-status"]').text()).toContain(
            'Committed revision 4',
        );
    });

    it('flushes dirty edits before exporting and pins the acknowledged revision', async () => {
        // Commit resolves after the export command is observed to be absent
        // until the flush finished: the revision sent to the backend must be
        // the receipt revision, not the stale session revision.
        let resolveCommit: (value: { revision: number }) => void = () => {};
        commitRecipeMock.mockReturnValueOnce(
            new Promise((resolve) => {
                resolveCommit = resolve;
            }),
        );
        invokeMock.mockResolvedValueOnce({ status: 'completed', receipt: completedReceipt() });

        const wrapper = await readyDialog({ dirty: true });
        const exportPromise = wrapper.get('[data-testid="develop-export-start"]').trigger('click');
        await flushPromises();

        expect(commitRecipeMock).toHaveBeenCalledTimes(1);
        expect(invokeMock).not.toHaveBeenCalledWith(
            'develop_export_developed',
            expect.anything(),
        );

        resolveCommit({ revision: 4 });
        await exportPromise;
        await flushPromises();

        const exportCalls = invokeMock.mock.calls.filter(
            ([command]) => command === 'develop_export_developed',
        );
        expect(exportCalls).toHaveLength(1);
        expect((exportCalls[0][1] as InvokeArgs).revision).toBe(4);
        expect(wrapper.emitted('committed')).toEqual([[{ revision: 4 }]]);
    });

    it('exports a clean session at its committed revision without re-committing', async () => {
        invokeMock.mockResolvedValueOnce({ status: 'completed', receipt: completedReceipt() });
        const wrapper = await readyDialog({ dirty: false });

        await wrapper.get('[data-testid="develop-export-start"]').trigger('click');
        await flushPromises();

        expect(commitRecipeMock).not.toHaveBeenCalled();
        const exportCalls = invokeMock.mock.calls.filter(
            ([command]) => command === 'develop_export_developed',
        );
        expect(exportCalls).toHaveLength(1);
        const args = exportCalls[0][1] as InvokeArgs;
        expect(args.revision).toBe(3);
        expect(args.assetId).toBe(42);
        expect(args.variantId).toBe('default');
        expect(args.format).toBe('png');
        expect(args.maxEdge).toBeNull();
        expect(String(args.destination)).toBe('D:/exports/developed.png');
        expect(args.exportJob).toBeTruthy();

        const receipt = wrapper.get('[data-testid="develop-export-receipt"]').text();
        expect(receipt).toContain('6000×4000');
        expect(receipt).toContain('Revision 4');
    });

    it('shows backend failures such as revision mismatches visibly', async () => {
        invokeMock.mockRejectedValueOnce(
            'export revision mismatch: requested committed revision 3 but the durable sidecar holds Some(5); flush edits and re-commit before exporting',
        );
        const wrapper = await readyDialog();

        await wrapper.get('[data-testid="develop-export-start"]').trigger('click');
        await flushPromises();

        const error = wrapper.get('[data-testid="develop-export-error"]').text();
        expect(error).toContain('revision mismatch');
        expect(wrapper.find('[data-testid="develop-export-receipt"]').exists()).toBe(false);
    });

    it('cancels the active export cooperatively by job id', async () => {
        let resolveExport: (value: unknown) => void = () => {};
        invokeMock.mockImplementationOnce(() => new Promise((resolve) => {
            resolveExport = resolve;
        }));

        const wrapper = await readyDialog();
        const exportPromise = wrapper.get('[data-testid="develop-export-start"]').trigger('click');
        await flushPromises();

        const cancelBefore = invokeMock.mock.calls.filter(
            ([command]) => command === 'develop_cancel_export',
        );
        expect(cancelBefore).toHaveLength(0);

        await wrapper.get('[data-testid="develop-export-cancel"]').trigger('click');
        await flushPromises();

        const cancelCalls = invokeMock.mock.calls.filter(
            ([command]) => command === 'develop_cancel_export',
        );
        expect(cancelCalls).toHaveLength(1);
        const exportCalls = invokeMock.mock.calls.filter(
            ([command]) => command === 'develop_export_developed',
        );
        expect((cancelCalls[0][1] as InvokeArgs).exportJob).toBe(
            (exportCalls[0][1] as InvokeArgs).exportJob,
        );

        resolveExport({ status: 'cancelled' });
        await exportPromise;
        await flushPromises();

        expect(wrapper.get('[data-testid="develop-export-status"]').text()).toContain(
            'no file was written',
        );
    });

    it('sends an explicit resize bound only when full resolution is unchecked', async () => {
        invokeMock.mockResolvedValueOnce({ status: 'completed', receipt: completedReceipt() });
        const wrapper = await readyDialog();
        await wrapper.get('[data-testid="develop-export-fullres"]').setValue(false);
        await wrapper.get('[data-testid="develop-export-maxedge"]').setValue(2048);

        await wrapper.get('[data-testid="develop-export-start"]').trigger('click');
        await flushPromises();

        const exportCall = invokeMock.mock.calls.find(
            ([command]) => command === 'develop_export_developed',
        );
        expect((exportCall?.[1] as InvokeArgs).maxEdge).toBe(2048);
    });

    it('suggests a destination through the save dialog and keeps explicit errors visible', async () => {
        saveMock.mockResolvedValueOnce('D:/exports/picked.png');
        const wrapper = await mountDialog();

        await wrapper.get('[data-testid="develop-export-browse"]').trigger('click');
        await flushPromises();

        expect(saveMock).toHaveBeenCalledTimes(1);
        expect(destinationInput(wrapper).element as HTMLInputElement).toBeTruthy();
        expect((destinationInput(wrapper).element as HTMLInputElement).value).toBe(
            'D:/exports/picked.png',
        );
    });
});
