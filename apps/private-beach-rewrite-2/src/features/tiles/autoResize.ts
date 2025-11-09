'use client';

import { snapSize } from './utils';
import type { TileSize, TileViewportSnapshot } from './types';

type AutoResizeInput = {
  metrics: TileViewportSnapshot;
  chromeWidthPx: number;
  chromeHeightPx: number;
};

function normalizePositive(value: number | null | undefined): number | null {
  if (typeof value !== 'number') {
    return null;
  }
  if (!Number.isFinite(value) || value <= 0) {
    return null;
  }
  return value;
}

export function computeAutoResizeSize(input: AutoResizeInput): TileSize | null {
  const rows = normalizePositive(input.metrics.hostRows);
  const cols = normalizePositive(input.metrics.hostCols);
  // Prefer the terminal-reported fixed metrics to avoid zoom/transform skew.
  const pixelsPerRow = normalizePositive(input.metrics.pixelsPerRow);
  const pixelsPerCol = normalizePositive(input.metrics.pixelsPerCol);
  if (!rows || !cols || !pixelsPerRow || !pixelsPerCol) {
    return null;
  }

  const terminalWidthPx = cols * pixelsPerCol;
  const terminalHeightPx = rows * pixelsPerRow;
  const chromeWidthPx = Number.isFinite(input.chromeWidthPx) ? Math.max(0, input.chromeWidthPx) : 0;
  const chromeHeightPx = Number.isFinite(input.chromeHeightPx) ? Math.max(0, input.chromeHeightPx) : 0;
  const desiredTileWidthPx = terminalWidthPx + chromeWidthPx;
  const desiredTileHeightPx = terminalHeightPx + chromeHeightPx;
  if (!Number.isFinite(desiredTileWidthPx) || !Number.isFinite(desiredTileHeightPx)) {
    return null;
  }

  // React Flow node sizes are specified in unscaled (pre-zoom) coordinates.
  const flowWidth = desiredTileWidthPx;
  const flowHeight = desiredTileHeightPx;
  if (!Number.isFinite(flowWidth) || !Number.isFinite(flowHeight)) {
    return null;
  }

  return snapSize({ width: flowWidth, height: flowHeight });
}
