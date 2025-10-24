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

  it('keeps cursor at zero after a rect of spaces', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);

    cache.applyUpdates([
      { type: 'rect', rows: [0, 1], cols: [0, 80], seq: 1, cell: packCell(' ', DEFAULT_STYLE) },
    ]);

    let snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(0);
    expect(snapshot.cursorCol).toBe(0);

    const prompt = '[user@host ~]$ ';
    cache.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 2, cells: packString(prompt) },
    ]);

    snapshot = cache.snapshot();
    expect(snapshot.cursorCol).toBe(prompt.length);
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

  it('keeps cursor column when server reports position beyond committed width', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(2, 80);
    const prompt = 'prompt%';
    cache.applyUpdates([
      {
        type: 'row',
        row: 1,
        seq: 1,
        cells: packString(prompt),
      },
    ], { authoritative: true });

    cache.enableCursorSupport(true);
    const desiredCol = prompt.length + 1;
    cache.applyUpdates([], {
      cursor: { row: 1, col: desiredCol, seq: 2, visible: true, blink: true },
    });

    const snapshot = cache.snapshot();
    expect(snapshot.cursorRow).toBe(1);
    expect(snapshot.cursorCol).toBe(desiredCol);
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
    expect(snapshot.cursorCol).toBe(3);
    expect(snapshot.predictedCursor?.row).toBe(0);
    expect(snapshot.predictedCursor?.col).toBe(3);

    cache.clearPredictionSeq(3);
    snapshot = cache.snapshot();
    expect(snapshot.predictedCursor).toBeNull();
  });

  it('clears predicted spaces when acked without output', () => {
    const cache = new TerminalGridCache({ initialCols: 16 });
    cache.setGridSize(1, 16);
    cache.registerPrediction(1, stringToBytes(' '));
    let snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(true);

    cache.ackPrediction(1, 100);
    snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(false);
  });

  it('drops predictions when server refuses to move past the prompt', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);
    const prompt = '(base) user@host %';
    cache.applyUpdates(
      [
        {
          type: 'row',
          row: 0,
          seq: 1,
          cells: packString(prompt),
        },
      ],
      { authoritative: true },
    );
    cache.enableCursorSupport(true);
    cache.applyUpdates([], {
      cursor: { row: 0, col: prompt.length, seq: 2, visible: true, blink: true },
    });

    cache.registerPrediction(3, Uint8Array.from([0x7f]));

    let snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(false);
    expect(snapshot.predictedCursor).toBeNull();
    expect(snapshot.cursorCol).toBe(prompt.length);

    cache.applyUpdates([], {
      cursor: { row: 0, col: prompt.length, seq: 4, visible: true, blink: true },
    });

    snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(false);
    expect(snapshot.cursorCol).toBe(prompt.length);
    expect(snapshot.predictedCursor).toBeNull();
  });

  it('blocks predictive backspace before the server cursor minimum', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);
    const prompt = 'user@host %';
    cache.applyUpdates(
      [
        {
          type: 'row',
          row: 0,
          seq: 1,
          cells: packString(prompt),
        },
      ],
      { authoritative: true },
    );
    cache.enableCursorSupport(true);
    cache.applyUpdates([], {
      cursor: { row: 0, col: prompt.length, seq: 2, visible: true, blink: true },
    });

    const changed = cache.registerPrediction(3, Uint8Array.from([0x7f]));
    expect(changed).toBe(false);

    const snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(false);
    expect(snapshot.predictedCursor).toBeNull();
    expect(snapshot.cursorCol).toBe(prompt.length);
  });

  it('allows predictive backspace when deleting freshly typed input', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);
    const prompt = '$ ';
    cache.applyUpdates(
      [
        {
          type: 'row',
          row: 0,
          seq: 1,
          cells: packString(prompt),
        },
      ],
      { authoritative: true },
    );
    cache.enableCursorSupport(true);
    cache.applyUpdates([], {
      cursor: { row: 0, col: prompt.length, seq: 2, visible: true, blink: true },
    });

    cache.registerPrediction(3, stringToBytes('a'));
    let snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(true);
    expect(snapshot.predictedCursor?.col).toBe(prompt.length + 1);

    cache.registerPrediction(4, Uint8Array.from([0x7f]));
    snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(true);
    expect(snapshot.predictedCursor?.col).toBe(prompt.length);
  });

  it('keeps prompt floor after cursor resets to column zero', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(2, 80);
    const prompt = '(base) host %';
    cache.enableCursorSupport(true);
    cache.applyUpdates(
      [
        {
          type: 'row',
          row: 1,
          seq: 1,
          cells: packString(prompt),
        },
      ],
      { authoritative: true, cursor: { row: 1, col: prompt.length, seq: 2, visible: true, blink: true } },
    );

    cache.applyUpdates([], {
      cursor: { row: 1, col: 0, seq: 3, visible: true, blink: true },
    });

    let snapshot = cache.snapshot();
    expect(snapshot.cursorCol).toBe(0);

    const changed = cache.registerPrediction(4, Uint8Array.from([0x7f]));
    expect(changed).toBe(false);

    snapshot = cache.snapshot();
    expect(snapshot.hasPredictions).toBe(false);
    expect(snapshot.cursorCol).toBe(0);
  });

  it('resets cursor when a row segment of spaces clears the line', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });
    cache.setGridSize(1, 80);
    const prompt = '$ ';
    cache.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 1, cells: packString(prompt) },
    ]);
    const typed = '$ hello';
    cache.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 2, cells: packString(typed) },
    ]);
    let snapshot = cache.snapshot();
    expect(snapshot.cursorCol).toBe(typed.length);

    const blanks = ' '.repeat(typed.length);
    cache.applyUpdates([
      { type: 'row_segment', row: 0, startCol: 0, seq: 3, cells: packString(blanks) },
    ]);
    snapshot = cache.snapshot();
    expect(snapshot.cursorCol).toBe(0);
  });
});

describe('TerminalGridCache visibleRows', () => {
  it('pads missing rows when viewport height exceeds loaded content while following tail', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setBaseRow(100);
    cache.setGridSize(3, 4);

    cache.applyUpdates(
      [
        { type: 'row', row: 100, seq: 1, cells: packString('r0') },
        { type: 'row', row: 101, seq: 2, cells: packString('r1') },
        { type: 'row', row: 102, seq: 3, cells: packString('r2') },
      ],
      { authoritative: true },
    );

    cache.setViewport(0, 5);

    const rows = cache.visibleRows();
    expect(rows).toHaveLength(5);
    expect(rows[0]?.kind).toBe('missing');
    expect(rows[1]?.kind).toBe('missing');
    const loaded = rows.filter((row) => row.kind === 'loaded');
    expect(loaded.map((row) => row.absolute)).toEqual([100, 101, 102]);
  });

  it('retains trailing blank rows before padding to viewport height', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setBaseRow(200);
    cache.setGridSize(5, 4);

    cache.applyUpdates(
      [
        { type: 'row', row: 200, seq: 1, cells: packString('line0') },
        { type: 'row', row: 201, seq: 2, cells: packString('line1') },
        { type: 'row', row: 202, seq: 3, cells: packString('line2') },
        { type: 'row', row: 203, seq: 4, cells: packString('    ') },
        { type: 'row', row: 204, seq: 5, cells: packString('    ') },
      ],
      { authoritative: true },
    );

    cache.setViewport(0, 7);

    const rows = cache.visibleRows();
    expect(rows).toHaveLength(7);
    expect(rows.slice(-2).map((row) => ({ kind: row?.kind, absolute: row?.absolute }))).toEqual([
      { kind: 'loaded', absolute: 203 },
      { kind: 'loaded', absolute: 204 },
    ]);
  });

  it('pads newly exposed tail rows with missing slots until refreshed', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setGridSize(6, 4);

    cache.applyUpdates(
      [
        { type: 'row', row: 0, seq: 1, cells: packString('r0  ') },
        { type: 'row', row: 1, seq: 2, cells: packString('r1  ') },
        { type: 'row', row: 2, seq: 3, cells: packString('r2  ') },
        { type: 'row', row: 3, seq: 4, cells: packString('r3  ') },
        { type: 'row', row: 4, seq: 5, cells: packString('r4  ') },
        { type: 'row', row: 5, seq: 6, cells: packString('r5  ') },
      ],
      { authoritative: true },
    );

    cache.setViewport(4, 2);
    let rows = cache.visibleRows();
    expect(rows.map((row) => row.kind)).toEqual(['loaded', 'loaded']);

    cache.setViewport(2, 4);
    rows = cache.visibleRows();
    expect(rows.map((row) => row.kind)).toEqual(['missing', 'missing', 'loaded', 'loaded']);

    cache.applyUpdates([{ type: 'row', row: 2, seq: 7, cells: packString('    ') }], { authoritative: true });
    cache.applyUpdates([{ type: 'row', row: 3, seq: 8, cells: packString('    ') }], { authoritative: true });

    rows = cache.visibleRows();
    expect(rows.map((row) => row.kind)).toEqual(['loaded', 'loaded', 'loaded', 'loaded']);
  });

  it('keeps padding active after followTail is disabled', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setGridSize(6, 4);

    cache.applyUpdates(
      [
        { type: 'row', row: 0, seq: 1, cells: packString('r0  ') },
        { type: 'row', row: 1, seq: 2, cells: packString('r1  ') },
        { type: 'row', row: 2, seq: 3, cells: packString('r2  ') },
        { type: 'row', row: 3, seq: 4, cells: packString('r3  ') },
        { type: 'row', row: 4, seq: 5, cells: packString('r4  ') },
        { type: 'row', row: 5, seq: 6, cells: packString('r5  ') },
      ],
      { authoritative: true },
    );

    cache.setViewport(4, 2);
    cache.visibleRows();

    cache.setViewport(2, 4);
    cache.setFollowTail(false);
    let rows = cache.visibleRows();
    expect(rows.map((row) => row.kind)).toEqual(['missing', 'missing', 'loaded', 'loaded']);

    cache.applyUpdates([{ type: 'row', row: 2, seq: 7, cells: packString('    ') }], { authoritative: true });
    cache.applyUpdates([{ type: 'row', row: 3, seq: 8, cells: packString('    ') }], { authoritative: true });

    rows = cache.visibleRows();
    expect(rows.map((row) => row.kind)).toEqual(['loaded', 'loaded', 'loaded', 'loaded']);
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
