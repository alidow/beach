import { describe, it, expect, vi } from 'vitest';
import { TerminalGridStore } from '../gridStore';
import { BackfillController } from '../backfillController';
import type { HostFrame } from '../protocol/types';

function seedRows(store: TerminalGridStore, start: number, end: number): void {
  const updates = [];
  for (let row = start; row < end; row += 1) {
    updates.push({
      type: 'row' as const,
      row,
      seq: row + 1,
      cells: new Array(10).fill(0),
    });
  }
  store.applyUpdates(updates, { authoritative: true });
}

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

describe('BackfillController follow-tail intent', () => {
  it("does not re-enable follow-tail when the user has scrolled away", () => {
    const store = new TerminalGridStore();
    store.setGridSize(180, 80);
    seedRows(store, 0, 150);

    // Mark a pending gap near the tail so maybeRequest issues a backfill.
    store.markPendingRange(150, 160);

    // Simulate user scrolling away from tail.
    store.setViewport(60, 24);
    store.setFollowTail(false);

    const sendFrame = vi.fn();
    const controller = new BackfillController(store, sendFrame);
    controller.handleFrame(helloFrame());

    const snapshotBefore = store.getSnapshot();
    controller.maybeRequest(snapshotBefore, {
      nearBottom: false,
      followTailDesired: false,
      phase: 'manual_scrollback',
      tailPaddingRows: 0,
    });

    const snapshotAfter = store.getSnapshot();
    expect(sendFrame).toHaveBeenCalled();
    expect(snapshotAfter.followTail).toBe(false);
    expect(snapshotAfter.viewportTop).toBe(60);
  });

  it('requests tail gaps while catching up without forcing follow-tail', () => {
    const store = new TerminalGridStore();
    store.setGridSize(200, 80);
    seedRows(store, 0, 140);
    store.markPendingRange(140, 150);
    store.setViewport(130, 24);
    store.setFollowTail(false);

    const sendFrame = vi.fn();
    const controller = new BackfillController(store, sendFrame);
    controller.handleFrame(helloFrame());

    const snapshotBefore = store.getSnapshot();
    controller.maybeRequest(snapshotBefore, {
      nearBottom: false,
      followTailDesired: true,
      phase: 'catching_up',
      tailPaddingRows: 0,
    });

    const snapshotAfter = store.getSnapshot();
    expect(sendFrame).toHaveBeenCalled();
    expect(snapshotAfter.followTail).toBe(false);
  });

  it('treats placeholder padding as tail intent when scanning for gaps', () => {
    const store = new TerminalGridStore();
    store.setGridSize(120, 80);
    seedRows(store, 0, 110);
    store.markPendingRange(110, 120);
    store.setViewport(96, 24);
    store.setFollowTail(true);

    const sendFrame = vi.fn();
    const controller = new BackfillController(store, sendFrame);
    controller.handleFrame(helloFrame());

    const snapshotBefore = store.getSnapshot();
    controller.maybeRequest(snapshotBefore, {
      nearBottom: false,
      followTailDesired: true,
      phase: 'follow_tail',
      tailPaddingRows: 12,
    });

    expect(sendFrame).toHaveBeenCalled();
    const snapshotAfter = store.getSnapshot();
    expect(snapshotAfter.followTail).toBe(true);
  });
});
