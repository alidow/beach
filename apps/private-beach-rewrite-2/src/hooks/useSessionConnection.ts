'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { viewerConnectionService } from '../../../private-beach/src/controllers/viewerConnectionService';
import { logConnectionEvent } from '@/features/logging/beachConnectionLogger';
import { recordTraceLog } from '@/features/trace/traceLogStore';
import type {
  SessionCredentialOverride,
  TerminalViewerState,
} from '../../../private-beach/src/hooks/terminalViewerTypes';

const IDLE_VIEWER_STATE: TerminalViewerState = {
  store: null,
  transport: null,
  connecting: false,
  error: null,
  status: 'idle',
  secureSummary: null,
  latencyMs: null,
};

function isTerminalTraceEnabled(): boolean {
  if (typeof globalThis !== 'undefined' && (globalThis as Record<string, any>).__BEACH_TILE_TRACE) {
    return true;
  }
  if (typeof process !== 'undefined' && process.env?.NEXT_PUBLIC_PRIVATE_BEACH_TERMINAL_TRACE === '1') {
    return true;
  }
  return false;
}

type UseSessionConnectionParams = {
  tileId: string;
  sessionId?: string | null;
  privateBeachId?: string | null;
  managerUrl?: string;
  authToken?: string | null;
  credentialOverride?: SessionCredentialOverride | null;
  traceContext?: { traceId?: string | null };
};

export function useSessionConnection({
  tileId,
  sessionId,
  privateBeachId,
  managerUrl,
  authToken,
  credentialOverride,
  traceContext,
}: UseSessionConnectionParams) {
  const [viewer, setViewer] = useState<TerminalViewerState>(IDLE_VIEWER_STATE);
  const lastLogKeyRef = useRef<string | null>(null);
  const lastViewerStatusRef = useRef<TerminalViewerStatus>('idle');
  const connectedAtRef = useRef<number | null>(null);

  const overrideSignature = useMemo(() => {
    if (!credentialOverride) {
      return 'none';
    }
    const passcode = credentialOverride.passcode?.trim() ?? '';
    const viewerToken = credentialOverride.viewerToken?.trim() ?? '';
    const auth = credentialOverride.authorizationToken?.trim() ?? '';
    const skip = credentialOverride.skipCredentialFetch ? '1' : '0';
    return [passcode, viewerToken, auth, skip].join('|');
  }, [credentialOverride]);

  const normalizedTraceId = traceContext?.traceId?.trim();
  const traceId = normalizedTraceId && normalizedTraceId.length > 0 ? normalizedTraceId : null;

  useEffect(() => {
    connectedAtRef.current = null;
    lastViewerStatusRef.current = 'idle';
  }, [sessionId]);

  useEffect(() => {
    let disconnect: (() => void) | null = null;

    if (!sessionId || !managerUrl || !authToken) {
      setViewer(IDLE_VIEWER_STATE);
      connectedAtRef.current = null;
      lastViewerStatusRef.current = 'idle';
    } else {
      disconnect = viewerConnectionService.connectTile(
        tileId,
        {
          sessionId,
          privateBeachId: privateBeachId ?? null,
          managerUrl,
          authToken,
          override: credentialOverride ?? undefined,
          traceId,
        },
        (snapshot) => {
          if (sessionId) {
            const logContext = {
              tileId,
              sessionId,
              privateBeachId: privateBeachId ?? null,
              managerUrl: managerUrl ?? null,
            };
            const previousStatus = lastViewerStatusRef.current;
            if (previousStatus === 'connected' && snapshot.status !== 'connected') {
              const duration = connectedAtRef.current ? Math.max(0, Date.now() - connectedAtRef.current) : null;
              connectedAtRef.current = null;
              logConnectionEvent(
                'fast-path:disconnect',
                logContext,
                {
                  durationMs: duration,
                  nextStatus: snapshot.status,
                  reason: snapshot.error ?? null,
                },
                'warn',
              );
            }
            if (snapshot.status !== previousStatus) {
              switch (snapshot.status) {
                case 'connecting':
                  logConnectionEvent('fast-path:attempt', logContext, {
                    reconnect: previousStatus === 'reconnecting' || previousStatus === 'error',
                  });
                  break;
                case 'connected':
                  connectedAtRef.current = Date.now();
                  logConnectionEvent(
                    previousStatus === 'reconnecting' ? 'fast-path:reconnect-success' : 'fast-path:success',
                    logContext,
                    {
                      latencyMs: snapshot.latencyMs ?? null,
                      secureMode: snapshot.secureSummary?.mode ?? null,
                    },
                  );
                  break;
                case 'reconnecting':
                  logConnectionEvent(
                    'fast-path:reconnect-start',
                    logContext,
                    { reason: snapshot.error ?? null },
                    'warn',
                  );
                  break;
                case 'error':
                  logConnectionEvent(
                    previousStatus === 'reconnecting' ? 'fast-path:reconnect-error' : 'fast-path:error',
                    logContext,
                    { error: snapshot.error ?? null },
                    'error',
                  );
                  break;
                case 'idle':
                  logConnectionEvent('fast-path:idle', logContext);
                  break;
                default:
                  break;
              }
              lastViewerStatusRef.current = snapshot.status;
            }
          } else {
            connectedAtRef.current = null;
            lastViewerStatusRef.current = snapshot.status;
          }
          if (typeof window !== 'undefined') {
            const traceEnabled = isTerminalTraceEnabled();
            if (!traceEnabled && !traceId) {
              setViewer(snapshot);
              return;
            }
            let keyRows: string | number = 'no-store';
            let gridSummary: { rows: number; viewportHeight: number; baseRow: number } | null = null;
            try {
              const gridSnapshot = snapshot.store?.getSnapshot();
              if (gridSnapshot) {
                keyRows = gridSnapshot.rows.length;
                gridSummary = {
                  rows: gridSnapshot.rows.length,
                  viewportHeight: gridSnapshot.viewportHeight,
                  baseRow: gridSnapshot.baseRow,
                };
              }
            } catch (error) {
              keyRows = `grid-error:${String(error)}`;
            }
            const nextKey = [
              snapshot.status,
              snapshot.connecting ? '1' : '0',
              snapshot.transport ? 'transport' : 'no-transport',
              snapshot.transportVersion ?? 0,
              keyRows,
            ].join('|');
            if (lastLogKeyRef.current !== nextKey) {
              lastLogKeyRef.current = nextKey;
              const payload = {
                tileId,
                trace_id: traceId ?? null,
                status: snapshot.status,
                connecting: snapshot.connecting,
                transport: Boolean(snapshot.transport),
                transportVersion: snapshot.transportVersion ?? 0,
                store: Boolean(snapshot.store),
                grid: gridSummary,
                latencyMs: snapshot.latencyMs ?? null,
                secureMode: snapshot.secureSummary?.mode ?? null,
                error: snapshot.error ?? null,
              };
              if (traceEnabled) {
                // eslint-disable-next-line no-console
                console.info('[rewrite-terminal][connection]', JSON.stringify(payload));
              }
              if (traceId) {
                recordTraceLog(traceId, {
                  source: 'viewer',
                  level: snapshot.error ? 'warn' : 'info',
                  message: `Viewer connection ${snapshot.status}`,
                  detail: payload,
                });
              }
            }
          }
          setViewer(snapshot);
        },
      );
    }

    return () => {
      if (disconnect) {
        disconnect();
      }
    };
  }, [authToken, credentialOverride, managerUrl, overrideSignature, privateBeachId, sessionId, tileId, traceId]);

  return viewer;
}
