import { describe, it, expect } from 'vitest';
import { buildAssignmentModel } from '../assignments';
import type { ControllerPairing, SessionSummary } from '../api';

function makeSession(overrides: Partial<SessionSummary>): SessionSummary {
  return {
    session_id: 's-0',
    private_beach_id: 'beach-1',
    harness_type: 'terminal',
    capabilities: [],
    location_hint: null,
    metadata: {},
    version: '1',
    harness_id: 'h-0',
    controller_token: null,
    controller_expires_at_ms: null,
    pending_actions: 0,
    pending_unacked: 0,
    last_health: null,
    ...overrides,
  };
}

function makePairing(overrides: Partial<ControllerPairing>): ControllerPairing {
  return {
    pairing_id: 'p-1',
    controller_session_id: 'agent-1',
    child_session_id: 'app-1',
    prompt_template: '',
    update_cadence: 'balanced',
    transport_status: { transport: 'fast_path' },
    created_at_ms: Date.now(),
    updated_at_ms: Date.now(),
    ...overrides,
  };
}

describe('buildAssignmentModel', () => {
  it('indexes assignments by agent and application and derives roles', () => {
    const sessions: SessionSummary[] = [
      makeSession({ session_id: 'agent-1', harness_type: 'controller' }),
      makeSession({ session_id: 'app-1', harness_type: 'worker' }),
      makeSession({ session_id: 'app-2', harness_type: 'worker' }),
    ];
    const pairings: ControllerPairing[] = [
      makePairing({ controller_session_id: 'agent-1', child_session_id: 'app-1' }),
      makePairing({ pairing_id: 'p-2', controller_session_id: 'agent-1', child_session_id: 'app-2' }),
    ];

    const model = buildAssignmentModel(sessions, pairings);

    expect(model.agents.map((s) => s.session_id)).toEqual(['agent-1']);
    expect(model.applications.map((s) => s.session_id)).toEqual(['app-1', 'app-2']);

    const agentEdges = model.assignmentsByAgent.get('agent-1');
    expect(agentEdges?.length).toBe(2);
    expect(agentEdges?.map((e) => e.pairing.child_session_id).sort()).toEqual(['app-1', 'app-2']);

    const app1Controllers = model.assignmentsByApplication.get('app-1');
    expect(app1Controllers?.map((p) => p.controller_session_id)).toEqual(['agent-1']);

    // Roles present for all session ids
    expect(model.roles.get('agent-1')).toBeDefined();
    expect(model.roles.get('app-1')).toBeDefined();
    expect(model.roles.get('app-2')).toBeDefined();
  });
});

