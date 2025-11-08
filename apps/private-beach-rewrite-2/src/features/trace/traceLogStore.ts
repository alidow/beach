import { useSyncExternalStore } from 'react';

export type TraceLogEntry = {
  id: string;
  timestamp: number;
  level: 'info' | 'warn' | 'error';
  source: string;
  message: string;
  detail?: Record<string, unknown> | null;
};

type Listener = () => void;

const MAX_LOGS_PER_TRACE = 200;
const EMPTY_LOGS: TraceLogEntry[] = [];
const buffers = new Map<string, TraceLogEntry[]>();
const listeners = new Set<Listener>();

function emit() {
  listeners.forEach((listener) => {
    try {
      listener();
    } catch (error) {
      console.error('[trace-log-store] listener error', error);
    }
  });
}

export function recordTraceLog(
  traceId: string | null | undefined,
  entry: Omit<TraceLogEntry, 'id' | 'timestamp'> & { id?: string; timestamp?: number },
) {
  if (!traceId) {
    return;
  }
  const normalized: TraceLogEntry = {
    id: entry.id ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    timestamp: entry.timestamp ?? Date.now(),
    level: entry.level ?? 'info',
    source: entry.source ?? 'dashboard',
    message: entry.message ?? '',
    detail: entry.detail ?? null,
  };
  const existing = buffers.get(traceId) ?? [];
  const next = existing.concat(normalized);
  if (next.length > MAX_LOGS_PER_TRACE) {
    next.splice(0, next.length - MAX_LOGS_PER_TRACE);
  }
  buffers.set(traceId, next);
  emit();
}

export function clearTraceLogs(traceId: string | null | undefined) {
  if (!traceId) {
    return;
  }
  if (buffers.delete(traceId)) {
    emit();
  }
}

export function useTraceLogs(traceId: string | null | undefined): TraceLogEntry[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => {
      if (!traceId) {
        return EMPTY_LOGS;
      }
      return buffers.get(traceId) ?? EMPTY_LOGS;
    },
    () => EMPTY_LOGS,
  );
}
