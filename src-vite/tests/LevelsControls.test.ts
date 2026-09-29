import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import LevelsControls from '@/components/develop/LevelsControls.vue';
import { DEFAULT_RECIPE } from '@/composables/useDevelopSession.types';
import { changeLevels, levelsFromPosition, levelsPosition, levelsHistogram } from '@/components/develop/levels';
import { fullResetPatch } from '@/components/develop/controls';
import en from '@/locales/en.json';
const neutral = () => ({ ...DEFAULT_RECIPE.levels.rgb });
function create(disabled = false) {
    return mount(LevelsControls, { props: { value: neutral(), channel: 'rgb', disabled }, global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { en } })] } });
}
describe('Levels', () => {
    it('keeps endpoints ordered and rejects empty/non-finite edits', () => {
        const value = { ...neutral(), inputBlack: 40, inputWhite: 200, outputBlack: 20, outputWhite: 230 };
        expect(changeLevels(value, 'inputBlack', 255).inputBlack).toBe(199);
        expect(changeLevels(value, 'inputWhite', -20).inputWhite).toBe(41);
        expect(changeLevels(value, 'outputBlack', 255).outputBlack).toBe(229);
        expect(changeLevels(value, 'outputWhite', -1).outputWhite).toBe(21);
        expect(changeLevels(value, 'midtone', 9).midtone).toBe(1);
        expect(changeLevels(value, 'inputBlack', NaN)).toBe(value);
        expect(changeLevels(value, 'inputBlack', Infinity)).toBe(value);
        expect(fullResetPatch().levels).toEqual(DEFAULT_RECIPE.levels);
    });
    it('midpoint follows its input interval and leftward movement brightens', () => {
        const value = { ...neutral(), inputBlack: 20, inputWhite: 220 };
        expect(levelsPosition(value, 'midtone')).toBe(120);
        expect(levelsFromPosition(value, 'midtone', 75).midtone).toBe(0.5);
        expect(levelsPosition({ ...value, midtone: -0.5 }, 'midtone')).toBe(165);
    });
    it('counts actual rendered RGB pixels with bounded sampling and validates short buffers', () => {
        const bins = levelsHistogram({ data: [10,20,30,255,10,40,50,255,99,99,99,0], width: 3, height: 1 });
        expect(bins.red[10]).toBe(2); expect(bins.green[40]).toBe(1); expect(bins.blue[50]).toBe(1); expect(bins.red[99]).toBe(0);
        expect(levelsHistogram({ data: [2], width: 20, height: 20 }).red.every(n => n === 0)).toBe(true);
        const data = new Uint8Array(100000 * 4).fill(255);
        expect(levelsHistogram({ data, width: 1000, height: 100 }).red[255]).toBeLessThanOrEqual(65536);
    });
    it('emits continuous drag updates with one settle and captures cancellation', async () => {
        const wrapper = create();
        const svg = wrapper.get('svg');
        vi.spyOn(svg.element, 'getBoundingClientRect').mockReturnValue({ left: 0, width: 280 } as DOMRect);
        await wrapper.get('[data-testid="levels-handle-inputBlack"]').trigger('pointerdown', { button: 0, pointerId: 1, clientX: 12 });
        await svg.trigger('pointermove', { pointerId: 1, clientX: 42 });
        await svg.trigger('pointermove', { pointerId: 1, clientX: 62 });
        expect(wrapper.emitted('live')?.map(args => (args[0] as any).inputBlack)).toEqual([30, 50]);
        await svg.trigger('pointerup', { pointerId: 1, clientX: 62 });
        expect(wrapper.emitted('settle')).toHaveLength(1);
        expect(wrapper.emitted('commit')).toBeUndefined();
        await wrapper.get('[data-testid="levels-handle-inputWhite"]').trigger('pointerdown', { button: 0, pointerId: 2, clientX: 232 });
        await svg.trigger('pointercancel', { pointerId: 2 });
        expect(wrapper.emitted('live')?.at(-1)?.[0]).toEqual(neutral());
        expect(wrapper.emitted('settle')).toHaveLength(2);
        wrapper.unmount();
    });
    it('supports keyboard, clamps numeric entry and disables input without a session', async () => {
        const wrapper = create();
        await wrapper.get('[data-testid="levels-handle-midtone"]').trigger('keydown', { key: 'ArrowLeft', shiftKey: true });
        expect((wrapper.emitted('commit')?.[0]?.[0] as any).midtone).toBe(0.1);
        await wrapper.get('[data-testid="levels-number-inputBlack"]').setValue('999');
        expect((wrapper.emitted('commit')?.at(-1)?.[0] as any).inputBlack).toBe(254);
        await wrapper.setProps({ disabled: true });
        const count = wrapper.emitted('commit')?.length;
        await wrapper.get('[data-testid="levels-handle-inputWhite"]').trigger('keydown', { key: 'ArrowLeft' });
        expect(wrapper.emitted('commit')).toHaveLength(count!);
        expect(wrapper.get('input').attributes('disabled')).toBeDefined();
        wrapper.unmount();
    });
});
