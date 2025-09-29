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
      true,
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
