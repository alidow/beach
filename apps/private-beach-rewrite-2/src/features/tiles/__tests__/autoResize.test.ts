import { describe, it, expect } from 'vitest';
import { computeAutoResizeSize } from '../autoResize';
import type { TileViewportSnapshot } from '../types';

const baseMetrics: TileViewportSnapshot = {
  tileId: 'tile-1',
  hostRows: 40,
  hostCols: 120,
  viewportRows: 38,
  viewportCols: 100,
  pixelsPerRow: 18,
  pixelsPerCol: 9,
  hostWidthPx: null,
  hostHeightPx: null,
  cellWidthPx: 9,
  cellHeightPx: 18,
};

describe('computeAutoResizeSize', () => {
  it('calculates snapped flow dimensions accounting for chrome', () => {
    const result = computeAutoResizeSize({
      metrics: baseMetrics,
      chromeWidthPx: 120,
      chromeHeightPx: 80,
    });
    expect(result).toEqual({ width: 1200, height: 800 });
  });

  it('returns null when host metrics are incomplete', () => {
    const missingMetrics: TileViewportSnapshot = { ...baseMetrics, hostRows: null };
    const result = computeAutoResizeSize({
      metrics: missingMetrics,
      chromeWidthPx: 0,
      chromeHeightPx: 0,
    });
    expect(result).toBeNull();
  });

  it('clamps negative chrome deltas to zero', () => {
    const result = computeAutoResizeSize({
      metrics: baseMetrics,
      chromeWidthPx: -50,
      chromeHeightPx: -10,
    });
    expect(result).toEqual({ width: 1080, height: 720 });
  });

  it('prefers host pixel sizes when available', () => {
    const metrics: TileViewportSnapshot = {
      ...baseMetrics,
      hostWidthPx: 640,
      hostHeightPx: 320,
    };
    const result = computeAutoResizeSize({
      metrics,
      chromeWidthPx: 10,
      chromeHeightPx: 20,
    });
    expect(result).toEqual({ width: 648, height: 344 });
  });
});
