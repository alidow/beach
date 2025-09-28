import { describe, expect, it } from 'vitest';
import { TerminalGridStore } from '../terminal/gridStore';
import { buildLines } from './BeachTerminal';

const PACKED_STYLE = 0;

function packRow(row: number, text: string, seq = 1) {
  return { type: 'row' as const, row, seq, cells: packString(text) };
}

describe('buildLines', () => {
  it('returns the tail of loaded rows when following the live tail', () => {
    const store = new TerminalGridStore();
    store.setBaseRow(90);
    store.setGridSize(24, 80);
    store.applyUpdates(
      [
        packRow(92, 'history'),
        packRow(100, 'current-line'),
        packRow(101, 'next-line'),
        packRow(102, 'future'),
      ],
      true,
    );
    store.setViewport(100, 2);

    const snapshot = store.getSnapshot();
    const lines = buildLines(snapshot, 600);

    expect(lines.map((line) => line.absolute)).toEqual([101, 102]);
    expect(lines.map((line) => textFromLine(line))).toEqual(['next-line', 'future']);
  });

  it('falls back to tail rows when viewport height is unknown', () => {
    const store = new TerminalGridStore();
    store.setGridSize(3, 80);
    store.applyUpdates([
      packRow(0, 'first'),
      packRow(1, 'second'),
      packRow(2, 'third'),
    ]);

    const snapshot = store.getSnapshot();
    const lines = buildLines(snapshot, 2);

    expect(lines.map((line) => line.absolute)).toEqual([1, 2]);
    expect(lines.map((line) => textFromLine(line))).toEqual(['second', 'third']);
  });

  it('respects viewportTop when not following the tail', () => {
    const store = new TerminalGridStore();
    store.setGridSize(4, 80);
    store.applyUpdates([
      packRow(0, 'row0'),
      packRow(1, 'row1'),
      packRow(2, 'row2'),
      packRow(3, 'row3'),
    ]);
    store.setFollowTail(false);
    store.setViewport(1, 2);

    const snapshot = store.getSnapshot();
    const lines = buildLines(snapshot, 10);

    expect(lines.map((line) => line.absolute)).toEqual([1, 2]);
    expect(lines.map((line) => textFromLine(line))).toEqual(['row1', 'row2']);
});

function textFromLine(line: ReturnType<typeof buildLines>[number]): string {
  return line.cells?.map((cell) => cell.char).join('').trimEnd() ?? '';
}
});

function packString(text: string): number[] {
  return Array.from(text).map((char) => packCell(char, PACKED_STYLE));
}

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0);
  if (codePoint === undefined) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}
