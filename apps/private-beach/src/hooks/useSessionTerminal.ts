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

type ConnectionListenerBundle = {
  connection: BrowserTransportConnection;
  connectionId: string;
  detachAll: () => void;
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
  const depsSignatureRef = useRef<string | null>(null);
  const reconnectTickSignatureRef = useRef<number>(0);
  const connectionSeqRef = useRef(0);
  const connectionListenersRef = useRef<ConnectionListenerBundle | null>(null);

  useEffect(() => {
    let cancelled = false;
    let reconnectTimer: number | null = null;
    const existingBundle = connectionListenersRef.current;
    const connectionId =
      existingBundle?.connectionId ?? `${connectionSeqRef.current++}`;
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

    const detachTransportListeners = (
      reason: string,
      bundleOverride?: ConnectionListenerBundle | null,
    ) => {
      const bundle = bundleOverride ?? connectionListenersRef.current;
      if (!bundle) {
        if (!bundleOverride && reason !== 'before-attach') {
          logEvent('detach-listeners:skip', { reason });
        }
        return;
      }
      if (connectionListenersRef.current === bundle) {
        connectionListenersRef.current = null;
      }
      try {
        bundle.detachAll();
      } catch (err) {
        console.warn('[terminal] detach listeners error', err);
      }
      logEvent('detach-listeners', { reason });
    };

    const closeCurrentConnection = (reason: string) => {
      const current = connectionRef.current;
      if (!current) {
        logEvent('close-current', { hasConnection: false, reason });
        return;
      }
      logEvent('close-current', { hasConnection: true, reason });
      try {
        current.close();
      } catch (err) {
        console.warn('[terminal] error closing previous connection', err);
      }
      connectionRef.current = null;
    };

    const performCleanup = (
      reason: string,
      options?: { closeConnection?: boolean; detachListeners?: boolean },
    ) => {
      const {
        closeConnection: shouldCloseConnection = true,
        detachListeners = shouldCloseConnection,
      } = options ?? {};
      cancelled = true;
      if (detachListeners) {
        detachTransportListeners(reason);
      }
      if (shouldCloseConnection) {
        closeCurrentConnection(reason);
      }
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      if (shouldCloseConnection) {
        setStatus('idle');
        setSecureSummary(null);
        setLatencyMs(null);
        lastHeartbeatRef.current = null;
      }
      logEvent('effect-cleanup', {
        reason,
        closeConnection: shouldCloseConnection,
        detachListeners,
      });
    };

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
    const depsSignature = JSON.stringify({
      sessionId: sessionId ?? null,
      privateBeachId: privateBeachId ?? null,
      managerUrl: trimmedManagerUrl,
      authToken: effectiveAuthToken,
      passcode: normalizedOverride.passcode,
      viewerTokenOverride: normalizedOverride.viewerToken,
      skipCredentialFetch: normalizedOverride.skipCredentialFetch,
    });
    const previousSignature = depsSignatureRef.current;
    const previousReconnectTick = reconnectTickSignatureRef.current;
    const hasExistingConnection = connectionRef.current !== null;
    const shouldReuseConnection =
      hasExistingConnection &&
      previousSignature === depsSignature &&
      previousReconnectTick === reconnectTick;

    depsSignatureRef.current = depsSignature;
    reconnectTickSignatureRef.current = reconnectTick;

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

    const attachTransportListeners = (connection: BrowserTransportConnection) => {
      detachTransportListeners('before-attach');
      const transportTarget = connection.transport;
      const detachFns: Array<() => void> = [];
      const bundle: ConnectionListenerBundle = {
        connection,
        connectionId,
        detachAll: () => {
          const removers = detachFns.splice(0);
          for (const fn of removers) {
            try {
              fn();
            } catch (err) {
              console.warn('[terminal] listener detach error', err);
            }
          }
        },
      };
      connectionListenersRef.current = bundle;

      const isCurrentConnection = () =>
        connectionListenersRef.current?.connection === connection &&
        connectionRef.current === connection;

      const openHandler = () => {
        if (!isCurrentConnection()) {
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
      detachFns.push(() =>
        transportTarget.removeEventListener('open', openHandler as EventListener),
      );

      const secureHandler = (event: Event) => {
        if (!isCurrentConnection()) {
          return;
        }
        const detail = (event as CustomEvent<SecureTransportSummary>).detail;
        setSecureSummary(detail);
      };
      transportTarget.addEventListener('secure', secureHandler as EventListener);
      detachFns.push(() =>
        transportTarget.removeEventListener('secure', secureHandler as EventListener),
      );

      const frameHandler = (event: Event) => {
        if (!isCurrentConnection()) {
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
      detachFns.push(() =>
        transportTarget.removeEventListener('frame', frameHandler as EventListener),
      );

      const closeHandler = () => {
        if (!isCurrentConnection()) {
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
        if (connectionRef.current === connection) {
          connectionRef.current = null;
        }
        detachTransportListeners('transport-close', bundle);
        setTransport(null);
        setConnecting(true);
        setError(null);
        setStatus('reconnecting');
        setSecureSummary(null);
        setLatencyMs(null);
        scheduleReconnect('transport-close');
      };
      transportTarget.addEventListener('close', closeHandler as EventListener);
      detachFns.push(() =>
        transportTarget.removeEventListener('close', closeHandler as EventListener),
      );

      const errorHandler = (event: Event) => {
        if (!isCurrentConnection()) {
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
      detachFns.push(() =>
        transportTarget.removeEventListener('error', errorHandler as EventListener),
      );

      const statusHandler = (event: Event) => {
        if (!isCurrentConnection()) {
          return;
        }
        const detail = (event as CustomEvent<string>).detail;
        if (detail?.startsWith('beach:status:')) {
          console.debug('[terminal] status signal', { sessionId, detail });
        }
      };
      transportTarget.addEventListener('status', statusHandler as EventListener);
      detachFns.push(() =>
        transportTarget.removeEventListener('status', statusHandler as EventListener),
      );

      const signalingCloseHandler = (event: Event) => {
        if (!isCurrentConnection()) {
          return;
        }
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
      detachFns.push(() =>
        connection.signaling.removeEventListener('close', signalingCloseHandler as EventListener),
      );

      const signalingErrorHandler = (event: Event) => {
        if (!isCurrentConnection()) {
          return;
        }
        const detail = (event as ErrorEvent).message ?? 'unknown';
        console.error('[terminal] signaling error', {
          sessionId,
          privateBeachId,
          message: detail,
        });
        logEvent('signaling-error', { message: detail });
      };
      connection.signaling.addEventListener('error', signalingErrorHandler as EventListener);
      detachFns.push(() =>
        connection.signaling.removeEventListener(
          'error',
          signalingErrorHandler as EventListener,
        ),
      );

      logEvent('attach-listeners', {
        remotePeerId: connection.remotePeerId ?? null,
      });
    };

    if (shouldReuseConnection) {
      logEvent('effect-start', {
        reused: true,
        wasConnected: wasConnectedRef.current,
        needsCredentialFetch,
        hasOverrideCredentials,
      });
      setConnecting(false);
      setError(null);
      if (status !== 'connected') {
        setStatus('connected');
      }
      const activeConnection = connectionRef.current;
      if (
        activeConnection &&
        (!connectionListenersRef.current ||
          connectionListenersRef.current.connection !== activeConnection)
      ) {
        attachTransportListeners(activeConnection);
        logEvent('reuse-attach-listeners', {});
      }
      if (activeConnection) {
        setTransport(activeConnection.transport);
      }
      return () =>
        performCleanup('reuse', { closeConnection: false, detachListeners: false });
    }

    detachTransportListeners('refresh-before-connect');
    closeCurrentConnection('refresh-before-connect');
    setTransport(null);
    store.reset();
    store.setFollowTail(true);
    setSecureSummary(null);
    setLatencyMs(null);
    lastHeartbeatRef.current = null;

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

        attachTransportListeners(connection);
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
