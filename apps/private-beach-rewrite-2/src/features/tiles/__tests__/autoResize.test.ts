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
};

describe('computeAutoResizeSize', () => {
  it('calculates snapped flow dimensions accounting for chrome and zoom', () => {
    const result = computeAutoResizeSize({
      metrics: baseMetrics,
      chromeWidthPx: 120,
      chromeHeightPx: 80,
      zoom: 0.5,
    });
    expect(result).toEqual({ width: 2400, height: 1600 });
  });

  it('returns null when host metrics are incomplete', () => {
    const missingMetrics: TileViewportSnapshot = { ...baseMetrics, hostRows: null };
    const result = computeAutoResizeSize({
      metrics: missingMetrics,
      chromeWidthPx: 0,
      chromeHeightPx: 0,
      zoom: 1,
    });
    expect(result).toBeNull();
  });

  it('falls back to zoom=1 when provided zoom is invalid', () => {
    const result = computeAutoResizeSize({
      metrics: baseMetrics,
      chromeWidthPx: -50,
      chromeHeightPx: -10,
      zoom: 0,
    });
    expect(result).toEqual({ width: 1080, height: 720 });
  });
});
