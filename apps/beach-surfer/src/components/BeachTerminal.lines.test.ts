import { describe, expect, it } from 'vitest';
import { TerminalGridStore } from '../terminal/gridStore';
import { buildLines } from './BeachTerminal';

const PACKED_STYLE = 0;
const ACK_GRACE_MS = 90;

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
      { authoritative: true },
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

  it('highlights predicted cursor position when available', () => {
    const store = new TerminalGridStore();
    store.setGridSize(1, 80);
    store.applyUpdates([packRow(0, '> ')], { authoritative: true });
    store.setCursorSupport(true);
    store.applyCursorFrame({ row: 0, col: 2, seq: 1, visible: true, blink: true });

    store.registerPrediction(2, stringToBytes('a'));

    const [line] = buildLines(store.getSnapshot(), 10);
    expect(line.cursorCol).toBe(3);
    expect(line.predictedCursorCol).toBe(3);
  });

  it('keeps predicted cells visible until cleared even if overlay is hidden', () => {
    const store = new TerminalGridStore();
    store.setGridSize(1, 80);
    store.applyUpdates([packRow(0, '> ')], { authoritative: true });
    store.registerPrediction(1, stringToBytes('a'));

    const snapshotWithPrediction = store.getSnapshot();
    expect(snapshotWithPrediction.hasPredictions).toBe(true);

    const [lineVisible] = buildLines(snapshotWithPrediction, 10, { visible: true, underline: false });
    expect(lineVisible.cells?.some((cell) => cell.predicted)).toBe(true);

    const hiddenOverlay = snapshotWithPrediction.hasPredictions
      ? { visible: true, underline: false }
      : { visible: false, underline: false };
    const [lineHidden] = buildLines(snapshotWithPrediction, 10, hiddenOverlay);
    expect(lineHidden.cells?.some((cell) => cell.predicted)).toBe(true);

    store.ackPrediction(1, 100);
    store.pruneAckedPredictions(100 + ACK_GRACE_MS, ACK_GRACE_MS);

    const snapshotAfterAck = store.getSnapshot();
    expect(snapshotAfterAck.hasPredictions).toBe(true);

    const overlayAfterAck = snapshotAfterAck.hasPredictions
      ? { visible: true, underline: false }
      : { visible: false, underline: false };
    const [lineAfterAck] = buildLines(snapshotAfterAck, 10, overlayAfterAck);
    expect(lineAfterAck.cells?.some((cell) => cell.predicted)).toBe(true);

    store.applyUpdates([packRow(0, '> ', 2)], { authoritative: true });
    store.pruneAckedPredictions(140 + ACK_GRACE_MS, ACK_GRACE_MS);

    const snapshotCleared = store.getSnapshot();
    expect(snapshotCleared.hasPredictions).toBe(false);

    const [lineCleared] = buildLines(snapshotCleared, 10, { visible: false, underline: false });
    expect(lineCleared.cells?.some((cell) => cell.predicted)).toBe(false);
  });
});

function textFromLine(line: ReturnType<typeof buildLines>[number]): string {
  return line.cells?.map((cell) => cell.char).join('').trimEnd() ?? '';
}

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

function stringToBytes(text: string): Uint8Array {
  return Uint8Array.from(Array.from(text).map((char) => char.codePointAt(0) ?? 0));
}
