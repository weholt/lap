import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import DevelopPreviewSurface from '@/components/develop/DevelopPreviewSurface.vue';

vi.mock('@/components/develop/masks/DevelopMaskOverlay.vue', () => ({ default: { props: ['target'], template: '<div />' } }));

function frame(width: number, height: number, generation = 1) {
    return { width, height, generation, handle: `${generation}`, bytes: new ArrayBuffer(width * height * 4) };
}

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe('Develop preview display size', () => {
    it('keeps the same image box and canvas while draft pixels become full-resolution pixels', async () => {
        let resized!: ResizeObserverCallback;
        const disconnect = vi.fn();
        vi.stubGlobal('ResizeObserver', class { constructor(cb: ResizeObserverCallback) { resized = cb; } observe() {} disconnect = disconnect; });
        vi.stubGlobal('ImageData', class { constructor(public data: Uint8ClampedArray, public width: number, public height: number) {} });
        const putImageData = vi.fn();
        vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({ putImageData } as any);
        const wrapper = mount(DevelopPreviewSurface, { props: { frame: frame(1536, 1024) } });
        resized([{ contentRect: { width: 1800, height: 900 } } as ResizeObserverEntry], {} as ResizeObserver);
        await flushPromises();
        const box = () => wrapper.get('[data-testid="develop-preview-image-box"]').attributes('style');
        const canvas = wrapper.get('canvas').element;
        const fullBox = box();
        expect(fullBox).toContain('width: 1350px');
        expect(fullBox).toContain('height: 900px');
        await wrapper.setProps({ frame: frame(768, 512, 2) });
        await flushPromises();
        expect(box()).toBe(fullBox);
        expect(wrapper.get('canvas').element).toBe(canvas);
        expect(canvas.width).toBe(768);
        await wrapper.setProps({ frame: frame(1536, 1024, 3) });
        await flushPromises();
        expect(box()).toBe(fullBox);
        expect(canvas.width).toBe(1536);
        // Resizing must fit the available width, and portrait geometry must also fit.
        resized([{ contentRect: { width: 600, height: 900 } } as ResizeObserverEntry], {} as ResizeObserver);
        await flushPromises();
        expect(box()).toContain('width: 600px');
        expect(box()).toContain('height: 400px');
        await wrapper.setProps({ frame: frame(1024, 1536, 4) });
        await flushPromises();
        expect(box()).toContain('height: 900px');
        expect(putImageData).toHaveBeenCalledTimes(4);
        wrapper.unmount();
        expect(disconnect).toHaveBeenCalled();
    });
});
