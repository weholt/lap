import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';

// Component tests for the presets panel (lap-62b / TASK-405, spec A9):
// hovering a preset is transient (never saves, never commits), leaving the
// preset cancels and restores the exact prior recipe, and applying a preset
// is exactly one transaction. Fake timers drive the hover delay.

import PresetsPanel, {
    PRESET_HOVER_PREVIEW_DELAY_MS,
    type DevelopPreset,
} from '@/components/develop/PresetsPanel.vue';
import { copySections } from '@/composables/useDevelopClipboard';
import { DEFAULT_RECIPE, type Recipe } from '@/composables/useDevelopSession.types';

function recipe(): Recipe {
    return structuredClone(DEFAULT_RECIPE);
}

function preset(id: string, name: string, values: Partial<Recipe>): DevelopPreset {
    return {
        id,
        name,
        payload: copySections({ ...recipe(), ...values } as Recipe, ['basic']),
    };
}

const PRESETS: DevelopPreset[] = [
    preset('warm', 'Warm Film', { exposure: 0.5, contrast: 10 }),
    preset('cool', 'Cool Blue', { exposure: -0.3, temperature: -15 }),
];

async function mountPanel(current: Recipe | null = recipe()) {
    const wrapper = mount(PresetsPanel, {
        props: { presets: PRESETS, recipe: current },
    });
    return wrapper;
}

function findPreset(wrapper: ReturnType<typeof mount>, id: string) {
    return wrapper.find(`[data-testid="preset-${id}"]`);
}

let wrapper: Awaited<ReturnType<typeof mountPanel>> | null = null;

beforeEach(() => {
    vi.useFakeTimers();
});

afterEach(() => {
    wrapper?.unmount();
    wrapper = null;
    vi.useRealTimers();
});

describe('PresetsPanel hover previews', () => {
    it('emits a transient preview only after the hover delay', async () => {
        wrapper = await mountPanel();
        findPreset(wrapper, 'warm').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS - 1);
        expect(wrapper.emitted('preview')).toBeUndefined();

        await vi.advanceTimersByTimeAsync(1);
        const previews = wrapper.emitted('preview');
        expect(previews).toHaveLength(1);
        const [merged, presetId] = previews![0] as [Recipe, string];
        expect(presetId).toBe('warm');
        expect(merged.exposure).toBe(0.5);
        expect(merged.contrast).toBe(10);
        // The panel never mutates the current recipe prop and never emits apply.
        expect(wrapper.emitted('apply')).toBeUndefined();
        expect(wrapper.props('recipe')?.exposure).toBe(0);
    });

    it('never previews when the pointer leaves before the delay', async () => {
        wrapper = await mountPanel();
        findPreset(wrapper, 'warm').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS - 1);
        findPreset(wrapper, 'warm').trigger('mouseleave');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);

        expect(wrapper.emitted('preview')).toBeUndefined();
        expect(wrapper.emitted('preview-cancel')).toBeUndefined();
    });

    it('cancel restores the exact prior recipe', async () => {
        const current = recipe();
        current.exposure = 0.25;
        current.temperature = 8;
        wrapper = await mountPanel(current);

        findPreset(wrapper, 'cool').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS);
        expect(wrapper.emitted('preview')).toHaveLength(1);

        findPreset(wrapper, 'cool').trigger('mouseleave');
        const cancels = wrapper.emitted('preview-cancel');
        expect(cancels).toHaveLength(1);
        const [restored] = cancels![0] as [Recipe];
        expect(restored).toEqual(current);
        expect(restored.exposure).toBe(0.25);
        expect(restored.temperature).toBe(8);
        expect(wrapper.emitted('apply')).toBeUndefined();
    });

    it('moving between presets cancels the first preview exactly once', async () => {
        wrapper = await mountPanel();
        findPreset(wrapper, 'warm').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS);
        findPreset(wrapper, 'warm').trigger('mouseleave');
        findPreset(wrapper, 'cool').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS);

        expect(wrapper.emitted('preview-cancel')).toHaveLength(1);
        expect(wrapper.emitted('preview')).toHaveLength(2);
    });
});

describe('PresetsPanel apply', () => {
    it('applying emits exactly one apply event with the merged recipe', async () => {
        wrapper = await mountPanel();
        findPreset(wrapper, 'warm').trigger('click');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);

        const applies = wrapper.emitted('apply');
        expect(applies).toHaveLength(1);
        const [merged, presetId] = applies![0] as [Recipe, string];
        expect(presetId).toBe('warm');
        expect(merged.exposure).toBe(0.5);
        // A click without hover never previews and never cancels.
        expect(wrapper.emitted('preview')).toBeUndefined();
        expect(wrapper.emitted('preview-cancel')).toBeUndefined();
    });

    it('applying while a hover preview is active supersedes it in one transaction', async () => {
        wrapper = await mountPanel();
        findPreset(wrapper, 'warm').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS);
        expect(wrapper.emitted('preview')).toHaveLength(1);

        findPreset(wrapper, 'warm').trigger('click');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);

        // Exactly one apply; the transient preview is superseded without an
        // extra cancel event (the applied recipe replaces the displayed one).
        expect(wrapper.emitted('apply')).toHaveLength(1);
        expect(wrapper.emitted('preview-cancel')).toBeUndefined();

        // The hover state is fully reset: leaving afterwards is a no-op.
        findPreset(wrapper, 'warm').trigger('mouseleave');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);
        expect(wrapper.emitted('preview-cancel')).toBeUndefined();
        expect(wrapper.emitted('apply')).toHaveLength(1);
    });

    it('rapid hover cycles never leak previews or applies', async () => {
        wrapper = await mountPanel();
        for (let cycle = 0; cycle < 5; cycle += 1) {
            findPreset(wrapper, 'warm').trigger('mouseenter');
            await vi.advanceTimersByTimeAsync(5);
            findPreset(wrapper, 'warm').trigger('mouseleave');
            await vi.advanceTimersByTimeAsync(5);
        }
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 3);

        expect(wrapper.emitted('preview')).toBeUndefined();
        expect(wrapper.emitted('preview-cancel')).toBeUndefined();
        expect(wrapper.emitted('apply')).toBeUndefined();
    });

    it('renders nothing actionable without a current recipe', async () => {
        wrapper = await mountPanel(null);
        findPreset(wrapper, 'warm').trigger('mouseenter');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);
        findPreset(wrapper, 'warm').trigger('click');
        await vi.advanceTimersByTimeAsync(PRESET_HOVER_PREVIEW_DELAY_MS * 2);

        expect(wrapper.emitted('preview')).toBeUndefined();
        expect(wrapper.emitted('apply')).toBeUndefined();
    });
});
