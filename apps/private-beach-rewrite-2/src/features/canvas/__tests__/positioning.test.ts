import { describe, it, expect } from 'vitest';
import { clampPointToBounds, snapPointToGrid } from '../positioning';

describe('canvas positioning helpers', () => {
  it('clamps dragged tiles that move beyond the visible canvas', () => {
    const next = clampPointToBounds(
      { x: 950, y: 640 },
      { width: 400, height: 300 },
      { width: 1200, height: 800 },
    );

    expect(next).toEqual({ x: 800, y: 500 });
  });

  it('prevents negative coordinates when the pointer crosses the top/left edges', () => {
    const next = clampPointToBounds(
      { x: -120, y: -40 },
      { width: 300, height: 200 },
      { width: 900, height: 600 },
    );

    expect(next).toEqual({ x: 0, y: 0 });
  });

  it('pins oversized tiles to the origin if the viewport is smaller than the tile', () => {
    const next = clampPointToBounds(
      { x: 120, y: 80 },
      { width: 1600, height: 1200 },
      { width: 1400, height: 900 },
    );

    expect(next).toEqual({ x: 0, y: 0 });
  });

  it('snaps raw coordinates to the configured grid', () => {
    expect(snapPointToGrid({ x: 47, y: 93 }, 20)).toEqual({ x: 40, y: 100 });
  });

  it('skips snapping when the grid size is invalid', () => {
    expect(snapPointToGrid({ x: 47, y: 93 }, 0)).toEqual({ x: 47, y: 93 });
    expect(snapPointToGrid({ x: 47, y: 93 }, Number.NaN)).toEqual({ x: 47, y: 93 });
  });
});
