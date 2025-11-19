import { describe, it, expect, vi, beforeEach, afterAll, beforeAll } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { SessionSummary } from '@private-beach/shared-api';
import { attachByCode, issueControllerHandshake } from '@/lib/api';
import { sendControlMessage } from '@/lib/road';

const originalFetch = global.fetch;
const mockFetch = vi.fn(async (input: RequestInfo | URL) => {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  if (url.includes('/api/manager-token')) {
    return new Response(JSON.stringify({ token: 'manager-token' }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  }
  if (url.includes('/wasm/argon2.wasm')) {
    return new Response(new Uint8Array([0, 1, 2]).buffer, { status: 200 });
  }
  return new Response('', { status: 200 });
});
global.fetch = mockFetch as typeof global.fetch;

vi.mock('@clerk/nextjs', () => ({
  useAuth: () => ({
    isLoaded: true,
    isSignedIn: true,
  }),
}));

vi.mock('../hooks/useSessionConnection', () => ({
  useSessionConnection: () => ({
    store: null,
    transport: null,
    connecting: false,
    error: null,
    status: 'connected',
    secureSummary: null,
    latencyMs: null,
  }),
}));

vi.mock('./SessionViewer', () => ({
  SessionViewer: () => <div data-testid="session-viewer" />,
}));

vi.mock('../../../../beach-surfer/src/transport/crypto/argon2.ts', () => ({
  deriveArgon2id: vi.fn(async () => new Uint8Array(32)),
}));

const refreshMock = vi.fn().mockResolvedValue('manager-token');

vi.mock('../hooks/useManagerToken', () => ({
  useManagerToken: () => ({
    token: 'manager-token',
    loading: false,
    error: null,
    isLoaded: true,
    isSignedIn: true,
    refresh: refreshMock,
  }),
  buildManagerUrl: () => 'http://manager.test',
  buildRoadUrl: () => 'http://road.test',
}));

vi.mock('@/lib/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/api')>();
  return {
    ...actual,
    attachByCode: vi.fn(),
    fetchSessionStateSnapshot: vi.fn().mockResolvedValue(null),
    updateSessionRoleById: vi.fn().mockResolvedValue(undefined),
    issueControllerHandshake: vi.fn(),
  };
});

vi.mock('@/lib/road', () => ({
  sendControlMessage: vi.fn().mockResolvedValue({ control_id: 'ctrl-1' }),
}));

const mockedAttachByCode = vi.mocked(attachByCode);
const mockedIssueControllerHandshake = vi.mocked(issueControllerHandshake);
const mockedSendControlMessage = vi.mocked(sendControlMessage);

type ApplicationTileType = typeof import('../ApplicationTile')['ApplicationTile'];
let ApplicationTile: ApplicationTileType;

function setHandshakeRenewOverride(value: number) {
  (globalThis as Record<string, unknown>).__BEACH_HANDSHAKE_RENEW_MIN_MS__ = value;
  if (typeof window !== 'undefined') {
    (window as Record<string, unknown>).__BEACH_HANDSHAKE_RENEW_MIN_MS__ = value;
  }
}

beforeAll(async () => {
  setHandshakeRenewOverride(10);
  ({ ApplicationTile } = await import('../ApplicationTile'));
});

describe('ApplicationTile controller handshakes', () => {
  beforeEach(() => {
    setHandshakeRenewOverride(10);
    vi.clearAllMocks();
    refreshMock.mockClear();
    mockedSendControlMessage.mockResolvedValue({ control_id: 'ctrl-1' });
    mockFetch.mockClear();
  });

  afterAll(() => {
    global.fetch = originalFetch;
    delete (globalThis as Record<string, unknown>).__BEACH_HANDSHAKE_RENEW_MIN_MS__;
    if (typeof window !== 'undefined') {
      delete (window as Record<string, unknown>).__BEACH_HANDSHAKE_RENEW_MIN_MS__;
    }
  });

  it('does not resend control message when renewing the controller lease', async () => {
    const sessionSummary: SessionSummary = {
      session_id: 'sess-123',
      metadata: {},
      harness_type: null,
      pending_actions: 0,
    };
    mockedAttachByCode.mockResolvedValue({ session: sessionSummary });
    mockedIssueControllerHandshake.mockResolvedValue({
      lease_expires_at_ms: Date.now() + 5_010,
    });

    render(
      <ApplicationTile
        tileId="tile-1"
        privateBeachId="pb-1"
        managerUrl="http://manager.test"
        roadUrl="http://road.test"
        sessionMeta={null}
        onSessionMetaChange={() => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText(/Session ID/i), { target: { value: 'sess-123' } });
    fireEvent.change(screen.getByLabelText(/Passcode/i), { target: { value: 'ABCDEF' } });
    const submitButton = screen.getByRole('button', { name: /connect/i });
    const formElement = submitButton.closest('form');
    expect(formElement).not.toBeNull();
    fireEvent.submit(formElement!);

    await waitFor(() => {
      expect(mockedAttachByCode).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(mockedIssueControllerHandshake).toHaveBeenCalledTimes(1);
    });
    expect(mockedSendControlMessage).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(mockedIssueControllerHandshake).toHaveBeenCalledTimes(2);
    });
    expect(mockedSendControlMessage).toHaveBeenCalledTimes(1);
  });

  it('shows a friendly message when the controller account is missing', async () => {
    const sessionSummary: SessionSummary = {
      session_id: 'sess-err',
      metadata: {},
      harness_type: null,
      pending_actions: 0,
    };
    mockedAttachByCode.mockResolvedValue({ session: sessionSummary });
    const err = new Error('account missing');
    (err as Record<string, unknown>).errorCode = 'account_missing';
    mockedIssueControllerHandshake.mockRejectedValue(err);

    render(
      <ApplicationTile
        tileId="tile-err"
        privateBeachId="pb-err"
        managerUrl="http://manager.test"
        roadUrl="http://road.test"
        sessionMeta={null}
        onSessionMetaChange={() => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText(/Session ID/i), { target: { value: 'sess-err' } });
    fireEvent.change(screen.getByLabelText(/Passcode/i), { target: { value: 'ABCDEF' } });
    const submitButton = screen.getByRole('button', { name: /connect/i });
    fireEvent.submit(submitButton.closest('form')!);

    await waitFor(() => {
      expect(mockedAttachByCode).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(screen.getByText(/controller account is missing/i)).toBeInTheDocument();
    });
  });
});
