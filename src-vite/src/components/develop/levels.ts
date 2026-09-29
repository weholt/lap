import type { LevelsChannel } from '@/composables/useDevelopSession.types';

export type LevelsKey = keyof LevelsChannel;
export type LevelsColor = 'rgb' | 'red' | 'green' | 'blue';
export interface LevelsPixels { data: ArrayLike<number>; width: number; height: number; }
export const LEVELS_KEYS: LevelsKey[] = ['inputBlack', 'midtone', 'inputWhite', 'outputBlack', 'outputWhite'];
const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));
export function levelsBounds(value: LevelsChannel, key: LevelsKey): [number, number] {
    switch (key) {
        case 'inputBlack': return [0, value.inputWhite - 1];
        case 'inputWhite': return [value.inputBlack + 1, 255];
        case 'outputBlack': return [0, value.outputWhite - 1];
        case 'outputWhite': return [value.outputBlack + 1, 255];
        case 'midtone': return [-1, 1];
    }
}
export function changeLevels(value: LevelsChannel, key: LevelsKey, next: number): LevelsChannel {
    if (!Number.isFinite(next)) return value;
    const [min, max] = levelsBounds(value, key);
    const rounded = key === 'midtone' ? Math.round(next * 100) / 100 : Math.round(next);
    return { ...value, [key]: clamp(rounded, min, max) };
}
export function levelsPosition(value: LevelsChannel, key: LevelsKey): number {
    return key === 'midtone' ? value.inputBlack + (value.inputWhite - value.inputBlack) * (0.5 - 0.45 * value.midtone) : value[key];
}
export function levelsFromPosition(value: LevelsChannel, key: LevelsKey, position: number): LevelsChannel {
    return changeLevels(value, key, key === 'midtone' ? (0.5 - (position - value.inputBlack) / (value.inputWhite - value.inputBlack)) / 0.45 : position);
}
/** Current rendered output, sampled once per frame, never per handle movement. */
export function levelsHistogram(pixels: LevelsPixels | null | undefined) {
    const bins = { red: new Uint32Array(256), green: new Uint32Array(256), blue: new Uint32Array(256) };
    if (!pixels || pixels.width <= 0 || pixels.height <= 0 || pixels.data.length < pixels.width * pixels.height * 4) return bins;
    const count = pixels.width * pixels.height;
    const stride = Math.max(1, Math.ceil(count / 65536));
    for (let i = 0; i < count; i += stride) {
        const offset = i * 4;
        if (pixels.data[offset + 3] === 0) continue;
        bins.red[pixels.data[offset]]++;
        bins.green[pixels.data[offset + 1]]++;
        bins.blue[pixels.data[offset + 2]]++;
    }
    return bins;
}
