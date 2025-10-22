import { render, screen, fireEvent } from '@testing-library/react';
import React from 'react';
import { describe, expect, it, vi } from 'vitest';
import TileCanvas from '../TileCanvas';
import type { ControllerPairing, SessionSummary } from '../../lib/api';

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

function createDataTransfer(): DataTransfer {
  const store: Record<string, string> = {};
  return {
    dropEffect: 'copy',
    effectAllowed: 'all',
    files: [],
    items: [],
    types: [],
    setData: (type: string, value: string) => {
      store[type] = value;
    },
    getData: (type: string) => store[type] ?? '',
    clearData: () => {
      Object.keys(store).forEach((key) => delete store[key]);
    },
    setDragImage: () => {},
  } as unknown as DataTransfer;
}

describe('TileCanvas', () => {
  const controller = makeSession({ session_id: 'controller-1', harness_type: 'controller' });
  const child = makeSession({ session_id: 'child-1', harness_type: 'worker' });

  it('calls onBeginPairing when a controller is dropped onto a child tile', async () => {
    const onBeginPairing = vi.fn();
    const onEditPairing = vi.fn();
    const pairLabel = new RegExp(`Pair controller ${controller.session_id.slice(0, 8)}`, 'i');

    render(
      <TileCanvas
        tiles={[controller, child]}
        onRemove={() => {}}
        onSelect={() => {}}
        managerToken="token"
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        refresh={async () => {}}
        pairings={[]}
        onBeginPairing={onBeginPairing}
        onEditPairing={onEditPairing}
      />,
    );

    const pairButton = await screen.findByLabelText(pairLabel);
    const dataTransfer = createDataTransfer();

    fireEvent.dragStart(pairButton, { dataTransfer });
    const overlay = await screen.findByTestId('pairing-drop-overlay-child-1');
    expect(overlay).toHaveTextContent(/drop controller here/i);
    fireEvent.dragEnter(overlay, { dataTransfer });
    expect(overlay).toHaveTextContent(/release to pair/i);
    fireEvent.dragOver(overlay, { dataTransfer });
    fireEvent.drop(overlay, { dataTransfer });
    fireEvent.dragEnd(pairButton, { dataTransfer });

    expect(onBeginPairing).toHaveBeenCalledWith('controller-1', 'child-1');
    expect(onEditPairing).not.toHaveBeenCalled();
  });

  it('routes drop events to onEditPairing when a pairing already exists', async () => {
    const onBeginPairing = vi.fn();
    const onEditPairing = vi.fn();
    const pairLabel = new RegExp(`Pair controller ${controller.session_id.slice(0, 8)}`, 'i');

    render(
      <TileCanvas
        tiles={[controller, child]}
        onRemove={() => {}}
        onSelect={() => {}}
        managerToken="token"
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        refresh={async () => {}}
        pairings={[makePairing({ pairing_id: 'pair-existing' })]}
        onBeginPairing={onBeginPairing}
        onEditPairing={onEditPairing}
      />,
    );

    const pairButton = await screen.findByLabelText(pairLabel);
    const dataTransfer = createDataTransfer();

    fireEvent.dragStart(pairButton, { dataTransfer });
    const overlay = await screen.findByTestId('pairing-drop-overlay-child-1');
    fireEvent.dragEnter(overlay, { dataTransfer });
    fireEvent.dragOver(overlay, { dataTransfer });
    fireEvent.drop(overlay, { dataTransfer });
    fireEvent.dragEnd(pairButton, { dataTransfer });

    expect(onBeginPairing).not.toHaveBeenCalled();
    expect(onEditPairing).toHaveBeenCalledWith(
      expect.objectContaining({ controller_session_id: 'controller-1', child_session_id: 'child-1' }),
    );
  });

  it('renders status badges for controller and child tiles', async () => {
    render(
      <TileCanvas
        tiles={[controller, child]}
        onRemove={() => {}}
        onSelect={() => {}}
        managerToken="token"
        viewerToken="viewer"
        managerUrl="http://localhost:8080"
        refresh={async () => {}}
        pairings={[makePairing({ update_cadence: 'balanced', transport_status: { transport: 'fast_path' } })]}
        onBeginPairing={() => {}}
        onEditPairing={() => {}}
      />,
    );

    const fastPathBadges = await screen.findAllByText('Fast-path');
    expect(fastPathBadges).toHaveLength(2);
    const cadenceBadges = await screen.findAllByText('Balanced');
    expect(cadenceBadges.length).toBeGreaterThan(0);
    expect(await screen.findByText('→ child-1')).toBeInTheDocument();
    expect(await screen.findByText(/← controll/i)).toBeInTheDocument();
  });
});
