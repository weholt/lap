import { describe, expect, it } from "vitest";
import { createDevelopThumbnailGuard } from "../src/composables/developThumbnailGuard";

describe("Develop thumbnail refresh ordering", () => {
  it("discards an older thumbnail response after a newer commit becomes ready", () => {
    const guard = createDevelopThumbnailGuard();
    const first = guard.markReady(1666);
    const second = guard.markReady(1666);
    expect(guard.isCurrent(1666, first)).toBe(false);
    expect(guard.isCurrent(1666, second)).toBe(true);
    expect(guard.isCurrent(1665, guard.markReady(1665))).toBe(true);
  });
});
