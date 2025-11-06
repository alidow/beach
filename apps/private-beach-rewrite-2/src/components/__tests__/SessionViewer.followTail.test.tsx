import { describe, it, expect, afterEach, vi, beforeAll } from 'vitest';
import { render, act, waitFor, cleanup } from '@testing-library/react';
import { SessionViewer } from '../SessionViewer';
import { TerminalGridStore } from '../../../../beach-surfer/src/terminal/gridStore';

vi.mock('../../../../beach-surfer/src/components/BeachTerminal', () => ({
  BeachTerminal: () => null,
}));

beforeAll(() => {
  vi.stubGlobal('fetch', vi.fn());
});

const seedStore = (): TerminalGridStore => {
  const store = new TerminalGridStore(80);
  store.setBaseRow(0);
  store.setGridSize(256, 80);
  const updates = [];
  for (let row = 0; row < 220; row += 1) {
    updates.push({
      type: 'row' as const,
      row,
      seq: row + 1,
      cells: Array.from({ length: 10 }, () => ({ char: 'x', styleId: 0, seq: row + 1 })),
    });
  }
  store.applyUpdates(updates, { authoritative: true });
  store.setViewport(200, 18);
  store.setFollowTail(true);
  return store;
};

describe('SessionViewer follow-tail intent (rewrite-2)', () => {
  afterEach(() => {
    cleanup();
  });

  it('keeps follow-tail enabled after viewport adjustments', async () => {
    const store = seedStore();
    const viewer = {
      store,
      transport: {} as any,
      connecting: false,
      error: null,
      status: 'connected' as const,
      secureSummary: null,
      latencyMs: null,
    };

    render(
      <SessionViewer
        viewer={viewer}
        sessionId="rewrite2-session"
        disableViewportMeasurements
      />,
    );

    expect(store.getSnapshot().followTail).toBe(true);

    await act(async () => {
      store.setViewport(180, 22);
    });

    await waitFor(() => {
      expect(store.getSnapshot().followTail).toBe(true);
    });
  });
});
