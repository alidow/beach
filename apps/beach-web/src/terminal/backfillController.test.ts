import { describe, expect, it } from 'vitest';
import { BackfillController } from './backfillController';
import { TerminalGridStore } from './gridStore';
import type { ClientFrame, HostFrame } from '../protocol/types';

function packCell(char: string, styleId = 0): number {
  const codePoint = char.codePointAt(0);
  if (codePoint === undefined) {
    throw new Error('invalid char');
  }
  return codePoint * 2 ** 32 + styleId;
}

function packRow(text: string): number[] {
  return Array.from(text, (char) => packCell(char));
}

describe('BackfillController', () => {
  function helloFrame(): Extract<HostFrame, { type: 'hello' }> {
    return {
      type: 'hello',
      subscription: 1,
      maxSeq: 0,
      features: 0,
      config: {
        snapshotBudgets: [],
        deltaBudget: 0,
        heartbeatMs: 0,
        initialSnapshotLines: 0,
      },
    };
  }

  it('requests tail backfill while following tail when gaps are present', () => {
    const store = new TerminalGridStore();
    store.setBaseRow(0);
    store.setGridSize(80, 80);

    const updates = [];
    for (let row = 20; row < 80; row += 1) {
      updates.push({
        type: 'row' as const,
        row,
        seq: row,
        cells: packRow(`row-${row}`),
      });
    }
    store.applyUpdates(updates, { authoritative: true });
    store.setViewport(32, 48);
    store.setFollowTail(true);

    const frames: ClientFrame[] = [];
    const controller = new BackfillController(store, (frame) => {
      frames.push(frame);
    });
    controller.handleFrame(helloFrame());

    controller.maybeRequest(store.getSnapshot(), true);

    expect(frames).toHaveLength(1);
    const request = frames[0];
    expect(request?.type).toBe('request_backfill');
    if (request?.type === 'request_backfill') {
      expect(request.startRow).toBeLessThan(20);
      expect(request.count).toBeGreaterThan(0);
    }
  });

  it('marks unresolved rows as missing after an empty history backfill', () => {
    const store = new TerminalGridStore();
    store.setBaseRow(0);
    store.setGridSize(10, 40);
    store.markPendingRange(0, 4);

    const controller = new BackfillController(store, () => {});
    controller.handleFrame(helloFrame());

    controller.finalizeHistoryBackfill({
      type: 'history_backfill',
      subscription: 1,
      requestId: 1,
      startRow: 0,
      count: 4,
      updates: [],
      more: false,
      cursor: undefined,
    });

    const snapshot = store.getSnapshot();
    for (let index = 0; index < 4; index += 1) {
      const row = snapshot.rows[index];
      expect(row?.kind).toBe('missing');
    }
  });
});
