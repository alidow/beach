import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import type { HostFrame } from '../../../../beach-surfer/src/protocol/types';
import type {
  BrowserTransportConnection,
  ConnectBrowserTransportOptions,
} from '../../../../beach-surfer/src/terminal/connect';
import { useSessionTerminal } from '../useSessionTerminal';

vi.mock('../lib/api', () => ({
  fetchViewerCredential: vi.fn(),
}));

const connectBrowserTransportMock = vi.hoisted(() =>
  vi.fn<[ConnectBrowserTransportOptions], Promise<BrowserTransportConnection>>(),
);

vi.mock('../../../../beach-surfer/src/terminal/connect', () => ({
  connectBrowserTransport: connectBrowserTransportMock,
}));

class FakeEventTarget extends EventTarget {
  send(): void {
    // no-op for tests
  }

  emit<TDetail>(type: string, detail?: TDetail): void {
    let event: Event;
    if (typeof detail === 'undefined') {
      event = new Event(type);
    } else if (type === 'error') {
      event = new Event(type);
      Object.assign(event, { error: detail });
    } else {
      event = new CustomEvent(type, { detail } as CustomEventInit);
    }
    this.dispatchEvent(event);
  }

  close(): void {
    this.emit('close');
  }
}

describe('useSessionTerminal', () => {
  beforeEach(() => {
    connectBrowserTransportMock.mockReset();
  });

  afterEach(() => {
    vi.clearAllTimers();
  });

  it('keeps transport listeners active when the connection is reused across rerenders', async () => {
    const fakeTransport = new FakeEventTarget();
    const fakeSignaling = new FakeEventTarget();
    const fakeConnection: BrowserTransportConnection = {
      transport: fakeTransport as unknown as BrowserTransportConnection['transport'],
      signaling: fakeSignaling as unknown as BrowserTransportConnection['signaling'],
      remotePeerId: 'peer-1',
      secure: null,
      close: vi.fn(),
    };
    connectBrowserTransportMock.mockResolvedValue(fakeConnection);

    const initialProps = {
      sessionId: 'session-1',
      privateBeachId: 'pb-1',
      managerUrl: 'https://manager.local',
      token: 'token-value',
      override: { viewerToken: 'viewer-1' },
    };

    const dateNowSpy = vi.spyOn(Date, 'now');
    try {
      const { result, rerender } = renderHook(
        (props) =>
          useSessionTerminal(
            props.sessionId,
            props.privateBeachId,
            props.managerUrl,
            props.token,
            props.override,
          ),
        {
          initialProps,
        },
      );

      await waitFor(() => {
        expect(result.current.status).toBe('connected');
      });
      expect(result.current.transport).toBe(fakeTransport);

      const heartbeat = (timestampMs: number, seq: number, nowMs: number) => {
        const frame: HostFrame = { type: 'heartbeat', timestampMs, seq };
        dateNowSpy.mockReturnValue(nowMs);
        act(() => {
          fakeTransport.emit('frame', frame);
        });
      };

      heartbeat(10_000, 1, 20_000);
      await waitFor(() => {
        expect(result.current.latencyMs).toBe(10_000);
      });

      rerender({
        ...initialProps,
        token: 'token-value ', // trailing space ensures dependency change but identical signature
      });

      await waitFor(() => {
        expect(result.current.status).toBe('connected');
        expect(result.current.transport).toBe(fakeTransport);
      });

      heartbeat(10_000, 2, 25_000);
      await waitFor(() => {
        expect(result.current.latencyMs).toBe(15_000);
      });

      rerender({
        ...initialProps,
        token: 'token-value  ',
      });

      await waitFor(() => {
        expect(result.current.status).toBe('connected');
        expect(result.current.transport).toBe(fakeTransport);
      });

      heartbeat(10_000, 3, 30_000);
      await waitFor(() => {
        expect(result.current.latencyMs).toBe(20_000);
      });

      expect(connectBrowserTransportMock).toHaveBeenCalledTimes(3);
    } finally {
      dateNowSpy.mockRestore();
    }
  });
});
