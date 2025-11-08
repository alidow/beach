'use client';

import { useEffect, useMemo, useRef, useState } from 'react';
import { viewerConnectionService } from '../../../private-beach/src/controllers/viewerConnectionService';
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
    if (!sessionId || !managerUrl || !authToken) {
      setViewer(IDLE_VIEWER_STATE);
      return;
    }
    const disconnect = viewerConnectionService.connectTile(
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
        if (typeof window !== 'undefined') {
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
            // eslint-disable-next-line no-console
            console.info('[rewrite-terminal][connection]', JSON.stringify(payload));
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
    return () => {
      disconnect();
    };
  }, [authToken, credentialOverride, managerUrl, overrideSignature, privateBeachId, sessionId, tileId, traceId]);

  return viewer;
}
