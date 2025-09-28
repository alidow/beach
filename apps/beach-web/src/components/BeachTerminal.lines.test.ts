import { describe, expect, it } from 'vitest';
import type { CellState, LoadedRow, TerminalGridSnapshot } from '../terminal/gridStore';
import { buildLines } from './BeachTerminal';

function makeLoadedRow(absolute: number, text: string, seq = 1): LoadedRow {
  const cells: CellState[] = Array.from(text).map((char, index) => ({
    char,
    styleId: 0,
    seq: seq + index,
  }));
  return {
    kind: 'loaded',
    absolute,
    latestSeq: seq,
    cells,
  };
}

describe('buildLines', () => {
  it('returns the tail of loaded rows when following the live tail', () => {
    const snapshot: TerminalGridSnapshot = {
      baseRow: 90,
      cols: 80,
      rows: [
        makeLoadedRow(92, 'history'),
        makeLoadedRow(100, 'current-line'),
        makeLoadedRow(101, 'next-line'),
        makeLoadedRow(102, 'future'),
      ],
      styles: new Map(),
      followTail: true,
      historyTrimmed: false,
      viewportTop: 100,
      viewportHeight: 2,
    };

    const lines = buildLines(snapshot, 600);

    expect(lines.map((line) => line.absolute)).toEqual([101, 102]);
    expect(lines.map((line) => line.text)).toEqual(['next-line', 'future']);
  });

  it('falls back to tail rows when viewport height is unknown', () => {
    const snapshot: TerminalGridSnapshot = {
      baseRow: 0,
      cols: 80,
      rows: [
        makeLoadedRow(0, 'first'),
        makeLoadedRow(1, 'second'),
        makeLoadedRow(2, 'third'),
      ],
      styles: new Map(),
      followTail: true,
      historyTrimmed: false,
      viewportTop: 0,
      viewportHeight: 0,
    };

    const lines = buildLines(snapshot, 2);

    expect(lines.map((line) => line.absolute)).toEqual([1, 2]);
    expect(lines.map((line) => line.text)).toEqual(['second', 'third']);
  });

  it('respects viewportTop when not following the tail', () => {
    const snapshot: TerminalGridSnapshot = {
      baseRow: 0,
      cols: 80,
      rows: [
        makeLoadedRow(0, 'row0'),
        makeLoadedRow(1, 'row1'),
        makeLoadedRow(2, 'row2'),
        makeLoadedRow(3, 'row3'),
      ],
      styles: new Map(),
      followTail: false,
      historyTrimmed: false,
      viewportTop: 1,
      viewportHeight: 2,
    };

    const lines = buildLines(snapshot, 10);

    expect(lines.map((line) => line.absolute)).toEqual([1, 2]);
    expect(lines.map((line) => line.text)).toEqual(['row1', 'row2']);
  });
});
