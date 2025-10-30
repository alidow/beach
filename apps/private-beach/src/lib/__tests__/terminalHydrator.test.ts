import { describe, expect, it } from 'vitest';
import { TerminalGridStore } from '../../../../beach-surfer/src/terminal/gridStore';
import styledFixture from '../../tests/fixtures/pong-lhs-terminal-styled.json';
import {
  buildViewerStateFromTerminalDiff,
  extractTerminalStateDiff,
  hydrateTerminalStoreFromDiff,
  makeDiffFromLines,
  type TerminalStateDiff,
} from '../terminalHydrator';

describe('terminalHydrator', () => {
  it('hydrates styled lines with style definitions', () => {
    const diff = styledFixture as TerminalStateDiff;
    const store = new TerminalGridStore(4);
    const success = hydrateTerminalStoreFromDiff(store, diff);
    expect(success).toBe(true);
    const snapshot = store.getSnapshot();
    const row0 = snapshot.rows[0];
    if (row0?.kind !== 'loaded') {
      throw new Error('row0 not loaded');
    }
    expect(row0.cells[0]?.char).toBe('+');
    expect(row0.cells[0]?.styleId).toBe(1);
    const style = snapshot.styles.get(1);
    expect(style?.fg).toBe(37788927);
  });

  it('builds viewer state from plain line fallback', () => {
    const diff = makeDiffFromLines(['hi'], 5);
    const viewer = buildViewerStateFromTerminalDiff(diff);
    expect(viewer).toBeTruthy();
    const snapshot = viewer?.store?.getSnapshot();
    const row = snapshot?.rows[0];
    if (!row || row.kind !== 'loaded') {
      throw new Error('row missing');
    }
    expect(row.cells[0]?.char).toBe('h');
    expect(row.cells[1]?.char).toBe('i');
  });

  it('extracts nested terminal diff from metadata', () => {
    const metadata = {
      viewer: {
        cached: {
          sequence: 9,
          payload: styledFixture.payload,
        },
      },
    };
    const extracted = extractTerminalStateDiff(metadata);
    expect(extracted).toBeTruthy();
    expect(extracted?.sequence).toBe(9);
    expect(extracted?.payload.type).toBe('terminal_full');
  });
});
