import { useEffect, useRef, useState } from 'react';
import { stateSseUrl } from '../lib/api';

type Cursor = { row: number; col: number } | null;

export type TerminalPreview = {
  lines: string[];
  cursor: Cursor;
  sequence: number;
  connecting: boolean;
  lastUpdatedAt?: number;
  error?: string | null;
};

type StateDiff = {
  sequence?: number;
  emitted_at?: string | number;
  payload?: {
    type?: string;
    lines?: unknown;
    cursor?: { row?: number; col?: number } | null;
    text?: unknown;
  };
};

function normalizeLines(lines: unknown): string[] {
  if (!Array.isArray(lines)) {
    return [];
  }
  return lines.map((line) => (typeof line === 'string' ? line : JSON.stringify(line ?? ''))).slice(0, 80);
}

function extractTerminalState(diff: StateDiff): Pick<TerminalPreview, 'lines' | 'cursor'> | null {
  const payload = diff.payload;
  if (!payload) return null;
  if (payload.type === 'terminal_full') {
    return {
      lines: normalizeLines(payload.lines),
      cursor: payload.cursor && typeof payload.cursor.row === 'number' && typeof payload.cursor.col === 'number'
        ? { row: payload.cursor.row, col: payload.cursor.col }
        : null,
    };
  }
  if (payload.type === 'terminal_text' && typeof payload.text === 'string') {
    return {
      lines: payload.text.split('\n').slice(0, 80),
      cursor: null,
    };
  }
  return null;
}

export function useSessionTerminal(sessionId: string | null | undefined, managerUrl: string, token: string | null): TerminalPreview {
  const [state, setState] = useState<TerminalPreview>({
    lines: [],
    cursor: null,
    sequence: 0,
    connecting: Boolean(sessionId),
    error: null,
  });
  const sourceRef = useRef<EventSource | null>(null);

  useEffect(() => {
    sourceRef.current?.close();
    if (!sessionId) {
      setState({ lines: [], cursor: null, sequence: 0, connecting: false, error: null });
      return () => {};
    }
    setState((prev) => ({ ...prev, connecting: true, error: null }));
    const url = stateSseUrl(sessionId, managerUrl, token || undefined);
    const es = new EventSource(url);
    sourceRef.current = es;

    const handleState = (msg: MessageEvent) => {
      try {
        const diff = JSON.parse(msg.data) as StateDiff;
        const snapshot = extractTerminalState(diff);
        if (!snapshot) {
          setState((prev) => ({ ...prev, connecting: false }));
          return;
        }
        setState((prev) => ({
          lines: snapshot.lines,
          cursor: snapshot.cursor ?? null,
          sequence: typeof diff.sequence === 'number' ? diff.sequence : prev.sequence,
          connecting: false,
          lastUpdatedAt: Date.now(),
          error: null,
        }));
      } catch (error) {
        setState((prev) => ({ ...prev, error: error instanceof Error ? error.message : String(error), connecting: false }));
      }
    };

    es.addEventListener('state', handleState);
    es.onerror = () => {
      setState((prev) => ({ ...prev, error: prev.error ?? 'Disconnected', connecting: true }));
    };

    return () => {
      es.removeEventListener('state', handleState);
      es.close();
    };
  }, [sessionId, managerUrl, token]);

  return state;
}
