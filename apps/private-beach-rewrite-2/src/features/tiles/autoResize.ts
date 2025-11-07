'use client';

import { snapSize } from './utils';
import type { TileSize, TileViewportSnapshot } from './types';

type AutoResizeInput = {
  metrics: TileViewportSnapshot;
  chromeWidthPx: number;
  chromeHeightPx: number;
  zoom: number;
  observedRowHeight?: number | null;
  observedCellWidth?: number | null;
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

function selectMetric(
  preferred: number | null,
  fallback: number | null,
  kind: 'row' | 'col',
): number | null {
  const lowerBound = kind === 'row' ? 6 : 4;
  const upperBound = kind === 'row' ? 40 : 32;
  const choose = (value: number | null) =>
    value != null && value >= lowerBound && value <= upperBound ? value : null;
  return choose(preferred) ?? choose(fallback) ?? preferred ?? fallback ?? null;
}

export function computeAutoResizeSize(input: AutoResizeInput): TileSize | null {
  const rows = normalizePositive(input.metrics.hostRows);
  const cols = normalizePositive(input.metrics.hostCols);
  const metricRow = normalizePositive(input.metrics.pixelsPerRow);
  const metricCol = normalizePositive(input.metrics.pixelsPerCol);
  const observedRow = normalizePositive(input.observedRowHeight);
  const observedCol = normalizePositive(input.observedCellWidth);
  const pixelsPerRow = selectMetric(metricRow, observedRow, 'row');
  const pixelsPerCol = selectMetric(metricCol, observedCol, 'col');
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

  const flowWidth = desiredTileWidthPx;
  const flowHeight = desiredTileHeightPx;
  if (!Number.isFinite(flowWidth) || !Number.isFinite(flowHeight)) {
    return null;
  }

  return snapSize({ width: flowWidth, height: flowHeight });
}
