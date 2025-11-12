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
  metadata: {
    createdAt: 100,
    updatedAt: 200,
    agentRelationships: {
      'rel-1': {
        id: 'rel-1',
        sourceId: 'first',
        targetId: 'second',
        sourceHandleId: 'source-right',
        targetHandleId: 'target-left',
        instructions: 'Monitor app',
        updateMode: 'poll',
        pollFrequency: 45,
      },
    },
    agentRelationshipOrder: ['rel-1'],
  },
};

describe('tile persistence helpers (rewrite-2)', () => {
	it('hydrates tile state with ordered entries and leaves interactive mode unset', () => {
		const state = layoutToTileState(SAMPLE_LAYOUT);
		expect(state.order).toEqual(['first', 'second']);
		expect(state.tiles.second.sessionMeta?.sessionId).toBe('sess-2');
		expect(state.interactiveId).toBeNull();
    expect(state.canvasViewport).toEqual({ zoom: 1.2, pan: { x: -12, y: 5 } });
    expect(state.relationshipOrder).toEqual(['rel-1']);
    expect(state.relationships['rel-1']).toMatchObject({
      sourceId: 'first',
      targetId: 'second',
      sourceHandleId: 'source-right',
      instructions: 'Monitor app',
      updateMode: 'poll',
      pollFrequency: 45,
      sourceSessionId: null,
      targetSessionId: 'sess-2',
      cadence: {
        idleSummary: false,
        allowChildPush: false,
        pollEnabled: true,
        pollFrequencySeconds: 45,
        pollRequireContentChange: false,
        pollQuietWindowSeconds: 0,
      },
    });
  });

  it('serializes tile state back into layout preserving viewport metadata', () => {
    const state = layoutToTileState(SAMPLE_LAYOUT);
    const serialized = tileStateToLayout(state, SAMPLE_LAYOUT);
    expect(serialized.tiles.second.position).toEqual({ x: 160, y: 96 });
    expect(serialized.tiles.second.metadata?.sessionMeta).toMatchObject({ sessionId: 'sess-2' });
    expect(serialized.metadata.createdAt).toBe(SAMPLE_LAYOUT.metadata.createdAt);
    expect(serialized.metadata.agentRelationships?.['rel-1']).toMatchObject({
      instructions: 'Monitor app',
      updateMode: 'poll',
      targetSessionId: 'sess-2',
      cadence: {
        idleSummary: false,
        allowChildPush: false,
        pollEnabled: true,
        pollFrequencySeconds: 45,
        pollRequireContentChange: false,
        pollQuietWindowSeconds: 0,
      },
    });
    expect(serialized.metadata.agentRelationshipOrder).toEqual(['rel-1']);
    expect(serialized.viewport).toEqual({ zoom: 1.2, pan: { x: -12, y: 5 } });
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
      relationships: {},
      relationshipOrder: [],
      activeId: null,
      resizing: {},
      interactiveId: null,
      viewport: {},
      canvasViewport: { zoom: 1, pan: { x: 0, y: 0 } },
    };
    const key = serializeTileStateKey(orphanState);
    expect(key).toContain('only');
    expect(key).toContain('relationships:none');
  });

  it('captures relationship signatures in the serialized key', () => {
    const state = layoutToTileState(SAMPLE_LAYOUT);
    const key = serializeTileStateKey(state);
    expect(key).toContain('rel-1:first:second');
    expect(key).toContain('viewport:1.200:-12.000:5.000');
  });
});
