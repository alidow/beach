import { describe, expect, it } from 'vitest';
import type { SharedCanvasLayout } from '../../canvas';
import type { BeachLayoutItem } from '../../lib/api';
import { extractGridDashboardMetadata, withLayoutDashboardMetadata, applyGridMetadataToLayout, beachItemsToGridSnapshot } from '../gridLayout';
import { applyGridAutosizeCommand, applyGridDragCommand, applyGridPresetCommand } from '../gridLayoutCommands';

function buildLayout(tileIds: string[]): SharedCanvasLayout {
  const tiles: SharedCanvasLayout['tiles'] = {};
  tileIds.forEach((id) => {
    tiles[id] = {
      id,
      kind: 'application',
      position: { x: 0, y: 0 },
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

function seedLayout(layout: SharedCanvasLayout, items: BeachLayoutItem[]): SharedCanvasLayout {
  const snapshot = beachItemsToGridSnapshot(items);
  const withDashboard = withLayoutDashboardMetadata(layout, snapshot);
  return applyGridMetadataToLayout(withDashboard, snapshot.tiles);
}

describe('grid layout commands', () => {
  it('updates tile positions on drag command', () => {
    const layout = seedLayout(
      buildLayout(['tile-1', 'tile-2']),
      [
        { id: 'tile-1', x: 0, y: 0, w: 16, h: 12 },
        { id: 'tile-2', x: 20, y: 0, w: 16, h: 12 },
      ],
    );
    const result = applyGridDragCommand(
      layout,
      [
        { i: 'tile-1', x: 12, y: 4, w: 16, h: 12 },
        { i: 'tile-2', x: 36, y: 0, w: 16, h: 12 },
      ],
      { cols: 64, rowHeightPx: 18 },
    );
    expect(result.mutated).toBe(true);
    const meta = extractGridDashboardMetadata(result.layout.tiles['tile-1']);
    expect(meta.layout).toEqual({ x: 12, y: 4, w: 16, h: 12 });
    expect(meta.manualLayout).toBe(true);
    const other = extractGridDashboardMetadata(result.layout.tiles['tile-2']);
    expect(other.layout.x).toBe(36);
  });

  it('applies preset layout and clears manualLayout flag', () => {
    const layout = seedLayout(
      buildLayout(['tile-1']),
      [{ id: 'tile-1', x: 0, y: 0, w: 16, h: 12, locked: true }],
    );
    const preset: BeachLayoutItem[] = [{ id: 'tile-1', x: 8, y: 6, w: 24, h: 18 }];
    const result = applyGridPresetCommand(layout, preset, { defaultCols: 64, defaultRowHeightPx: 20 });
    expect(result.mutated).toBe(true);
    const meta = extractGridDashboardMetadata(result.layout.tiles['tile-1']);
    expect(meta.layout).toEqual({ x: 8, y: 6, w: 24, h: 18 });
    expect(meta.manualLayout).toBe(false);
  });

  it('marks autosized tiles as non-manual layout', () => {
    const layout = seedLayout(
      buildLayout(['tile-1']),
      [{ id: 'tile-1', x: 0, y: 0, w: 16, h: 12, locked: false }],
    );
    const result = applyGridAutosizeCommand(
      layout,
      [{ i: 'tile-1', x: 0, y: 0, w: 32, h: 24 }],
      { cols: 64, rowHeightPx: 18 },
    );
    expect(result.mutated).toBe(true);
    const meta = extractGridDashboardMetadata(result.layout.tiles['tile-1']);
    expect(meta.layout).toEqual({ x: 0, y: 0, w: 32, h: 24 });
    expect(meta.manualLayout).toBe(false);
  });
});
