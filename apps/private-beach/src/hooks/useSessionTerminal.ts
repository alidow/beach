import { useEffect, useRef, useState } from 'react';
import { fetchViewerCredential } from '../lib/api';
import { connectBrowserTransport, type BrowserTransportConnection } from '../../../beach-surfer/src/terminal/connect';
import type { HostFrame } from '../../../beach-surfer/src/protocol/types';
import { TerminalGridStore } from '../../../beach-surfer/src/terminal/gridStore';

type Cursor = { row: number; col: number } | null;

export type TerminalPreview = {
  lines: string[];
  cursor: Cursor;
  sequence: number;
  connecting: boolean;
  lastUpdatedAt?: number;
  error?: string | null;
  latencyMs?: number | null;
};

function trimLine(line: string): string {
  let end = line.length;
  while (end > 0 && line[end - 1] === ' ') {
    end -= 1;
  }
  return line.slice(0, end);
}

export function useSessionTerminal(
  sessionId: string | null | undefined,
  privateBeachId: string | null | undefined,
  managerUrl: string,
  token: string | null,
): TerminalPreview {
  const [state, setState] = useState<TerminalPreview>({
    lines: [],
    cursor: null,
    sequence: 0,
    connecting: Boolean(sessionId),
    error: null,
    latencyMs: null,
  });
  const connectionRef = useRef<BrowserTransportConnection | null>(null);
  const storeRef = useRef<TerminalGridStore | null>(null);
  const lastSeqRef = useRef<number>(0);
  const lastLatencyRef = useRef<number | null>(null);

  useEffect(() => {
    connectionRef.current?.close();
    connectionRef.current = null;
    storeRef.current = null;

    if (!sessionId || !privateBeachId || !token || token.trim().length === 0) {
      setState((prev) => ({
        lines: [],
        cursor: null,
        sequence: 0,
        connecting: false,
        lastUpdatedAt: prev.lastUpdatedAt,
        error: prev.error ?? null,
        latencyMs: null,
      }));
      return () => {};
    }

    let canceled = false;
    const cleanups: Array<() => void> = [];
    const trimmedToken = token.trim();
    const store = new TerminalGridStore(80);
    storeRef.current = store;
    lastSeqRef.current = 0;
    lastLatencyRef.current = null;

    const updateFromStore = (latencyMs: number | null = null) => {
      const snapshot = store.getSnapshot();
      const visible = snapshot.visibleRows(80);
      const lines = visible.map((row) => {
        if (row.kind !== 'loaded') {
          return '';
        }
        const text = row.cells.map((cell) => cell.char).join('');
        return trimLine(text);
      });
      let cursor: Cursor = null;
      if (snapshot.cursorRow != null && snapshot.cursorCol != null) {
        const idx = visible.findIndex((row) => row.kind === 'loaded' && row.absolute === snapshot.cursorRow);
        if (idx >= 0) {
          cursor = { row: idx, col: snapshot.cursorCol ?? 0 };
        }
      }
      const effectiveLatency = latencyMs ?? lastLatencyRef.current ?? null;
      lastLatencyRef.current = effectiveLatency;
      setState({
        lines,
        cursor,
        sequence: lastSeqRef.current,
        connecting: false,
        lastUpdatedAt: Date.now(),
        error: null,
        latencyMs: effectiveLatency,
      });
    };

    const handleHostFrame = (frame: HostFrame) => {
      if (canceled) {
        return;
      }
      switch (frame.type) {
        case 'hello': {
          store.reset();
          store.setCursorSupport(Boolean(frame.features & 1));
          break;
        }
        case 'grid': {
          store.setBaseRow(frame.baseRow);
          store.setGridSize(frame.historyRows, frame.cols);
          const historyEnd = frame.baseRow + frame.historyRows;
          const viewportRows = Math.max(1, frame.viewportRows ?? 40);
          const viewportTop = Math.max(frame.baseRow, historyEnd - viewportRows);
          store.setViewport(viewportTop, viewportRows);
          store.setFollowTail(true);
          break;
        }
        case 'snapshot':
        case 'delta':
        case 'history_backfill': {
          const authoritative = frame.type !== 'delta';
          store.applyUpdates(frame.updates, {
            authoritative,
            origin: frame.type,
            cursor: frame.cursor ?? null,
          });
          if (frame.type === 'snapshot' && !frame.hasMore) {
            store.setFollowTail(true);
          }
          if (frame.type === 'history_backfill' && !frame.more) {
            store.setFollowTail(true);
          }
          if (frame.type === 'snapshot' || frame.type === 'delta') {
            lastSeqRef.current = frame.watermark;
          }
          break;
        }
        case 'cursor': {
          store.applyCursorFrame(frame.cursor);
          break;
        }
        case 'heartbeat': {
          const latency = Math.max(0, Date.now() - frame.timestampMs);
          updateFromStore(latency);
          return;
        }
        case 'snapshot_complete':
        case 'input_ack':
          return;
        case 'shutdown': {
          setState((prev) => ({
            ...prev,
            connecting: false,
            error: prev.error ?? 'Viewer disconnected',
            latencyMs: prev.latencyMs ?? null,
          }));
          return;
        }
      }

      updateFromStore();
    };

    const connect = async () => {
      try {
        setState((prev) => ({ ...prev, connecting: true, error: null }));
        const credential = await fetchViewerCredential(privateBeachId, sessionId, trimmedToken, managerUrl);
        if (canceled) {
          return;
        }
        const connection = await connectBrowserTransport({
          sessionId,
          baseUrl: managerUrl,
          passcode: credential.credential,
          clientLabel: 'manager-viewer',
        });
        if (canceled) {
          connection.close();
          return;
        }
        connectionRef.current = connection;
        const transport = connection.transport;
        const frameListener = (event: Event) => handleHostFrame((event as CustomEvent<HostFrame>).detail);
        transport.addEventListener('frame', frameListener as EventListener);
        cleanups.push(() => transport.removeEventListener('frame', frameListener as EventListener));

        const errorListener = (event: Event) => {
          if (canceled) return;
          const err = (event as any).error ?? new Error('transport error');
          setState((prev) => ({
            ...prev,
            connecting: false,
            error: err instanceof Error ? err.message : String(err),
          }));
        };
        transport.addEventListener('error', errorListener as EventListener);
        cleanups.push(() => transport.removeEventListener('error', errorListener as EventListener));

        const closeListener = () => {
          if (canceled) return;
          setState((prev) => ({
            ...prev,
            connecting: false,
            error: prev.error ?? 'Viewer disconnected',
          }));
        };
        transport.addEventListener('close', closeListener, { once: true });
        cleanups.push(() => transport.removeEventListener('close', closeListener as EventListener));

        const statusListener = (event: Event) => {
          const detail = (event as CustomEvent<string>).detail;
          if (detail === 'beach:status:approval_granted') {
            setState((prev) => ({ ...prev, connecting: false }));
          }
        };
        transport.addEventListener('status', statusListener as EventListener);
        cleanups.push(() => transport.removeEventListener('status', statusListener as EventListener));
      } catch (error) {
        if (canceled) {
          return;
        }
        const message = error instanceof Error ? error.message : String(error);
        console.error('[terminal] viewer connect failed', {
          sessionId,
          managerUrl,
          error: message,
        });
        setState((prev) => ({
          ...prev,
          connecting: false,
          error: message,
        }));
      }
    };

    connect();

    return () => {
      canceled = true;
      for (const fn of cleanups) {
        try {
          fn();
        } catch (error) {
          console.warn('[terminal] error during cleanup', error);
        }
      }
      connectionRef.current?.close();
      connectionRef.current = null;
      storeRef.current = null;
      lastLatencyRef.current = null;
    };
  }, [sessionId, privateBeachId, managerUrl, token]);

  return state;
}
