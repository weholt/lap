import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { baseParse } from '@vue/compiler-dom';
import { parse } from '@vue/compiler-sfc';
import { developPreviewPresentation } from '../src/composables/developPreviewPresentation';

function tree(file: string) {
  return baseParse(parse(readFileSync(new URL(file, import.meta.url), 'utf8')).descriptor.template!.content);
}
function findPath(node: any, predicate: (n: any) => boolean, parents: any[] = []): any[] | null {
  if (predicate(node)) return [...parents, node];
  for (const child of node.children ?? []) {
    const found = findPath(child, predicate, [...parents, node]);
    if (found) return found;
  }
  return null;
}
function attr(node: any, name: string, value: string) {
  return node.props?.some((p: any) => p.name === name && p.value?.content === value);
}

describe('Develop preview viewport ownership', () => {
  it('places the editing preview inside MediaViewer media-overlay slot, not across its toolbar', () => {
    const path = findPath(tree('../src/components/Content.vue'), n => attr(n, 'data-testid', 'develop-central-preview'))!;
    expect(path.some(n => n.tag === 'MediaViewer')).toBe(true);
    expect(path.some(n => n.tag === 'template' && n.props?.some((p: any) => p.name === 'slot' && p.arg?.content === 'media-overlay'))).toBe(true);
  });
  it('anchors custom overlays in the media area, which excludes the pinned toolbar', () => {
    const path = findPath(tree('../src/components/MediaViewer.vue'), n => n.tag === 'slot' && attr(n, 'name', 'media-overlay'));
    expect(path).not.toBeNull();
    expect(path!.some(n => attr(n, 'ref', 'mediaAreaRef'))).toBe(true);
  });
  it('keeps the source covered while a newly selected asset is opening', () => {
    expect(developPreviewPresentation(true, false, 12, 11)).toEqual({
      showOverlay: true,
      useCurrentFrame: false,
    });
    expect(developPreviewPresentation(true, false, 12, 12)).toEqual({
      showOverlay: true,
      useCurrentFrame: true,
    });
    expect(developPreviewPresentation(true, true, 12, 12).showOverlay).toBe(false);
    expect(developPreviewPresentation(false, false, 12, 11).showOverlay).toBe(false);
  });
});
