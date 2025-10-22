import { test, expect } from '@playwright/test';
import { applyControllerPairingEvent } from '../src/hooks/useControllerPairingStreams';
import type { ControllerPairing } from '../src/lib/api';

test('applyControllerPairingEvent merges SSE pairing updates', () => {
  const initial: ControllerPairing[] = [];
  const added = applyControllerPairingEvent(initial, {
    controllerId: 'controller-1',
    childId: 'child-1',
    action: 'added',
    pairing: {
      pairing_id: 'pair-1',
      controller_session_id: 'controller-1',
      child_session_id: 'child-1',
      update_cadence: 'balanced',
      prompt_template: 'Assist beachgoer',
      transport_status: { transport: 'pending' },
      created_at_ms: 1000,
      updated_at_ms: 1000,
    },
  });
  expect(added).toHaveLength(1);
  expect(added[0]).toMatchObject({
    controller_session_id: 'controller-1',
    child_session_id: 'child-1',
    update_cadence: 'balanced',
    transport_status: { transport: 'pending' },
  });

  const updated = applyControllerPairingEvent(added, {
    controllerId: 'controller-1',
    childId: 'child-1',
    action: 'updated',
    pairing: {
      pairing_id: 'pair-1',
      controller_session_id: 'controller-1',
      child_session_id: 'child-1',
      update_cadence: 'fast',
      prompt_template: 'Assist beachgoer',
      transport_status: { transport: 'fast_path', latency_ms: 25 },
      created_at_ms: 1000,
      updated_at_ms: 1500,
    },
  });
  expect(updated).toHaveLength(1);
  expect(updated[0]).toMatchObject({
    update_cadence: 'fast',
    transport_status: { transport: 'fast_path', latency_ms: 25 },
  });

  const removed = applyControllerPairingEvent(updated, {
    controllerId: 'controller-1',
    childId: 'child-1',
    action: 'removed',
  });
  expect(removed).toHaveLength(0);
});
