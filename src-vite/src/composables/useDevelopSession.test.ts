import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Contract tests for the develop-session composable (lap-a52 / TASK-302).
//
// Governing contract: docs/raw-development/spec.md ("Recipe and session
// contract", A4). The composable must enforce, on the frontend side of the
// bounded preview IPC:
//   - explicit asset/variant/session/revision identities on every command;
//   - delayed or stale preview replies (old generation, other session/asset)
//     can never update the currently displayed asset;
//   - pixels are fetched through the bounded raw-bytes handle transport, never
//     carried inside the JSON envelope.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { useDevelopSession } from '@/composables/useDevelopSession';
import { DEFAULT_RECIPE, RECIPE_SCHEMA_VERSION } from '@/composables/useDevelopSession.types';

type InvokeArgs = Record<string, unknown>;

function openedSession(overrides: Partial<Record<string, unknown>> = {}) {
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
    };
}

function completedTicket(overrides: { ticket?: Partial<Record<string, unknown>> } = {}) {
    return {
        status: 'completed',
        ticket: {
            sessionId: 11,
            assetId: '42',
            variantId: 'default',
            generation: 1,
            quality: 'settled',
            width: 320,
            height: 200,
            handle: 'handle-gen-1',
            byteLen: 320 * 200 * 4,
            ...(overrides.ticket ?? {}),
        },
    };
}

describe('useDevelopSession', () => {
    beforeEach(() => {
        invokeMock.mockReset();
    });

    afterEach(() => {
        vi.restoreAllMocks();
    });

    it('opens a session and exposes the committed envelope', async () => {
        invokeMock.mockResolvedValueOnce(openedSession());
        const develop = useDevelopSession();

        const session = await develop.openEditSession(42, 'default');

        expect(invokeMock).toHaveBeenCalledWith('develop_open_edit_session', {
            assetId: 42,
            variantId: 'default',
        });
        expect(session.revision).toBe(3);
        expect(develop.session.value?.sessionId).toBe(11);
        expect(develop.session.value?.envelope.recipe).toEqual(DEFAULT_RECIPE);
    });

    it('renders previews through the bounded handle transport and fetches raw bytes once', async () => {
        invokeMock
            .mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce(completedTicket())
            .mockResolvedValueOnce(new Uint8Array([1, 2, 3, 4]).buffer);

        const develop = useDevelopSession();
        await develop.openEditSession(42);
        await develop.renderPreview(structuredClone(DEFAULT_RECIPE));

        expect(invokeMock).toHaveBeenCalledWith('develop_render_preview', {
            sessionId: 11,
            generation: 1,
            envelope: expect.objectContaining({ assetId: '42' }),
            quality: 'settled',
            maxEdge: 1536,
        });
        expect(invokeMock).toHaveBeenCalledWith('develop_take_preview_frame', {
            handle: 'handle-gen-1',
        });
        expect(develop.preview.value?.generation).toBe(1);
    });

    it('drops a delayed stale reply so it can never replace a newer generation', async () => {
        let releaseGeneration1: (value: unknown) => void = () => {};
        const delayedGen1 = new Promise((resolve) => {
            releaseGeneration1 = resolve;
        });

        invokeMock
            .mockResolvedValueOnce(openedSession())
            // generation 1 hangs in flight (slow renderer)
            .mockImplementationOnce(() => delayedGen1)
            // generation 2 completes immediately
            .mockResolvedValueOnce(
                completedTicket({
                    ticket: { generation: 2, handle: 'handle-gen-2', byteLen: 8 },
                }),
            )
            .mockResolvedValueOnce(new Uint8Array([9, 9, 9, 9]).buffer);

        const develop = useDevelopSession();
        await develop.openEditSession(42);

        const slow = develop.renderPreview(structuredClone(DEFAULT_RECIPE));
        await develop.renderPreview(structuredClone(DEFAULT_RECIPE));
        expect(develop.preview.value?.generation).toBe(2);

        // The delayed reply for generation 1 finally arrives as completed.
        releaseGeneration1(
            completedTicket({ ticket: { generation: 1, handle: 'handle-gen-1' } }),
        );
        await expect(slow).resolves.toBeNull();
        expect(develop.preview.value?.generation).toBe(2);
        expect(develop.preview.value?.handle).toBe('handle-gen-2');
        // The stale frame bytes were never fetched.
        expect(invokeMock).not.toHaveBeenCalledWith('develop_take_preview_frame', {
            handle: 'handle-gen-1',
        });
    });

    it('cannot let a completed preview from another session or asset update the current asset', async () => {
        invokeMock
            .mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce(
                completedTicket({
                    ticket: { sessionId: 99, assetId: '77', generation: 1 },
                }),
            );

        const develop = useDevelopSession();
        await develop.openEditSession(42);
        const outcome = await develop.renderPreview(structuredClone(DEFAULT_RECIPE));

        expect(outcome).toBeNull();
        expect(develop.preview.value).toBeNull();
        expect(invokeMock).not.toHaveBeenCalledWith(
            'develop_take_preview_frame',
            expect.anything(),
        );
    });

    it('sends the expected revision on commit and adopts the durable revision', async () => {
        invokeMock
            .mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce({
                sessionId: 11,
                revision: 4,
                contentHash: 'a'.repeat(64),
                sidecarPath: 'C:/photos/raw.CR2.lapedit.json',
                projectionApplied: true,
                projectionError: null,
            });

        const develop = useDevelopSession();
        await develop.openEditSession(42);
        const recipe = structuredClone(DEFAULT_RECIPE);
        recipe.exposure = 0.5;
        const receipt = await develop.commitRecipe(recipe);

        expect(invokeMock).toHaveBeenLastCalledWith('develop_commit_recipe', {
            sessionId: 11,
            expectedRevision: 3,
            envelope: expect.objectContaining({
                recipe: expect.objectContaining({ exposure: 0.5 }),
            }),
        });
        expect(receipt.revision).toBe(4);
        expect(develop.session.value?.revision).toBe(4);
        expect(develop.session.value?.envelope.revision).toBe(4);
    });

    it('rejects preview work after the session is closed and clears state', async () => {
        invokeMock
            .mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce({ sessionId: 11, revision: 3 });

        const develop = useDevelopSession();
        await develop.openEditSession(42);
        await develop.closeEditSession();

        expect(invokeMock).toHaveBeenCalledWith('develop_close_edit_session', {
            sessionId: 11,
        });
        expect(develop.session.value).toBeNull();
        await expect(
            develop.renderPreview(structuredClone(DEFAULT_RECIPE)),
        ).rejects.toThrow(/no open develop session/i);
        expect(invokeMock).not.toHaveBeenCalledWith(
            'develop_render_preview',
            expect.anything() as InvokeArgs,
        );
    });

    it('keeps the previous preview when a render reports cancelled', async () => {
        invokeMock
            .mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce(
                completedTicket({
                    ticket: { generation: 1, handle: 'handle-gen-1' },
                }),
            )
            .mockResolvedValueOnce(new Uint8Array([1, 1, 1, 1]).buffer)
            .mockResolvedValueOnce({ status: 'cancelled' });

        const develop = useDevelopSession();
        await develop.openEditSession(42);
        await develop.renderPreview(structuredClone(DEFAULT_RECIPE));
        await develop.renderPreview(structuredClone(DEFAULT_RECIPE), {
            quality: 'interactive',
        });

        expect(develop.preview.value?.handle).toBe('handle-gen-1');
    });
});


describe('async session lifetime regressions', () => {
    beforeEach(() => invokeMock.mockReset());
    it('discards frame bytes if a newer generation finishes during transfer', async () => {
        let release!: (value: ArrayBuffer) => void;
        invokeMock.mockResolvedValueOnce(openedSession())
            .mockResolvedValueOnce(completedTicket())
            .mockImplementationOnce(() => new Promise(resolve => { release = resolve; }))
            .mockResolvedValueOnce(completedTicket({ ticket: { generation: 2, handle: 'new' } }))
            .mockResolvedValueOnce(new ArrayBuffer(4));
        const session = useDevelopSession();
        await session.openEditSession(42);
        const old = session.renderPreview(structuredClone(DEFAULT_RECIPE));
        await vi.waitFor(() => expect(release).toBeDefined());
        await session.renderPreview(structuredClone(DEFAULT_RECIPE));
        release(new ArrayBuffer(4));
        expect(await old).toBeNull();
        expect(session.preview.value?.generation).toBe(2);
    });
    it('does not resurrect a closed session after commit acknowledgement', async () => {
        let release!: (value: unknown) => void;
        invokeMock.mockResolvedValueOnce(openedSession())
            .mockImplementationOnce(() => new Promise(resolve => { release = resolve; }))
            .mockResolvedValueOnce({sessionId: 11, revision: 4});
        const session = useDevelopSession();
        await session.openEditSession(42);
        const commit = session.commitRecipe(structuredClone(DEFAULT_RECIPE));
        await session.closeEditSession();
        release({sessionId: 11, revision: 4});
        await commit;
        expect(session.session.value).toBeNull();
    });
});


describe('overlapping open and close', () => {
    beforeEach(() => invokeMock.mockReset());
    it('closes a superseded backend open instead of replacing the latest asset', async () => {
        let release!: (value: unknown) => void;
        invokeMock.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }))
            .mockResolvedValueOnce(openedSession({sessionId: 22, assetId: '77'}))
            .mockResolvedValueOnce({sessionId: 11, revision: 3});
        const session = useDevelopSession();
        const first = session.openEditSession(42);
        const rejected = expect(first).rejects.toThrow('superseded');
        await session.openEditSession(77);
        release(openedSession());
        await rejected;
        expect(session.session.value?.sessionId).toBe(22);
        expect(invokeMock).toHaveBeenLastCalledWith('develop_close_edit_session', {sessionId: 11});
    });
    it('does not clear a new session when an old close completes late', async () => {
        let release!: (value: unknown) => void;
        invokeMock.mockResolvedValueOnce(openedSession())
            .mockImplementationOnce(() => new Promise(resolve => { release = resolve; }))
            .mockResolvedValueOnce(openedSession({sessionId: 22, assetId: '77'}));
        const session = useDevelopSession();
        await session.openEditSession(42);
        const closing = session.closeEditSession();
        await session.openEditSession(77);
        release({sessionId: 11, revision: 3});
        await closing;
        expect(session.session.value?.sessionId).toBe(22);
    });
});

describe('interactive preview quality', () => {
    it('uses a smaller interactive transport and preserves settled resolution', async () => {
        invokeMock.mockReset();
        invokeMock.mockResolvedValueOnce(openedSession()).mockResolvedValue({ status: 'cancelled' });
        const session = useDevelopSession();
        await session.openEditSession(42);
        await session.renderPreview(structuredClone(DEFAULT_RECIPE), { quality: 'interactive' });
        await session.renderPreview(structuredClone(DEFAULT_RECIPE), { quality: 'settled' });
        const calls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'develop_render_preview');
        expect(calls[0][1].maxEdge).toBe(768);
        expect(calls[1][1].maxEdge).toBe(1536);
    });
});
