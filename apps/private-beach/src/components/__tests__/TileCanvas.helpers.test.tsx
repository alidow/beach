import { describe, it, expect } from 'vitest';
import { clampZoom, computeZoomForSize, estimateHostSize, getColumnWidth } from '../TileCanvas';

describe('TileCanvas helpers', () => {
  it('clampZoom respects min/max and measurement-derived floor', () => {
    expect(clampZoom(0)).toBeGreaterThan(0); // min enforced
    expect(clampZoom(10)).toBeLessThanOrEqual(1); // max enforced
    const zoom = clampZoom(0.01, { width: 100, height: 100 });
    expect(zoom).toBeGreaterThan(0.01);
  });

  it('estimateHostSize returns padded pixel size from rows/cols', () => {
    const { width, height } = estimateHostSize(80, 24);
    expect(width).toBeGreaterThan(80);
    expect(height).toBeGreaterThan(24);
  });

  it('computeZoomForSize fits measurement to host + viewport', () => {
    const z1 = computeZoomForSize({ width: 800, height: 600 }, 80, 24, null, null);
    const z2 = computeZoomForSize({ width: 400, height: 300 }, 80, 24, null, null);
    expect(z1).toBeGreaterThan(z2);
  });

  it('getColumnWidth returns null on invalid inputs and finite value otherwise', () => {
    expect(getColumnWidth(null, 0)).toBeNull();
    expect(getColumnWidth(1200, 12)).toBeGreaterThan(0);
  });
});

