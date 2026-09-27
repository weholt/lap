import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { createPinia, setActivePinia } from 'pinia';

// Component regressions for the native Develop panel (lap-0e9 / TASK-303;
// managed continuation of lap-f19.3). Spec refs: docs/raw-development/spec.md
// A10 ("keyboard/numeric input, localization, original comparison, save
// status") and A2 (per-asset settings isolation).
//
// The range/step metadata must come from the generated engine descriptors
// (RECIPE_PARAM_RANGES), not from hand-copied panel constants.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import DevelopPanel from '@/components/DevelopPanel.vue';
import { useDevelopEditor } from '@/composables/useDevelopEditor';
import {
    DEFAULT_RECIPE,
    RECIPE_PARAM_RANGES,
    RECIPE_SCHEMA_VERSION,
} from '@/composables/useDevelopSession.types';
import enMessages from '@/locales/en.json';
import deMessages from '@/locales/de.json';
import frMessages from '@/locales/fr.json';
import jaMessages from '@/locales/ja.json';

const LOCALE_MESSAGES: Record<string, Record<string, unknown>> = {
    en: enMessages as Record<string, unknown>,
    de: deMessages as Record<string, unknown>,
    fr: frMessages as Record<string, unknown>,
    ja: jaMessages as Record<string, unknown>,
};

function makeI18n(locale = 'en') {
    return createI18n({
        legacy: false,
        locale,
        fallbackLocale: 'en',
        messages: LOCALE_MESSAGES,
    });
}

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

function completedTicket(assetId: number) {
    return {
        status: 'completed',
        ticket: {
            sessionId: 100 + assetId,
            assetId: String(assetId),
            variantId: 'default',
            generation: 1,
            quality: 'settled',
            width: 8,
            height: 8,
            handle: `handle-${assetId}`,
            byteLen: 8 * 8 * 4,
        },
    };
}

function queueSuccessfulOpen(assetId: number) {
    invokeMock
        .mockResolvedValueOnce(openedSession(assetId))
        .mockResolvedValueOnce(completedTicket(assetId))
        .mockResolvedValueOnce(new Uint8Array(8 * 8 * 4).buffer);
}

async function mountPanel(locale = 'en') {
    queueSuccessfulOpen(7);
    const wrapper = mount(DevelopPanel, {
        props: { file: { id: 7, name: 'photo.CR2' } },
        global: {
            plugins: [makeI18n(locale), setActivePinia(createPinia())],
        },
    });
    await flushPromises();
    return wrapper;
}

describe('DevelopPanel', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        setActivePinia(createPinia());
    });

    afterEach(async () => {
        await useDevelopEditor().disposeForTests();
        vi.restoreAllMocks();
    });

    it('uses the generated engine descriptors for exposure and white balance ranges', async () => {
        const wrapper = await mountPanel();

        const exposure = wrapper.get('[data-testid="develop-slider-exposure"]').element as HTMLInputElement;
        expect(Number(exposure.min)).toBe(RECIPE_PARAM_RANGES.exposure.min);
        expect(Number(exposure.max)).toBe(RECIPE_PARAM_RANGES.exposure.max);
        expect(Number(exposure.step)).toBe(RECIPE_PARAM_RANGES.exposure.step);

        const temperature = wrapper.get('[data-testid="develop-slider-temperature"]').element as HTMLInputElement;
        expect(Number(temperature.min)).toBe(RECIPE_PARAM_RANGES.temperature.min);
        expect(Number(temperature.max)).toBe(RECIPE_PARAM_RANGES.temperature.max);

        const tint = wrapper.get('[data-testid="develop-slider-tint"]').element as HTMLInputElement;
        expect(Number(tint.min)).toBe(RECIPE_PARAM_RANGES.tint.min);
        expect(Number(tint.max)).toBe(RECIPE_PARAM_RANGES.tint.max);
    });

    it('edits exposure through keyboard/numeric input and Enter', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        const input = wrapper.get('[data-testid="develop-input-exposure"]');
        await input.setValue('0.75');
        await input.trigger('keydown', { key: 'Enter' });

        expect(editor.recipe.value?.exposure).toBe(0.75);
        expect(editor.dirty.value).toBe(true);
        expect(editor.saveState.value).toBe('pending');
    });

    it('edits white balance through the sliders', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        await wrapper.get('[data-testid="develop-slider-temperature"]').setValue('-40');
        await wrapper.get('[data-testid="develop-slider-tint"]').setValue('15');

        expect(editor.recipe.value?.temperature).toBe(-40);
        expect(editor.recipe.value?.tint).toBe(15);
    });

    it('rejects non-numeric input instead of corrupting the recipe', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        const input = wrapper.get('[data-testid="develop-input-exposure"]');
        await input.setValue('not-a-number');
        await input.trigger('change');

        expect(editor.recipe.value?.exposure).toBe(0);
        expect(editor.dirty.value).toBe(false);
    });

    it('resets a single control and all controls to recipe defaults', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        await wrapper.get('[data-testid="develop-slider-exposure"]').setValue('2');
        await wrapper.get('[data-testid="develop-reset-exposure"]').trigger('click');
        expect(editor.recipe.value?.exposure).toBe(DEFAULT_RECIPE.exposure);

        await wrapper.get('[data-testid="develop-slider-temperature"]').setValue('50');
        await wrapper.get('[data-testid="develop-slider-tint"]').setValue('-50');
        await wrapper.get('[data-testid="develop-reset-all"]').trigger('click');

        expect(editor.recipe.value?.temperature).toBe(DEFAULT_RECIPE.temperature);
        expect(editor.recipe.value?.tint).toBe(DEFAULT_RECIPE.tint);
        expect(editor.recipe.value?.exposure).toBe(DEFAULT_RECIPE.exposure);
    });

    it('toggles the original comparison view', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        const toggle = () => wrapper.get('[data-testid="develop-view-original"]');

        expect(toggle().attributes('aria-pressed')).toBe('false');

        await toggle().trigger('click');
        expect(toggle().attributes('aria-pressed')).toBe('true');
        expect(editor.showOriginal.value).toBe(true);

        await toggle().trigger('click');
        expect(toggle().attributes('aria-pressed')).toBe('false');
        expect(editor.showOriginal.value).toBe(false);
    });

    it('shows the localized failed save status and retries the commit', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        editor.setParam('exposure', 1.25);
        invokeMock.mockRejectedValueOnce(new Error('failed to write sidecar: access denied'));
        expect(await editor.flush()).toBe(false);
        await flushPromises();

        const status = wrapper.get('[data-testid="develop-save-status"]');
        expect(status.text()).toBe(enMessages.develop.save.failed);
        expect(wrapper.get('[data-testid="develop-retry"]').isVisible()).toBe(true);

        invokeMock.mockResolvedValueOnce({
            sessionId: 107,
            revision: 1,
            contentHash: 'c'.repeat(64),
            sidecarPath: 'C:/photos/photo.CR2.lapedit.json',
            projectionApplied: true,
            projectionError: null,
        });
        await wrapper.get('[data-testid="develop-retry"]').trigger('click');
        await flushPromises();

        expect(editor.saveState.value).toBe('saved');
        expect(wrapper.get('[data-testid="develop-save-status"]').text()).toBe(
            enMessages.develop.save.saved,
        );
        expect(wrapper.find('[data-testid="develop-retry"]').exists()).toBe(false);
    });

    it('localizes control labels for the active locale', async () => {
        const en = await mountPanel('en');
        expect(en.get('[data-testid="develop-label-exposure"]').text()).toBe(
            (enMessages.develop as { exposure: string }).exposure,
        );
        expect(en.get('[data-testid="develop-label-temperature"]').text()).toBe(
            (enMessages.develop as { temperature: string }).temperature,
        );
        en.unmount();

        const de = await mountPanel('de');
        expect(de.get('[data-testid="develop-label-exposure"]').text()).toBe(
            (deMessages as unknown as { develop: { exposure: string } }).develop.exposure,
        );
        de.unmount();
    });

    it('declares the develop keys in every supported locale', () => {
        for (const [name, messages] of Object.entries(LOCALE_MESSAGES)) {
            const develop = (messages as Record<string, any>).develop;
            expect(develop, `locale ${name} must declare develop keys`).toBeTruthy();
            for (const key of ['title', 'exposure', 'temperature', 'tint', 'whiteBalance', 'resetAll', 'viewOriginal']) {
                expect(typeof develop[key], `locale ${name} key develop.${key}`).toBe('string');
            }
            for (const key of ['idle', 'pending', 'saving', 'saved', 'conflict', 'failed']) {
                expect(typeof develop.save[key], `locale ${name} key develop.save.${key}`).toBe('string');
            }
        }
    });
});
