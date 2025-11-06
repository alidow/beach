import { describe, expect, it } from 'vitest';
import type { CanvasLayout } from '@/lib/api';
import { layoutToTileState, serializeTileStateKey, tileStateToLayout } from '../persistence';
import type { TileState } from '../types';

const BASE_LAYOUT: CanvasLayout = {
  version: 3,
  viewport: { zoom: 1, pan: { x: 4, y: 8 } },
  tiles: {
    alpha: {
      id: 'alpha',
      kind: 'application',
      position: { x: 32, y: 64 },
      size: { width: 320, height: 240 },
      zIndex: 2,
      metadata: {
        sessionMeta: {
          sessionId: 'session-alpha',
          title: 'Alpha',
          status: 'attached',
        },
        createdAt: 10,
        updatedAt: 20,
      },
    },
    beta: {
      id: 'beta',
      kind: 'application',
      position: { x: 0, y: 0 },
      size: { width: 200, height: 120 },
      zIndex: 1,
    },
  },
  agents: {},
  groups: {},
  controlAssignments: {},
  metadata: { createdAt: 1, updatedAt: 2 },
};

describe('tile persistence helpers', () => {
  it('hydrates tile state from a layout payload', () => {
    const state = layoutToTileState(BASE_LAYOUT);
    expect(state.order).toEqual(['beta', 'alpha']);
    expect(state.tiles.alpha.position).toEqual({ x: 32, y: 64 });
    expect(state.tiles.alpha.sessionMeta?.sessionId).toEqual('session-alpha');
    expect(state.tiles.beta.size.height).toBeGreaterThan(0);
  });

  it('serializes tile state back into a layout and preserves metadata', () => {
    const state = layoutToTileState(BASE_LAYOUT);
    const next = tileStateToLayout(state, BASE_LAYOUT);
    expect(next.tiles.alpha.position).toEqual({ x: 32, y: 64 });
    expect(next.tiles.alpha.metadata?.sessionMeta).toMatchObject({ sessionId: 'session-alpha' });
    expect(next.metadata.createdAt).toEqual(BASE_LAYOUT.metadata.createdAt);
    expect(next.metadata.updatedAt).toBeGreaterThanOrEqual(next.metadata.createdAt);
  });

  it('serializes orphaned tiles even when order is empty', () => {
    const orphanState: TileState = {
      tiles: {
        lone: {
          id: 'lone',
          nodeType: 'application',
          position: { x: 12, y: 16 },
          size: { width: 180, height: 140 },
          sessionMeta: null,
          createdAt: 1,
          updatedAt: 1,
        },
      },
      order: [],
      activeId: null,
      resizing: {},
    };
    const key = serializeTileStateKey(orphanState);
    expect(key).toContain('lone');
  });
});
