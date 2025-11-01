import { describe, expect, it } from 'vitest';
import type { SharedCanvasLayout } from '../../canvas';
import type { BeachLayoutItem } from '../../lib/api';
import {
  DEFAULT_GRID_COLS,
  DEFAULT_GRID_H_UNITS,
  DEFAULT_GRID_W_UNITS,
  DEFAULT_ROW_HEIGHT_PX,
  GRID_LAYOUT_VERSION,
  applyGridMetadataToLayout,
  beachItemsToGridSnapshot,
  extractGridDashboardMetadata,
  extractGridLayoutSnapshot,
  gridSnapshotToBeachItems,
  gridSnapshotToReactGrid,
  reactGridToGridSnapshot,
  withLayoutDashboardMetadata,
} from '../gridLayout';

function buildLayout(tileIds: string[]): SharedCanvasLayout {
  const tiles: SharedCanvasLayout['tiles'] = {};
  tileIds.forEach((id, index) => {
    tiles[id] = {
      id,
      kind: 'application',
      position: { x: index * 120, y: 0 },
      size: { width: 400, height: 280 },
      zIndex: 1,
      metadata: {},
    };
  });
  const now = Date.now();
  return {
    version: 3,
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    tiles,
    groups: {},
    agents: {},
    controlAssignments: {},
    metadata: { createdAt: now, updatedAt: now },
  };
}

function applySnapshot(layout: SharedCanvasLayout, items: BeachLayoutItem[]): SharedCanvasLayout {
  const snapshot = beachItemsToGridSnapshot(items);
  const withDashboard = withLayoutDashboardMetadata(layout, snapshot);
  return applyGridMetadataToLayout(withDashboard, snapshot.tiles);
}

describe('grid layout conversions', () => {
  it('applies beach layout items to canvas tiles and round-trips', () => {
    const layout = buildLayout(['a', 'b']);
    const items: BeachLayoutItem[] = [
      {
        id: 'a',
        x: 4,
        y: 2,
        w: 16,
        h: 12,
        widthPx: 512,
        heightPx: 384,
        zoom: 0.8,
        locked: true,
        toolbarPinned: false,
        gridCols: 96,
        rowHeightPx: 20,
        layoutVersion: 5,
      },
      {
        id: 'b',
        x: 24,
        y: 8,
        w: 32,
        h: 18,
        widthPx: 640,
        heightPx: 420,
        zoom: 1,
        locked: false,
        toolbarPinned: true,
        gridCols: 96,
        rowHeightPx: 20,
        layoutVersion: 5,
      },
    ];

    const next = applySnapshot(layout, items);
    expect(next).not.toBe(layout);
    const snapshot = extractGridLayoutSnapshot(next);
    expect(snapshot.gridCols).toBe(DEFAULT_GRID_COLS);
    const metaA = extractGridDashboardMetadata(next.tiles['a']);
    expect(metaA.layout).toEqual({ x: 4, y: 2, w: 16, h: 12 });
    expect(metaA.widthPx).toBe(512);
    expect(metaA.heightPx).toBe(384);
    expect(metaA.zoom).toBeCloseTo(0.8);
    expect(metaA.locked).toBe(true);
    expect(metaA.toolbarPinned).toBe(false);
    const back = gridSnapshotToBeachItems(next);
    expect(back).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: 'a', x: 4, y: 2, w: 16, h: 12, widthPx: 512, heightPx: 384 }),
        expect.objectContaining({ id: 'b', x: 24, y: 8, w: 32, h: 18, widthPx: 640, heightPx: 420 }),
      ]),
    );
  });

  it('converts between react-grid-layout arrays and snapshot metadata', () => {
    const layout = buildLayout(['t1']);
    const initialSnapshot = extractGridLayoutSnapshot(layout);
    const nextSnapshot = reactGridToGridSnapshot(
      [
        {
          i: 't1',
          x: 12,
          y: 4,
          w: 20,
          h: 14,
          minW: 4,
          minH: 4,
        },
      ],
      { cols: 64, rowHeightPx: 18, layoutVersion: GRID_LAYOUT_VERSION, previous: initialSnapshot },
    );
    expect(nextSnapshot.gridCols).toBe(64);
    const applied = applyGridMetadataToLayout(withLayoutDashboardMetadata(layout, nextSnapshot), nextSnapshot.tiles);
    const metadata = extractGridDashboardMetadata(applied.tiles['t1']);
    expect(metadata.layout).toEqual({ x: 12, y: 4, w: 20, h: 14 });
    expect(metadata.rowHeightPx).toBe(18);
    const rgl = gridSnapshotToReactGrid(applied, { fallbackCols: 64 });
    expect(rgl).toEqual([
      expect.objectContaining({ i: 't1', x: 12, y: 4, w: 20, h: 14 }),
    ]);
  });

  it('provides sensible defaults when metadata is missing', () => {
    const layout = buildLayout(['x']);
    const metadata = extractGridDashboardMetadata(layout.tiles['x']);
    expect(metadata.layout.w).toBe(DEFAULT_GRID_W_UNITS);
    expect(metadata.layout.h).toBe(DEFAULT_GRID_H_UNITS);
    expect(metadata.gridCols).toBe(DEFAULT_GRID_COLS);
    expect(metadata.rowHeightPx).toBe(DEFAULT_ROW_HEIGHT_PX);
  });
});
