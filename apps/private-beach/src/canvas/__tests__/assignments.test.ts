import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Mock } from 'vitest';
import {
  applyOptimisticAssignments,
  removeOptimisticAssignment,
  applyAssignmentResults,
  summarizeAssignmentFailures,
  extractSuccessfulPairings,
  createAssignmentsBatch,
} from '../assignments';
import type { CanvasLayout } from '../types';
import type { ControllerPairing } from '../../lib/api';
import { batchControllerAssignments, createControllerPairing } from '../../lib/api';

vi.mock('../../lib/api', () => ({
  batchControllerAssignments: vi.fn(),
  createControllerPairing: vi.fn(),
}));

const mockBatch = batchControllerAssignments as unknown as Mock;
const mockCreatePairing = createControllerPairing as unknown as Mock;
const originalFetch = global.fetch;

function baseLayout(): CanvasLayout {
  return {
    version: 3,
    tiles: {},
    groups: {},
    agents: {},
    controlAssignments: {},
    viewport: { zoom: 1, pan: { x: 0, y: 0 } },
    metadata: { createdAt: 0, updatedAt: 0 },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  global.fetch = originalFetch;
});

describe('assignment state helpers', () => {
  it('tracks optimistic assignment keys and removes them on demand', () => {
    const layout = baseLayout();
    const assigned = applyOptimisticAssignments(layout, 'agent-1', { type: 'tile', id: 'tile-7' });
    expect(assigned.controlAssignments['agent-1|tile|tile-7']).toEqual({
      controllerId: 'agent-1',
      targetType: 'tile',
      targetId: 'tile-7',
    });

    const removed = removeOptimisticAssignment(assigned, 'agent-1', { type: 'tile', id: 'tile-7' });
    expect(removed.controlAssignments['agent-1|tile|tile-7']).toBeUndefined();
  });

  it('keeps optimistic assignment on full success and drops it on failure', () => {
    const layout = applyOptimisticAssignments(baseLayout(), 'agent-2', { type: 'group', id: 'group-9' });
    const pending = { controllerId: 'agent-2', target: { type: 'group' as const, id: 'group-9' } };

    const successResponse = {
      results: [{ controllerId: 'agent-2', childId: 'tile-1', ok: true }],
    };
    const persisted = applyAssignmentResults(layout, pending, successResponse);
    expect(persisted.controlAssignments['agent-2|group|group-9']).toBeDefined();

    const failureResponse = {
      results: [{ controllerId: 'agent-2', childId: 'tile-1', ok: false }],
    };
    const rolledBack = applyAssignmentResults(layout, pending, failureResponse);
    expect(rolledBack.controlAssignments['agent-2|group|group-9']).toBeUndefined();
  });

  it('summarizes failures and extracts successful pairings', () => {
    const pairing: ControllerPairing = {
      controller_session_id: 'agent-3',
      child_session_id: 'tile-1',
      created_at: 'now',
      updated_at: 'now',
      private_beach_id: 'pb',
      prompt_template: null,
      update_cadence: 'balanced',
    };
    const response = {
      results: [
        { controllerId: 'agent-3', childId: 'tile-1', ok: true, pairing },
        { controllerId: 'agent-3', childId: 'tile-2', ok: false, error: 'denied' },
      ],
    };

    expect(summarizeAssignmentFailures(response)).toContain('tile-2');
    expect(extractSuccessfulPairings(response)).toEqual([pairing]);
  });
});

describe('createAssignmentsBatch', () => {
  it('prefers manager batch endpoint when private beach id is provided', async () => {
    mockBatch.mockResolvedValue([
      {
        controller_session_id: 'agent-5',
        child_session_id: 'tile-10',
        ok: true,
        error: null,
        pairing: null,
      },
    ]);

    const result = await createAssignmentsBatch(
      [{ controllerId: 'agent-5', childIds: ['tile-10'], promptTemplate: null }],
      'token',
      'https://manager.local',
      { privateBeachId: 'pb-1' },
    );

    expect(mockBatch).toHaveBeenCalledWith(
      'pb-1',
      [
        {
          controller_session_id: 'agent-5',
          child_session_id: 'tile-10',
          prompt_template: null,
          update_cadence: 'balanced',
        },
      ],
      'token',
      'https://manager.local',
    );
    expect(mockCreatePairing).not.toHaveBeenCalled();
    expect(result.results).toHaveLength(1);
    expect(result.results[0]).toMatchObject({ controllerId: 'agent-5', ok: true });
  });

  it('falls back to per-child pairing when batch endpoints fail', async () => {
    mockBatch.mockRejectedValue(new Error('offline'));
    mockCreatePairing.mockResolvedValue({
      controller_session_id: 'agent-6',
      child_session_id: 'tile-20',
      private_beach_id: 'pb-2',
      prompt_template: 'hi',
      update_cadence: 'balanced',
      created_at: 'now',
      updated_at: 'now',
    });
    const mockedFetch = vi.fn().mockResolvedValue({
      ok: false,
      json: async () => ({}),
    });
    global.fetch = mockedFetch as unknown as typeof global.fetch;

    const result = await createAssignmentsBatch(
      [{ controllerId: 'agent-6', childIds: ['tile-20'], promptTemplate: 'hi' }],
      'token',
      'https://manager.local',
      { privateBeachId: 'pb-2' },
    );

    expect(mockedFetch).toHaveBeenCalledOnce();
    expect(mockCreatePairing).toHaveBeenCalledWith(
      'agent-6',
      {
        child_session_id: 'tile-20',
        prompt_template: 'hi',
        update_cadence: 'balanced',
      },
      'token',
      'https://manager.local',
    );
    expect(result.results).toMatchObject([
      {
        controllerId: 'agent-6',
        childId: 'tile-20',
        ok: true,
        pairing: { controller_session_id: 'agent-6', child_session_id: 'tile-20' },
      },
    ]);
  });
});
