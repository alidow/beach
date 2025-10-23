import { describe, expect, it } from 'vitest';
import { applyControllerPairingEvent } from '../useControllerPairingStreams';

describe('applyControllerPairingEvent', () => {
  it('adds assignments when an event is received', () => {
    const result = applyControllerPairingEvent([], {
      controllerId: 'controller-1',
      childId: 'child-1',
      action: 'added',
      pairing: {
        pairing_id: 'pair-1',
        controller_session_id: 'controller-1',
        child_session_id: 'child-1',
        update_cadence: 'fast',
        prompt_template: 'Drive with precision',
        transport_status: { transport: 'fast_path', latency_ms: 42, last_event_ms: 1234 },
        created_at_ms: 1111,
        updated_at_ms: 2222,
      },
    });

    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      controller_session_id: 'controller-1',
      child_session_id: 'child-1',
      update_cadence: 'fast',
    });
  });

  it('removes assignments on a removed event', () => {
    const current = [
      {
        pairing_id: 'pair-1',
        controller_session_id: 'controller-1',
        child_session_id: 'child-1',
        update_cadence: 'balanced',
        prompt_template: null,
        transport_status: { transport: 'fast_path' },
        created_at_ms: 0,
        updated_at_ms: 0,
      },
    ];

    const result = applyControllerPairingEvent(current, {
      controllerId: 'controller-1',
      childId: 'child-1',
      action: 'removed',
    });

    expect(result).toHaveLength(0);
  });
});
