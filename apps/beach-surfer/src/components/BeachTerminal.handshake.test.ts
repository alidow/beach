import { describe, expect, it } from 'vitest';
import { TerminalGridStore } from '../terminal/gridStore';

const PACKED_STYLE = 0;

type GridHandshake = {
  baseRow: number;
  historyRows: number;
  cols: number;
};

function packCell(char: string, styleId: number): number {
  const codePoint = char.codePointAt(0);
  if (codePoint == null) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}

function packString(text: string): number[] {
  return Array.from(text).map((char) => packCell(char, PACKED_STYLE));
}

function packRow(row: number, text: string, seq = row + 1) {
  return { type: 'row' as const, row, seq, cells: packString(text) };
}

function populateStoreWithHistory(store: TerminalGridStore, totalRows: number, cols: number) {
  store.reset();
  store.setHistoryOrigin(0);
  store.setGridSize(totalRows, cols);
  const updates = Array.from({ length: totalRows }, (_, index) =>
    packRow(index, `Line ${index + 1}: Test`),
  );
  store.applyUpdates(updates, { authoritative: true });
  store.setViewport(0, Math.min(totalRows, 24));
}

function applyGridFrame(
  store: TerminalGridStore,
  frame: GridHandshake,
  lastMeasuredViewportRows = 24,
) {
  const snapshot = store.getSnapshot();
  const hydratedRows = snapshot.rows.length;
  const hydratedBaseRow = snapshot.baseRow;
  const hydratedCols = snapshot.cols;
  const hydratedViewportTop = snapshot.viewportTop;
  const hasHydratedHistory = hydratedRows > 0;
  const handshakeHasHistory = frame.historyRows > 0;
  const nextBaseRow = (() => {
    if (hasHydratedHistory) {
      if (!handshakeHasHistory) {
        return hydratedBaseRow;
      }
      return Math.min(hydratedBaseRow, frame.baseRow);
    }
    if (handshakeHasHistory) {
      return frame.baseRow;
    }
    return snapshot.baseRow;
  })();
  if (snapshot.baseRow !== nextBaseRow) {
    store.setBaseRow(nextBaseRow);
  }
  const hydratedEnd = hydratedBaseRow + hydratedRows;
  const handshakeEnd = handshakeHasHistory ? frame.baseRow + frame.historyRows : hydratedEnd;
  const unionEnd = Math.max(hydratedEnd, handshakeEnd);
  const desiredTotalRows = Math.max(hydratedRows, unionEnd - nextBaseRow);
  const desiredCols = Math.max(hydratedCols || 0, frame.cols);
  store.setGridSize(desiredTotalRows, desiredCols);
  store.setFollowTail(false);
  const deviceViewport = Math.max(1, Math.min(lastMeasuredViewportRows, 512));
  const viewportTopCandidate = hasHydratedHistory
    ? hydratedViewportTop
    : handshakeHasHistory
      ? frame.baseRow
      : snapshot.viewportTop;
  const maxViewportTop = Math.max(nextBaseRow, unionEnd - deviceViewport);
  const clampedViewportTop = Math.min(
    Math.max(viewportTopCandidate, nextBaseRow),
    maxViewportTop,
  );
  store.setViewport(clampedViewportTop, deviceViewport);
}

describe('BeachTerminal grid handshake reconciliation', () => {
  it('ignores base-row hints when the host has no history yet', () => {
    const store = new TerminalGridStore();
    applyGridFrame(store, { baseRow: 62, historyRows: 0, cols: 153 }, 6);
    const snapshot = store.getSnapshot();
    expect(snapshot.baseRow).toBe(0);
    expect(snapshot.viewportTop).toBe(0);
    expect(snapshot.rows.length).toBe(0);
  });

  it('preserves prehydrated history when subsequent grid frames advertise a higher base row', () => {
    const store = new TerminalGridStore(80);
    populateStoreWithHistory(store, 153, 80);
    applyGridFrame(store, { baseRow: 91, historyRows: 62, cols: 80 }, 24);

    const snapshot = store.getSnapshot();
    expect(snapshot.baseRow).toBe(0);
    expect(snapshot.viewportTop).toBe(0);
    expect(snapshot.rows.length).toBeGreaterThanOrEqual(153);
    expect(snapshot.rows[0]?.kind).toBe('loaded');
    expect(snapshot.rows[0]?.absolute).toBe(0);
  });
});
