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
});

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0);
  if (codePoint === undefined) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}
