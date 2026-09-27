import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { createPinia, setActivePinia } from 'pinia';

// Save-path routing regressions for the info panel's quick save
// (lap-0e9 / TASK-303; docs/raw-development/spec.md "Lap integration points").
//
// quickSave is an entry point that historically built an editImage request
// with identical source/destination paths. For developed assets (a develop
// session with uncommitted recipe state) it must route to an awaited recipe
// commit instead, and must never write pixels to the source path.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

const listenMock = vi.fn();

vi.mock('@tauri-apps/api/event', () => ({
    listen: (...args: unknown[]) => listenMock(...args),
    emit: vi.fn(),
}));

vi.mock('@tauri-apps/api/webviewWindow', () => ({
    WebviewWindow: class {
        static getByLabel() {
            return Promise.resolve(null);
        }
    },
    getCurrentWebviewWindow: () => ({
        close: vi.fn(),
        destroy: vi.fn(),
        isVisible: vi.fn().mockResolvedValue(false),
        listen: vi.fn().mockResolvedValue(() => {}),
    }),
}));

import FileInfo from '@/components/FileInfo.vue';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import { DEFAULT_RECIPE, RECIPE_SCHEMA_VERSION } from '@/composables/useDevelopSession.types';
import { useUIStore } from '@/stores/uiStore';
import enMessages from '@/locales/en.json';

// Command-dispatched responses: mount-time catalog queries share the invoke
// mock, so queued replies are keyed by command, not call order.
const commandResponses = new Map<string, unknown[]>();

function queueCommand(command: string, response: unknown) {
    if (!commandResponses.has(command)) commandResponses.set(command, []);
    commandResponses.get(command)!.push(response);
}

const fileInfo = () => ({
    id: 7,
    name: 'photo.CR2',
    file_path: 'C:/photos/photo.CR2',
    file_type: 3,
    width: 6000,
    height: 4000,
    size: 24000,
    e_orientation: 1,
});

function openedSession(assetId: number, revision = 0) {
    return {
        sessionId: 100 + assetId,
        assetId: String(assetId),
        variantId: 'default',
        revision,
        dimensions: [6000, 4000],
        sourceFingerprint: 'f'.repeat(64),
        envelope: {
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
        },
    };
}

const stubs = {
    Breadcrumb: true,
    TButton: true,
    FavoriteRatingControl: true,
    ImageHistogram: true,
    MapView: true,
};

async function mountFileInfo() {
    const pinia = createPinia();
    setActivePinia(pinia);
    const wrapper = mount(FileInfo, {
        props: { fileInfo: fileInfo() },
        global: {
            plugins: [
                createI18n({ legacy: false, locale: 'en', messages: { en: enMessages } }),
                pinia,
            ],
            stubs,
        },
    });
    await flushPromises();
    return wrapper;
}

describe('FileInfo quickSave develop routing', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        invokeMock.mockImplementation(async (command: string) => {
            const queue = commandResponses.get(command);
            if (!queue || queue.length === 0) return undefined;
            const response = queue.shift();
            if (response instanceof Error) throw response;
            return response;
        });
        listenMock.mockReset().mockResolvedValue(() => {});
        commandResponses.clear();
        setActivePinia(createPinia());
    });

    afterEach(async () => {
        commandResponses.clear();
        await useDevelopEditor().disposeForTests();
        vi.restoreAllMocks();
    });

    it('commits the dirty develop recipe instead of writing pixels to the source path', async () => {
        const editor = useDevelopEditor();
        queueCommand('develop_open_edit_session', openedSession(7));
        await editor.openAsset({ id: 7 });
        editor.setParam('exposure', 0.5);

        queueCommand('develop_commit_recipe', {
            sessionId: 107,
            revision: 1,
            contentHash: 'c'.repeat(64),
            sidecarPath: 'C:/photos/photo.CR2.lapedit.json',
            projectionApplied: true,
            projectionError: null,
        });

        const wrapper = await mountFileInfo();
        const result = await wrapper.vm.quickSave();
        await flushPromises();

        expect(result).toBe(true);
        const commitCalls = invokeMock.mock.calls.filter(([c]) => c === 'develop_commit_recipe');
        expect(commitCalls.length).toBe(1);
        expect(commitCalls[0][1]).toEqual(
            expect.objectContaining({ sessionId: 107, expectedRevision: 0 }),
        );
        expect(invokeMock.mock.calls.some(([c]) => c === 'edit_image')).toBe(false);
        expect(editor.dirty.value).toBe(false);
        expect(editor.saveState.value).toBe('saved');
    });

    it('fails without writing pixels when the recipe commit fails', async () => {
        const editor = useDevelopEditor();
        queueCommand('develop_open_edit_session', openedSession(7));
        await editor.openAsset({ id: 7 });
        editor.setParam('exposure', 0.5);

        queueCommand('develop_commit_recipe', new Error('failed to write sidecar: access denied'));

        const wrapper = await mountFileInfo();
        const result = await wrapper.vm.quickSave();
        await flushPromises();

        expect(result).toBe(false);
        expect(invokeMock.mock.calls.some(([c]) => c === 'edit_image')).toBe(false);
        expect(editor.dirty.value).toBe(true);
        expect(editor.saveState.value).toBe('failed');
    });

    it('keeps the legacy editImage path for non-develop CSS adjustments', async () => {
        const wrapper = await mountFileInfo();

        // The component's own Pinia instance (the editor's legacy CSS path).
        const uiStore = useUIStore();
        uiStore.setActiveAdjustments('C:/photos/photo.CR2', { brightness: 10 });

        queueCommand('edit_image', true);

        const result = await wrapper.vm.quickSave();
        await flushPromises();

        expect(result).toBe(true);
        expect(invokeMock.mock.calls.some(([c]) => c === 'edit_image')).toBe(true);
        expect(invokeMock.mock.calls.some(([c]) => c === 'develop_commit_recipe')).toBe(false);
    });
});
