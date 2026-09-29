import { mount } from '@vue/test-utils';
import { createI18n } from 'vue-i18n';
import { describe, it, expect, vi } from 'vitest';
import ColorBalanceWheel from '@/components/develop/ColorBalanceWheel.vue';
import en from '@/locales/en.json';

function wheel(value = { hue: 0, saturation: 0, luminance: 0 }, master = false) {
    const w = mount(ColorBalanceWheel, {
        props: { value, zone: master ? 'global' : 'shadows', label: 'Shadows', master },
        global: { plugins: [createI18n({ legacy: false, locale: 'en', messages: { en } })] },
    });
    vi.spyOn(w.get('svg').element, 'getBoundingClientRect').mockReturnValue({ x: 0, y: 0, left: 0, top: 0, width: 320, height: 240, right: 320, bottom: 240, toJSON() {} });
    return w;
}

describe('Color Balance wheel', () => {
    it('drags the puck through the full hue circle and clamps saturation with one gesture', async () => {
        const w = wheel();
        const target = w.get('[data-control="puck"]');
        await target.trigger('pointerdown', { button: 0, pointerId: 1, clientX: 160, clientY: 170 });
        await w.get('svg').trigger('pointermove', { pointerId: 1, clientX: 160, clientY: 260 });
        await w.get('svg').trigger('pointerup', { pointerId: 1, clientX: 160, clientY: 260 });
        expect(w.emitted('live')?.[0]?.[0]).toMatchObject({ hue: 270, saturation: 50 });
        expect(w.emitted('live')?.at(-1)?.[0]).toMatchObject({ hue: 270, saturation: 100 });
        expect(w.emitted('settle')).toHaveLength(1);
        w.unmount();
    });
    it('changes hue on the rim without changing saturation or luminance', async () => {
        const w = wheel({ hue: 20, saturation: 35, luminance: 12 });
        await w.get('[data-control="hue"]').trigger('pointerdown', { button: 0, pointerId: 1, clientX: 60, clientY: 120 });
        await w.get('svg').trigger('pointerup', { pointerId: 1, clientX: 60, clientY: 120 });
        expect(w.emitted('live')?.[0]?.[0]).toEqual({ hue: 180, saturation: 35, luminance: 12 });
        w.unmount();
    });
    it('supports keyboard hue/saturation changes and double-click neutralization', async () => {
        const w = wheel({ hue: 359, saturation: 35, luminance: 12 });
        await w.get('[data-control="puck"]').trigger('keydown', { key: 'ArrowRight' });
        expect(w.emitted('commit')?.[0]?.[0]).toEqual({ hue: 0, saturation: 35, luminance: 12 });
        await w.get('[data-control="puck"]').trigger('dblclick');
        expect(w.emitted('commit')?.[1]?.[0]).toEqual({ hue: 359, saturation: 0, luminance: 12 });
        w.unmount();
    });
    it('adjusts independent saturation and lightness arcs and ends canceled pointer gestures once', async () => {
        const w = wheel({ hue: 240, saturation: 30, luminance: 12 });
        await w.get('[data-control="saturation"]').trigger('pointerdown', { button: 0, pointerId: 1, clientX: 24, clientY: 120 });
        expect(w.emitted('live')?.[0]?.[0]).toEqual({ hue: 240, saturation: 50, luminance: 12 });
        await w.get('svg').trigger('pointercancel', { pointerId: 1 });
        await w.get('svg').trigger('lostpointercapture', { pointerId: 1 });
        expect(w.emitted('settle')).toHaveLength(1);
        await w.get('[data-control="luminance"]').trigger('pointerdown', { button: 0, pointerId: 2, clientX: 296, clientY: 120 });
        expect(w.emitted('live')?.at(-1)?.[0]).toEqual({ hue: 240, saturation: 30, luminance: 0 });
        w.unmount();
    });
    it('commits numeric hue on Enter and ignores empty input', async () => {
        const w = wheel();
        const input = w.get('[data-testid="develop-input-grading-shadows-hue"]');
        (input.element as HTMLInputElement).value = '240';
        await input.trigger('keydown', { key: 'Enter' });
        expect(w.emitted('commit')?.[0]?.[0]).toEqual({ hue: 240, saturation: 0, luminance: 0 });
        (input.element as HTMLInputElement).value = '';
        await input.trigger('change');
        expect(w.emitted('commit')).toHaveLength(1);
        w.unmount();
    });
    it('disables Master lightness and closes canceled gestures on unmount', async () => {
        const w = wheel({ hue: 0, saturation: 0, luminance: 7 }, true);
        expect(w.get('[data-control="luminance"]').attributes('aria-disabled')).toBe('true');
        await w.get('[data-control="luminance"]').trigger('pointerdown', { button: 0, pointerId: 1, clientX: 290, clientY: 50 });
        expect(w.emitted('live')).toBeUndefined();
        await w.get('[data-control="puck"]').trigger('pointerdown', { button: 0, pointerId: 2, clientX: 200, clientY: 120 });
        w.unmount();
        expect(w.emitted('settle')).toHaveLength(1);
    });
});
