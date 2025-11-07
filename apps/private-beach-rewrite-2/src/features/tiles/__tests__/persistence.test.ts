import { describe, expect, it } from 'vitest';
import type { CanvasLayout } from '@/lib/api';
import { layoutToTileState, serializeTileStateKey, tileStateToLayout } from '../persistence';
import type { TileState } from '../types';

const SAMPLE_LAYOUT: CanvasLayout = {
  version: 3,
  viewport: { zoom: 1.2, pan: { x: -12, y: 5 } },
  tiles: {
    second: {
      id: 'second',
      kind: 'application',
      position: { x: 160, y: 96 },
      size: { width: 320, height: 240 },
      zIndex: 2,
      metadata: {
        sessionMeta: {
          sessionId: 'sess-2',
          title: 'Second',
          status: 'attached',
        },
      },
    },
    first: {
      id: 'first',
      kind: 'application',
      position: { x: 0, y: 0 },
      size: { width: 280, height: 180 },
      zIndex: 1,
    },
  },
  agents: {},
  groups: {},
  controlAssignments: {},
  metadata: { createdAt: 100, updatedAt: 200 },
};

describe('tile persistence helpers (rewrite-2)', () => {
  it('hydrates tile state with ordered entries and interactive id', () => {
    const state = layoutToTileState(SAMPLE_LAYOUT);
    expect(state.order).toEqual(['first', 'second']);
    expect(state.tiles.second.sessionMeta?.sessionId).toBe('sess-2');
    expect(state.interactiveId).toBe('first');
  });

  it('serializes tile state back into layout preserving viewport metadata', () => {
    const state = layoutToTileState(SAMPLE_LAYOUT);
    const serialized = tileStateToLayout(state, SAMPLE_LAYOUT);
    expect(serialized.tiles.second.position).toEqual({ x: 160, y: 96 });
    expect(serialized.tiles.second.metadata?.sessionMeta).toMatchObject({ sessionId: 'sess-2' });
    expect(serialized.metadata.createdAt).toBe(SAMPLE_LAYOUT.metadata.createdAt);
  });

  it('includes orphan tiles in the serialized key', () => {
    const orphanState: TileState = {
      tiles: {
        only: {
          id: 'only',
          nodeType: 'application',
          position: { x: 10, y: 10 },
          size: { width: 100, height: 100 },
          sessionMeta: null,
          createdAt: 1,
          updatedAt: 1,
        },
      },
      order: [],
      activeId: null,
      resizing: {},
      interactiveId: null,
      viewport: {},
    };
    const key = serializeTileStateKey(orphanState);
    expect(key).toContain('only');
  });
});
