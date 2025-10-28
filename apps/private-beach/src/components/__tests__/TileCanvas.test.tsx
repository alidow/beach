import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import React from 'react';
import { describe, expect, it, vi } from 'vitest';
import TileCanvas from '../TileCanvas';
import type { ControllerPairing, SessionRole, SessionSummary } from '../../lib/api';
import type { AssignmentEdge } from '../../lib/assignments';

vi.mock('../AutoGrid', () => ({
  __esModule: true,
  default: ({ children }: { children: React.ReactNode }) => <div data-testid="auto-grid">{children}</div>,
}));

vi.mock('../SessionTerminalPreview', () => ({
  __esModule: true,
  SessionTerminalPreview: ({ sessionId }: { sessionId: string }) => (
    <div data-testid={`preview-${sessionId}`} />
  ),
}));

const mockViewer = {
  store: { kind: 'store' },
  transport: { kind: 'transport' },
  connecting: false,
  error: null,
  status: 'connected' as const,
  secureSummary: null,
  latencyMs: null,
};

vi.mock('../hooks/useSessionTerminal', () => ({
  __esModule: true,
  useSessionTerminal: vi.fn(() => mockViewer),
}));

function makeSession(overrides: Partial<SessionSummary>): SessionSummary {
  return {
    session_id: 'session-0',
    private_beach_id: 'beach-1',
    harness_type: 'terminal',
    capabilities: [],
    location_hint: null,
    metadata: {},
    version: '1',
    harness_id: 'harness-0',
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
    pairing_id: 'pair-1',
    controller_session_id: 'controller-1',
    child_session_id: 'child-1',
    prompt_template: '',
    update_cadence: 'balanced',
    transport_status: { transport: 'fast_path' },
    created_at_ms: Date.now(),
    updated_at_ms: Date.now(),
    ...overrides,
  };
}

describe('TileCanvas', () => {
  const agent = makeSession({ session_id: 'agent-1', harness_type: 'controller' });
  const application = makeSession({ session_id: 'app-1', harness_type: 'worker' });
  const assignment = makePairing({ controller_session_id: 'agent-1', child_session_id: 'app-1' });

  const roles = new Map<string, SessionRole>([
    ['agent-1', 'agent'],
    ['app-1', 'application'],
  ]);

  const assignmentsByAgent = new Map<string, AssignmentEdge[]>([
    ['agent-1', [{ pairing: assignment, application }]],
  ]);

  const assignmentsByApplication = new Map<string, ControllerPairing[]>([
    ['app-1', [assignment]],
  ]);

  it('shows assignment bar for agents and opens detail on click', async () => {
    const onOpenAssignment = vi.fn();
    render(
      <TileCanvas
        tiles={[agent]}
        onRemove={() => {}}
        onSelect={() => {}}
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        preset="grid2x2"
        savedLayout={[]}
        onLayoutPersist={() => {}}
        roles={roles}
        assignmentsByAgent={assignmentsByAgent}
        assignmentsByApplication={assignmentsByApplication}
        onRequestRoleChange={() => {}}
        onOpenAssignment={onOpenAssignment}
      />,
    );

    await screen.findByTestId('auto-grid');
    expect(screen.getByText('1 assignment')).toBeInTheDocument();
    fireEvent.click(screen.getByText('Show ▾'));
    await waitFor(() => expect(screen.getByText(/Hide/)).toBeInTheDocument());
    const assignmentButton = screen.getByText(application.session_id.slice(0, 8)).closest('button');
    expect(assignmentButton).not.toBeNull();
    fireEvent.click(assignmentButton!);
    expect(onOpenAssignment).toHaveBeenCalledWith(assignment);
  });

  it('displays controllers on application tiles', async () => {
  render(
    <TileCanvas
      tiles={[application]}
      onRemove={() => {}}
      onSelect={() => {}}
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        preset="grid2x2"
        savedLayout={[]}
        onLayoutPersist={() => {}}
        roles={roles}
        assignmentsByAgent={assignmentsByAgent}
        assignmentsByApplication={assignmentsByApplication}
        onRequestRoleChange={() => {}}
        onOpenAssignment={() => {}}
      />,
  );

  await screen.findByTestId('auto-grid');
  expect(screen.getByText(assignment.controller_session_id.slice(0, 6))).toBeInTheDocument();
  });

  it('invokes onRequestRoleChange when toggling role', async () => {
    const onRequestRoleChange = vi.fn();
    render(
      <TileCanvas
        tiles={[application]}
        onRemove={() => {}}
        onSelect={() => {}}
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        preset="grid2x2"
        savedLayout={[]}
        onLayoutPersist={() => {}}
        roles={roles}
        assignmentsByAgent={assignmentsByAgent}
        assignmentsByApplication={assignmentsByApplication}
        onRequestRoleChange={onRequestRoleChange}
        onOpenAssignment={() => {}}
      />,
    );

    await screen.findByTestId('auto-grid');
    fireEvent.click(screen.getByText('Set as Agent'));
    expect(onRequestRoleChange).toHaveBeenCalledWith(application, 'agent');
  });

  it('normalizes oversized saved layouts and persists the corrected width', async () => {
    const onLayoutPersist = vi.fn();
    render(
      <TileCanvas
        tiles={[application]}
        onRemove={() => {}}
        onSelect={() => {}}
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        preset="grid2x2"
        savedLayout={[
          {
            id: application.session_id,
            x: 0,
            y: 0,
            w: 12,
            h: 3,
            widthPx: 1200,
            zoom: 1,
            locked: false,
          },
        ]}
        onLayoutPersist={onLayoutPersist}
        roles={roles}
        assignmentsByAgent={assignmentsByAgent}
        assignmentsByApplication={assignmentsByApplication}
        onRequestRoleChange={() => {}}
        onOpenAssignment={() => {}}
      />,
    );

    await waitFor(() => expect(onLayoutPersist).toHaveBeenCalled());
    const snapshot = onLayoutPersist.mock.calls[0]?.[0];
    expect(Array.isArray(snapshot)).toBe(true);
    expect(snapshot[0]?.w).toBe(3);
    expect(snapshot[0]?.zoom).toBeLessThan(1);
  });
});
