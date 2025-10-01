import { describe, expect, it } from 'vitest';
import { TerminalGridCache } from './cache';

const DEFAULT_STYLE = 0;

describe('TerminalGridCache cursor hints', () => {
  it('places the cursor after trailing blanks supplied by row updates', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);

    cache.applyUpdates(
      [
        {
          type: 'row',
          row: 0,
          seq: 1,
          cells: packString('% '),
        },
      ],
      { authoritative: true },
    );

    const snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(0);
    expect(snapshot.cursorCol).toBe(2);
  });

  it('treats cursor hints as mutations even when the grid cells stay unchanged', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);

    cache.applyUpdates([
      {
        type: 'row',
        row: 0,
        seq: 0,
        cells: [],
      },
    ]);

    const changed = cache.applyUpdates([
      {
        type: 'rect',
        rows: [0, 1],
        cols: [0, 80],
        seq: 0,
        cell: packCell(' ', DEFAULT_STYLE),
      },
    ]);

    expect(changed).toBe(true);
    const snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(0);
    expect(snapshot.cursorCol).toBe(0);
  });

  it('applies cursor frames when support is enabled', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(2, 80);
    cache.applyUpdates([
      {
        type: 'row',
        row: 0,
        seq: 1,
        cells: packString('prompt'),
      },
    ], { authoritative: true });

    cache.enableCursorSupport(true);
    cache.applyUpdates([], {
      cursor: { row: 0, col: 6, seq: 2, visible: true, blink: false },
    });

    const snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(0);
    expect(snapshot.cursorCol).toBe(6);
    expect(snapshot.cursorVisible).toBe(true);
    expect(snapshot.cursorAuthoritative).toBe(true);
    expect(snapshot.cursorSeq).toBe(2);
  });

  it('tracks predicted cursor while preserving authoritative cursor', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);
    cache.applyUpdates([
      {
        type: 'row',
        row: 0,
        seq: 1,
        cells: packString('> '),
      },
    ], { authoritative: true });

    cache.enableCursorSupport(true);
    cache.applyUpdates([], {
      cursor: { row: 0, col: 2, seq: 2, visible: true, blink: true },
    });

    cache.registerPrediction(3, stringToBytes('a'));

    let snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(0);
    expect(snapshot.cursorCol).toBe(2);
    expect(snapshot.predictedCursor?.row).toBe(0);
    expect(snapshot.predictedCursor?.col).toBe(3);

    cache.clearPredictionSeq(3);
    snapshot = cache.snapshot();
    expect(snapshot.predictedCursor).toBeNull();
  });
});

function packString(text: string, styleId = DEFAULT_STYLE): number[] {
  return Array.from(text).map((char) => packCell(char, styleId));
}

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0);
  if (codePoint === undefined) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}

function stringToBytes(value: string): Uint8Array {
  return new TextEncoder().encode(value);
}
