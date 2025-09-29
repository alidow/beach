import { describe, expect, it } from 'vitest';
import type { Update } from '../protocol/types';
import { TerminalGridStore } from './gridStore';

const PACKED_A = packCell('A', 1);
const PACKED_B = packCell('B', 1);

describe('TerminalGridStore', () => {
  it('applies row updates and exposes row text', () => {
    const store = new TerminalGridStore();
    const updates: Update[] = [{ type: 'row', row: 10, seq: 1, cells: [PACKED_A, PACKED_B] }];
    store.applyUpdates(updates);

    const snapshot = store.getSnapshot();
    expect(snapshot.rows).toHaveLength(1);
    const text = store.getRowText(10);
    expect(text).toBe('AB');
  });

  it('prefills pending rows when grid size is set', () => {
    const store = new TerminalGridStore();
    store.setGridSize(3, 80);

    const snapshot = store.getSnapshot();
    expect(snapshot.rows).toHaveLength(3);
    expect(snapshot.rows.every((row) => row.kind === 'pending')).toBe(true);
  });

  it('applies individual cell updates respecting sequence numbers', () => {
    const store = new TerminalGridStore();
    store.applyUpdates([{ type: 'row', row: 5, seq: 1, cells: [PACKED_A] }]);

    store.applyUpdates([{ type: 'cell', row: 5, col: 0, seq: 0, cell: PACKED_B }]);
    expect(store.getRowText(5)).toBe('A');

    store.applyUpdates([{ type: 'cell', row: 5, col: 0, seq: 2, cell: PACKED_B }]);
    expect(store.getRowText(5)).toBe('B');
  });

  it('removes rows when trim updates arrive', () => {
    const store = new TerminalGridStore();
    store.applyUpdates([
      { type: 'row', row: 0, seq: 1, cells: [PACKED_A] },
      { type: 'row', row: 1, seq: 1, cells: [PACKED_B] },
    ]);
    expect(store.getSnapshot().rows).toHaveLength(2);

    store.applyUpdates([{ type: 'trim', start: 0, count: 1, seq: 2 }]);
    expect(store.getRow(0)).toBeUndefined();
    expect(store.getRowText(1)).toBe('B');
  });

  it('stores style definitions', () => {
    const store = new TerminalGridStore();
    store.applyUpdates([{ type: 'style', id: 2, seq: 1, fg: 0x112233, bg: 0x445566, attrs: 0b0000_0100 }]);
    const snapshot = store.getSnapshot();
    expect(snapshot.styles.get(2)).toEqual({ id: 2, fg: 0x112233, bg: 0x445566, attrs: 0b0000_0100 });
  });

  it('clears trailing characters when a row segment rewrites from column zero', () => {
    const store = new TerminalGridStore();
    store.setGridSize(24, 80);

    const prompt = '(base) % ';
    const command = 'echo hi';

    store.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 1, cells: packString(`${prompt}${command}`) },
    ], true);

    store.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 2, cells: packString(prompt) },
    ], true);

    expect(store.getRowText(0)).toBe('(base) %');
  });

  it('drops previous session state on reset', () => {
    const store = new TerminalGridStore();
    store.setGridSize(24, 80);
    store.applyUpdates([{ type: 'row', row: 0, seq: 1, cells: packString('old session') }], true);
    expect(store.getRowText(0)).toBe('old session');

    store.reset();
    store.setGridSize(24, 80);
    store.applyUpdates([{ type: 'row', row: 0, seq: 2, cells: packString('new session') }], true);

    const loadedRows = store.getSnapshot().rows.filter((row) => row.kind === 'loaded');
    expect(loadedRows).toHaveLength(1);
    expect(store.getRowText(0)).toBe('new session');
  });

  it('lowers the base row when authoritative updates reference earlier history', () => {
    const store = new TerminalGridStore();
    store.setBaseRow(10);
    store.setGridSize(5, 80);

    store.applyUpdates([{ type: 'row', row: 8, seq: 1, cells: packString('history') }], true);

    expect(store.getSnapshot().baseRow).toBe(8);
  });

  it('returns visible rows following the tail by default', () => {
    const store = new TerminalGridStore();
    store.setGridSize(5, 80);
    store.applyUpdates(
      [
        { type: 'row', row: 0, seq: 1, cells: packString('zero') },
        { type: 'row', row: 1, seq: 1, cells: packString('one') },
        { type: 'row', row: 2, seq: 1, cells: packString('two') },
        { type: 'row', row: 3, seq: 1, cells: packString('three') },
        { type: 'row', row: 4, seq: 1, cells: packString('four') },
      ],
      true,
    );
    store.setViewport(0, 3);

    const snapshot = store.getSnapshot();
    const visible = snapshot.visibleRows(10);
    expect(visible.map((row) => row.absolute)).toEqual([2, 3, 4]);
  });

  it('respects manual viewport when follow tail is disabled', () => {
    const store = new TerminalGridStore();
    store.setGridSize(5, 80);
    store.applyUpdates(
      [
        { type: 'row', row: 0, seq: 1, cells: packString('zero') },
        { type: 'row', row: 1, seq: 1, cells: packString('one') },
        { type: 'row', row: 2, seq: 1, cells: packString('two') },
      ],
      true,
    );
    store.setViewport(0, 2);
    store.setFollowTail(false);
    store.setViewport(0, 2);
    store.setViewport(1, 2);

    const snapshot = store.getSnapshot();
    const visible = snapshot.visibleRows(10);
    expect(visible.map((row) => row.absolute)).toEqual([1, 2]);
  });

  it('reports the first gap within a range', () => {
    const store = new TerminalGridStore();
    store.setGridSize(5, 80);
    store.applyUpdates([{ type: 'row', row: 0, seq: 1, cells: packString('row0') }], true);
    store.markRowPending(1);
    store.applyUpdates([{ type: 'row', row: 2, seq: 1, cells: packString('row2') }], true);

    expect(store.firstGapBetween(0, 3)).toBe(1);
    expect(store.firstGapBetween(0, 1)).toBeNull();
  });

  it('falls back to raw char codes when packed high bits are missing', () => {
    const store = new TerminalGridStore();
    store.setBaseRow(0);
    store.setGridSize(1, 80);
    store.applyUpdates([{ type: 'cell', row: 0, col: 0, seq: 1, cell: '('.codePointAt(0)! }], true);

    expect(store.getRowText(0)).toBe('(');
  });

  it('records predictive characters and clears them on ack', () => {
    const store = new TerminalGridStore();
    store.setGridSize(1, 80);
    store.applyUpdates([{ type: 'row', row: 0, seq: 1, cells: packString('> ') }], true);

    store.registerPrediction(1, stringToBytes('ls'));
    let snapshot = store.getSnapshot();
    expect(snapshot.getPrediction(0, 2)?.char).toBe('l');
    expect(snapshot.getPrediction(0, 3)?.char).toBe('s');

    store.clearPrediction(1);
    snapshot = store.getSnapshot();
    expect(snapshot.getPrediction(0, 2)).toBeNull();
    expect(snapshot.getPrediction(0, 3)).toBeNull();
  });

  it('drops stale predictions when authoritative updates overwrite cells', () => {
    const store = new TerminalGridStore();
    store.setGridSize(1, 80);
    store.applyUpdates([{ type: 'row', row: 0, seq: 1, cells: packString('> ') }], true);

    store.registerPrediction(1, stringToBytes('hi'));
    expect(store.getSnapshot().getPrediction(0, 2)?.char).toBe('h');

    store.applyUpdates([{ type: 'row_segment', row: 0, startCol: 2, seq: 2, cells: packString('hi') }], true);

    const snapshot = store.getSnapshot();
    expect(snapshot.getPrediction(0, 2)).toBeNull();
    expect(snapshot.getPrediction(0, 3)).toBeNull();
    expect(store.getRowText(0)).toBe('> hi');
  });
});

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0);
  if (codePoint === undefined) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}

function packString(text: string, styleId = 0): number[] {
  return Array.from(text).map((char) => packCell(char, styleId));
}

function stringToBytes(text: string): Uint8Array {
  return Uint8Array.from(Array.from(text).map((char) => char.codePointAt(0) ?? 0));
}
