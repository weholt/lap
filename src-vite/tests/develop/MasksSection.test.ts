import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { createPinia, setActivePinia } from 'pinia';

// Component tests for the native mask tools section (lap-78d).
//
// Runs against the REAL develop editor (invoke mocked) so the transaction
// semantics asserted here are the ones the editor enforces: one committed
// history entry per completed gesture, parameter edits as single implicit
// transactions, explicit display of unrenderable kinds, and reset. Tool
// arming is UI-only state on the shared singleton.

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import enMessages from '@/locales/en.json';
import MasksSection from '@/components/develop/masks/MasksSection.vue';
import { __resetMaskToolsForTests, useMaskTools } from '@/components/develop/masks/useMaskTools';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import {
    DEFAULT_RECIPE,
    RECIPE_SCHEMA_VERSION,
    type Recipe,
} from '@/composables/useDevelopSession.types';

const commandResponses = new Map<string, unknown[]>();

function queueCommand(command: string, response: unknown) {
    if (!commandResponses.has(command)) commandResponses.set(command, []);
    commandResponses.get(command)!.push(response);
}

function envelopeFor(assetId: number, revision: number) {
    return {
        schemaVersion: RECIPE_SCHEMA_VERSION,
        engineVersion: 'lap/0.3.2/rapidraw-edit-model/0.1.0',
        assetId: String(assetId),
        variantId: 'default',
        revision,
        sourceFingerprint: 'f'.repeat(64),
        decode: {},
        recipe: structuredClone(DEFAULT_RECIPE),
        resources: {},
        unsupported: {},
    };
}

function openedSession(assetId: number, revision = 0) {
    return {
        sessionId: 100 + assetId,
        assetId: String(assetId),
        variantId: 'default',
        revision,
        dimensions: [6000, 4000],
        sourceFingerprint: 'f'.repeat(64),
        envelope: envelopeFor(assetId, revision),
    };
}

function receipt(assetId: number, revision: number) {
    return {
        sessionId: 100 + assetId,
        revision,
        contentHash: 'c'.repeat(64),
        sidecarPath: `C:/photos/raw-${assetId}.CR2.lapedit.json`,
        projectionApplied: true,
        projectionError: null,
    };
}

function makeI18n() {
    return createI18n({
        legacy: false,
        locale: 'en',
        fallbackLocale: 'en',
        messages: { en: enMessages as Record<string, unknown> },
    });
}

/** The editor's committed history size (one entry per transaction). */
function historySize(): number {
    const editor = useDevelopEditor() as unknown as { historySize: { value: number } };
    return editor.historySize.value;
}

describe('MasksSection', () => {
    beforeEach(async () => {
        invokeMock.mockReset();
        invokeMock.mockImplementation(async (command: string) => {
            const queue = commandResponses.get(command);
            if (!queue) throw new Error(`unexpected develop invoke: ${command}`);
            const response = queue.shift();
            if (response instanceof Error) throw response;
            return response;
        });
        commandResponses.clear();
        setActivePinia(createPinia());
        __resetMaskToolsForTests();
        const editor = useDevelopEditor();
        queueCommand('develop_open_edit_session', openedSession(1));
        queueCommand('develop_render_preview', {
            status: 'completed',
            ticket: {
                sessionId: 101, assetId: '1', variantId: 'default',
                generation: 1, quality: 'settled', maxEdge: 1536,
                width: 64, height: 48, handle: 'h1', byteLen: 64 * 48 * 4,
            },
        });
        queueCommand('develop_take_preview_frame', new Uint8Array(64 * 48 * 4).buffer);
        await editor.openAsset({ id: 1 });
    });

    afterEach(async () => {
        commandResponses.clear();
        await useDevelopEditor().disposeForTests();
        __resetMaskToolsForTests();
        vi.restoreAllMocks();
    });

    async function mountSection() {
        const wrapper = mount(MasksSection, {
            global: {
                plugins: [makeI18n()],
            },
        });
        await flushPromises();
        return wrapper;
    }

    function commitGesture(tools: ReturnType<typeof useMaskTools>) {
        // Simulates the overlay's pointer flow (down → move → up).
        tools.controller.startGesture('radial', { x: 0.5, y: 0.5 });
        tools.controller.extendGesture({ x: 0.6, y: 0.6 });
        tools.controller.completeGesture({ x: 0.65, y: 0.65 });
    }

    it('shows the empty hint and the three tools', async () => {
        const wrapper = await mountSection();
        expect(wrapper.find('[data-testid="develop-masks-section"]').exists()).toBe(true);
        for (const tool of ['brush', 'linear', 'radial']) {
            expect(wrapper.find(`[data-testid="develop-mask-tool-${tool}"]`).exists()).toBe(true);
        }
        expect(wrapper.text()).toContain('No masks yet');
    });

    it('arming a tool is UI-only state that never touches the recipe', async () => {
        const wrapper = await mountSection();
        const tools = useMaskTools();
        await wrapper.find('[data-testid="develop-mask-tool-radial"]').trigger('click');
        expect(tools.activeTool.value).toBe('radial');
        await wrapper.find('[data-testid="develop-mask-tool-radial"]').trigger('click');
        expect(tools.activeTool.value).toBe('');
        expect(historySize()).toBe(0);
    });

    it('a completed gesture adds one mask and exactly one history entry', async () => {
        await mountSection();
        const tools = useMaskTools();
        commitGesture(tools);
        const recipe = useDevelopEditor().recipe.value as Recipe;
        expect(recipe.masks).toHaveLength(1);
        expect(recipe.masks[0].subMasks[0].geometry).toMatchObject({ type: 'radial' });
        expect(recipe.masks[0].subMasks[0].parameters).toMatchObject({ centerX: expect.any(Number) });
        expect(historySize()).toBe(1);
    });

    it('parameter edits are single transactions and reset clears everything', async () => {
        await mountSection();
        const tools = useMaskTools();
        commitGesture(tools);
        const before = historySize();

        const recipe = useDevelopEditor().recipe.value as Recipe;
        const maskId = recipe.masks[0].id;
        tools.controller.setMaskOpacity(maskId, 40);
        expect(historySize()).toBe(before + 1);
        tools.controller.setMaskFlag(maskId, 'invert', true);
        expect(historySize()).toBe(before + 2);

        tools.controller.resetMasks();
        expect(((useDevelopEditor().recipe.value as Recipe).masks)).toHaveLength(0);
    });

    it('surfaces unsupported kinds explicitly instead of hiding them', async () => {
        const tools = useMaskTools();
        commitGesture(tools);
        // Show a mask whose sub-mask has no typed geometry, the way an
        // imported AI mask arrives: the section must name the kind.
        const editor = useDevelopEditor();
        const recipe = structuredClone(editor.recipe.value) as Recipe;
        (recipe.masks[0].subMasks[0] as { geometry: unknown }).geometry = null;
        (recipe.masks[0].subMasks[0] as { type: string }).type = 'ai-subject';
        (editor as unknown as { applyRecipePatch(p: Partial<Recipe>, l: string): void })
            .applyRecipePatch({ masks: recipe.masks }, 'test');
        await flushPromises();
        const wrapper = await mountSection();
        expect(wrapper.text()).toContain('ai-subject');
    });
});
