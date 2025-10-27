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

describe('TerminalGridCache authoritative snapshots', () => {
  it('overwrites stale rows when snapshots replay lower sequence numbers', () => {
    const cache = new TerminalGridCache({ initialCols: 8 });
    cache.setGridSize(1, 8);

    cache.applyUpdates(
      [{ type: 'row', row: 0, seq: 400, cells: packString('OLD ') }],
      { authoritative: true },
    );
    expect(cache.getRowText(0)).toBe('OLD');

    cache.applyUpdates(
      [{ type: 'row', row: 0, seq: 10, cells: packString('NEW ') }],
      { authoritative: true },
    );

    const snapshot = cache.snapshot();
    const row = snapshot.rows[0];
    expect(row?.kind).toBe('loaded');
    if (row?.kind === 'loaded') {
      const text = row.cells.map((cell) => cell.char).join('').trimEnd();
      expect(text).toBe('NEW');
    }
  });
});

describe('TerminalGridCache visibleRows', () => {
  it('anchors the initial viewport to the base row when the server origin is unknown', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setGridSize(4, 4);

    cache.setViewport(20, 10);

    const snapshot = cache.snapshot();
    expect(snapshot.followTail).toBe(false);
    expect(snapshot.viewportTop).toBe(0);
    expect(snapshot.viewportHeight).toBe(4);
    const rows = cache.visibleRows(2);
    expect(rows[0]?.absolute).toBe(0);
  });

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

    cache.setFollowTail(true);
    cache.setViewport(0, 5);

    const rows = cache.visibleRows();
    expect(rows).toHaveLength(5);
    expect(rows[0]?.kind).toBe('missing');
    expect(rows[1]?.kind).toBe('missing');
    const loaded = rows.filter((row) => row.kind === 'loaded');
    expect(loaded.map((row) => row.absolute)).toEqual([100, 101, 102]);
  });

  it('does not reuse older history rows when viewport exceeds PTY height in follow-tail mode', () => {
    const cache = new TerminalGridCache({ initialCols: 4 });
    cache.setBaseRow(80);
    cache.setGridSize(8, 4);

    for (let index = 0; index < 8; index += 1) {
      cache.applyUpdates(
        [{ type: 'row', row: 80 + index, seq: 10 + index, cells: packString(`t${index.toString().padEnd(3, ' ')}`) }],
        { authoritative: true },
      );
    }

    cache.setBaseRow(60);
    for (let index = 0; index < 20; index += 1) {
      cache.applyUpdates(
        [{ type: 'row', row: 60 + index, seq: 100 + index, cells: packString(`h${index.toString().padEnd(3, ' ')}`) }],
        { authoritative: true },
      );
    }

    cache.setFollowTail(true);
    cache.setViewport(0, 12);

    const rows = cache.visibleRows();
    expect(rows).toHaveLength(12);
    expect(rows.slice(0, 4).map((row) => row.kind)).toEqual(['missing', 'missing', 'missing', 'missing']);
    expect(rows.slice(4).map((row) => row.absolute)).toEqual([80, 81, 82, 83, 84, 85, 86, 87]);
  });

  it('retains trailing blank rows even when viewport height exceeds grid size', () => {
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
    const loadedTail = rows.filter((row) => row.kind === 'loaded').slice(-2).map((row) => ({
      kind: row?.kind,
      absolute: row?.absolute,
    }));
    expect(loadedTail).toEqual([
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

    cache.setFollowTail(true);
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

    cache.setFollowTail(true);
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

  it('keeps tail padding when authoritative backfill replays existing rows', () => {
    const cache = new TerminalGridCache({ initialCols: 16 });
    cache.setGridSize(8, 16);

    const hudLines = ['Unknown command', 'Commands', 'Mode', '> '];
    hudLines.forEach((line, index) => {
      cache.applyUpdates(
        [
          {
            type: 'row',
            row: index,
            seq: index + 1,
            cells: packString(line.padEnd(16, ' ')),
          },
        ],
        { authoritative: true },
      );
    });

    cache.setFollowTail(true);
    cache.setViewport(0, 4);
    cache.visibleRows();

    cache.setViewport(0, 8);
    let rows = cache.visibleRows();
    // After PTY resize fix: rows 4-7 were created as blank loaded rows by setGridSize
    // They should be 'loaded', not 'missing', since they're part of the grid
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);

    const replayUpdates = hudLines.map((line, index) => ({
      type: 'row' as const,
      row: index,
      seq: 100 + index,
      cells: packString(line.padEnd(16, ' ')),
    }));
    cache.applyUpdates(replayUpdates, { authoritative: true, origin: 'history_backfill' });

    rows = cache.visibleRows();
    // After backfill replay, rows 4-7 remain as loaded (blank)
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);
  });

  it('keeps tail padding when delta replays identical rows', () => {
    const cache = new TerminalGridCache({ initialCols: 16 });
    cache.setGridSize(8, 16);

    const hudLines = ['Unknown command', 'Commands', 'Mode', '> '];
    hudLines.forEach((line, index) => {
      cache.applyUpdates(
        [
          {
            type: 'row',
            row: index,
            seq: index + 1,
            cells: packString(line.padEnd(16, ' ')),
          },
        ],
        { authoritative: true },
      );
    });

    cache.setFollowTail(true);
    cache.setViewport(0, 4);
    cache.visibleRows();

    cache.setViewport(0, 8);
    let rows = cache.visibleRows();
    // After PTY resize fix: rows 4-7 were created as blank loaded rows by setGridSize
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);

    const deltaUpdates = hudLines.map((line, index) => ({
      type: 'row' as const,
      row: index,
      seq: 500 + index,
      cells: packString(line.padEnd(16, ' ')),
    }));
    cache.applyUpdates(deltaUpdates, { authoritative: false, origin: 'delta' });

    rows = cache.visibleRows();
    // After delta replay, rows 4-7 remain as loaded (blank)
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);
  });

  it('keeps tail padding when history replays reuse original sequence numbers', () => {
    const cache = new TerminalGridCache({ initialCols: 16 });
    cache.setGridSize(8, 16);

    const hudLines = ['Unknown command', 'Commands', 'Mode', '> '];
    hudLines.forEach((line, index) => {
      cache.applyUpdates(
        [
          {
            type: 'row',
            row: index,
            seq: index + 1,
            cells: packString(line.padEnd(16, ' ')),
          },
        ],
        { authoritative: true },
      );
    });

    cache.setViewport(0, 4);
    cache.visibleRows();

    cache.setViewport(0, 8);
    let rows = cache.visibleRows();
    // After PTY resize fix: rows 4-7 were created as blank loaded rows by setGridSize
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);

    const staleUpdates = hudLines.map((line, index) => ({
      type: 'row' as const,
      row: index,
      seq: index + 1,
      cells: packString(line.padEnd(16, ' ')),
    }));
    cache.applyUpdates(staleUpdates, { authoritative: true, origin: 'history_backfill' });

    rows = cache.visibleRows();
    // After history replay, rows 4-7 remain as loaded (blank)
    expect(rows.slice(4, 8).map((row) => row.kind)).toEqual([
      'loaded',
      'loaded',
      'loaded',
      'loaded',
    ]);
  });

  it('PTY resize creates blank loaded rows not pending rows', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });

    // Initial grid with 24 rows (simulating initial PTY size)
    cache.setGridSize(24, 80);

    // Verify all rows are loaded (not pending)
    const initialSnapshot = cache.snapshot();
    expect(initialSnapshot.rows.length).toBe(24);
    expect(initialSnapshot.rows.every((row) => row.kind === 'loaded')).toBe(true);

    // Add some content to the first few rows
    for (let i = 0; i < 4; i++) {
      cache.applyUpdates(
        [
          {
            type: 'row',
            row: i,
            seq: i + 1,
            cells: packString(`Line ${i}`.padEnd(80, ' ')),
          },
        ],
        { authoritative: true },
      );
    }

    // Simulate PTY resize from 24 rows to 36 rows
    cache.setGridSize(36, 80);

    // Verify new rows (24-35) are loaded with blank content, not pending
    const afterResize = cache.snapshot();
    expect(afterResize.rows.length).toBe(36);

    // Check that rows 24-35 are loaded (not pending)
    for (let i = 24; i < 36; i++) {
      const row = afterResize.rows[i];
      expect(row).toBeDefined();
      expect(row.kind).toBe('loaded');
      if (row.kind === 'loaded') {
        // Verify they're blank (all spaces)
        const text = row.cells.map((cell) => cell.char).join('');
        expect(text.trim()).toBe('');
        // Verify they have a non-zero latestSeq to prevent backfill
        expect(row.latestSeq).toBeGreaterThan(0);
      }
    }

    // Verify original content rows (0-3) are unchanged
    for (let i = 0; i < 4; i++) {
      const row = afterResize.rows[i];
      expect(row.kind).toBe('loaded');
      if (row.kind === 'loaded') {
        const text = row.cells.map((cell) => cell.char).join('').trim();
        expect(text).toBe(`Line ${i}`);
      }
    }

    // Most importantly: rows 4-23 should be loaded (blank), not pending
    for (let i = 4; i < 24; i++) {
      const row = afterResize.rows[i];
      expect(row.kind).toBe('loaded');
      if (row.kind === 'loaded') {
        // These rows should also have non-zero latestSeq
        expect(row.latestSeq).toBeGreaterThan(0);
      }
    }
  });

  it('PTY resize does not trigger backfill - newly created rows have non-zero latestSeq', () => {
    const cache = new TerminalGridCache({ initialCols: 80 });

    // Apply some initial content to establish a sequence number
    cache.applyUpdates([{
      type: 'row',
      row: 0,
      seq: 1,
      cells: packString('Initial content'.padEnd(80, ' ')),
    }], { authoritative: true });

    // Simulate PTY with 24 rows
    cache.setBaseRow(0);
    cache.setGridSize(24, 80);

    let snapshot = cache.snapshot();
    expect(snapshot.rows.length).toBe(24);
    expect(snapshot.baseRow).toBe(0);

    // Verify all 24 rows are loaded with non-zero latestSeq
    for (let i = 0; i < 24; i++) {
      const row = snapshot.rows[i];
      expect(row?.kind).toBe('loaded');
      if (row?.kind === 'loaded') {
        expect(row.latestSeq).toBeGreaterThan(0);
      }
    }

    // Now resize from 24 to 36 rows (simulate PTY resize taller)
    cache.setGridSize(36, 80);

    snapshot = cache.snapshot();
    expect(snapshot.rows.length).toBe(36);

    // **CRITICAL TEST**: Newly created rows (24-35) must have non-zero latestSeq
    // to prevent backfill controller from treating them as gaps
    for (let i = 24; i < 36; i++) {
      const row = snapshot.rows[i];
      expect(row?.kind).toBe('loaded');
      if (row?.kind === 'loaded') {
        // This is the key fix: latestSeq > 0 prevents findTailGap from requesting backfill
        expect(row.latestSeq).toBeGreaterThan(0);
        // Verify they're blank
        const text = row.cells.map((cell) => cell.char).join('').trim();
        expect(text).toBe('');
      }
    }

    // SIMULATE findTailGap logic (from backfillController.ts line 238-266)
    const maxLoadedRowIndex = snapshot.rows.findLastIndex(r => r.kind === 'loaded');
    expect(maxLoadedRowIndex).toBeGreaterThanOrEqual(35);

    if (maxLoadedRowIndex >= 0) {
      const maxLoadedRow = snapshot.rows[maxLoadedRowIndex];
      if (maxLoadedRow?.kind === 'loaded') {
        const BACKFILL_LOOKAHEAD_ROWS = 64;
        const scanStart = Math.max(snapshot.baseRow, maxLoadedRow.absolute - BACKFILL_LOOKAHEAD_ROWS);

        // Scan for gaps like findTailGap does
        let gapFound = false;
        for (let absolute = scanStart; absolute <= maxLoadedRow.absolute; absolute += 1) {
          const index = absolute - snapshot.baseRow;
          if (index < 0 || index >= snapshot.rows.length) {
            continue;
          }
          const slot = snapshot.rows[index];

          // backfillController.ts line 257-261: check if row is a gap
          if (!slot || slot.kind !== 'loaded') {
            gapFound = true;
            break;
          }
          // backfillController.ts line 263-266: THIS WAS THE BUG!
          // Rows with latestSeq === 0 are treated as gaps
          if (slot.latestSeq === 0) {
            gapFound = true;
            break;
          }
        }

        // With the fix, no gaps should be found
        expect(gapFound).toBe(false);
      }
    }
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
