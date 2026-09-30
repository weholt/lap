/** Track the latest completed derivative for each asset independently. */
export function createDevelopThumbnailGuard() {
  const epochs = new Map<number, number>();
  return {
    markReady(assetId: number): number {
      const epoch = (epochs.get(assetId) ?? 0) + 1;
      epochs.set(assetId, epoch);
      return epoch;
    },
    isCurrent(assetId: number, epoch: number): boolean {
      return epochs.get(assetId) === epoch;
    },
  };
}
