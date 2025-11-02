import { vi } from 'vitest';

vi.mock('../../controllers/viewerConnectionService', () => {
  const connectTile = vi.fn((_tileId: string, _input: unknown, subscriber: (snapshot: any) => void) => {
    subscriber({
      store: null,
      transport: null,
      connecting: false,
      error: null,
      status: 'connected',
      secureSummary: null,
      latencyMs: null,
    });
    return () => {};
  });
  return {
    viewerConnectionService: {
      connectTile,
      disconnectTile: vi.fn(),
      getTileMetrics: vi.fn(() => ({
        started: 0,
        completed: 0,
        retries: 0,
        failures: 0,
        disposed: 0,
      })),
      resetMetrics: vi.fn(),
    },
  };
});

import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import React from 'react';
import { describe, expect, it } from 'vitest';
import TileCanvas from '../TileCanvas';
import type { ControllerPairing, SessionRole, SessionSummary } from '../../lib/api';
import type { AssignmentEdge } from '../../lib/assignments';
import type { TerminalViewerState } from '../../hooks/terminalViewerTypes';
import { sessionTileController } from '../../controllers/sessionTileController';

vi.mock('../AutoGrid', () => ({
  __esModule: true,
  default: ({ children }: { children: React.ReactNode }) => <div data-testid="auto-grid">{children}</div>,
}));

vi.mock('../SessionTerminalPreview', () => {
  const React = require('react');
  const { useEffect } = React as typeof import('react');
  return {
    __esModule: true,
    SessionTerminalPreview: (props: any) => {
      const {
        sessionId,
        viewer,
        onPreviewMeasurementsChange,
        onPreviewStatusChange,
      } = props;

      useEffect(() => {
        const status =
          viewer.status === 'connected'
            ? viewer.latencyMs != null
              ? 'ready'
              : 'initializing'
            : viewer.status === 'error'
              ? 'error'
              : 'connecting';
        onPreviewStatusChange?.(status);
        if (viewer.latencyMs != null) {
          onPreviewMeasurementsChange?.(sessionId, {
            scale: 1,
            targetWidth: 420,
            targetHeight: 320,
            rawWidth: 420,
            rawHeight: 320,
            hostRows: 24,
            hostCols: 80,
            measurementVersion: 1,
          });
        }
      }, [sessionId, viewer.latencyMs, viewer.status, onPreviewMeasurementsChange, onPreviewStatusChange]);
      return (
        <div data-testid={`preview-${sessionId}`}>
          <span data-testid={`status-${sessionId}`}>{viewer.status}</span>
          <span data-testid={`latency-${sessionId}`}>{viewer.latencyMs ?? 'none'}</span>
          <span data-testid={`error-${sessionId}`}>{viewer.error ?? 'none'}</span>
        </div>
      );
    },
  };
});

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

const connectedViewer: TerminalViewerState = {
  store: null,
  transport: null,
  connecting: false,
  error: null,
  status: 'connected',
  secureSummary: null,
  latencyMs: 12,
};

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

  beforeEach(() => {
    sessionTileController.hydrate({
      layout: null,
      sessions: [],
      agents: [],
      privateBeachId: null,
      managerUrl: '',
      managerToken: null,
    });
  });

  function renderCanvas(overrides: Partial<React.ComponentProps<typeof TileCanvas>> = {}) {
    const baseProps: React.ComponentProps<typeof TileCanvas> = {
      tiles: [application],
      onRemove: () => {},
      onSelect: () => {},
      viewerToken: 'viewer',
      managerUrl: 'http://localhost:8080',
      preset: 'grid2x2',
      savedLayout: [],
      onLayoutPersist: () => {},
      roles,
      assignmentsByAgent,
      assignmentsByApplication,
      onRequestRoleChange: () => {},
      onOpenAssignment: () => {},
      ...overrides,
    };
    const tilesForOverride = overrides.tiles ?? baseProps.tiles;
    const viewerOverrides = overrides.viewerOverrides ?? Object.fromEntries(
      tilesForOverride.map((session) => [session.session_id, connectedViewer]),
    );
    return render(<TileCanvas {...baseProps} {...overrides} viewerOverrides={viewerOverrides} />);
  }

  it('shows assignment bar for agents and opens detail on click', async () => {
    const onOpenAssignment = vi.fn();
    renderCanvas({ tiles: [agent], onOpenAssignment });

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
  renderCanvas({ tiles: [application] });

  await screen.findByTestId('auto-grid');
  expect(screen.getByText(assignment.controller_session_id.slice(0, 6))).toBeInTheDocument();
  });

  it('invokes onRequestRoleChange when toggling role', async () => {
    const onRequestRoleChange = vi.fn();
    renderCanvas({ tiles: [application], onRequestRoleChange });

    await screen.findByTestId('auto-grid');
    fireEvent.click(screen.getByText('Set as Agent'));
    expect(onRequestRoleChange).toHaveBeenCalledWith(application, 'agent');
  });

  it('persists the controller grid layout after hydration', async () => {
    const onLayoutPersist = vi.fn();
    renderCanvas({
      tiles: [application],
      savedLayout: [
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
      ],
      onLayoutPersist,
    });

    try {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 250));
      });
      expect(onLayoutPersist).toHaveBeenCalled();
      const snapshot = onLayoutPersist.mock.calls[0]?.[0];
      expect(Array.isArray(snapshot)).toBe(true);
      const item = snapshot.find((entry: any) => entry.id === application.session_id);
      expect(item).toBeDefined();
      expect(item.id).toBe(application.session_id);
      expect(item.x).toBe(0);
      expect(item.y).toBe(0);
      expect(item.w).toBeGreaterThan(0);
      expect(item.h).toBeGreaterThan(0);
      expect(item.gridCols).toBeGreaterThan(0);
      expect(item.rowHeightPx).toBeGreaterThan(0);
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 500));
      });
      expect(onLayoutPersist.mock.calls.length).toBeLessThanOrEqual(2);
    } finally {
      // no-op
    }
  });

  it('updates lock button state when controller view state changes', async () => {
    renderCanvas();
    await screen.findByTestId('auto-grid');

    const lockButton = screen.getByTitle('Lock tile and resize host PTY');
    expect(lockButton).toHaveAttribute('aria-pressed', 'false');

    act(() => {
      sessionTileController.updateTileViewState('app-1', 'test', {
        locked: true,
        zoom: 1,
      });
    });

    await waitFor(() => {
      expect(lockButton).toHaveAttribute('aria-pressed', 'true');
      expect(lockButton).toHaveAttribute('title', 'Unlock tile (resize without touching host)');
    });
  });


  it('keeps the toolbar visible when controller pins the tile toolbar', async () => {
    renderCanvas();
    await screen.findByTestId('auto-grid');

    const nameLabel = screen.getByText(application.session_id.slice(0, 8));
    const toolbar = nameLabel.closest('button')?.parentElement as HTMLElement;
    expect(toolbar.className).toContain('opacity-0');

    act(() => {
      sessionTileController.setTileToolbarPinned('app-1', true);
    });

    await waitFor(() => {
      expect(toolbar.className).toContain('opacity-100');
    });
  });

  it('renders updated zoom label when controller view state changes', async () => {
    renderCanvas();
    await screen.findByTestId('auto-grid');

    await waitFor(() => {
      expect(screen.getByText(/Zoom/)).toBeInTheDocument();
    });

    act(() => {
      sessionTileController.updateTileViewState('app-1', 'test', {
        zoom: 0.5,
      });
    });

    await waitFor(() => {
      expect(screen.getByText('Zoom 50%')).toBeInTheDocument();
    });
  });

  it('shows expanded overlay when Expand tile is pressed', async () => {
    renderCanvas();
    await screen.findByTestId('auto-grid');

    const expandButton = screen.getByTitle('Expand tile');
    fireEvent.click(expandButton);

    await waitFor(() => {
      expect(screen.getByText('Expanded view active…')).toBeInTheDocument();
    });
  });

  it('throttles layout persistence when the controller applies snapshots', async () => {
    const onLayoutPersist = vi.fn();
    renderCanvas({ onLayoutPersist });
    await screen.findByTestId('auto-grid');

    await waitFor(() => expect(onLayoutPersist).toHaveBeenCalled());
    onLayoutPersist.mockClear();

    const snapshot = {
      tiles: {
        'app-1': {
          layout: { x: 0, y: 0, w: 32, h: 28 },
        },
      },
      gridCols: 128,
      rowHeightPx: 12,
    };

    act(() => {
      sessionTileController.applyGridSnapshot('test-initial', snapshot);
    });

    expect(onLayoutPersist).not.toHaveBeenCalled();

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
    });

    expect(onLayoutPersist).toHaveBeenCalledTimes(1);
    const firstPersist = onLayoutPersist.mock.calls[0][0];
    expect(firstPersist).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: 'app-1', x: 0, y: 0 })]),
    );

    act(() => {
      sessionTileController.applyGridSnapshot('test-second', {
        tiles: {
          'app-1': {
            layout: { x: 16, y: 4, w: 32, h: 28 },
          },
        },
        gridCols: 128,
        rowHeightPx: 12,
      });
      sessionTileController.applyGridSnapshot('test-third', {
        tiles: {
          'app-1': {
            layout: { x: 20, y: 6, w: 32, h: 28 },
          },
        },
        gridCols: 128,
        rowHeightPx: 12,
      });
    });
    const intermediateSnapshot = sessionTileController.getGridLayoutSnapshot();
    expect(intermediateSnapshot.tiles['app-1']?.layout.x).toBe(20);
    expect(intermediateSnapshot.tiles['app-1']?.layout.y).toBe(6);

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
    });

    expect(onLayoutPersist).toHaveBeenCalledTimes(2);
    const secondPersist = onLayoutPersist.mock.calls[1][0];
    expect(secondPersist).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: 'app-1', x: 20, y: 6 })]),
    );
  });
});
