import { describe, it, expect } from 'vitest';
import { TerminalGridStore } from '../gridStore';
import type { CellState } from '../cache';
import { buildLines } from '../../components/BeachTerminal';

function makeCells(text: string): CellState[] {
  return text.split('').map((char, index) => ({
    char,
    styleId: 0,
    seq: index + 1,
  }));
}

describe('TerminalGridCache tail fallback', () => {
  it('materialises loaded rows when gridHeight === 0 in follow-tail mode', () => {
    const store = new TerminalGridStore();
    const cache = (store as unknown as { cache: any }).cache;

    store.setBaseRow(90);
    store.setGridSize(10, 80);
    const updates = [
      {
        type: 'row' as const,
        row: 96,
        seq: 1,
        cells: makeCells('Line 96'),
      },
      {
        type: 'row' as const,
        row: 97,
        seq: 2,
        cells: makeCells('Line 97'),
      },
      {
        type: 'row' as const,
        row: 98,
        seq: 3,
        cells: makeCells('Line 98'),
      },
      {
        type: 'row' as const,
        row: 99,
        seq: 4,
        cells: makeCells('Line 99'),
      },
    ];
    store.applyUpdates(updates, { authoritative: true });
    store.setViewport(95, 4);
    store.setFollowTail(true);
    cache.gridHeight = 0;

    const snapshot = store.getSnapshot();
    const lines = buildLines(snapshot, 4);

    expect(lines.map((line) => line.kind)).toEqual(['loaded', 'loaded', 'loaded', 'loaded']);
    expect(lines[lines.length - 1]?.absolute).toBe(99);
  });

  it('reuses previous tail snapshot while awaiting new rows', () => {
    const store = new TerminalGridStore();
    const cache = (store as unknown as { cache: any }).cache;

    store.setBaseRow(90);
    store.setGridSize(10, 80);
    const updates = [
      {
        type: 'row' as const,
        row: 96,
        seq: 1,
        cells: makeCells('Line 96'),
      },
      {
        type: 'row' as const,
        row: 97,
        seq: 2,
        cells: makeCells('Line 97'),
      },
      {
        type: 'row' as const,
        row: 98,
        seq: 3,
        cells: makeCells('Line 98'),
      },
      {
        type: 'row' as const,
        row: 99,
        seq: 4,
        cells: makeCells('Line 99'),
      },
    ];
    store.applyUpdates(updates, { authoritative: true });
    store.setViewport(95, 4);
    store.setFollowTail(true);

    buildLines(store.getSnapshot(), 4);

    cache.rows = [];
    cache.gridHeight = 0;
    cache.viewportHeight = 4;
    cache.viewportTop = 96;
    cache.followTail = true;

    const fallbackSnapshot = store.getSnapshot();
    const fallbackLines = buildLines(fallbackSnapshot, 4);

    expect(fallbackLines.map((line) => line.kind)).toEqual(['loaded', 'loaded', 'loaded', 'loaded']);
    expect(fallbackLines[fallbackLines.length - 1]?.absolute).toBe(99);
  });
});
