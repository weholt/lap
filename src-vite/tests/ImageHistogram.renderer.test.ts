import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';

// Renderer-input regressions for the histogram (lap-a58 / TASK-305;
// governing contract docs/raw-development/spec.md, A6 and the "Image
// analytics" requirement): the histogram must accept renderer analytics
// (bins) or already-rendered pixels, and its legacy simulated filter
// calculation must never apply the adjustments a second time on rendered
// input. A delayed/stale renderer frame must never overwrite newer data.

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (...args: unknown[]) => invokeMock(...args),
}));

import ImageHistogram from '@/components/ImageHistogram.vue';

const BIN_COUNT = 256;

function sparseBins(populatedBins: number[], value = 1000): number[] {
    const bins = new Array<number>(BIN_COUNT).fill(0);
    for (const bin of populatedBins) bins[bin] = value;
    return bins;
}

async function mountHistogram(props: Record<string, unknown> = {}) {
    const wrapper = mount(ImageHistogram, {
        props,
        global: {
            mocks: {
                $t: (key: string) => key,
            },
        },
    });
    // Renderer inputs are applied asynchronously (stale-frame guard); settle
    // the pending update before the test asserts.
    await new Promise((resolve) => setTimeout(resolve, 10));
    return wrapper;
}

describe('ImageHistogram renderer inputs', () => {
    let wrapper: Awaited<ReturnType<typeof mountHistogram>> | null = null;

    beforeEach(() => {
        invokeMock.mockReset();
        setActivePinia(createPinia());
    });

    afterEach(() => {
        vi.restoreAllMocks();
        wrapper?.unmount();
        wrapper = null;
    });

    it('builds the histogram from renderer analytics bins without canvas or simulated adjustments', async () => {
        wrapper = await mountHistogram({
            rendererBins: { luma: sparseBins([10, 11]), red: sparseBins([200]) },
        });

        const bins = (wrapper.vm as any).currentBins();
        expect(bins.luma[10]).toBeGreaterThan(0);
        expect(bins.luma[11]).toBeGreaterThan(0);
        expect(bins.luma[200]).toBe(0);
        expect(bins.red[200]).toBeGreaterThan(0);
        expect(bins.red[10]).toBe(0);
        // The renderer input path never fetches or decodes a source image.
        expect(invokeMock).not.toHaveBeenCalled();
    });

    it('computes bins from already-rendered RGBA pixels', async () => {
        // 2x2 opaque pure-red pixels.
        const data = new Uint8ClampedArray([
            255, 0, 0, 255, 255, 0, 0, 255,
            255, 0, 0, 255, 255, 0, 0, 255,
        ]);
        wrapper = await mountHistogram({
            rendererPixels: { data, width: 2, height: 2 },
        });

        const bins = (wrapper.vm as any).currentBins();
        expect(bins.red[255]).toBeGreaterThan(0);
        expect(bins.green[255]).toBe(0);
        expect(bins.blue[255]).toBe(0);
        // Luma of pure red (0.2126 * 255) lands around bin 54.
        expect(bins.luma[54]).toBeGreaterThan(0);
    });

    it('never applies the simulated adjustments again on rendered input', async () => {
        wrapper = await mountHistogram({
            rendererBins: { luma: sparseBins([128]) },
            applyAdjustments: true,
            adjustments: { brightness: 100, contrast: 80, saturation: 200, hue: 90, blur: 4, filter: 'invert' },
        });
        const before = (wrapper.vm as any).currentBins();

        // Simulated-adjustment prop churn (the double-application regression)
        // must not change the rendered histogram.
        await wrapper.setProps({
            adjustments: { brightness: -100, contrast: -80, saturation: 0, hue: -90, blur: 0, filter: 'grayscale' },
        });
        const after = (wrapper.vm as any).currentBins();
        expect(after.luma[128]).toBe(before.luma[128]);
    });

    it('drops a delayed stale renderer frame so it never overwrites newer data', async () => {
        wrapper = await mountHistogram({
            rendererBins: { luma: sparseBins([30]) },
        });

        // Newer frame first, then the stale one arrives late (same tick order
        // as a slow renderer reply racing a fresh render).
        await wrapper.setProps({ rendererBins: { luma: sparseBins([220]) } });
        await wrapper.setProps({ rendererBins: { luma: sparseBins([30]) } });
        await new Promise((resolve) => setTimeout(resolve, 80));

        const bins = (wrapper.vm as any).currentBins();
        expect(bins.luma[30]).toBeGreaterThan(0);
        expect(bins.luma[220]).toBe(0);
    });

    it('clears the histogram when the renderer input becomes invalid', async () => {
        wrapper = await mountHistogram({
            rendererBins: { luma: sparseBins([128]) },
        });
        expect((wrapper.vm as any).currentBins().luma[128]).toBeGreaterThan(0);

        await wrapper.setProps({ rendererBins: null });
        await new Promise((resolve) => setTimeout(resolve, 10));
        const bins = (wrapper.vm as any).currentBins();
        expect(bins.luma[128]).toBe(0);
    });
});
