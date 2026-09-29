import { ref, shallowRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';

import {
    type CapabilityReport,
    type ClosedEditSession,
    type CommitReceipt,
    type OpenedEditSession,
    type PreviewWait,
    type Recipe,
    type RecipeEnvelopeValue,
} from './useDevelopSession.types';

/**
 * Frontend contract for Lap's develop sessions (lap-a52 / TASK-302,
 * docs/raw-development/spec.md "Recipe and session contract").
 *
 * The backend owns session lifetimes, bounded preview scheduling and durable
 * recipe persistence; this composable is the typed IPC adapter. It enforces
 * the frontend half of the stale-result contract (spec A4): a delayed reply
 * from an older generation, or a completed ticket from another session or
 * asset, can never update the currently displayed asset.
 *
 * Preview pixels travel exclusively through the bounded raw-byte handle
 * transport (`develop_take_preview_frame` returns an ArrayBuffer), never as
 * base64 inside the JSON envelope.
 */

export interface DevelopPreviewState {
    inputAt?: number;
    quality?: 'interactive' | 'settled';
    handle: string;
    width: number;
    height: number;
    generation: number;
    bytes: ArrayBuffer;
}

export interface RenderOptions {
    inputAt?: number;
    /** Fast draft while dragging (default: settled full quality). */
    quality?: 'interactive' | 'settled';
    /** Maximum preview edge length; bounded by the backend transport. */
    maxEdge?: number;
}

/** Settled preview edge; draft uses half the edge (one quarter the pixels). */
const DEFAULT_PREVIEW_EDGE = 1536;

export function useDevelopSession() {
    const session = shallowRef<OpenedEditSession | null>(null);
    const preview = shallowRef<DevelopPreviewState | null>(null);
    /** Strictly increasing per open session; older replies are dropped. */
    const latestGeneration = ref(0);
    let closed = false;
    let lifetime = 0;

    /**
     * Opens the edit session for one catalog asset. The returned session
     * carries the committed envelope (sidecar-aligned revision, 0 = unedited).
     */
    async function openEditSession(
        assetId: number | string,
        variantId = 'default',
    ): Promise<OpenedEditSession> {
        const opening = ++lifetime;
        const opened = await invoke<OpenedEditSession>('develop_open_edit_session', {
            assetId,
            variantId,
        });
        if (opening !== lifetime) {
            await invoke('develop_close_edit_session', { sessionId: opened.sessionId });
            throw new Error('develop session open superseded');
        }
        session.value = opened;
        preview.value = null;
        latestGeneration.value = 0;
        closed = false;
        return opened;
    }

    /**
     * Renders one preview generation for the open session. Resolves `null`
     * for coalesced/cancelled/stale outcomes (the displayed state is left
     * untouched); throws for explicit failures.
     */
    async function renderPreview(
        recipe: Recipe,
        options: RenderOptions = {},
    ): Promise<DevelopPreviewState | null> {
        const current = session.value;
        if (!current || closed) {
            throw new Error('no open develop session');
        }
        const requestLifetime = lifetime;
        const generation = latestGeneration.value + 1;
        latestGeneration.value = generation;
        const envelope: RecipeEnvelopeValue = { ...current.envelope, recipe };
        const outcome = await invoke<PreviewWait>('develop_render_preview', {
            sessionId: current.sessionId,
            generation,
            envelope,
            quality: options.quality ?? 'settled',
            maxEdge: options.maxEdge ?? (options.quality === 'interactive' ? 768 : DEFAULT_PREVIEW_EDGE),
        });

        // Stale guard, part 1: the session must still be the one this request
        // belongs to (asset switches or closes invalidate in-flight replies).
        const still = session.value;
        if (!still || closed || requestLifetime !== lifetime) {
            return null;
        }
        if (still.sessionId !== current.sessionId || still.assetId !== current.assetId) {
            return null;
        }
        if (outcome.status === 'cancelled') {
            return null;
        }
        if (outcome.status === 'failed') {
            throw new Error(`develop preview failed (${outcome.code}): ${outcome.message}`);
        }

        // Stale guard, part 2: only the newest accepted generation may update
        // the displayed pixels; delayed older replies are discarded.
        const ticket = outcome.ticket;
        if (
            ticket.sessionId !== still.sessionId ||
            ticket.assetId !== still.assetId ||
            ticket.variantId !== still.variantId ||
            ticket.generation < latestGeneration.value
        ) {
            return null;
        }

        const bytes = await invoke<ArrayBuffer>('develop_take_preview_frame', {
            handle: ticket.handle,
        });
        // Fetching the pixels is another asynchronous boundary: the earlier
        // ticket check cannot protect against a switch/close/new render here.
        if (closed || requestLifetime !== lifetime ||
            session.value?.sessionId !== current.sessionId ||
            generation !== latestGeneration.value) return null;
        const state: DevelopPreviewState = {
            inputAt: options.inputAt,
            quality: options.quality ?? 'settled',
            handle: ticket.handle,
            width: ticket.width,
            height: ticket.height,
            generation: ticket.generation,
            bytes,
        };
        preview.value = state;
        return state;
    }

    /**
     * Validates and durably persists the recipe with optimistic revision
     * control. The session adopts the acknowledged revision only after the
     * backend confirms the durable sidecar write.
     *
     * `envelopePatch` merges host-managed envelope fields (e.g. the
     * `unsupported` payload retained by an rrdata import, lap-5c2) into the
     * committed envelope; the patch becomes part of the session envelope on
     * acknowledgment so later commits keep it.
     */
    async function commitRecipe(
        recipe: Recipe,
        envelopePatch?: Record<string, unknown>,
    ): Promise<CommitReceipt> {
        const current = session.value;
        if (!current || closed) {
            throw new Error('no open develop session');
        }
        const envelope: RecipeEnvelopeValue = {
            ...current.envelope,
            ...(envelopePatch ?? {}),
            recipe,
        };
        const receipt = await invoke<CommitReceipt>('develop_commit_recipe', {
            sessionId: current.sessionId,
            expectedRevision: current.revision,
            envelope,
        });
        if (closed || session.value?.sessionId !== current.sessionId) return receipt;
        current.revision = receipt.revision;
        current.envelope = { ...current.envelope, ...envelopePatch, revision: receipt.revision, recipe };
        session.value = { ...current };
        return receipt;
    }

    /**
     * Closes the session: the backend cancels previews, lets pending saves
     * settle, and releases buffers and preview handles. Subsequent preview
     * work on this composable is rejected.
     */
    async function closeEditSession(): Promise<ClosedEditSession | null> {
        ++lifetime;
        const current = session.value;
        if (!current) {
            return null;
        }
        closed = true;
        try {
            return await invoke<ClosedEditSession>('develop_close_edit_session', {
                sessionId: current.sessionId,
            });
        } finally {
            if (session.value?.sessionId === current.sessionId) {
                session.value = null;
                preview.value = null;
                latestGeneration.value = 0;
            }
        }
    }

    /** Explicit capability report for the pinned engine (GPU + identity). */
    async function getCapabilities(): Promise<CapabilityReport> {
        return invoke<CapabilityReport>('develop_get_capabilities');
    }

    return {
        session,
        preview,
        latestGeneration,
        openEditSession,
        renderPreview,
        commitRecipe,
        closeEditSession,
        getCapabilities,
    };
}
