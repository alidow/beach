'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { fetchViewerCredential } from '../lib/api';
import {
  connectBrowserTransport,
  type BrowserTransportConnection,
} from '../../../beach-surfer/src/terminal/connect';
import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalTransport } from '../../../beach-surfer/src/transport/terminalTransport';

export type TerminalViewerState = {
  store: TerminalGridStore | null;
  transport: TerminalTransport | null;
  connecting: boolean;
  error: string | null;
};

export function useSessionTerminal(
  sessionId: string | null | undefined,
  privateBeachId: string | null | undefined,
  managerUrl: string,
  token: string | null,
): TerminalViewerState {
  const store = useMemo(() => new TerminalGridStore(80), [sessionId]);
  const [transport, setTransport] = useState<TerminalTransport | null>(null);
  const [connecting, setConnecting] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const [reconnectTick, setReconnectTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let reconnectTimer: number | null = null;
    const cleanupListeners: Array<() => void> = [];

    const closeCurrentConnection = () => {
      const current = connectionRef.current;
      if (!current) {
        return;
      }
      try {
        current.close();
      } catch (err) {
        console.warn('[terminal] error closing previous connection', err);
      }
      connectionRef.current = null;
    };

    closeCurrentConnection();
    setTransport(null);
    store.reset();
    store.setFollowTail(true);

    const trimmedToken = token?.trim();
    if (!sessionId || !privateBeachId || !trimmedToken) {
      setConnecting(false);
      if (!trimmedToken) {
        setError(null);
      }
      return () => {
        cancelled = true;
        for (const fn of cleanupListeners) {
          try {
            fn();
          } catch (err) {
            console.warn('[terminal] cleanup error', err);
          }
        }
        if (reconnectTimer !== null) {
          window.clearTimeout(reconnectTimer);
        }
      };
    }

    setConnecting(true);
    setError(null);

    const scheduleReconnect = (source: string) => {
      if (cancelled) {
        return;
      }
      if (reconnectTimer !== null) {
        return;
      }
      reconnectTimer = window.setTimeout(() => {
        reconnectTimer = null;
        if (cancelled) {
          return;
        }
        console.info('[terminal] scheduling viewer reconnect', {
          sessionId,
          privateBeachId,
          source,
        });
        setReconnectTick((tick) => tick + 1);
      }, 1_500);
    };

    (async () => {
      try {
        const credential = await fetchViewerCredential(
          privateBeachId,
          sessionId,
          trimmedToken,
          managerUrl,
        );
        if (cancelled) {
          return;
        }
        const viewerToken =
          credential.credential_type === 'viewer_token' ? credential.credential : undefined;
        const effectivePasscode =
          credential.credential_type === 'viewer_token'
            ? credential.passcode ?? null
            : credential.credential;
        if (!effectivePasscode || effectivePasscode.trim().length === 0) {
          throw new Error('viewer passcode unavailable');
        }
        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl: managerUrl,
          passcode: effectivePasscode,
          viewerToken,
          clientLabel: 'private-beach-dashboard',
          authorizationToken: trimmedToken,
        });
        if (cancelled) {
          connection.close();
          return;
        }
        connectionRef.current = connection;
        setTransport(connection.transport);
        setConnecting(false);
        setError(null);

        const transportTarget = connection.transport;

        const openHandler = () => {
          if (cancelled) {
            return;
          }
          console.info('[terminal] data channel open', {
            sessionId,
            privateBeachId,
            remotePeerId: connection.remotePeerId ?? null,
          });
        };
        transportTarget.addEventListener('open', openHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('open', openHandler as EventListener),
        );

        const closeHandler = () => {
          if (cancelled) {
            return;
          }
          console.warn('[terminal] data channel closed', {
            sessionId,
            privateBeachId,
            remotePeerId: connection.remotePeerId ?? null,
          });
          setTransport(null);
          setConnecting(true);
          setError('Viewer disconnected');
          scheduleReconnect('transport-close');
        };
        transportTarget.addEventListener('close', closeHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('close', closeHandler as EventListener),
        );

        const errorHandler = (event: Event) => {
          if (cancelled) {
            return;
          }
          const err = (event as any).error;
          const message = err instanceof Error ? err.message : String(err ?? 'transport error');
          console.error('[terminal] data channel error', {
            sessionId,
            privateBeachId,
            message,
          });
          setError(message);
        };
        transportTarget.addEventListener('error', errorHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('error', errorHandler as EventListener),
        );

        const statusHandler = (event: Event) => {
          const detail = (event as CustomEvent<string>).detail;
          if (detail?.startsWith('beach:status:')) {
            console.debug('[terminal] status signal', { sessionId, detail });
          }
        };
        transportTarget.addEventListener('status', statusHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('status', statusHandler as EventListener),
        );

        const signalingCloseHandler = (event: Event) => {
          const detail = (event as CustomEvent<CloseEvent>).detail;
          console.warn('[terminal] signaling closed', {
            sessionId,
            privateBeachId,
            code: detail?.code ?? null,
            reason: detail?.reason ?? '',
            wasClean: detail?.wasClean ?? null,
          });
        };
        connection.signaling.addEventListener('close', signalingCloseHandler as EventListener);
        cleanupListeners.push(() =>
          connection.signaling.removeEventListener(
            'close',
            signalingCloseHandler as EventListener,
          ),
        );
        const signalingErrorHandler = (event: Event) => {
          const detail = (event as ErrorEvent).message ?? 'unknown';
          console.error('[terminal] signaling error', {
            sessionId,
            privateBeachId,
            message: detail,
          });
        };
        connection.signaling.addEventListener('error', signalingErrorHandler as EventListener);
        cleanupListeners.push(() =>
          connection.signaling.removeEventListener(
            'error',
            signalingErrorHandler as EventListener,
          ),
        );
      } catch (err) {
        if (cancelled) {
          return;
        }
        const message = err instanceof Error ? err.message : String(err);
        console.error('[terminal] viewer connect failed', {
          sessionId,
          managerUrl,
          error: message,
        });
        setError(message);
        setConnecting(false);
      }
    })();

    return () => {
      cancelled = true;
      for (const fn of cleanupListeners) {
        try {
          fn();
        } catch (err) {
          console.warn('[terminal] cleanup error', err);
        }
      }
      cleanupListeners.length = 0;
      closeCurrentConnection();
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
      }
    };
  }, [sessionId, privateBeachId, managerUrl, token, store, reconnectTick]);

  return {
    store,
    transport,
    connecting,
    error,
  };
}
