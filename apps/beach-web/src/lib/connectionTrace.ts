const TRACE_NAMESPACE = '[beach-trace][connect]';
const TRACE_HISTORY_KEY = '__BEACH_TRACE_HISTORY';
const TRACE_ACTIVE_KEY = '__BEACH_TRACE_ACTIVE';
const TRACE_CURRENT_KEY = '__BEACH_TRACE_CURRENT';
const TRACE_LAST_KEY = '__BEACH_TRACE_LAST';
const TRACE_HISTORY_LIMIT = 10;

export type ConnectionTraceOutcome = 'success' | 'error' | 'cancelled';

export interface ConnectionTraceContext {
  sessionId?: string;
  baseUrl?: string;
}

export interface ConnectionTraceEvent {
  name: string;
  elapsedMs: number;
  data: Record<string, unknown>;
}

export interface ConnectionTraceSnapshot {
  context: ConnectionTraceContext;
  outcome: ConnectionTraceOutcome;
  startedAt: number;
  finishedAt: number;
  events: ConnectionTraceEvent[];
}

const activeTraces = new Set<ConnectionTrace>();

export class ConnectionTrace {
  private readonly enabled: boolean;
  private readonly startedAt: number;
  private readonly context: ConnectionTraceContext;
  private readonly events: ConnectionTraceEvent[] = [];
  private closed = false;
  private outcome: ConnectionTraceOutcome | null = null;

  constructor(context: ConnectionTraceContext = {}) {
    this.enabled = isTraceEnabled();
    this.startedAt = now();
    this.context = context;
    if (this.enabled) {
      this.emit('start', {});
    }
  }

  mark(name: string, extra: Record<string, unknown> = {}): void {
    if (!this.enabled || this.closed) {
      return;
    }
    this.emit(name, extra);
  }

  finish(outcome: ConnectionTraceOutcome, extra: Record<string, unknown> = {}): void {
    if (!this.enabled || this.closed) {
      return;
    }
    this.closed = true;
    this.outcome = outcome;
    const finishedAt = now();
    this.emit('complete', { outcome, ...extra });
    const snapshot = this.snapshot(outcome, finishedAt);
    recordSnapshot(snapshot);
    unregisterActiveTrace(this);
  }

  isEnabled(): boolean {
    return this.enabled;
  }

  getEvents(): ConnectionTraceEvent[] {
    return this.events.map((event) => ({
      name: event.name,
      elapsedMs: event.elapsedMs,
      data: { ...event.data },
    }));
  }

  private emit(name: string, extra: Record<string, unknown>): void {
    const elapsed = Number((now() - this.startedAt).toFixed(2));
    const payload = { ...extra };
    this.events.push({ name, elapsedMs: elapsed, data: payload });
    // eslint-disable-next-line no-console
    console.debug(TRACE_NAMESPACE, name, {
      ...this.context,
      elapsed_ms: elapsed,
      ...payload,
    });
  }

  private snapshot(outcome: ConnectionTraceOutcome, finishedAt: number): ConnectionTraceSnapshot {
    return {
      context: { ...this.context },
      outcome,
      startedAt: this.startedAt,
      finishedAt,
      events: this.getEvents(),
    };
  }
}

export function createConnectionTrace(context: ConnectionTraceContext = {}): ConnectionTrace | null {
  const trace = new ConnectionTrace(context);
  if (!trace.isEnabled()) {
    return null;
  }
  registerActiveTrace(trace);
  return trace;
}

function registerActiveTrace(trace: ConnectionTrace): void {
  activeTraces.add(trace);
  if (typeof globalThis === 'undefined') {
    return;
  }
  const host = globalThis as any;
  host[TRACE_ACTIVE_KEY] = activeTraces;
  host[TRACE_CURRENT_KEY] = trace;
}

function unregisterActiveTrace(trace: ConnectionTrace): void {
  activeTraces.delete(trace);
  if (typeof globalThis === 'undefined') {
    return;
  }
  const host = globalThis as any;
  if (host[TRACE_CURRENT_KEY] === trace) {
    const next = Array.from(activeTraces).pop() ?? null;
    host[TRACE_CURRENT_KEY] = next;
  }
}

function recordSnapshot(snapshot: ConnectionTraceSnapshot): void {
  if (typeof globalThis === 'undefined') {
    return;
  }
  const host = globalThis as any;
  const history: ConnectionTraceSnapshot[] = Array.isArray(host[TRACE_HISTORY_KEY])
    ? host[TRACE_HISTORY_KEY]
    : (host[TRACE_HISTORY_KEY] = []);
  history.push(snapshot);
  while (history.length > TRACE_HISTORY_LIMIT) {
    history.shift();
  }
  host[TRACE_LAST_KEY] = snapshot;
}

function isTraceEnabled(): boolean {
  if (typeof globalThis === 'undefined') {
    return false;
  }
  const globalFlag =
    (globalThis as any).__BEACH_TRACE ??
    (globalThis as any).BEACH_TRACE ??
    (typeof window !== 'undefined' ? (window as any).__BEACH_TRACE ?? (window as any).BEACH_TRACE : undefined);
  return Boolean(globalFlag);
}

function now(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}
