'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { fetchViewerCredential } from '../lib/api';
import {
  connectBrowserTransport,
  type BrowserTransportConnection,
} from '../../../beach-surfer/src/terminal/connect';
import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';
import type { TerminalTransport } from '../../../beach-surfer/src/transport/terminalTransport';
import type { SecureTransportSummary } from '../../../beach-surfer/src/transport/webrtc';
import type { HostFrame } from '../../../beach-surfer/src/protocol/types';

export type TerminalViewerStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';

export type TerminalViewerState = {
  store: TerminalGridStore | null;
  transport: TerminalTransport | null;
  connecting: boolean;
  error: string | null;
  status: TerminalViewerStatus;
  secureSummary: SecureTransportSummary | null;
  latencyMs: number | null;
};

export type SessionCredentialOverride = {
  passcode?: string | null;
  viewerToken?: string | null;
  authorizationToken?: string | null;
  skipCredentialFetch?: boolean;
};

export function useSessionTerminal(
  sessionId: string | null | undefined,
  privateBeachId: string | null | undefined,
  managerUrl: string,
  token: string | null,
  override?: SessionCredentialOverride,
): TerminalViewerState {
  const store = useMemo(() => {
    void sessionId;
    return new TerminalGridStore(80);
  }, [sessionId]);
  const [transport, setTransport] = useState<TerminalTransport | null>(null);
  const [connecting, setConnecting] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<TerminalViewerStatus>('idle');
  const [secureSummary, setSecureSummary] = useState<SecureTransportSummary | null>(null);
  const [latencyMs, setLatencyMs] = useState<number | null>(null);
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const lastHeartbeatRef = useRef<number | null>(null);
  const wasConnectedRef = useRef<boolean>(false);
  const [reconnectTick, setReconnectTick] = useState(0);
  const connectionSeqRef = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let reconnectTimer: number | null = null;
    const cleanupListeners: Array<() => void> = [];
    const connectionId = `${connectionSeqRef.current++}`;
    const logEvent = (event: string, extra: Record<string, unknown> = {}) => {
      if (typeof window === 'undefined') return;
      console.info('[terminal-conn]', {
        event,
        connectionId,
        sessionId,
        privateBeachId,
        managerUrl,
        tokenPresent: Boolean(token && token.trim().length > 0),
        reconnectTick,
        ...extra,
      });
    };

    const closeCurrentConnection = () => {
      const current = connectionRef.current;
      if (!current) {
        logEvent('close-current', { hasConnection: false });
        return;
      }
      logEvent('close-current', { hasConnection: true });
      try {
        current.close();
      } catch (err) {
        console.warn('[terminal] error closing previous connection', err);
      }
      connectionRef.current = null;
    };

    const performCleanup = (reason: string) => {
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
      setStatus('idle');
      setSecureSummary(null);
      setLatencyMs(null);
      lastHeartbeatRef.current = null;
      logEvent('effect-cleanup', { reason });
    };

    closeCurrentConnection();
    setTransport(null);
    store.reset();
    store.setFollowTail(true);
    setSecureSummary(null);
    setLatencyMs(null);
    lastHeartbeatRef.current = null;

    const normalizedOverride = {
      passcode: override?.passcode?.trim() ?? null,
      viewerToken: override?.viewerToken?.trim() ?? null,
      authorizationToken: override?.authorizationToken?.trim() ?? null,
      skipCredentialFetch: override?.skipCredentialFetch ?? false,
    };
    const hasOverrideCredentials =
      Boolean(normalizedOverride.passcode && normalizedOverride.passcode.length > 0) ||
      Boolean(normalizedOverride.viewerToken && normalizedOverride.viewerToken.length > 0);
    const trimmedManagerToken = token?.trim() ?? '';
    const effectiveAuthToken =
      normalizedOverride.authorizationToken && normalizedOverride.authorizationToken.length > 0
        ? normalizedOverride.authorizationToken
        : trimmedManagerToken;
    const needsCredentialFetch = !normalizedOverride.skipCredentialFetch && !hasOverrideCredentials;
    const trimmedManagerUrl = managerUrl.trim();

    if (!sessionId || trimmedManagerUrl.length === 0) {
      setConnecting(false);
      setError(null);
      setStatus('idle');
      wasConnectedRef.current = false;
      logEvent('idle-no-session-or-url', {
        hasSessionId: Boolean(sessionId),
        hasManagerUrl: trimmedManagerUrl.length > 0,
      });
      return () => performCleanup('no-session-or-url');
    }

    if (needsCredentialFetch) {
      if (!privateBeachId || effectiveAuthToken.length === 0) {
        setConnecting(false);
        if (effectiveAuthToken.length === 0) {
          setError(null);
        }
        setStatus('idle');
        wasConnectedRef.current = false;
        logEvent('idle-missing-credentials', {
          hasPrivateBeachId: Boolean(privateBeachId),
          hasAuthToken: effectiveAuthToken.length > 0,
        });
        return () => performCleanup('missing-credentials');
      }
    } else if (!hasOverrideCredentials) {
      setConnecting(false);
      setError('Missing session credentials');
      setStatus('idle');
      wasConnectedRef.current = false;
      logEvent('idle-missing-override-credentials', {});
      return () => performCleanup('missing-override-credentials');
    }

    setConnecting(true);
    setError(null);
    setStatus(wasConnectedRef.current ? 'reconnecting' : 'connecting');
    setSecureSummary(null);
    setLatencyMs(null);
    logEvent('effect-start', {
      wasConnected: wasConnectedRef.current,
      needsCredentialFetch,
      hasOverrideCredentials,
    });

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
        logEvent('schedule-reconnect', { source });
        setReconnectTick((tick) => tick + 1);
      }, 1_500);
    };

    (async () => {
      try {
        let effectivePasscode: string | undefined;
        let viewerTokenForTransport: string | undefined;

        if (hasOverrideCredentials) {
          if (normalizedOverride.passcode && normalizedOverride.passcode.length > 0) {
            effectivePasscode = normalizedOverride.passcode;
          }
          if (normalizedOverride.viewerToken && normalizedOverride.viewerToken.length > 0) {
            viewerTokenForTransport = normalizedOverride.viewerToken;
          }
        } else {
          logEvent('fetch-viewer-credential:start');
          const credential = await fetchViewerCredential(
            privateBeachId!,
            sessionId,
            effectiveAuthToken,
            trimmedManagerUrl,
          );
          if (cancelled) {
            return;
          }
          const credentialType = credential.credential_type?.toLowerCase();
          logEvent('fetch-viewer-credential:success', {
            credentialType: credentialType ?? 'unknown',
          });
          if (credentialType === 'viewer_token') {
            viewerTokenForTransport = credential.credential?.trim() || undefined;
            if (credential.passcode != null) {
              const candidate = String(credential.passcode).trim();
              if (candidate.length > 0) {
                effectivePasscode = candidate;
              }
            }
          } else if (credential.credential != null) {
            const candidate = String(credential.credential).trim();
            if (candidate.length > 0) {
              effectivePasscode = candidate;
            }
          }
        }
        if (!viewerTokenForTransport && (!effectivePasscode || effectivePasscode.length === 0)) {
          throw new Error('viewer passcode unavailable');
        }
        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl: trimmedManagerUrl,
          passcode: effectivePasscode && effectivePasscode.length > 0 ? effectivePasscode : undefined,
          viewerToken: viewerTokenForTransport,
          clientLabel: 'private-beach-dashboard',
          authorizationToken: effectiveAuthToken.length > 0 ? effectiveAuthToken : undefined,
        });
        if (cancelled) {
          connection.close();
          return;
        }
        connectionRef.current = connection;
        wasConnectedRef.current = true;
        setTransport(connection.transport);
        setConnecting(false);
        setError(null);
        setStatus('connected');
        setSecureSummary(connection.secure ?? null);
        setLatencyMs(null);
        lastHeartbeatRef.current = null;
        logEvent('connect-browser-transport:success', {
          remotePeerId: connection.remotePeerId ?? null,
          secureMode: connection.secure?.mode ?? 'plaintext',
        });

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
          logEvent('transport-open', {
            remotePeerId: connection.remotePeerId ?? null,
          });
        };
        transportTarget.addEventListener('open', openHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('open', openHandler as EventListener),
        );

        const secureHandler = (event: Event) => {
          if (cancelled) {
            return;
          }
          const detail = (event as CustomEvent<SecureTransportSummary>).detail;
          setSecureSummary(detail);
        };
        transportTarget.addEventListener('secure', secureHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('secure', secureHandler as EventListener),
        );

        const frameHandler = (event: Event) => {
          if (cancelled) {
            return;
          }
          const detail = (event as CustomEvent<HostFrame>).detail;
          if (detail?.type === 'heartbeat' && typeof detail.timestampMs === 'number') {
            lastHeartbeatRef.current = detail.timestampMs;
            const now = Date.now();
            const latency = Math.max(0, now - detail.timestampMs);
            setLatencyMs(latency);
          }
        };
        transportTarget.addEventListener('frame', frameHandler as EventListener);
        cleanupListeners.push(() =>
          transportTarget.removeEventListener('frame', frameHandler as EventListener),
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
          logEvent('transport-close', {
            remotePeerId: connection.remotePeerId ?? null,
          });
          setTransport(null);
          setConnecting(true);
          setError(null);
          setStatus('reconnecting');
          setSecureSummary(null);
          setLatencyMs(null);
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
          logEvent('transport-error', { message });
          setError(message);
          setStatus('error');
          setSecureSummary(null);
          setLatencyMs(null);
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
          logEvent('signaling-close', {
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
          logEvent('signaling-error', { message: detail });
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
        logEvent('connect-browser-transport:error', { message });
        setError(message);
        setConnecting(false);
        setStatus('error');
        setSecureSummary(null);
        setLatencyMs(null);
      }
    })();

    return () => performCleanup('dependency-change');
  }, [
    sessionId,
    privateBeachId,
    managerUrl,
    token,
    override?.passcode,
    override?.viewerToken,
    override?.authorizationToken,
    override?.skipCredentialFetch,
    store,
    reconnectTick,
  ]);

  return {
    store,
    transport,
    connecting,
    error,
    status,
    secureSummary,
    latencyMs,
  };
}
