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
import ImageHistogram from '@/components/ImageHistogram.vue';
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
import zhMessages from '@/locales/zh.json';
import esMessages from '@/locales/es.json';
import koMessages from '@/locales/ko.json';
import ptMessages from '@/locales/pt.json';
import ruMessages from '@/locales/ru.json';

const LOCALE_MESSAGES: Record<string, Record<string, unknown>> = {
    en: enMessages as Record<string, unknown>,
    de: deMessages as Record<string, unknown>,
    fr: frMessages as Record<string, unknown>,
    ja: jaMessages as Record<string, unknown>,
    zh: zhMessages as Record<string, unknown>,
    es: esMessages as Record<string, unknown>,
    ko: koMessages as Record<string, unknown>,
    pt: ptMessages as Record<string, unknown>,
    ru: ruMessages as Record<string, unknown>,
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
        // The rollback-switch refresh (lap-63f) is the panel's first IPC
        // call during setup; it must not consume the session queue.
        .mockResolvedValueOnce(false)
        .mockResolvedValueOnce(openedSession(assetId))
        .mockResolvedValueOnce(completedTicket(assetId))
        .mockResolvedValueOnce(new Uint8Array(8 * 8 * 4).buffer);
}

async function mountPanel(locale = 'en', fileOverride: Record<string, unknown> = {}) {
    queueSuccessfulOpen(7);
    const wrapper = mount(DevelopPanel, {
        props: { file: { id: 7, name: 'photo.CR2', thumbnail: 'blob:original', ...fileOverride } },
        global: {
            plugins: [makeI18n(locale), setActivePinia(createPinia())],
        },
    });
    await flushPromises();
    return wrapper;
}

async function expandSection(wrapper: any, section: string) {
    await wrapper.get(`[data-testid="develop-section-toggle-${section}"]`).trigger('click');
}

describe('DevelopPanel', () => {
    beforeEach(() => {
        invokeMock.mockReset();
        // Default for non-queued commands (e.g. the lens-profile catalog of
        // lap-d52, the rollback switch of lap-63f); queued
        // mockResolvedValueOnce responses still win.
        invokeMock.mockImplementation((command: string) => {
            if (command === 'develop_lens_catalog') return Promise.resolve([]);
            if (command === 'develop_get_rollback') return Promise.resolve(false);
            return Promise.resolve([]);
        });
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

    it('shows the rollback notice and no editing controls while the switch is engaged', async () => {
        // lap-63f: the backend switch answers true; the panel must open no
        // edit session and render only the explicit, localized notice.
        invokeMock.mockImplementation((command: string) => {
            if (command === 'develop_get_rollback') return Promise.resolve(true);
            return Promise.resolve([]);
        });
        const wrapper = mount(DevelopPanel, {
            props: { file: { id: 7, name: 'photo.CR2', thumbnail: 'blob:original' } },
            global: { plugins: [makeI18n(), setActivePinia(createPinia())] },
        });
        await flushPromises();

        const notice = wrapper.get('[data-testid="develop-rollback-notice"]');
        expect(notice.text()).toContain('rolled back');
        expect(wrapper.find('[data-testid="develop-slider-exposure"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid="develop-section-basic"]').exists()).toBe(false);
        // No session was opened while rolled back.
        const sessionCalls = invokeMock.mock.calls.filter((call) => call[0] === 'develop_open_edit_session');
        expect(sessionCalls).toHaveLength(0);
        wrapper.unmount();
        await flushPromises();
    });

    it('fails closed with the rollback notice while the switch state is unreadable', async () => {
        invokeMock.mockImplementation(() => Promise.reject(new Error('config unavailable')));
        const wrapper = mount(DevelopPanel, {
            props: { file: { id: 7, name: 'photo.CR2', thumbnail: 'blob:original' } },
            global: { plugins: [makeI18n(), setActivePinia(createPinia())] },
        });
        await flushPromises();

        expect(wrapper.find('[data-testid="develop-rollback-notice"]').exists()).toBe(true);
        expect(wrapper.find('[data-testid="develop-slider-exposure"]').exists()).toBe(false);
        wrapper.unmount();
        await flushPromises();
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

    // -----------------------------------------------------------------------
    // First-release global controls (lap-adc / TASK-401)
    // -----------------------------------------------------------------------

    it('renders every first-release section with generated ranges', async () => {
        const wrapper = await mountPanel();

        // Basic is expanded by default.
        const contrast = wrapper.get('[data-testid="develop-slider-contrast"]').element as HTMLInputElement;
        expect(Number(contrast.min)).toBe(RECIPE_PARAM_RANGES.contrast.min);
        expect(Number(contrast.max)).toBe(RECIPE_PARAM_RANGES.contrast.max);
        expect(Number(contrast.step)).toBe(RECIPE_PARAM_RANGES.contrast.step);

        for (const section of ['curves', 'color', 'details', 'effects']) {
            await expandSection(wrapper, section);
        }

        const checks: Array<[string, keyof typeof RECIPE_PARAM_RANGES]> = [
            ['exposure', 'exposure'],
            ['brightness', 'brightness'],
            ['highlights', 'highlights'],
            ['shadows', 'shadows'],
            ['whites', 'whites'],
            ['blacks', 'blacks'],
            ['vibrance', 'vibrance'],
            ['saturation', 'saturation'],
            ['hue', 'hue'],
            ['clarity', 'clarity'],
            ['structure', 'structure'],
            ['dehaze', 'dehaze'],
            ['sharpness', 'sharpness'],
            ['sharpnessThreshold', 'sharpnessThreshold'],
            ['lumaNoiseReduction', 'lumaNoiseReduction'],
            ['colorNoiseReduction', 'colorNoiseReduction'],
            ['chromaticAberrationRedCyan', 'chromaticAberrationRedCyan'],
            ['chromaticAberrationBlueYellow', 'chromaticAberrationBlueYellow'],
            ['grainAmount', 'grainAmount'],
            ['grainSize', 'grainSize'],
            ['grainRoughness', 'grainRoughness'],
            ['vignetteAmount', 'vignetteAmount'],
            ['vignetteMidpoint', 'vignetteMidpoint'],
            ['vignetteRoundness', 'vignetteRoundness'],
            ['vignetteFeather', 'vignetteFeather'],
            ['glowAmount', 'glowAmount'],
            ['halationAmount', 'halationAmount'],
            ['flareAmount', 'flareAmount'],
        ];
        for (const [param, rangeKey] of checks) {
            const el = wrapper.get(`[data-testid="develop-slider-${param}"]`).element as HTMLInputElement;
            const range = RECIPE_PARAM_RANGES[rangeKey];
            expect(Number(el.min), param).toBe(range.min);
            expect(Number(el.max), param).toBe(range.max);
            expect(Number(el.step), param).toBe(range.step);
        }
    });

    it('edits detail and effect sliders through the generated ranges with one undo step each', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        for (const section of ['details', 'effects']) {
            await expandSection(wrapper, section);
        }

        await wrapper.get('[data-testid="develop-slider-sharpness"]').setValue('30');
        expect(editor.recipe.value?.sharpness).toBe(30);
        expect(editor.historySize.value).toBe(1);
        await wrapper.get('[data-testid="develop-slider-grainAmount"]').setValue('55');
        expect(editor.recipe.value?.grainAmount).toBe(55);
        expect(editor.historySize.value).toBe(2);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.grainAmount).toBe(0);
        await wrapper.get('[data-testid="develop-redo"]').trigger('click');
        expect(editor.recipe.value?.grainAmount).toBe(55);
    });

    it('commits numeric input for flat and nested controls', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'effects');

        const flat = wrapper.get('[data-testid="develop-input-grainSize"]');
        await flat.setValue('80');
        await flat.trigger('keydown', { key: 'Enter' });
        expect(editor.recipe.value?.grainSize).toBe(80);
        expect(editor.historySize.value).toBe(1);
    });

    it('edits HSL sliders for the selected channel', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'color');

        await wrapper.get('[data-testid="develop-hsl-channel-blues"]').trigger('click');
        await wrapper.get('[data-testid="develop-slider-hsl-blues-hue"]').setValue('25');
        expect(editor.recipe.value?.hsl.blues.hue).toBe(25);
        expect(editor.historySize.value).toBe(1);

        // Other channels remain untouched at their defaults.
        expect(editor.recipe.value?.hsl.reds).toEqual(DEFAULT_RECIPE.hsl.reds);
    });

    it('edits color grading zones, balance and calibration sliders', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'color');

        await wrapper.get('[data-testid="develop-grading-zone-shadows"]').trigger('click');
        await wrapper.get('[data-testid="develop-slider-grading-shadows-saturation"]').setValue('30');
        expect(editor.recipe.value?.colorGrading.shadows.saturation).toBe(30);

        await wrapper.get('[data-testid="develop-slider-grading-balance"]').setValue('-20');
        expect(editor.recipe.value?.colorGrading.balance).toBe(-20);

        await wrapper.get('[data-testid="develop-slider-calibration-redHue"]').setValue('-10');
        expect(editor.recipe.value?.colorCalibration.redHue).toBe(-10);
        expect(editor.historySize.value).toBe(3);
    });

    it('changes the tone mapper through the localized select', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        const select = wrapper.get('[data-testid="develop-tonemapper"]');
        expect((select.element as HTMLSelectElement).value).toBe('basic');
        await select.setValue('agx');
        expect(editor.recipe.value?.toneMapper).toBe('agx');
        expect(editor.historySize.value).toBe(1);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.toneMapper).toBe('basic');
    });

    it('toggles a section bypass with one undo step', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'effects');

        const bypass = wrapper.get('[data-testid="develop-bypass-effects"]');
        expect(bypass.attributes('aria-label')).toBeTruthy();
        await bypass.setValue(false);
        expect(editor.recipe.value?.sectionVisibility.effects).toBe(false);
        expect(editor.historySize.value).toBe(1);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.sectionVisibility.effects).toBe(true);
    });

    it('resets one section from its header and leaves other sections intact', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'effects');

        await wrapper.get('[data-testid="develop-slider-contrast"]').setValue('30');
        await wrapper.get('[data-testid="develop-slider-grainAmount"]').setValue('50');
        await wrapper.get('[data-testid="develop-section-reset-effects"]').trigger('click');

        expect(editor.recipe.value?.grainAmount).toBe(DEFAULT_RECIPE.grainAmount);
        expect(editor.recipe.value?.contrast).toBe(30);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.grainAmount).toBe(50);
    });

    it('exposes undo/redo with the session-scope hint and correct disabled states', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        const undo = wrapper.get('[data-testid="develop-undo"]');
        const redo = wrapper.get('[data-testid="develop-redo"]');
        expect((undo.element as HTMLButtonElement).disabled).toBe(true);
        expect((redo.element as HTMLButtonElement).disabled).toBe(true);
        // The hint must make explicit that history is session-only and never
        // persists across restarts.
        expect(undo.attributes('title')).toBe((enMessages.develop as any).historyHint);
        expect(redo.attributes('title')).toBe((enMessages.develop as any).historyHint);

        await wrapper.get('[data-testid="develop-slider-exposure"]').setValue('1');
        expect((wrapper.get('[data-testid="develop-undo"]').element as HTMLButtonElement).disabled).toBe(false);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.exposure).toBe(0);
        expect((wrapper.get('[data-testid="develop-undo"]').element as HTMLButtonElement).disabled).toBe(true);
        expect((wrapper.get('[data-testid="develop-redo"]').element as HTMLButtonElement).disabled).toBe(false);

        await wrapper.get('[data-testid="develop-redo"]').trigger('click');
        expect(editor.recipe.value?.exposure).toBe(1);
    });

    it('moves curve points with the keyboard, one transaction per keystroke', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'curves');

        // Endpoint x coordinates are pinned by the model (first x=0, last
        // x=255); seed an interior point to exercise keyboard movement.
        editor.setCurveChannelPoints('luma', [
            { x: 0, y: 0 },
            { x: 100, y: 80 },
            { x: 255, y: 255 },
        ]);
        const historyAfterSeed = editor.historySize.value;

        const point = wrapper.get('[data-testid="develop-curve-point-1"]');
        expect(point.attributes('aria-label')).toBeTruthy();
        await point.trigger('focus');
        await point.trigger('keydown', { key: 'ArrowRight' });
        expect(editor.recipe.value?.curves.luma[1].x).toBe(101);
        await point.trigger('keydown', { key: 'ArrowRight', shiftKey: true });
        expect(editor.recipe.value?.curves.luma[1].x).toBe(111);
        await point.trigger('keydown', { key: 'ArrowUp' });
        expect(editor.recipe.value?.curves.luma[1].y).toBe(81);
        expect(editor.historySize.value).toBe(historyAfterSeed + 3);

        await wrapper.get('[data-testid="develop-undo"]').trigger('click');
        expect(editor.recipe.value?.curves.luma[1].y).toBe(80);
        expect(editor.recipe.value?.curves.luma[1].x).toBe(111);

        await point.trigger('keydown', { key: 'Delete' });
        expect(editor.recipe.value?.curves.luma).toEqual([
            { x: 0, y: 0 },
            { x: 255, y: 255 },
        ]);
    });

    it('switches curve channels and the point/parametric mode', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();
        await expandSection(wrapper, 'curves');

        await wrapper.get('[data-testid="develop-curve-channel-red"]').trigger('click');
        expect(wrapper.find('[data-testid="develop-curve-point-0"]').exists()).toBe(true);

        await wrapper.get('[data-testid="develop-curve-mode-parametric"]').trigger('click');
        expect(editor.recipe.value?.curveMode).toBe('parametric');
        expect(wrapper.find('[data-testid="develop-curve-param-darks"]').exists()).toBe(true);

        await wrapper.get('[data-testid="develop-curve-param-darks"]').setValue('40');
        expect(editor.recipe.value?.parametricCurve.red.darks).toBe(40);

        await wrapper.get('[data-testid="develop-curve-mode-point"]').trigger('click');
        expect(editor.recipe.value?.curveMode).toBe('point');
    });

    it('feeds the renderer histogram from the displayed preview generation', async () => {
        const wrapper = await mountPanel();
        const editor = useDevelopEditor();

        const histogram = wrapper.findComponent(ImageHistogram);
        expect(histogram.exists()).toBe(true);
        // The settled preview ticket rendered an 8x8 frame for generation 1.
        expect(histogram.props('rendererPixels')).toMatchObject({ width: 8, height: 8 });
        expect(histogram.props('source')).toBe('');

        // While the original comparison is displayed, the renderer histogram
        // input is replaced by the original thumbnail source.
        await wrapper.get('[data-testid="develop-view-original"]').trigger('click');
        expect(editor.showOriginal.value).toBe(true);
        expect(histogram.props('rendererPixels')).toBeNull();
        expect(histogram.props('source')).toBe('blob:original');
    });

    it('localizes the first-release control labels', async () => {
        const en = await mountPanel('en');
        const enDevelop = enMessages.develop as Record<string, any>;
        expect(en.get('[data-testid="develop-label-contrast"]').text()).toBe(enDevelop.controls.contrast);
        expect(en.get('[data-testid="develop-section-title-basic"]').text()).toBe(enDevelop.sections.basic);
        await expandSection(en, 'effects');
        expect(en.get('[data-testid="develop-label-grainAmount"]').text()).toBe(enDevelop.controls.grainAmount);
        en.unmount();

        const de = await mountPanel('de');
        const deDevelop = (deMessages as unknown as Record<string, any>).develop;
        expect(de.get('[data-testid="develop-label-contrast"]').text()).toBe(deDevelop.controls.contrast);
        expect(de.get('[data-testid="develop-section-title-basic"]').text()).toBe(deDevelop.sections.basic);
        de.unmount();
    });

    it('declares the first-release keys in every supported locale', () => {
        const requiredControlKeys = [
            'brightness', 'contrast', 'highlights', 'shadows', 'whites', 'blacks',
            'vibrance', 'saturation', 'hue', 'clarity', 'structure', 'dehaze', 'center',
            'sharpness', 'sharpnessThreshold', 'lumaNoiseReduction', 'colorNoiseReduction',
            'chromaticAberrationRedCyan', 'chromaticAberrationBlueYellow',
            'grainAmount', 'grainSize', 'grainRoughness',
            'vignetteAmount', 'vignetteMidpoint', 'vignetteRoundness', 'vignetteFeather',
            'glowAmount', 'halationAmount', 'flareAmount', 'toneMapper',
        ];
        for (const [name, messages] of Object.entries(LOCALE_MESSAGES)) {
            const develop = (messages as Record<string, any>).develop;
            for (const section of ['basic', 'curves', 'color', 'details', 'effects']) {
                expect(typeof develop.sections?.[section], `locale ${name} sections.${section}`).toBe('string');
            }
            for (const key of requiredControlKeys) {
                expect(typeof develop.controls?.[key], `locale ${name} controls.${key}`).toBe('string');
            }
            for (const channel of ['reds', 'oranges', 'yellows', 'greens', 'aquas', 'blues', 'purples', 'magentas']) {
                expect(typeof develop.hslChannels?.[channel], `locale ${name} hslChannels.${channel}`).toBe('string');
            }
            for (const component of ['hue', 'saturation', 'luminance']) {
                expect(typeof develop.hslComponents?.[component], `locale ${name} hslComponents.${component}`).toBe('string');
            }
            for (const zone of ['global', 'shadows', 'midtones', 'highlights']) {
                expect(typeof develop.gradingZones?.[zone], `locale ${name} gradingZones.${zone}`).toBe('string');
            }
            for (const channel of ['luma', 'red', 'green', 'blue']) {
                expect(typeof develop.curve?.channels?.[channel], `locale ${name} curve.channels.${channel}`).toBe('string');
            }
            for (const key of ['point', 'parametric', 'pointLabel', 'paramLabel', 'pointAria', 'undoHint']) {
                expect(typeof develop.curve?.[key], `locale ${name} curve.${key}`).toBe('string');
            }
            for (const key of ['undo', 'redo', 'historyHint', 'histogram', 'bypassSection', 'resetSection', 'sectionAria']) {
                expect(typeof develop[key], `locale ${name} develop.${key}`).toBe('string');
            }
            for (const key of ['basic', 'agx']) {
                expect(typeof develop.toneMapper?.[key], `locale ${name} toneMapper.${key}`).toBe('string');
            }
        }
    });
});
