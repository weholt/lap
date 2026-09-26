import { describe, expect, it } from 'vitest';

import {
  clampCenterInRotatedRect,
  getMaxInscribedRect,
} from '@/common/geometry';

// These tests pin the image-editor geometry contracts that the RAW development
// extraction (docs/raw-development/spec.md) must keep or consciously migrate:
// crop rectangles are expressed in the rotated image's axis-aligned bounding
// box space and stay centered inside it.

describe('getMaxInscribedRect', () => {
  it('returns the full image at angle 0 with free aspect', () => {
    const rect = getMaxInscribedRect(6000, 4000, 0);
    expect(rect.x).toBe(0);
    expect(rect.y).toBe(0);
    expect(rect.width).toBe(6000);
    expect(rect.height).toBe(4000);
  });

  it('fits an aspect-ratio-constrained rect inside the image at angle 0', () => {
    const wide = getMaxInscribedRect(6000, 4000, 0, 2);
    expect(wide.width).toBe(6000);
    expect(wide.height).toBe(3000);
    expect(wide.x).toBe(0);
    expect(wide.y).toBe(500);

    const taller = getMaxInscribedRect(4000, 6000, 0, 2);
    expect(taller.width).toBe(4000);
    expect(taller.height).toBe(2000);
    expect(taller.x).toBe(0);
    expect(taller.y).toBe(2000);
  });

  it('returns a centered rect in the rotated bounding box for a square at 45 degrees', () => {
    const rect = getMaxInscribedRect(1000, 1000, 45, 1);
    // Bounding box of a 1000x1000 square rotated by 45 degrees.
    const bb = 1000 * (Math.cos(Math.PI / 4) + Math.sin(Math.PI / 4));
    // Half-extent along x is bounded by W / (2 * (cos + sin / R)) with R = 1.
    const expected = 1000 / (Math.cos(Math.PI / 4) + Math.sin(Math.PI / 4));
    // The 1px safety inset scales both sides equally: w = h = expected - 2.
    expect(rect.width).toBeCloseTo(expected - 2, 6);
    expect(rect.height).toBeCloseTo(expected - 2, 6);
    expect(rect.x).toBeCloseTo((bb - (expected - 2)) / 2, 6);
    expect(rect.y).toBeCloseTo((bb - (expected - 2)) / 2, 6);
  });

  it('keeps aspect ratio while shrinking for the safety inset', () => {
    const rect = getMaxInscribedRect(1000, 1000, 30, 1.5);
    expect(rect.width / rect.height).toBeCloseTo(1.5, 6);
    expect(rect.x).toBeGreaterThan(0);
    expect(rect.y).toBeGreaterThan(0);
  });
});

describe('clampCenterInRotatedRect', () => {
  it('degenerates to the axis-aligned clamp at theta 0', () => {
    const clamped = clampCenterInRotatedRect(200, -100, 10, 5, 1, 0, 100, 50);
    expect(clamped.x).toBeCloseTo(90, 9);
    expect(clamped.y).toBeCloseTo(-45, 9);
  });

  it('leaves offsets that already fit untouched', () => {
    const clamped = clampCenterInRotatedRect(10, 10, 10, 5, 1, 0, 100, 50);
    expect(clamped.x).toBeCloseTo(10, 9);
    expect(clamped.y).toBeCloseTo(10, 9);
  });
});
